# Agent向けCLI応答仕様

コマンド・引数・既定値の一覧はコードから生成する [CLIコマンド・引数一覧](commands.md) を参照してください。以下は応答の読み方と操作上の判断基準です。

`doctor.cli.response_version: 1` のCLIは、通常実行の標準出力に字下げなしのJSONを1行だけ返す。`spec workflow read` の読取資料だけは `.arp/config.yml` の `agent_read_format`（既定TOON）で返す。成功は終了コード0・`ok: true`、引数エラーや実行失敗は終了コード2・`ok: false`。エラーも標準出力に返し、標準エラーとの結合は不要。`--help` と `--version` の明示要求はテキストを返す。

`ok` はコマンド自体の成否。文書の採用・書き戻し可否は `state`・`blockers`・`complete` を確認する。操作の提案や状態名は権限付与を意味しない。

## 小さな応答と詳細取得

`spec workflow` は進行管理と外部実行の機械向けJSONを返す。`next` / `inspect --task <ID>` は概要、`read --task <ID>` は指示・原文・文脈・返信契約の一括閲覧。readは `--max-bytes`（4096〜48000、既定48000）以内の設定形式のUTF-8 bytesでページへ自動分割し、本文を省略しない。同じreadの `page.next_offset` を `--offset` に指定して続ける。個別確認だけ `--pointer <path>` で絞り込む。workflowでは `--full` を使用しない。`status: blocked`（予算切れ・実行失敗など）は `ok: false`・終了コード2。修正で解消できない指摘は停止せず、レビュー後に `status: needs_decision`（草稿を `draft/` に出力、正式exportは不可）で終わる。手順は [進行管理](../guides/semantic-workflow.md)、実行先との接続は [runner契約](../guides/semantic-runners.md) を参照。

| 操作 | 既定の結果 | 詳細の取得 |
| --- | --- | --- |
| doctor | 実装・機能・応答仕様 | 同じ応答 |
| skills install / documents init | state・件数またはroot | 保存先を確認 |
| import（ファイル） | 変更時は文書ID・候補パス・抽出上の注意。暗号化を解除した場合は `unprotected`（`protection` と、IRM・秘密度ラベルでは解除した `removed`）。同じSHA-256なら `state: unchanged` と `pending_proposal`（既存候補の有無）。`--adopt` で採用した場合は候補パスの代わりに `document` と `state: imported`、採用しなかった場合は `state: needs_record` と `not_adopted`（`edited` / `reviewed`） | 再抽出が必要なら `--force`。既存候補があれば保持する |
| import（フォルダ） | imported・unchanged・pending_proposals・skipped・failed・missingの文書ID（pending_proposalsは変更なしでも残っている候補。failedは理由付き。1件でもあれば `ok: false` だが他の原本は取り込み済み）と、暗号化を解除した原本の `unprotected`。`--adopt` では採用した `adopted` と、採用しなかった `not_adopted`（文書IDと `reason`）。採用に失敗した原本はfailedに入り、候補が残る | 各候補のcontentと対応表。再抽出が必要なら `--force` |
| remove | 削除した文書ID | 残りの文書をstatusで確認 |
| discard | 破棄した候補の文書ID | 原本と採用済み文書は保持する |
| record / review | 対象ID・実行した状態 | 保存されたformation.json / review.json |
| adopt | 変更時は文書ID・本文パス・needs_review。同じ内容なら `state: skipped`・`reason: no_changes` として候補だけを除去 | 本文と管理情報。変更時は確認後にreviewを記録する |
| record / adopt / review（フォルダIDまたは `--all`） | recorded・adopted・reviewedのいずれか、skipped（状態・blockers・error・reason付き）、failed（理由付き。1件でもあれば `ok: false` だが他の文書は処理済み）、summaryの件数。処理がなければstateは `unchanged` | 一覧が省略された場合はsummaryとstatusで確認 |
| record / adopt / review `--dry-run` | planned（`--include-hashes` でcontent付き）・skipped・summary。`--out` 指定時は保存した計画のパス `plan` | 保存された計画（`batch-plan`）の全件 |
| check / status | 全体の状態別件数・対象のページ。候補には採用済み文書と内容が同じかを示す `no_changes` を含む | 対象IDで絞る、または次ページ |
| diff | 比較別の全変更件数・差分のページ | 次ページ、または `--out <新規パス>` |
| export（outなし） | 全体件数・計画のページ・complete | 次ページ、必要な場合は `--full` |
| export（outあり） | 全体件数・`state: written`・Excelとreportのパス | 保存された完全なreport |
| confirm-export | 確認した出力パス・ハッシュと確認記録 | apply前に必須 |
| apply | `confirm-export` で確認した候補の原本反映結果と再抽出候補（applyによる成形として記録され `ready_to_adopt`）。未確認なら拒否。出力が原本と同じなら原本を書き換えず候補も作らない `state: unchanged` | 候補をdiffで確認してadopt |
| values | `state`（`written`・`planned`・`unchanged`）、edit数、空のセルへの入力数 `new_cells`、書き戻し処理の試し書きの `verification`、`check`、`next_actions` | 拒否は `error.code` と `error.edit`・`error.target` |
| operations list / remove | 未適用の操作のページ / 取り消した `operation` と本文の変化（`removed_values`・`restored_values`・`reapplied_edits`・`removed_pages`・`restored_pages`）、`check` | 依存する操作は `error.dependents` |
| render | 描画した画像のパス・幅・高さ、Excelはシート・範囲、他はページと全ページ数 `pages`、候補の `output_sha256`（`--include-hashes`） | 画像を開いて確認し、confirm-exportを記録する |
| schema | プロパティ名・型・必須かどうか | `--pointer <JSON Pointer>`、必要な場合は `--full` |
| spec check | ready/blocked・種類別 summary・coverage・問題のページ | `--offset` / `--limit`、`--full`、または `--out <診断.json>` |
| spec capture / sources / prompt / schema / render | 出力パス | render は `.report.json` も保存。手順は [設計書生成](specifications.md) |
| spec semantic packet / assemble / review-packet / review-apply / finalize / changes | タスク、モデル、分類、監査、採番、変更範囲 | [意味判断に絞った生成](semantic-generation.md)。assembleの成功とreadyは別。finalizeは初回専用 |
| spec registry preflight | 正本初回登録の読み取り専用ゲート検査 | `init` 前の拒否理由確認。`ready: false` は `ok: false`・終了コード2 |

check/status/diff/export計画/schema/operations listの一覧は `items` と `page` を返す。既定は最大20件、`--limit 1..100` で調整できる。`summary` は全件を集計する。実際のページサイズはバイト制限によってさらに小さくなる。

check/statusは各文書・候補の検査結果を `.arp/cache/status/rows.json` に保存し、文書のすべての管理ファイル・原本・領域画像、候補の基になる採用済み文書、設定、プログラムのバイト列が同じ間は再利用する。ページ送りや繰り返しの確認では全ファイルのハッシュだけを計算し、変わった文書だけを検査し直す。サイズや更新日時だけでは判定しない。削除した文書の結果は次の確認で消える。保存できない場合も結果は返す。

`--out` を付けない `diff` と `export` も、ページ送りのために結果を `.arp/cache/diff/`・`.arp/cache/export/` に保存し、元にしたファイル（候補・採用済み文書・registry、またはGitのコミットと文書、exportでは文書・原本・設定）とプログラムのバイト列が同じ間は再利用する。種類ごとに新しい8件まで残す。ページに出さない `semantic_changes` は、ページ送りの応答では計算しない（`--full` と `--out` では従来どおり含める）。

構造解釈の機械推定（表の推定）は抽出結果だけで決まるため、`.arp/cache/structure/` に抽出結果のハッシュ・文書ID・原本・プログラムごとに保存し、status・check・編集・検索索引の更新で再利用する（新しい16件まで）。statusの行の `structure_ready`・`structure_conflicts`・`structure_outdated` の元になる構造解釈の再生結果（準備完了か・衝突の件数・要素の要約）も、`.arp/cache/structure-status/` に修正記録・抽出結果・行列の識別ID・原本・プログラムごとに保存し、値や行列の編集の後のように、それらが同じ間は再利用する（新しい64件まで）。画像領域を持つ修正記録は画像を読むため毎回再生する。`--out` を付けない `spec check` は、モデルのファイルがプロジェクト（上位のフォルダに `.arp/config.yml` がある）の中にあれば、検証結果を `.arp/cache/spec-check/` に入力・モデルのファイルとプログラムのハッシュごとに保存し、ページ送りで再利用する（新しい8件まで）。プロジェクトの外では毎回検証する。

文書の検査に成功すると、その管理ファイルの組を `.arp/cache/inspection/` に全ファイルのハッシュ・設定・プログラムごとに記録する（新しい64件まで）。status・check・`structure-read`・`elements-read`・`extraction-read`・検索索引の更新・diff・record・review・adoptは、同じファイルを読み直す間、本文の値・対応表・範囲の検証と本文ページの読み込みを省き、記録した未解決の対応表の件数・本文と抽出結果の違い・ページの一覧・構造の要約を使う。文書の置き場所とID、原本のハッシュ、レビューは毎回確かめる。編集・export・applyは毎回すべてを検証する。`.arp/cache/` の記録は失われても作り直すため、保存時にディスクへの書き込み完了を待たない。

```json
{"items":[],"ok":true,"page":{"next_offset":null,"offset":0,"returned":0,"total":0},"summary":{}}
```

`page.next_offset` が数値なら、同じ対象・条件で `--offset <値>` を指定する。nullなら次のページはない。ページング中に対象が変化した場合は先頭から取得し直す。`--offset` が件数以上なら空ページとなる。ページングは出力のみを制限し、検証は全対象に実行する。表示外の不正な文書も失敗として全体のsummaryと終了コードに反映される。

通常の応答は改行を含め最大16 KiB（workflowのread/inspect/handoff/receipt/next/validate-replyと `spec structure` の各コマンドは対象外。readは `--max-bytes` で分割）、各一覧項目は約2 KiB。大きなフィールドは値を切り詰めずフィールドごと省略し、`omitted_fields` にその名前を列挙する。項目の `index` は全体での0始まりの位置。オブジェクト全体を表示できない場合は `omitted: true` と元のJSONバイト数を返す。これは元データの空値やnullではない。ページが最終でも、値が省略されていれば全内容を確認したことにはならない。

完全な差分は `documents diff ... --out <新規JSONパス>` で保存でき、保存応答には `report` を返す。JSONファイルは `comparisons[].changes`（本文パス）と `comparisons[].semantic_changes`（原本位置での値変更）を持ち、値の省略やページングはしない。`--format markdown` は `--out` との併用だけに対応する。

`--full` はサイズ制限とページングを解除する明示的な選択で、`--limit/--offset` とは併用できない。読み取り操作で必要な場合に使い、巨大な結果はファイルへリダイレクトする。record/review/import/adopt/exportの書き込みを、応答の再取得のために繰り返してはならない。保存された証跡・本文・reportを読む。exportの書き込み応答はページを返さず、`--out` と `--limit/--offset` の併用を拒否する。

`--include-hashes` はrecord/review/check/status/export/values/rows/columns/renderに検証ハッシュを含める。check/statusでは本文の `content` と原本の `source_sha256` を返し、どちらも `documents values` の要求の `base`・`source_sha256` に使う。`--full` とは独立しており、完全な検証情報が必要なら両方を指定する。diffのハッシュで表された変更は差分そのものとして維持される。保存形式と検証に用いる完全なSHA-256は変更しない。

## 行・列の構造変更

行・列の追加と削除は `documents rows insert|delete` と `documents columns insert|delete` で行います。Word文書とPowerPoint文書では `documents rows` が表の外の段落と表の行を追加・削除します（`--sheet` はWordでは `document` などのパーツ名、PowerPointでは `slide-N`・`notes-N` または追加したスライドの操作ID、行は本文YAMLの `rN`）。PowerPointの新しい段落は、書式元（`--style-from`、既定は上の行）の段落と同じ図形に入ります。対象は採用済み文書（文書ID）または候補（`--proposal <文書ID>`）です。1回の実行で、次の手順をまとめて処理します。

- `mappings.yml` の `operations` への操作の追記
- 追加分の値の本文への書き込み（追加した行・列の識別IDは `<操作ID>-<番号>`）
- 削除される原本セルの値の、本文からの除去

書き込む前に、変更後の状態を `check` と同じ検証にかけます。ActiveX コントロールを含むブックなど export が拒否する条件も確かめます。VBA プロジェクトを持つブックは、VBA コード内のセル番地を書き換えないことを了承する `--vba-unchanged` がなければ `vba_not_updated` で拒否します（[Office・PDF文書](../guides/documents.md)）。拒否した場合は何も書き込みません。文書フォルダは検証済みのコピーと rename で入れ替えるため、途中で失敗しても `mappings.yml` と本文が食い違ったまま残ることはありません。

- 位置: `--after` / `--before` には、原本シートの行番号（`15` または `r15`）、列名（`C`）、先に挿入した行・列のキー（`<操作ID>-<番号>`）、または `last`（値のある最後の行・列）を指定します。行番号は原本シートの番号のままで、記録済みの操作による移動を考慮した `at` はCLIが計算します。値のある最後の行・列より後ろの番号や、記録済みの操作で削除された行・列は `invalid_position` で拒否します。
- 値: `--values` には、1行（列）につき1オブジェクトのリストを YAML または JSON で渡します（ファイル、または `-` で標準入力）。行は列名、列は `r<行番号>` がキーです。`--count` を省くと項目数が使われ、両方を指定して数が合わなければ `count_mismatch` になります。行の値は、同じ列の既存値の型に揃えます。基準は `--style-from` の行の値で、空ならその上で最も近い値です。文字列の列の数値は文字列に、日付書式の列の `YYYY/MM/DD` や `YYYY-MM-DD` はシリアル値にします。揃えられない値は `value_type_mismatch` で拒否します。変換した値は応答の `converted` に返します。追加する列の値は、渡したまま保存します。
- 書式: `--style-from` を省くと、挿入位置の上の行を使います（Excelと同じ）。書き戻しでは、その行の空のセルを含む全セルの書式を追加行に写し、`dimension` を追加行まで広げます。Excelでは `--style-from 9:10` のように範囲を指定すると、追加行は組ごとにその行を順に書式元にします（行数は組の倍数でなければ `invalid_count`）。操作には `style_from` と `style_rows` を記録します。
- 結合: `--merges`（Excelのみ）を付けると、書式元の行の内側にある結合を、追加行の各組の同じ位置にも作ります（Excelの「コピーしたセルの挿入」と同じ。操作の `merges: true`）。付けない場合はExcelの行挿入と同じく、結合は作りません。写した結合の左上以外への値は `merged_non_anchor` で拒否し、写す結合が既存の結合（挿入で広がった結合を含む）と重なる場合は検証で拒否します。後の操作は写した結合も他の結合と同じく移動します。
- 再実行と競合: `--id` を指定した同じコマンドの再実行は `state: unchanged` を返し、何も書き込みません。同じIDで設定や値が違う場合は `operation_id_conflict` です。`--base` には `check --include-hashes` の `content` を渡します。その後に文書が変わっていれば `base_changed` で拒否します。
- 応答: 成功時は `state`（`written`、`--dry-run` なら `planned`、再実行なら `unchanged`）、`operation`、`span`（反映後の最初と最後の行・列）、`keys`、`converted`、`vba`（`--vba-unchanged` を指定したとき）、`neighbors`（行の操作のみ。前後にある本文の行）、`check`（書き込み後の検証状態）、`next_actions` を返します。
- エラーコード: `invalid_document`、`base_changed`、`unsupported_format`、`sheet_not_found`（`error.sheets` にシート名の一覧）、`source_changed`、`invalid_operation_id`、`operation_id_conflict`、`invalid_position`、`count_mismatch`、`invalid_values`、`value_type_mismatch`、`merged_non_anchor`、`category_row_deleted`（本文の区分の値を持つ行を、区分の他の記録を残して削除する。Excelが区分の値をその行と一緒に消すため）、`structural_edit_unsupported`、`vba_not_updated`（VBAプロジェクトを持つブックで `--vba-unchanged` がない。`error.vba` にセルを指している可能性のあるコード行）、`invalid_arguments`（VBAプロジェクトのない文書への `--vba-unchanged` 等）、`invalid_reason`（空の `--reason`）、`invalid_count`（`--count` が0）、`validation_failed`。

`documents export` は、`mappings.yml` の `operations` に記録した行・列の操作と本文の値を組み合わせてExcelへ反映します。行・列の操作は `insert_rows`、`delete_rows`、`insert_columns`、`delete_columns`、`move_columns`（移動先 `to` も持つ）で、`documents rows|columns` が記録します。各操作は `id`、`sheet`、`reason`、`at`、`count` を持ち、行の追加では `style_from` に書式をコピーする元の行を記録します（非表示・折りたたみはコピーしません）。追加した列はExcelと同じく左の列の書式を引き継ぐため、列の操作は `style_from` を持ちません。

追加した行・列の識別IDは `<操作ID>-<番号>`（番号は1から `count` まで）で、`--after` / `--before` の位置や応答の `keys` に使います。本文は表を記録の木として持ち、追加した行は見出しをキーにした記録のノードとして入ります。行番号・列名・識別IDは本文のキーになりません。本文の記録・列の数や木の形を手で変えると検証で拒否するため、行・列の追加・削除は必ず `documents rows|columns` で行い、削除した行・列の値の除去も同じコマンドに任せます。追加した行と列が交わるセルへの書き込みは検証で拒否します。

結合範囲の内側への挿入は、Excelと同じく結合を広げます。広がった結合の左上以外になるセルはExcelに表示されないため、そこへの書き込みは検証と書き戻しで拒否します。値は結合の左上に書くか、結合の外に行・列を追加してください。

構造変更のexport reportには、反映した `operations`、値の `changes`、数式の `formula_changes`（原本の位置 `cell` と出力での位置 `final_cell`）、削除された原本セルの `deleted_cells` が保存されます。`vba_unchanged` の操作があれば、VBAのコード行の一覧 `vba` も保存されます。全シートの数式参照・名前定義・テーブル・条件付き書式・入力規則は移動し、移動した数式のキャッシュは無効化されます。既存のDrawing XMLにある画像・図形のセルアンカーも行・列操作に追従します。

PNG画像は、文書の `assets/` に置いたファイルを `documents images add <文書ID> --sheet <シート> --from <左上のセル> --to <右下のセル> --asset assets/image001.png --reason <理由>` で追加します（候補は `--proposal <文書ID>`、`--id`・`--base`・`--dry-run` は行の操作と同じ）。`mappings.yml` の `operations` に次の `add_image` 操作を記録し、試し書きと `check` と同じ検証に通った場合だけ書き込みます。画像は左上のセルと一緒に移動し、大きさを保ちます。画像追加後のExcelは `export --out` では `.arp/cache/export/` に生成されます。原本更新は `documents apply` を使います。画像の回転・トリミング・絶対座標・図形編集・グラフ参照の変更は未対応です。

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

## 値の編集

採用済み文書（文書ID）または候補（`--proposal <文書ID>`）の値・数式・図形の文字・フォントは `documents values <文書ID> --input <要求ファイル>` で変えます。要求は `documents schema value-edits` の契約に従うJSONまたはYAML（`-` で標準入力）で、`check --include-hashes` の `content` を `base`、`source_sha256` をそのまま入れます。各editは `sheet` と、`cell`（`extraction-read` の出力の原本の番地）・`shape`（Excelの図形の抽出ID）・`element`（`elements-read` の出力の要素ID）のいずれかで対象を指定し、値は `before`（期待する現在値。値のないセルは `null`）と `after`、フォントは `font: {before, after}` で書きます。

- 未適用の行・列・スライドの操作があっても編集できます。挿入した行・複製したスライドの値は `element`（`cell_<操作ID>-1_C` など）で指定します。
- Excelでは原本で空のセルにも値を入れられます。表の空欄・挿入した行の未入力のセルは同じ規則の要素ID、原本の範囲の空のセルは番地で指定し、表の範囲の内側なら表の記録に、外なら本文の要素になります。シートの行・列の範囲外、結合の左上以外のセルは拒否します。
- 全editを未適用の操作とともに文書形式ごとの書き戻し処理で試し書きし、1件でも拒否されれば何も保存しません。成功した要求のactor・reason・editsは `mappings.yml` の `value_edits` に記録し、exportのreportにも載せます。同じ値を指定し直した要求は `unchanged` で、記録しません。
- エラーコード: `invalid_request`、`invalid_document`、`base_changed`、`source_changed`、`unsupported_format`、`sheet_not_found`（`error.sheets`）、`target_not_found`、`not_editable`、`duplicate_target`、`before_mismatch`（`error.current`）、`value_type_mismatch`、`font_unsupported`、`validation_failed`、`writeback_refused`。editを特定できる拒否は `error.edit`（`edits` の添字）と `error.target` を持ちます。`writeback_refused` では、32件以内の要求なら単独でも拒否されるeditの添字を `error.edits` に返します。

## 操作の取り消し

`documents operations list <文書ID>`（候補は `--proposal`）は未適用の操作を記録順に返します。`documents operations remove <文書ID> --id <操作ID> [--base <content>] [--dry-run]` は操作を本文への影響とともに取り消し、`check` と同じ検証に通った場合だけ書き込みます。

- 挿入（`insert_rows`・`insert_columns`・`insert_slide`・`add_slide`）を取り消すと、追加した行・列の値とページを本文から除き、それを対象にした `value_edits` の記録も除きます。
- 削除（`delete_rows`・`delete_columns`・`delete_slide`）を取り消すと、削除した行・列・スライドの値を原本の抽出の値で戻し、表の所属は構造解釈から戻します。戻した値を対象にした `value_edits` の記録（値・フォントの編集）は記録順に当て直し、件数を `reapplied_edits` に返します。記録の変更前が戻した値と一致しない編集は当て直しません。スライドの削除で一緒に取り除かれた行の操作も戻りません。
- その他の操作（列の移動、スライドの移動・表示、図形・画像の操作）は操作だけを取り除きます。
- 同じシートで後に記録した行・列・画像の操作、または取り消す操作が追加したスライド・図形を指す操作があれば `operation_has_dependents`（`error.dependents`）で拒否します。後の操作から取り消してください。存在しない操作IDは `operation_not_found`（`error.operations`）です。

## export候補の描画

`documents render <文書ID> --candidate <exportの出力> --out <新規PNG>` は、export が書いた候補（隣に `.report.json` があり、そのハッシュと一致するもの）のページをPNGに描画します。Excelは `--sheet`（必須）と `--range`（省略時は使用範囲）をExcelで（report が `requires_excel_recalculation` の候補は全再計算してから。応答の `recalculated`）、WordとPowerPointはOfficeが保存したPDFの `--page`（スライドは非表示のものも含めた候補での順序）を、PDFはその `--page` をWindowsで描画します。WindowsとデスクトップOffice（Excel、Word・PowerPointの文書ではそのアプリ）が必要です。描画は確認の材料であり、画像を開いて実際に確認した範囲だけを `confirm-export` に記録します。

## スライドの追加・削除

PowerPoint文書のスライドは `documents slides insert|delete` で複製・削除します。レイアウトからの追加（`slides add`）、移動（`slides move`）、表示・非表示（`slides hide|show`）、図形の変更（`documents shapes`）の引数は [CLIコマンド・引数一覧](commands.md) と[操作リファレンス](../../surface/skills/arp4/references/operations.md)を参照してください。行の操作と同じく、採用済み文書（文書ID）または候補（`--proposal <文書ID>`）を対象に、`mappings.yml` の `operations` への追記と本文のページの追加・削除を1回の実行で行い、`check` と同じ検証と export の事前確認に通った場合だけ書き込みます。

- 追加: `documents slides insert <文書ID> --from <スライド> --after|--before <スライド> --reason <理由>` は、`--from` のスライドを PowerPoint の「スライドの複製」と同じく複製して、指定した位置に置きます。追加したスライドの名前は操作ID（`--id`、省略時は `add-slide-<番号>`）です。本文には `content/<操作ID>.yml` を作り、複製元のページの現在の値（編集済みの値を含む）を写します。複製元にノートがあれば、ノートも複製して `content/notes-<操作ID>.yml` を作ります。追加したスライドの文字は、`documents values` で `sheet` に操作ID、`element` に `documents elements-read` のそのページの要素IDを指定して変えます。行の操作を記録したスライドは複製元にできません（`invalid_slide`）。先に複製してから行を追加・削除するか、`documents operations remove` でその操作を取り除いてください。
- 削除: `documents slides delete <文書ID> --slide slide-<番号> --reason <理由>` は、原本のスライドを、そのノートと本文のページとともに削除します。そのスライドとノートに記録した行の操作も取り除き、応答の `dropped_operations` に操作IDを返します。追加したスライドは削除せず、それを追加した操作を `documents operations remove` で取り除いてください。
- スライドの指定: 原本のスライドは `slide-<番号>`（原本での番号）、追加したスライドは操作IDで指定します。番号は操作後も変わりません。操作は記録順に適用し、先の操作で削除したスライドや後の操作で追加するスライドは指定できません。最後の1枚は削除できません。
- 複製する部品: レイアウト・マスター・画像・動画・ハイパーリンク先は元のスライドと共有し、グラフ（埋め込みブックを含む）・SmartArt・埋め込みオブジェクト・ノートなどそのスライドだけの部品は新しい名前で複製します。コメントは元のスライドへのレビューのため複製しません。セクションは隣のスライドと同じセクションに入ります。
- 削除する部品: スライドとそのスライドからしか参照されない部品（ノート・グラフ・コメント等）を取り除き、セクション・目的別スライドショー・アウトライン表示の一覧からも外します。
- 拒否: 他のスライドのハイパーリンクや動作設定から参照されているスライドの削除と、目的別スライドショーが空になる削除は `structural_edit_unsupported` で拒否します。リンクを PowerPoint で外してから削除してください。操作IDがスライド名・ノート名と重なる場合、数字だけの場合、存在しないスライドを指定した場合は `invalid_slide` です。
- 再実行と競合: `--id` を指定した同じコマンドの再実行は `state: unchanged` を返します。同じIDで設定が違う場合は `operation_id_conflict` です。`--base` と `--dry-run` は行の操作と同じです。
- 応答: 成功時は `state`、`operation`、`added_pages`・`removed_pages`（追加・削除した本文ファイル）、`slides`（操作後のスライドの順序）、`check`、`next_actions` を返します。
- エラーコード: `invalid_document`、`base_changed`、`unsupported_format`（PowerPoint以外の文書）、`source_changed`、`invalid_reason`、`invalid_operation_id`、`operation_id_conflict`、`invalid_slide`、`invalid_layout`（`slides add` の `--layout` がレイアウトにない、または同名が複数）、`page_exists`、`structural_edit_unsupported`、`validation_failed`。

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
| imported | `import --adopt` で抽出したまま採用し、その後の編集がない。レビュー対象外（一括reviewはskipする）。編集するとneeds_review |
| reviewed | 現在内容のレビュー記録がある |
| blocked | blockersにsource_missing / source_changed / unresolved_mappings / authority_changedがある |
| invalid | 証跡や文書構造の検証に失敗。errorを確認する |

`check --require-reviewed` は未レビューでも失敗する（importedも未レビューとして扱う）。status/checkの通常実行では、needs_recordなどの作業途中の状態やblocked自体はコマンド失敗ではない。invalidは失敗する。採用した候補は削除する。別の候補が残っている場合、そのbaseと現在データが異なるためauthority_changedになる。現在の正本だけを見るなら文書IDで絞る。

## エラーとスクリプトでの利用

workflowの返信が検証で拒否された場合、`error.draft` は未受理の下書きの版を示します。同じsubmit/validate-replyに `{"draft":"<revision>","set":{"/items/1/name":"修正値"}}` を渡すと、保持した返信に差分を結合して全体を再検証します。`remove` で既存フィールドや配列要素を削除できます。全体が通るまで受理せず、修正後も不正なら最新の下書きを保存します。`draft_conflict` は古い版・異なる版、`draft_missing` は下書きなし、`invalid_draft_patch` は差分構造・操作の不正で、これらは下書きを変更しません。詳しくは [返信の検証と提出](../guides/semantic-workflow.md#返信の検証と提出) を参照してください。

エラーは `error.code/message` を持つ。コードは `invalid_arguments`（CLI引数の構文・値）、`operation_failed`（実行・前提条件）、`validation_failed`（check/statusの全体検証）、`partial_search`（searchが不正な文書を除外した。`failed` を確認）、`search_response_too_large`（複数クエリの応答が上限を超える）。messageは診断用で、文字列一致で分岐しない。長いメッセージはUTF-8境界で最大1024バイトに制限し、`truncated: true` を返す。

返信の検証失敗（workflowのvalidate-reply/submit/update）は `reply_validation_failed` で、`error.diagnostics` に診断の配列を構造化したまま返す。診断の `path` が返信内の位置、`message` が違反した規則で、返信の値そのものは短いスカラーだけ `value` に添える。応答の上限に収まらない場合は末尾の診断を丸ごと省き、省いた件数を `error.diagnostics_omitted` に返す。診断を途中で切ることはない。submitの拒否は完全な診断をタスクに保存し、`read --pointer previous_error/diagnostics` でページ単位に読める。validate-replyは保存しないため、返された分を修正して再検証する。

- check/statusの一覧は `items` に返す。
- 通常diffは平坦な `items` を返し、比較の種類は `summary` のキーで示す。種類が複数ある場合だけ各項目に `comparison` を付ける。`--full` と保存ファイルは元の階層を維持し、`semantic_changes` もそちらにだけある。
- 通常export計画の各配列は `items[].category/value` にまとめ、件数を `summary` に返す。`--full` と保存reportは元の配列を維持する。書き込み応答（`--out` あり）とapplyの `report` は件数の `summary`・`changed_parts`・`complete` などの書き戻し情報を返し、詳細は `.report.json` にある。
- schemaの全体は `--full` の `schema` にある。通常は `--pointer /properties/<名前>` で必要な制約だけを取得する。
- `--format json` は既定なので省略できる。
- import/adopt/applyの `proposal`・`document` はプロジェクトルートからの相対パス。exportの `output`・`report` は指定した `--out` のまま返す。
- `doctor` の `limitations`（文章）は `--full` のときだけ返す。`capabilities` は常に返す。
- `--include-hashes` だけでは全件・全値の取得にはならない。保存情報は応答の省略に影響されない。

`spec assign-ids --input <入力> --model <候補> --plan <対応表> [--previous <前回モデル>] --out <採番済みモデル>` で永続IDを採番する。出力モデルと `.ids.json` 台帳を一緒に保存し、次回は --previous を指定する。対応表と分割・統合・廃止の形式は [設計書生成](specifications.md#永続idの採番) を参照。
