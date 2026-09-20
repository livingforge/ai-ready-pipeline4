"""テナントの登録・停止・照会。

利用申請の受け付けそのものは社内のワークフロー（別システム）で行い、
承認が下りたものだけをここへ入れる。**承認前のテナントは API を使えない。**
"""

from __future__ import annotations

from datetime import date

from kotonoha.common import ids
from kotonoha.common import logging as applog
from kotonoha.common.clock import today
from kotonoha.common.errors import AlreadyExists, InvalidInput, NotFound
from kotonoha.tenant import classification as cls
from kotonoha.tenant.models import Tenant
from kotonoha.tenant.quota import DEFAULT_QUOTA

log = applog.get(__name__)


class TenantService:
    """テナントの一生を見る。"""

    def __init__(self, tenant_repo) -> None:
        self._tenants = tenant_repo

    def register(self, tenant_id: str, name: str, department: str, *,
                 classification: str, cost_center: str,
                 embed_model: str = "voyage-4",
                 monthly_quota: int = DEFAULT_QUOTA,
                 applied_at: date | None = None) -> Tenant:
        """申請を登録する。**この時点ではまだ使えない**（未承認）。

        :raises InvalidInput: 識別子の形が不正／知らない機密区分
        :raises AlreadyExists: 同じ識別子が既にある
        """
        if not ids.valid_tenant_id(tenant_id):
            raise InvalidInput(f"テナント識別子の形が不正です: {tenant_id}",
                               tenant_id=tenant_id)
        cls.of(classification)          # 知らない区分ならここで落ちる
        if self._tenants.find(tenant_id) is not None:
            raise AlreadyExists(f"同じテナントがあります: {tenant_id}", tenant_id=tenant_id)

        tenant = Tenant(
            tenant_id=tenant_id,
            tenant_name=name,
            department=department,
            classification=classification,
            embed_model=embed_model,
            monthly_quota=monthly_quota,
            cost_center=cost_center,
            status="A",
            applied_at=applied_at or today(),
            approved_at=None,
        )
        self._tenants.save(tenant)
        log.info("テナントを登録しました（未承認） tenant=%s class=%s",
                 tenant_id, classification)
        return tenant

    def approve(self, tenant_id: str, *, at: date | None = None) -> Tenant:
        """承認する。ここで初めて使えるようになる。"""
        tenant = self.get(tenant_id)
        tenant.approved_at = at or today()
        self._tenants.save(tenant)
        log.info("テナントを承認しました tenant=%s", tenant_id)
        return tenant

    def suspend(self, tenant_id: str, reason: str) -> Tenant:
        """止める。**データは消さない** —— 再開できるようにしておく。"""
        tenant = self.get(tenant_id)
        tenant.status = "S"
        self._tenants.save(tenant)
        log.warning("テナントを停止しました tenant=%s reason=%s", tenant_id, reason)
        return tenant

    def change_quota(self, tenant_id: str, quota: int) -> Tenant:
        """月間上限を変える。

        ★ **利用申請台帳（Excel）は自動では直らない。** ここを変えたら
        情報システム部へ連絡して台帳も直す必要があるが、その手順は
        どこにも書かれていない（README の仕込み C1 の原因）。
        """
        if quota <= 0:
            raise InvalidInput("上限は 1 以上で指定してください", quota=quota)
        tenant = self.get(tenant_id)
        before = tenant.monthly_quota
        tenant.monthly_quota = quota
        self._tenants.save(tenant)
        log.info("月間上限を変更しました tenant=%s %d -> %d", tenant_id, before, quota)
        return tenant

    def get(self, tenant_id: str) -> Tenant:
        """引く。

        :raises NotFound: 無い
        """
        tenant = self._tenants.find(tenant_id)
        if tenant is None or tenant.status == "D":
            raise NotFound(f"テナントがありません: {tenant_id}", tenant_id=tenant_id)
        return tenant

    def list_active(self) -> list[Tenant]:
        """使えるテナントの一覧。締め処理が回す。"""
        return [t for t in self._tenants.all() if t.active]
