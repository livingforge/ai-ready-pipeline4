-- マイグレーション 2026-02-10: 極秘区分の受け入れ
--
-- 法務部（legal-contract）の利用申請にあたって入れた変更。
-- 極秘（30）は外部 API へ出せないので、どの経路で埋め込んだかを
-- 監査ログに残せるようにした。

ALTER TABLE t_audit_log ADD COLUMN route VARCHAR(16);
COMMENT ON COLUMN t_audit_log.route IS 'external=外部API internal=社内GPU';

ALTER TABLE t_tenant ADD COLUMN embed_model VARCHAR(64) NOT NULL DEFAULT 'voyage-4';
COMMENT ON COLUMN t_tenant.embed_model IS '極秘テナントは voyage-4-nano（社内GPU）';

ALTER TABLE t_usage_daily ADD COLUMN gpu_seconds INTEGER NOT NULL DEFAULT 0;
ALTER TABLE t_usage_monthly ADD COLUMN gpu_seconds INTEGER NOT NULL DEFAULT 0;

INSERT INTO m_price (price_kind, valid_from, valid_to, unit_price, note) VALUES
    ('gpu_second', DATE '2026-02-01', DATE '9999-12-31', 0.0140,
     '社内 GPU の減価償却と電力を占有秒で按分');
