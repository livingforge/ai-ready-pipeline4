"""``GET /v1/jobs/{job_id}`` —— 取り込みの進捗。

失敗した明細も返す。**どの文書が入らなかったか**が分からないと
入れ直せない。
"""

from __future__ import annotations

from kotonoha.api.errors import guard
from kotonoha.api.schemas import job_to_dict
from kotonoha.framework.routing import Request, Response, Router

#: 失敗の明細を返す上限。多いときは切って ``truncated`` を立てる。
MAX_FAILURES = 100



def register(ingest_service, tracker) -> Router:
    """依存を挿して経路を組み立てる。"""
    router = Router(prefix="/v1")

    @router.get("/jobs/{job_id}", summary="ジョブの進捗を引く")
    @guard
    def get_job(request: Request) -> Response:
        """進捗と失敗の明細。**他テナントのジョブは 404。**"""
        job = ingest_service.status(request.path_params["job_id"],
                                    request.tenant_id)
        body = job_to_dict(job)

        failures = tracker.failures(job.job_id)
        body["failures"] = [
            {"seq_no": item.seq_no, "external_id": item.external_id,
             "error": item.error_message}
            for item in failures[:MAX_FAILURES]
        ]
        body["failures_truncated"] = len(failures) > MAX_FAILURES
        return Response(status=200, body=body)

    return router
