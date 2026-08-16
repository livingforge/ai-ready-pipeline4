"""テナント・API キー・コレクションの値。``t_tenant`` などと対。"""

from __future__ import annotations

from dataclasses import dataclass, field
from datetime import date, datetime

from kotonoha.common.clock import now


@dataclass
class Tenant:
    """テナント（利用部門）。利用申請台帳の 1 行にあたる。"""

    tenant_id: str
    tenant_name: str
    department: str
    classification: str
    embed_model: str
    monthly_quota: int
    cost_center: str
    status: str = "A"                    # A=有効 S=停止 D=廃止
    applied_at: date | None = None
    approved_at: date | None = None

    @property
    def active(self) -> bool:
        return self.status == "A" and self.approved_at is not None


@dataclass
class ApiKey:
    """API キー。平文は持たない（``key_hash`` だけ）。"""

    key_id: str
    tenant_id: str
    key_hash: str
    label: str = ""
    expires_at: datetime | None = None
    revoked_at: datetime | None = None
    last_used_at: datetime | None = None
    created_at: datetime = field(default_factory=now)

    def usable(self, at: datetime | None = None) -> bool:
        """いま使えるか。失効・期限切れは使えない。"""
        when = at or now()
        if self.revoked_at is not None:
            return False
        return self.expires_at is None or self.expires_at > when


@dataclass
class Collection:
    """コレクション（検索単位）。索引・インデックスとも呼ばれる。"""

    collection_id: str
    tenant_id: str
    collection_name: str
    classification: str
    embed_model: str
    embed_dim: int
    index_alias: str
    chunk_count: int = 0
    status: str = "A"                    # A=有効 R=再構築中 D=削除済
    created_at: datetime = field(default_factory=now)

    @property
    def writable(self) -> bool:
        """取り込みを受け付けられるか。再構築中は受けない。"""
        return self.status == "A"

    @property
    def readable(self) -> bool:
        """検索できるか。**再構築中も読める**（旧を読み続ける）。"""
        return self.status in ("A", "R")
