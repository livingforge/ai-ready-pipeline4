-- Kotonoha: 文書・チャンク・ベクトル
--
-- 原文は S3 互換のオブジェクトストアに置き、ここには位置だけを持つ。
-- チャンクの分割規則（512 トークン・オーバーラップ 64）は
-- **この DDL にも設計文書にも書かれていない** —— kotonoha/ingest/chunker.py
-- にしか無い（README の仕込み A1）。

CREATE EXTENSION IF NOT EXISTS vector;

CREATE TABLE t_document (
    document_id      VARCHAR(36)   NOT NULL,   -- UUID
    collection_id    VARCHAR(36)   NOT NULL,
    external_id      VARCHAR(256),             -- 取り込み元での識別子（任意）
    title            VARCHAR(512),
    source_uri       VARCHAR(1024) NOT NULL,   -- オブジェクトストア上の位置
    content_type     VARCHAR(64)   NOT NULL,   -- text/markdown, application/pdf など
    content_hash     CHAR(64)      NOT NULL,   -- SHA-256。再取り込みの判定に使う
    byte_size        INTEGER       NOT NULL,
    chunk_count      SMALLINT      NOT NULL DEFAULT 0,
    metadata         JSONB         NOT NULL DEFAULT '{}',  -- 検索時の絞り込みに使う
    ingested_at      TIMESTAMP     NOT NULL DEFAULT CURRENT_TIMESTAMP,
    deleted_at       TIMESTAMP,
    CONSTRAINT pk_document PRIMARY KEY (document_id),
    CONSTRAINT fk_document_collection
        FOREIGN KEY (collection_id) REFERENCES t_collection (collection_id)
);

COMMENT ON TABLE  t_document IS '取り込んだ文書。原文は持たず位置だけを持つ';
COMMENT ON COLUMN t_document.content_hash IS '同じハッシュなら再取り込みを飛ばす';
COMMENT ON COLUMN t_document.metadata IS '部署・年度・製品などの絞り込みキー。スキーマは決めていない';

-- チャンク（分割単位・断片とも呼ばれる）。
CREATE TABLE t_chunk (
    chunk_id         VARCHAR(36)   NOT NULL,   -- UUID
    document_id      VARCHAR(36)   NOT NULL,
    collection_id    VARCHAR(36)   NOT NULL,   -- 検索の絞り込みで毎回使うので非正規化
    seq_no           SMALLINT      NOT NULL,   -- 文書内の通し番号（0 始まり）
    body             TEXT          NOT NULL,   -- チャンク本文
    token_count      SMALLINT      NOT NULL,
    heading_path     VARCHAR(512),             -- 見出しの階層（"1章 > 1.2 節"）
    char_start       INTEGER       NOT NULL,   -- 原文中の開始位置
    char_end         INTEGER       NOT NULL,
    created_at       TIMESTAMP     NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT pk_chunk PRIMARY KEY (chunk_id),
    CONSTRAINT uq_chunk_seq UNIQUE (document_id, seq_no),
    CONSTRAINT fk_chunk_document
        FOREIGN KEY (document_id) REFERENCES t_document (document_id) ON DELETE CASCADE
);

COMMENT ON TABLE  t_chunk IS 'チャンク。分割規則は ingest/chunker.py にしか書かれていない';
COMMENT ON COLUMN t_chunk.heading_path IS '見出し境界で切ったときの階層。検索結果の文脈表示に使う';

-- ベクトル。チャンクと 1 対 1 だが、再インデックス中は 2 世代が同居するので別表。
CREATE TABLE t_embedding (
    chunk_id         VARCHAR(36)   NOT NULL,
    index_name       VARCHAR(64)   NOT NULL,   -- 実インデックス名（別名ではない）
    embed_model      VARCHAR(64)   NOT NULL,
    vec              vector(1024)  NOT NULL,   -- ★ ADR-003 は 1024 次元 float
    created_at       TIMESTAMP     NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT pk_embedding PRIMARY KEY (chunk_id, index_name),
    CONSTRAINT fk_embedding_chunk
        FOREIGN KEY (chunk_id) REFERENCES t_chunk (chunk_id) ON DELETE CASCADE
);

COMMENT ON TABLE  t_embedding IS 'ベクトル。再インデックス中は新旧 2 つの index_name が同居する';
COMMENT ON COLUMN t_embedding.vec IS 'ADR-003 で 1024 次元と決めた。★実装は int8 量子化を入れている（未反映）';

-- 埋め込みキャッシュ。同一テキスト・同一モデルの再計算を避ける。
CREATE TABLE t_embed_cache (
    text_hash        CHAR(64)      NOT NULL,   -- SHA-256（正規化後の本文）
    embed_model      VARCHAR(64)   NOT NULL,
    vec              vector(1024)  NOT NULL,
    hit_count        INTEGER       NOT NULL DEFAULT 0,
    created_at       TIMESTAMP     NOT NULL DEFAULT CURRENT_TIMESTAMP,
    expires_at       TIMESTAMP     NOT NULL,   -- 作成 + 30 日
    CONSTRAINT pk_embed_cache PRIMARY KEY (text_hash, embed_model)
);

COMMENT ON TABLE  t_embed_cache IS '埋め込みキャッシュ。当たった分は課金に数えない';

CREATE INDEX ix_document_collection ON t_document (collection_id) WHERE deleted_at IS NULL;
CREATE INDEX ix_document_external ON t_document (collection_id, external_id);
CREATE INDEX ix_chunk_collection ON t_chunk (collection_id);
CREATE INDEX ix_embed_cache_expire ON t_embed_cache (expires_at);

-- HNSW 索引。m と ef_construction は運用で調整した値で、
-- **その根拠はどこにも書かれていない**（runbook にも ADR にも無い）。
CREATE INDEX ix_embedding_hnsw ON t_embedding
    USING hnsw (vec vector_cosine_ops)
    WITH (m = 16, ef_construction = 200);
