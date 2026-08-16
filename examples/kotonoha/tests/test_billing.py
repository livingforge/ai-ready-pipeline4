"""利用量の計測・按分・締め・書き出し。

★ **「キャッシュに当たった分は数えない」も「端数は基盤持ち」も、
   コードと runbook にしかない**（README の仕込み A）。
"""

from __future__ import annotations

from datetime import date
from decimal import Decimal

import pytest
from conftest import make_source

from kotonoha.billing import allocate, export
from kotonoha.billing import price as pricelib
from kotonoha.billing.models import Invoice, MonthlyUsage
from kotonoha.common.clock import year_month
from kotonoha.common.errors import InvalidInput, NotFound


# ── 計測 ─────────────────────────────────────────────────────────
def test_埋め込みが数えられる(services):
    services.meter.record_embed("cs-support", billed=10, cached=3)
    totals = services.meter.month_to_date("cs-support", year_month())
    assert totals.embed_chunks == 10
    assert totals.cached_chunks == 3


def test_課金対象は投げた分だけ(services):
    """**キャッシュに当たった分は上限にも課金にも数えない。**"""
    services.meter.record_embed("cs-support", billed=10, cached=90)
    tenant = services.tenants.get("cs-support")
    assert services.quota.status(tenant).used == 10


def test_検索が数えられる(services):
    services.meter.record_search("cs-support")
    services.meter.record_search("cs-support", reranked=True)
    totals = services.meter.month_to_date("cs-support", year_month())
    assert totals.search_calls == 2
    assert totals.rerank_calls == 1


def test_取り込みが利用量に乗る(services, manual_collection):
    services.ingest.submit("cs-support", manual_collection.collection_id,
                           [make_source("点検の手順。" * 100)])
    services.worker.drain()
    assert services.meter.month_to_date(
        "cs-support", year_month()).embed_chunks > 0


# ── 単価 ─────────────────────────────────────────────────────────
def test_単価を引ける(services):
    unit = services.prices.unit_price("embed_chunk", date(2026, 8, 1))
    assert unit == Decimal("0.0180")


def test_期間外の単価は無い(services):
    with pytest.raises(NotFound):
        services.prices.unit_price("embed_chunk", date(2020, 1, 1))


def test_4種類の単価が揃っている(services):
    prices = services.prices.all_on(date(2026, 8, 1))
    assert set(prices) == set(pricelib.KINDS)


# ── 按分 ─────────────────────────────────────────────────────────
def test_円未満は切り捨て():
    assert allocate.line_amount(10, Decimal("0.0180")) == 0
    assert allocate.line_amount(100, Decimal("0.0180")) == 1
    assert allocate.line_amount(1000, Decimal("0.0180")) == 18


def test_数量0は0円():
    assert allocate.line_amount(0, Decimal("1.5")) == 0


def test_使っていない品目は明細に出ない():
    usage = MonthlyUsage(tenant_id="t", year_month="202608", embed_chunks=1000)
    invoice = allocate.build_invoice(
        usage, {k: Decimal("0.02") for k in pricelib.KINDS}, "CC-1")
    assert [line[0] for line in invoice.lines] == [
        pricelib.LABELS[pricelib.EMBED_CHUNK]]


def test_端数は基盤が持つ():
    """★ この決めごとは runbook にしかない。"""
    invoice = Invoice(tenant_id="t", year_month="202608", cost_center="CC-1",
                      lines=[("品目", 100, Decimal("0.018"), 1)])
    assert allocate.residual([invoice], actual_total_yen=5) == 4


def test_取りすぎは負になる():
    invoice = Invoice(tenant_id="t", year_month="202608", cost_center="CC-1",
                      lines=[("品目", 1000, Decimal("0.018"), 18)])
    assert allocate.residual([invoice], actual_total_yen=10) == -8


def test_年月から月初が出る():
    assert allocate.month_start("202608") == date(2026, 8, 1)


# ── 締め ─────────────────────────────────────────────────────────
def test_締められる(services):
    services.meter.record_embed("cs-support", billed=10_000, cached=0)
    result = services.close.run(year_month())
    assert result.closed
    assert result.total_yen > 0


def test_締めた月は二度締めない(services):
    """**二重に請求しない。**"""
    services.meter.record_embed("cs-support", billed=10_000, cached=0)
    services.close.run(year_month())
    second = services.close.run(year_month())
    assert "cs-support" in second.skipped


def test_年月の形が不正だと弾かれる(services):
    with pytest.raises(InvalidInput):
        services.close.run("2026-08")


def test_締めを取り消せる(services):
    services.meter.record_embed("cs-support", billed=10_000, cached=0)
    services.close.run(year_month())
    reopened = services.close.reopen("cs-support", year_month())
    assert not reopened.closed


def test_締めていないものは取り消せない(services):
    with pytest.raises(InvalidInput):
        services.close.reopen("cs-support", year_month())


def test_原価センタが載る(services):
    services.meter.record_embed("legal-contract", billed=10_000, cached=0)
    result = services.close.run(year_month())
    legal = [i for i in result.invoices if i.tenant_id == "legal-contract"][0]
    assert legal.cost_center == "CC-1150"


# ── 書き出し ─────────────────────────────────────────────────────
def test_CSVの列が固定():
    """**会計システムが読む様式。列の順も名前も変えられない。**"""
    text = export.to_csv([])
    assert text.split("\r\n")[0] == ",".join(export.HEADER)


def test_CSVに明細が並ぶ():
    invoice = Invoice(tenant_id="t1", year_month="202608", cost_center="CC-1",
                      lines=[("エンベディング（チャンク）", 1000, Decimal("0.018"), 18)])
    text = export.to_csv([invoice])
    assert "t1" in text and "CC-1" in text and "18" in text


def test_Shift_JISで書き出せる():
    invoice = Invoice(tenant_id="t1", year_month="202608", cost_center="CC-1",
                      lines=[("エンベディング（チャンク）", 1000, Decimal("0.018"), 18)])
    assert export.to_bytes([invoice]).decode("cp932")


def test_渡せない文字は弾かれる():
    """**静かに化けたまま渡すと原価センタが取り違えられる。**"""
    invoice = Invoice(tenant_id="t1", year_month="202608", cost_center="CC-1",
                      lines=[("𠮟責", 1, Decimal("1"), 1)])
    with pytest.raises(InvalidInput):
        export.to_bytes([invoice])


def test_原価センタごとに集計できる():
    invoices = [
        Invoice(tenant_id="a", year_month="202608", cost_center="CC-1",
                lines=[("x", 1, Decimal("1"), 10)]),
        Invoice(tenant_id="b", year_month="202608", cost_center="CC-1",
                lines=[("x", 1, Decimal("1"), 5)]),
        Invoice(tenant_id="c", year_month="202608", cost_center="CC-2",
                lines=[("x", 1, Decimal("1"), 3)]),
    ]
    assert export.summary_rows(invoices) == [("CC-1", 15), ("CC-2", 3)]
