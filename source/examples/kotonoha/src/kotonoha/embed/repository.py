"""埋め込み側の保存先の約束。実装は ``store`` と ``demo.memory_store``。"""

from __future__ import annotations

from typing import Protocol

from kotonoha.embed.cache import CacheEntry
from kotonoha.embed.models import Vector


class EmbeddingRepository(Protocol):
    """``t_embedding``。**インデックス名で世代を分ける。**

    再インデックス中は新旧 2 つの ``index_name`` が同居するので、
    どの世代のものかを必ず指定して読み書きする。
    """

    def save(self, chunk_id: str, index_name: str, vector: Vector) -> None: ...
    def save_many(self, index_name: str, items: list[tuple[str, Vector]]) -> None: ...
    def find(self, chunk_id: str, index_name: str) -> Vector | None: ...
    def delete_by_chunk(self, chunk_id: str) -> int: ...
    def delete_index(self, index_name: str) -> int: ...
    def count_in_index(self, index_name: str) -> int: ...


class EmbedCacheRepository(Protocol):
    """``t_embed_cache``。"""

    def get(self, key: str) -> CacheEntry | None: ...
    def put(self, entry: CacheEntry) -> None: ...
    def purge_expired(self) -> int: ...
