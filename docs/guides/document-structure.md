# 原本を変更しない Office・PDF の構造解釈

構造の正規データは文書モデルの `mappings.yml` にある修正記録です。`documents structure-read` が出す整理YAMLは編集用の派生ビューで、`documents structure-save` が機械推定との差分を文書モデルへ保存します。`text` を `table` に変えたり、表を分割・統合して見出し対応を修正しても、Excelのセル値・書式・結合・配置は変更しません。`documents export/apply` は構造の修正記録を原本への書き戻しに使いません。

編集用ビューは、例えば `interpretations/<ID>.yml` に置きます。文書管理ディレクトリの中に置くと未管理ファイルとして拒否されます。Gitで管理する正規データは `.arp/documents/<ID>/mappings.yml` であり、ビューを編集した後は必ず `documents structure-save` を実行します。

`.arp/work/structure/<ID>.yml` を入力に指定した場合は、`documents structure-save` の保存成功後にその作業ファイルを自動削除します。保存に失敗した場合と、それ以外の場所にある入力ファイルは削除しません。取込候補の採用だけでは作業ファイルは削除されません。

この形式では `mappings.yml` の `interpretation` が必須です。以前の文書モデルは原本から新しい文書プロジェクトへ取り込み直し、必要な整理を `documents structure-save` で登録してください。

## 必須となる資料

Word（.docx）、PPTX、PDFは構造解釈・読取完了・現在の内容に対するacceptedレビューを必須とします。Excel（.xlsx/.xlsm）は、Excelテーブル定義またはセル配置から推定された表、画像、3個以上の図形からなる図表候補のいずれかがある場合に必須です。画像はOCR不可でも対象で、OCR可能な場合は実施結果を記録します。実行結果が空でも有効です。拡張子・図形数の閾値の正本は `contracts/structure-policy.json`、生成された一覧は機能リファレンスを参照してください。

通常のセル表の検出は既存の配置推定に依存します。図形の意味的な所属は機械判定できないため、同一シートのshape・connector・graphicが3個以上あれば図表候補とします。groupコンテナ自体は数えません。独立した図形が含まれていた場合も、その関係を確認してdescriptionに記録します。

captureは全資料の判定理由を `structure_requirements` に保存し、必須資料の承認済み整理が欠ける場合は出力しません。保存済み入力の読込みとrenderでも検証します。判定情報のない入力は再captureしてください。草稿もこの前提を省略できません。条件に該当しないExcelとテキスト原本は整理なしでcaptureできます。

Word・PPTX・PDFのsheet/cellsは既存パーサーの論理的な文字列コンテナです。A1等は文字列の通し番号で、物理的なセルやページ上の座標ではありません。本文・表・見出しの関係を原本から確認して整理します。自動画像描画はExcel限定で、他形式の表示確認には外部で取得したPNGをregionで登録できます。そのrangeには対応する文字列の通し番号の範囲を指定します。

## 画像・図表とOCRの記録

Excelの画像と図表候補はinitで `visuals` に未確認状態で作成されます。sourcesは抽出JSONのオブジェクトを指し、省略・差替えは拒否します。画像はOCR結果を採用した時点でstateをread、actorを採用者、descriptionを実施内容に更新できます。空の結果なら「OCR実行済み・認識文字なし」と記録し、画像を見ずにロゴと断定しません。図表は読み取った内容と図形同士の関係を記録します。未実施のnot_examinedや読取不能のunreadableは、実行済みOCRの空の結果とは区別します。LLMによる画像参照は完了条件ではありません。

Excelの埋込み画像は取込時にWindows OCRを自動実行し、抽出JSONのassetsに結果を記録します。structure initはこの結果をvisualsのocrへ引き継ぎます。OCRで結果を得た場合はstatusをavailableとし、textに実施結果（空文字も可）、reasonに使用手段と実施内容を記録します。OCRを利用できない、または画像を処理できない場合はunavailableとし、textを空、reasonに理由を記録します。OCR実行済みで文字が得られなかった場合をunavailableや未実施にしません。Windows以外では自動OCRは利用できません。

ユーザーから画像確認・補正の指示がなければ、空の結果を含めてOCR結果をそのまま採用します。空の結果にはロゴ等も含まれるため、これだけを理由に追加のLLM解析を始めたり、承認を止めたりしません。LLM解析はコストがかかるため任意です。

ユーザーから指示がある場合は、LLMが実際に画像を参照してOCR結果を修正できます。空の結果でも重要な内容があると判明した場合は、読み取った内容を反映します。修正後の文字列でocr.textを置き換えてよく、元のOCR文字列を別フィールドに残す必要はありません。ocr.llm_correctionにactor・user_instruction（指示内容）・判明しているmodel、ocr.reasonに補正内容を記録し、visualsのevidenceに参照した画像領域IDまたは抽出画像のsource参照を指定します。画像を開いていないのに画像確認済みと記録しません。置換すると既存レビューは失効するため、再レビュー・再captureします。

OCR結果とLLM補正結果は構造解釈の参考情報であり、原本から抽出した正確な引用文字列には追加しません。

## 抽出済みの図形・画像とグラフ

まず `spec structure read` の `drawings` を読みます。図形の文字・形状・位置・明示された接続先を簡潔に返し、空の任意項目は省きます。位置はセル座標と必要なEMUオフセットで示し、グループ内の変換は保持します。接続先IDだけで意味上の指揮命令・処理順序までは断定しません。詳細な原本情報は抽出JSONに保持し、整理YAMLへ転記する必要はありません。

埋め込み画像はimportで保存されたファイルをそのまま使用します。ファイル名は文書内の初出順に `image-001.png` のように付け、同じ内容の画像は共有します。SHA-256は抽出JSONに検証用として保持します。readは抽出JSONと同じディレクトリの `assets/` にあるファイルのハッシュを検証し、絶対パスを返します。`drawings` の `image_id` を `image_paths` の `id` と対応させ、`path` を画像閲覧機能で開きます。`image_paths` はハッシュを表示せず、配置ごとの `source`・シート・位置を返します。同じ画像を複数箇所に配置した場合、IDとパスは共通で、sourceは配置ごとに異なります。登録済み領域の画像IDは `region-<領域ID>` です。画像IDはこの文書の読取結果内の識別子で、再取り込みをまたぐ識別や根拠の参照には使いません。visualのevidenceには、そのvisualのsourcesにある画像のsource参照を指定します。撮影し直したり、画像のためだけにregionを作ったりする必要はありません。抽出JSONを移動する場合はassetsも一緒に保持します。形式によって画像閲覧機能が対応しない場合は、表示できる形式へ変換したPNGをregionに登録します。

図形の組合せや画像内の関係を整理できる場合は、visualに任意の `graph` を追加します。nodesは対象、edgesはfromからtoへの名前付きの関係です。ノード・関係のsourcesは、そのvisualの原本参照またはevidenceを指します。図形から読み取った関係と画像から解釈した関係はreadingに根拠を記録します。visionは実際の画像閲覧とvisualのevidenceを必要とします。OCRの文字列だけから線の接続や階層を推測しません。関係の循環や自己参照は図の内容として許し、存在しないノードや別visualの出典参照は拒否します。

例えば画像を指すvisualのsourcesが `/sheets/0/drawings/0` の場合、次のように関係を記録できます（visual内の抜粋）。

```yaml
evidence: [/sheets/0/drawings/0]
graph:
  nodes:
    - {id: manager, label: 管理者, sources: [/sheets/0/drawings/0]}
    - {id: operator, label: 担当者, sources: [/sheets/0/drawings/0]}
  edges:
    - {from: manager, to: operator, label: 指示, sources: [/sheets/0/drawings/0]}
  reading: {method: vision, actor: reader-1, reason: 抽出画像の文字と矢印を確認}
```

グラフは構造の解釈であり、原文引用を新設しません。acceptedレビューとcaptureを経て意味抽出・独立レビューのpacketへ渡されます。ラベル・接続・根拠を編集した場合も再レビューが必要です。

## 整理と画像確認

```powershell
arp4 documents structure-read order --root C:/project --out C:/project/interpretations/order.yml
arp4 spec structure --root C:/project check --extraction C:/project/.arp/documents/order/extraction.json --structure C:/project/interpretations/order.yml
```

初期状態はシートごとに `kind: text`、セルごとに `role: unassigned`、`text_state: not_examined` です。これは「文章だと判定済み」を意味しません。Agentまたは人が原本を読み、まとまりに応じてelementsを分割・統合し、kind・role・headers・text_state・readingを編集します。セル値は複製せず、sheetとaddressで原本の抽出セルを参照します。抽出済みセルは数式・注記を含め、ちょうど一つのelementへ所属させます。未収録の座標は確認済みの空欄とはみなしません。

表題と表の対応先が明確な場合は、表題セルも対応する `table` elementに含め、roleは `text` とします。表題を列見出しやデータへ変える必要はありません。文書全体のタイトルや複数の表に共通する節見出しを、無理に一つの表へ所属させないでください。対応が曖昧なら本文に保持し、判断できない関係を実施記録へ残します。表題を別本文に置く場合に失われる、表と表題の対応を明示するための操作手順です。

表の説明を実データから分離できる場合は、説明セルを別のtext elementへ移し、table elementの `descriptions: [説明elementのID]` で読む順序を指定します。tableのcellsは表本体・見出し・単位を保持します。説明のセルはtext側だけが所有し、tableへ複製しません。参照は同じシートのtext elementに限定し、複数の表から共通の説明を参照できます。自由な要約文で原本セルを置換せず、分離した説明の原文と対応を意味抽出へ引き継ぎます。説明が表専用か、共通の背景説明かは原文とreadingの理由に基づいて判断します。

多段見出しは各セルの `headers` に同じelement内の見出しセルIDを列挙します。見出しセル自身にも上位見出しを指定できます。複数の見出し・循環・存在しないIDを検証します。原本の結合範囲は抽出結果に残り、headersの編集では変わりません。結合に隠れたセルの値も削除・上書きせず保持し、見え方との違いをreading.reasonで説明します。

複雑な表は `render` でExcelの指定範囲をPNGへ描画し、画像領域と原本・画像のハッシュを一度に登録できます。Windowsとデスクトップ版Microsoft Excelが必要です。原本を読み取り専用で開き、Excelの描画結果を別の一時ブック経由で出力します。原本は保存せず、前後のハッシュも確認します。

```powershell
arp4 spec structure --root C:/project render --extraction C:/project/.arp/documents/order/extraction.json --structure C:/project/interpretations/order.yml --element sheet-1 --id sheet-view --image evidence/order-sheet.png --range A1:H30
```

対象シートは指定したelementまたはvisualのsheetから決まります。`--range` を省略すると、elementはシートのUsedRange、visualは参照図形のセルアンカーを囲む範囲を使います。oneCellAnchor・absoluteAnchorなどセル範囲を確定できないvisualではrangeを明示します。回転・はみ出しや周囲の説明がある場合も、必要な範囲を明示して確認します。画像を自動縮小して読めなくすることはせず、8192px/辺または3200万画素の上限を超える範囲は拒否します。その場合はセル範囲を分割し、異なるidとimageで取得します。非表示シートは表示状態を変更せず拒否します。UsedRangeの外の図形や画像まで必要なら、それを含むセル範囲を明示してください。これは範囲の描画であり、埋め込み画像ファイルを個別に抽出する機能ではありません。

応答の `image_path` をAgentの画像閲覧機能で開きます。`rendering` にExcelのバージョン、実際のセル範囲、画像サイズを返します。描画・登録しただけではvision読取やレビュー完了にはならず、既存レビューはpendingになります。対話型Agentは必要な範囲のrender→画像確認→整理YAML修正→check/review→structure-save→documents review→captureを実施できます。

ExcelのCopyPictureを使うため、**実行時にWindowsのクリップボードが画像に置き換わります**。ARP同士の描画はセッション内で排他します。既存のユーザーのExcelプロセスは使わず、専用プロセスを作成します。VBAとイベント、外部リンク更新、自動計算を抑制し、Excel 4.0マクロシートや外部データ接続を持つブックは拒否します。標準タイムアウトは120秒で、COM呼び出しが停止した場合も、隔離済みExcelをヘルパーとともに終了します。無人サービス環境でのOffice動作や、保護・パスワード付きブックは保証しません。

外部で取得した原本由来PNGを使う場合は、次のregionコマンドで登録します。生成AIの画像を証拠として使いません。

```powershell
arp4 spec structure --root C:/project region --extraction C:/project/.arp/documents/order/extraction.json --structure C:/project/interpretations/order.yml --element sheet-1 --id sheet-view --image evidence/order-sheet.png --range A1:H30 --bbox 0,0,1,1
arp4 spec structure --root C:/project read --extraction C:/project/.arp/documents/order/extraction.json --structure C:/project/interpretations/order.yml
```

`range` と `bbox` は同じ領域を指します。bboxは渡す画像全体を基準にしたx,y,width,height（0〜1）です。切り抜きPNGを渡すなら、そのPNG内の座標を指定します。regionコマンドは画像のハッシュと現在の原本ハッシュを登録し、指定elementのevidenceへ追加します。画像は `.arp/work/` や `.arp/cache/` に置かず、原本・整理YAMLとともに保持してください。

`read` は整理結果、原文セル、簡潔な図形情報、検証済みのローカル画像パス・ハッシュを返します。登録領域にはbboxも付けます。画像閲覧機能を持つ対話型Agentはそのパスを画像として開き、原文セルと比較します。JSONにパスがあるだけでは画像を見たことになりません。画像を見た場合だけreading.methodをvisionにし、actor・判明しているmodel・判断理由を記録します。画像閲覧できない実行環境では未確認として残します。既存のsemantic自動runnerへ画像バイトを自動添付する機能はありません。

ハッシュは原本と画像の版を固定しますが、画像がその原本のその領域を実際に写していることまでは証明しません。対応範囲・解像度・見出し・隠れた行列・注記をレビューしてください。visionの各セルは、参照画像のセル範囲に含まれる必要があります。

## 読取状態とレビュー

セルは、原文が読めたread、空欄を確認したempty、確認したが読めなかったunreadable、未確認のnot_examinedを区別します。数式のあるセルや原文が存在するセルをemptyにできません。画像だけに存在するOCR文字列を原文として追加する契約はなく、既存の引用を画像説明に置き換えません。

```powershell
arp4 spec structure --root C:/project review --extraction C:/project/.arp/documents/order/extraction.json --structure C:/project/interpretations/order.yml --decision accepted --actor reviewer --reason "原本・画像とセル、見出し、注記の対応を確認"
```

reviewは実際に確認した人・Agentが実施記録を保存する操作です。機械検証の成功だけでレビュー済みにしません。accepted/rejectedは整理全体の内容ハッシュに結び付きます。未確認・読取不能・未割当のroleが残るacceptedは拒否します。kind・headers・画像参照・readingなどを変更すると以前のレビューは失効します。rejectedの記録は保存できますがcaptureには使えません。

## 意味抽出への接続

```powershell
arp4 documents structure-save order --root C:/project --input C:/project/interpretations/order.yml
arp4 documents review order --root C:/project --reviewer reviewer
arp4 spec capture --root C:/project --extraction C:/project/.arp/documents/order/extraction.json --document order --out C:/project/.arp/cache/order-input.json
```

現在の原本・抽出結果・画像のハッシュと、文書モデル内のacceptedレビューを検証します。原文の文字列・出典ID・セル位置を変更せず、派生した整理結果を入力に保存し、抽出・独立レビューpacketの `sources.tables[].structure` にシートごとに渡します。構造変更でもpacketが変わるため、workflowへ新しいinputをupdateすると再抽出・再レビュー対象になります。構造の承認と、要件・仕様の意味レビューは別です。

整理を変更しても、すでに保存したinputや進行中workflowは自動更新されません。再captureし、既存のworkflow update手順で渡します。原本そのものが変わった場合は再importし、旧・新の抽出結果を照合してからレビューし直します。ハッシュだけを新しい値に書き換えて古い判断を承認済みにしません。

原本を変更する前に `documents structure-save` で修正を文書モデルへ保存します。記録には機械推定と異なる要素・図形だけを含め、参照する原本セル・図形オブジェクトのハッシュを付けます。画像領域の登録とレビューも記録します。現行の原本が必要なので、原本変更後に旧版の修正記録を新たに作ることはできません。

同じ文書ID・原本パスで再importすると、修正記録を候補へ引き継ぎ、新版の抽出結果へ自動で再適用します。`documents status` の `structure_conflicts` を確認し、採用後に `documents structure-read` で新版のビューを取り出します。セル位置が維持された修正要素は表・本文のまとまりを引き継ぎ、内容が変わったセルは `text_state: not_examined` に戻します。セルが削除・移動された要素や旧版の画像領域に依存する要素は新版の機械推定に戻します。図形・画像は参照先の抽出オブジェクトが一致し、旧版の画像領域に依存しない場合だけ引き継ぎます。原本ハッシュが同じなら画像領域とレビューを保持できます。原本が変わった場合のレビューはpendingです。競合箇所を確認・修正し、review、structure-save、documents review、再captureを行います。

契約の全フィールドは[生成リファレンス](../reference/document-structure-contract.md)、コマンド引数は[CLIリファレンス](../reference/commands.md)を参照してください。図・画像の自動要素分解、Word/PDF/PPTXの物理的な領域モデル、画像だけの文字からの出典生成は引き続き設計対象です。
