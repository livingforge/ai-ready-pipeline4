"""月間の利用上限。

上限は**取り込み（埋め込み）にだけ掛かる**。検索は止めない —— 業務が
止まると困る、というのが利用申請の時に各部門と決めたことである。

**注意（利用申請台帳との食い違い）**

``DEFAULT_QUOTA`` は 300,000 で、テナントに個別の値が無ければこれを使う。
法務部（legal-contract）は 2026/02 の申請時に「月 100,000」で承認されて
おり、``資料/利用申請台帳.xlsx`` にもそう書いてある。ところが 2026/06 に
契約書の一括取り込みで上限に当たり、口頭の依頼で ``t_tenant`` の値だけを
直した。**台帳は直っていない**（README の仕込み C1）。

どちらが正しいのかは、台帳を持っている情報システム部に確認が要る。
"""

from __future__ import annotations

from dataclasses import dataclass

from kotonoha.common import logging as applog
from kotonoha.common.clock import year_month
from kotonoha.common.errors import QuotaExceeded

log = applog.get(__name__)

#: テナントに個別の値が無いときの月間チャンク上限。
#: ★ 法務部の台帳は 100,000 と書いている。
DEFAULT_QUOTA = 300_000

#: この割合を超えたら警告を出す。運用が先に気づけるように。
WARN_RATIO = 0.8


@dataclass
class QuotaStatus:
    """いまの消化状況。"""

    tenant_id: str
    year_month: str
    used: int
    quota: int

    @property
    def remaining(self) -> int:
        return max(0, self.quota - self.used)

    @property
    def ratio(self) -> float:
        return self.used / self.quota if self.quota else 0.0

    @property
    def exceeded(self) -> bool:
        return self.used >= self.quota

    @property
    def warning(self) -> bool:
        return not self.exceeded and self.ratio >= WARN_RATIO


class QuotaChecker:
    """上限を見る。``store.usage_repo`` から当月の実績を引く。"""

    def __init__(self, usage_repo) -> None:
        self._usage = usage_repo

    def status(self, tenant, *, month: str | None = None) -> QuotaStatus:
        """当月の消化状況を返す。"""
        ym = month or year_month()
        used = self._usage.embed_chunks_in_month(tenant.tenant_id, ym)
        quota = tenant.monthly_quota or DEFAULT_QUOTA
        return QuotaStatus(tenant.tenant_id, ym, used, quota)

    def ensure_can_ingest(self, tenant, additional: int) -> QuotaStatus:
        """``additional`` チャンク分の取り込みを受けてよいか確かめる。

        **見積りの時点で弾く。**途中まで埋め込んでから止めると、課金だけ
        されて使えないものが残るため。

        :raises QuotaExceeded: 上限を超える
        """
        status = self.status(tenant)
        if status.used + additional > status.quota:
            raise QuotaExceeded(
                f"月間の上限を超えます（上限 {status.quota:,} / "
                f"消化 {status.used:,} / 今回 {additional:,}）",
                tenant_id=tenant.tenant_id,
                quota=status.quota, used=status.used, additional=additional,
            )
        if status.warning:
            log.warning("上限に近づいています tenant=%s %d/%d (%.0f%%)",
                        tenant.tenant_id, status.used, status.quota, status.ratio * 100)
        return status
