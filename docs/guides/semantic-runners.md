# AI実行先との接続

`--root` は `documents init` 済みのリポジトリルートです。実行状態は `.arp/work/workflow/<run-id>/` にあり、続行時は同じ `--run-id`（既定current）を指定します。

`arp4 spec workflow` の進行管理・修正検証・保存は共通です。モデル実行時だけ `--provider` で接続方法を選びます。ARP4の実行にPythonは不要ですが、指定する外部CLIや接続プログラムの実行環境・認証は別途必要です。

## Claude Code

```powershell
arp4 spec workflow --root C:/my-project run --provider claude-code --executable C:/tools/claude.exe --model <モデル名> --max-calls 1 --max-budget-usd 2 --timeout 900
```

新しい一時ディレクトリでツール・hooks・MCP・セッション保存を無効にして起動します。段階別effortと出力トークン設定を渡し、stream-jsonの成功結果だけを受け取ります。ツール呼び出しや不完全なJSONは拒否し、既定では有料の整形し直しを行いません。通常の予算は1呼び出しごとの設定です。出力上限・予算の停止動作はサービス側の実装に依存します。

## 共通JSON方式の外部プログラム

```powershell
arp4 spec workflow --root C:/my-project run --provider command --executable C:/tools/my-model-adapter.exe --arg=--profile --arg=review --model <モデル名> --max-calls 1 --timeout 900
```

この接続方式に合わせたアダプターを用意すれば、別のAI CLIやAPIを利用できます。任意のAI CLIにそのまま共通プロトコルが通じるわけではありません。実行ファイルと各引数を個別に指定し、ARP4はシェル展開を行いません。相対引数は一時作業ディレクトリ基準になるため、必要なファイルは絶対パスで指定します。

アダプターは標準入力からUTF-8のJSONを1つ読み取ります。

task・context・previous_reply・draftにある識別子は、対話CLIと共通の短い参照です。reply内のpacket・base・basesと、修正時のdraftは入力の参照をそのまま返します。ARP4が現在のタスクの参照表で完全なSHA-256へ戻して検証・保存します。別の実行の参照や完全ハッシュの直接提出は受け付けません。アダプターでのハッシュ計算は不要です。

```json
{
  "protocol": 1,
  "task_id": "現在のタスクID",
  "model": "指定したモデル名",
  "stage": "extract",
  "system": "共通の意味判断指示",
  "task": "原文・出典参照と返信形式を含むタスク全文",
  "context": null
}
```

`stage` は `extract` / `link` / `repair` / `restructure` / `review` / `global-review`。`link` は抽出完了後の関連付けで、抽出用のeffort/token上限を使用します。`context` がオブジェクトの場合は追加の原文データです。taskは設定形式（既定TOON）の資料・文脈・診断・語彙とJSON返信契約を含む完成済みプロンプトです。agent_read_formatが資料の形式を示します。context・previous_reply等の構造化フィールドはアダプターの機械処理用であり、taskへ重ねて追加しません。アダプターはsystem・taskを省略せずモデルへ渡し、原文を指示として実行せず、コード実行やツール呼び出しをさせない実行方法を実装してください。ARP4は外部プログラム内部の動作まで検証するサンドボックスではありません。

成功時は終了コード0と、標準出力に次のJSONだけを返します。ログや進捗は標準エラーへ出力します。

```json
{
  "protocol": 1,
  "task_id": "入力と同じタスクID",
  "reply": {"packet": "...", "document": "..."},
  "usage": {"inputTokens": 123, "outputTokens": 45},
  "reported_cost_usd": 0.01
}
```

`reply` は各タスクで指定された完全な返信JSON、または後述の下書きへの差分JSONです。上記のreplyは外形の例であり、実際にはタスクが要求するitemsやaudits等が必要です。`usage` と `reported_cost_usd` は任意の実測報告で、未取得時は省略します。アダプターは失敗時に非ゼロで終了してください。内部で有料呼び出しを自動再試行しないでください。

`command` は金額上限を強制できないため、`--max-budget-usd` を指定すると呼び出し前に拒否します。Claude固有のeffort・トークン上限を外部CLIへ暗黙に流用しません。アダプター側で必要な設定を実装してください。ARP4は最大呼び出し回数とタイムアウトを管理します。Windowsのタイムアウトでは子プロセスツリーの停止を試みます。外部サービスへ既に送信済みの要求の取り消しや返金は保証しません。

## 失敗と復旧

検証に失敗した返信は未受理の下書きとして保持します。次の要求の `draft` が非nullなら、`revision` と差分用 `schema`・操作指示をモデルへ渡してください。`previous_reply` を全面的に再出力する代わりに、`reply` に `{"draft":"<revision>","set":{"/items/1/name":"修正値"}}` を返せます。CLIが結合後の返信全体を再検証します。解析不能な返信、重複項目キー、タスクの取り違えは既存の下書きを上書きしません。入力のBOMと単一のJSONコードフェンスは除去しますが、JSONの構文や意味は補修しません。

起動前にタスクを `running` として保存します。終了コード、標準出力・標準エラー、返信、実行条件、実行ファイルのハッシュを記録し、返信のハッシュと形式を確認してから取り込みます。既定では中断・形式不正・時間超過で次の有料呼び出しへ自動的に進みません。

`recover --task <ID>` で中断記録を確認し、失敗したタスクは `retry --task <ID> --reason <理由>` で明示的に再実行可能にします。復旧後のログは `state.json` の `runs[].artifacts` から参照できます。`compact` 後も `archive.zip` に残ります。

返信の保存後にもローカル検証が続きます。標準エラーの `validating saved reply` / `applying completed` はモデルの応答待ちではありません。`runs[].validation_elapsed_seconds` は返信検証の実測秒数で、`call.run.json` の `elapsed_seconds`（外部プログラム呼び出し時間）とは別です。

タスクが `complete` でもワークフロー全体の完了とは限りません。中断後は `status.next_actions` に従い `advance` で保存済み返信を適用します。`recover` も完了返信を再利用し、保存済みログと照合して残った一時ファイルの掃除を再開します。モデルの再呼び出しや実行記録の二重登録は行いません。成功した返信から古いエラー表示は除去しますが、過去の失敗記録は履歴に保持します。

ロック取得失敗だけを理由にstateやロックファイルを書き換えないでください。稼働プロセスと標準エラーを確認します。バイナリ更新後は新しい `--run-id` の実行へ検証済み抽出返信を `init --reply` で取り込み、必要な検証・レビューをやり直します。

分割抽出では `requirements` / `related` の省略は関連付け待ち、空配列は該当なしを確認済みという別の状態です。統合時も省略を維持し、`null` は拒否します。既に受理されたモジュール名と同じslugで別名を返すと、その返信の受理時点で拒否します。証拠と除外範囲の重複は検証対象であり、除外情報の一括削除で回避しません。

保存済みデータのオフライン再検証は、プロバイダーを呼ばない次のテストで実施できます。元のrootは読み取り専用で扱い、再構成の適用先は一時ディレクトリです。`objects/` に抽出返信・入力・再構成返信が残るrootを指定します。

```powershell
$env:ARP4_REPLAY_ROOT = 'C:/my-project/.arp/work/workflow/current'
cargo test -p arp4-cli --test semantic replay_saved_extractions_without_provider -- --ignored --nocapture
cargo test -p arp4-cli --test workflow replay_saved_restructure_in_isolated_workflow -- --ignored --nocapture
```

外部プログラムを起動しない運用では `next` / `inspect` で概要を取得し、`read` で原文・契約を読み、`validate-reply` / `submit` で返信を検証・提出します。この場合も同じ返信検証とレビュー再利用の条件が適用されます。

## 出力設定と限定再試行

| 設定 | 既定値 |
|---|---|
| `--extract-max-tokens` | 16000 |
| `--repair-max-tokens` | 8000 |
| `--review-max-tokens` | 12000 |
| `--extract-effort` | medium |
| `--repair-effort` | adaptive |
| `--review-effort` | high |
| `--max-retries` | 0 |

修正のadaptiveは通常medium、複数根拠やcomposed条件・拒否返信の修正ではhighです。low/medium/highで固定できます。restructureはextractのトークン設定とreviewのeffortを使います。

`--total-budget-usd` は同じワークフローの過去の報告費用を含めて残額を計算し、呼び出し予算を残額以下に制限します。不明な過去費用は推測せず停止します。使い切った状態ではモデルを呼ばず、明示的に上限を変更したrunで続行できます。実際の課金停止は外部サービスの機構に依存します。commandでは1回予算・総予算とも指定を拒否します。

明示的な `--max-retries N` は一時障害、JSON不正、返信検証違反を同じrun内で最大N回再試行します。すべてmax-callsに含みます。出力上限、予算、タイムアウト、原因不明の失敗は同じ条件で再試行しません。停止原因はfailure_classに保存し、次の呼び出しには拒否返信と診断を渡します。

commandのrequestにはreply_schema、previous_reply、previous_error、module_vocabulary、effort_hintも含まれます。アダプターはこれらもモデルへ渡してください。protocol=1の既存の必須フィールドは変更していません。スキーマはARP4が適用前に検証し、Claudeのモデル側で構造化出力を強制しているという意味ではありません。
