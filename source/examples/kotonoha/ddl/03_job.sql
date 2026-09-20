-- Kotonoha: 取り込みジョブ・再インデックスジョブ
--
-- 取り込みは件数が多いので受付だけ返して非同期に走らせる。
-- 進捗は /v1/jobs/{job_id} で引く。

CREATE TABLE t_ingest_job (
    job_id           VARCHAR(36)   NOT NULL,   -- UUID
    collection_id    VARCHAR(36)   NOT NULL,
    tenant_id        VARCHAR(32)   NOT NULL,
    status           VARCHAR(16)   NOT NULL,   -- queued/running/succeeded/failed/canceled
    total_count      INTEGER       NOT NULL DEFAULT 0,   -- 受け付けた文書数
    done_count       INTEGER       NOT NULL DEFAULT 0,
    failed_count     INTEGER       NOT NULL DEFAULT 0,
    chunk_count      INTEGER       NOT NULL DEFAULT 0,   -- 埋め込んだチャンク数（課金の単位）
    cached_count     INTEGER       NOT NULL DEFAULT 0,   -- キャッシュに当たった数（課金しない）
    error_message    VARCHAR(1024),
    queued_at        TIMESTAMP     NOT NULL DEFAULT CURRENT_TIMESTAMP,
    started_at       TIMESTAMP,
    finished_at      TIMESTAMP,
    CONSTRAINT pk_ingest_job PRIMARY KEY (job_id),
    CONSTRAINT fk_ingest_job_collection
        FOREIGN KEY (collection_id) REFERENCES t_collection (collection_id),
    CONSTRAINT ck_ingest_job_status CHECK (
        status IN ('queued', 'running', 'succeeded', 'failed', 'canceled'))
);

COMMENT ON TABLE  t_ingest_job IS '取り込みジョブ。インジェスト・登録とも呼ばれる';
COMMENT ON COLUMN t_ingest_job.cached_count IS 'キャッシュに当たった分。課金に数えない';

-- ジョブの明細。文書 1 件ごとの成否を残す。
CREATE TABLE t_ingest_job_item (
    job_id           VARCHAR(36)   NOT NULL,
    seq_no           INTEGER       NOT NULL,
    external_id      VARCHAR(256),
    document_id      VARCHAR(36),              -- 成功したら入る
    status           VARCHAR(16)   NOT NULL,   -- pending/done/skipped/failed
    skip_reason      VARCHAR(64),              -- same_hash / empty / unsupported_type
    error_message    VARCHAR(1024),
    finished_at      TIMESTAMP,
    CONSTRAINT pk_ingest_job_item PRIMARY KEY (job_id, seq_no),
    CONSTRAINT fk_ingest_job_item_job
        FOREIGN KEY (job_id) REFERENCES t_ingest_job (job_id) ON DELETE CASCADE
);

COMMENT ON TABLE  t_ingest_job_item IS '取り込みジョブの明細。1 文書 1 行';

-- 再インデックスジョブ。モデル更新時に全件を計算し直す。
CREATE TABLE t_reindex_job (
    job_id           VARCHAR(36)   NOT NULL,
    collection_id    VARCHAR(36)   NOT NULL,
    from_model       VARCHAR(64)   NOT NULL,
    to_model         VARCHAR(64)   NOT NULL,
    from_index       VARCHAR(64)   NOT NULL,   -- 旧インデックス名
    to_index         VARCHAR(64)   NOT NULL,   -- 新インデックス名
    status           VARCHAR(16)   NOT NULL,   -- queued/building/verifying/switched/failed
    total_chunks     INTEGER       NOT NULL DEFAULT 0,
    done_chunks      INTEGER       NOT NULL DEFAULT 0,
    switched_at      TIMESTAMP,                -- 別名を張り替えた時刻
    old_dropped_at   TIMESTAMP,                -- 旧インデックスを消した時刻
    queued_at        TIMESTAMP     NOT NULL DEFAULT CURRENT_TIMESTAMP,
    finished_at      TIMESTAMP,
    CONSTRAINT pk_reindex_job PRIMARY KEY (job_id),
    CONSTRAINT fk_reindex_job_collection
        FOREIGN KEY (collection_id) REFERENCES t_collection (collection_id),
    CONSTRAINT ck_reindex_job_status CHECK (
        status IN ('queued', 'building', 'verifying', 'switched', 'failed'))
);

COMMENT ON TABLE  t_reindex_job IS '再インデックス。旧を読ませたまま新を作り、別名を張り替える';
COMMENT ON COLUMN t_reindex_job.old_dropped_at IS '旧インデックスの保持期間は runbook にある（7 日）';

CREATE INDEX ix_ingest_job_tenant ON t_ingest_job (tenant_id, queued_at DESC);
CREATE INDEX ix_ingest_job_status ON t_ingest_job (status) WHERE status IN ('queued', 'running');
CREATE INDEX ix_reindex_job_collection ON t_reindex_job (collection_id, queued_at DESC);
