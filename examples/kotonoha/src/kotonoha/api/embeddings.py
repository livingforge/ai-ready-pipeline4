"""``POST /v1/embeddings`` —— テキストをベクトルにする。

**取り込みを伴わない**素のエンベディング。各テナントが自分でベクトルを
持ちたいとき（別の検索基盤へ入れる、類似度を自前で計算する）に使う。

★ カスタマーサポート部は PoC 時代からこの経路だけを使っており、
  コレクションを作っていない。**取り込みと検索を使っていないテナントが
  いる**という事実は、稟議書の想定（基盤として使ってもらう）と食い違う
  —— 「回答生成の担当」の争点（README の仕込み D1）と地続きである。
"""

from __future__ import annotations

from kotonoha.api.errors import guard
from kotonoha.api.schemas import EmbedRequestBody
from kotonoha.embed.models import EmbedRequest
from kotonoha.framework.routing import Request, Response, Router


def register(embed_service, tenant_service, meter) -> Router:
    """依存を挿して経路を組み立てる。"""
    router = Router(prefix="/v1")

    @router.post("/embeddings", summary="テキストをベクトルにする")
    @guard
    def create_embeddings(request: Request) -> Response:
        """エンベディングを作る。

        キャッシュに当たったぶんは課金に数えない（``usage.cached`` に出る）。
        """
        body = EmbedRequestBody(**request.body)
        tenant = tenant_service.get(request.tenant_id)

        result = embed_service.embed(EmbedRequest(
            texts=body.input,
            model=body.model or tenant.embed_model,
            classification=tenant.classification,
            input_type=body.input_type,
            tenant_id=tenant.tenant_id,
        ))
        meter.record_embed(tenant.tenant_id,
                           billed=result.billed_count,
                           cached=result.cached_count)

        return Response(status=200, body={
            "object": "list",
            "model": result.model,
            "data": [
                {"index": index, "embedding": vector.values,
                 "quantized": vector.quantized}
                for index, vector in enumerate(result.vectors)
            ],
            "usage": {
                "billed": result.billed_count,
                "cached": result.cached_count,
                "elapsed_ms": result.elapsed_ms,
            },
        })

    return router
