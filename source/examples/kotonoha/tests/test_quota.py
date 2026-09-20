"""月間の上限。

★ **法務部の上限は台帳（100,000）と実装の既定（300,000）で食い違う。**
   この検証は「実装がどうなっているか」しか言えない
   （README の仕込み C1）。
"""

from __future__ import annotations

import pytest

from kotonoha.common.errors import QuotaExceeded
from kotonoha.tenant.quota import DEFAULT_QUOTA, WARN_RATIO, QuotaChecker


class _Usage:
    """当月の消化量を返すだけの見本。"""

    def __init__(self, used: int) -> None:
        self.used = used

    def embed_chunks_in_month(self, tenant_id, year_month):
        return self.used


def _checker(used: int) -> QuotaChecker:
    return QuotaChecker(_Usage(used))


def test_既定は30万():
    """★ 台帳の法務部は 10 万。**この差が仕込み C1。**"""
    assert DEFAULT_QUOTA == 300_000


def test_消化状況を返す(services):
    tenant = services.tenants.get("cs-support")
    status = _checker(1000).status(tenant)
    assert status.used == 1000
    assert status.quota == 500_000
    assert status.remaining == 499_000


def test_収まっていれば通る(services):
    tenant = services.tenants.get("cs-support")
    _checker(0).ensure_can_ingest(tenant, 100)


def test_超えると弾かれる(services):
    tenant = services.tenants.get("cs-support")
    with pytest.raises(QuotaExceeded):
        _checker(499_999).ensure_can_ingest(tenant, 100)


def test_ちょうど上限までは通る(services):
    tenant = services.tenants.get("cs-support")
    _checker(499_900).ensure_can_ingest(tenant, 100)


def test_1件超えると弾かれる(services):
    tenant = services.tenants.get("cs-support")
    with pytest.raises(QuotaExceeded):
        _checker(499_900).ensure_can_ingest(tenant, 101)


def test_8割で警告が立つ(services):
    tenant = services.tenants.get("cs-support")
    status = _checker(int(500_000 * WARN_RATIO)).status(tenant)
    assert status.warning
    assert not status.exceeded


def test_超えたら警告ではなく超過(services):
    tenant = services.tenants.get("cs-support")
    status = _checker(500_000).status(tenant)
    assert status.exceeded
    assert not status.warning


def test_法務部の上限は台帳の値で入っている(services):
    """★ 実運用では 2026/06 に 30 万へ直されている（台帳は未更新）。"""
    tenant = services.tenants.get("legal-contract")
    assert tenant.monthly_quota == 100_000
    assert tenant.monthly_quota != DEFAULT_QUOTA


def test_弾かれた理由に数字が入る(services):
    """運用が問い合わせを受けたときに答えられるように。"""
    tenant = services.tenants.get("cs-support")
    with pytest.raises(QuotaExceeded) as caught:
        _checker(499_999).ensure_can_ingest(tenant, 100)
    assert caught.value.detail["quota"] == 500_000
    assert caught.value.detail["used"] == 499_999
    assert caught.value.detail["additional"] == 100
