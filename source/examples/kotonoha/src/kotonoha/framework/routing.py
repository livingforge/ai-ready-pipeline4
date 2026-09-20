"""ルーティングと HTTP の受け口。``fastapi.APIRouter`` / ``FastAPI`` に相当する。

パスの ``{name}`` を取り出して引数に渡すところまでで、依存性注入や
バックグラウンドタスクは持たない —— API の形を見せるのが目的なので、
そこまで真似ても資材として意味がない。
"""

from __future__ import annotations

import json
import re
from dataclasses import dataclass, field
from typing import Any, Callable

from kotonoha.framework.errors import HttpError

_PARAM = re.compile(r"\{([a-zA-Z_][a-zA-Z0-9_]*)\}")


@dataclass
class Request:
    """受け取った要求。"""

    method: str
    path: str
    headers: dict[str, str] = field(default_factory=dict)
    query: dict[str, str] = field(default_factory=dict)
    body: dict = field(default_factory=dict)
    path_params: dict[str, str] = field(default_factory=dict)
    #: 認証で解決したテナント。``api.auth`` が入れる。
    tenant_id: str | None = None
    key_id: str | None = None

    def header(self, name: str, default: str = "") -> str:
        return self.headers.get(name.lower(), default)


@dataclass
class Response:
    """返す応答。"""

    status: int = 200
    body: Any = None
    headers: dict[str, str] = field(default_factory=dict)

    def to_json(self) -> str:
        return json.dumps(self.body, ensure_ascii=False, default=str)


@dataclass
class Route:
    method: str
    pattern: re.Pattern
    template: str
    handler: Callable
    name: str
    summary: str


class Router:
    """1 つのまとまりぶんの経路。"""

    def __init__(self, prefix: str = "") -> None:
        self.prefix = prefix.rstrip("/")
        self.routes: list[Route] = []

    def route(self, method: str, path: str, *, summary: str = "") -> Callable:
        def decorate(handler: Callable) -> Callable:
            template = self.prefix + path
            regex = "^" + _PARAM.sub(r"(?P<\1>[^/]+)", template) + "$"
            self.routes.append(Route(
                method=method.upper(),
                pattern=re.compile(regex),
                template=template,
                handler=handler,
                name=handler.__name__,
                summary=summary or (handler.__doc__ or "").strip().split("\n")[0],
            ))
            return handler
        return decorate

    def get(self, path: str, **kw) -> Callable:
        return self.route("GET", path, **kw)

    def post(self, path: str, **kw) -> Callable:
        return self.route("POST", path, **kw)

    def delete(self, path: str, **kw) -> Callable:
        return self.route("DELETE", path, **kw)


class App:
    """まとめて受ける入口。``middleware`` は前から順に呼ぶ。"""

    def __init__(self, title: str, version: str) -> None:
        self.title = title
        self.version = version
        self.routes: list[Route] = []
        self.middleware: list[Callable] = []

    def include(self, router: Router) -> None:
        self.routes.extend(router.routes)

    def use(self, hook: Callable) -> None:
        """要求ごとに前処理を挟む（認証・監査・レート制限）。"""
        self.middleware.append(hook)

    def handle(self, request: Request) -> Response:
        """1 要求を処理する。**例外は必ずここで応答へ変える。**"""
        try:
            for hook in self.middleware:
                hook(request)
            route = self._match(request)
            result = route.handler(request)
            if isinstance(result, Response):
                return result
            return Response(status=200, body=result)
        except HttpError as exc:
            return Response(status=exc.status, body=exc.to_body())

    def _match(self, request: Request) -> Route:
        allowed = False
        for route in self.routes:
            matched = route.pattern.match(request.path)
            if not matched:
                continue
            if route.method != request.method:
                allowed = True
                continue
            request.path_params = matched.groupdict()
            return route
        if allowed:
            raise HttpError(405, "method_not_allowed", f"{request.method} は使えません")
        raise HttpError(404, "not_found", f"{request.path} はありません")

    def describe(self) -> list[dict]:
        """経路の一覧。``demo.cli routes`` が使う。"""
        return [
            {"method": r.method, "path": r.template, "name": r.name, "summary": r.summary}
            for r in sorted(self.routes, key=lambda r: (r.template, r.method))
        ]
