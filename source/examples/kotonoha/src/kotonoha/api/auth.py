"""API キーからテナントを解決する。

``Authorization: Bearer kot_xxxxx`` を受ける。**失敗の理由は返さない** ——
「キーが無い」と「失効している」を区別して返すと、有効なキーの存在を
探れてしまう。
"""

from __future__ import annotations

from kotonoha.common import logging as applog
from kotonoha.framework.errors import unauthorized
from kotonoha.framework.routing import Request

log = applog.get(__name__)

#: 認証を要しない経路。
PUBLIC_PATHS = ("/healthz", "/readyz", "/v1/openapi.json")

_PREFIX = "bearer "


def extract_secret(request: Request) -> str:
    """``Authorization`` ヘッダから平文の鍵を取り出す。"""
    raw = request.header("authorization")
    if raw.lower().startswith(_PREFIX):
        return raw[len(_PREFIX):].strip()
    # 一部の社内クライアントが ``X-Api-Key`` で投げてくるので受ける。
    # **推奨しない**が、移行の途中なので残してある。
    return request.header("x-api-key").strip()


class AuthMiddleware:
    """テナントを解決して ``request`` へ載せる。"""

    def __init__(self, apikey_service, tenant_service) -> None:
        self._keys = apikey_service
        self._tenants = tenant_service

    def __call__(self, request: Request) -> None:
        if request.path in PUBLIC_PATHS:
            return
        secret = extract_secret(request)
        key = self._keys.authenticate(secret)
        if key is None:
            log.info("認証に失敗しました path=%s", request.path)
            raise unauthorized()

        tenant = self._tenants._tenants.find(key.tenant_id)
        if tenant is None or not tenant.active:
            # **キーは正しいがテナントが停止中。** 401 で返す
            # （403 だと「テナントは実在する」と分かってしまう）。
            log.info("停止中のテナントです tenant=%s", key.tenant_id)
            raise unauthorized()

        request.tenant_id = key.tenant_id
        request.key_id = key.key_id
