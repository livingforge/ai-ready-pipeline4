"""埋め込みキャッシュ。

★ **「キャッシュに当たった分は課金しない」という規則はコードにしかない。**
   利用申請の説明資料は「取り込んだチャンク数で課金」としか書いていない
   （README の仕込み A）。

★ **機密区分をまたいでキャッシュを共有していない。** 鍵にモデル名が入り、
   極秘は必ず ``voyage-4-nano`` になるので結果として分かれる ——
   意図した設計だが、そう書いてある文書は無い。
"""

from __future__ import annotations

from datetime import timedelta

from kotonoha.common.clock import expires_in, freeze, now
from kotonoha.common.hashing import cache_key, normalize_for_hash
from kotonoha.demo.memory_store import MemoryCacheStore
from kotonoha.embed.cache import EmbedCache
from kotonoha.embed.models import Vector


def _cache(ttl_days: int = 30) -> EmbedCache:
    return EmbedCache(MemoryCacheStore(), ttl_days=ttl_days)


def _vector(seed: float = 0.5) -> Vector:
    return Vector(values=[seed, seed], model="voyage-4")


def test_鍵は正規化した本文とモデルから作る():
    assert cache_key("点検 の 手順", "voyage-4") == cache_key("点検  の  手順", "voyage-4")


def test_モデルが違えば鍵が違う():
    assert cache_key("点検", "voyage-4") != cache_key("点検", "voyage-4-nano")


def test_機密区分をまたがない():
    """極秘は必ず voyage-4-nano なので、鍵が別になる。"""
    assert cache_key("契約書の条項", "voyage-4") != \
        cache_key("契約書の条項", "voyage-4-nano")


def test_全角と半角が同じ鍵になる():
    assert normalize_for_hash("Ａ２２１０") == "A2210"


def test_最初は全部外れる():
    lookup = _cache().lookup(["a", "b"], "voyage-4")
    assert lookup.hit_count == 0
    assert lookup.misses == [0, 1]


def test_入れると当たる():
    cache = _cache()
    cache.store(["a"], [_vector()], "voyage-4")
    lookup = cache.lookup(["a"], "voyage-4")
    assert lookup.hit_count == 1
    assert lookup.misses == []


def test_当たりと外れが混ざる():
    cache = _cache()
    cache.store(["a"], [_vector()], "voyage-4")
    lookup = cache.lookup(["a", "b", "a"], "voyage-4")
    assert sorted(lookup.hits) == [0, 2]
    assert lookup.misses == [1]


def test_件数が合わなければ入れない():
    """**壊れた対応を残さない。**"""
    cache = _cache()
    cache.store(["a", "b"], [_vector()], "voyage-4")
    assert cache.lookup(["a"], "voyage-4").hit_count == 0


def test_期限が切れると外れる():
    cache = _cache(ttl_days=30)
    cache.store(["a"], [_vector()], "voyage-4")
    freeze(now() + timedelta(days=31))
    assert cache.lookup(["a"], "voyage-4").hit_count == 0


def test_期限内は当たる():
    cache = _cache(ttl_days=30)
    cache.store(["a"], [_vector()], "voyage-4")
    freeze(now() + timedelta(days=29))
    assert cache.lookup(["a"], "voyage-4").hit_count == 1


def test_期限切れを掃除できる():
    cache = _cache(ttl_days=1)
    cache.store(["a", "b"], [_vector(), _vector(0.2)], "voyage-4")
    freeze(now() + timedelta(days=2))
    assert cache.purge() == 2


def test_保持日数の既定は30日():
    from kotonoha.common.settings import SETTINGS
    assert SETTINGS.cache_ttl_days == 30
