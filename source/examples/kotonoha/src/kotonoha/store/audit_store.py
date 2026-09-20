"""``t_audit_log`` の書き込みと点検。

**本文の列を持たない。** 情報セキュリティ点検表の指摘に従い、検索語は
``query_hash`` にしか残さない。挿入の SQL に本文の列が無いことが、
その担保になっている。

★ ただしアプリログ側（``common.audit.record``）が DEBUG で本文を出して
   おり、指摘に違反している（README の仕込み C2）。**ここは正しく、
   直すべきなのは ``common/audit.py`` のほうである。**
"""

from __future__ import annotations

from kotonoha.common.audit import AuditEntry
from kotonoha.store.connection import Connection

#: **本文の列は無い。**
INSERT_AUDIT = """
INSERT INTO t_audit_log (
    occurred_at, tenant_id, key_id, operation, collection_id,
    classification, embed_model, route, item_count, query_hash,
    elapsed_ms, status_code, client_ip
) VALUES (%s, %s, %s, %s, %s, %s, %s, %s, %s, %s, %s, %s, %s)
"""

#: 極秘が外部経路へ出た記録。**常に 0 件であるべき。**
SELECT_VIOLATIONS = """
SELECT audit_id, occurred_at, tenant_id, operation, embed_model, route
FROM t_audit_log
WHERE classification = '30' AND route = 'external'
  AND occurred_at >= %s
ORDER BY occurred_at DESC
"""

SELECT_BY_TENANT = """
SELECT occurred_at, operation, classification, embed_model, route,
       item_count, elapsed_ms, status_code
FROM t_audit_log
WHERE tenant_id = %s AND occurred_at >= %s
ORDER BY occurred_at DESC
LIMIT %s
"""

#: 保持期間を過ぎた記録の削除。**5 年**（機密区分 20 の保持期間に合わせた）。
DELETE_EXPIRED = """
DELETE FROM t_audit_log
WHERE occurred_at < CURRENT_TIMESTAMP - (%s || ' years')::interval
"""


class SqlAuditRepository:
    """``t_audit_log``。"""

    def __init__(self, conn: Connection) -> None:
        self._conn = conn

    def write(self, entry: AuditEntry) -> None:
        self._conn.execute(INSERT_AUDIT, (
            entry.occurred_at, entry.tenant_id, entry.key_id, entry.operation,
            entry.collection_id, entry.classification, entry.embed_model,
            entry.route, entry.item_count, entry.query_hash,
            entry.elapsed_ms, entry.status_code, entry.client_ip,
        ))

    def violations(self, since) -> list[dict]:
        """極秘が外部へ出た記録。セキュリティ部門が月次で見る。"""
        return self._conn.fetch_all(SELECT_VIOLATIONS, (since,))

    def by_tenant(self, tenant_id: str, since, limit: int = 200) -> list[dict]:
        return self._conn.fetch_all(SELECT_BY_TENANT, (tenant_id, since, limit))

    def purge(self, retention_years: int = 5) -> None:
        """保持期間を過ぎたものを消す。日次のバッチが呼ぶ。"""
        self._conn.execute(DELETE_EXPIRED, (retention_years,))
