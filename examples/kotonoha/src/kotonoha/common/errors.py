"""業務の例外。HTTP へは ``api.errors`` が翻訳する。

**業務層は HTTP を知らない。** ここの例外は「何が起きたか」だけを言い、
どのステータスで返すかは API 層が決める。
"""

from __future__ import annotations


class KotonohaError(Exception):
    """基底。``code`` は機械が読む短い符号。"""

    code = "internal_error"

    def __init__(self, message: str, **detail) -> None:
        super().__init__(message)
        self.message = message
        self.detail = detail


class NotFound(KotonohaError):
    """指したものが無い。"""

    code = "not_found"


class AlreadyExists(KotonohaError):
    """同じ名前のものが既にある。"""

    code = "already_exists"


class InvalidInput(KotonohaError):
    """入力が業務上の規則に合わない。"""

    code = "invalid_input"


class QuotaExceeded(KotonohaError):
    """月間の上限を超えた。**検索は止めない**（取り込みだけ止める）。"""

    code = "quota_exceeded"


class ClassificationViolation(KotonohaError):
    """機密区分の規則に反する操作。

    極秘を外部 API へ出そうとした、コレクションの区分を下げようとした、
    などがこれ。**握りつぶしてはならない** —— 監査ログに残して上へ投げる。
    """

    code = "classification_violation"


class ProviderError(KotonohaError):
    """埋め込みの提供元が失敗した。再試行の対象。"""

    code = "provider_error"


class IndexBusy(KotonohaError):
    """再インデックス中で受け付けられない。"""

    code = "index_busy"
