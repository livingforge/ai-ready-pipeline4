"""課金側の保存先の約束。"""

from __future__ import annotations

from datetime import date
from typing import Protocol

from kotonoha.billing.models import DailyUsage, MonthlyUsage, Price


class UsageRepository(Protocol):
    """``t_usage_daily`` ``t_usage_monthly``。"""

    def find_daily(self, tenant_id: str, usage_date: date) -> DailyUsage | None: ...
    def save_daily(self, usage: DailyUsage) -> None: ...
    def list_daily_in_month(self, tenant_id: str,
                            year_month: str) -> list[DailyUsage]: ...
    def find_monthly(self, tenant_id: str, year_month: str) -> MonthlyUsage | None: ...
    def save_monthly(self, usage: MonthlyUsage) -> None: ...
    def embed_chunks_in_month(self, tenant_id: str, year_month: str) -> int:
        """当月の課金対象チャンク数。**上限判定が読む。**"""
        ...


class PriceRepository(Protocol):
    """``m_price``。"""

    def list_by_kind(self, kind: str) -> list[Price]: ...
    def save(self, price: Price) -> None: ...
    def all(self) -> list[Price]: ...
