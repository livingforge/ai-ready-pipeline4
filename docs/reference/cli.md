# Agent向けCLI応答仕様

コマンド・引数・既定値の一覧はコードから生成する [CLIコマンド・引数一覧](commands.md) を参照してください。以下は応答の読み方と操作上の判断基準です。

`doctor.cli.response_version: 1` のCLIは、通常実行の標準出力に字下げなしのJSONを1行だけ返す。`spec workflow read` の読取資料だけは `.arp/config.yml` の `agent_read_format`（既定TOON）で返す。成功は終了コード0・`ok: true`、引数エラーや実行失敗は終了コード2・`ok: false`。エラーも標準出力に返し、標準エラーとの結合は不要。`--help` と `--version` の明示要求はテキストを返す。

`ok` はコマンド自体の成否。文書の採用・書き戻し可否は `state`・`blockers`・`complete` を確認する。操作の提案や状態名は権限付与を意味しない。

## 引数の誤り

コマンドが受け付けない引数のうち、省いても結果が変わらないものだけは受け付けて実行し、応答の `ignored_options`（`option` と `reason` の配列）に示す。対象は、`doctor` の `--root`、`documents` 以外の `--include-hashes`、`--format json`（JSONを返すコマンドに限る。`spec workflow read` は除く）、ページ送りのない応答の `--limit`。`--offset` などそれ以外の誤りは `invalid_arguments` で拒否する。`--dry-run`・`--base`・`--expect`・`--proposal`・`--reason` のように、無視すると対象や安全確認、記録が変わる引数を黙って無視しないためである。

`invalid_arguments` の `error` には、`message` に加えて、解析が到達したサブコマンド（`command`）、そのコマンドが受け付けるオプション（`accepted`）、サブコマンドがあればその一覧（`subcommands`）を返す。受け付けない引数があればその名前を `argument` に返す。外側のコマンドの引数をサブコマンドの後に置いた場合（`spec workflow status --root .` など）は、正しい位置を `hint` に返す。

プロジェクト内で実行した場合は、拒否した引数と無視した引数の回数を `.arp/cache/argument-errors.json` の `rejected`・`ignored` に「サブコマンド 引数」の形で数える。無視する対象を見直すときの材料であり、Agentの手順では読まない。プロジェクト外では何も書かない。

## 小さな応答と詳細取得

`spec workflow` は進行管理と外部実行の機械向けJSONを返す。`next` / `inspect --task <ID>` は概要、`read --task <ID>` は指示・原文・文脈・返信契約の一括閲覧。readは `--max-bytes`（4096〜48000、既定48000）以内の設定形式のUTF-8 bytesでページへ自動分割し、本文を省略しない。同じreadの `page.next_offset` を `--offset` に指定して続ける。個別確認だけ `--pointer <path>` で絞り込む。workflowでは `--full` を使用しない。`status: blocked`（予算切れ・実行失敗など）は `ok: false`・終了コード2。修正で解消できない指摘は停止せず、レビュー後に `status: needs_decision`（草稿を `draft/` に出力、正式exportは不可）で終わる。手順は [進行管理](../guides/semantic-workflow.md)、実行先との接続は [runner契約](../guides/semantic-runners.md) を参照。

| 操作 | 既定の結果 | 詳細の取得 |
| --- | --- | --- |
| doctor | 実装・機能・応答仕様 | 同じ応答 |
| skills install / documents init | state・件数またはroot | 保存先を確認 |
| import（ファイル） | 変更時は文書ID・候補パス・抽出上の注意。暗号化を解除した場合は `unprotected`（`protection` と、IRM・秘密度ラベルでは解除した `removed`）。同じSHA-256なら `state: unchanged` と `pending_proposal`（既存候補の有無）。`--adopt` で採用した場合は候補パスの代わりに `document` と `state: imported`、採用しなかった場合は `state: needs_record` と `not_adopted`（`edited` / `reviewed`）。採用に失敗した場合は `ok: false` で、エラーに段階と対象パスを含み、候補が残る（`documents adopt` で採用できる） | 再抽出が必要なら `--force`。既存候補があれば保持する |
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
| export（outあり） | 全体件数・`state: written`・Excelとreportのパス。Excelで `--calculate` を付けると、デスクトップExcelが計算した数式の結果を候補に書き込み、`calculation`（`results`・`missing`・Excelの `version`）を返す | 保存された完全なreport |
| confirm-export | 確認した出力パス・ハッシュと確認記録 | apply前に必須 |
| apply | `confirm-export` で確認した候補の原本反映結果と再抽出候補（applyによる成形として記録され `ready_to_adopt`）。未確認なら拒否。出力が原本と同じなら原本を書き換えず候補も作らない `state: unchanged` | 候補をdiffで確認してadopt |
| values | `state`（`written`・`planned`・`unchanged`）、edit数、空のセルへの入力数 `new_cells`、書き戻し処理の試し書きの `verification`、Excelで文字が収まらない見込みの `layout_warnings`、`check`、`next_actions` | 拒否は `error.code` と `error.edit`・`error.target` |
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

- 位置: `--after` / `--before` には、原本シートの行番号（`15` または `r15`）、列名（`C`）、先に挿入した行・列のキー（`<操作ID>-<番号>`）、または `last`（値のある最後の行・列）を指定します。行番号は原本シートの番号のままで、記録済みの操作による移動を考慮した `at` はCLIが計算します。シートが使う範囲（値、結合、罫線だけの空欄、メモ、図形、高さ・幅を指定した行・列のうち最後のもの）より後ろの番号や、記録済みの操作で削除された行・列は `invalid_position` で拒否します。値のない結合の内側（値のある最後の列より右に広がる備考欄など）や図形の横にも挿入できます。
- 値: `--values` には、1行（列）につき1オブジェクトのリストを YAML または JSON で渡します（ファイル、または `-` で標準入力）。行は列名、列は `r<行番号>` がキーです。値を `{formula: "=ROW()-4"}` と書くと、そのセルに数式を入れます（`=` で始まる文字列そのままは文字列）。`--count` を省くと項目数が使われ、両方を指定して数が合わなければ `count_mismatch` になります。行の値は、同じ列の既存値の型に揃えます。基準は `--style-from` の行の値で、空ならその上で最も近い値です。文字列の列の数値は文字列に、日付書式の列の `YYYY/MM/DD` や `YYYY-MM-DD` はシリアル値にします。揃えられない値は `value_type_mismatch` で拒否します。変換した値は応答の `converted` に返します。追加する列の値は、渡したまま保存します。
- 書式: `--style-from` を省くと、挿入位置の上の行を使います（Excelと同じ）。書き戻しでは、その行の空のセルを含む全セルの書式を追加行に写し、`dimension` を追加行まで広げます。Excelでは `--style-from 9:10` のように範囲を指定すると、追加行は組ごとにその行を順に書式元にします（行数は組の倍数でなければ `invalid_count`）。操作には `style_from` と `style_rows` を記録します。
- 結合: `--merges`（Excelのみ）を付けると、書式元の行の内側にある結合を、追加行の各組の同じ位置にも作ります（Excelの「コピーしたセルの挿入」と同じ。操作の `merges: true`）。付けない場合はExcelの行挿入と同じく、結合は作りません。このとき書式元の行の内側に結合があれば、応答の `merges_not_copied`（作らなかった結合の件数 `count`、先頭8件の `merges`、`hint`）で示します。写した結合の左上以外への値は `merged_non_anchor` で拒否し、写す結合が既存の結合（挿入で広がった結合を含む）と重なる場合は検証で拒否します。後の操作は写した結合も他の結合と同じく移動します。
- 再実行と競合: `--id` を指定した同じコマンドの再実行は `state: unchanged` を返し、何も書き込みません。同じIDで設定や値が違う場合は `operation_id_conflict` です。`--base` には `check --include-hashes` の `content` を渡します。その後に文書が変わっていれば `base_changed` で拒否します。
- 応答: 成功時は `state`（`written`、`--dry-run` なら `planned`、再実行なら `unchanged`）、`operation`、`span`（反映後の最初と最後の行・列）、`keys`、`converted`、`vba`（`--vba-unchanged` を指定したとき）、`neighbors`（行の操作のみ。前後にある本文の行と、その左から4つの値）、`merges_not_copied`（上記）、`check`（書き込み後の検証状態）、`next_actions` を返します。
- エラーコード: `invalid_document`、`base_changed`、`unsupported_format`、`sheet_not_found`（`error.sheets` にシート名の一覧）、`source_changed`、`invalid_operation_id`、`operation_id_conflict`、`invalid_position`、`count_mismatch`、`invalid_values`、`value_type_mismatch`、`merged_non_anchor`、`category_row_deleted`（本文の区分の値を持つ行を、区分の他の記録を残して削除する。Excelが区分の値をその行と一緒に消すため）、`structural_edit_unsupported`、`vba_not_updated`（VBAプロジェクトを持つブックで `--vba-unchanged` がない。`error.vba` にセルを指している可能性のあるコード行）、`invalid_arguments`（VBAプロジェクトのない文書への `--vba-unchanged` 等）、`invalid_reason`（空の `--reason`）、`invalid_count`（`--count` が0）、`validation_failed`。

## 行の移動

`documents rows move <文書ID> --sheet <シート> --from <行> [--count <行数>] (--after|--before) <行> --reason <理由>`（Excelのみ。候補は `--proposal <文書ID>`、`--id`・`--base`・`--dry-run`・`--vba-unchanged` は行の挿入と同じ）は、Excelの「切り取ったセルの挿入」と同じく、原本シートの `--from` から `--count` 行（既定1）を移動先の前後へ移します。移動先は行の挿入と同じく、原本の行番号・挿入した行のキー・`last` で指定します。

- 記録: 次の操作を記録します。応答の `operations` はその一覧、`formulas` は書き直した数式の件数です。
  1. 移動先に同じ行数を挿入する（`<--id>-insert`。`insert_rows` で、書式元は移動する行、`merges: true` で書式・高さ・結合を写す）
  2. 移動する行の値と数式を挿入した行へ写す（`<--id>-copy`。`documents cells copy --values-only` と同じ値の編集）
  3. 移動する行のリンク・メモ・入力規則を挿入した行に設定し（`<--id>-set-link-1` など。`set_link` は `keep_format: true` でセルの書式を変えない）、移動する行に収まる図形を移動先に置く（`<--id>-place-1` など。`place_drawing` でセルの中の位置 `from_offset`・`to_offset` も保つ）
  4. 移動する行を削除する（`<--id>-delete`）
  5. 移動する行を読む数式（ほかの行・シートのものと、移動する行の中のもの）を、Excelが書く式に書き直す（値の編集。`written_after: <--id>-delete` で、移動の後の位置で書く）。Excelと同じく、セルの参照は行と一緒に動き、範囲は、移動する行と移動先をともに内側に含むなら端を保ち、そうでなければ端の行と一緒に動き、端が入れ替わる場合は残る行だけの範囲になる（`SUM(A5:A10)` で10行目を13行目の前へ移すと `SUM(A5:A12)`、5行目を14行目の前へ移すと `SUM(A5:A9)`）
  6. 移動する行に記録済みの書式（`format_cells`）を移動先に記録し直す（`<--id>-<元の操作ID>-<番号>`）。リンク・メモ・入力規則・条件付き書式・図形の配置は、試し書きのブックにあるとおり 3. で移る
- 照合: 未適用の変更をすべて書いた試し書きのブックで、移動する行と移動先を調べます。次の場合は `structural_edit_unsupported` で拒否します（Excelでは行と一緒に動くが、上の操作では同じにならないため）。
  - 名前定義が移動する行を単独のセルで参照する、または範囲の端の行が移動する（範囲の内側だけで行が動く場合と、端が動かない場合は受け付ける）
  - 入力規則・条件付き書式・リンクの範囲が、移動する行と移動先の片方だけを含む（両方を内側に含む範囲と、移動する行だけにかかるリンク・メモ・セルを参照しない入力規則は受け付ける）。条件付き書式は移動する行だけにかかるものも拒否する
  - 図形が移動する行とその外の行にまたがる（移動する行と移動先を内側に含む図形と、セルに合わせて動かない図形を除く）、スレッド形式のコメントがある
  - 移動する行に記録済みのコピー・書式のコピー（`--from`）・結合・画像の追加の操作がある（取り消して移動し、記録し直す）、移動する行の間に記録済みの操作で行が挿入・削除されている
- 移動先が移動する行の間かその直前・直後なら `invalid_position` です。拒否した場合は何も書き込みません。
- 取り消し: `documents operations remove` で後の操作から順に取り消します。`<--id>-delete` を取り消すと、5. で書き直した数式は元の式に戻ります。削除を取り消すと、行の値と、移動する前にその行へ入れた値の編集・新しい数式も戻ります。
- Excelとの違い: 入力規則の範囲の書き方だけが異なることがあります（Excelは `B2:B3 B4:B9` のように分けて書き、ARPは `B2:B9` と書く。対象のセルは同じ）。

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
- Excelでは原本で空のセルにも値を入れられます。表の空欄・挿入した行の未入力のセルは同じ規則の要素ID、原本の範囲の空のセルは番地で指定し、表の範囲の内側なら表の記録に、外なら本文の要素になります。抽出のセルが届く行・列より先のセル（表の横の空いた列など）は、行・列の識別IDを足して受け付けます。結合の左上以外のセルは拒否します。
- 新しい数式: editに `"formula": true` を付けると、原本で空のセル・挿入した行列のセルに `after` の数式（`=` から始まる原文。英語の関数名、カンマ区切り）を数式として入れます。数式は、既存の数式と同じく原本の行・列の位置で書き、未適用の行・列の操作では参照が一緒に動きます（挿入した行のセルの数式もほかの参照と同じく原本の位置で書く）。`"written_after": "<操作ID>"` を付けた数式（新しい数式と、既存の数式の編集）は、その行・列・セルの操作（とそれより前の操作）の後のセルの位置で書いたものとし、exportではそれより後の操作だけが参照を動かします（`documents cells copy` と `rows move` が記録する）。その操作を取り消すと、その数式の編集も取り消して前の式に戻し（応答の `reverted_formulas`）、それより前の操作を取り消すことは `operation_has_dependents` で拒否します。値のあるセルへの数式は `not_editable` で拒否します（既存の数式は `formula` を付けずに編集する）。入れた数式は `after: null` で消せ、挿入の操作を取り消すとその行列の数式も除かれます。exportは数式を計算結果なしで書き、Excelが開くときに計算します（`--calculate` で結果も書ける）。
- 全editを未適用の操作とともに文書形式ごとの書き戻し処理で試し書きし、1件でも拒否されれば何も保存しません。成功した要求のactor・reason・editsは `mappings.yml` の `value_edits` に記録し、exportのreportにも載せます。同じ値を指定し直した要求は `unchanged` で、記録しません。
- 収まりの見込み: Excelでは、試し書きした出力（未適用の行・列操作を含む）のセルの幅・高さ（結合範囲はまとめて）、折り返し・縮小表示・横位置、フォントの大きさ、図形の大きさ（アンカーのセル範囲とグループの拡大率）と余白から、書いた文字が収まるかを見積もり、`layout_warnings` に返します。文字幅は全角1文字を1em、その他を0.55emとする概算なので、変更前の文字より多くの場所が必要で、しかも足りない場合だけを返します。各項目は `edit`（`edits` の添字）、`sheet`、`cell` または `shape`、結合範囲 `merged`、`issue`、`estimate`（`width_pt`・`height_pt`・`font_size`・`wrap`、行数の `needed_lines`・`available_lines`、折り返さない文字の `needed_width_pt`・`available_width_pt`）を持ちます。`issue` は次のとおりです。
  - `clipped`: 文字が切れて表示される。高さを固定した行と結合セルの折り返し、結合・配置ではみ出せないセル、右の空でないセルで止まるはみ出しが該当する。
  - `row_grows`: 折り返す文字でExcelが開いたときに行を高くする（切れないが、行の位置と印刷のページが変わる）。
  - `line_breaks_hidden`: 折り返しのないセルの改行は表示されない。
  - `overflows_shape`: 図形の文字が図形に収まらない。
  警告があっても編集は保存されます（`--dry-run` で事前に確認できる）。見込みは確認の手掛かりで、`documents render` で描画して実際に確かめます。
- エラーコード: `invalid_request`、`invalid_document`、`base_changed`、`source_changed`、`unsupported_format`、`sheet_not_found`（`error.sheets`）、`target_not_found`、`not_editable`、`duplicate_target`、`before_mismatch`（`error.current`）、`value_type_mismatch`、`font_unsupported`、`validation_failed`、`writeback_refused`。editを特定できる拒否は `error.edit`（`edits` の添字）と `error.target` を持ちます。`cell` は原本の番地です。未適用の行・列・セルの操作で位置が変わるシートで `before_mismatch` か `target_not_found` になったときは、`error.moved` に、その原本のセルが今ある番地 `shown_at`（削除されていれば `null`）と、指定した番地に今あるもの `address_shows`（原本のセル `cell`、または挿入した行・列 `inserted_by`）を返します。export・`format`・`cells` の番地（操作の後の番地）と取り違えたときの手掛かりです。`writeback_refused` では、32件以内の要求なら単独でも拒否されるeditの添字を `error.edits` に返します。

## 書式の変更

Excelのセル範囲の書式は `documents format <文書ID> --sheet <シート> --range <範囲> --reason <理由>` で変えます（候補は `--proposal <文書ID>`、`--id`・`--base`・`--dry-run` は行の操作と同じ）。次の指定を1つ以上組み合わせます（`--from` と併用すると、写した書式に重ねて変える）。

- `--from <セル>`: そのセルの書式（罫線・塗り・表示形式・フォント・配置・保護）を範囲の全セルに写す。値のないセルにも書式だけのセルを作る。
- `--wrap true|false`・`--shrink true|false`・`--horizontal <横位置>`・`--vertical <縦位置>`: 範囲の各セルの書式の配置だけを変える（`--from` と併用すると写した書式の配置を変える）。同じ内容の書式がブックにあればそれを使う。
- `--row-height <ポイント>|auto`: 範囲の行の高さ。`auto` は高さの指定を外し、Excelが開くときに文字に合わせる（結合セルは合わせない）。
- `--column-width <文字数>`: 範囲の列の幅（Excelの列幅と同じ単位）。
- `--bold true|false`・`--italic`・`--strike`・`--underline none|single|double`: 範囲の各セルのフォントの太字・斜体・取り消し線・下線だけを変える（フォント名・大きさ・色は `documents values` の `font` で変える）。
- `--fill <RRGGBB>|none`: 単色の塗りつぶし、または塗りつぶしなし。
- `--number-format <書式記号>`: 表示形式（`0.00`、`#,##0`、`yyyy/mm/dd`、文字列の `@` など）。値は変わらない。
- `--border <辺>:<線>[:<RRGGBB>]`: 罫線を引く（繰り返し指定でき、順に当てる）。辺は `all`（外枠と内側）、`outline`（外枠）、`inside`（内側の縦横）、`horizontal`・`vertical`（内側の横・縦）、`top`・`bottom`・`left`・`right`（範囲の上下左右の辺）。線は `thin`・`medium`・`thick`・`dashed`・`dotted`・`double`・`hair` など、`none` は線を消す。色を省くと自動の色。Excelと同じく、範囲の外周の辺の線は範囲のセルが持ち、向かい合う隣のセルの同じ辺の線は外す（範囲を後で削除すると線も一緒に消える）。
- `--merge`: 範囲を結合する。左上以外のセルに値があれば `writeback_refused` で拒否する。既存の結合と重なる場合も拒否する。`--unmerge` は範囲の内側の結合を解除する。

範囲と `--from` のセルは、それより前に記録した行・列の操作の後の番地（exportの出力の番地、`layout_warnings` の `cell` と同じ）です。後に記録した行・列の操作では、範囲がセルと一緒に移動します。後に記録したセルの挿入・削除（`documents cells`）が範囲の一部だけをずらす場合、セルの書式はずれたセルと一緒に動き（範囲が分かれる）、行の高さ・列幅は元の行・列に残り、結合は `writeback_refused` で拒否します。後に記録した挿入で、書式元の行・左の列・上（左）のセルがこの範囲にある行・列・セルは、Excelと同じく変えた書式を受け継ぎ、行・列ごと挿入した場合は `--row-height`・`--column-width` も受け継ぎます（結合は受け継ぎません）。値は変わらず、exportは書式を当てた後に全セルの値が変わっていないことを確かめます。操作は `mappings.yml` の `operations` に `format_cells` として記録し、試し書きと `check` と同じ検証に通った場合だけ書き込みます。exportの最後に出力へ当てるため、`documents values` の収まりの見込みにも反映されます。取り消しは `documents operations remove` です。

```yaml
operations:
  - {id: fmt-1, kind: format_cells, sheet: 画面項目定義, reason: 長い説明を収める, range: "AN9:AV9", row_height: 48, vertical: top}
```

## 図形の配置

Excelの図形・画像・グループ・グラフ（グループの外にあるもの）は `documents drawings place <文書ID> --sheet <シート> --drawing <抽出のdrawingsのID> --reason <理由>` で動かします（候補は `--proposal <文書ID>`、`--id`・`--base`・`--dry-run` は行の操作と同じ）。

- `--from <セル> --to <セル>`: 左上の角と右下の角のセルで位置と大きさを決める（`images add` と同じく、各セルの左上の角に置く）。セルは、それより前に記録した行・列の操作の後の番地です。
- `--placement twoCell|oneCell|absolute`: その後に記録した行・列の操作で、セルに合わせて移動とサイズ変更をするか（`twoCell`）、移動だけか（`oneCell`）、動かないか（`absolute`）。Excelの図形の書式設定の「プロパティ」と同じです。行を挿入する前に画像・グループを `oneCell` にすると、挿入した行で伸びません。

exportは、その図形のアンカーを原本から記録順に追い、前の操作では元の配置設定で、後の操作では新しい配置設定で動かします（Excelで途中で配置設定を変えた場合と同じ）。グループの中の図形はグループと一緒に動くため拒否し、`error.message` にグループのIDを示します。記録した行・列の削除で一緒に削除される図形（セルに合わせて移動とサイズ変更をし、削除する範囲に収まるもの）は `target_not_found` で拒否します。配置の後に記録した行・列の削除は、Excelと同じく配置した位置と配置設定で図形を一緒に削除するかを決めます（配置で削除する範囲の外へ出した図形は残り、範囲の中へ入れた図形は削除され、本文から文字も消える）。`--placement twoCell` はExcelと同じく `editAs` を書かずに保存します。動かした図形（グループならその中の図形）に接続した線は自分の位置に残るため、応答の `joined_connectors` に示します。必要なら同じコマンドで線も動かしてください。操作は `mappings.yml` の `operations` に `place_drawing` として記録します。

```yaml
operations:
  - {id: place-1, kind: place_drawing, sheet: システム構成図, reason: 写真を伸ばさない, drawing: "xl/drawings/drawing4.xml#113", placement: oneCell}
```

## セル範囲のコピー

`documents cells copy <文書ID> --sheet <シート> --range <範囲> --to <写し先の左上のセル> --reason <理由>` は、Excelの「コピーと貼り付け」と同じく、範囲を同じシートの別の場所へ写します（候補は `--proposal <文書ID>`、`--id`・`--base`・`--dry-run` は行の操作と同じ）。範囲と写し先は、それより前に記録した行・列の操作の後の番地です。

- 値と数式: 値の編集（`value_edits`、`actor` は `documents cells copy`）として記録し、本文の写し先にすぐ表示します。数式は相対参照を写した距離だけずらし（`$` の付いた参照はそのまま）、写し先の空のセルには新しい数式として入れます。記録済みの行・列・セルの操作があれば、写した数式は最後の操作の後のセルの位置で書き（`written_after`）、後の操作だけが参照を動かします。このとき原本の数式のあるセルへの貼り付けは `not_editable` で拒否します。値のあるセルへ数式を、数式のあるセルへ値を写す場合は `not_editable` で拒否します（先に `documents values` で空にする）。
- 書式・結合・リンク・メモ・入力規則・条件付き書式: `copy_cells` 操作として記録し、exportの最後に当てます。写し先のものは置き換え、写し先の一部だけにかかる結合は `writeback_refused` で拒否します。相対参照のある入力規則・条件付き書式は写し先から書いた別の規則にし、参照の変わらない入力規則は規則の範囲を広げます（Excelと同じ）。後に記録した行・列の操作がコピー元の範囲の内側に行・列を挿入・削除すると、exportは拒否します（コピーをその後に記録し直す）。
- `--values-only` は値と数式だけ、`--formats-only` は書式と結合などだけを写します。応答の `values` は記録した値の編集の件数、`operation` は記録した `copy_cells` 操作です。

```yaml
operations:
  - {id: copy-1, kind: copy_cells, sheet: 一覧, reason: 記録の形を写す, range: "A6:L7", to: A21}
```

## リンク・メモ・入力規則・条件付き書式

Excelのセルが値と書式のほかに持つものは、次のコマンドで付けたり外したりします（候補は `--proposal <文書ID>`、`--id`・`--base`・`--dry-run` は行の操作と同じ）。範囲（`--range`、メモは `--cell` とも書ける）は、それより前に記録した行・列の操作の後の番地です。操作は `mappings.yml` の `operations` に記録し、試し書きと `check` と同じ検証に通った場合だけ書き込み、exportの最後に出力へ当てて読み直して確かめます。後に記録した行・列・セルの操作では範囲がセルと一緒に動き、入力規則と条件付き書式はExcelと同じく範囲の中や直後に挿入した行・列にも広がります。本文の値は変わりません。

- リンク: `documents links set <文書ID> --sheet <シート> --range <範囲> (--location <シート!セルまたは名前> | --url <URL>) [--tooltip <文字>] --reason <理由>`（`set_link`）。範囲にかかるリンクは置き換え、セルはExcelと同じく下線とリンクの色になる。`documents links clear`（`clear_links`）は範囲にかかるリンクを外し、リンクの見た目（下線とリンクの色）だけを外す（Excelの「リンクの削除」と違い、罫線などの書式は残す）。セルに表示する文字は `documents values` で変える。
- メモ: `documents notes set <文書ID> --sheet <シート> --cell <セル> --text <文字> [--author <作成者>]`（`set_note`）は、セルにメモを付けるかメモの文字を置き換える（メモの部品がなければ作る）。`documents notes clear --range <範囲>`（`clear_notes`）は範囲のメモを削除する。スレッドコメントのあるセルは `writeback_refused` で拒否するので、Excelで編集する。
- 入力規則: `documents validations set <文書ID> --sheet <シート> --range <範囲> ...`（`set_validation`）。範囲を既存の入力規則から外したうえで、`--list <a,b,c>`（ドロップダウンの項目。255文字まで）、`--source <=$J$1:$J$5>`（項目を並べたセル）、`--type whole|decimal|date|time|text-length --operator <比較> --value1 <値> [--value2 <値>]`、`--formula <=数式>` のどれかを付ける。`--allow-blank false`、`--error-style stop|warning|information`、`--prompt-title`・`--prompt`・`--error-title`・`--error` も指定できる。`documents validations clear --range <範囲>`（`clear_validations`）は範囲を入力規則から外し、範囲の外のセルの規則は残す。
- 条件付き書式: `documents conditional-formats add <文書ID> --sheet <シート> --range <範囲> (--formula <=数式> | --cell-is <比較> --value1 <値> [--value2 <値>]) [--fill <RRGGBB>] [--font-color <RRGGBB>] [--bold] [--italic] [--strike] [--number-format <書式記号>] [--stop] --reason <理由>`（`add_conditional_format`）。Excelと同じく、新しい規則はほかの規則より先に評価する。数式は範囲の左上のセルを基準に書く（`=$C5>100`）。書式を1つも指定しないと `invalid_rule` で拒否する。`documents conditional-formats clear --range <範囲>`（`clear_conditional_formats`）は範囲を条件付き書式から外す。

比較の `--operator`・`--cell-is` は `between`・`not-between`・`equal`・`not-equal`・`greater`・`less`・`greater-or-equal`・`less-or-equal` です。既存のリンク・入力規則・条件付き書式は抽出結果のシートの `links`・`validations`・`conditional_formats`、メモは `comments` に記録し、`documents extraction-read` で読めます（この版より前に取り込んだ文書は `documents import --force` で取り込み直すと記録されます）。

```yaml
operations:
  - {id: link-1, kind: set_link, sheet: 一覧, reason: 仕様書へのリンク, range: B5, url: "https://example.com/spec"}
  - {id: validation-1, kind: set_validation, sheet: 一覧, reason: 区分を選ぶ, range: "E5:E20", rule: {type: list, formula1: "\"通常,特急\""}}
```

## セルの挿入・削除

Excelのセル範囲は `documents cells insert <文書ID> --sheet <シート> --range <範囲> --shift down|right --reason <理由>` で挿入し、`documents cells delete <文書ID> --sheet <シート> --range <範囲> --shift up|left --reason <理由>` で削除します（候補は `--proposal <文書ID>`、`--id`・`--base`・`--dry-run`・`--vba-unchanged` は行の操作と同じ）。Excelの「セルの挿入」「削除」と同じく、下・上へずらすときは範囲の列の中で、右・左へずらすときは範囲の行の中でだけセルが動きます。

- 書き戻し: 範囲の列（行）の中のセル、そこに収まる結合・テーブル・図形、その中のセルを指す参照は、行（列）全体の挿入・削除と同じに動きます。範囲の列（行）の外のセルと参照は動きません。範囲をまたぐ参照は書いたままにし、削除で端のセルがすべて消える場合は縮めます（`SUM(B5:D5)` から `C5:D5` を削除すると `B5:B5`）。条件付き書式と入力規則は、残る部分、動く部分、挿入したセルに分け、動いた部分の相対参照が別のセルを指すことになる場合は、その部分を新しい位置から書いた別の規則にします（Excelと同じ）。挿入したセルは上（下へずらす場合）または左（右へずらす場合）のセルの書式になります。右へずらす場合と列の挿入では、Excelと同じく左右のセルが同じ辺に同じ線を引く罫線だけを残します。
- 拒否: テーブル・配列数式・結合の一部だけに範囲の列（行）がかかるずらし方は `writeback_refused` で拒否します（Excelは結合を黙って解除しますが、ARPは解除しません）。範囲を広げるか、先に `documents format --unmerge` で結合を解除してください。
- 本文: 値はずれた先の行・列にすぐ表示されます。挿入したセルは空で、その位置の要素ID（`cell_r5_C` など）を `documents values` の `element` に指定して値を入れます。削除したセルの値は本文から除き、応答の `deleted`（`block`・`row`・`column`・`value`）に返します。応答の `moved_values` は位置の変わった値の件数です。ずれたセルが原本の行・列の範囲より先に出る場合は、その行・列の識別IDを足します。表のセルからずれた値は、ずれた先がどの表にも入っていなければ元の表に残ります（Excelで開いた後に読み直したときも表の続きになるのと同じ）。表の途中でセルをずらすと表の記録の行がそろわなくなるため、記録の追加・削除には `documents rows` を使ってください。

範囲は、それより前に記録した行・列の操作の後の番地です。操作は `mappings.yml` の `operations` に `insert_cells`・`delete_cells` として記録し、試し書きと `check` と同じ検証に通った場合だけ、本文と一緒に書き込みます。

```yaml
operations:
  - {id: gap, kind: insert_cells, sheet: 価格, reason: 品目の間を空ける, range: "B3:C3", shift: down}
```

## 操作の取り消し

`documents operations list <文書ID>`（候補は `--proposal`）は未適用の操作を記録順に返します。`documents operations remove <文書ID> --id <操作ID> [--base <content>] [--dry-run]` は操作を本文への影響とともに取り消し、`check` と同じ検証に通った場合だけ書き込みます。

- 挿入（`insert_rows`・`insert_columns`・`insert_slide`・`add_slide`）を取り消すと、追加した行・列の値とページを本文から除き、それを対象にした `value_edits` の記録も除きます。
- セルの挿入・削除（`insert_cells`・`delete_cells`）を取り消すと、ずらした値を元の位置に戻します。挿入したセルの値は本文から除き（`removed_values`）、削除したセルは原本の抽出の値で戻し（`restored_values`）、戻したセルの表の所属は構造解釈から戻します。削除の取り消しでは、戻したセルへの `value_edits` を当て直します。
- 削除（`delete_rows`・`delete_columns`・`delete_slide`）を取り消すと、削除した行・列・スライドの値を原本の抽出の値で戻し、表の所属は構造解釈から戻します。戻した値を対象にした `value_edits` の記録（値・フォントの編集）は記録順に当て直し、戻したセルのうち値も数式もないセルには、記録で最後に入れた新しい数式（後で消したものを除く。`written_after` も保つ）を入れ直します。件数を `reapplied_edits` に返します。記録の変更前が戻した値と一致しない編集は当て直しません。スライドの削除で一緒に取り除かれた行の操作も戻りません。
- その他の操作（列の移動、スライドの移動・表示、図形・画像の操作）は操作だけを取り除きます。
- 取り消す操作の後の位置で書いた数式の編集（`written_after` がその操作を指すもの）は一緒に取り消し、数式を前の式に戻します（`reverted_formulas`）。後の操作の後の位置で書いた数式が残っていれば `operation_has_dependents` で拒否します。
- 同じシートで後に記録した行・列・セル・画像の操作、または取り消す操作が追加したスライド・図形を指す操作があれば `operation_has_dependents`（`error.dependents`）で拒否します。後の操作から取り消してください。存在しない操作IDは `operation_not_found`（`error.operations`）です。

## export候補の描画

`documents render <文書ID> --candidate <exportの出力> --out <新規PNG>` は、export が書いた候補（隣に `.report.json` があり、そのハッシュと一致するもの）のページをPNGに描画します。Excelは `--sheet`（必須）と `--range`（省略時は使用範囲）をExcelで（report が `requires_excel_recalculation` の候補は全再計算してから。応答の `recalculated`）、WordとPowerPointはOfficeが保存したPDFの `--page`（スライドは非表示のものも含めた候補での順序）を、PDFはその `--page` をWindowsで描画します。WindowsとデスクトップOffice（Excel、Word・PowerPointの文書ではそのアプリ）が必要です。描画は確認の材料であり、画像を開いて実際に確認した範囲だけを `confirm-export` に記録します。

## シートの追加・複製・削除・名前の変更

Excelのワークシートは `documents sheets add|copy` で追加・複製し、`documents sheets delete|rename` で原本のシートを削除・名前変更します（候補は `--proposal <文書ID>`、`--id`・`--base`・`--dry-run` は行の操作と同じ）。`mappings.yml` の `operations` への追記、本文のページ `content/<シート名>.yml` とlayoutのシートの作成を1回の実行で行い、`check` と同じ検証と試し書きに通った場合だけ書き込みます。

- 追加: `documents sheets add <文書ID> --name <シート名> [--after|--before <シート>] --reason <理由>` は、Excelの「シートの挿入」と同じく空のワークシートを追加します（行の既定の高さは先頭のシートと同じ）。位置を省くと最後に置きます。
- 複製: `documents sheets copy <文書ID> --sheet <原本のシート> --name <シート名> [--after|--before <シート>] --reason <理由>` は、Excelの「移動またはコピー」（コピーを作成する）と同じく、セル・書式・行の高さと列幅・結合・入力規則・条件付き書式・リンク・メモ・図形と画像・印刷設定を写します。複製元のシートを指す数式・入力規則・条件付き書式はコピーを指し、ブックの名前のうち複製元を指すものと複製元のシートの名前（印刷範囲を含む）はコピーのシートの名前として作ります。テーブルはExcelと同じ規則の新しい名前（元の名前とブックのテーブル数+2の番号。セル番地と読める場合は `_` を挟む。例: `Lens3`、`Tab1_3`）になり、コピーの数式の構造化参照もその名前を指します。グラフはコピーの部品（スタイル・色を含む）になり、系列はコピーのシートを指します（Excelと同じ）。本文のページは複製元のページの現在の値（編集済みの値と新しい数式を含む）を写します。複製元に記録した書式・コピー・リンク・メモ・入力規則・条件付き書式・図形の配置・画像の操作は、コピーにも操作ID `<--id>-<元の操作ID>` で記録し直し、応答の `copied_operations` に返します（取り消すときはそれらを先に取り除く）。
- 削除: `documents sheets delete <文書ID> --sheet <原本のシート> --reason <理由>`（`delete_sheet`）は、Excelのシートの削除と同じく、そのシートを指すほかのシートの数式・入力規則・条件付き書式・グラフの系列・名前とそのシートのテーブルへの参照を `#REF!` にし、そのシートの名前（印刷範囲を含む）を除き、そのシートだけが使う部品（図形・メモ・テーブル・印刷設定など）も除きます。リンクの参照先はExcelと同じく変えません。本文のページと、そのシートに記録した操作（応答の `dropped_operations`）・値の編集・位置対応も除きます。最後の表示中のシート、ピボットテーブルが読むシートは拒否します。
- 名前の変更: `documents sheets rename <文書ID> --sheet <原本のシート> --name <新しい名前> --reason <理由>`（`rename_sheet`）は、Excelと同じく、そのシートを指す数式・入力規則・条件付き書式・グラフの系列・名前を新しい名前にします（リンクの参照先は変えない）。
- 順序: exportは追加・複製を最初に、削除・名前の変更をほかのすべての編集の後に行います。そのため、ほかのコマンド（値・行・書式・複製元・位置）は、名前を変えるシートを原本の名前で指します。削除・名前の変更で空く名前は、exportするまでほかのシートに付けられません。
- 名前と位置: シート名は31文字まで、`: \ / ? * [ ]` を含まず、`'` で始まらず終わらず、ほかのシートと重なってはいけません（大文字小文字を区別しない）。`--after`・`--before` には原本のシートか先に追加したシートの名前を指定します。
- 追加したシートの編集: 値は `documents values` で `sheet` に新しいシート名、`cell` にそのシートの番地（複製では複製元と同じ番地）を指定して変え、行・列・セルの操作、書式、リンク・メモ・入力規則・条件付き書式、画像はほかのシートと同じく `--sheet <新しいシート名>` で記録します。`documents elements-read` と `check` は追加したシートを含めて読みます。`documents extraction-read` は原本の抽出なので、追加したシートを含みません。
- 拒否: ピボットテーブル・スレッド形式のコメント・コントロール・埋め込みオブジェクトを持つシートの複製は `invalid_sheet` または試し書きの `writeback_refused` で拒否します。行・列・セルの挿入・削除・移動を記録したシートは複製元にできません（`invalid_sheet`。先に複製するか、その操作を取り除く）。名前の誤り・重なりも `invalid_sheet`、同じ名前の本文ファイルがあれば `page_exists` です。
- 取り消し: `documents operations remove` でシートの追加・複製を取り除くと、本文のページとlayoutのシート、そのシートへの `value_edits` も除きます。削除を取り除くと、シートは原本の値で戻ります（削除で除いた操作と値の編集は戻らない）。名前の変更は操作だけを取り除きます。そのシートに記録した操作が残っていれば `operation_has_dependents` で拒否します（後の操作から取り消す）。
- 応答: 成功時は `state`、`operation`、`page`（ページID `sheet-<操作ID>`）、`added_pages`、`sheets`（操作後のワークシートの順序。グラフシートは含まない）、`copied_operations`、`check` を返します。export reportの `added_sheets`・`deleted_sheets`・`renamed_sheets` に追加・削除・名前を変えたシートを記録します。
- Excelとの違い: Excelはコピーを選択したシートにしますが、ARPは選択中のシートを変えません。`docProps/app.xml` のシート一覧は更新しません（Excelが保存時に作り直す）。

```yaml
operations:
  - {id: cp, kind: copy_sheet, from: 一覧, name: 一覧 (2), after: 一覧, reason: 版ごとの表を作る}
  - {id: memo, kind: add_sheet, name: メモ, reason: 補足を書く}
  - {id: rn, kind: rename_sheet, sheet: 一覧, name: 一覧（旧）, reason: 版を分ける}
  - {id: dl, kind: delete_sheet, sheet: 作業用, reason: 不要}
```

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
