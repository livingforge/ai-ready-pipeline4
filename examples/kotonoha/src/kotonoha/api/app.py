"""経路をまとめる。``demo.wiring`` がここを呼ぶ。

**middleware の順は意味を持つ。** 計時 → 認証 → レート制限。
認証の前にレート制限を掛けるとテナントが分からず均せない。
"""

from __future__ import annotations

from kotonoha import __version__
from kotonoha.api import collections, documents, embeddings, jobs, search, usage
from kotonoha.api.auth import AuthMiddleware
from kotonoha.api.middleware import RateLimitMiddleware, TimingMiddleware
from kotonoha.framework.routing import App, Request, Response, Router

TITLE = "Kotonoha 社内エンベディング基盤 API"


def build_app(services) -> App:
    """``demo.wiring.Services`` から組み立てる。"""
    app = App(title=TITLE, version=__version__)

    # 順に呼ばれる。**この順を変えない。**
    app.use(TimingMiddleware())
    app.use(AuthMiddleware(services.apikeys, services.tenants))
    app.use(RateLimitMiddleware(services.limiter))

    app.include(_health_router())
    app.include(embeddings.register(services.embed, services.tenants,
                                    services.meter))
    app.include(collections.register(services.collections))
    app.include(documents.register(services.ingest, services.documents,
                                   services.chunks, services.embeddings,
                                   services.collections))
    app.include(search.register(services.search, services.meter))
    app.include(jobs.register(services.ingest, services.tracker))
    app.include(usage.register(services.tenants, services.quota, services.meter))
    return app


def _health_router() -> Router:
    """死活監視。**認証を要さない**（``auth.PUBLIC_PATHS``）。"""
    router = Router()

    @router.get("/healthz", summary="生きているか")
    def healthz(request: Request) -> Response:
        return Response(status=200, body={"status": "ok", "version": __version__})

    @router.get("/readyz", summary="受け付けられるか")
    def readyz(request: Request) -> Response:
        """**依存の疎通は見ていない。** 本番では pgvector と
        OpenSearch を叩く必要があるが、入れていない ——
        ``docs/runbook/oncall.md`` に既知の穴として書いてある。
        """
        return Response(status=200, body={"status": "ready"})

    return router
