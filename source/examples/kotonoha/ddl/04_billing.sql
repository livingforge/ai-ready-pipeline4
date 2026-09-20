-- Kotonoha: 利用量計測とチャージバック
--
-- 課金の単位は「埋め込んだチャンク数」と「検索の呼び出し回数」。
-- キャッシュに当たった埋め込みは数えない（業務ルール「利用量の数え方」）。
-- 実装は kotonoha/billing/。

CREATE TABLE t_usage_daily (
    tenant_id        VARCHAR(32)   NOT NULL,
    usage_date       DATE          NOT NULL,
    embed_chunks     INTEGER       NOT NULL DEFAULT 0,   -- 埋め込んだチャンク数
    cached_chunks    INTEGER       NOT NULL DEFAULT 0,   -- キャッシュに当たった数
    search_calls     INTEGER       NOT NULL DEFAULT 0,
    rerank_calls     INTEGER       NOT NULL DEFAULT 0,
    gpu_seconds      INTEGER       NOT NULL DEFAULT 0,   -- 自前ホストの占有秒（極秘テナントのみ）
    updated_at       TIMESTAMP     NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT pk_usage_daily PRIMARY KEY (tenant_id, usage_date),
    CONSTRAINT fk_usage_daily_tenant
        FOREIGN KEY (tenant_id) REFERENCES t_tenant (tenant_id)
);

COMMENT ON TABLE  t_usage_daily IS '日次の利用量。上限判定はこの表の当月合計で行う';
COMMENT ON COLUMN t_usage_daily.gpu_seconds IS '社内 GPU の占有秒。極秘テナントの按分に使う';

CREATE TABLE t_usage_monthly (
    tenant_id        VARCHAR(32)   NOT NULL,
    year_month       CHAR(6)       NOT NULL,   -- YYYYMM
    embed_chunks     INTEGER       NOT NULL DEFAULT 0,
    cached_chunks    INTEGER       NOT NULL DEFAULT 0,
    search_calls     INTEGER       NOT NULL DEFAULT 0,
    rerank_calls     INTEGER       NOT NULL DEFAULT 0,
    gpu_seconds      INTEGER       NOT NULL DEFAULT 0,
    amount_yen       INTEGER       NOT NULL DEFAULT 0,   -- 按分後の金額（円・整数）
    cost_center      VARCHAR(16)   NOT NULL,
    closed_at        TIMESTAMP,                -- 締めた時刻。NULL は未締め
    CONSTRAINT pk_usage_monthly PRIMARY KEY (tenant_id, year_month),
    CONSTRAINT fk_usage_monthly_tenant
        FOREIGN KEY (tenant_id) REFERENCES t_tenant (tenant_id)
);

COMMENT ON TABLE  t_usage_monthly IS '月次の締め結果。経理へはこの表から CSV で渡す';
COMMENT ON COLUMN t_usage_monthly.amount_yen IS '円未満は切り捨て。端数は基盤側で持つ';

-- 単価マスタ。改定に備えて適用期間を持つ。
CREATE TABLE m_price (
    price_kind       VARCHAR(32)   NOT NULL,   -- embed_chunk/search_call/rerank_call/gpu_second
    valid_from       DATE          NOT NULL,
    valid_to         DATE          NOT NULL,
    unit_price       NUMERIC(10,4) NOT NULL,   -- 円（小数 4 桁まで）
    note             VARCHAR(256),
    CONSTRAINT pk_price PRIMARY KEY (price_kind, valid_from),
    CONSTRAINT ck_price_kind CHECK (
        price_kind IN ('embed_chunk', 'search_call', 'rerank_call', 'gpu_second'))
);

COMMENT ON TABLE  m_price IS '単価マスタ。外部 API の請求実績を翌月に按分して決める';

-- 当月の利用量（上限判定に使うビュー）。
CREATE VIEW v_usage_current_month AS
SELECT
    d.tenant_id,
    TO_CHAR(d.usage_date, 'YYYYMM')  AS year_month,
    SUM(d.embed_chunks)              AS embed_chunks,
    SUM(d.cached_chunks)             AS cached_chunks,
    SUM(d.search_calls)              AS search_calls,
    t.monthly_quota                  AS monthly_quota
FROM t_usage_daily d
JOIN t_tenant t ON t.tenant_id = d.tenant_id
WHERE TO_CHAR(d.usage_date, 'YYYYMM') = TO_CHAR(CURRENT_DATE, 'YYYYMM')
GROUP BY d.tenant_id, TO_CHAR(d.usage_date, 'YYYYMM'), t.monthly_quota;

COMMENT ON VIEW v_usage_current_month IS '当月の利用量と上限。tenant/quota.py が引く';

CREATE INDEX ix_usage_daily_date ON t_usage_daily (usage_date);
CREATE INDEX ix_usage_monthly_close ON t_usage_monthly (year_month) WHERE closed_at IS NULL;
