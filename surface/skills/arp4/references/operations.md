# ARP 操作リファレンス

必要な操作の節だけを読む。workflowの割当・再利用・終了判断は [進行管理](orchestration.md)、担当の読取・検証・提出手順はカスタムAgentの定義に従う。この資料全体を担当へ渡さない。

- 「文書ワークフロー」: import、成形、diff、原本への反映。
- 「メタモデルと設計書」: 構造解釈、capture、workflowの準備、明示的な個別CLI操作。
- 「正本の継続保守」: registryへの登録、更新、承認。
- 「CLI応答の読み方」: ページ送り、省略、エラーの確認。

以下の `docs/` で始まる文書パスはARPソースルート基準。導入先の文書プロジェクトに存在するとは限らない。実行時の引数・対応機能・返信契約はCLIのhelp・doctor・readを参照する。

## 文書ワークフロー

利用者が取り込みをAIに委任した場合は、原本の配置と初期化を確認し、import、候補の成形と検査、record、adoptまで進める。各CLI段階のたびに利用者を呼び戻さず、処理件数、採用済み文書、確認が必要な文書と原本位置・理由をまとめて報告する。取り込みの失敗・未対応形式、原本でしか確認できない未抽出情報、曖昧な対応、検査のblockers、判断に必要な権限や情報の不足は対象文書を明示して利用者へ確認する。確認できない内容を読めたことにしない。

adoptは候補を現在版にする操作であり、確認済みの証跡ではない。採用直後の状態は `needs_review`。AIが実際に内容を確認した場合は実際の確認者名でreviewを記録できるが、人による確認を行っていない文書を人の名前でreviewしない。利用者に確認を求める場合は、該当文書と確認箇所・理由を提示し、その文書のreviewを残す。未レビュー文書が残る場合は件数を報告する。

原本は既定で `docs/`、設定は `.arp/config.yml`。初期化は既存の原本やAGENTS.mdを変更せず、Gitに改行を変換させないためsourcesフォルダと `.arp/` の `.gitattributes` に `* -text` を追記する（削除しない）。文書IDはsourcesフォルダ（既定 `docs/`）からの原本の相対パス（拡張子込み、例 `xxx/yyy/仕様.xlsx`）で、採用データは `.arp/documents/<文書ID>/`、候補は `.arp/changes/<文書ID>/` と原本のフォルダ構造をそのまま映す。候補は文書ごとに1つで、再importで置き換わる。原本コピーと版別保存は作らない。原本・変換結果・要件や判断をGitへコミットする。`.arp/work/` と `.arp/cache/` はGit対象外。

最初に `arp4 doctor` を確認する。`implementation: rust` の場合はこの節の範囲で作業する。
起動できない場合は [arp4-setup](../../arp4-setup/SKILL.md) を参照する。

- 初期化は `documents init --root <project>`、取り込みは `documents import <原本またはフォルダ>`。原本はsourcesフォルダの中に置く（外は拒否）。ファイル・フォルダのどちらも、ハッシュが同じ候補・採用済み文書はunchangedとして保持する。ファイル応答の `pending_proposal` とフォルダ応答の `pending_proposals` は残っている候補を示す。再抽出するときだけ `--force` を使う。フォルダ指定は配下の対応形式をまとめて取り込み、未対応形式はskipped、原本が消えた文書はmissingとして返す。破損などで読めない原本はfailedに文書IDと理由を返し、他の原本の取り込みは続ける。failedがあれば `ok: false`・終了コード2になるので、該当原本を直して再importする。不要な候補だけを消すときは `documents discard <文書ID>` を使う。
  対象は `.xlsx` / `.xlsm` / `.xltx` / `.xltm` のセル値・数式原文・結合範囲。PNG画像は `assets/` の画像を `add_image` 操作でセル範囲に追加できる。埋込み画像はWindowsで取込時にOCRを自動実行し、結果または失敗理由を抽出JSONのassetsへ記録する。メモ・スレッドコメント、図形の割り当てマクロ、フォームコントロールのリンク先セル・選択肢範囲は抽出する。図形の意味・ActiveXの設定・印刷情報は未抽出で、R001に記録する。
  未抽出の情報は原本で確認し、読み取れたと扱わない。
- 原本の移動・リネーム・削除は、別文書の追加と旧文書の削除として扱う。原本を動かしたら `documents import docs` で新しいパスを取り込み、missingの文書を `documents remove <文書IDまたはフォルダ>` で削除する（原本が残っていれば拒否）。旧文書の根拠・承認は引き継がないため、新しいextractionでcaptureし直してレビューする。
- `.arp/documents/<文書ID>/` 以下のパスが200文字を超える原本と、Windowsで使えない名前（末尾の `.`・空白、`CON` 等の予約名）の原本は取込を拒否する。原本のフォルダ名・ファイル名の変更を利用者に依頼する。
- `.docx` / `.docm` / `.dotx` / `.dotm` / `.pptx` / `.pdf` も同じワークフローで取り込み・同形式出力できる。暗号化された原本はimportが暗号化を解除し、原本を暗号化なしで上書きしてから取り込む（応答の `unprotected`）。パスワード暗号化は `--password-stdin` で標準入力から渡す。パスワードは利用者に確認し、ファイル・コミット・報告に書かない。IRM・秘密度ラベルはWindowsのOfficeがサインイン中のアカウントの権限で解除する。権限・ラベルポリシーで拒否されたら原本は変わらないので、利用者に保護の解除を依頼する。バイナリ形式（`.xls` / `.xlsb` / `.doc` / `.ppt`）、Strict Open XMLはエラーになる。利用者に標準のOpen XML形式で保存し直すよう依頼し、読み取れたと扱わない。Wordは本文・表・テキストボックス・ヘッダー・脚注・コメント等を段落と表のセル単位で扱い、表は行・列・結合と表の範囲を保つ（`rN` は段落・表の行の番号、列は表の列）。見出し行は推定なので構造レビューで確認する。PPTXもWordと同じく、スライドとノートの段落と表のセルを扱う（`rN` は段落・表の行の番号）。PDFはページのテキスト描画文字列を扱い、位置情報側の `rN/A` は文字列の通し番号。いずれも物理セルではない。WordとPPTXの書き戻しは変更箇所のrunだけを書き換える。段落内の改行・タブは追加・削除・置換できるが（YAMLの `\n` は段落内の改行として書く）、段落の分割・結合は拒否する。既存文字列を編集し、空にするには `""` を使う。行の追加削除はExcelとWord・PowerPoint（段落・表の行）、スライドの複製・削除はPowerPoint、列の追加削除と画像はExcel限定。PDFの画像/OCR・Form XObject・注釈・フォーム、PPTXのマスター等は未抽出（ノートは全スライドの後に `notes-N` として抽出し、書き戻せる）。PDFは原フォントで表現できない文字と改行・タブを拒否する。Wordのフィールドの表示結果（日付・ページ番号・目次等）はWordが再計算するため書き戻しを拒否する。文書データと連携したテキストのコンテンツコントロール（表紙の表題等）は、連携先データと同じデータの他のコントロールもまとめて書き換える。Excelのマクロシート・ダイアログシート・グラフシートは抽出せずR001にシート名を記録し、原本の部品は保持する。行・列の追加削除は全シートの数式・名前付き範囲・条件付き書式・入力規則・テーブル・ピボット参照元・グラフ系列等をExcelと同じ結果になるよう移動する。メモ・スレッドコメント・フォームコントロール（アンカーとリンク先セル）も移動する。VBA・マクロシート・ダイアログシート・ActiveXコントロールを含むブックと、Excelも拒否する変更（テーブル見出し行の削除、ピボットや配列数式を横切る変更等）はexportの計画時点で拒否する。その場合は行・列の変更をExcelで行い再取り込みする（セル値の編集は可能）。文字のはみ出しや再組版は出力ファイルを開いて確認する。
- `.txt` / `.md` / `.csv` / `.tsv` はUTF-8（BOM可、最大32 MiB）の正式な原本としてimportし、extractionからspec captureで出典化できる。TXTは行、Markdownは見出し・段落・リスト・表・コード等のブロック、CSV/TSVはレコード・列単位で抽出する。Markdown記法・複数行本文を保持し、positionに見出し階層・行範囲・原本基準のUTF-8バイト範囲を持つ。MarkdownのA1等はブロック番号、CSV/TSVのA1等は列・レコード番号で物理行ではない。CSV/TSVは全項目文字列、先頭行も通常レコードとして扱い、先頭ゼロ・空欄・引用符内改行を保持する。リンク先・画像は読まない。TXT/Markdownは最大1,048,576行、CSV/TSVは16,384列・1,048,576フィールド。非UTF-8・NUL・不正CSVを拒否し、型や文字コードは推測変換しない。
- 本文は原本を直接編集し、同じパスのまま再import→候補のdiff→record→adoptする。contentのYAMLは確認用の派生ビューで、本文変更・構造操作・export/applyは未対応。再取込候補の `documents diff <文書ID>` はsource_impactで位置移動・文脈/順序変更・本文変更候補・追加削除・曖昧な対応を示し、registryがあれば同じ見出し範囲と依存する項目をaffected_entriesに示す。modified_candidate・ambiguousは対応を自動確定しない。根拠と承認は自動移行せず、再取込後はcaptureを更新して必要なレビューを行う。参照資料だけに使う--referenceとは区別する。
- importのJSONにある `document_id` と `proposal` を使い、本文YAMLと対応表を確認して実際の成形を行う。
  WordとPPTXは段落内の改行・タブを編集できる。PDFは改行・タブを置換文字列に含めず、既存の文字列を個別に編集する。
  型、page_id、既存の行・列、field ID、セル対応を維持する。
- Excel・Word・PowerPointの行（Word・PowerPointは表の外の段落と表の行。PowerPointの新しい段落は書式元の段落と同じ図形に入る）とExcelの列の追加削除は `mappings.yml` と本文YAMLを手で編集せず、`documents rows insert|delete` / `documents columns insert|delete` で行う（候補は `--proposal <文書ID>`、採用後は文書ID）。1回の実行で操作をreason付きで記録し、追加分の値と原本への対応を保存し、削除した行・列の値を本文から除く。checkと同じ検証に通らなければ何も書き込まず、成功時は応答の `check` に結果を返す。
  - 位置は `--after` / `--before` に、原本シートの行番号（`15`・`r15`）か列名（`C`）、先に挿入した行・列のキー（`<操作ID>-<番号>`）、または `last`（値のある最後の行・列）で指定する。削除は `--from` に原本の行・列を指定する。
  - 値は `--values <YAML/JSONファイル>`（`-` で標準入力）に1行（列）1オブジェクトのリストで書く。行は列名がキー（`- {B: "8", C: 2025/11/21}`）、列は `r<行番号>` がキー。`--count` を省くと項目数になる。行の値は同じ列の既存値（`--style-from` の行、空ならその上で最も近い値）の型に揃う（文字列の列の数値は文字列に、日付の列の `YYYY/MM/DD` は日付に）。揃えられない値は拒否される。変換は応答の `converted` で確認する。先頭のゼロなど、表記を保つ値は引用符で囲む。
  - 書式は既定で挿入位置の上の行から写す（Excelと同じ）。見出しの直下に挿入するときは `--style-from <データ行>` を指定する。
  - 先に `--dry-run` で `operation`・`span`・`neighbors`（挿入・削除位置の上下の行）・`converted` を確認する。`--id <操作ID>` を付ければ同じコマンドの再実行は `unchanged` になり、二重には挿入されない。
  - 他の担当と並行する場合は `documents check <文書ID> --include-hashes` の `content` を `--base` に渡す。その後に文書が変わっていれば `base_changed` で拒否される。
  - PowerPointのスライドは `documents slides insert <文書ID> --from <スライド> --after|--before <スライド> --id <操作ID> --reason <理由>` で複製し、`documents slides delete <文書ID> --slide slide-<番号> --reason <理由>` で削除する。原本のスライドは `slide-<番号>`（原本での番号のまま）、追加したスライドは操作IDで指定する。複製すると `content/<操作ID>.yml`（ノートがあれば `content/notes-<操作ID>.yml`）に複製元の値が写るので、そのページを編集して文字を変える。コメントは複製しない。追加したスライドを消すときは削除ではなく操作を取り除く。他のスライドからリンクされたスライド、目的別スライドショーが空になる削除は `structural_edit_unsupported` で拒否されるので、PowerPointでリンクを外してもらう。
  - レイアウトから新しいスライドを作るときは `documents slides add <文書ID> --layout <レイアウト名またはpart> --after|--before <スライド> --id <操作ID> --reason <理由>`。レイアウトは抽出の `slide_layouts`（名前・part・プレースホルダー）で選ぶ。日付・フッター・スライド番号以外のプレースホルダーが空で作られ、`content/<操作ID>.yml` に空の文字列として並ぶので、そこに文字を書く。追加したスライドを消すときは操作を取り除く。空のプレースホルダーは既存のスライドでも空の文字列として抽出され、文字を書き込める（画像・グラフ・表・メディア用のプレースホルダーは除く）。
  - PowerPointの図形は `documents shapes update|add|delete|add-picture|replace-picture|add-connector <文書ID> --slide <スライド> ... --reason <理由>` で変える。図形は抽出の `drawings` のID（`<スライド部品>#<cNvPr id>`）か、追加した操作のID（`--id`）で指定し、位置・大きさはスライド上のポイント（`--left/--top/--width/--height`）、回転は度、色は `RRGGBB` か `none`。追加する図形の文字は `--text` で渡し、適用後は本文で編集する。画像は文書フォルダの `assets/` にPNGを置いて指定する。本文の文字を持つ図形とコネクターが結ぶ図形の削除は拒否されるので、段落を行削除して適用するか先にコネクターを削除する。
  - 並べ替えは `documents slides move <文書ID> --slide <スライド> --after|--before <スライド> --reason <理由>`、スライドショーでの非表示・再表示は `documents slides hide|show <文書ID> --slide <スライド> --reason <理由>`。スライドの指定は複製と同じ（原本は `slide-<番号>`、追加したスライドは操作ID）。本文のページは変わらず、ページ名も原本での番号のまま。セクションのある文書では移動先のスライドのセクションに入り、目的別スライドショーの順序は変わらない。非表示の状態は抽出の `state`（`hidden`／`visible`）で確認する。
  - 拒否は `error.code` で判断する。`sheet_not_found`（`error.sheets` にシート一覧）、`invalid_position`、`count_mismatch`、`invalid_values`、`value_type_mismatch`、`operation_id_conflict`、`merged_non_anchor`（挿入で広がる結合の左上以外への値。結合の左上に書くか、結合の外に挿入する）、`structural_edit_unsupported`（VBA・マクロシート・ActiveXコントロール等を含むブック、テーブル見出しの切断など。Excelで編集して再importする）、`invalid_document`（編集前の文書がcheckに通らない）、`validation_failed`。
  - 取り消しのコマンドはない。Gitで文書フォルダを戻す。
- `documents check --proposal <文書ID>` と `documents diff <文書ID>` で確認し、
  実際に作業したactor/model/promptを `documents record <文書ID> --model <モデル> --actor <担当> --prompt <ファイル>` で記録する。
  実施していない成形を記録しない。委任された範囲で `documents adopt <文書ID>` を行い、確認後に `documents review <文書ID> --reviewer <実際の確認者>` を行う。
- record・adopt・reviewは文書IDの代わりにフォルダID、または `--all` で一括処理できる。対象はその操作を待つ状態（`needs_record`・`ready_to_adopt`・`needs_review`）の文書だけで、他は `skipped` に状態・blockers・errorを付けて返す。`failed` があれば他を処理したうえで終了コード2になり、同じコマンドの再実行で残りだけを処理する。
  - 一括処理では先に `--dry-run --out <計画.json>` で対象とcontentハッシュを保存し、確認後に `--expect <計画.json>` を付けて実行する。利用者から一括処理を委任されている場合はAIが計画を確認し、判断が必要な文書だけ利用者へ示す。計画外の文書（`not_planned`）と計画後に内容が変わった文書（`changed_since_plan`）は処理されない。
  - 一括recordは全件に同じactor/model/promptを記録する。全件で実際に同じ作業をした場合だけ使う。一括adopt・reviewも利用者から与えられた権限の範囲で行う。reviewは実際に確認した文書だけに記録する。
- Excel本文の画像・グラフ・図形は `image: null`・`chart: null`・`drawing: null` として表す。同じ配列位置の `layout.yml` の `element_metadata` が持つ `ref` は該当ページの `visuals` で解決する。画像はasset、グラフはchart_part、原本オブジェクトはsourceから確認し、解釈はstructure-readのvisualsをsourceで照合する。図形文字のメタデータの `visual` はその参照との対応であり、OCRや画像説明を原本文字へ混ぜない。セルアンカーのない図形は本文末尾に置かれるため、本文順だけで位置や意味上の読み順を断定しない。
- すべての対応文書の本文は読む順の文書要素として読む。本文の表は値の二次元配列で、ID・見出し関係・数式区分は同じ配列位置の `layout.yml` の `element_metadata` に保持する。原本照合には位置対応と `extraction.json` を使う。書き戻し対応形式では本文の値を編集できるが、配列の追加削除・並べ替えとメタデータの手編集は行わない。同じ形の配列の手動交換は値変更と区別できず、元の位置への値変更として扱われる。未収録セルのnullは編集できない。表と本文の区分は `documents structure-save`、行列の追加削除・移動は専用コマンドを使う。構造保存は本文と位置対応も更新するため、未適用の行列操作があれば先に適用する。
- 採用後は `documents values <文書ID> --input <編集JSON>` で既存内容の変更を保存し（行・列の追加削除は上記のコマンドで行う）、`documents check <文書ID>`、`documents diff --document <文書ID>`、
  `documents review <文書ID> --reviewer <担当>`、`documents export <文書ID> --out <プロジェクト>/.arp/cache/export/<新規名>.<原本と同じ拡張子>` の順で確認・反映する。
  原本へapplyする場合は、出力した文書を実際に開き、変更箇所と周囲の折り返し・はみ出し・結合・印刷配置を確認する。未確認のままapplyしない。出力と `.report.json` はapplyが終わるまで保持し、その後に利用者が成果物を求めた場合だけ残す。
  文字列の先頭が `=` でも数式化しない。既存の数式は 本文要素または表セルの `formula` にある数式原文（`=` から始まるファイル内の表記。新しい関数は `_xlfn.` 付き）を本文内の同じIDで編集すると置き換わり、行・列操作では参照が移動する。スピルする関数・`LET`・`LAMBDA`・`#`・`@` と値セルの数式化はExcelで行う。Excelテーブルの見出しセルの編集は列名の変更になり、ブック内の構造化参照も書き換わる（空・重複・改行を含む列名は拒否）。集計行のラベルも編集できる。図形の文字は `shapes` 表の `text` を改行の数を変えずに編集する。図形の追加・削除・移動、画像の回転・トリミング・絶対座標は拒否される。`add_image` は `anchor.from.cell` / `anchor.to.cell` で配置する。構造変更時は全シートの数式参照・名前定義・テーブル・条件付き書式・入力規則・結合・既存Drawingのセルアンカーを移動し、Excelで再計算する。移動した数式の計算結果は空になり、Excelで開くと再計算される。
- 既存内容の編集は `documents values <文書ID> --input <編集JSON> [--dry-run]` を使う。入力は `documents schema value-edits` の契約に従い、checkのcontentと原本ハッシュ、期待する旧値を照合する。対象は原本の抽出にあるsheet・cellで指定する。全変更を文書形式ごとの書き戻し処理で事前検証し、1件でも不正なら保存しない。管理YAMLを直接編集しない。既存数式のbefore・afterには `=` 付きの数式原文を指定し、文字列セルの `=` は文字列のまま扱う。構造操作中なら先にapply・record・adoptしてから内容を編集する。読み取り専用のフィールド、結合の非アンカー、型の異なる値は拒否する。Excelでrunをまたぐrich text変更やふりがなを壊す変更は拒否するため、必要ならExcelで編集して再importする。Word・PowerPointは段落内の改行・タブの変更に対応し、PDFは元フォントで表せない文字や改行・タブを拒否する。文字数だけでは収まりを判定できず、任意長の文章と固定レイアウトの両立は保証しない。対応形式・対象の一覧は生成された機能・契約リファレンスを参照する。
- フォント名・サイズ・色は、原本の値を `extraction.json` の各セル・図形の `font`、現在値を `layout.yml` の `element_metadata` の `font` で読む。スタイル・テーマの継承を解決した表示上の値で、要素内のrunで異なる項目は `mixed`、解決できない項目は `null`。変更は `documents values` のeditに `font: {before, after}` を書く（`after` は変える項目だけ、`before` は同じ項目の現在値）。値の変更と同じeditにまとめられる。現在の対象はExcelのセルと図形の文字（`cell` の代わりに `shape` へ図形の抽出IDを指定）とWord・PowerPointの段落・表のセルで、Excelのセルはフォント名が1つなので `latin` と `east_asian` に同じ名前を指定する。Wordのサイズは0.5ポイント単位、PowerPointの色は `RRGGBB` だけ（`auto` は不可）。PDFのフォントは読み取りのみ。色を指定するとテーマ色の参照は外れる。`layout.yml` の `font` を直接編集しない。
- 原本へ反映する場合はreview→export→出力を開いたレイアウト確認→`documents confirm-export <文書ID> --candidate <出力パス> --output-sha256 <reportの出力ハッシュ> --reviewer <担当> --reason <実際に確認した範囲・結果> --layout-reviewed`→`documents apply <文書ID>`。これは自動描画判定ではなく、実際に見た結果の記録である。出力を開けなければ未確認として止める。applyは確認した候補そのものを適用し、本文・原本・候補・reportが変わると拒否する。成功後は再抽出した候補が `needs_record` になり、確認・record・adoptを行う。`diff --document` はGitのHEADとの比較なので先に基準をコミットする。
- 中断時は `documents status`。`resume`、`edit-base/plan` は未対応なので呼び出さない。
- 設計情報を探す前に `documents search-refresh` で索引を作成・更新する。文書の採用・削除・編集後や外部で原本を変更した後にも再実行する。`documents search "検索語"` は保存済み索引だけを読み、原本の変更を確認しない。採用済み抽出の段落・表の行を検索し、原本位置・抽出版・最後の索引更新時点の原本とレビューの状態を返す。`--document <文書IDまたはフォルダ>` で絞り、続きは同じ条件で `--offset <page.next_offset> --revision <revision>`。`omitted_fields` があれば絞り込みと `--full` で本文・出典を確認する。`failed` があれば不完全な検索結果であり「該当なし」と扱わない。検索順位を仕様の承認とみなさない。索引はローカルキャッシュで、用語辞書は `.arp/search-synonyms.yml`。詳細は docs/guides/document-search.md。
  型や構造の変更が必要なら未対応と報告し、検証を回避しない。
- `--root` は全コマンドで指定できる。importの相対パスはプロジェクト基準、prompt・outの相対パスは実行ディレクトリ基準。

## メタモデルと設計書

Word・PPTX・PDFは構造解釈・読取完了・acceptedレビューを必須とする。Excelは表（セル配置からの推定を含む）・画像・同一シートに3個以上の図形による図表候補のいずれかがあれば必須。画像はOCRが可能なら実施結果もvisualsへ記録する。OCR不可なら理由を記録し、未確認のまま承認しない。詳細な判定と読取手順は docs/guides/document-structure.md。条件に該当するcaptureでは `--root` と `--document` を省略しない。

OCR実行済みの空の結果も有効とし、未実施・OCR不可とは区別する。ユーザー指示がなければ空の結果も含めてOCR結果を採用し、追加のLLM画像解析は行わない。OCR結果の採用をもって画像のreadを記録できるが、画像を見たとは申告しない。LLM画像解析はコストがかかるため任意。ユーザーから画像確認・補正の指示がある場合は実画像を参照し、必要な補正でocr.textを置換してよい。元OCR文字列の別保存は不要。ocr.llm_correctionに担当・指示内容・判明しているモデル、reasonに補正内容、visualsのevidenceに画像領域IDまたは抽出画像のsource参照を記録し、再レビュー・再captureする。空の結果だけからロゴと断定しない。

- Excelの構造解釈は `documents structure-read <ID> --out <整理YAML>` で編集用ビューを取り出し、`spec structure check/read/region/render/review` で確認してから `documents structure-save <ID> --input <整理YAML>` で文書モデルのmappingsへ修正を保存する。まずreadの簡潔なdrawingsとimage_pathsを使い、図形の文字・形状・位置・明示接続先を読む。drawingsのimage_idをimage_pathsのidと対応させ、pathの画像を直接開く。画像IDは文書の読取結果内の識別子であり、visualのevidenceにはIDではなく画像のsource参照を記録する。同じ画像の複数配置はIDを共有するがsourceは異なる。画像ファイル名は短い連番で、ハッシュ検証はCLI内部で行う。図・画像の関係はvisualのgraphへ根拠付きで整理する。表の説明を別text elementへ分離する場合は、tableのdescriptionsから参照し、セルを二重所属させない。
- 見出し・結合・図形の配置等を描画して確認する場合は `spec structure --root <root> render --extraction <抽出JSON> --structure <整理YAML> --element <elementまたはvisualのID> --id <画像領域ID> --image evidence/<新規名>.png --range A1:H30` を呼び、返されたimage_pathを開く。range省略時はvisualのセルアンカー範囲、elementはシートのUsedRangeを使う。アンカーから範囲を決められないvisualはrangeを明示する。図形を個別に切って関係を失わないよう、図のまとまりを含める。WindowsとデスクトップExcelが必要で、クリップボードは画像に置き換わる。大きすぎる範囲は分割し、外部で用意したPNGはregionで登録する。実際に画像を見てからvision読取を記録し、描画成功だけで確認済みにしない。semantic自動runnerへの画像自動添付は未対応。画像取得・閲覧できなければ未確認・読取不能を残す。acceptedレビュー後に `documents structure-save <ID> --input <整理YAML>` と `documents review <ID> --reviewer <担当>` を行い、`spec capture --root <root> --extraction <抽出JSON> --document <ID> --out <入力JSON>` で意味抽出に接続する。編集後は再capture・workflow update・再レビューが必要。整理YAMLは派生ビューで、原本への書き戻しには使わない。詳しくは docs/guides/document-structure.md。
- 原本が変わると、再importで採用済みの版の構造解釈を新版へ移す。`documents apply` は書き戻した行・列の操作の位置と数で移す。Excel・Word等で原本を直接編集した場合は、旧・新の抽出結果の行・列を値で照合して挿入・削除を求め、同じ処理で移す（段落の追加・削除も行として扱う）。結果は `apply`・`import` 応答の `structure` と候補の `mappings.yml` の `interpretation.carried` に記録され、`by` が `apply` か `alignment` を示す。
  - 挿入・削除位置より後ろのセルはアドレス・ID・見出しのつながりごと移り、削除した行・列のセルとそれへのつながりは除かれる。
  - 表の範囲の内側への挿入、または表の最終行の直後への挿入で `style_from` が表内の行（`rows insert` と直接編集の照合の既定）なら、追加行・列は表に入り、書式元の行（なければ隣の行・列）と同じ役割・見出しのつながりを持つ。広がった結合の見出しはそのまま、他の見出しは追加行・列の同じ位置のセルへつながる。値のないセルは要素に入らない。
  - 値が変わったセルは要素・役割・見出しを保ち、`text_state` を新しい値から付け直す（読取者が `unreadable`・`not_examined` にしたセルは `not_examined`）。新しいセルは、機械推定がまとめる既存の要素、なければ範囲に含む最も小さい要素に加わり、隣のセルの役割を写す（表を本文に訂正した範囲の新しい行も本文のまま）。新しいセルだけの表や、どの範囲にも入らないセルは機械推定になる。
  - 画像・図形の読取結果は写っている内容（画像のハッシュ、図形の文字・形状）で対応付ける。移動や他の画像の追加では失われず、内容が変わったものだけ未確認に戻る。画像領域は範囲がまるごと移動し写っているセルが変わらなければ移動後の範囲で残る。
  - `carried.affected` に影響を受けた要素・図形・画像領域とその理由（`moved`・`added`・`removed`・`changed`・`inferred`）が1件ずつ入る。要素の項目は `cells` に対象セルのIDを持つ（`moved` は件数だけ `carried.moved`）。
  - `carried.review` が `kept` なら構造のレビューはacceptedのまま（`apply` の操作だけで、値の変更がない場合）。`pending` でも修正はやり直さない。`affected` を確認し、`documents structure-read <ID> --proposal --out <整理YAML>` → 新しいセルの役割付与と `spec structure review` → `documents structure-save <ID> --proposal --input <整理YAML>` → `documents record` → `documents adopt` の順で候補のまま承認し直す。
  - 修正記録には機械推定と異なる点だけが入る。構造が推定どおりの要素は読取記録だけが残り、再生のたびに推定し直される（新しい行も推定で表に入る）。値から決まらない `text_state`（`unreadable` 等）はセルのハッシュ付きで残り、値が変わると `not_examined` に戻る。
  - 引き継げない場合（シート名・構成の変更等）は `structure.reasons` に `not carried:` と理由が入り、修正記録はセル番地で再照合される。`documents status` の `structure_conflicts` と、`structure-read --proposal` 応答の `interpretation.conflicts`（要素ごとのIDと理由）を確認する。

- 操作にはnext/inspect/statusの短い `task_ref` を使える（8桁以上の一意なID接頭辞。曖昧なら拒否）。内部ID・保存ハッシュは維持する。Agent向けのpacket/base/bases/draft revisionは実行ごとの短い参照で、readの値をそのまま使う。ハッシュ計算や別実行からの転用はしない。Git Bashでは `--pointer instructions` や `--pointer packet/sources/rows` のように先頭の `/` を省くとパス変換を避けられる。返されたpointerも先頭の `/` だけ省略してよい。
- 読取資料は `.arp/config.yml` の `agent_read_format` に従います（省略時 `toon`、選択肢は `toon` / `json`）。Agentは `workflow read` のTOONを直接読み、内部JSONファイルの先読みやJSONへの変換は行いません。JSON設定時はそのJSONを読みます。返信・差分・validate-reply/submit・機械間の受け渡しはJSONです。形式を変更した場合はreadをoffset 0から再開します。
- workflowは `--full` 非対応。`read --task <ID>` は全節の本文をページで返す。ページ送りにはCLIが返すnext_commandを使う。版が変わったら `--revision` を外してoffset 0から再開する。`--max-bytes` は4096〜48000（既定48000）の設定形式（TOON／JSON）の応答bytes上限で、ホストの表示トークン数の保証ではない。出力が省略・ファイル退避される場合は上限を小さくして先頭から読み直し、省略・退避を読取完了と扱わない。子pointer探索・固定刻み・`head -c` は不要。inspectの `section_bytes` とreadの `page.content_bytes` で配送前の量を確認できる。`status` の `tasks` は `--limit/--offset` でページ送りし、`next_actions` は同じ操作をまとめてtask_refを列挙する（先頭20件、総数は `count`）。`notices` は同じ警告を文書一覧付きでまとめる。ロック競合はCLIが最大30秒待機する。期限超過時は稼働プロセスを確認し、自作の待機ループを重ねない。
- 並列抽出で共通分類が分かる場合は `init --modules <語彙.json>` で先に共有する。抽出の `modules` は新規定義だけを記入できる（なければ `{}`）。itemsが参照する登録済みmoduleの正式名はCLIが補う。明示した名前の競合と未登録IDは拒否する。新しい語彙は他Agentの提出で増える。`module_vocabulary_mismatch` は診断のpath/expected/actualと最新語彙を担当が確認し、意味を判断して返信JSONを編集する。別の意味を持つmoduleへ機械置換しない。

- `validate-reply` の `valid` は受理可否。拒否は `error.code: reply_validation_failed` と `error.diagnostics`（構造化配列。`path` が返信内の位置）で返る。`error.diagnostics_omitted` があれば末尾の診断を丸ごと省いた件数で、返された分を修正して再検証する。submitの拒否は `read --pointer previous_error/diagnostics` でページ単位に読める。抽出では `quality_check.quantity.passed` とdiagnosticsも確認する。`quantity_basis_mismatch` は数量句の引用範囲、`ambiguous_quantity_basis` は複数数量の選択を修正し、候補を採用済みの解釈として扱わない。周辺の条件・統計・前後比較は保持する。固定語彙外の単位は `value.interpretation.kind=opaque_unit` と `unit_basis` で原文に結び付ける。登録済みの数値なし表現は `reviewed_lexical` を使い、未登録表現は `quantity_expression` として解釈待ちで保持する。保持期間・所要時間はscalar、反復間隔はperiod。日付・年度を検証回避のため条件へ移さない。

- 対話型Agentはstatusで担当task_refを確認し、`read --task <ID>` の全ページを読む。instructions・packet・context・scope・reply_schema・module_vocabulary・references・previous_error・previous_reply・draft・reply_templateはまとめて含まれる。linkの全体資料はpacketだけ、repairの原文はcontextだけに含まれる。repairのpacketは識別情報で、内部の全文資料は検証・追加文脈取得用に保持される。`entries` のpointerは本文の位置であり、取得先の案内ではない。連続した配列要素は `array_offset`（先頭の0始まり添字）と `array_total`（配列全体の要素数）付きでまとめて返る。長い文字列の断片は `string_offset` と `string_total_bytes`（UTF-8 bytes）付きで連続して返る。原文の `sources.rows` は `sources.columns` 順で、table列は `sources.tables` の添字。個別の再確認だけ `--pointer <場所>` で絞り込める。`next: null` はstatus/next_actionsで確認する。task文字列を解析しない。
- `/reply_template` は未記入の雛形。返信の `packet`・`document`（reviewの `sheet`/`scope`、repairの `base`、restructureの `bases`）は省略でき、CLIがタスクから補う。`items` 等の作業項目は補わない。返信は `validate-reply --task <ID> --json '<JSON>'` と `submit --task <ID> --json '<JSON>' --origin interactive-agent --actor <担当>` でも直接渡せる。サブエージェントには同じCLI・作業設定と別々のtask_idを渡し、readから提出までCLIで行う。nextは占有しないので親が割り当てる。原文コピーや返信結合は不要。
- 明白な分類のreasonと根拠のないverificationは書かない。statementは構造化項目の言い直しなら省略できるが、そこだけに含まれる役割・例外は保持する。valueのbasis/unit_basis/semantics_basisとcondition.evidenceはCLIがitem.evidenceへ合流するため、evidenceには追加の根拠だけを書く。他の欄に根拠があれば `evidence: []` にできる。推測・複合条件・除外の理由は維持する。
- 返信は直接JSONファイルへ保存してよい。`validate-reply --task <ID> --reply <返信.json>` はsubmitと同じ検証を行い、状態・履歴・オブジェクトを保存しない。提出は `submit --task <ID> --reply <返信.json> --origin interactive-agent --actor <担当> --model <実際のモデル> --reason-file <理由.txt>`。理由ファイルはUTF-8（先頭BOM可）で、引用符・JSON断片・改行をそのまま保存する。短い理由は `--reason <理由>` でもよいが、`--reason-file` とは同時指定不可。retryも同じ理由入力を使える。両コマンドの `--reply -` はUTF-8 stdin入力。理由ファイルは標準入力に非対応。モデル不明なら省略する。
- 分割返信の結合・linkの検証適用・repair/restructureの適用はCLIが担当する。selfcheck・merge・修正適用のスクリプトを自作しない。task文字列を解析せず構造化packetを使う。独立レビューは別Agent実行または人が担当し、自己点検で代用しない。workflowの担当への引継ぎはhandoffを使う。
- 変更なしのrepair返信（`changes: []`）は `--reason` の理由付きで `unresolved`（`status.unresolved`、作業ルートの `unresolved.json`）に記録され、対象項目が変わるまで同じ指摘はrepairに再発行されない。宣言されていないmoduleへの変更など複数文書にまたがる指摘はrepairではなくrestructureへ回る。レビューでは `context.escalation` の `findings`/`unresolved` にある既知の指摘を再報告しない（同じaction・itemsの再報告は既存記録に畳み込まれる）。packetが変わらず指摘がすべて既知の文書は再レビューされない。
- 同一文書の複数項目を直す指摘はfindings.itemsへ全対象を列挙し、一括repairにする。restructureで解決不能なら `{"defer":{"kind":"information_required","reason":"具体的な不足情報"}}` を返す（kindはinformation_required / validator_support / unresolved）。対象文書が変わるまで保留され、解決済みとは扱わない。editsのopen_issuesで論点だけを変更でき、items全体の再送は不要。
- 実測usageが取得できる場合は `submit --usage-file usage.json` に `{"reported_cost_usd":0.25,"usage":{"input_tokens":100,"output_tokens":20}}` を渡す。未知の値は省略する。費用nullは未計測、cost_complete=falseは計測不足。validate-replyのquality_checkで引用文字カバレッジと数量診断を確認し、valid=trueだけで意味品質を承認しない。
- 提出後の実測値は `record-usage --task <ID> --submission <番号> --usage-file usage.json` で追記する。番号はreceiptまたはhistoryのsubmissionを使う。同一値の再送は加算せず、記録済み値と異なる追記は拒否される。再利用した担当の生涯累計ではなく当該提出までの差分を渡す。token_usage_completeは入出力トークン、cost_completeは金額の記録状況を別々に示す。完了後もhistoryが最新の計測台帳であり、公開時のprovenance.jsonはその時点のスナップショットとして保持する。
- 修正予算と上限後の判断は [進行管理](orchestration.md) の「完了の確認」に従う。正式exportが拒否された場合は `status.drafts` の `draft/design.draft.md` と `draft/open-issues.json` を草稿として報告する。
- 入力に含めなかった資料は「除外した」と報告し、「存在しない」と書かない。Markdown・テキストの参照資料は `init --reference <ファイル>...`（差し替えは `update --reference`）で渡し、`read --task <ID>` のreferencesで読む。出典・evidenceにはならず、矛盾や用語の判断根拠として message/reason に `ref:<パス>` で引用する。
- 初回抽出は文書単位。指示・schema・原文・文脈を含むタスクのUTF-8サイズが既定96 KiBを超える場合にシート、さらにsource範囲へ分割する。`init --extract-max-bytes` で調整できる。文字数・source数制限は既定無効で、必要な場合だけ明示指定する。

- 初回生成は `spec workflow --root <リポジトリルート> init --input <入力JSON>` で開始する。親はstatusとhandoffで担当へ割り当て、担当がreadから検証・提出まで行う。以後は [進行管理](orchestration.md) に従う。正式生成後の更新はregistryを使う。
- 利用者が自動runnerを選んだ場合は `run --provider claude-code --executable <CLI> --model <モデル>` または共通JSON方式の `--provider command` を使う。同じタスクをカスタムAgentにも割り当てない。失敗した有料呼び出しは既定では自動再試行しない。明示的な `--max-retries` は `--max-calls` 内の限定再試行。実行中はrecover、失敗後は理由付きretryで扱う。詳しくは docs/guides/semantic-workflow.md と docs/guides/semantic-runners.md。
- エラー時は `status.next_actions` に従い、同じrootと `run_id` を使う。未受理返信は `previous_reply` に保持され、`draft.revision` と `draft.schema` に従って同じsubmitへ `{"draft":"<revision>","set":{"/items/1/name":"修正値"}}` を渡せる。既存フィールド・配列要素の削除は `remove`。CLIが結合して全体を再検証するため正しい部分を再出力しない。再拒否時は最新の版を読み直す。下書きは正式な受理ではない。受理済み抽出の差し替えは `update --reply`。initの繰り返しやstate.jsonの直接編集で回避しない。
- `read --task <ID>` に含まれるreply_schemaの契約・共通module語彙・前回の拒否返信と診断を読む。quantityのunitを欠落させず、平均をscalarへ、条件をunspecifiedへ変えて検査を回避しない。複数セル条件はcomposedと正確な根拠を使う。
- 抽出のsources/contextと追加文脈要求を区別する。追加・分割・統合はrestructureのmappingに従う。外部補修はsubmitのactor/model/reasonへ記録し、history/provenance.jsonで自動処理と区別する。

- `spec capture --extraction <パース結果JSON>... --out <入力JSON>` で入力を固定化する。入力は候補または `.arp/documents/<文書ID>/extraction.json`。
- `spec sources --input <入力JSON> --out <原文.md>` でシート・セル順の全文を読む。大きい入力は `--document <文書ID>` で分割する。
- 新規生成は `spec semantic packet --input <入力> --document <ID> --out <タスク>` で文書別の全文・表情報・短い出典参照と意味判断契約を受け取る。解釈・分類・条件・数量・引用・関連・検証方法だけを返信し、コード・引用位置・正式ID・完全モデルを作らない。`spec semantic assemble --input <入力> --reply <返信>... --actor <担当> --out <フォルダー>` がモデル・分類・初回採番計画・診断を構築する。未処理範囲の自動除外は禁止。数量の未対応表現は診断を報告し、Agent自身が検証器の文法を探索するループを回さない。
- `spec semantic review-packet --input <入力> --model <モデル> --catalog <分類> [--document <ID>] --out <タスク>` で文書別の独立した原文レビューと、document未指定の文書間レビューを実施する。実施した確認だけ返信する。`review-apply` はpacket一致を検証し、部分レビューと未解決指摘も保存する。保存成功は最終受理ではない。`workflow init` と `review-apply` の `--review-plan` で理由付きの監査対象外出典を指定でき、省略時は全出典を監査する。全体レビューと計画上の必須監査、修正対象の指摘の解消が最終受理条件となる。`finalize` はレビュー済みモデルと分類、初回採番計画から永続ID・台帳・分類参照を構築する。承認はしない。詳しくは docs/reference/semantic-generation.md。従来の直接編集時だけ `spec prompt` / `spec schema` の完全モデル契約を使う。
- 再実行は `spec semantic changes --before <旧入力> --after <新入力> --out <計画>` で変更文書を絞り、同一packetの返信だけを再利用する。変更された主張は文書間レビューをやり直す。finalizeは初回専用で、正本更新は既存のregistry applyまたはassign-ids --previousの明示的な同一性判断を維持する。
- `spec check --input <入力JSON> --model <モデル.yml>` で確認する。blocked の矛盾・未処理範囲・レビュー不足を解消し、`spec render --input <入力JSON> --model <モデル.yml> --out <設計書.md>` で生成する。未解決の草稿は `--draft` を指定する。
- 対象範囲はパース済み情報。ready は意味上の完全性を保証しない。矛盾は根拠ある判断を decisions に残し、原文の記述を削除して隠さない。
- check の summary は全問題の種類別件数、coverage は全入力の処理・監査件数。`--out <診断.json>` で全診断を保存できる。render は本文と `<出力名>.report.json` を保存し、草稿本文は診断集計のみを表示する。
- spec の相対パスは実行ディレクトリ基準。workflowとregistryの `--root` はリポジトリルート。省略時は `.arp/config.yml` をGit境界内で探索する。workflowの実行は `--run-id` で区別し、作業状態は `.arp/work/workflow/<run-id>/` に置く。
- 要件・仕様の名称は name、一時キーは id に分ける。`spec assign-ids --input <入力> --model <候補> --plan <対応表> --out <採番済みモデル>` で採番してから check/render する。継続時は `--previous <前回モデル>` を指定し、前回モデルと `.ids.json` 台帳を維持する。対応表は全項目に new/retain/replace と理由を指定し、旧項目の廃止は retire に理由を記す。分割・統合は predecessors で旧IDを記録し、新IDを発行する。名称だけで同一性を判断しない。監査自由文の一時キーは台帳の history.aliases で追跡する。

## 正本の継続保守

- 抽出モデルと `.ids.json` を `spec registry init --input <入力> --model <採番済みモデル> --catalog <分類表> --project <名称>` で登録する。分類表には全IDの category（requirement/specification/observation/estimate/reference）、module、requirements、related と、actor/reason/modules/open_issues/open_issue_documents を指定する。open_issue_documentsは各事項の対象文書を列挙し、全体共通は空配列、事項なしは空オブジェクトとする。各項目の分類理由reasonとverificationは任意で、省略は検証済みを意味しない。曖昧な分類には理由を添える。初回抽出のリンクは省略でき、workflowのlink段階で補完後に独立レビューする。参考・実績・試算は原表型value.kind=tableで保持できるが、設計条件の抽出や数量照合の回避には使わない。初回登録は全件 proposed であり承認ではない。
- 正本は `.arp/registry/` の registry.json・records/*.json・evidence/・archive/。`spec registry check` で検証し、`spec registry render` で機能別本文と根拠・対応表を生成する。Markdownを直接正本として編集しない。
- 更新は `spec registry apply --change <変更JSON>`。変更には check の base_hash、実際の actor、reason を必須とし、entries（全項目内容、proposed/approval:null）、retire、approve、modules、resolve_issues、add_issues を指定できる。新規IDは new:<キー>、既存IDは維持。原資料がない新規判断は evidence:[] と根拠理由を明記する。追加根拠の入力は --input で保存する。
- 承認には実際の確認者と理由、要件・仕様の acceptance を記録する。open_issues を勝手に解決しない。修正・廃止の依存先は再確認待ちになる。`git diff -- .arp/registry` で確認する。現在版を更新し、過去版はGitで管理する。生成物は `.arp/cache/registry/` にある。

- 数量の別セル見出しが「平均」を指定する場合は `semantics: mean` と `semantics_basis`（「平均」の完全一致引用）を使う。引用を item.evidence に含め、同じ文書・シートの同じ行または同じ列の上方にある別セルを指定する。実際の見出しとの対応・否定・例外は原文レビューが必要。数量句全体の basis と比較方法も保持する。

## CLI応答の読み方

- 通常実行は成功・失敗とも標準出力の1行JSON。`ok: false` または終了コード2なら `error` と対象の状態を確認する。`ok: true` はコマンド成功であり、成形完了・採用権限・書き戻し可能の保証ではない。
- check/statusの `items[].state` は `needs_record`（実際の成形後にrecord）、`ready_to_adopt`（委任された範囲でadopt）、`needs_review`（採用直後または変更後で、内容を確認してreview）、`reviewed`、`blocked`（blockersを解消）、`invalid`（errorを確認）。採用済み文書の変更にはrecordを再実行しない。
- 一覧・差分・export計画は `summary` が全体、`items` が最大20件。`page.next_offset` が数値なら、同じ読み取りコマンドに `--offset <値>` を付けて続ける。`--limit 1..100` で件数を指定できる。読み取り中に文書を変更した場合は先頭から確認し直す。
- 通常応答は最大16 KiB。`omitted_fields` にあるフィールドや `omitted: true` は未表示であり、空値・変更なしではない。`page.next_offset: null` でも値が省略されていれば全内容を確認したと扱わない。
- diffの詳細は `items` の `comparison/path/kind/before/after`。大きな値は `documents diff ... --out <新規JSONパス>` で完全な差分を保存し、必要な箇所を読む。exportの `--out` は原本と同形式の文書の書き込みで、応答は件数と `report` のパス。続きの取得のために書き込みコマンドを再実行しない。
- schemaは既定でプロパティ一覧。制約は `documents schema <種類> --pointer /properties/<名前>` で取得する。`--full` は全件・全値を返すため大きくなる。必要な読み取り操作だけで指定し、巨大な結果はファイルへリダイレクトする。
- ハッシュ照合が必要な場合だけ `--include-hashes` を付ける。本文は `content/`、検証状態はcheck/statusを参照する。保存された証跡とexportの `.report.json` は完全なハッシュと値を保持する。

- 同じ意味・対象・条件の重複は1項目へ統合し、各出典を根拠に残す。条件・例外の差や矛盾は消さない。不要表現は引用範囲と理由を付けて除外する。見出し・単位・注記は解釈根拠として保持し、意味不明・未処理の範囲を不要として除外しない。原文の処理状況と独立レビューの実施状況は別に報告する。
