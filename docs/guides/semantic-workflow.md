# 設計書生成の進行管理と成果物保存

`arp4 spec workflow` は、capture済み入力から抽出、機械診断、局所修正、独立レビュー、初回採番、正式設計書出力までをRustで管理します。Pythonは不要です。進行管理・成果物保存はAI実行先に依存せず、Claude Codeと共通JSON方式の外部実行プログラムに接続できます。通常の操作では個別の修正スクリプトや中間モデルのコピーを作りません。

読み取り・事前検証は共有ロック、更新は排他ロックを使います。競合時はCLIが最大30秒待機します。期限超過時は稼働プロセスを確認し、待機ループを重ねたりロックファイルを削除したりしません。共通分類が分かる場合は抽出前の `init --modules <語彙.json>` で共有します。新規モジュールの名前衝突は引き続き診断として返し、Agentが意味を判断して修正します。

抽出返信の省略可能項目、抽出後の `link` 段階、参考・実績表の保持は [出力項目の方針](../reference/semantic-output-policy.md) を参照してください。

## Agentが読む資料の形式

`.arp/config.yml` に次を指定します。省略時もTOONです。

```yaml
agent_read_format: toon
```

JSON表示にする場合は `json` に変更します。設定の正式な列挙値・既定値は [文書契約](../reference/document-contracts.md) のproject-configを参照してください。

適用対象は `workflow read` の本文ページ、`semantic packet` / `review-packet` のタスクファイル、自動runnerがモデルへ渡す資料です。Agentはその形式を直接読み、返信と差分はJSONで提出します。保存オブジェクト、ハッシュ、status等の制御応答、汎用runnerの標準入出力はJSONを維持します。`read` のバイト上限は表示形式に対して適用され、形式変更時は先頭から読み直します。

単独の `spec semantic` は `--root <プロジェクト>` で設定元を指定できます。省略時は作業ディレクトリからGit境界まで探索し、設定がなければTOONを使います。workflowは指定したプロジェクトの設定を使います。

## 開始と続行

対話型の親Agentはarp4スキルからカスタムAgentへ委任します。タスクごとの新規起動を避け、役割別の担当を上限付きで再利用します。導入とホスト別の配置は [カスタムAgentによる委任](custom-agents.md) を参照してください。

### 対話型Agentから続行する

操作には `next`・`inspect`・`status` が返す `task_ref`（通常8桁）を使えます。`--task` は8桁以上の一意な接頭辞を受け付け、曖昧なら拒否します。保存・監査用の64桁IDと内部ハッシュは維持します。Agent向けのpacket・base・bases・draft revisionは実行ごとの短い参照です。同じ実行のreadが返した値を使い、ハッシュ計算や別実行からの転用はしません。

抽出の `validate-reply` は `valid`（返信の受理可否）に加えて `quality_check.quantity.passed` と `quality_check.quantity.diagnostics` を返します。受理可能でも数量の指摘があれば正式出力はできません。引用範囲・複数数量の指摘は候補を参照して同じタスクの返信を修正し、対応不足はvalidator_supportとして報告します。機械修正タスクにも診断の原文・候補を引き継ぎます。

モデルをCLIから起動しない場合も同じ状態管理・検証・結合処理を使います。新しい実行ディレクトリには [AGENT_BRIEF.md](AGENT_BRIEF.md) を保存します。

親Agentは割当と提出確認を担当します。`handoff` の情報を担当Agentへ渡し、link・restructureを含め、担当が本文取得・判断・ファイル保存・検証・提出まで行います。親は事前に原文を読んだり、子の返信JSONを書き戻したりしません。子の完了報告は300字以内の提出結果・要判断事項・成果物パスとし、親は `receipt` の記録で確認します。詳しい分担手順は上記のAGENT_BRIEFを参照してください。

```powershell
# 親: statusから異なるtaskを選び、handoffの情報をそのまま担当へ渡す
arp4 spec workflow --root C:/my-project status
arp4 spec workflow --root C:/my-project handoff --task <ID>
# 担当: 割り当てられたtaskだけを読み、自分のactorで提出する
arp4 spec workflow --root C:/my-project read --task <ID> --max-bytes 12000
# 続きは page.next_command.powershell を同じ spec workflow --root ... の後ろに付ける
# page.complete: true になるまで本文を読む
arp4 spec workflow --root C:/my-project validate-reply --task <ID> --reply reply.json
arp4 spec workflow --root C:/my-project submit --task <ID> --reply reply.json --origin interactive-agent --actor <担当> --model <モデル> --reason-file reason.txt
# 親: 通知の本文ではなくCLIに記録された提出を確認する
arp4 spec workflow --root C:/my-project receipt --task <ID>
```

handoffは予約を行いません。親が同じtaskの重複割当を防ぎます。receiptは最終提出の受理可否を返し、未提出ならnullです。引退したtaskも照会できます。受理成功は意味的な正解や公開承認を保証しません。

`next` は読み取り専用で最初のpendingタスクの概要（task_ref・段階・状態・文書・一括読み取りコマンド）を `next` に返し、取得による占有やモデル呼び出しは行いません。`inspect --task <ID>` は同じ概要に監査用の64桁ID、作業ルート、節の一覧と操作ガイドを加えます。本文とschemaを重複表示しません。本文・契約はreadで取得します。workflowの --full は使用できません。

#### 本文とページ送り

`read --task <ID>` は最大48000 UTF-8 JSON bytesのページで本文を返します。対話型Agentは12000 bytesを開始値とし、表示が省略・退避されれば上限を下げてrevisionなし・offset 0から再開します。担当IDが分かればreadから始め、next/inspectを毎回挟みません。表示加工用の一時ファイルやPython、head/tailによる切り出しは使いません。

続きは `page.next_command.bash` / `.powershell` のサブコマンドを、同じ実行ファイルの `spec workflow` と `--root`・`--run-id` の後ろに付けて実行します。offset・revision・limit・max-bytes・選択pointerをCLIが埋め込みます。作業ディレクトリも維持してください。`page.complete: true` の最終ページではnext_commandとnext_offsetがnullになります。これは選択部分の配送完了であり、画面での読了や意味判断の完了を保証しません。個別確認のpointer読取だけで全節を読んだことにはなりません。

担当タスクの全文を読んだら返信を検証・提出して次へ進みます。全タスクの原文を先にダンプせず、共通語彙はmodule_vocabularyを使い、文書間の関連はlink・レビュー工程で判断します。

`read --task <ID>` はinstructions、packet、context、scope、reply_schema、module_vocabulary、references、previous_error、previous_reply、draft、reply_templateをこの順に返します。

`entries` の各要素は `pointer` と実際の `value` を持ちます。大きなobjectや配列の子要素はCLIが自動展開して同じページ列へ含めるため、子pointerの探索は不要です。空配列・空object・nullも値として保持します。長い文字列は `string_offset` と `string_total_bytes`（UTF-8 bytes）付きの連続した断片です。pointerは本文の位置を示すラベルです。

`page.unit` は常にfragmentsで、offsetは文字数や配列添字ではありません。`--limit 1..10000`（既定10000）は必要な場合だけ断片数を制限します。

圧縮表は原文packetのまま、`sources.rows` を `sources.columns` 順で読み、table列を `sources.tables` の添字として扱います。

Git Bashではpointerの先頭の `/` を省略できます。個別確認用の `--pointer <JSON Pointer>` も同じ自動展開とページ送りを使い、選択した単一のobjectやscalarが収まる場合は `value` に全体を返します。読み取り中にタスクや共有語彙を変更した場合は先頭から読み直します。雛形の空配列は未作業であり、検証・レビューの完了を意味しません。

#### 返信の検証と提出

返信に検証エラーがある場合、正しい部分を含む返信全体を未受理の下書きとして保持します。`error.draft` または `read --task <ID> --pointer draft` の `revision` が下書きの版です。`previous_reply` で内容、`previous_error` で診断を確認し、同じ `submit` に変更箇所だけを提出できます。

```json
{
  "draft": "CLIが返したrevision参照",
  "set": {"/items/1/classification/category": "specification"},
  "remove": ["/items/1/unexpected_field"]
}
```

このJSONを `submit --task <ID> --reply correction.json` で提出します。`set` はJSON Pointerで指定した値の置換、またはobjectのメンバー追加、`remove` は既存メンバー・配列要素の削除です。不要な操作は省略します。パスはすべて修正前の下書きが基準で、配列を削除しても他の修正位置はずれません。配列への追加は配列全体を `set` します。親子や同じパスへの重複操作、存在しない親、範囲外の添字、タスク識別情報への操作は拒否します。

CLIが結合後の返信全体を再検証し、合格した場合だけタスクを完了します。別のエラーが残れば結合後の下書きと診断を保存するので、最新の版を読み直して続けます。保持された項目は意味的な正しさを保証されたものではなく、項目間の参照・網羅性も再検証します。正式出力には後続の意味診断と独立レビューが必要です。

古い版・別タスクの版による修正、解析不能なJSON、重複する項目キー、タスク識別情報の不一致は下書きを変更しません。完全な修正版の再提出も可能です。`validate-reply` も同じ差分形式を受け付けますが、下書き・履歴を保存せず、版も更新しません。未受理の下書き修正と、受理済み抽出への `update --reply` は別の操作です。

返信のUTF-8 BOM、外側の空白、返信全体を囲む単一のJSONコードフェンスは入力経路によらず除去します。task IDの英字大小も吸収します。JSONの重複キー、末尾カンマ、前後の説明文は拒否し、引用文・数量・条件・型は自動修正しません。

返信は `validate-reply --task <ID> --json '<JSON>'` / `submit --task <ID> --json '<JSON>' --origin interactive-agent --actor <担当>` で直接渡せます。CLIが把握している `packet`・`document`（reviewの `sheet`/`scope`、repairの `base`、restructureの `bases`）は省略でき、タスクから補って検証・保存します。指定した場合は一致を検証します。`items` や `links` などの作業項目は補いません。長い返信は既存のUTF-8 stdin `--reply -`、または返信ファイルを使います。JSONの表示加工・結合・修正適用スクリプトは不要です。シェルの引用規則に従って渡してください。

サブエージェントには同じCLI・作業設定と担当task_idを渡します。担当者はreadから始めて返信を提出できます。親Agentによる原文コピーや返信結合は不要です。`next` は占有しないので、親がstatusから別々のIDを割り当てます。独立レビューは抽出とは別担当が実施します。`next: null` の場合はstatus/next_actionsで完了・停止・適用待ちを区別します。failedタスクもreadで確認して修正返信を提出します。

対話用の本文取得はreadを使い、statusで担当と進捗を確認します。プロンプト・バイナリ変更により既存workflowの継続制約とpacket照合が働くため、進行中の実行ファイルを置き換えず、新しいrun-idで検証します。

`validate-reply` と `submit` の `--reply -` はUTF-8の標準入力からJSONを読みます。返信JSONを直接ファイルへ保存する方式も使えます。事前検証はsubmitと同じ検証器を使い、中間候補はメモリだけに置くため、採用状態・オブジェクト・履歴を変更しません。成功は受理可能という意味で、未解決の意味診断や独立レビューがなくなることを意味しません。提出時には最新の状態に対して再検証します。

結合・キーの名前空間化・関連付けの検証適用・局所修正・再構成の適用はRustの共通処理が担当します。Agentは意味判断JSONを作り、selfcheck/merge/repair適用スクリプトは作りません。関連の意味判断や独立レビュー自体はAgentまたは人が担当します。

`--origin interactive-agent` は対話型Agent経由の自己申告を保存します。既定のexternal、有料runnerのproviderとは区別し、historyのinteractive_agent_submissionsに受理件数を集計します。モデルの実行事実や独立性を証明する機構ではありません。

### リポジトリ内の作業先と成果物

最初にリポジトリで `arp4 documents init` を実行します。共有設定は `.arp/config.yml` だけです。
`--root` はリポジトリルートを指定します。省略時は現在位置から設定を探し、Gitルートを越えません。

```powershell
arp4 documents init
arp4 spec workflow init --input input.json
arp4 spec workflow status
arp4 spec workflow run --provider claude-code --executable claude --model <モデル名>
# 別のローカル実行。続行するときも同じIDを指定する
arp4 spec workflow --run-id experiment init --input input.json
arp4 spec workflow --run-id experiment status
```

作業状態は `.arp/work/workflow/current/`（`--run-id` の既定値は `current`）、完成済み出力は `.arp/cache/output/` に保存します。
実行IDは英数字・ハイフン・アンダースコアを使います。実行履歴は途中再開や診断用のローカルデータで、文書の版履歴はGitが担当します。
`work/` と `cache/` はGit対象外です。共有する要件・仕様・判断は `spec registry init` で `.arp/registry/` に移し、原本とともにコミットしてください。

正常完了した場合のみ固定出力を更新し、手編集や管理対象外ファイルがあれば上書きを拒否します。
`export` は保存された成果物ハッシュを検証して再公開します。`status` や `inspect` は固定出力を変更しません。
補助ファイルは各実行ディレクトリの `debug/` に置きます。入力・返信などCLI引数の相対パスは起動位置が基準です。

`init` と `advance` はモデルを呼ばず、次の作業までローカル処理だけを進めます。`run` は指定回数までモデルを呼び、返信の検証・修正適用・再構築・必要なレビューの選択を行います。途中で回数上限に達したら同じ `run` で続行できます。`--max-budget-usd` は**１呼び出しごと**のrunnerの停止設定で、実行全体の予算ではありません。`--max-calls` の既定値は1、修正回数の上限は実行全体の `--max-rounds`（既定値は生成CLIリファレンス参照）です。`status.round` はrepair/restructureの適用回数、`status.rounds_remaining` は実行全体の残り回数です。文書別の `repair_rounds` は計測値で、独立した予算ではありません。上限後も必要な修正確認を行い、内容の誤りなどが残れば `needs_decision` として草稿と残件を出します。`advance` の再実行では再開せず、情報を追加した `update` で再開します。品質・推論トークンの総量を保証する設定ではありません。

Markdown・テキストの参照資料（ADR、運用手順など）は `init --reference docs/adr/0008.md ...` で渡し、`update --reference` で差し替えます。参照資料は抽出の出典にはならず、レビュー・修正タスクが `read --pointer references` で読み、矛盾や用語の判断根拠として `ref:<パス>` で引用します。レビュータスクの `context.references` にも本文が入ります。

Markdown・テキスト・CSV・TSV自体から要件・仕様を抽出する場合は、`documents import` で正式な原本として取り込み、そのextraction.jsonを `spec capture` に渡します。packetの各行の `position` には種類・見出し・原本の行/バイト範囲を保持します。再取込候補のdiffで出典の対応と再レビュー対象を確認できます。[文書操作](documents.md)を参照してください。

既存の抽出返信がある場合は `init --reply a.reply.json b.reply.json` で取り込めます。現在のCLIが作るpacketと一致する返信だけが使えます。入力データや原本はコピーの変更ではなく、内容ハッシュで固定します。CLIバイナリの変更も検出し、別バージョンの検証結果を混ぜません。

モデル以外の実行環境で処理する場合も同じ状態管理が使えます。

```powershell
arp4 spec workflow --root C:/my-project inspect --task <statusに出たID>
arp4 spec workflow --root C:/my-project submit --task <ID> --reply reply.json
arp4 spec workflow --root C:/my-project advance
```

`inspect` は要求されたタスクの概要を返し、`read` で必要な入力を取得します。`submit` はpacket・対象・監査範囲等を検証し、次の処理へ進みます。既存のレビュー全体の再利用は、モデルや出典を含むpacketの完全一致で判断します。修正確認では変更のない出典の監査だけを明示的に引き継ぐ場合があります。各packetには機械的な `review_risk` と `evidence_clusters` が含まれますが、これは優先順位付けのヒントであり承認ではありません。キャッシュされたレビューでも、最後はRust CLIで計画上の必須監査と全体レビューの完了を検証します。既定は全出典の監査です。`init --review-plan <JSON>` で元の出典IDごとの対象外理由を指定すると、原文の処理範囲を保ったまま独立監査の対象を減らせます。計画はstatusでも確認できます。形式と部分レビューの保存については [意味抽出](../reference/semantic-generation.md) を参照してください。

`--root` は `workflow` の直後、サブコマンドの前に指定します。起動した `arp4` 自身が検証・採番・描画も担当します。担当AgentにはタスクIDと作業設定を渡し、readで原文・context・scope・契約を取得します。`submit` に渡すファイルは抽出・修正・レビューの返信JSONそのものです。`status: blocked` は終了コード2を返します。

別のAI CLI/APIに接続する場合は [AI実行先との接続](semantic-runners.md) の `command` プロトコルを実装します。`command` は金額上限を強制できないため `--max-budget-usd` を指定すると起動前に拒否します。

## 修正と診断の振り分け

同一文書内の複数項目を直すレビュー指摘は、対象項目集合を分割せず一括repairにします。対象が重なる指摘は同じタスクにまとめます。`allowed_keys` の外は変更できません。`external_context_required` は現在の外部引用を `current_external_sources`、外部項目を `required_external_items`、ローカル経由も含む依存辺を `dependency_edges` に示します。外部資料を新しく供給したという意味ではなく、リンク自体の妥当性を確認する手掛かりです。

同時点でrestructureが必要な指摘もある場合は、実行可能なfield repairを同じrestructureへ統合します。構造変更と局所修正を一括検証・適用し、修正予算は1回だけ消費します。manual_triageや既に保留された指摘は修正対象へ混ぜず、判断待ちとして維持します。修正後に新しく判明した欠陥は次の確認で扱います。

restructureは `{"defer":{"kind":"information_required","reason":"具体的な不足情報"}}` で変更せず保留できます。kindはinformation_required / validator_support / unresolvedです。basesは他の返信同様、省略時にCLIが補います。保留にedits/replacements/mappingを混ぜることはできません。未解決記録を保存し、同じ対象文書版では再発行しません。対象文書が変更されると保留を再評価します。正式出力への承認ではありません。

差分editsにはopen_issuesの完全なリストも指定でき、items全体の再送は不要です。repairとrestructureのchangesは `/condition/reason` と `/classification/reason` に対応し、既存の親オブジェクトへreasonだけを追加・訂正できます。

`validate-reply` の `quality_check` は抽出・repairの引用文字カバレッジと数量診断を返します。repairはbefore/afterを比較でき、原文範囲の縮小を確認できます。カバレッジは意味の網羅性ではなく、validも品質承認ではありません。`inspect` は提出履歴のある過去タスクをretiredとして表示します。statusのreferencesは件数と取得コマンドのみで、escalationはreasonを持つオブジェクトです。

対話型提出の実測usageは `submit --usage-file usage.json` で記録できます。JSON例は `{"reported_cost_usd":0.25,"usage":{"input_tokens":100,"output_tokens":20}}`。usageにはcached_input_tokens、reasoning_tokensも指定できます。未計測フィールドは省略します。費用はrunnerと対話型・外部提出の判明分を合計し、未計測はnull、部分計測はcost_complete=falseとunmeasured_callsで示します。historyに提出ごとのusageを保持します。CLI外の親Agent・未提出の準備作業は集計に含まれず、実行全体の費用上限を強制する機構ではありません。

提出後に判明した実測値は `record-usage --task <ID> --submission <番号> --usage-file usage.json` で追記できます。タスク内の提出番号はreceiptとhistoryに表示され、拒否された提出も別番号です。完了・retired後も追記可能です。同一値の再送は加算せず、異なる既存値への上書きは拒否します。担当再利用時は累計をそのまま複数提出へ付けず、提出間の差分を記録します。`token_usage_complete` と `unmeasured_token_calls` は入出力トークンの記録状況で、金額の完全性とは独立しています。完了時のprovenance.jsonは公開時点のスナップショットであり、事後計測はhistoryで確認します。形式は[計測・残件の契約](../reference/workflow-reports.md)を参照してください。

- フィールド修正可能な機械診断と構造化レビュー指摘だけを修正タスクに送ります。
- 同じ原文を使う対象はまとめ、対象項目と直接の関連項目・原文の範囲だけを渡します。
- 追加の文脈要求は必要なシートを加えたタスクへ変換します。修正と誤認して適用しません。
- 同じ文書に複数の修正タスクがあっても、元のbaseに対してすべて検証してからまとめて適用します。
- 項目追加・削除・分割・統合は専用の `restructure` タスクで処理します。未対応の数量表現、原本の判断、対象の不明な指摘は `deferred` に残し、解決したとはみなしません。これらは停止せず独立レビューへ回し、最後まで残れば `needs_decision` の未解決事項になります。
- 原本由来の警告は `notices` に残し、通常のフィールド修正に混ぜません。

レビュー契約v1の指摘は次の形式です。自由文を検索して対象IDを推測する処理はありません。

```json
{"message":"原文を超えた条件が付いている", "items":["doc-a/limit"], "action":"field_repair"}
```

既存の文字列形式の指摘は受け取れますが、対象を自動推測せずmanual_triageとして扱います。指摘は候補として原文と再照合させます。無変更の修正返信は解決を意味せず、無限に同じ修正を呼びません。

## 修正で解消できない指摘

次の場合、workflowは停止せず、残った機械診断とレビュー指摘を独立レビューへ回します（`escalation.reason`）。

| reason | 条件 |
|---|---|
| `repair_no_progress` | 修正返信が抽出を変更しなかった（changesが空など） |
| `stalled_diagnostics` | 同じ診断が3回の修正後も変わらない |
| `unrepairable` | 自動修正・再構築の対象がない指摘だけが残った（manual_triage、ラウンド上限到達など） |
| `round_limit` | 再構築が上限に達した文書を変更しようとした |

修正返信が一部の文書だけを変更した場合も、変更なし（`changes: []`）で返された修正タスクの指摘・診断は `--reason` の理由とともに `unresolved` に記録します。記録された指摘は対象項目が変わるまでrepairに再発行せず、`deferred` に `declined: true` で残します。項目が変わると記録は無効になり、通常の振り分けに戻ります。`update --reply` で抽出を訂正すると記録は消えます。これらの内部記録とレビューのmanual_triage等は、status・レビューcontext・draft/open-issues.jsonの `concerns` に集約して表示します。`open` は未解決、`needs_decision` は人判断待ち、`improvement` は任意改善です。同じ残件のIDと理由を引き継ぎ、unresolvedファイルにない指摘も消しません。

複数文書にまたがる `field_repair` 指摘（表紙・改訂履歴のmodule統一など）は、文書単位のrepairでは宣言されていないmoduleを使えないため、`cross_document_finding` として `restructure` タスクへ回します。

独立レビューの結果はpacketごとに保存します。packetが変わらず、そのレビューの指摘がすべて既知（`findings` または `unresolved` に記録済み）の文書は再レビューしません。同じaction・itemsの指摘を別のレビュー担当が再報告した場合は既存の記録に畳み込み、escalationを無効にしません。

レビュータスクの `context.escalation.concerns` に、その文書に関係する既知の残件と理由が入ります。文書を限定できない残件は全担当に供給します。抽出が誤りなら修正内容を指摘として返し、通常の修正に戻ります。抽出が原文に忠実で診断が検証器の限界なら、その診断への指摘は返しません。

レビュー後も未解決の事項が残る場合、同じモデルを再レビューせず `status: needs_decision` で終了します。作業ルートの `draft/` に `design.draft.md`（草稿）、`design.draft.report.json`、`open-issues.json`（未解決の診断・指摘・レビュー済みか）を出力します。`needs_decision` は `ok: true` ですが正式な export・publish はできません。人が判断して `update --reply` で抽出を訂正すると、escalationは無効になり通常の修正・レビューから再開します。抽出やレビュー指摘が変わった場合も同様です。

## 大きなシートをさらに分割する

既定のレビュー単位はadaptiveです。小さい文書は文書単位にまとめ、大きい文書はシート単位に分割します。`init --review-granularity document` または `sheet` で固定できます。シートより細かい範囲を使う場合は、原文構造を確認したregion計画を `init --regions regions.json` で指定します。見出し・単位・注記を共通文脈として明示できるため、表の境界を機械的に推測して例外を落とすことを避けられます。

以下は出典がs1〜s3だけの文書の例です。s1を共通見出しとして各範囲に渡します。s1自体も別の範囲で監査します。

```json
{
  "doc-a": {
    "packet": "現在の抽出packetのfingerprint",
    "regions": [
      {"sources":["s1"], "context":[]},
      {"sources":["s2"], "context":["s1"]},
      {"sources":["s3"], "context":["s1"]}
    ]
  }
}
```

region計画のsourcesは文書の全出典をちょうど１回ずつ覆う必要があります。未知の参照、重複、欠落、古いpacketは拒否します。contextには同じ文書の出典を指定します。共通文脈の選択が意味的に十分かどうかは独立レビューで確認します。レビュー計画で対象外を指定した場合もregion計画は全文を保持し、レビュータスクの作成時に対象外出典をsourcesからcontextへ移します。対象がなくなるregionには独立監査タスクを作りません。

`spec check`、`assemble`、`review-apply` の `coverage` には、出典ごとの `matrix` が含まれます。各行は原文文字数、引用で覆われた文字数、未覆い範囲、意味監査の有無を示します。網羅性の集計は機械判定に任せ、意味上の正しさはレビュー結果として別に扱います。

この区切りは修正とレビューの両方で使います。修正は対象のregionと共通文脈だけを受け取り、別範囲が必要なら追加要求を返せます。レビューではsourcesだけを監査し、contextを監査対象に数えません。共通文脈・項目・依存先が変わると該当レビューが失効します。regionを宣言していない文書はシート単位、シート情報がない文書は全文に戻します。

CLI単体でも `review-packet --document doc-a --source s2 --context-source s1` を使えます。返信にはpacketの `scope` をコピーします。初回の全体の矛盾・同義語レビューは全主張を対象とし、修正確認では変更と影響範囲を確認します。

## 実行の失敗・外部修正・再開

既定では失敗した有料呼び出しを自動再試行しません。`run --max-retries N` を指定した場合だけ、一時障害・JSON不正・返信検証違反を `--max-calls` の範囲内で再試行します。プロセス中断時には実行中の状態を残します。

```powershell
arp4 spec workflow --root C:/my-project recover --task <ID>
arp4 spec workflow --root C:/my-project retry --task <ID> --reason "一時的な接続障害を解消"
arp4 spec workflow --root C:/my-project run --provider claude-code --executable C:/path/to/claude.exe --model <利用するモデル>
```

`recover` は残されたrunner記録と返信のハッシュを照合し、完了済みなら取り込みます。未完了ならログを保存してfailedにします。`retry` は失敗タスクだけを明示的に再実行可能に戻し、この操作自体はモデルを呼びません。

生成途中の入力や、別途判断して修正した抽出返信は `update --input input-updated.json --reply corrected.reply.json` で更新できます。同じpacketの抽出返信・レビューを再利用し、変更文書だけ再抽出します。必要なら `--regions` も更新します。変更のないupdateで失敗タスクを暗黙に再試行することはできません。正式生成済みの正本更新は既存のregistry手順を使い、初回採番を繰り返しません。

位置だけの変更では、最新の原本位置を保ったまま意味判断を再利用します。出典構造が対応する本文変更では、変更したMarkdownの節またはOfficeのシートを再抽出し、影響のない項目を保持します。Markdownのレビューも、document指定や明示的なregions計画がなければ節別に行い、祖先節の全文を共通文脈として含めます。祖先節・依存項目が変わったレビューは再取得します。構造変更や曖昧な対応は広い範囲で処理し、全体の主張が変わった場合のglobalレビューは継続します。

部分再抽出の `read --pointer context` はprevious_itemsとreserved_keysを返します。古い項目は比較用であり、現在の原文から対象範囲全体を確認してください。同一の主張だけキーを保持し、保持対象のキーを上書きしません。CLIが原文引用・対象範囲・統合結果を検証してから採用します。更新でレビューの一群を中断した場合も、完了済みで指摘のない返信は同じ意味判断用packetに限って再利用します。

## 成果物の管理

`.arp/work/workflow/<run-id>/` の内部は次の構成です。Git対象外の実行状態であり、文書の版履歴ではありません。

| 場所 | 内容 |
|---|---|
| state.json | 現在の段階、採用返信、レビュー、未解決事項、呼び出し記録への参照 |
| objects/ | 内容ハッシュごとに１つだけ保存する原文・タスク・モデル・返信・ログ |
| archive.zip | 過去の状態・旧成果物・ログの圧縮保存。ハッシュで読み取り可能 |
| work/ | モデル実行中の一時ファイル。記録の保存・照合後に重複コピーを除去 |
| latest/ | 正式生成成功時の設計書・モデル・台帳・分類とmanifest |

毎ラウンドの草稿やバイナリコピーは生成しません。同じ内容の成果物は同じオブジェクトを参照します。成功時にはlatestへ出力し、不要になった展開済みオブジェクトを自動で圧縮保存します。途中でも次を実行できます。

```powershell
arp4 spec workflow --root C:/my-project compact
```

アーカイブ内容を検証してから、同じ内容の展開済みファイルだけを除去します。監査ログ・失敗記録・過去の状態は失いません。必要な過去のレビューはarchiveからそのまま読み出し、余分な展開コピーを作りません。容量が無限に一定になる仕組みではなく、固有の履歴は圧縮して保持します。

既存の `C:/arp4-sales-validation` はこの管理形式ではありません。自動削除・書き換えの対象にはせず、新しいrun-idで運用します。古いファイル名だけで採用状態や削除可否を推測しません。

## 実行状態と配布

`state.json` は `version: 1` と `engine: arp4-rust` を使用します。入力と抽出返信は `init --input ... --reply ...` で取り込めます。packetが一致しない返信は拒否します。

標準利用は `arp4 spec workflow` です。進行管理・修正・保存・Claude Code接続は実行ファイルに含まれ、Pythonを必要としません。`build/` に残るPythonは開発用の検証環境作成・計測・誤り注入試験だけです。標準配布にはこの手順とrunner契約も含めます。利用者の `latest/` や実行履歴は案件ごとの作業ルートに保存し、製品ZIPに混ぜません。

`cargo test --workspace --locked` で実Rust CLIと固定返信を使った進行管理、修正・レビュー再利用、復旧、保存を検証します。Windowsでは汎用接続もローカルのテスト用プログラムで検証します。有料モデルの接続確認・実測トークン削減率・意味品質は別途同条件で比較する必要があります。

## Agentの返信契約と適用前検証

`read` の `/reply_schema`、`/module_vocabulary`、`/previous_error`、`/previous_reply` で契約・共通語彙・拒否診断を取得します。条件・値の定義はモデルスキーマから生成します。修正担当が初回抽出のセッションを引き継ぐ必要はありません。

`submit` とrunnerは、スキーマ、原文引用、必須項目、参照先を検証してから受理します。修正はbaseの一時コピーで検査し、無効な返信で採用データを置き換えません。独立して検査できるスキーマ違反・項目ごとの引用違反をまとめて返します。JSONの解析や全体整合性の検査は、最初の検出で停止する場合があります。

拒否されたsubmit返信と診断は履歴に残り、同じtaskへ修正返信をsubmitできます。拒否応答は `error.code: reply_validation_failed` と構造化した `error.diagnostics` で、応答の上限に収まらない分は末尾から丸ごと省いて `diagnostics_omitted` に件数を返します。保存された完全な診断は `read --pointer previous_error/diagnostics` でページ単位に読めます。`update` の検証失敗では採用状態を保存しません。`status.next_actions` と `run_id` が同じルート（`--root` と `--run-id`）での再開方法を示します。フォルダの複製やstate.jsonの手編集は必要ありません。

`status` の `tasks` は各タスクのtask_ref・段階・状態・文書と、拒否時の構造化診断だけを返し、`--limit/--offset` でページ送りします。内部ID・packet・返信のハッシュはstate.jsonと履歴にあり、応答には含めません。`next_actions` は同じ操作のタスクを1件にまとめ、`tasks` にtask_ref（先頭20件）と `count` を返します。空の `blocked`・`deferred`・`exports` などは省略します。原本由来の `notices` は同じ警告を文書一覧付きで1件にまとめます。

除外範囲と項目の根拠、または除外範囲同士が重なる返信は、個別の `validate-reply` / `submit` で `excluded_span_overlap` として拒否します。診断には文書、競合する項目・JSONパス、文字範囲を含みます。`"s26"` はそのソース全文を指すため、同じセルの記号だけを除外する場合は、根拠を記号と重ならない正確な引用に絞ってください。

受理済みの抽出返信を修正する場合は、`update --reply corrected.reply.json` で修正します。未完了タスクが残っている間、`advance` は受理済み返信だけを部分適用しません。最後の返信を検証するときは他文書も含めて検査するため、診断の文書を確認してください。

既存workflowへの更新は、初期化時と同じバイナリで行います。実行ファイルを差し替えると `binary_hash` の検査で更新操作が拒否されます。修正版バイナリで再検証する場合は、既存workflowを保持し、新しいworkflowへ抽出返信をインポートします。

修正のsetは完全置換に加え、スキーマに列挙した既存の末端フィールドの編集に対応します。親と末端の同時編集は拒否します。型の変更・未存在フィールドの追加は完全置換し、完成した値を検証します。

```json
{"base":"現在のbaseハッシュ","changes":[{"key":"limit","set":{"/value/unit_basis":{"source":"s70","quote":"件"}}}]}
```

## 初回抽出の分割と共通語彙

初回抽出は文書全体が基本です。`init --extract-max-bytes 98304`（96 KiB）が既定で、指示・返信schema・語彙・packet（原文、表情報、文脈）を含むタスク本文のUTF-8サイズを測ります。CLI応答のJSONエスケープや重複表示、runnerが追加する再試行情報を含む通信全体のサイズ・トークン数ではありません。上限を超える文書だけシート単位、さらにsource範囲へ分割します。`--extract-max-chars` と `--extract-max-sources` は既定0（無効）で、明示した場合は追加の上限になります。単一sourceと必要文脈が上限を超える場合は原文を切らず診断で停止します。`--regions` は明示的な分割計画として優先され、サイズ制限で再分割しません。既存rootには保存済みの分割設定を適用し、新しい既定値へ自動変更しません。

自動分割では同じシートの先頭8sourceと直前8sourceを文脈候補として添付します。見出しと断定する処理ではありません。`scope.sources` は処理対象、`scope.context` は参照専用です。出典番号を維持し、範囲外の引用と文脈だけを対象にした項目・除外を拒否します。文脈不足なら `packet`・`document`・`request_tables`・`reason` だけを返して追加の表を要求できます。提供済みの文脈だけを再要求するループは拒否します。

完了済みの分割返信を保存・再利用し、そろった時点でローカルキーと参照を区画別に名前空間化して統合します。重複や区画をまたぐ意味関係は後続レビューで確認します。

`init --modules modules.json` でslugから表示名へのJSONオブジェクトを指定できます。同じslugの別名は拒否します。新規slugは返信に残し、受理済み返信の語彙も後続呼び出しへ渡します。表示名を黙って最初の名前に置き換えることはしません。

## 複数セルの条件と構造変更

複数セルの条件はcomposedとして保持します。evidenceは二つ以上の異なる原文断片で、項目の根拠にも含めます。operatorはallまたはany、textは組合せの解釈、reasonは行・列と適用範囲の説明です。各引用は機械検査し、組合せの意味は独立レビューで確認します。単一引用と一致しないためにunspecifiedへ落としません。

```json
{"basis":"composed","text":"機密区分高の資料を社外へ持ち出す場合","operator":"all","evidence":[{"source":"s1","quote":"機密区分高"},{"source":"s2","quote":"社外持ち出し"}],"reason":"行見出しと動作列がこの制約に適用される"}
```

抽出漏れ、重複、分割・統合はrestructureタスクへ送ります。対象と関係する文書を渡し、bases、差分edits、旧新キー対応mappingを返させます。editsはdocument/add/remove/changesを持ち、変更しない項目の再出力を省けます。完全なreplacementsも使用できますが、editsとの同時指定は拒否します。追加は空のfrom、削除は空のto、分割・統合は複数キーを明示します。全参照・引用・既存文字範囲の網羅性を検査して一括適用し、影響するレビューを再取得します。未処理の別のレビュー指摘は残します。

同じ診断が修正後も3回変わらない場合は `stalled_diagnostics` として修正を打ち切り、独立レビューへ回します。文言だけの変更を進展とは扱いません。未対応表現は対応不足として保持し、textやscalarへの変換で回避しません。

## 補修履歴と評価

`submit --actor <実行者> --model <モデル名> --reason-file reason.txt` で外部補修の担当と理由を残せます。担当の省略時はexternalです。submit/update/importとrunnerはoriginで区別し、actor名を変えても自動実行にはなりません。

`submit` と `retry` の理由は、短文なら `--reason <理由>`、引用符・JSON断片・改行を含む場合は `--reason-file <PATH>` を使います。両者は同時指定できません。ファイルはUTF-8で読み、先頭BOMだけを除去し、改行や前後の空白は保持します。空白だけの本文、不正なUTF-8、読み込み失敗は状態や提出履歴を変更せずエラーにします。理由ファイルは標準入力に対応しません（`-` はファイル名です）。submitで両方省略した場合の理由は `Externally supplied reply`、retryはいずれか必須です。

`history` で全履歴、`status.provenance` で呼び出し数・失敗数・外部介入数・報告費用の集計を確認できます（語彙、初回受理率などは `history`）。`history` の `submissions`（提出の担当・理由・受理可否・拒否診断）と `runs`（provider呼び出しの受理可否・使用量・費用）はtask_refで示し、`--limit/--offset` を両方の一覧に同じ範囲で適用します（`page.total` は長い方）。オブジェクトのハッシュとstate.jsonの完全な記録はprovenance.jsonと履歴に残ります。正式成果物にはprovenance.jsonを添付します。呼び出し数、報告費用、初回返信受理率、修正回数、外部介入の段階・文書、原本警告を確認できます。初回受理率は構造・引用検査の指標で、意味品質の正解率ではありません。

保存済み状態はstatus/inspect/historyで確認できます。生成の継続はバイナリ一致を要求します。実行ファイルを変更した場合は、検証済み返信を新しい実行へimportします。

### レビューの終了と改善候補

指摘ゼロを目標にせず、今回の利用に必要な正しさを確認します。内容の誤り・重要な未抽出・条件欠落・矛盾・参照切れ・二重計上は修正対象です。意味を保った細分化や任意の関連追加は `improvement` として記録し、修正タスクを作らず、完了を妨げません。指摘本文には具体的な後工程への影響を、改善候補には今回保留する理由と再検討条件を記載します。完了出力の `improvements.json` に候補を残します。

初回は指定された全出典と全体整合をレビューします。変更後はCLIが前回結果と変更項目から修正確認を計画します。`context.confirmation.required_sources` だけを新たに監査し、変更項目・関連項目・既存指摘を確認します。変更のない出典の監査はCLIが前回の根拠付きで引き継ぎます。担当向けpacketから影響外の項目と出典を省き、必要な出典を共有する項目・関係先・明示的な文脈・項目に紐付かない見出し等は残します。引用spanの参照は削減後の表に振り直します。共有文脈の変更や対象項目を限定できない指摘では全packetを渡します。検証用の全packetと識別子はCLI内に保持し、runnerと対話型readには同じ縮小表示を使います。削減率は入力と影響範囲によります。

粒度は、独立した変更・検証・参照が必要か、条件や意味が失われるかで決めます。文や表の行の数に合わせた一律分割は求めません。ただし契約で必要な数量型・根拠は省略できません。同じレビュー担当の継続を推奨しますが、完了はCLIの状態で判定します。

statement中の数値は構造化済み数量の再掲や例外説明として保持できます。観測・見込み・参考の原表は許可されたtable表現を使えます。relatedの逆向き辺を一律には要求しません。これらの外形だけをvalidate-replyの拒否条件にせず、意味の欠落や独立した制約の未抽出を原文に照らして判断します。
