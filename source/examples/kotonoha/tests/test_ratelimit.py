"""秒間の要求数の制限。

★ **設定は 100 rps だが ``docs/runbook/rate-limit.md`` は 60 rps と
   書いている**（README の仕込み A3）。Ingress 側の設定はこの
   リポジトリに入っていないので、**コードだけを読んでも食い違いに
   気づけない。**
"""

from __future__ import annotations

from datetime import timedelta

import pytest

from kotonoha.common.clock import freeze, now
from kotonoha.common.errors import QuotaExceeded
from kotonoha.common.settings import SETTINGS
from kotonoha.tenant.ratelimit import RateLimiter


def test_設定の既定は100rps():
    """★ runbook は 60 rps。**どちらが正しいのか記録が無い。**"""
    assert SETTINGS.rate_limit_rps == 100


def test_上限までは通る():
    limiter = RateLimiter(rps=5)
    for _ in range(5):
        limiter.check("t1")


def test_超えると弾かれる():
    limiter = RateLimiter(rps=3)
    for _ in range(3):
        limiter.check("t1")
    with pytest.raises(QuotaExceeded):
        limiter.check("t1")


def test_テナントごとに独立している():
    """品質保証部が使い切っても、ほかの部門は止まらない。"""
    limiter = RateLimiter(rps=2)
    limiter.check("t1")
    limiter.check("t1")
    limiter.check("t2")           # 例外が出ないこと


def test_時間が経つと戻る():
    limiter = RateLimiter(rps=2)
    limiter.check("t1")
    limiter.check("t1")
    freeze(now() + timedelta(seconds=1))
    limiter.check("t1")           # 補充されている


def test_溜まりは上限を超えない():
    limiter = RateLimiter(rps=2)
    freeze(now() + timedelta(seconds=100))
    limiter.check("t1")
    limiter.check("t1")
    with pytest.raises(QuotaExceeded):
        limiter.check("t1")


def test_瞬間的な許容を別に決められる():
    limiter = RateLimiter(rps=2, burst=5)
    for _ in range(5):
        limiter.check("t1")


def test_重い要求は多く消費できる():
    limiter = RateLimiter(rps=5)
    limiter.check("t1", cost=5)
    with pytest.raises(QuotaExceeded):
        limiter.check("t1")


def test_溜まりを捨てられる():
    limiter = RateLimiter(rps=1)
    limiter.check("t1")
    limiter.reset("t1")
    limiter.check("t1")           # 例外が出ないこと


def test_全部捨てられる():
    limiter = RateLimiter(rps=1)
    limiter.check("t1")
    limiter.check("t2")
    limiter.reset()
    limiter.check("t1")
    limiter.check("t2")


def test_弾かれた理由にrpsが入る():
    limiter = RateLimiter(rps=1)
    limiter.check("t1")
    with pytest.raises(QuotaExceeded) as caught:
        limiter.check("t1")
    assert caught.value.detail["rps"] == 1
