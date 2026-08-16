"""テナントの登録・承認・停止・上限変更。

**承認前のテナントは使えない。** 利用申請の受け付けは社内のワークフロー
（別システム）で行い、承認が下りたものだけをここへ入れる。
"""

from __future__ import annotations

from datetime import date

import pytest

from kotonoha.common.errors import AlreadyExists, InvalidInput, NotFound


def _register(services, tenant_id="new-team", **kw):
    defaults = dict(classification="10", cost_center="CC-9999")
    defaults.update(kw)
    return services.tenants.register(tenant_id, "新しい用途", "新規部門", **defaults)


def test_登録できる(services):
    tenant = _register(services)
    assert tenant.tenant_id == "new-team"
    assert tenant.applied_at is not None


def test_登録した時点ではまだ使えない(services):
    """**承認前は API を使えない。**"""
    tenant = _register(services)
    assert tenant.approved_at is None
    assert not tenant.active


def test_承認すると使えるようになる(services):
    _register(services)
    tenant = services.tenants.approve("new-team")
    assert tenant.approved_at is not None
    assert tenant.active


def test_識別子の形が不正だと弾かれる(services):
    with pytest.raises(InvalidInput):
        _register(services, tenant_id="ABC")           # 大文字
    with pytest.raises(InvalidInput):
        _register(services, tenant_id="1team")         # 数字始まり
    with pytest.raises(InvalidInput):
        _register(services, tenant_id="ab")            # 短すぎる


def test_知らない機密区分は弾かれる(services):
    with pytest.raises(InvalidInput):
        _register(services, classification="99")


def test_同じ識別子は登録できない(services):
    with pytest.raises(AlreadyExists):
        _register(services, tenant_id="cs-support")


def test_停止するとactiveでなくなる(services):
    """**データは消さない** —— 再開できるようにしておく。"""
    tenant = services.tenants.suspend("cs-support", "予算超過")
    assert tenant.status == "S"
    assert not tenant.active


def test_停止しても引ける(services):
    services.tenants.suspend("cs-support", "予算超過")
    assert services.tenants.get("cs-support") is not None


def test_知らないテナントは引けない(services):
    with pytest.raises(NotFound):
        services.tenants.get("nothing")


def test_上限を変えられる(services):
    """★ **利用申請台帳（Excel）は自動では直らない。**

    情報システム部へ連絡して台帳も直す必要があるが、その手順は
    どこにも書かれていない（README の仕込み C1 の原因）。
    """
    tenant = services.tenants.change_quota("legal-contract", 300_000)
    assert tenant.monthly_quota == 300_000


def test_上限は1以上(services):
    with pytest.raises(InvalidInput):
        services.tenants.change_quota("cs-support", 0)


def test_使えるテナントの一覧(services):
    """締め処理が回す対象。"""
    assert len(services.tenants.list_active()) == 4
    services.tenants.suspend("cs-support", "試験")
    assert len(services.tenants.list_active()) == 3


def test_申請日を指定できる(services):
    tenant = _register(services, applied_at=date(2026, 5, 1))
    assert tenant.applied_at == date(2026, 5, 1)


def test_台帳の4件が入っている(services):
    """``資料/利用申請台帳.xlsx`` と対。"""
    ids = sorted(t.tenant_id for t in services.tenants.list_active())
    assert ids == ["cs-support", "legal-contract", "qa-defect", "sales-proposal"]


def test_法務部だけが極秘(services):
    secret = [t.tenant_id for t in services.tenants.list_active()
              if t.classification == "30"]
    assert secret == ["legal-contract"]


def test_極秘テナントの既定モデルは社内ホスト(services):
    assert services.tenants.get("legal-contract").embed_model == "voyage-4-nano"
