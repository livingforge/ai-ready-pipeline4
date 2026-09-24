# ARP 操作リファレンス

必要な操作の節だけを読む。workflowの割当・再利用・終了判断は [進行管理](orchestration.md)、担当の読取・検証・提出手順はカスタムAgentの定義に従う。この資料全体を担当へ渡さない。

- 「文書ワークフロー」: import、成形、diff、原本への反映。
- 「メタモデルと設計書」: 構造解釈、capture、workflowの準備、明示的な個別CLI操作。
- 「正本の継続保守」: registryへの登録、更新、承認。
- 「CLI応答の読み方」: ページ送り、省略、エラーの確認。

以下の `docs/` で始まる文書パスはARPソースルート基準。導入先の文書プロジェクトに存在するとは限らない。実行時の引数・対応機能・返信契約はCLIのhelp・doctor・readを参照する。

## 文書ワークフロー

原本は既定で `docs/`、設定は `.arp/config.yml`。初期化は既存docsやAGENTS.mdを変更しない。採用データは `.arp/documents/<文書ID>/`、候補は `.arp/changes/`。原本コピーと版別保存は作らない。原本・変換結果・要件や判断をGitへコミットする。`.arp/work/` と `.arp/cache/` はGit対象外。

最初に `arp4 doctor` を確認する。`implementation: rust` の場合はこの節の範囲で作業する。
起動できない場合は [arp4-setup](../../arp4-setup/SKILL.md) を参照する。

- 初期化は `documents init --root <project>`、取り込みは `documents import <原本> --id <文書ID>`。
  対象は `.xlsx` / `.xlsm` のセル値・数式原文・結合範囲。PNG画像は `assets/` の画像を `add_image` 操作でセル範囲に追加できる。埋込み画像はWindowsで取込時にOCRを自動実行し、結果または失敗理由を抽出JSONのassetsへ記録する。図形の意味・コメント・印刷情報は未抽出で、R001に記録する。
  未抽出の情報は原本で確認し、読み取れたと扱わない。
- `.docx` / `.pptx` / `.pdf` も同じワークフローで取り込み・同形式出力できる。Wordは本文・表・ヘッダー等、PPTXはスライド本文・表のrun単位の文字列、PDFはページのテキスト描画文字列を扱う。本文の `rows/rN/A` は文字列の通し番号であり物理セルではない。既存文字列を編集し、空にするには `""` を使う。構造操作はExcel限定。PDFの画像/OCR・Form XObject・注釈・フォーム、PPTXのノート・マスター等は未抽出。PDFは原フォントで表現できない文字と改行・タブを拒否する。文字のはみ出しや再組版は出力ファイルを開いて確認する。
- `.txt` / `.md` / `.csv` / `.tsv` はUTF-8（BOM可、最大32 MiB）の正式な原本としてimportし、extractionからspec captureで出典化できる。TXTは行、Markdownは見出し・段落・リスト・表・コード等のブロック、CSV/TSVはレコード・列単位で抽出する。Markdown記法・複数行本文を保持し、positionに見出し階層・行範囲・原本基準のUTF-8バイト範囲を持つ。MarkdownのA1等はブロック番号、CSV/TSVのA1等は列・レコード番号で物理行ではない。CSV/TSVは全項目文字列、先頭行も通常レコードとして扱い、先頭ゼロ・空欄・引用符内改行を保持する。リンク先・画像は読まない。TXT/Markdownは最大1,048,576行、CSV/TSVは16,384列・1,048,576フィールド。非UTF-8・NUL・不正CSVを拒否し、型や文字コードは推測変換しない。
- 本文は原本を直接編集し、同じ文書IDで再import→候補のdiff→record→adoptする。contentのYAMLは確認用の派生ビューで、本文変更・構造操作・export/applyは未対応。再取込候補の `documents diff <候補ID>` はsource_impactで位置移動・文脈/順序変更・本文変更候補・追加削除・曖昧な対応を示し、registryがあれば同じ見出し範囲と依存する項目をaffected_entriesに示す。modified_candidate・ambiguousは対応を自動確定しない。根拠と承認は自動移行せず、再取込後はcaptureを更新して必要なレビューを行う。参照資料だけに使う--referenceとは区別する。
- importのJSONにある `proposal_id` と `proposal` を使い、本文YAMLと対応表を確認して実際の成形を行う。
  Word/PPTX/PDFは置換文字列に改行・タブを含めず、既存runを個別に編集する。
  型、page_id、既存の行・列、field ID、セル対応を維持する。行・列の追加削除は管理側 `mappings.yml` に `insert_rows` / `delete_rows` / `insert_columns` / `delete_columns` をreason付きで記録する。本文の追加行・列のmappingはCLIが再生成する。
- `documents check --proposal <候補ID>` と `documents diff <候補ID>` で確認し、
  実際に作業したactor/model/promptを `documents record <候補ID> --model <モデル> --actor <担当> --prompt <ファイル>` で記録する。
  実施していない成形を記録しない。権限のある担当者が `documents adopt <候補ID> --reviewer <担当>` を行う。
- 採用後は既存の本文の値を同じ型で編集し、`documents check <文書ID>`、`documents diff --document <文書ID>`、
  `documents review <文書ID> --reviewer <担当>`、`documents export <文書ID> --out <プロジェクト>/.arp/cache/export/<新規名>.<原本と同じ拡張子>` の順で確認・反映する。
  文字列の先頭が `=` でも数式化しない。既存数式の直接変更、図形操作、画像の回転・トリミング・絶対座標は拒否される。`add_image` は `anchor.from.cell` / `anchor.to.cell` で配置する。構造変更時は同一シートのA1形式の数式参照と既存Drawingのセルアンカーを移動し、Excelで再計算する。
- 原本へ反映する場合はレビュー後に `documents apply <文書ID>`。原本変更を検出すると拒否する。成功後は再抽出した候補が `needs_record` になり、確認・record・adoptを行う。`diff --document` はGitのHEADとの比較なので先に基準をコミットする。
- 中断時は `documents status`。`resume`、`edit-base/plan` は未対応なので呼び出さない。
  型や構造の変更が必要なら未対応と報告し、検証を回避しない。
- `--root` は全コマンドで指定できる。importの相対パスはプロジェクト基準、prompt・outの相対パスは実行ディレクトリ基準。

## メタモデルと設計書

Word・PPTX・PDFは構造解釈・読取完了・acceptedレビューを必須とする。Excelは表（セル配置からの推定を含む）・画像・同一シートに3個以上の図形による図表候補のいずれかがあれば必須。画像はOCRが可能なら実施結果もvisualsへ記録する。OCR不可なら理由を記録し、未確認のまま承認しない。詳細な判定と読取手順は docs/guides/document-structure.md。条件に該当するcaptureでは `--root` と `--document` を省略しない。

OCR実行済みの空の結果も有効とし、未実施・OCR不可とは区別する。ユーザー指示がなければ空の結果も含めてOCR結果を採用し、追加のLLM画像解析は行わない。OCR結果の採用をもって画像のreadを記録できるが、画像を見たとは申告しない。LLM画像解析はコストがかかるため任意。ユーザーから画像確認・補正の指示がある場合は実画像を参照し、必要な補正でocr.textを置換してよい。元OCR文字列の別保存は不要。ocr.llm_correctionに担当・指示内容・判明しているモデル、reasonに補正内容、visualsのevidenceに画像領域IDまたは抽出画像のsource参照を記録し、再レビュー・再captureする。空の結果だけからロゴと断定しない。

- Excelの構造解釈は `documents structure-read <ID> --out <整理YAML>` で編集用ビューを取り出し、`spec structure check/read/region/render/review` で確認してから `documents structure-save <ID> --input <整理YAML>` で文書モデルのmappingsへ修正を保存する。まずreadの簡潔なdrawingsとimage_pathsを使い、図形の文字・形状・位置・明示接続先を読む。drawingsのimage_idをimage_pathsのidと対応させ、pathの画像を直接開く。画像IDは文書の読取結果内の識別子であり、visualのevidenceにはIDではなく画像のsource参照を記録する。同じ画像の複数配置はIDを共有するがsourceは異なる。画像ファイル名は短い連番で、ハッシュ検証はCLI内部で行う。図・画像の関係はvisualのgraphへ根拠付きで整理する。表の説明を別text elementへ分離する場合は、tableのdescriptionsから参照し、セルを二重所属させない。
- 見出し・結合・図形の配置等を描画して確認する場合は `spec structure --root <root> render --extraction <抽出JSON> --structure <整理YAML> --element <elementまたはvisualのID> --id <画像領域ID> --image evidence/<新規名>.png --range A1:H30` を呼び、返されたimage_pathを開く。range省略時はvisualのセルアンカー範囲、elementはシートのUsedRangeを使う。アンカーから範囲を決められないvisualはrangeを明示する。図形を個別に切って関係を失わないよう、図のまとまりを含める。WindowsとデスクトップExcelが必要で、クリップボードは画像に置き換わる。大きすぎる範囲は分割し、外部で用意したPNGはregionで登録する。実際に画像を見てからvision読取を記録し、描画成功だけで確認済みにしない。semantic自動runnerへの画像自動添付は未対応。画像取得・閲覧できなければ未確認・読取不能を残す。acceptedレビュー後に `documents structure-save <ID> --input <整理YAML>` と `documents review <ID> --reviewer <担当>` を行い、`spec capture --root <root> --extraction <抽出JSON> --document <ID> --out <入力JSON>` で意味抽出に接続する。編集後は再capture・workflow update・再レビューが必要。整理YAMLは派生ビューで、原本への書き戻しには使わない。詳しくは docs/guides/document-structure.md。

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
- check/statusの `items[].state` は `needs_record`（実際の成形後にrecord）、`ready_to_adopt`（権限のある担当者がadopt）、`needs_review`（変更内容を確認してreview）、`reviewed`、`blocked`（blockersを解消）、`invalid`（errorを確認）。採用済み文書の変更にはrecordを再実行しない。
- 一覧・差分・export計画は `summary` が全体、`items` が最大20件。`page.next_offset` が数値なら、同じ読み取りコマンドに `--offset <値>` を付けて続ける。`--limit 1..100` で件数を指定できる。読み取り中に文書を変更した場合は先頭から確認し直す。
- 通常応答は最大16 KiB。`omitted_fields` にあるフィールドや `omitted: true` は未表示であり、空値・変更なしではない。`page.next_offset: null` でも値が省略されていれば全内容を確認したと扱わない。
- diffの詳細は `items` の `comparison/path/kind/before/after`。大きな値は `documents diff ... --out <新規JSONパス>` で完全な差分を保存し、必要な箇所を読む。exportの `--out` は原本と同形式の文書の書き込みで、応答は件数と `report` のパス。続きの取得のために書き込みコマンドを再実行しない。
- schemaは既定でプロパティ一覧。制約は `documents schema <種類> --pointer /properties/<名前>` で取得する。`--full` は全件・全値を返すため大きくなる。必要な読み取り操作だけで指定し、巨大な結果はファイルへリダイレクトする。
- ハッシュ照合が必要な場合だけ `--include-hashes` を付ける。本文は `content/`、検証状態はcheck/statusを参照する。保存された証跡とexportの `.report.json` は完全なハッシュと値を保持する。

- 同じ意味・対象・条件の重複は1項目へ統合し、各出典を根拠に残す。条件・例外の差や矛盾は消さない。不要表現は引用範囲と理由を付けて除外する。見出し・単位・注記は解釈根拠として保持し、意味不明・未処理の範囲を不要として除外しない。原文の処理状況と独立レビューの実施状況は別に報告する。
