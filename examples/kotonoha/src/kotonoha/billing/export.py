"""経理へ渡す CSV。

社内の会計システムが読む固定の様式。**列の順も名前も変えられない**ので、
ここを直すときは経理へ連絡が要る（``docs/runbook/billing.md``）。

文字符号化は Shift_JIS ——会計システムがそれしか読めない。
**読めない文字は落とさずエラーにする** ——静かに化けたまま渡すと、
原価センタが取り違えられる。
"""

from __future__ import annotations

import csv
import io

from kotonoha.common.errors import InvalidInput
from kotonoha.billing.models import Invoice

#: 会計システムが期待する列。**この順で出す。**
HEADER = ["年月", "原価センタ", "テナント", "品目", "数量", "単価", "金額"]

#: 会計システムの符号化。
ENCODING = "cp932"


def to_csv(invoices: list[Invoice]) -> str:
    """請求内訳を CSV の文字列にする。"""
    buffer = io.StringIO(newline="")
    writer = csv.writer(buffer, lineterminator="\r\n")
    writer.writerow(HEADER)
    for invoice in sorted(invoices, key=lambda i: (i.cost_center, i.tenant_id)):
        for label, quantity, unit, amount in invoice.lines:
            writer.writerow([
                invoice.year_month, invoice.cost_center, invoice.tenant_id,
                label, quantity, f"{unit:.4f}", amount,
            ])
    return buffer.getvalue()


def to_bytes(invoices: list[Invoice]) -> bytes:
    """会計システムへ渡すバイト列。

    :raises InvalidInput: Shift_JIS で表せない文字が混ざっている
    """
    text = to_csv(invoices)
    try:
        return text.encode(ENCODING)
    except UnicodeEncodeError as exc:
        bad = text[exc.start:exc.end]
        raise InvalidInput(
            f"会計システムへ渡せない文字が含まれています: {bad!r}",
            character=bad, position=exc.start,
        ) from exc


def summary_rows(invoices: list[Invoice]) -> list[tuple[str, int]]:
    """原価センタごとの合計。突合の確認に使う。"""
    totals: dict[str, int] = {}
    for invoice in invoices:
        totals[invoice.cost_center] = totals.get(invoice.cost_center, 0) + invoice.total_yen
    return sorted(totals.items())
