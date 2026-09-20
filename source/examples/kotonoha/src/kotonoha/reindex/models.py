"""再インデックスの値。``t_reindex_job`` と対。"""

from __future__ import annotations

from dataclasses import dataclass, field
from datetime import datetime

from kotonoha.common.clock import now


@dataclass
class ReindexJob:
    """1 回ぶんの再インデックス。"""

    job_id: str
    collection_id: str
    from_model: str
    to_model: str
    from_index: str
    to_index: str
    status: str = "queued"      # queued/building/verifying/switched/failed
    total_chunks: int = 0
    done_chunks: int = 0
    switched_at: datetime | None = None
    old_dropped_at: datetime | None = None
    queued_at: datetime = field(default_factory=now)
    finished_at: datetime | None = None
    error_message: str | None = None

    @property
    def progress(self) -> float:
        return self.done_chunks / self.total_chunks if self.total_chunks else 0.0

    @property
    def switched(self) -> bool:
        return self.switched_at is not None


@dataclass
class Plan:
    """見積り。始める前に利用部門へ知らせる。"""

    collection_id: str
    from_model: str
    to_model: str
    total_chunks: int
    estimated_calls: int
    estimated_minutes: int
    #: 次元が変わるか。変わるなら索引を作り直す必要がある
    dimension_changes: bool = False
    warnings: list[str] = field(default_factory=list)


@dataclass
class Verification:
    """突合の結果。"""

    old_count: int
    new_count: int
    sampled: int
    #: 抜き取りで上位が一致した割合
    agreement: float
    passed: bool
    notes: list[str] = field(default_factory=list)
