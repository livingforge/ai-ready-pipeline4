# API の使い方

**正本は `openapi.yaml`**（リポジトリの根）。この文書は補足で、
食い違ったら `openapi.yaml` が正しい。

## 共通

- 認証: `Authorization: Bearer <API キー>`
- 本文: `application/json; charset=utf-8`
- 基底: `https://kotonoha.internal.adatum.example/`

### エラーの形

```json
{"error": {"code": "quota_exceeded", "message": "月間の上限を超えます（…）",
           "detail": {"quota": 100000, "used": 99980, "additional": 50}}}
```

| ステータス | いつ |
| --- | --- |
| 400 | 入力が不正（形・長さ・件数） |
| 401 | API キーが無い／不正／テナントが停止中 |
| 403 | 機密区分の規則に反する |
| 404 | 無い（**他テナントのものも 404**） |
| 405 | そのメソッドは使えない |
| 409 | 同じ名前がある／再インデックス中 |
| 429 | 秒間または月間の上限を超えた |
| 502 | 埋め込みの提供元が失敗した |

**`code` で分岐すること。** `message` は日本語で、予告なく変わる。

## エンドポイント

| メソッド | パス | 何をするか |
| --- | --- | --- |
| POST | `/v1/embeddings` | テキストをベクトルにする |
| POST | `/v1/collections` | コレクションを作る |
| GET | `/v1/collections` | 一覧 |
| GET | `/v1/collections/{id}` | 1 件 |
| POST | `/v1/collections/{id}/documents` | 取り込む（**202・非同期**） |
| DELETE | `/v1/collections/{id}/documents/{doc_id}` | 消す |
| POST | `/v1/collections/{id}/search` | 検索する |
| GET | `/v1/jobs/{job_id}` | 取り込みの進捗 |
| GET | `/v1/usage` | 当月の利用量と上限 |
| GET | `/healthz` `/readyz` | 死活監視（認証不要） |

**再インデックスの API は無い。** 運用へ依頼する。

## 気をつけること

### `input_type` を必ず指定する

`POST /v1/embeddings` の `input_type` は `query` か `document`。
retrieval では**必ず指定する** —— 省くと精度が落ちる。
検索語を埋めるなら `query`、文書なら `document`。

### 取り込みは 202 で返る

**まだ入っていない。** `Location` ヘッダのジョブを引いて進捗を見る。
`status` が `succeeded` になっても `failures` が空とは限らない
（1 件の失敗で全体は止めていない）。

### 件数の上限

| 対象 | 上限 |
| --- | --- |
| `/v1/embeddings` の `input` | 128 件 |
| 取り込みの `documents` | 1,000 件 |
| 検索の `top_k` | 100 |
| 検索語の長さ | 2,000 文字 |
| 絞り込みの条件 | 16 個（値は各 32 個まで） |

### 429 のとき

**指数バックオフで再試行すること。** 即座に再送すると 429 が返り続ける。

- 秒間の上限 → `code` が `rate_limited`。少し待てば通る
- 月間の上限 → `code` が `quota_exceeded`。**待っても通らない。**
  上限の変更を AI基盤グループへ依頼する

### 検索は上限に掛からない

月間上限の対象は取り込み（埋め込み）だけ。**検索は止めない。**

## 変更の知らせ方

破壊的な変更は 1 か月前に社内ポータルで知らせる。
追加（新しい項目・新しいエンドポイント）は予告なく入る。

**応答に知らない項目が増えても落ちないように書くこと。**
