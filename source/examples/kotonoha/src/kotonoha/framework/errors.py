"""HTTP のエラー。``fastapi.HTTPException`` に相当する。"""

from __future__ import annotations


class HttpError(Exception):
    """HTTP のステータスを持つ例外。

    :param status: HTTP ステータスコード
    :param code: 機械が読む短い符号（``quota_exceeded`` など）
    :param message: 人が読む説明。**機密の本文を入れてはならない**
    """

    def __init__(self, status: int, code: str, message: str,
                 detail: dict | None = None) -> None:
        super().__init__(f"{status} {code}: {message}")
        self.status = status
        self.code = code
        self.message = message
        self.detail = detail or {}

    def to_body(self) -> dict:
        """レスポンスの本文にする。"""
        body = {"error": {"code": self.code, "message": self.message}}
        if self.detail:
            body["error"]["detail"] = self.detail
        return body


def bad_request(code: str, message: str, **detail) -> HttpError:
    return HttpError(400, code, message, detail or None)


def unauthorized(message: str = "API キーが不正です") -> HttpError:
    return HttpError(401, "unauthorized", message)


def forbidden(code: str, message: str, **detail) -> HttpError:
    return HttpError(403, code, message, detail or None)


def not_found(code: str, message: str, **detail) -> HttpError:
    return HttpError(404, code, message, detail or None)


def too_many_requests(code: str, message: str, **detail) -> HttpError:
    return HttpError(429, code, message, detail or None)
