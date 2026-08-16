-- マイグレーション 2026-05-18: 埋め込みの量子化
--
-- ★ この変更は ADR に反映されていない。
--
-- ADR-003（2025/12/08）は「埋め込みは 1024 次元の float で持つ」と決めているが、
-- 品質保証部の取り込み量が試算の 3 倍になり、ベクトルの保管費が予算を超えた。
-- int8 量子化で 1/4 に落とす判断を運用の場でして、そのまま入れてある。
-- ADR-003 は書き換えていない（README の仕込み B1）。
--
-- 判断の経緯は docs/runbook/reindex.md の末尾に 3 行だけ残っている。

ALTER TABLE t_embedding ADD COLUMN vec_i8 BYTEA;
COMMENT ON COLUMN t_embedding.vec_i8 IS 'int8 量子化した表現。1024 バイト。★ADR-003 未反映';

ALTER TABLE t_embedding ADD COLUMN quantized BOOLEAN NOT NULL DEFAULT FALSE;
COMMENT ON COLUMN t_embedding.quantized IS 'TRUE なら vec ではなく vec_i8 を読む';

ALTER TABLE t_embed_cache ADD COLUMN vec_i8 BYTEA;
ALTER TABLE t_embed_cache ADD COLUMN quantized BOOLEAN NOT NULL DEFAULT FALSE;

-- 量子化済みのものは vec を NULL にして容量を空ける。
-- 既存分の詰め替えは reindex ジョブが少しずつやる。
ALTER TABLE t_embedding ALTER COLUMN vec DROP NOT NULL;

CREATE INDEX ix_embedding_quantized ON t_embedding (index_name) WHERE quantized = TRUE;
