"""金額の按分。

**円未満は切り捨てる。** 端数は基盤側（AI基盤グループの原価センタ）が
持つ。各部門へ 1 円単位で割り振ると合計が合わなくなるためで、
これは経理と合意した扱いである（``docs/runbook/billing.md``）。

★ この「端数は基盤持ち」という決めごとは runbook にしかなく、
   **稟議書にも利用申請の説明資料にも書かれていない。**
"""

from __future__ import annotations

from datetime import date
from decimal import ROUND_FLOOR, Decimal

from kotonoha.billing import price as pricelib
from kotonoha.billing.models import Invoice, MonthlyUsage


def line_amount(quantity: int, unit_price: Decimal) -> int:
    """1 明細の金額。**円未満切り捨て。**"""
    if quantity <= 0:
        return 0
    exact = Decimal(quantity) * unit_price
    return int(exact.quantize(Decimal("1"), rounding=ROUND_FLOOR))


def build_invoice(usage: MonthlyUsage, prices: dict[str, Decimal],
                  cost_center: str) -> Invoice:
    """1 テナントぶんの請求内訳を組む。

    **数量が 0 の品目は明細に出さない** —— 使っていないものが並ぶと
    各部門が読みにくい。
    """
    invoice = Invoice(tenant_id=usage.tenant_id, year_month=usage.year_month,
                      cost_center=cost_center)
    quantities = {
        pricelib.EMBED_CHUNK: usage.embed_chunks,
        pricelib.SEARCH_CALL: usage.search_calls,
        pricelib.RERANK_CALL: usage.rerank_calls,
        pricelib.GPU_SECOND: usage.gpu_seconds,
    }
    for kind in pricelib.KINDS:
        quantity = quantities.get(kind, 0)
        if quantity <= 0:
            continue
        unit = prices[kind]
        invoice.lines.append(
            (pricelib.LABELS[kind], quantity, unit, line_amount(quantity, unit)))
    return invoice


def residual(invoices: list[Invoice], actual_total_yen: int) -> int:
    """按分後の合計と実額の差。**基盤側が持つ端数。**

    正なら基盤の持ち出し、負なら取りすぎ。取りすぎは翌月の単価で
    調整する（``docs/runbook/billing.md``）。
    """
    allocated = sum(inv.total_yen for inv in invoices)
    return actual_total_yen - allocated


def month_start(year_month: str) -> date:
    """``YYYYMM`` から月初の日付。単価を引くのに使う。"""
    return date(int(year_month[:4]), int(year_month[4:]), 1)
