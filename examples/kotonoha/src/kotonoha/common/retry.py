"""再試行。外部 API と社内 GPU の呼び出しに掛ける。

指数バックオフだが**待たない**（``sleep`` を差し替えられるようにしてある）。
テストが実時間を待つのを避けるためで、本番は ``time.sleep`` が入る。
"""

from __future__ import annotations

import time
from dataclasses import dataclass
from typing import Callable, TypeVar

from kotonoha.common import logging as applog
from kotonoha.common.errors import ProviderError

log = applog.get(__name__)

T = TypeVar("T")


@dataclass
class Policy:
    """再試行の方針。"""

    attempts: int = 3
    base_seconds: float = 0.5
    max_seconds: float = 8.0

    def delay(self, attempt: int) -> float:
        """``attempt`` 回目（1 始まり）の待ち時間。"""
        return min(self.base_seconds * (2 ** (attempt - 1)), self.max_seconds)


#: 既定の方針。3 回まで、0.5 → 1.0 → 2.0 秒。
DEFAULT = Policy()


def call(fn: Callable[[], T], *, policy: Policy = DEFAULT,
         sleep: Callable[[float], None] = time.sleep,
         label: str = "") -> T:
    """``fn`` を呼び、``ProviderError`` なら方針に従って再試行する。

    :raises ProviderError: 最後の試行も失敗したとき（そのまま投げ直す）
    """
    last: ProviderError | None = None
    for attempt in range(1, policy.attempts + 1):
        try:
            return fn()
        except ProviderError as exc:
            last = exc
            if attempt == policy.attempts:
                break
            wait = policy.delay(attempt)
            log.warning("再試行します %s attempt=%d/%d wait=%.1fs reason=%s",
                        label or fn.__name__, attempt, policy.attempts, wait, exc.message)
            sleep(wait)
    assert last is not None
    raise last
