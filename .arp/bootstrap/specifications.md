# 仕様整理と設計書生成

## 設計

抽出・文書編集は `arp4 documents`、仕様整理・設計書生成は `arp4 spec` が担当します。
仕様は文書を横断するため、文書ごとの対応表やExcel操作には埋め込みません。
旧ラウンド・凍結・自動採番・旧保存形式は使いません。

| 段階 | 正本・保存先 | 責務 |
| --- | --- | --- |
| 入力 | `.arp/evidence/spec-inputs/<hash>.json` | 抽出結果または文書正本の版と出典単位を固定する |
| 仕様整理 | `knowledge/specifications/<id>/spec.yml` | Agent・人が型付きの仕様、関係、判断理由、除外理由、未解決事項を書く |
| レビュー | 同ディレクトリの `review.json` | 検証済み仕様のハッシュとレビュー担当を記録する |
| 設計書生成 | `.arp/out/specifications/<id>/<hash>/` | 仕様から決定的にMarkdownを生成する。生成物は編集正本にしない |

入力を準備しても意味の分類は自動確定しません。安定したIDは整理する人・Agentが付けます。
機械は必須項目、ID重複、参照先、関係の型、出典の実在、入力の網羅性を検証します。
推論した仕様には判断理由が必要です。不要な入力には除外理由を残します。

正式出力では空の仕様、草稿項目、未解決事項、未整理入力、入力の更新、未レビューを拒否します。
`--draft` は内容・参照の整合性を検証した上で、未完了箇所を明示して出力します。
同じ仕様・入力・レビュー・出力モードから同じ出力を作り、既存の生成物が改変されていれば上書きしません。
生成は文書正本・仕様正本・原本を変更せず、入力の固定版と出典一覧を出力に同梱します。

入力形式、検証、レンダリング、保存処理を別モジュールにし、設計書の種類を追加する際に
抽出処理やレビュー条件を変更せずに済む構成にします。

実装は `specifications/contracts.py`（型）、`model.py`（純粋な検証）、`sources.py`（入力変換）、
`render.py`（純粋な出力）、`workspace.py`（保存・レビュー）、`cli.py`（操作）に分かれます。

## 操作

```shell
arp4 documents init --root <project>
arp4 documents import docs/基本設計.xlsx --id order-design --root <project>
arp4 spec init order-system --title 受注管理システム --proposal <出力された候補名> --root <project>
arp4 spec sources order-system --root <project>
arp4 spec schema
```

`--proposal` は候補の編集本文ではなく、候補が参照する機械抽出結果を読みます。
成形後の文書を使う場合は `--document <採用済み文書ID>` を指定します。
両方とも繰り返し指定でき、複数文書から1つの仕様へ整理できます。同じ文書の複数版は混ぜません。
文書正本を使う場合、正式出力の前にその文書もレビューしてください。

作成された `knowledge/specifications/order-system/organize.md` に従い、Agent・人が `spec.yml` を編集します。
APIキーやLLM呼び出しは組み込んでいません。型の詳細は `arp4 spec schema` で確認できます。

```shell
arp4 spec check order-system --root <project>
arp4 spec build order-system --draft --root <project>
arp4 spec review order-system --reviewer <レビュー担当> --root <project>
arp4 spec check order-system --require-reviewed --root <project>
arp4 spec build order-system --root <project>
```

`check` は不正な仕様や未完了項目があれば終了コード1を返します。
レビュー前でも整合性・入力網羅性が揃えば成功します。レビューも必須にする場合は `--require-reviewed` を指定します。
`build` は成功すると成果物の `index.md` を表示します。生成先にはMarkdown、固定仕様、入力JSON、
各ファイルのハッシュを持つ `manifest.json` が含まれます。

入力更新後は新しい文書候補または正本を指定して更新します。

```shell
arp4 spec refresh order-system --proposal <新しい候補名> --root <project>
```

`refresh.json` に追加・変更・削除された入力単位と影響する仕様IDを記録します。
仕様本文とIDは維持し、変更された出典を使う項目と関係上の依存項目を草稿へ戻します。
該当する除外理由は再確認、解決済み課題は再オープンの対象です。
出典が消えた場合、`check` は参照切れを報告します。IDの位置だけで同じ意味と判断しないでください。
更新後は内容・対応を整理し、状態を確定して再レビューします。

## 仕様の型と関係

各項目に `id`、`kind`、`title`、`statement`、`status`、`basis`、`sources`、`rationale`、`details` を持たせます。
`status` は `draft` / `confirmed`、`basis` は `source` / `inferred` です。
出典は `{document: <文書ID>, unit: <sourcesに表示されたID>}` で指定します。
`basis: source` には出典、`basis: inferred` には空でない判断理由が必要です。

| 種類 | 主な内容 |
| --- | --- |
| requirement / function | 要件・受入条件、機能の入出力・規則 |
| component / process / program | システム構成、処理手順、モジュール・言語・例外処理 |
| entity / field | テーブル・項目の物理名、型、NULL・主キー |
| screen / interface | 画面項目・操作、APIの要求・応答・エラー |
| test_plan / test / test_result | テスト方針・条件、ケース、実測結果・実行証跡 |
| migration / deployment / operation | 移行・照合、リリース・切り戻し、監視・復旧 |
| security / project_task / decision | 脅威・対策、担当・予定・成果物、設計判断と影響 |

関係は `relations` に `{from: <ID>, to: <ID>, kind: <関係>}` として記述します。
`satisfies`（要件を充足）、`implements`（機能等を実装）、`verifies`（テストで検証）、
`result_of`（テストの実施結果）、`contains`（テーブルの項目）、`uses`（利用）、`depends_on`（依存）を扱います。
項目は1つのテーブルに属し、テスト実行結果は1つのテストを参照します。
参照の存在・型は機械検証し、記述内容の妥当性は人・Agentがレビューします。

対象外にする入力は `exclusions` に `{source: {document: ..., unit: ...}, reason: ..., status: confirmed}`
を記録します。使用中の出典と除外は重複できません。未確定なら `status: draft` とします。
未解決事項は `issues` に記録し、`resolved` にする場合は `resolution` を必須とします。

## 生成する成果物

`arp4 spec kinds` で一覧を取得できます。すべてMarkdownです。

| 工程 | 成果物 |
| --- | --- |
| 計画・要件 | プロジェクト計画書、要件定義書 |
| 設計 | 基本設計書、詳細設計書、テーブル定義書、画面・帳票項目設計書、インターフェース設計書、プログラム設計書 |
| テスト | テスト計画書、テスト仕様書、テスト結果報告書 |
| 移行・運用 | 移行設計書、リリース手順書、運用設計・手順書、セキュリティ設計書 |
| 管理 | 設計判断記録、課題管理表、トレーサビリティ |

該当する仕様がない成果物・節は「未定義」と明記します。存在しない設計や実行結果を補いません。
表題や節構成は共通テンプレートであり、組織固有のExcel/Word帳票への出力はこの生成機能の対象外です。
生成した設計書を直接直すと次回出力との整合性が失われるため、元の `spec.yml` を編集します。
現行の `documents export` による原本Excel書き戻しは独立した機能として継続します。

ソースリポジトリの `examples/specification-demo/` に、手作業で整理した架空システムの実行例があります。
