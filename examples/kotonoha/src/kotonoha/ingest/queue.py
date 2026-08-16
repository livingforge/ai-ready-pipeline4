"""取り込みの待ち行列。本番は Redis の List、ここはメモリ。

**先入れ先出しだが、テナントで均す。** 品質保証部が 200 万件を投げると
後ろのカスタマーサポート部が何時間も待たされるので、テナントごとの列を
順に回す（ラウンドロビン）。

滞留の見張りは SLO にある（キュー滞留 30 分以内）。深さが閾値を超えたら
アラートが出て、ワーカを増やす手順が ``docs/runbook/ingest.md`` にある。
"""

from __future__ import annotations

from collections import deque
from dataclasses import dataclass
from typing import Protocol

from kotonoha.common import logging as applog
from kotonoha.common.clock import now

log = applog.get(__name__)

#: これを超えたら警告。SLO の滞留 30 分に相当する目安。
WARN_DEPTH = 500


@dataclass
class QueuedJob:
    """待ち行列に入っている 1 件。"""

    job_id: str
    tenant_id: str
    collection_id: str
    enqueued_at: object = None

    def waited_seconds(self) -> float:
        if self.enqueued_at is None:
            return 0.0
        return max(0.0, (now() - self.enqueued_at).total_seconds())


class JobQueue(Protocol):
    """待ち行列。"""

    def push(self, item: QueuedJob) -> None: ...
    def pop(self) -> QueuedJob | None: ...
    def depth(self) -> int: ...


class FairQueue:
    """テナントで均す待ち行列。"""

    def __init__(self) -> None:
        self._lanes: dict[str, deque[QueuedJob]] = {}
        self._order: deque[str] = deque()

    def push(self, item: QueuedJob) -> None:
        item.enqueued_at = item.enqueued_at or now()
        lane = self._lanes.get(item.tenant_id)
        if lane is None:
            lane = deque()
            self._lanes[item.tenant_id] = lane
            self._order.append(item.tenant_id)
        lane.append(item)
        if self.depth() > WARN_DEPTH:
            log.warning("取り込みの待ちが増えています depth=%d", self.depth())

    def pop(self) -> QueuedJob | None:
        """次の 1 件。テナントを順に回す。"""
        for _ in range(len(self._order)):
            tenant_id = self._order[0]
            self._order.rotate(-1)
            lane = self._lanes.get(tenant_id)
            if lane:
                return lane.popleft()
        return None

    def depth(self) -> int:
        return sum(len(lane) for lane in self._lanes.values())

    def depth_of(self, tenant_id: str) -> int:
        return len(self._lanes.get(tenant_id, ()))

    def oldest_wait_seconds(self) -> float:
        """いちばん長く待っている件の待ち時間。滞留の監視が読む。"""
        waits = [item.waited_seconds()
                 for lane in self._lanes.values() for item in lane]
        return max(waits, default=0.0)
