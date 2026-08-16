"""利用量と請求の値。``t_usage_daily`` ``t_usage_monthly`` ``m_price`` と対。"""

from __future__ import annotations

from dataclasses import dataclass, field
from datetime import date, datetime
from decimal import Decimal


@dataclass
class DailyUsage:
    """1 テナント 1 日ぶん。"""

    tenant_id: str
    usage_date: date
    embed_chunks: int = 0
    cached_chunks: int = 0
    search_calls: int = 0
    rerank_calls: int = 0
    gpu_seconds: int = 0

    def add(self, other: "DailyUsage") -> None:
        self.embed_chunks += other.embed_chunks
        self.cached_chunks += other.cached_chunks
        self.search_calls += other.search_calls
        self.rerank_calls += other.rerank_calls
        self.gpu_seconds += other.gpu_seconds


@dataclass
class MonthlyUsage:
    """1 テナント 1 か月ぶん。締めるとここへ確定する。"""

    tenant_id: str
    year_month: str
    embed_chunks: int = 0
    cached_chunks: int = 0
    search_calls: int = 0
    rerank_calls: int = 0
    gpu_seconds: int = 0
    amount_yen: int = 0
    cost_center: str = ""
    closed_at: datetime | None = None

    @property
    def closed(self) -> bool:
        return self.closed_at is not None

    @property
    def cache_hit_ratio(self) -> float:
        total = self.embed_chunks + self.cached_chunks
        return self.cached_chunks / total if total else 0.0


@dataclass(frozen=True)
class Price:
    """単価 1 行。``m_price`` と対。"""

    price_kind: str
    valid_from: date
    valid_to: date
    unit_price: Decimal
    note: str = ""

    def applies_on(self, when: date) -> bool:
        return self.valid_from <= when <= self.valid_to


@dataclass
class Invoice:
    """1 テナントぶんの請求内訳。経理へ渡す形。"""

    tenant_id: str
    year_month: str
    cost_center: str
    lines: list[tuple[str, int, Decimal, int]] = field(default_factory=list)
    # (品目, 数量, 単価, 金額円)

    @property
    def total_yen(self) -> int:
        return sum(line[3] for line in self.lines)
