"""``t_tenant`` ``t_api_key`` ``t_collection`` の読み書き。"""

from __future__ import annotations

from kotonoha.store.connection import Connection
from kotonoha.tenant.models import ApiKey, Collection, Tenant

_TENANT_COLUMNS = (
    "tenant_id, tenant_name, department, classification, embed_model, "
    "monthly_quota, cost_center, status, applied_at, approved_at"
)

SELECT_TENANT = f"SELECT {_TENANT_COLUMNS} FROM t_tenant WHERE tenant_id = %s"

SELECT_TENANTS = f"SELECT {_TENANT_COLUMNS} FROM t_tenant ORDER BY tenant_id"

UPSERT_TENANT = f"""
INSERT INTO t_tenant ({_TENANT_COLUMNS})
VALUES (%s, %s, %s, %s, %s, %s, %s, %s, %s, %s)
ON CONFLICT (tenant_id) DO UPDATE SET
    tenant_name    = EXCLUDED.tenant_name,
    department     = EXCLUDED.department,
    classification = EXCLUDED.classification,
    embed_model    = EXCLUDED.embed_model,
    monthly_quota  = EXCLUDED.monthly_quota,
    cost_center    = EXCLUDED.cost_center,
    status         = EXCLUDED.status,
    approved_at    = EXCLUDED.approved_at,
    updated_at     = CURRENT_TIMESTAMP
"""

_KEY_COLUMNS = ("key_id, tenant_id, key_hash, label, expires_at, "
                "revoked_at, last_used_at, created_at")

SELECT_KEY = f"SELECT {_KEY_COLUMNS} FROM t_api_key WHERE key_id = %s"

#: **失効していないものだけ**を照合の対象にする。
SELECT_LIVE_KEYS = f"""
SELECT {_KEY_COLUMNS} FROM t_api_key
WHERE revoked_at IS NULL
  AND (expires_at IS NULL OR expires_at > CURRENT_TIMESTAMP)
"""

UPSERT_KEY = f"""
INSERT INTO t_api_key ({_KEY_COLUMNS})
VALUES (%s, %s, %s, %s, %s, %s, %s, %s)
ON CONFLICT (key_id) DO UPDATE SET
    label        = EXCLUDED.label,
    expires_at   = EXCLUDED.expires_at,
    revoked_at   = EXCLUDED.revoked_at,
    last_used_at = EXCLUDED.last_used_at
"""

_COLLECTION_COLUMNS = (
    "collection_id, tenant_id, collection_name, classification, embed_model, "
    "embed_dim, index_alias, chunk_count, status, created_at"
)

SELECT_COLLECTION = (f"SELECT {_COLLECTION_COLUMNS} FROM t_collection "
                     f"WHERE collection_id = %s")

SELECT_COLLECTION_BY_NAME = (
    f"SELECT {_COLLECTION_COLUMNS} FROM t_collection "
    f"WHERE tenant_id = %s AND collection_name = %s AND status <> 'D'")

SELECT_COLLECTIONS_BY_TENANT = (
    f"SELECT {_COLLECTION_COLUMNS} FROM t_collection "
    f"WHERE tenant_id = %s ORDER BY collection_name")

UPSERT_COLLECTION = f"""
INSERT INTO t_collection ({_COLLECTION_COLUMNS})
VALUES (%s, %s, %s, %s, %s, %s, %s, %s, %s, %s)
ON CONFLICT (collection_id) DO UPDATE SET
    embed_model = EXCLUDED.embed_model,
    chunk_count = EXCLUDED.chunk_count,
    status      = EXCLUDED.status,
    updated_at  = CURRENT_TIMESTAMP
"""


class SqlTenantRepository:
    """``t_tenant``。"""

    def __init__(self, conn: Connection) -> None:
        self._conn = conn

    def find(self, tenant_id: str) -> Tenant | None:
        row = self._conn.fetch_one(SELECT_TENANT, (tenant_id,))
        return _to_tenant(row) if row else None

    def all(self) -> list[Tenant]:
        return [_to_tenant(r) for r in self._conn.fetch_all(SELECT_TENANTS)]

    def save(self, tenant: Tenant) -> None:
        self._conn.execute(UPSERT_TENANT, (
            tenant.tenant_id, tenant.tenant_name, tenant.department,
            tenant.classification, tenant.embed_model, tenant.monthly_quota,
            tenant.cost_center, tenant.status, tenant.applied_at,
            tenant.approved_at,
        ))


class SqlApiKeyRepository:
    """``t_api_key``。"""

    def __init__(self, conn: Connection) -> None:
        self._conn = conn

    def find(self, key_id: str) -> ApiKey | None:
        row = self._conn.fetch_one(SELECT_KEY, (key_id,))
        return _to_key(row) if row else None

    def all(self) -> list[ApiKey]:
        return [_to_key(r) for r in self._conn.fetch_all(SELECT_LIVE_KEYS)]

    def list_by_tenant(self, tenant_id: str) -> list[ApiKey]:
        return [k for k in self.all() if k.tenant_id == tenant_id]

    def save(self, key: ApiKey) -> None:
        self._conn.execute(UPSERT_KEY, (
            key.key_id, key.tenant_id, key.key_hash, key.label,
            key.expires_at, key.revoked_at, key.last_used_at, key.created_at,
        ))


class SqlCollectionRepository:
    """``t_collection``。"""

    def __init__(self, conn: Connection) -> None:
        self._conn = conn

    def find(self, collection_id: str) -> Collection | None:
        row = self._conn.fetch_one(SELECT_COLLECTION, (collection_id,))
        return _to_collection(row) if row else None

    def find_by_name(self, tenant_id: str, name: str) -> Collection | None:
        row = self._conn.fetch_one(SELECT_COLLECTION_BY_NAME, (tenant_id, name))
        return _to_collection(row) if row else None

    def list_by_tenant(self, tenant_id: str) -> list[Collection]:
        rows = self._conn.fetch_all(SELECT_COLLECTIONS_BY_TENANT, (tenant_id,))
        return [_to_collection(r) for r in rows]

    def all(self) -> list[Collection]:
        return []

    def save(self, collection: Collection) -> None:
        self._conn.execute(UPSERT_COLLECTION, (
            collection.collection_id, collection.tenant_id,
            collection.collection_name, collection.classification,
            collection.embed_model, collection.embed_dim,
            collection.index_alias, collection.chunk_count,
            collection.status, collection.created_at,
        ))


def _to_tenant(row: dict) -> Tenant:
    return Tenant(
        tenant_id=row["tenant_id"], tenant_name=row["tenant_name"],
        department=row["department"], classification=row["classification"],
        embed_model=row["embed_model"], monthly_quota=row["monthly_quota"],
        cost_center=row["cost_center"], status=row["status"],
        applied_at=row["applied_at"], approved_at=row["approved_at"],
    )


def _to_key(row: dict) -> ApiKey:
    return ApiKey(
        key_id=row["key_id"], tenant_id=row["tenant_id"],
        key_hash=row["key_hash"], label=row["label"] or "",
        expires_at=row["expires_at"], revoked_at=row["revoked_at"],
        last_used_at=row["last_used_at"], created_at=row["created_at"],
    )


def _to_collection(row: dict) -> Collection:
    return Collection(
        collection_id=row["collection_id"], tenant_id=row["tenant_id"],
        collection_name=row["collection_name"],
        classification=row["classification"], embed_model=row["embed_model"],
        embed_dim=row["embed_dim"], index_alias=row["index_alias"],
        chunk_count=row["chunk_count"], status=row["status"],
        created_at=row["created_at"],
    )
