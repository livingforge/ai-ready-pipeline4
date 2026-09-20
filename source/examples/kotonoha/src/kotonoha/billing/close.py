"""月次の締め。翌月 5 営業日目に回すバッチ。

日次の実績を月次へ畳み、単価を掛けて金額を確定する。
**締めたあとの月は書き換えない** —— 経理へ渡したあとに数字が動くと
突合が壊れる。訂正が要るときは翌月で調整する。
"""

from __future__ import annotations

from dataclasses import dataclass, field

from kotonoha.common import logging as applog
from kotonoha.common.clock import now
from kotonoha.common.errors import InvalidInput
from kotonoha.billing import allocate
from kotonoha.billing.models import Invoice, MonthlyUsage

log = applog.get(__name__)


@dataclass
class CloseResult:
    """締めた結果。"""

    year_month: str
    closed: list[MonthlyUsage] = field(default_factory=list)
    invoices: list[Invoice] = field(default_factory=list)
    skipped: list[str] = field(default_factory=list)

    @property
    def total_yen(self) -> int:
        return sum(inv.total_yen for inv in self.invoices)


class MonthlyClose:
    """締める。"""

    def __init__(self, usage_repo, tenant_service, price_book) -> None:
        self._usage = usage_repo
        self._tenants = tenant_service
        self._prices = price_book

    def run(self, year_month: str) -> CloseResult:
        """1 か月ぶん締める。

        :raises InvalidInput: ``YYYYMM`` の形でない
        """
        if len(year_month) != 6 or not year_month.isdigit():
            raise InvalidInput(f"年月は YYYYMM で指定してください: {year_month}",
                               year_month=year_month)

        when = allocate.month_start(year_month)
        prices = self._prices.all_on(when)
        result = CloseResult(year_month=year_month)

        for tenant in self._tenants.list_active():
            existing = self._usage.find_monthly(tenant.tenant_id, year_month)
            if existing is not None and existing.closed:
                # 締め済みは触らない。**二重に請求しないため。**
                result.skipped.append(tenant.tenant_id)
                continue

            monthly = self._fold(tenant, year_month)
            invoice = allocate.build_invoice(monthly, prices, tenant.cost_center)
            monthly.amount_yen = invoice.total_yen
            monthly.closed_at = now()
            self._usage.save_monthly(monthly)

            result.closed.append(monthly)
            result.invoices.append(invoice)

        log.info("月次締めを終えました %s テナント=%d 合計=%d 円 飛ばし=%d",
                 year_month, len(result.closed), result.total_yen,
                 len(result.skipped))
        return result

    def _fold(self, tenant, year_month: str) -> MonthlyUsage:
        monthly = MonthlyUsage(tenant_id=tenant.tenant_id, year_month=year_month,
                               cost_center=tenant.cost_center)
        for row in self._usage.list_daily_in_month(tenant.tenant_id, year_month):
            monthly.embed_chunks += row.embed_chunks
            monthly.cached_chunks += row.cached_chunks
            monthly.search_calls += row.search_calls
            monthly.rerank_calls += row.rerank_calls
            monthly.gpu_seconds += row.gpu_seconds
        return monthly

    def reopen(self, tenant_id: str, year_month: str) -> MonthlyUsage:
        """締めを取り消す。**経理へ渡す前にしか使えない。**

        :raises InvalidInput: 締めていない
        """
        monthly = self._usage.find_monthly(tenant_id, year_month)
        if monthly is None or not monthly.closed:
            raise InvalidInput(f"締めていません: {tenant_id} {year_month}",
                               tenant_id=tenant_id, year_month=year_month)
        monthly.closed_at = None
        self._usage.save_monthly(monthly)
        log.warning("締めを取り消しました tenant=%s %s", tenant_id, year_month)
        return monthly
