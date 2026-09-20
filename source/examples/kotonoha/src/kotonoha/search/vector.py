"""ベクトル検索。pgvector の HNSW 索引を引く。

**検索語も同じモデルで埋め込む。** コレクションを作ったときのモデルで
埋めないとベクトル空間が違うので、まったく当たらない ——
``collection.embed_model`` を必ず使う。

``ef_search`` は再現率と速さの釣り合いを決める。既定は 100 で、
これは **SLO の p95 800ms から逆算した値**だが、その計算は
``docs/runbook/search-tuning.md`` にしか書かれていない。
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Protocol

from kotonoha.embed.models import Vector
from kotonoha.search.fusion import Ranked, to_ranked

#: HNSW の探索幅。大きいほど当たるが遅い。
DEFAULT_EF_SEARCH = 100

#: 融合へ渡す候補数。top_k より多めに取る。
CANDIDATE_MULTIPLIER = 5
MIN_CANDIDATES = 50


@dataclass
class VectorHit:
    chunk_id: str
    similarity: float


class VectorStore(Protocol):
    """ベクトルの置き場。``t_embedding`` / pgvector。"""

    def search(self, index_name: str, vector: Vector, limit: int,
               chunk_ids: list[str] | None = None,
               ef_search: int = DEFAULT_EF_SEARCH) -> list[VectorHit]: ...


class VectorSearch:
    """ベクトルで引く。"""

    def __init__(self, store: VectorStore, embed_service) -> None:
        self._store = store
        self._embed = embed_service

    def search(self, text: str, collection, top_k: int, *,
               chunk_ids: list[str] | None = None,
               ef_search: int = DEFAULT_EF_SEARCH) -> list[Ranked]:
        """検索語をベクトルにして引き、順位の列を返す。

        :param chunk_ids: 事前に絞り込んだ範囲。``None`` なら全体
        """
        vector = self._embed.embed_query(
            text, collection.embed_model, collection.classification,
            tenant_id=collection.tenant_id)
        limit = candidate_count(top_k)
        hits = self._store.search(collection.index_alias, vector, limit,
                                  chunk_ids=chunk_ids, ef_search=ef_search)
        return to_ranked([h.chunk_id for h in hits], [h.similarity for h in hits])


def candidate_count(top_k: int) -> int:
    """融合へ渡す候補数。**top_k より多く取る。**

    片方の検索でしか上位に来ない結果を拾うため。少なく取ると融合の
    意味が薄れ、多く取ると遅くなる。
    """
    return max(MIN_CANDIDATES, top_k * CANDIDATE_MULTIPLIER)
