"""検索。ベクトルと全文を融合し、リランクして返す。

    query      検索語の正規化
    vector     ベクトル検索（pgvector / HNSW）
    keyword    全文検索（OpenSearch / BM25）
    fusion     ★ RRF。定数 k=60 の根拠はここにしか無い
    rerank     ★ ADR-005 は「第2次リリース」だが実装済み
    filters    機密区分とメタデータの絞り込み
    highlight  当たった箇所の切り出し
    explain    点数の内訳（調査用）
    service    上を束ねた入口
"""
