"""業務の例外を HTTP のステータスへ翻訳する。

**この対応表が唯一の翻訳点である。** 業務層は HTTP を知らないので、
ここを見れば「どの失敗が何番で返るか」が分かる。

★ この表は ``openapi.yaml`` の ``responses`` と対だが、**同期する仕組みが
  無い。** 片方を直しても片方は直らない。
"""

from __future__ import annotations

from kotonoha.common import errors as biz
from kotonoha.framework.errors import HttpError
from kotonoha.framework.schema import ValidationError

#: 業務の例外 → HTTP ステータス。
STATUS = {
    biz.NotFound: 404,
    biz.AlreadyExists: 409,
    biz.InvalidInput: 400,
    biz.QuotaExceeded: 429,
    biz.ClassificationViolation: 403,
    biz.ProviderError: 502,
    biz.IndexBusy: 409,
}


def to_http(exc: Exception) -> HttpError:
    """業務の例外を HTTP の例外へ。知らないものは 500。"""
    if isinstance(exc, HttpError):
        return exc
    if isinstance(exc, ValidationError):
        return HttpError(400, "invalid_request", "入力が不正です",
                         {"fields": [{"field": f, "reason": m}
                                     for f, m in exc.errors]})
    for kind, status in STATUS.items():
        if isinstance(exc, kind):
            return HttpError(status, exc.code, exc.message, exc.detail or None)
    # **中身を漏らさない。** 詳細はログにだけ残す。
    return HttpError(500, "internal_error", "内部エラーが発生しました")


def guard(handler):
    """ハンドラを包んで例外を翻訳する。"""

    def wrapped(request):
        try:
            return handler(request)
        except Exception as exc:
            raise to_http(exc) from exc

    wrapped.__name__ = handler.__name__
    wrapped.__doc__ = handler.__doc__
    return wrapped
