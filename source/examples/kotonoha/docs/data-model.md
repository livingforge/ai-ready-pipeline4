# データの持ち方

`ddl/` の表定義と対。**表の一覧はここ、列の詳細は DDL のコメント**を見る。

## 全体

```
t_tenant ─┬─ t_api_key
          └─ t_collection ─┬─ t_document ── t_chunk ── t_embedding
                           ├─ t_ingest_job ── t_ingest_job_item
                           └─ t_reindex_job

t_usage_daily ── t_usage_monthly     m_price
t_audit_log                          t_embed_cache
```

## 表

| 表 | 中身 | 増え方 |
| --- | --- | --- |
| `t_tenant` | テナント（利用部門） | 4 件。年に数件増える |
| `t_api_key` | API キー（ハッシュのみ） | テナントあたり 1〜3 |
| `t_collection` | 検索の単位 | テナントあたり 1〜5 |
| `t_document` | 取り込んだ文書 | 100 万件規模 |
| `t_chunk` | 分割したチャンク | **500 万件規模** |
| `t_embedding` | ベクトル | チャンク × 世代数 |
| `t_embed_cache` | 埋め込みキャッシュ | 30 日で消える |
| `t_ingest_job` | 取り込みジョブ | 日に数百 |
| `t_ingest_job_item` | ジョブの明細 | ジョブ × 文書数 |
| `t_reindex_job` | 再インデックス | 年に数回 |
| `t_usage_daily` | 日次の利用量 | テナント × 日 |
| `t_usage_monthly` | 月次の締め結果 | テナント × 月 |
| `m_price` | 単価マスタ | 月に 4 行増える |
| `t_audit_log` | 監査ログ | **日に数万件。5 年保持** |

## 決めごと

**文書は論理削除、チャンクは物理削除。**
: 文書の行を残すのは監査ログから辿れるようにするため。チャンクを消すのは
  検索に出続けると困るため。ベクトルは `ON DELETE CASCADE` で一緒に消える。

**ベクトルは索引名で世代を分ける。**
: 再インデックス中は新旧 2 つの `index_name` が同居する。検索は別名
  （`t_collection.index_alias`）を見るので、張り替えるだけで切り替わる。

**原文はデータベースに入れない。**
: `t_document.source_uri` に位置だけを持つ。実体はオブジェクトストアで、
  **機密区分ごとに別バケット**（`store/object_store.py`）。

**監査ログに本文の列は無い。**
: 情報セキュリティ点検表の指摘。検索語は `query_hash` にしか残さない。

**`t_chunk.collection_id` は非正規化。**
: `t_document` を辿れば分かるが、検索の絞り込みで毎回使うので持たせている。

## 保持期間

| 対象 | 期間 | 根拠 |
| --- | --- | --- |
| 監査ログ | 5 年 | 機密区分 20（社外秘）の保持期間 |
| 埋め込みキャッシュ | 30 日 | 実測で 30 日を超えると当たらなくなる |
| 旧インデックス | 7 日 | 切り戻しの猶予（`docs/runbook/reindex.md`） |
| 文書・チャンク | 無期限 | 各部門が消すまで残す |

**極秘（区分 30）の保持期間は 3 年**と規程にあるが、**実装で自動削除して
いない。** 各部門が消す運用になっているが、確認する仕組みが無い。
情報セキュリティ部の次回点検で指摘される可能性がある（未対応）。

## 索引

| 索引 | 対象 | 用途 |
| --- | --- | --- |
| `ix_embedding_hnsw` | `t_embedding.vec` | ベクトル検索（HNSW） |
| `ix_chunk_collection` | `t_chunk.collection_id` | 絞り込み |
| `ix_document_external` | `t_document(collection_id, external_id)` | 再取り込みの判定 |
| `ix_audit_tenant_time` | `t_audit_log(tenant_id, occurred_at)` | 監査の照会 |
| `ix_usage_daily_date` | `t_usage_daily.usage_date` | 締め |

HNSW の `m=16` / `ef_construction=200` は運用で調整した値。
**根拠は記録されていない**（`docs/runbook/reindex.md` の覚え書きに
`ef_construction` を上げた経緯が 1 行あるだけ）。
