# 文書正本の管理とExcel書き戻し

`arp4 documents` は、既存開発環境へ組み込むための文書管理コマンドです。
Office原本 → 機械抽出JSON → Agentによる成形 → 編集可能なMarkdown正本 → Excel書き戻しを扱います。
対応表の版は `2`（旧版 `1` も読み込み可能）、本文・抽出の版は `1` です。
CLIから `arp4 documents schema <種類>` でJSON Schemaを取得できます。

## 導入と配置

ARP本体はPythonパッケージとしてインストールします。利用先へARPの `src/`、`tests/`、
`pyproject.toml` をコピーする必要はありません。`build/deploy.py` はプロジェクト導入用の
`.arp/bootstrap/`（wheel・固定した依存・導入スクリプト）を `arp4-publish` に作成します。

```powershell
python C:/arp4-publish/.arp/bootstrap/install.py --root C:/my-project
```

この配布物は作成時のPythonマイナーバージョン・OS・CPU向けです。対応環境はmanifest.jsonに記録します。
導入はオフラインで行い、`.arp/runtime/` の専用環境と文書管理を初期化します。
テンプレートには `.arp/bootstrap/` をそのまま組み込めます。

開発中の本体を利用する例（Windows）:

```powershell
python -m venv C:/my-project/.arp/runtime
C:/my-project/.arp/runtime/Scripts/python.exe -m pip install "C:/arp4[parse]"
C:/my-project/.arp/runtime/Scripts/arp4.exe documents init --root C:/my-project
```

テンプレート配布では、検証したwheelと依存関係を固定して導入してください。アプリケーション側の
Python環境を共有する必要はありません。以下の例の `arp4` はインストールした実行ファイルです。

```text
project/
  src/                         既存アプリケーション
  tests/                       既存アプリケーションのテスト
  docs/                        既存の開発文書
  knowledge/                   --directory で変更可能
    README.md                  読む順序・正本・更新ルール
    AGENTS.md                  編集・検索・成形の手順
    documents/order-design/
      document.yml             文書ID、採用した抽出結果、原本の版
      original/基本設計.xlsx     集約した原本。Git管理・更新時は再取り込み
      content/*.md             通常検索する正本。直接編集可
      mappings.yml             出典とExcel書き戻し先
      assets/                  本文に必要な画像
      formation.json           成形時の実行記録への対応
      review.json              レビューした内容のハッシュ
    spec/                      文書から整理した仕様データ。Git管理
  .arp/
    config.yml                 文書・仕様データの配置設定
    documents.code-workspace   既存設定を上書きしないVSCode設定
    evidence/                  Git管理。原本・抽出・成形の根拠
    proposals/                 Git管理。未採用候補・置換前の正本
    rounds/                    既存仕様パイプラインの処理記録
    cache/                     Git対象外。再生成可能なキャッシュ
    out/                       Git対象外。Excelなどの出力
    runtime/                   Git対象外。任意の専用実行環境
```

正本の過去版はGit履歴で管理します。出典として参照される抽出・成形の根拠は
内容ハッシュで固定した `evidence/` に保持します。古い根拠や候補の自動削除は行いません。
`.arp/` を丸ごとignoreしないでください。バイナリ原本・抽出結果・正本・対応表を一緒にコミットします。
`review` / `adopt` はGitのコミットやブランチ操作、リモートへの送信をしません。

## 正本の定義

このモードでは `knowledge/documents/*/content/` が開発で参照する文書の正本です。
原本と内容が異なることは正常です。`original/` は再取り込みする原本の置き場で、受領時点の版は
`.arp/evidence/sources/` に固定して保持します。
`knowledge/spec/` はこの文書から整理した仕様データとして扱い、食い違いは文書の正本を基準にレビューします。
既存の `parse → freeze → build → publish` モードのコマンドと保存形式は互換性のため維持しています。
配置設定のない旧プロジェクトのみ `.arp/spec/` を使います。文書正本モードでは共通のパス解決を通して
同じコマンドが `knowledge/spec/` を読み書きします。

配置設定 `.arp/config.yml` の例:

```yaml
schema_version: '1'
documents:
  directory: knowledge
spec:
  directory: knowledge/spec
```

`--directory` を変えた場合、仕様データもその下の `spec/` へ配置します。
モデル名やプロンプトは成形記録、公開設定など従来の個別機能の設定は既存ファイルで管理します。

### 旧構成からの移行

```shell
arp4 documents upgrade-layout --root <project>
```

旧 `.arp/documents.yml` を `.arp/config.yml` へ移行し、旧設定は
`.arp/layout-legacy-documents.yml` に保存します。`.arp/spec/` は内容を変更せず
`knowledge/spec/` へ移します。両方に仕様データが存在する場合は上書きせず停止します。
原本を集約した新しい文書候補に現在の本文・対応表・画像を引き継ぎ、候補名を
`.arp/layout-migration.json` に記録します。再実行時は記録済みの同じ移行を繰り返しません。
移行候補の確認・成形記録・採用が必要です。旧正本・候補・抽出・レビュー記録は書き換えません。
旧原本が取り込み時から変更されている場合は、先に変更内容を照合してください。

文書の採用は `adopt`、直接編集後のレビューは `review` です。これらは操作した人・Agentの申告を
内容ハッシュへ結びつけます。署名や認証の代わりではありません。基準ブランチへの採用権限はPRで管理します。

## 取り込み・LLM成形・採用

```shell
arp4 documents init --root <project> --directory knowledge
arp4 documents import docs/基本設計.xlsx --id order-design --root <project>
```

初回の原本はプロジェクト内に置きます。取り込み時に `knowledge/documents/<文書ID>/original/` へコピーし、
以後はその原本を更新・再取り込みします。入力元は削除しません。同名の集約済み原本と内容が異なる入力を
渡すと停止するため、更新は `original/` 内で行ってください。文書IDはファイル名を変更しても維持します。
原本のバイト列を固定して保存し、機械抽出JSON、画像、
未採用候補、候補と同名の `.prompt.md` を作成します。採用済み正本は変更しません。
Excel・Word・PowerPoint・PDF・CSVなど、既存パーサーの対応形式を利用できます。
環境依存のOCRはこの取り込みでは無効にし、未読取の申告を残します。

**LLMを呼び出すのは作業中のAgentです。** 候補の `.prompt.md` と `knowledge/AGENTS.md` に従い、
抽出JSON・画像を読んで候補の `content/` と `mappings.yml` を編集します。APIキーやモデルへの外部送信を
CLIへ暗黙に組み込んでいません。機械生成の候補をLLMの成形結果として記録しないでください。

```shell
arp4 documents check --proposal <候補名> --root <project>
arp4 documents record <候補名> --model <実際のモデル> --actor <担当> --prompt <実際の手順.md> --root <project>
arp4 documents diff <候補名> --root <project>
arp4 documents adopt <候補名> --reviewer <担当> --root <project>
```

`record` は入力の抽出ハッシュ、実際の成形出力、モデル、担当、プロンプト原文・ハッシュを保存します。
使用モデルが不明なら `unknown`、人だけで成形した場合は `human` とします。
記録後に候補を編集した場合は、再度recordが必要です。LLM出力の再現性を前提にせず、出力そのものを保存します。

## Markdownの契約

```markdown
---
schema_version: "1"
document_id: order-design
page_id: order-fields
---

# 受注項目

<!-- arp:block id="order-fields" -->

## 項目の定義

受注番号の項目定義を以下に示す。

| id | type | value |
| --- | --- | --- |
| order-number-length | number | 10 |
| order-number-label | string | "受注番号" |
```

- ファイル先頭に必須frontmatter。文書内でページIDは一意です。
- H1は先頭に一つ。以降の本文は単独行の管理マーカーで始まる節に所属します。
- 節IDはページ内で一意。管理マーカー風の文字列をコードフェンス内に書いても制御情報になりません。
- 自由な文章・通常のMarkdown表・コード・画像を使えます。生HTMLは管理マーカー以外禁止です。
- 全ての表で先頭・末尾の `|` と一定の列数を要求します。
- `id / type / value` の表は書き戻し値の表です。型はstring/number/boolean/null、値はJSONリテラルです。
  値の中のバックスラッシュはMarkdown用にもう一度エスケープし、`|` は `\|` とします。
  この表の値はMarkdownのリンクやHTMLとして解釈しません。
- ローカルリンク・画像は文書ディレクトリ内に限定します。フラグメントは節IDとして検証します。
  管理コメントはHTMLアンカーではないため、Markdownプレビューのスクロール移動までは保証しません。
- 文書ID・ページID・節ID・行IDは表示名と分けて維持します。取り込み直後の仮IDは成形時に整理できます。

JSON Schemaだけでは節、表、参照、出典の整合性は検証できません。Markdown構文解析と相互参照検査も実行します。
形式検証は記述の意味の正しさを保証しません。説明文と型付き表へ同じ数値を重複記述した場合も、意味の整合性はレビュー対象です。

## 出典と書き戻し先

`mappings.yml` の例:

```yaml
schema_version: "1"
entries:
  - page: order-fields
    block: order-fields
    field: null
    origins:
      - page: page-1
        block: s1-t1
    reason: 説明文。数値変更は型付き項目で指定する
    writeback: excluded
    target: null
  - page: order-fields
    block: order-fields
    field: order-number-length
    origins: []
    reason: 原本の桁数セルに対応する
    writeback: cell
    target:
      sheet: 項目定義
      cell: D8
omissions: []
```

これは抜粋です。実際には全ての節・型付き項目への対応が必要です。出典のpage/blockは
`document.yml` が固定した抽出JSON内のIDです。書き戻し先は独立したtargetです。
`pending` は未確定、`excluded` は理由付き対象外、`cell` は既存セルへの更新です。
新規記述に架空の原本出典は不要ですが、reasonが必要です。

抽出した節・セルを正本から削除する場合、`omissions` に `origin` または `target` と `reason` を追加します。
これは「原本セルを消す」という指示ではなく、採用・書き戻しの対象外にする宣言です。無断の情報脱落を検出します。

## 直接編集・検証・検索

```shell
arp4 documents check --root <project>
arp4 documents review order-design --reviewer <担当> --root <project>
arp4 documents check --require-reviewed --format json --root <project>
arp4 documents list --root <project>
```

検証は形式、出典、原本の変更、レビュー、書き戻し対応を分けて報告します。
形式エラーは終了コード1。通常checkでの未レビュー・原本更新・書き戻しpendingは警告です。
CIで `--require-reviewed` を指定すると、本文・対応表・画像などがレビュー後に変わっている場合も終了コード1です。

VSCodeでは `.arp/documents.code-workspace` を開きます。タスク `ARP: validate on save` を起動すると、
保存後に再検証してProblemsへ表示します。終了はタスク停止またはCtrl+Cです。
ワークスペースには機械抽出・候補・旧ラウンド・生成物の検索除外を用意しています。
文書だけを検索するときは、検索のfiles to includeへ `knowledge/documents/*/content/**/*.md` を指定します。
Agentの別の検索手段にも同じ範囲を渡してください。

## 再取り込み・移行・仕様パイプラインとの連携

原本更新後に同じ文書IDでimportすると新しい候補を作ります。基準版、現在の正本、候補の差分はdiffで確認できます。
前回の成形結果は `formation.json` が参照する `evidence/formed/<content-hash>/` にあります。
取り込み後に現在の正本が編集されていた場合、adoptは停止します。再取り込みして現在の正本と統合してください。
行の追加・並べ替えがある場合、セル番地だけで項目の同一性を決めず、ID・出典・targetをレビューします。
自動的な意味のマージは行いません。

```shell
arp4 documents migrate .arp/rounds/r001/parsed --source docs/基本設計.xlsx --id order-design --root <project>
arp4 documents prepare-spec order-design --root <project>
```

migrateは旧Markdownのsourceコメントで原本に対応するファイルを選び、編集済み内容を候補へ保存します。
旧内容はまずコードフェンス内に保持し、Agentが新規約へ成形します。旧ファイルは変更しません。
古い編集の内容を機械抽出した事実として扱いません。画像などは原本から改めて抽出したものと照合してください。
prepare-specは正本の形式・レビューを検証してから既存パイプラインの新規ラウンドを作ります。
そこから整理・freeze・build・publishを利用できます。ラウンドには入力正本のハッシュを残します。

## Excel書き戻し

```shell
arp4 documents export order-design --root <project>
arp4 documents export order-design --out .arp/out/基本設計-revised.xlsx --root <project>
```

引数なしのexportは計画だけをJSONで表示します。pendingがある場合は未反映を報告し、終了コード1です。
`--out` は未反映がなく、現在の正本がレビュー済みで、原本の版が一致している場合だけ出力します。
原本や既存成果物は上書きしません。出力は `.arp/out/` 内の新規ファイルに限定します。

対応範囲:

- `.xlsx` / `.xlsm` の既存セルの文字列・数値・真偽値・空欄への更新。
- 既存セルの型を維持。結合セルは左上セルだけを更新。
- 通常のセル値更新はXML方式で、変更不要なZIPエントリーの内容をそのまま保持。
- 行の挿入・削除、数式の設定・変更・クリア、図形の追加・削除・編集、画像の追加・置換、接続線の追加に対応。
  これらはWindows + Microsoft Excelを使うExcel方式で処理し、保存後に再オープンして照合。
- Excel方式はExcel自身がブックを保存するため、変更対象外XMLのバイト同一性は保証しない。
  既存セルの移動、明示した値・数式、図形の属性と接続を検証し、変更されたZIPパートを報告する。
- デジタル署名付きブックの更新は署名を無効化するため拒否。列の挿入・削除は今回の操作契約に含めない。
- 数式のあるブックで値を変更した場合、数式の計算キャッシュを削除し、Excelでの全再計算を指定。
  ARP自身は数式を計算しない。計算前のキャッシュに依存する他ツールには注意が必要。
- 出力を再パースして更新値を照合。隣に `.report.json` を作り、入力版・変更・対象外・未反映・再計算要否を記録。

Excelアプリケーションでの表示・全機能の動作までをこの照合だけで保証するものではありません。
出力Excelが顧客から更新されて戻った場合は、プロジェクト内の原本の場所へ置いて再取り込みします。

### 行・数式・図形の書き戻し

```shell
python -m pip install "ai-ready-pipeline4[parse,writeback]"
arp4 documents drawings order-design --root <project>
arp4 documents export order-design --engine auto --out .arp/out/updated.xlsx --root <project>
```

`auto` は必要な変更から `xml` / `excel` を選びます。`--engine xml` で高度な更新を要求すると停止します。
Excel方式は別プロセスの非表示Excelを起動し、マクロ・イベント・リンク更新を無効化します。
ユーザーが開いているExcelには接続せず、原本は読み取り専用で開き、ステージング先へ保存します。
120秒のタイムアウトを設け、異常時は自分が起動したExcelだけを終了します。
無効な数式などがある場合は新しい成果物を採用せず、元ファイルを変更しません。

既存の対応表を拡張する場合、`schema_version: "2"` と `operations: []` を設定します。
本文・対応表の変更でレビュー状態は無効になるため、check → reviewを実行してください。

行操作は **原本の行番号** で指定します。複数操作はシートごとに下の行から実行され、重複する範囲は拒否されます。
既存セルのtargetは原本のまま維持し、移動先を機械が計算します。削除行に書き戻す有効な項目が残っていれば停止します。
削除する値は正本から取り除くか理由付きexcludedにします。delete_rows自体が削除の根拠を記録します。

```yaml
schema_version: "2"
entries:
  # 既存の対応に加え、本文のnew-order項目を挿入した行へ書く例
  - page: order-fields
    block: order-fields
    field: new-order
    origins: []
    reason: 新規項目
    writeback: cell
    target: {sheet: 項目定義, insertion: add-order, offset: 0, column: B}
omissions: []
operations:
  - id: add-order
    kind: insert_rows
    sheet: 項目定義
    at: 8
    count: 2
    style_from: 7
    reason: 項目を2件追加
  - id: remove-old
    kind: delete_rows
    sheet: 項目定義
    at: 20
    count: 1
    reason: 廃止した項目を削除
```

`offset` は追加した行の中の0始まりの位置です。`style_from` は任意で、原本の指定行から書式をコピーします。
明示しない場合はExcelの通常の挿入書式になります。値や数式はコピー元から自動採用しません。
他シートの数式、名前定義、テーブル範囲の追従はExcelに任せます。

数式を変更する項目は、型付き表に `string` とJSON文字列 `"=SUM(B2:B4)"` を記述し、
対応を `writeback: formula` にします。式中の座標は **行操作後の最終座標** です。
値をnullにすると数式をクリアします。`writeback: cell` の `"=..."` は従来どおり文字列で、式として評価しません。
新規取り込みでは数式キャッシュと数式原文を別の型付き項目に出します。
数式原文の対応は初期状態ではexcludedであり、変更を明示したものだけformulaに切り替えます。

図形の値は本文の型付き表に置き、対応を `writeback: operation` / `target: null` にします。
対応表には値を重複させず、その項目の `{page, block, field}` を参照します。

```yaml
operations:
  - id: update-process
    kind: update_shape
    sheet: 業務フロー
    name: Process
    reason: 処理名と配置の変更
    properties:
      text: {page: diagram, block: process, field: label}
      left: {page: diagram, block: process, field: x}
      top: {page: diagram, block: process, field: y}
      fill: {page: diagram, block: process, field: color}
```

操作は次のとおりです。

| kind | 内容 |
| --- | --- |
| update_shape | 原本の図形名を指定して文字、位置、寸法、回転、塗り、線、文字サイズを更新 |
| add_shape | 新しい名前とshape_typeを指定。rectangle / rounded_rectangle / ellipse / textbox / line |
| delete_shape | 指定した図形を削除 |
| add_picture | 本文のasset参照が指す文書内assets画像を埋め込む |
| replace_picture | 既存画像を置換。指定しない外形の位置・寸法・回転は維持 |
| add_connector | begin/endに図形名と接続siteを指定。straight / elbow / curve |

propertiesのキーは `text / left / top / width / height / rotation / fill / line / line_weight / font_size`。
位置・寸法・線幅・文字サイズはポイント、回転は0以上360未満の度、色は `#RRGGBB` または `none` です。
新規図形・画像ではleft/top/width/heightが必要です。画像のassetも本文の文字列項目への参照で、
`assets/logo.png` などのパスを指定します。書き戻し前に画像の実在・内容ハッシュを確認します。
図形名は原本にある最上位の図形名を使います。グループ内の子要素の直接編集は、この操作契約の対象外です。

Excel APIの契約は [Formula2](https://learn.microsoft.com/en-us/office/vba/excel/concepts/cells-and-ranges/range-formula-vs-formula2) と
[AddPicture](https://learn.microsoft.com/en-us/office/vba/api/excel.shapes.addpicture) を参照しています。

## テスト

`python -m pytest tests/test_documents.py -q` で、Office実例の取り込み、形式違反、根拠改変、
直接編集のレビュー無効化、再取り込みの競合、旧形式移行、Excelの値・数式・書式と変更対象外パートの保持を検証します。

`tests/test_document_operations.py` は構造変更の検証を追加しています。Windows + Excel環境で
`ARP_TEST_EXCEL=1` を設定すると、実Excelで行の移動・数式・名前定義・テーブル・図形・画像・接続線の保存と再読込まで確認します。
