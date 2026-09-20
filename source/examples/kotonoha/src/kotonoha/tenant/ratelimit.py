"""テナントごとの秒間要求数の制限。トークンバケツ。

**注意（runbook との食い違い）**

既定値は ``settings.rate_limit_rps`` で 100 rps。ところが
``docs/runbook/rate-limit.md`` は「Ingress とアプリの両方で 60 rps に
揃える」と書いている。アプリ側だけが 100 になっており、どちらが意図した
値なのか記録が無い（README の仕込み A3）。

Ingress 側（Kubernetes の設定）はこのリポジトリに入っていないので、
**コードだけを読んでも食い違いに気づけない** —— runbook を読んで初めて
分かる、という形にしてある。
"""

from __future__ import annotations

from dataclasses import dataclass, field

from kotonoha.common.clock import now
from kotonoha.common.errors import QuotaExceeded
from kotonoha.common.settings import SETTINGS


@dataclass
class _Bucket:
    tokens: float
    last: float


@dataclass
class RateLimiter:
    """トークンバケツ。テナントごとに 1 つ持つ。

    :param rps: 秒間の許容数。省略すると設定の値（100）
    :param burst: 瞬間的に許す数。省略すると ``rps`` と同じ
    """

    rps: int = SETTINGS.rate_limit_rps
    burst: int | None = None
    _buckets: dict[str, _Bucket] = field(default_factory=dict, repr=False)

    def __post_init__(self) -> None:
        self._capacity = float(self.burst or self.rps)

    def check(self, tenant_id: str, *, cost: int = 1) -> None:
        """1 要求ぶん消費する。

        :raises QuotaExceeded: 秒間の許容を超えた（HTTP では 429）
        """
        stamp = now().timestamp()
        bucket = self._buckets.get(tenant_id)
        if bucket is None:
            bucket = _Bucket(tokens=self._capacity, last=stamp)
            self._buckets[tenant_id] = bucket
        else:
            elapsed = max(0.0, stamp - bucket.last)
            bucket.tokens = min(self._capacity, bucket.tokens + elapsed * self.rps)
            bucket.last = stamp
        if bucket.tokens < cost:
            raise QuotaExceeded(
                f"秒間の要求数が上限（{self.rps} rps）を超えました",
                tenant_id=tenant_id, rps=self.rps,
            )
        bucket.tokens -= cost

    def reset(self, tenant_id: str | None = None) -> None:
        """溜まりを捨てる。テストと運用の手当てで使う。"""
        if tenant_id is None:
            self._buckets.clear()
        else:
            self._buckets.pop(tenant_id, None)
