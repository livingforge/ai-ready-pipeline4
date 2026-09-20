"""全文検索（BM25）。OpenSearch を引く。

日本語は kuromoji で分かち書きする。**型番と品番はベクトル検索では
まったく当たらない**ので、この経路が要る ——「A-2210-B」のような文字列は
意味を持たないベクトルになり、コサイン類似度で上位に来ない。
ハイブリッドにした理由の 8 割はこれである（``docs/adr/0006-hybrid-search.md``）。
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Protocol

from kotonoha.search import query as querylib
from kotonoha.search.fusion import Ranked, to_ranked
from kotonoha.search.vector import candidate_count

#: BM25 の係数。OpenSearch の既定値をそのまま使っている。
#: **調整していない** —— 調整する根拠になる評価データが無い。
BM25_K1 = 1.2
BM25_B = 0.75


@dataclass
class KeywordHit:
    chunk_id: str
    score: float


class KeywordStore(Protocol):
    """全文の置き場。OpenSearch。"""

    def search(self, index_name: str, terms: list[str], limit: int,
               chunk_ids: list[str] | None = None,
               phrase: str | None = None) -> list[KeywordHit]: ...


class KeywordSearch:
    """語で引く。"""

    def __init__(self, store: KeywordStore) -> None:
        self._store = store

    def search(self, text: str, collection, top_k: int, *,
               chunk_ids: list[str] | None = None) -> list[Ranked]:
        """語に割って引き、順位の列を返す。

        引用符で囲まれていれば完全一致で引く（``"A-2210-B"``）。
        """
        phrase = querylib.strip_quotes(text) if querylib.is_phrase(text) else None
        words = querylib.terms(querylib.strip_quotes(text))
        if not words and not phrase:
            return []
        hits = self._store.search(collection.index_alias, words,
                                  candidate_count(top_k),
                                  chunk_ids=chunk_ids, phrase=phrase)
        return to_ranked([h.chunk_id for h in hits], [h.score for h in hits])
