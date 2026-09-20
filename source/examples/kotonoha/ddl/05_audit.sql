-- Kotonoha: 監査ログ
--
-- 情報セキュリティ点検表.xlsx の指摘に「ログに本文を残さない」がある。
-- この表は本文の列を持たない —— ★ ただし kotonoha/common/audit.py が
-- DEBUG レベルでクエリ本文をアプリログへ出しており、指摘に違反している。
-- 未修正のまま残してある（README の仕込み C2）。

CREATE TABLE t_audit_log (
    audit_id         BIGSERIAL     NOT NULL,
    occurred_at      TIMESTAMP     NOT NULL DEFAULT CURRENT_TIMESTAMP,
    tenant_id        VARCHAR(32)   NOT NULL,
    key_id           VARCHAR(32),              -- 呼び出しに使った API キー
    operation        VARCHAR(32)   NOT NULL,   -- embed/search/ingest/delete/reindex
    collection_id    VARCHAR(36),
    classification   CHAR(2)       NOT NULL,   -- 扱った情報の機密区分
    embed_model      VARCHAR(64),              -- 実際に呼んだモデル
    route            VARCHAR(16),              -- external=外部API internal=社内GPU
    item_count       INTEGER       NOT NULL DEFAULT 0,
    query_hash       CHAR(64),                 -- 検索語の SHA-256。**本文は持たない**
    elapsed_ms       INTEGER       NOT NULL DEFAULT 0,
    status_code      SMALLINT      NOT NULL,
    client_ip        VARCHAR(45),
    CONSTRAINT pk_audit_log PRIMARY KEY (audit_id)
);

COMMENT ON TABLE  t_audit_log IS '監査ログ。5 年保持（機密区分 20 の保持期間に合わせた）';
COMMENT ON COLUMN t_audit_log.query_hash IS '本文ではなくハッシュ。同じ検索の再現判定にだけ使う';
COMMENT ON COLUMN t_audit_log.route IS '極秘（30）で external が記録されたら事故。監視が拾う';

CREATE INDEX ix_audit_tenant_time ON t_audit_log (tenant_id, occurred_at DESC);
CREATE INDEX ix_audit_operation ON t_audit_log (operation, occurred_at DESC);

-- 極秘データが外部 API へ出ていないかの点検ビュー。
-- セキュリティ部門が月次で見る。0 件であることが正しい。
CREATE VIEW v_audit_violation AS
SELECT
    audit_id, occurred_at, tenant_id, operation, embed_model, route
FROM t_audit_log
WHERE classification = '30'
  AND route = 'external';

COMMENT ON VIEW v_audit_violation IS '極秘が外部 API へ出た記録。常に 0 件であるべき';
