# Agent向けCLI応答仕様

コマンド・引数・既定値の一覧はコードから生成する [CLIコマンド・引数一覧](commands.md) を参照してください。以下は応答の読み方と操作上の判断基準です。

`doctor.cli.response_version: 1` のCLIは、通常実行の標準出力に字下げなしのJSONを1行だけ返す。成功は終了コード0・`ok: true`、引数エラーや実行失敗は終了コード2・`ok: false`。エラーも標準出力に返し、標準エラーとの結合は不要。`--help` と `--version` の明示要求はテキストを返す。

`ok` はコマンド自体の成否。文書の採用・書き戻し可否は `state`・`blockers`・`complete` を確認する。操作の提案や状態名は権限付与を意味しない。

## 小さな応答と詳細取得

`spec workflow` は進行管理と外部実行の機械向けJSONを返す。`next` / `inspect --task <ID>` は概要、`read --task <ID>` は指示・原文・文脈・返信契約の一括閲覧。readは最大48000 UTF-8 JSON bytesのページへ自動分割し、本文を省略しない。同じreadの `page.next_offset` を `--offset` に指定して続ける。個別確認だけ `--pointer <path>` で絞り込む。workflowでは `--full` を使用しない。`status: blocked`（予算切れ・実行失敗など）は `ok: false`・終了コード2。修正で解消できない指摘は停止せず、レビュー後に `status: needs_decision`（草稿を `draft/` に出力、正式exportは不可）で終わる。手順は [進行管理](../guides/semantic-workflow.md)、実行先との接続は [runner契約](../guides/semantic-runners.md) を参照。

| 操作 | 既定の結果 | 詳細の取得 |
| --- | --- | --- |
| doctor | 実装・機能・応答仕様 | 同じ応答 |
| skills install / documents init | state・件数またはroot | 保存先を確認 |
| import | 文書ID・候補ID・候補パス・抽出上の注意 | 候補のcontentと対応表 |
| record / review | 対象ID・実行した状態 | 保存されたformation.json / review.json |
| adopt | 文書ID・本文パス・reviewed | 本文と管理情報 |
| check / status | 全体の状態別件数・対象のページ | 対象IDで絞る、または次ページ |
| diff | 比較別の全変更件数・差分のページ | 次ページ、または `--out <新規パス>` |
| export（outなし） | 全体件数・計画のページ・complete | 次ページ、必要な場合は `--full` |
| export（outあり） | 全体件数・written・Excelとreportのパス | 保存された完全なreport |
| apply | 原本反映結果と再抽出候補（needs_record） | 候補を確認してrecord・adopt |
| schema | プロパティ名・型・必須かどうか | `--pointer <JSON Pointer>`、必要な場合は `--full` |
| spec check | ready/blocked・種類別 summary・coverage・問題のページ | `--offset` / `--limit`、`--full`、または `--out <診断.json>` |
| spec capture / sources / prompt / schema / render | 出力パス | render は `.report.json` も保存。手順は [設計書生成](specifications.md) |
| spec semantic packet / assemble / review-packet / review-apply / finalize / changes | タスク、モデル、分類、監査、採番、変更範囲 | [意味判断に絞った生成](semantic-generation.md)。assembleの成功とreadyは別。finalizeは初回専用 |
| spec registry preflight | 正本初回登録の読み取り専用ゲート検査 | `init` 前の拒否理由確認 |

check/status/diff/export計画/schemaの一覧は `items` と `page` を返す。既定は最大20件、`--limit 1..100` で調整できる。`summary` は全件を集計する。実際のページサイズはバイト制限によってさらに小さくなる。

```json
{"items":[],"ok":true,"page":{"next_offset":null,"offset":0,"returned":0,"total":0},"summary":{}}
```

`page.next_offset` が数値なら、同じ対象・条件で `--offset <値>` を指定する。nullなら次のページはない。ページング中に対象が変化した場合は先頭から取得し直す。`--offset` が件数以上なら空ページとなる。ページングは出力のみを制限し、検証は全対象に実行する。表示外の不正な文書も失敗として全体のsummaryと終了コードに反映される。

通常の応答は改行を含め最大16 KiB、各一覧項目は約2 KiB。大きなフィールドは値を切り詰めずフィールドごと省略し、`omitted_fields` にその名前を列挙する。項目の `index` は全体での0始まりの位置。オブジェクト全体を表示できない場合は `omitted: true` と元のJSONバイト数を返す。これは元データの空値やnullではない。ページが最終でも、値が省略されていれば全内容を確認したことにはならない。

完全な差分は `documents diff ... --out <新規JSONパス>` で保存でき、保存応答には `report` を返す。JSONファイルは従来の `comparisons[].changes` 形式で、値の省略やページングはしない。`--format markdown` は `--out` との併用だけに対応する。

`--full` はサイズ制限とページングを解除する明示的な選択で、`--limit/--offset` とは併用できない。読み取り操作で必要な場合に使い、巨大な結果はファイルへリダイレクトする。record/review/import/adopt/exportの書き込みを、応答の再取得のために繰り返してはならない。保存された証跡・本文・reportを読む。exportの書き込み応答はページを返さず、`--out` と `--limit/--offset` の併用を拒否する。

`--include-hashes` はrecord/review/check/status/exportに検証ハッシュを含める。`--full` とは独立しており、完全な検証情報が必要なら両方を指定する。diffのハッシュで表された変更は差分そのものとして維持される。保存形式と検証に用いる完全なSHA-256は変更しない。

## 行・列の構造変更

`documents export` は本文YAMLの追加・削除を、管理側 `mappings.yml` の `operations` と組み合わせてExcelへ反映できます。対応する操作は `insert_rows`、`delete_rows`、`insert_columns`、`delete_columns` です。各操作には `id`、`sheet`、`reason`、`at`、`count` を指定し、行の追加時は必要に応じて `style_from` で書式をコピーする元の行を指定します（非表示・折りたたみはコピーしません）。追加した列はExcelと同じく左の列の書式を引き継ぐため、列の操作には `style_from` を指定できません。

操作で追加する行・列は、本文の `blocks.<block>.rows` に `<操作ID>-<番号>`（番号は1から `count` まで）のキーで書きます。`id: add` の `insert_rows` で追加した1行目は `add-1: {B: 新項目}`、`insert_columns` で追加した列の値は既存行の `r3: {add-1: 値}` のように書きます。既存の行 `r<number>` と列の英字は原本の位置を指し、操作後も変わらないため、追加した行・列と同じキーにはなりません。CLIはこのキーから挿入操作を特定してmapping entryを再生成します。削除時は本文から対象の行・列を削除し、対応するdelete操作を残します。操作のない追加・削除、原本で値のないセルへの書き込み、操作の範囲外の番号、追加した行と列が交わるセルは検証で拒否します。

結合範囲の内側への挿入は、Excelと同じく結合を広げます。広がった結合の左上以外になるセルはExcelに表示されないため、そこへの書き込みは検証と書き戻しで拒否します。値は結合の左上に書くか、結合の外に行・列を追加してください。

構造変更のexport reportには、反映した `operations`、値の `changes`、削除された原本セルの `deleted_cells` が保存されます。同一シートのA1形式の数式参照は移動し、数式キャッシュは無効化されます。既存のDrawing XMLにある画像・図形のセルアンカーも行・列操作に追従します。

PNG画像は、文書の `assets/` に置いたファイルを `add_image` 操作から追加できます。`asset` は `assets/image001.png`、`anchor.from.cell` と `anchor.to.cell` は配置範囲を指定します。画像追加後のExcelは `export --out` では `.arp/cache/export/` に生成されます。原本更新は `documents apply` を使います。画像の回転・トリミング・絶対座標・図形編集・グラフ参照の変更は未対応です。

```yaml
operations:
  - id: add-image-001
    kind: add_image
    sheet: Sheet1
    reason: "図を配置"
    asset: assets/image001.png
    anchor:
      from: {cell: B3}
      to: {cell: F15}
```

## 作業状態

| check/statusのstate | 意味 |
| --- | --- |
| needs_record | 候補が未記録、または記録後に変更された。実際の成形後にrecordする |
| ready_to_adopt | 候補の現在内容の記録があり、現在の検査で採用を妨げる条件がない。担当者が内容を確認してadoptする |
| needs_review | 採用済み文書が未レビュー、またはレビュー後に変更された |
| reviewed | 現在内容のレビュー記録がある |
| blocked | blockersにsource_changed / unresolved_mappings / authority_changedがある |
| invalid | 証跡や文書構造の検証に失敗。errorを確認する |

`check --require-reviewed` は未レビューでも失敗する。status/checkの通常実行では、needs_recordなどの作業途中の状態やblocked自体はコマンド失敗ではない。invalidは失敗する。採用した候補は削除する。別の候補が残っている場合、そのbaseと現在データが異なるためauthority_changedになる。現在の正本だけを見るなら文書IDで絞る。

## エラーとスクリプトでの利用

workflowの返信が検証で拒否された場合、`error.draft` は未受理の下書きの版を示します。同じsubmit/validate-replyに `{"draft":"<revision>","set":{"/items/1/name":"修正値"}}` を渡すと、保持した返信に差分を結合して全体を再検証します。`remove` で既存フィールドや配列要素を削除できます。全体が通るまで受理せず、修正後も不正なら最新の下書きを保存します。`draft_conflict` は古い版・異なる版、`draft_missing` は下書きなし、`invalid_draft_patch` は差分構造・操作の不正で、これらは下書きを変更しません。詳しくは [返信の検証と提出](../guides/semantic-workflow.md#返信の検証と提出) を参照してください。

エラーは `error.code/message` を持つ。コードは `invalid_arguments`（CLI引数の構文・値）、`operation_failed`（実行・前提条件）、`validation_failed`（check/statusの全体検証）。messageは診断用で、文字列一致で分岐しない。長いメッセージはUTF-8境界で最大1024バイトに制限し、`truncated: true` を返す。

返信の検証失敗（workflowのvalidate-reply/submit/update）は `reply_validation_failed` で、`error.diagnostics` に診断の配列を構造化したまま返す。診断の `path` が返信内の位置、`message` が違反した規則で、返信の値そのものは短いスカラーだけ `value` に添える。応答の上限に収まらない場合は末尾の診断を丸ごと省き、省いた件数を `error.diagnostics_omitted` に返す。診断を途中で切ることはない。submitの拒否は完全な診断をタスクに保存し、`read --pointer previous_error/diagnostics` でページ単位に読める。validate-replyは保存しないため、返された分を修正して再検証する。

- check/statusの一覧は `items` に返す。
- 通常diffは平坦な `items` を返し、比較の種類は `summary` のキーで示す。種類が複数ある場合だけ各項目に `comparison` を付ける。`--full` と保存ファイルは元の階層を維持する。
- 通常export計画の各配列は `items[].category/value` にまとめ、件数を `summary` に返す。`--full` と保存reportは元の配列を維持する。書き込み応答（`--out` あり）とapplyの `report` は件数の `summary` と `changed_parts`・`complete` だけを返し、詳細は `.report.json` にある。
- schemaの全体は `--full` の `schema` にある。通常は `--pointer /properties/<名前>` で必要な制約だけを取得する。
- `--format json` は既定なので省略できる。
- import/adopt/applyの `proposal`・`document` はプロジェクトルートからの相対パス。exportの `output`・`report` は指定した `--out` のまま返す。
- `doctor` の `limitations`（文章）は `--full` のときだけ返す。`capabilities` は常に返す。
- `--include-hashes` だけでは全件・全値の取得にはならない。保存情報は応答の省略に影響されない。

`spec assign-ids --input <入力> --model <候補> --plan <対応表> [--previous <前回モデル>] --out <採番済みモデル>` で永続IDを採番する。出力モデルと `.ids.json` 台帳を一緒に保存し、次回は --previous を指定する。対応表と分割・統合・廃止の形式は [設計書生成](specifications.md#永続idの採番) を参照。
