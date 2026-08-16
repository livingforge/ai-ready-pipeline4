"""検索側の保存先の約束。

ベクトルと全文は別の置き場（pgvector と OpenSearch）にある。
**両方に入れる責任は取り込み側**（``ingest.pipeline``）が持ち、
ここは引くだけ。
"""

from __future__ import annotations

from typing import Protocol

from kotonoha.embed.models import Vector
from kotonoha.search.keyword import KeywordHit
from kotonoha.search.vector import VectorHit


class VectorIndex(Protocol):
    """pgvector 側。"""

    def search(self, index_name: str, vector: Vector, limit: int,
               chunk_ids: list[str] | None = None,
               ef_search: int = 100) -> list[VectorHit]: ...
    def upsert(self, index_name: str, chunk_id: str, vector: Vector) -> None: ...
    def delete(self, index_name: str, chunk_id: str) -> None: ...


class KeywordIndex(Protocol):
    """OpenSearch 側。"""

    def search(self, index_name: str, terms: list[str], limit: int,
               chunk_ids: list[str] | None = None,
               phrase: str | None = None) -> list[KeywordHit]: ...
    def index(self, index_name: str, chunk_id: str, body: str) -> None: ...
    def delete(self, index_name: str, chunk_id: str) -> None: ...
    def alias(self, alias: str, index_name: str) -> None:
        """別名を張り替える。**再インデックスの切替がここを呼ぶ。**"""
        ...
