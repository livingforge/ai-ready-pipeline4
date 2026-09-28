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
| import（ファイル） | 変更時は文書ID・候補パス・抽出上の注意。同じSHA-256なら `state: unchanged` と `pending_proposal`（既存候補の有無） | 再抽出が必要なら `--force`。既存候補があれば保持する |
| import（フォルダ） | imported・unchanged・pending_proposals・skipped・failed・missingの文書ID（pending_proposalsは変更なしでも残っている候補。failedは理由付き。1件でもあれば `ok: false` だが他の原本は取り込み済み） | 各候補のcontentと対応表。再抽出が必要なら `--force` |
| remove | 削除した文書ID | 残りの文書をstatusで確認 |
| discard | 破棄した候補の文書ID | 原本と採用済み文書は保持する |
| record / review | 対象ID・実行した状態 | 保存されたformation.json / review.json |
| adopt | 変更時は文書ID・本文パス・needs_review。同じ内容なら `state: skipped`・`reason: no_changes` として候補だけを除去 | 本文と管理情報。変更時は確認後にreviewを記録する |
| record / adopt / review（フォルダIDまたは `--all`） | recorded・adopted・reviewedのいずれか、skipped（状態・blockers・error・reason付き）、failed（理由付き。1件でもあれば `ok: false` だが他の文書は処理済み）、summaryの件数。処理がなければstateは `unchanged` | 一覧が省略された場合はsummaryとstatusで確認 |
| record / adopt / review `--dry-run` | planned（`--include-hashes` でcontent付き）・skipped・summary。`--out` 指定時は保存した計画のパス `plan` | 保存された計画（`batch-plan`）の全件 |
| check / status | 全体の状態別件数・対象のページ。候補には採用済み文書と内容が同じかを示す `no_changes` を含む | 対象IDで絞る、または次ページ |
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

`--include-hashes` はrecord/review/check/status/export/rows/columnsに検証ハッシュを含める。`--full` とは独立しており、完全な検証情報が必要なら両方を指定する。diffのハッシュで表された変更は差分そのものとして維持される。保存形式と検証に用いる完全なSHA-256は変更しない。

## 行・列の構造変更

行・列の追加と削除は `documents rows insert|delete` と `documents columns insert|delete` で行います。Word文書とPowerPoint文書では `documents rows` が表の外の段落と表の行を追加・削除します（`--sheet` はWordでは `document` などのパーツ名、PowerPointでは `slide-N`・`notes-N` または追加したスライドの操作ID、行は本文YAMLの `rN`）。PowerPointの新しい段落は、書式元（`--style-from`、既定は上の行）の段落と同じ図形に入ります。対象は採用済み文書（文書ID）または候補（`--proposal <文書ID>`）です。1回の実行で、次の手順をまとめて処理します。

- `mappings.yml` の `operations` への操作の追記
- 追加分の値の、本文への `<操作ID>-<番号>` キーでの書き込み
- 削除される原本セルの値の、本文からの除去

書き込む前に、変更後の状態を `check` と同じ検証にかけます。VBA を含むブックなど export が拒否する条件も確かめます。拒否した場合は何も書き込みません。文書フォルダは検証済みのコピーと rename で入れ替えるため、途中で失敗しても `mappings.yml` と本文が食い違ったまま残ることはありません。

- 位置: `--after` / `--before` には、原本シートの行番号（`15` または `r15`）、列名（`C`）、先に挿入した行・列のキー（`<操作ID>-<番号>`）、または `last`（値のある最後の行・列）を指定します。行番号は原本シートの番号のままで、記録済みの操作による移動を考慮した `at` はCLIが計算します。値のある最後の行・列より後ろの番号や、記録済みの操作で削除された行・列は `invalid_position` で拒否します。
- 値: `--values` には、1行（列）につき1オブジェクトのリストを YAML または JSON で渡します（ファイル、または `-` で標準入力）。行は列名、列は `r<行番号>` がキーです。`--count` を省くと項目数が使われ、両方を指定して数が合わなければ `count_mismatch` になります。行の値は、同じ列の既存値の型に揃えます。基準は `--style-from` の行の値で、空ならその上で最も近い値です。文字列の列の数値は文字列に、日付書式の列の `YYYY/MM/DD` や `YYYY-MM-DD` はシリアル値にします。揃えられない値は `value_type_mismatch` で拒否します。変換した値は応答の `converted` に返します。追加する列の値は、渡したまま保存します。
- 書式: `--style-from` を省くと、挿入位置の上の行を使います（Excelと同じ）。書き戻しでは、その行の空のセルを含む全セルの書式を追加行に写し、`dimension` を追加行まで広げます。
- 再実行と競合: `--id` を指定した同じコマンドの再実行は `state: unchanged` を返し、何も書き込みません。同じIDで設定や値が違う場合は `operation_id_conflict` です。`--base` には `check --include-hashes` の `content` を渡します。その後に文書が変わっていれば `base_changed` で拒否します。
- 応答: 成功時は `state`（`written`、`--dry-run` なら `planned`、再実行なら `unchanged`）、`operation`、`span`（反映後の最初と最後の行・列）、`keys`、`converted`、`neighbors`（行の操作のみ。前後にある本文の行）、`check`（書き込み後の検証状態）、`next_actions` を返します。
- エラーコード: `invalid_document`、`base_changed`、`unsupported_format`、`sheet_not_found`（`error.sheets` にシート名の一覧）、`source_changed`、`invalid_operation_id`、`operation_id_conflict`、`invalid_position`、`count_mismatch`、`invalid_values`、`value_type_mismatch`、`merged_non_anchor`、`structural_edit_unsupported`、`validation_failed`。

`documents export` は本文YAMLの追加・削除を、管理側 `mappings.yml` の `operations` と組み合わせてExcelへ反映できます。対応する操作は `insert_rows`、`delete_rows`、`insert_columns`、`delete_columns` です。各操作には `id`、`sheet`、`reason`、`at`、`count` を指定し、行の追加時は必要に応じて `style_from` で書式をコピーする元の行を指定します（非表示・折りたたみはコピーしません）。追加した列はExcelと同じく左の列の書式を引き継ぐため、列の操作には `style_from` を指定できません。

操作で追加する行・列は、本文の `blocks.<block>.rows` に `<操作ID>-<番号>`（番号は1から `count` まで）のキーで書きます。`id: add` の `insert_rows` で追加した1行目は `add-1: {B: 新項目}`、`insert_columns` で追加した列の値は既存行の `r3: {add-1: 値}` のように書きます。既存の行 `r<number>` と列の英字は原本の位置を指し、操作後も変わらないため、追加した行・列と同じキーにはなりません。CLIはこのキーから挿入操作を特定してmapping entryを再生成します。削除時は本文から対象の行・列を削除し、対応するdelete操作を残します。操作のない追加・削除、原本で値のないセルへの書き込み、操作の範囲外の番号、追加した行と列が交わるセルは検証で拒否します。

結合範囲の内側への挿入は、Excelと同じく結合を広げます。広がった結合の左上以外になるセルはExcelに表示されないため、そこへの書き込みは検証と書き戻しで拒否します。値は結合の左上に書くか、結合の外に行・列を追加してください。

構造変更のexport reportには、反映した `operations`、値の `changes`、数式の `formula_changes`（原本の位置 `cell` と出力での位置 `final_cell`）、削除された原本セルの `deleted_cells` が保存されます。全シートの数式参照・名前定義・テーブル・条件付き書式・入力規則は移動し、移動した数式のキャッシュは無効化されます。既存のDrawing XMLにある画像・図形のセルアンカーも行・列操作に追従します。

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

## スライドの追加・削除

PowerPoint文書のスライドは `documents slides insert|delete` で追加・削除します。行の操作と同じく、採用済み文書（文書ID）または候補（`--proposal <文書ID>`）を対象に、`mappings.yml` の `operations` への追記と本文のページの追加・削除を1回の実行で行い、`check` と同じ検証と export の事前確認に通った場合だけ書き込みます。

- 追加: `documents slides insert <文書ID> --from <スライド> --after|--before <スライド>` は、`--from` のスライドを PowerPoint の「スライドの複製」と同じく複製して、指定した位置に置きます。追加したスライドの名前は操作ID（`--id`、省略時は `add-slide-<番号>`）です。本文には `content/<操作ID>.yml` を作り、複製元のページの現在の値（編集済みの値を含む）を写します。複製元にノートがあれば、ノートも複製して `content/notes-<操作ID>.yml` を作ります。追加したスライドの文字は、このページの `rN/A` を通常どおり編集して書き戻します。
- 削除: `documents slides delete <文書ID> --slide slide-<番号>` は、原本のスライドを、そのノートと本文のページとともに削除します。追加したスライドは削除せず、それを追加した操作を取り除いてください。
- スライドの指定: 原本のスライドは `slide-<番号>`（原本での番号）、追加したスライドは操作IDで指定します。番号は操作後も変わりません。操作は記録順に適用し、先の操作で削除したスライドや後の操作で追加するスライドは指定できません。最後の1枚は削除できません。
- 複製する部品: レイアウト・マスター・画像・動画・ハイパーリンク先は元のスライドと共有し、グラフ（埋め込みブックを含む）・SmartArt・埋め込みオブジェクト・ノートなどそのスライドだけの部品は新しい名前で複製します。コメントは元のスライドへのレビューのため複製しません。セクションは隣のスライドと同じセクションに入ります。
- 削除する部品: スライドとそのスライドからしか参照されない部品（ノート・グラフ・コメント等）を取り除き、セクション・目的別スライドショー・アウトライン表示の一覧からも外します。
- 拒否: 他のスライドのハイパーリンクや動作設定から参照されているスライドの削除と、目的別スライドショーが空になる削除は `structural_edit_unsupported` で拒否します。リンクを PowerPoint で外してから削除してください。操作IDがスライド名・ノート名と重なる場合、数字だけの場合、存在しないスライドを指定した場合は `invalid_slide` です。
- 再実行と競合: `--id` を指定した同じコマンドの再実行は `state: unchanged` を返します。同じIDで設定が違う場合は `operation_id_conflict` です。`--base` と `--dry-run` は行の操作と同じです。
- 応答: 成功時は `state`、`operation`、`added_pages`・`removed_pages`（追加・削除した本文ファイル）、`slides`（操作後のスライドの順序）、`check`、`next_actions` を返します。
- エラーコード: `invalid_document`、`base_changed`、`unsupported_format`（PowerPoint以外の文書）、`source_changed`、`invalid_reason`、`invalid_operation_id`、`operation_id_conflict`、`invalid_slide`、`page_exists`、`structural_edit_unsupported`、`validation_failed`。

記録される操作は次の形です。`insert_slide` は `after` と `before` のどちらか一方を持ちます。

```yaml
operations:
  - {id: cover, kind: insert_slide, from: slide-1, before: slide-1, reason: 表紙を追加}
  - {id: del-slide-1, kind: delete_slide, slide: slide-3, reason: 不要なスライド}
```

export は出力を読み直し、スライドとノートの順序と文字列が操作と編集どおりであることを確かめてから書き込みます。export report の `slide_order` に出力のスライドの順序を記録します。

## 作業状態

| check/statusのstate | 意味 |
| --- | --- |
| needs_record | 候補が未記録、または記録後に変更された。実際の成形後にrecordする |
| ready_to_adopt | 候補の現在内容の記録があり、現在の検査で採用を妨げる条件がない。adoptで現在版にする |
| needs_review | 採用直後、またはレビュー後に変更された。確認した担当者がreviewする |
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
