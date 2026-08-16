"""``/v1/collections`` —— コレクションの作成と一覧。

**削除の経路は無い。** 作ったコレクションを消すには運用へ依頼する
（``docs/runbook/collection.md``）—— 誤って消すと再取り込みに何時間も
掛かるためだが、**この判断はどの ADR にも書かれていない。**
"""

from __future__ import annotations

from kotonoha.api.errors import guard
from kotonoha.api.schemas import CreateCollectionBody, collection_to_dict
from kotonoha.framework.routing import Request, Response, Router


def register(collection_service) -> Router:
    """依存を挿して経路を組み立てる。"""
    router = Router(prefix="/v1")

    @router.post("/collections", summary="コレクションを作る")
    @guard
    def create_collection(request: Request) -> Response:
        """作る。機密区分はテナントから継承する（下げられない）。"""
        body = CreateCollectionBody(**request.body)
        collection = collection_service.create(
            request.tenant_id, body.name,
            classification=body.classification,
            embed_model=body.model,
        )
        return Response(status=201, body=collection_to_dict(collection))

    @router.get("/collections", summary="コレクションの一覧")
    @guard
    def list_collections(request: Request) -> Response:
        """自分のテナントのものだけ返す。"""
        items = collection_service.list_for(request.tenant_id)
        return Response(status=200, body={
            "object": "list",
            "data": [collection_to_dict(c) for c in items],
        })

    @router.get("/collections/{collection_id}", summary="コレクションを引く")
    @guard
    def get_collection(request: Request) -> Response:
        """1 つ返す。**他テナントのものは 404。**"""
        collection = collection_service.get(
            request.path_params["collection_id"], tenant_id=request.tenant_id)
        return Response(status=200, body=collection_to_dict(collection))

    return router
