"""``t_usage_daily`` ``t_usage_monthly`` ``m_price`` の読み書き。"""

from __future__ import annotations

from datetime import date
from decimal import Decimal

from kotonoha.billing.models import DailyUsage, MonthlyUsage, Price
from kotonoha.store.connection import Connection

_DAILY_COLUMNS = ("tenant_id, usage_date, embed_chunks, cached_chunks, "
                  "search_calls, rerank_calls, gpu_seconds")

SELECT_DAILY = (f"SELECT {_DAILY_COLUMNS} FROM t_usage_daily "
                f"WHERE tenant_id = %s AND usage_date = %s")

SELECT_DAILY_IN_MONTH = f"""
SELECT {_DAILY_COLUMNS} FROM t_usage_daily
WHERE tenant_id = %s AND TO_CHAR(usage_date, 'YYYYMM') = %s
ORDER BY usage_date
"""

UPSERT_DAILY = f"""
INSERT INTO t_usage_daily ({_DAILY_COLUMNS})
VALUES (%s, %s, %s, %s, %s, %s, %s)
ON CONFLICT (tenant_id, usage_date) DO UPDATE SET
    embed_chunks  = EXCLUDED.embed_chunks,
    cached_chunks = EXCLUDED.cached_chunks,
    search_calls  = EXCLUDED.search_calls,
    rerank_calls  = EXCLUDED.rerank_calls,
    gpu_seconds   = EXCLUDED.gpu_seconds,
    updated_at    = CURRENT_TIMESTAMP
"""

#: 上限判定。**キャッシュ分は数えない**ので ``embed_chunks`` だけを足す。
SUM_EMBED_CHUNKS = """
SELECT COALESCE(SUM(embed_chunks), 0) AS n
FROM t_usage_daily
WHERE tenant_id = %s AND TO_CHAR(usage_date, 'YYYYMM') = %s
"""

_MONTHLY_COLUMNS = ("tenant_id, year_month, embed_chunks, cached_chunks, "
                    "search_calls, rerank_calls, gpu_seconds, amount_yen, "
                    "cost_center, closed_at")

SELECT_MONTHLY = (f"SELECT {_MONTHLY_COLUMNS} FROM t_usage_monthly "
                  f"WHERE tenant_id = %s AND year_month = %s")

UPSERT_MONTHLY = f"""
INSERT INTO t_usage_monthly ({_MONTHLY_COLUMNS})
VALUES (%s, %s, %s, %s, %s, %s, %s, %s, %s, %s)
ON CONFLICT (tenant_id, year_month) DO UPDATE SET
    embed_chunks  = EXCLUDED.embed_chunks,
    cached_chunks = EXCLUDED.cached_chunks,
    search_calls  = EXCLUDED.search_calls,
    rerank_calls  = EXCLUDED.rerank_calls,
    gpu_seconds   = EXCLUDED.gpu_seconds,
    amount_yen    = EXCLUDED.amount_yen,
    closed_at     = EXCLUDED.closed_at
"""

SELECT_PRICES_BY_KIND = """
SELECT price_kind, valid_from, valid_to, unit_price, note
FROM m_price WHERE price_kind = %s ORDER BY valid_from DESC
"""

UPSERT_PRICE = """
INSERT INTO m_price (price_kind, valid_from, valid_to, unit_price, note)
VALUES (%s, %s, %s, %s, %s)
ON CONFLICT (price_kind, valid_from) DO UPDATE SET
    valid_to   = EXCLUDED.valid_to,
    unit_price = EXCLUDED.unit_price,
    note       = EXCLUDED.note
"""


class SqlUsageRepository:
    """``t_usage_daily`` ``t_usage_monthly``。"""

    def __init__(self, conn: Connection) -> None:
        self._conn = conn

    def find_daily(self, tenant_id: str, usage_date: date) -> DailyUsage | None:
        row = self._conn.fetch_one(SELECT_DAILY, (tenant_id, usage_date))
        return _to_daily(row) if row else None

    def list_daily_in_month(self, tenant_id: str,
                            year_month: str) -> list[DailyUsage]:
        rows = self._conn.fetch_all(SELECT_DAILY_IN_MONTH, (tenant_id, year_month))
        return [_to_daily(r) for r in rows]

    def save_daily(self, usage: DailyUsage) -> None:
        self._conn.execute(UPSERT_DAILY, (
            usage.tenant_id, usage.usage_date, usage.embed_chunks,
            usage.cached_chunks, usage.search_calls, usage.rerank_calls,
            usage.gpu_seconds,
        ))

    def embed_chunks_in_month(self, tenant_id: str, year_month: str) -> int:
        row = self._conn.fetch_one(SUM_EMBED_CHUNKS, (tenant_id, year_month))
        return int(row["n"]) if row else 0

    def find_monthly(self, tenant_id: str, year_month: str) -> MonthlyUsage | None:
        row = self._conn.fetch_one(SELECT_MONTHLY, (tenant_id, year_month))
        return _to_monthly(row) if row else None

    def save_monthly(self, usage: MonthlyUsage) -> None:
        self._conn.execute(UPSERT_MONTHLY, (
            usage.tenant_id, usage.year_month, usage.embed_chunks,
            usage.cached_chunks, usage.search_calls, usage.rerank_calls,
            usage.gpu_seconds, usage.amount_yen, usage.cost_center,
            usage.closed_at,
        ))


class SqlPriceRepository:
    """``m_price``。"""

    def __init__(self, conn: Connection) -> None:
        self._conn = conn

    def list_by_kind(self, kind: str) -> list[Price]:
        rows = self._conn.fetch_all(SELECT_PRICES_BY_KIND, (kind,))
        return [_to_price(r) for r in rows]

    def all(self) -> list[Price]:
        return []

    def save(self, price: Price) -> None:
        self._conn.execute(UPSERT_PRICE, (
            price.price_kind, price.valid_from, price.valid_to,
            price.unit_price, price.note,
        ))


def _to_daily(row: dict) -> DailyUsage:
    return DailyUsage(
        tenant_id=row["tenant_id"], usage_date=row["usage_date"],
        embed_chunks=row["embed_chunks"], cached_chunks=row["cached_chunks"],
        search_calls=row["search_calls"], rerank_calls=row["rerank_calls"],
        gpu_seconds=row["gpu_seconds"],
    )


def _to_monthly(row: dict) -> MonthlyUsage:
    return MonthlyUsage(
        tenant_id=row["tenant_id"], year_month=row["year_month"],
        embed_chunks=row["embed_chunks"], cached_chunks=row["cached_chunks"],
        search_calls=row["search_calls"], rerank_calls=row["rerank_calls"],
        gpu_seconds=row["gpu_seconds"], amount_yen=row["amount_yen"],
        cost_center=row["cost_center"], closed_at=row["closed_at"],
    )


def _to_price(row: dict) -> Price:
    return Price(
        price_kind=row["price_kind"], valid_from=row["valid_from"],
        valid_to=row["valid_to"], unit_price=Decimal(str(row["unit_price"])),
        note=row["note"] or "",
    )
