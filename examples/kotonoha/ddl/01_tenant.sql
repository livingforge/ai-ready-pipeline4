-- Kotonoha: テナント・APIキー・コレクション
--
-- 機密区分（classification）はテナントに付き、コレクションはそれを継承する。
-- 継承した区分を下げることはできない（業務ルール「機密区分の継承」）。
-- 実装は kotonoha/tenant/collection.py の create_collection。

CREATE TABLE t_tenant (
    tenant_id        VARCHAR(32)   NOT NULL,   -- テナント識別子（cs-support など）
    tenant_name      VARCHAR(128)  NOT NULL,   -- 表示名
    department       VARCHAR(128)  NOT NULL,   -- 所管部門
    classification   CHAR(2)       NOT NULL,   -- 機密区分 10=一般 20=社外秘 30=極秘
    embed_model      VARCHAR(64)   NOT NULL,   -- 既定の埋め込みモデル
    monthly_quota    INTEGER       NOT NULL,   -- 月間チャンク上限
    cost_center      VARCHAR(16)   NOT NULL,   -- チャージバック先の原価センタ
    status           CHAR(1)       NOT NULL,   -- A=有効 S=停止 D=廃止
    applied_at       DATE          NOT NULL,   -- 利用申請日
    approved_at      DATE,                     -- 承認日（未承認は NULL）
    created_at       TIMESTAMP     NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at       TIMESTAMP     NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT pk_tenant PRIMARY KEY (tenant_id),
    CONSTRAINT ck_tenant_class CHECK (classification IN ('10', '20', '30')),
    CONSTRAINT ck_tenant_status CHECK (status IN ('A', 'S', 'D'))
);

COMMENT ON TABLE  t_tenant IS 'テナント（利用部門）';
COMMENT ON COLUMN t_tenant.classification IS '機密区分。30 は外部 API へ送出してはならない';
COMMENT ON COLUMN t_tenant.monthly_quota IS '月間の埋め込みチャンク上限。超過は 429';

-- API キー。値そのものは持たず、ハッシュだけを持つ。
CREATE TABLE t_api_key (
    key_id           VARCHAR(32)   NOT NULL,   -- キー識別子（先頭 8 桁を公開）
    tenant_id        VARCHAR(32)   NOT NULL,
    key_hash         CHAR(64)      NOT NULL,   -- SHA-256（16 進 64 桁）
    label            VARCHAR(128),             -- 用途のメモ
    expires_at       TIMESTAMP,                -- 無期限は NULL
    revoked_at       TIMESTAMP,                -- 失効済みは日時が入る
    last_used_at     TIMESTAMP,
    created_at       TIMESTAMP     NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT pk_api_key PRIMARY KEY (key_id),
    CONSTRAINT fk_api_key_tenant FOREIGN KEY (tenant_id) REFERENCES t_tenant (tenant_id)
);

COMMENT ON TABLE  t_api_key IS 'API キー。平文は発行時に一度だけ返し、以後は持たない';
COMMENT ON COLUMN t_api_key.key_hash IS 'SHA-256。照合は kotonoha/tenant/apikey.py';

-- コレクション（検索の単位）。
CREATE TABLE t_collection (
    collection_id    VARCHAR(36)   NOT NULL,   -- UUID
    tenant_id        VARCHAR(32)   NOT NULL,
    collection_name  VARCHAR(128)  NOT NULL,
    classification   CHAR(2)       NOT NULL,   -- テナントから継承。下げられない
    embed_model      VARCHAR(64)   NOT NULL,   -- 作成時に固定。以後変えられない
    embed_dim        SMALLINT      NOT NULL,   -- 次元。モデルに従う
    index_alias      VARCHAR(64)   NOT NULL,   -- 実インデックスへの別名（再インデックスで張り替える）
    chunk_count      INTEGER       NOT NULL DEFAULT 0,
    status           CHAR(1)       NOT NULL,   -- A=有効 R=再構築中 D=削除済
    created_at       TIMESTAMP     NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at       TIMESTAMP     NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT pk_collection PRIMARY KEY (collection_id),
    CONSTRAINT uq_collection_name UNIQUE (tenant_id, collection_name),
    CONSTRAINT fk_collection_tenant FOREIGN KEY (tenant_id) REFERENCES t_tenant (tenant_id),
    CONSTRAINT ck_collection_class CHECK (classification IN ('10', '20', '30')),
    CONSTRAINT ck_collection_status CHECK (status IN ('A', 'R', 'D'))
);

COMMENT ON TABLE  t_collection IS 'コレクション（検索単位）。索引・インデックスとも呼ばれる';
COMMENT ON COLUMN t_collection.index_alias IS '再インデックスは別名の張り替えで無停止に切り替える';
COMMENT ON COLUMN t_collection.embed_model IS '作成時に固定。モデルを変えるには再インデックスが要る';

CREATE INDEX ix_collection_tenant ON t_collection (tenant_id, status);
CREATE INDEX ix_api_key_tenant ON t_api_key (tenant_id) WHERE revoked_at IS NULL;
