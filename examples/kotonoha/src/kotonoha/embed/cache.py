"""埋め込みキャッシュ。同一テキスト・同一モデルの再計算を避ける。

保持は 30 日（``settings.cache_ttl_days``）。鍵は正規化した本文とモデル名の
SHA-256（``common.hashing.cache_key``）。

**キャッシュに当たった分は課金に数えない。** 提供元へ投げていないので
費用が発生していない、というのが理由である（業務ルール「利用量の数え方」）。
実測では取り込みの 2 割前後がキャッシュに当たる —— 同じ通達文や定型の
前書きが複数の文書に入っているため。

★ **機密区分をまたいでキャッシュを共有していない。** 鍵にモデル名が入り、
極秘は必ず ``voyage-4-nano`` になるので、結果として区分ごとに分かれる。
これは意図した設計だが、**そう書いてある文書は無い** —— 鍵の作り方を
変えると静かに区分をまたぐので、直す人が気づけない形になっている。
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Protocol

from kotonoha.common import logging as applog
from kotonoha.common.clock import expires_in, now
from kotonoha.common.hashing import cache_key
from kotonoha.common.settings import SETTINGS
from kotonoha.embed.models import Vector

log = applog.get(__name__)


@dataclass
class CacheEntry:
    """``t_embed_cache`` の 1 行。"""

    text_hash: str
    embed_model: str
    vector: Vector
    hit_count: int = 0
    expires_at: object = None


class CacheStore(Protocol):
    """キャッシュの置き場。Redis / ``t_embed_cache`` / メモリ。"""

    def get(self, key: str) -> CacheEntry | None: ...
    def put(self, entry: CacheEntry) -> None: ...
    def purge_expired(self) -> int: ...


@dataclass
class Lookup:
    """引いた結果。当たった分と外れた分を分けて返す。"""

    hits: dict[int, Vector]        # 入力の添字 -> ベクトル
    misses: list[int]              # 外れた入力の添字

    @property
    def hit_count(self) -> int:
        return len(self.hits)


class EmbedCache:
    """引く・入れる。"""

    def __init__(self, store: CacheStore, ttl_days: int | None = None) -> None:
        self._store = store
        self._ttl_days = ttl_days or SETTINGS.cache_ttl_days

    def lookup(self, texts: list[str], model: str,
               input_type: str = "document") -> Lookup:
        """まとめて引く。当たった添字と外れた添字を返す。

        **``input_type`` を鍵に含める** —— 検索語と文書は別のベクトルに
        なるので、混ぜると取り違える。
        """
        hits: dict[int, Vector] = {}
        misses: list[int] = []
        for index, text in enumerate(texts):
            entry = self._store.get(cache_key(text, model, input_type))
            if entry is None or _expired(entry):
                misses.append(index)
            else:
                entry.hit_count += 1
                hits[index] = entry.vector
        if hits:
            log.debug("キャッシュに当たりました model=%s %d/%d",
                      model, len(hits), len(texts))
        return Lookup(hits=hits, misses=misses)

    def store(self, texts: list[str], vectors: list[Vector], model: str,
              input_type: str = "document") -> None:
        """入れる。件数が合わないときは何もしない（壊れた対応を残さない）。"""
        if len(texts) != len(vectors):
            log.warning("キャッシュに入れられません 件数不一致 %d != %d",
                        len(texts), len(vectors))
            return
        for text, vector in zip(texts, vectors):
            self._store.put(CacheEntry(
                text_hash=cache_key(text, model, input_type),
                embed_model=model,
                vector=vector,
                expires_at=expires_in(self._ttl_days),
            ))

    def purge(self) -> int:
        """期限切れを掃除する。日次のバッチが呼ぶ。"""
        removed = self._store.purge_expired()
        if removed:
            log.info("期限切れのキャッシュを削除しました %d 件", removed)
        return removed


def _expired(entry: CacheEntry) -> bool:
    return entry.expires_at is not None and entry.expires_at <= now()
