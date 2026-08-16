"""``GET /v1/usage`` —— 当月の利用量と上限。

★ **金額は返さない。** 各部門が自分の請求額を API から取れると、
  締める前の暫定値が独り歩きする —— 経理と揉めたことがあり、
  返さない形にした。この経緯は ``docs/runbook/billing.md`` にしかない。
"""

from __future__ import annotations

from kotonoha.api.errors import guard
from kotonoha.common.clock import year_month
from kotonoha.framework.routing import Request, Response, Router


def register(tenant_service, quota_checker, meter) -> Router:
    """依存を挿して経路を組み立てる。"""
    router = Router(prefix="/v1")

    @router.get("/usage", summary="当月の利用量と上限を引く")
    @guard
    def get_usage(request: Request) -> Response:
        """自分のテナントのぶんだけ返す。**金額は含まない。**"""
        tenant = tenant_service.get(request.tenant_id)
        month = request.query.get("month") or year_month()
        status = quota_checker.status(tenant, month=month)
        totals = meter.month_to_date(tenant.tenant_id, month)

        return Response(status=200, body={
            "tenant_id": tenant.tenant_id,
            "year_month": month,
            "quota": {
                "monthly_chunks": status.quota,
                "used": status.used,
                "remaining": status.remaining,
                "ratio": round(status.ratio, 4),
                "warning": status.warning,
            },
            "usage": {
                "embed_chunks": totals.embed_chunks,
                "cached_chunks": totals.cached_chunks,
                "search_calls": totals.search_calls,
                "rerank_calls": totals.rerank_calls,
                "gpu_seconds": totals.gpu_seconds,
            },
        })

    return router
