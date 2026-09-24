# 意味判断に絞った設計書生成

分類理由・検証方法の省略、抽出後の関連付け、参考・実績表の原表保持については [出力項目の方針](semantic-output-policy.md) を参照してください。

各段階の実行・再開・レビュー再利用・成果物保存をまとめて管理する場合は [進行管理ワークフロー](../guides/semantic-workflow.md) を使います。以下は個別コマンドの説明です。

通常の利用はPython不要の `arp4 spec workflow` を使用します。Rust版ワークフローは局所修正・範囲選択・レビュー再利用を内部で行います。以下の個別操作もRust CLIで実行できます。Pythonは末尾の開発用計測と検証環境作成にのみ使用します。

新規の生成は `spec semantic` を使います。Claude は解釈・分類・要件との対応・矛盾の判断を返し、Rust CLI がモデルJSON、出典ID、引用位置、初回採番用対応表、分類カタログを構築します。Markdown は既存の render が生成します。

## 処理の分担

| 処理 | 担当 |
|---|---|
| 資料取り込み、原文保存、出典の短縮参照、表情報の重複除去 | CLI |
| 主張の分割、条件・数値の意味、5分類、関連・要件参照、検証方法 | Claude |
| 引用の検索、Unicode文字位置、正式な出典ID、モデル構築 | CLI |
| 原文からの独立レビュー、文書間の矛盾・同義・関係の確認 | 独立したClaude実行または人 |
| 初回ID採番、分類の参照変換、正本の検証・閲覧文書の整形 | CLI |

引用・除外・監査・承認をプログラムが推測して補完することはありません。未処理文字や未レビューは従来どおり正式生成をブロックします。意味上の正しさを機械検証だけで保証するものでもありません。

## 1. 資料を固定し、文書ごとのタスクを作る

Word・PPTX・PDF、および表・画像・図表候補のあるExcelは、先に [構造解釈とレビュー](../guides/document-structure.md) を完了し、captureへ `--root` と `--structure` を指定します。以下は構造解釈が不要な入力の例です。

```powershell
arp4 spec capture --extraction extraction-a.json extraction-b.json --out input.json
arp4 spec semantic packet --input input.json --document doc-a --out tasks/extract-a.md
arp4 spec semantic packet --input input.json --document doc-b --out tasks/extract-b.md
```

タスクには契約とその文書の全文を含みます。長い出典ハッシュ、入力全体のJSON、完成形スキーマ、ソースコード、過去のログを追加で渡す必要はありません。表のセル番地・シート・結合範囲・表示形式・値と数式の区別は残ります。図形等の未抽出警告も残します。原文を省略・要約して削減しません。

`build/prepare_spec_validation.py --out <新規フォルダー> --corpus examples/kotonoha/資料` も文書別タスクを生成します。既存の入力があれば再取り込みせず packet だけを作れます。

## 2. タスクの意味判断を実行する

個別コマンドで作成したタスクは、AIまたは人が原文と返信形式を確認し、返信JSONを保存します。抽出と独立レビューは別の作業として行います。自動実行・再開・記録保存をまとめる場合は、最初からワークフローを使用します。

```powershell
arp4 spec workflow --root C:/my-project init --input input.json
arp4 spec workflow --root C:/my-project run --provider claude-code --executable C:/path/to/claude.exe --model <モデル名> --max-calls 1 --max-budget-usd 2 --timeout 900
```

この場合はワークフロー自身がタスクを作成し、返信の検証・修正・レビュー・出力へ進みます。1回のrunでモデルを呼ぶ回数は `--max-calls` で指定します。既定では失敗した有料呼び出しを自動再試行せず、実行条件・使用量・返信・生ログを作業ルートへ保存します。限定再試行は `--max-retries` で明示します。

外部のAIから返信を投入する場合は `inspect --task <ID>` で概要を確認し、`read --task <ID>` の全ページから本文・context・返信契約を取得して、`submit --task <ID> --reply reply.json` で返します。別のAI CLI/APIの自動実行には共通JSON方式の `--provider command` を使用します。接続契約と各実行先の制限は [AI実行先との接続](../guides/semantic-runners.md) を参照してください。

以下のassemble・review-apply・finalizeは個別操作の説明です。ワークフローではこれらを内部で実行するため、同じ処理を手動で重ねる必要はありません。

## 3. 意味判断だけの返信を機械的に展開する

返信の小さな例です。packetはタスク中の値をコピーします。

```json
{
  "packet": "タスクのfingerprint", "document": "doc-a",
  "modules": {"login": "ログイン"}, "open_issues": [], "exclusions": [],
  "items": [{
    "key": "lock", "name": "失敗上限", "section": "process",
    "subject": "ログイン", "property": "失敗上限",
    "condition": {"basis": "unspecified"},
    "value": {"kind": "quantity", "amount": 5, "unit": "回", "comparison": "lte", "semantics": "scalar", "basis": "s1"},
    "statement": "失敗の上限は5回", "verification": "失敗上限の設定が5回であることを照合",
    "evidence": ["s1"],
    "classification": {"category": "specification", "module": "login", "requirements": [], "related": [], "reason": "具体的な境界値。目的の要件は資料に未記載"}
  }]
}
```

`"s1"` はセル全文。部分引用は `{"source":"s1","quote":"上限5回"}` とします。同じ文字列が複数箇所にあるときだけ、1始まりの `occurrence` を指定します。位置の推測、引用の正規化、先頭一致への自動決定は行いません。未知の項目参照、変更された資料への古い返信、重複キー等も拒否します。

文書間レビューで真の重複と判断した場合は、1項目へ両文書の根拠をまとめられます。別文書は `{"source":"doc-b/s1","quote":"正確な引用"}` と指定し、引用を省略した別文書参照は禁止します。重複項目を削除するときはその参照も明示的に更新し、異なる値を持つ矛盾候補を重複扱いにして消してはいけません。

```powershell
arp4 spec semantic assemble --input input.json --reply extract-a.reply.json extract-b.reply.json --actor claude-opus-5-extraction --out assembled
```

`assembled/model.json`、`catalog.json`、`identity-plan.json`、`check.json` ができます。assemble の成功は ready ではありません。まず check.json の診断を確認し、修正が必要な文書だけの返信を直して新しい出力先へ再構築します。数量の未対応表現があれば診断を残し、Agentに検証器を調べさせたりtext型へ逃がしたりしません。原文の解釈を変えず、検証器の対応要否を開発側で判断します。

全入力文書の返信が必要です。欠けた文書や余った文字を除外扱いにしません。初回抽出ではrequirements/relatedを省略でき、ワークフローでは全抽出完了後のlink段階で既知の `document/key` による横断リンクを補完します。既存形式の文書内キーも受け付けます。モジュール名や、曖昧な分類の理由もレビュー対象です。共通のモジュール語彙を使うと文書間の表記揺れを減らせます。

## 4. 文書別の原文レビューと全体の整合性レビュー

```powershell
arp4 spec semantic review-packet --input input.json --model assembled/model.json --catalog assembled/catalog.json --document doc-a --out tasks/review-a.md
arp4 spec semantic review-packet --input input.json --model assembled/model.json --catalog assembled/catalog.json --document doc-b --out tasks/review-b.md
arp4 spec semantic review-packet --input input.json --model assembled/model.json --catalog assembled/catalog.json --out tasks/review-global.md
```

これらのタスクも抽出とは独立したAI実行または人によるレビューに渡します。文書別タスクは原文から網羅性を確認し、document未指定のタスクは全項目の矛盾・重複・横断参照を確認します。全体タスクには全原文・監査記録を再掲載せず、主張とその引用・分類を渡します。

ワークフローでは文書別をreview、全体をglobal-reviewの段階として管理します。入力は共通の項目キーをcolumns/rowsへ、重複する引用をspan_tableへまとめた可逆形式が標準です。`review-packet --expanded` で展開形式も出せます。どちらも内容のfingerprintは同一です。文書別には使用するモジュールと関係文書の未解決事項だけを渡し、直接参照する要件・関連項目と外部根拠は依存情報として残します。カタログの `open_issue_documents` に各未解決事項の対象文書を明示します。全体共通の課題は空配列を指定し、対応の欠落は拒否します。

返信は `packet`, `document`, `audits`, `findings`。auditsには実際に確認した短い出典参照と理由を記します。同じ理由で確認できた複数出典だけをグループにできます。globalではauditsは空です。findingsが残る返信、原文のレビュー漏れ、全体レビューの欠落、変更されたタスクに対する古いレビューは適用できません。

```powershell
arp4 spec semantic review-apply --input input.json --model assembled/model.json --catalog assembled/catalog.json --review review-a.reply.json review-b.reply.json review-global.reply.json --reviewer claude-opus-5-independent --out reviewed.json
arp4 spec semantic finalize --input input.json --model reviewed.json --catalog assembled/catalog.json --plan assembled/identity-plan.json --out finalized
arp4 spec registry preflight --input input.json --model finalized/model.json --catalog finalized/catalog.json --project example
arp4 spec registry init --input input.json --model finalized/model.json --catalog finalized/catalog.json --project example
arp4 spec registry render
```

review-applyは監査を展開し、モデル・分類カタログのハッシュとcoverage matrixを `.semantic-review.json` に保存します。finalizeはこの一致と従来の検証を確認したうえで、初回の永続IDと台帳を発行し、関連参照を変換します。`--registry --project example` を指定すると、同じ検証済みbundleから `.arp/registry/` に初回正本も作成します。先に `documents init` を実行します。この場合、別途 `registry init` は不要です。出力モデル・台帳・カタログはセットで保存します。正本への取り込みは全件提案で、承認は行いません。正本を書き込む前にbundleの既存内容と親ディレクトリを検査しますが、システム障害に対する複数出力の原子的保存ではありません。

## 5. 変更のあった資料だけを再処理する

```powershell
arp4 spec semantic changes --before input-before.json --after input-after.json --out changes.json
```

document IDを維持して再captureします。出力のreextractだけを再抽出し、reusable文書の返信は再利用できます。Officeのセル位置・書式・結合や警告が変わっても再処理対象になります。入力文書全体が変わっても、無関係な文書のrevision変更だけでは抽出packetは変わりません。

テキスト原本の行・バイト範囲は出典位置として保持し、意味判断用packetのハッシュからは除外します。TXTの物理行番号に由来するセル位置も除外します。本文・順序・見出し・列情報が同一なら、空行の挿入等で位置が変わっても抽出返信を再利用し、新しいcaptureから根拠位置とモデル全体のinput_hashを組み直します。原本・captureのハッシュ検証は継続し、registryの根拠・承認を自動更新するものではありません。

文書別レビューは渡した項目・分類・原文・共通情報が同じ場合だけ再利用できます。全体の主張が変わればglobalレビューをやり直します。削除された文書はremovedに出し、古い項目を黙って残しません。

生成途中の `spec workflow update --input <新capture>` は、出典数・順序・構造が対応する本文変更を節・シート単位で再抽出します。Markdownでは変更した節とその配下、必要な祖先節を対象・文脈に分けます。項目の根拠が複数範囲にまたがる場合は対象を広げ、変更のない項目をCLIが統合します。構造変更、共通の前置きの変更、未解決事項、外部文書の根拠変更等では文書単位へ戻します。明示的なregions計画は優先し、自動の部分再抽出を使いません。既存キーの保持・新規キーの判断は抽出担当が行い、消えた項目への関連はlink段階で再検討します。

### 局所修正とレビューの再利用

`spec workflow` は最新の返信をassembleし、機械診断・レビュー指摘から局所修正タスクを作成します。`inspect` は概要を返し、`read` で本文・context・scopeを取得します。入力はreadの子pointerとpage.next_offsetを辿って取得します。修正返信は指定されたbaseハッシュと対象キーに対して検証し、対象外・重複変更・ローカル参照切れを拒否します。無変更返信では未解決のまま停止します。

contextには対象項目と直接の参照先・参照元が使うシートの全文を残します。見出し・単位・離れた脚注・結合情報は削りません。明示的なregion計画があれば、その出典範囲と共通文脈を使います。別文書の根拠や構造変更が必要な問題は、範囲を定める作業としてdeferredに残します。

不足するシートが必要なら、修正の代わりに `{"base":"対象のハッシュ","request_tables":[2],"reason":"別シートの注記が必要"}` を返せます。表番号はcontextのavailable_tablesから選びます。ワークフローは要求を検証して新しいタスクを作り、追加要求を修正として適用しません。

```powershell
arp4 spec workflow --root C:/my-project status
arp4 spec workflow --root C:/my-project inspect --task <ID>
arp4 spec workflow --root C:/my-project submit --task <ID> --reply repair.reply.json
arp4 spec workflow --root C:/my-project advance
```

修正後はレビューpacketの完全一致を確認し、指摘なし・監査完了の保存済み返信だけを完了済みタスクとして再利用します。変更されたシートや依存先に対応するレビュー、全体の主張が変わった場合のglobalレビューはやり直します。未取得の必須レビューは新しいタスクになります。最終的に計画上の必須監査と全体レビューの完了を検証します。

個別操作でもシート単位のレビュータスクを生成できます。

```powershell
arp4 spec semantic review-packet --input input.json --model assembled/model.json --catalog assembled/catalog.json --document doc-a --sheet Sheet1 --out tasks/review-a-sheet1.md
```

シート別返信にはpacketのsheetもコピーします。シート外の監査と重複監査を拒否します。監査漏れは部分結果として保存でき、計画上の必須監査が揃うまで最終受理できません。シート情報が一部でもない文書は、ワークフローでは文書単位のレビューに戻します。詳細は [進行管理ワークフロー](../guides/semantic-workflow.md) を参照してください。

`review-apply` の成功は保存の成功です。応答の `ready` が最終受理の可否を示します。次回の `--model` に保存したモデルを指定すると、内容が一致するpacketの返信だけを再利用します。同じpacketの部分監査は確認者を保持して蓄積し、既存出典への新しい監査はその出典の記録を更新します。保存済み指摘の解消には、そのpacketの監査対象全体を確認した新しい返信が必要です。変更されたpacketの監査と指摘は失効し、再レビューが必要です。未解決指摘を含む返信も保存できます。修正対象の指摘は最終受理を妨げますが、`improvement` は改善候補として保存し、完了を妨げません。現在有効な監査は出典ごとに1件です。

独立レビューのコストを制限する場合、`workflow init --review-plan <JSON>` または `review-apply --review-plan <JSON>` で計画を指定します。計画の `omitted_sources` は元の出典IDから対象外理由へのマップです（形式の正本は [モデル契約](model-contract.md) の `review_plan`）。空のマップは全件監査です。指定しない場合は保存済み計画を引き継ぎ、新規モデルでは全件監査となります。対象外出典も原文の処理対象であり、仕様への統合・理由付き除外を省略できません。workflowでは対象外出典を文脈として残し、監査タスクの対象だけを減らします。全体整合性レビューは必須です。coverageには実監査数、必須監査数、対象外数と出典別理由を表示し、未監査を監査済みとは扱いません。

同じ意味・対象・条件の繰り返しは1項目に統合して各出典を根拠として保持します。条件・例外の差や矛盾は保持します。不要な表現は正確な引用範囲と理由を持つexclusionsとし、見出し・単位・注記は必要な解釈根拠として残します。意味不明な記述はopen_issuesに残し、未処理部分を自動除外しません。統合・除外の判断と、その判断を独立レビューしたかどうかは別に記録します。

抽出packetの警告も文書別に絞ります。別文書に属する警告の変更だけでは無関係な文書を再抽出しません。所属不明の警告は全体共通として残します。

finalizeは**初回専用**です。正本更新の永続IDは [registry apply](../guides/registry.md) または既存の `assign-ids --previous` で明示的に継承・分割・統合・廃止します。変更計画は再抽出範囲を絞る機能で、意味上の同一性や正本変更を自動承認する機能ではありません。

## 計測

`python build/measure_semantic_packets.py --input input.json --binary target/debug/arp4.exe --out <新規フォルダー>` はモデルを呼ばず、原文の保存とUTF-8バイト数の削減を確認します。トークン・費用・品質の削減率はこのバイト比較だけでは分かりません。実行ログのmodel_usage、reported_cost_usd、tool_calls、compactionsと、同じ品質基準のレビュー結果を併せて比較してください。

`--model assembled/model.json --catalog assembled/catalog.json` を追加すると、文書別・全体のレビュー入力を圧縮形式と展開形式で比較できます。既存Kotonohaの495項目・8文書を使ったオフライン計測では、全9タスク合計1,525,113→1,176,703バイト（22.8%減）。1文書の未解決事項だけを変更した場合は7文書のfingerprintが維持され、変更文書と全体の2タスクだけが再レビュー対象になりました。Claude Codeの実呼び出しと品質・実トークンの再測定は未実施です。
