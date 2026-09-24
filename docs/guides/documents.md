# 文書操作と対応範囲

形式別の対応範囲と制限はこの文書にまとめます。導入とExcelの取り込みから書き戻しまでの手順は [導入ガイド](rust-preview.md)、応答と構造操作のフィールドは [CLI応答仕様](../reference/cli.md) を参照してください。

## 形式別の対応

| 原本 | 取り込み | 編集と反映 |
|---|---|---|
| Excel (`.xlsx` / `.xlsm`) | セル値・数式原文・結合範囲 | 通常セル値、明示した行列の追加削除、PNG追加 |
| Word (`.docx`) | 本文・表・ヘッダー等の文字列 | 既存runの文字列を編集し、同形式へ書き戻す |
| PowerPoint (`.pptx`) | スライド本文・表の文字列 | 既存runの文字列を編集し、同形式へ書き戻す |
| PDF (`.pdf`) | ページのテキスト描画命令内の文字列 | 元フォントで表現できる文字列へ置換し、同形式へ書き戻す |
| TXT・Markdown・CSV・TSV | UTF-8の本文と構造 | 原本を直接編集して同じ文書IDで再取り込み。export/applyは未対応 |

形式間の変換は行いません。表は概要であり、抽出対象外と書き戻し条件は以下を確認してください。

## Office・PDFと共通の制限

- 対応する取り込みは `.xlsx` / `.xlsm` / `.docx` / `.pptx` / `.pdf` / `.txt` / `.md` / `.csv` / `.tsv` です。テキスト・Markdown・CSV・TSVはUTF-8で構造に応じて取り込み、原本を直接編集して再取込します（export/applyは未対応）。詳細は後述のテキスト原本の説明を参照してください。Excelはセル値・数式原文・結合範囲を抽出します。図形・画像・コメント・印刷情報・OCRは未抽出で、候補と抽出JSONのR001に記録します。原本の外観も確認してください。
- Word/PPTX/PDFも[導入ガイド](rust-preview.md) と同じ `import → YAML編集 → record → adopt → review → export` を使います。出力は原本と同じ拡張子の新規ファイルを `.arp/cache/export/` に指定します。形式間の変換は行いません。詳細と検証手順は [複数形式の往復テスト](../../examples/document-roundtrip/README.md) を参照してください。
- Wordは本文・表・ヘッダー・フッター・脚注・文末脚注、PPTXはスライドの本文・表のXML文字列をrun単位で編集します。その他のZIP部品とrunの書式を保持します。PDFは各ページのテキスト描画命令内の文字列を編集し、原本のフォント・座標・画像等のオブジェクトを保持します。画像PDFのOCR、PDFのForm XObject内の文字・注釈・フォーム、PPTXのノート・マスター・グラフ内部は抽出対象外です。
- 他形式のYAMLも `blocks/table-1/rows/rN/A` を使います。この場合 `rN/A` は文字列の通し番号でありExcelの物理セルではありません。Wordの `document.yml` は本文、PPTXの `slide-N.yml` はスライド、PDFの `page-N.yml` はページに対応します。文字列を空にするには `""` を指定してください。Word/PPTX/PDFのrun・ページ・スライドの追加削除は未対応です。Excelのみ行列の追加削除とPNG画像の追加に対応します。
- Word/PPTX/PDFの置換文字列には改行・タブを指定できません。既存の文字列単位で編集してください。PDFは元フォントの文字コードで往復変換できない変更を拒否します。暗号化PDFと署名付きPDF/Officeの書き戻しも対象外です。Wordの再組版、PPTX/PDFの文字のはみ出し・重なりは出力ファイルを開いて確認してください。PDFの自動改行・レイアウト再構築は行いません。
- Excelの書き戻しは通常セルの値、明示した行・列の追加削除、`assets/` のPNG画像を `add_image` 操作で指定したセル範囲へ追加する処理に対応します。既存画像のセルアンカーは構造変更に追従しますが、画像の抽出・完全な外観再構築、回転・トリミング・絶対座標、図形編集、COMによる書き戻しは未対応です。構造変更時は同一シートのA1形式の数式参照を移動し、キャッシュを無効化してExcel起動時の再計算を指定します。マクロ部品は保持対象ですが、マクロ実行の検証はしていません。WindowsのExcel COMは `spec structure render` の読み取り専用描画に使用します。
- Excelの見出し推定・空行ごとの表分割は未対応です。元の行・列を保つ表を生成します。数式原文は別の表に保持します。
- `resume`、`edit-base/plan/apply`、文書内のローカルリンクを持つ本文の検証は未対応です。`spec` によるメタモデル検証と Markdown 設計書生成は [仕様書生成](../reference/specifications.md)を参照してください。
- 保存形式は `contracts/document-schemas.json`、出典・成形とレビューのハッシュは正規化した JSON を基準にします。
- diffは本文・対応表・除外理由・原本情報・画像ハッシュを比較します。画像の見た目は比較しません。
  通常応答は `items` に変更内容を出力します。`--out <新規パス>` で全差分JSON、`--format markdown --out <新規パス>` で全差分Markdownを保存できます。標準出力は保存先を示すJSONです。


Excelの表を原本の表示と結び付け、書き戻しから独立して整理する手順は[原本を変更しない整理](document-structure.md)を参照してください。修正記録は文書モデルの `mappings.yml` に保存し、`documents structure-read/save` で整理ビューを編集します。WindowsとデスクトップExcelがあれば指定範囲をPNGに描画でき、領域・版の対応と多段見出し・読取レビュー状態を保持できます。図の要素分解と他形式の領域モデルは[文書表現案](../design/visual-document-model.md)の未実装部分です。

## テキスト・Markdown・CSV・TSV原本

`.txt` / `.md` / `.csv` / `.tsv` は正式な文書原本として取り込めます。UTF-8（BOMあり・なし）、最大32 MiBに対応します。TXT・Markdownは最大1,048,576行、CSV・TSVは最大16,384列・1,048,576フィールドです。文字コードを推測・変換せず、非UTF-8やNULを含む入力は拒否します。Shift_JIS・UTF-16は、利用者が原本をUTF-8に変換してから取り込んでください。

```powershell
arp4 documents import docs/requirements.md --id requirements --root C:/my-project
# 返された候補IDの内容を確認し、判断記録を保存して採用する
arp4 documents record <候補ID> --model <モデル名> --actor <担当> --prompt C:/my-project/review.txt --root C:/my-project
arp4 documents adopt <候補ID> --reviewer <担当> --root C:/my-project
arp4 spec capture --extraction C:/my-project/.arp/documents/requirements/extraction.json --out C:/my-project/.arp/cache/requirements-input.json
```

Officeと同じく、抽出結果を `spec capture` から要件・仕様の根拠として利用できます。レビュー・修正時だけの参考資料にする `spec workflow init --reference` とは用途が異なります。

本文は原本を直接編集します。編集後は同じ `--id requirements` で再度importし、候補を確認してrecord・adoptします。原本のハッシュが変わると以前の取込状態は古くなり、新しい候補に以前のレビューは引き継ぎません。再取込後のextractionからcaptureし直して要件・仕様のレビューを進めます。

`.arp/documents/<ID>/content/` に確認用の派生ビューを生成します。対応付けられた本文の変更はcheck時に拒否します。これらの原本に対する `export` / `apply` は未対応です。ARPは原本を書き換えないため、BOM・改行・末尾改行はそのまま残ります。外部エディタでの変更はそのエディタの保存設定に従います。

TXTは論理シート `text` の `A1`・`A2`…が原本の行番号です。空行も数え、LF・CRLF・CRを行区切りとして扱います。抽出値から行区切りと先頭BOMを除き、その他の空白・タブは保持します。空ファイルはimportできますが、抽出可能な本文がないためspec captureは拒否します。

MarkdownはCommonMarkのブロック構造と表・タスクリスト・脚注等の拡張を解析します。論理シート `text` の `A1`等はブロック番号です。見出し階層、段落、リスト、表、コード、引用等を識別し、本文はMarkdown記法を含む原文のまま保持します。複数行の段落、リスト全体、表全体、コードブロックをそれぞれ一つの出典とし、表には列見出しも付けます。コード内の `#` を見出しとして扱いません。リンク定義などパーサーのブロック外にある非空白の原文も出典として残します。リンク先や画像は読み込みません。

各出典の `position` はブロックの種類・見出し階層・行範囲（1始まり、両端含む）・UTF-8バイト範囲（0始まり、終端を含まない、BOMを含む原本基準）を持ちます。capture後の `context.position` と、抽出・レビューpacketの `sources.columns` にある `position` にも渡します。位置は原本ハッシュと組み合わせて使い、永続IDとはみなしません。以前の行単位Markdownの取込データは再importして作り直してください。

## 再取込の変更追跡

原本を編集して同じIDでimportした後、採用前に候補の差分を確認します。

```powershell
arp4 documents diff <候補ID> --root C:/my-project
# JSONの全詳細、または保存するMarkdownレポート
arp4 documents diff <候補ID> --full --root C:/my-project
arp4 documents diff <候補ID> --format markdown --out changes.md --root C:/my-project
```

通常のデータ差分に加え、TXT・Markdown・CSV・TSVの候補には `source_impact` が含まれます。本文・構造・見出しを比較し、以下を区別します。

| kind | 意味 |
|---|---|
| `moved` | 同じ見出し・構造・本文で、一意に対応する出典の位置移動 |
| `reordered` | 同じ出典同士の相対的な順番が変更された |
| `context_changed` | 一意に一致する本文が別の見出し・列文脈等へ移動した |
| `modified_candidate` | 同じ文脈に残った旧・新出典が各1件。本文変更の対応候補で、要確認 |
| `ambiguous` | 重複本文や複数の変更候補があり、対応を確定できない |
| `added` / `removed` | 追加・削除 |
| `stale_evidence` | レジストリの根拠が今回の比較元とは異なる版を参照している |

`.arp/registry/` があれば根拠の版を照合し、同じ見出し範囲（配下の小見出しを含む）の項目と要件参照・関連参照を通じた依存項目を `affected_entries` に表示します。本文変更・追加・削除・順序や文脈の変更・曖昧な対応は `review_required: true` です。TXT・CSVには見出し階層がないため、本文変更時は文書全体の根拠付き項目を保守的に対象にします。

このレポートは再レビュー対象の判断に使います。根拠ID・スナップショット・承認状態を自動更新したり、レビュー済み状態を新しい候補へ移したりはしません。`authority_changed: true` の場合は取込候補を作った後に採用側も変化しているため、最新の原本と採用状態から候補を作り直してください。

## CSV・TSV

CSVはカンマ、TSVはタブを区切り文字とします。先頭行も通常のレコードとして取り込み、ヘッダーや型を推測しません。全フィールドを文字列として保持するため、`001`・`false`・数式に見える文字列も変換・実行しません。引用符内の区切り文字と改行、二重引用符のエスケープ、空欄に対応します。空の物理行はレコードに数えませんが、`""` の空フィールドは保持します。列数不一致・閉じていない引用符・引用符外の不正な文字は拒否します。

論理シート `records` の行はレコード番号、列はフィールド番号です。`position.record`・`position.column` にその番号を持ち、行・バイト範囲は引用符を含む原本フィールドの範囲です。抽出値はCSV構文を復号した文字列なので、引用符やエスケープを含む原本断片そのものとは区別します。先頭行もcaptureの出典になり、空フィールドはextractionに保持します（空文字自体はcaptureの引用対象になりません）。

JSON・YAMLの形式別対応は含みません。`.arp/` 内の生成物は原本として取り込めません。
