"""要求ごとの前処理。レート制限と監査の下ごしらえ。

★ **レート制限の値がここで効く。** ``settings.rate_limit_rps`` は 100 だが
   ``docs/runbook/rate-limit.md`` は 60 と書いている
   （README の仕込み A3）。
"""

from __future__ import annotations

from kotonoha.api.auth import PUBLIC_PATHS
from kotonoha.common import logging as applog
from kotonoha.common.clock import now
from kotonoha.common.errors import QuotaExceeded
from kotonoha.framework.errors import too_many_requests
from kotonoha.framework.routing import Request

log = applog.get(__name__)


class RateLimitMiddleware:
    """秒間の要求数を見る。**認証のあとに置く**（テナントが要る）。"""

    def __init__(self, limiter) -> None:
        self._limiter = limiter

    def __call__(self, request: Request) -> None:
        if request.path in PUBLIC_PATHS or not request.tenant_id:
            return
        try:
            self._limiter.check(request.tenant_id)
        except QuotaExceeded as exc:
            raise too_many_requests("rate_limited", exc.message,
                                    **exc.detail) from exc


class TimingMiddleware:
    """処理時間を測る。監査ログとアクセスログが読む。"""

    def __call__(self, request: Request) -> None:
        request.headers["x-started-at"] = str(now().timestamp())


def elapsed_ms(request: Request) -> int:
    """``TimingMiddleware`` が入れた時刻からの経過。"""
    started = request.header("x-started-at")
    if not started:
        return 0
    return int((now().timestamp() - float(started)) * 1000)
