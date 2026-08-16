"""運用の操作と HTTP の待ち受け。``python -m kotonoha.demo.cli <コマンド>``

**設計文書に対応する成果物ではない。** 本番の運用操作は社内の管理画面
（別システム）から行う。

    serve     HTTP で待ち受ける（JDK 内蔵のサーバ相当。標準ライブラリのみ）
    routes    経路の一覧を出す
    reindex   再インデックスを回す
    close     月次を締める
    audit     監査ログの点検（極秘が外部へ出ていないか）
"""

from __future__ import annotations

import argparse
import json
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer

from kotonoha.common import logging as applog
from kotonoha.common.clock import year_month
from kotonoha.demo import fixtures
from kotonoha.demo.wiring import build
from kotonoha.framework.routing import Request

DEFAULT_PORT = 8080


def _handler_class(app, secrets):
    class Handler(BaseHTTPRequestHandler):
        server_version = "Kotonoha/1.4"

        def do_GET(self): self._dispatch("GET")
        def do_POST(self): self._dispatch("POST")
        def do_DELETE(self): self._dispatch("DELETE")

        def _dispatch(self, method: str) -> None:
            length = int(self.headers.get("Content-Length") or 0)
            raw = self.rfile.read(length) if length else b""
            try:
                body = json.loads(raw) if raw else {}
            except json.JSONDecodeError:
                self._send(400, {"error": {"code": "invalid_json",
                                           "message": "JSON が壊れています"}})
                return

            path, _, query = self.path.partition("?")
            request = Request(
                method=method, path=path,
                headers={k.lower(): v for k, v in self.headers.items()},
                query=dict(p.split("=", 1) for p in query.split("&") if "=" in p),
                body=body,
            )
            response = app.handle(request)
            self._send(response.status, response.body)

        def _send(self, status: int, body) -> None:
            payload = b"" if body is None else json.dumps(
                body, ensure_ascii=False, default=str).encode("utf-8")
            self.send_response(status)
            self.send_header("Content-Type", "application/json; charset=utf-8")
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            if payload:
                self.wfile.write(payload)

        def log_message(self, fmt, *args):
            return                      # アクセスログは出さない（うるさい）

    return Handler


def cmd_serve(args) -> int:
    from kotonoha.api.app import build_app
    services = build()
    secrets = fixtures.install(services)
    app = build_app(services)

    print(f"待ち受けます http://127.0.0.1:{args.port}")
    print("API キー:")
    for tenant_id, secret in secrets.items():
        print(f"  {tenant_id:16s} {secret}")
    print("\n例:")
    print(f"  curl -s http://127.0.0.1:{args.port}/healthz")
    print(f"  curl -s -X POST http://127.0.0.1:{args.port}/v1/collections \\")
    print(f"    -H 'Authorization: Bearer {secrets['cs-support']}' \\")
    print("    -H 'Content-Type: application/json' -d '{\"name\":\"manual\"}'")

    server = HTTPServer(("127.0.0.1", args.port),
                        _handler_class(app, secrets))
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        print("\n止めました。")
    return 0


def cmd_routes(args) -> int:
    from kotonoha.api.app import build_app
    app = build_app(build())
    for route in app.describe():
        print(f"  {route['method']:6s} {route['path']:48s} {route['summary']}")
    return 0


def cmd_reindex(args) -> int:
    services = build()
    fixtures.install(services)
    collection = services.collections.create(args.tenant, args.collection)
    plan = services.reindex.plan(collection.collection_id, args.to_model)
    print(f"  {plan.from_model} -> {plan.to_model}")
    print(f"  チャンク={plan.total_chunks} 見積={plan.estimated_minutes} 分")
    for warning in plan.warnings:
        print(f"  ※ {warning}")
    return 0


def cmd_close(args) -> int:
    services = build()
    fixtures.install(services)
    result = services.close.run(args.month or year_month())
    print(f"  {result.year_month} テナント={len(result.closed)} "
          f"合計={result.total_yen:,} 円")
    return 0


def cmd_audit(args) -> int:
    services = build()
    fixtures.install(services)
    violations = services.audit.violations()
    print(f"  監査ログ={len(services.audit.entries)} 件")
    print(f"  極秘が外部経路へ出た件数={len(violations)}（0 が正しい）")
    return 1 if violations else 0


def main(argv: list[str] | None = None) -> int:
    applog.setup("WARNING")
    parser = argparse.ArgumentParser(prog="kotonoha", description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)

    serve = sub.add_parser("serve", help="HTTP で待ち受ける")
    serve.add_argument("--port", type=int, default=DEFAULT_PORT)
    serve.set_defaults(func=cmd_serve)

    routes = sub.add_parser("routes", help="経路の一覧")
    routes.set_defaults(func=cmd_routes)

    reindex = sub.add_parser("reindex", help="再インデックスの見積り")
    reindex.add_argument("--tenant", default="cs-support")
    reindex.add_argument("--collection", default="manual")
    reindex.add_argument("--to-model", default="voyage-4-large")
    reindex.set_defaults(func=cmd_reindex)

    close = sub.add_parser("close", help="月次を締める")
    close.add_argument("--month", help="YYYYMM")
    close.set_defaults(func=cmd_close)

    audit = sub.add_parser("audit", help="監査ログの点検")
    audit.set_defaults(func=cmd_audit)

    args = parser.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
