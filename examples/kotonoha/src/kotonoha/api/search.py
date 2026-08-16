"""``POST /v1/collections/{id}/search`` —— 検索。

**上限に掛からない。** 取り込みだけが月間上限の対象で、検索は止めない
（業務が止まると困る、という利用申請時の合意）。
"""

from __future__ import annotations

from kotonoha.api.errors import guard
from kotonoha.api.schemas import SearchBody, hit_to_dict
from kotonoha.framework.routing import Request, Response, Router
from kotonoha.search.models import SearchQuery


def register(search_service, meter) -> Router:
    """依存を挿して経路を組み立てる。"""
    router = Router(prefix="/v1")

    @router.post("/collections/{collection_id}/search", summary="検索する")
    @guard
    def search(request: Request) -> Response:
        """ベクトルと全文を融合し、リランクして返す。"""
        body = SearchBody(**request.body)
        result = search_service.search(SearchQuery(
            text=body.query,
            collection_id=request.path_params["collection_id"],
            tenant_id=request.tenant_id,
            top_k=body.top_k,
            filters=body.filters or {},
            rerank=body.rerank,
            explain=body.explain,
        ))
        meter.record_search(request.tenant_id, reranked=result.reranked)

        return Response(status=200, body={
            "object": "list",
            "data": [hit_to_dict(hit) for hit in result.hits],
            "meta": {
                "candidates": result.total_candidates,
                "reranked": result.reranked,
                "sources": result.sources,
                "elapsed_ms": result.elapsed_ms,
            },
        })

    return router
