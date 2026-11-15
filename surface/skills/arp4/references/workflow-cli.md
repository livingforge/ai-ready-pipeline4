# workflowのCLI操作

親が行うworkflowの開始、自動runner、エラー時の再開、共有語彙、参照資料、usage記録。割当と完了判断は [進行管理](orchestration.md)。

- 初回生成は `spec workflow --root <リポジトリルート> init --input <入力JSON>` で開始する。親はstatusとhandoffで担当へ割り当て、担当がreadから検証・提出まで行う。以後は [進行管理](orchestration.md) に従う。正式生成後の更新はregistryを使う。
- 利用者が自動runnerを選んだ場合は `run --provider claude-code --executable <CLI> --model <モデル>` または共通JSON方式の `--provider command` を使う。同じタスクをカスタムAgentにも割り当てない。失敗した有料呼び出しは既定では自動再試行しない。明示的な `--max-retries` は `--max-calls` 内の限定再試行。実行中はrecover、失敗後は理由付きretryで扱う。詳しくは docs/guides/semantic-workflow.md と docs/guides/semantic-runners.md。
- エラー時は `status.next_actions` に従い、同じrootと `run_id` を使う。未受理返信は `previous_reply` に保持され、`draft.revision` と `draft.schema` に従って同じsubmitへ `{"draft":"<revision>","set":{"/items/1/name":"修正値"}}` を渡せる。既存フィールド・配列要素の削除は `remove`。CLIが結合して全体を再検証するため正しい部分を再出力しない。再拒否時は最新の版を読み直す。下書きは正式な受理ではない。受理済み抽出の差し替えは `update --reply`。initの繰り返しやstate.jsonの直接編集で回避しない。
- 操作にはnext/inspect/statusの短い `task_ref` を使える（8桁以上の一意なID接頭辞。曖昧なら拒否）。内部ID・保存ハッシュは維持する。Agent向けのpacket/base/bases/draft revisionは実行ごとの短い参照で、readの値をそのまま使う。ハッシュ計算や別実行からの転用はしない。Git Bashでは `--pointer instructions` や `--pointer packet/sources/rows` のように先頭の `/` を省くとパス変換を避けられる。返されたpointerも先頭の `/` だけ省略してよい。
- 並列抽出で共通分類が分かる場合は `init --modules <語彙.json>` で先に共有する。抽出の `modules` は新規定義だけを記入できる（なければ `{}`）。itemsが参照する登録済みmoduleの正式名はCLIが補う。明示した名前の競合と未登録IDは拒否する。新しい語彙は他Agentの提出で増える。`module_vocabulary_mismatch` は診断のpath/expected/actualと最新語彙を担当が確認し、意味を判断して返信JSONを編集する。別の意味を持つmoduleへ機械置換しない。
- 実測usageが取得できる場合は `submit --usage-file usage.json` に `{"reported_cost_usd":0.25,"usage":{"input_tokens":100,"output_tokens":20}}` を渡す。未知の値は省略する。費用nullは未計測、cost_complete=falseは計測不足。validate-replyのquality_checkで引用文字カバレッジと数量診断を確認し、valid=trueだけで意味品質を承認しない。
- 提出後の実測値は `record-usage --task <ID> --submission <番号> --usage-file usage.json` で追記する。番号はreceiptまたはhistoryのsubmissionを使う。同一値の再送は加算せず、記録済み値と異なる追記は拒否される。再利用した担当の生涯累計ではなく当該提出までの差分を渡す。token_usage_completeは入出力トークン、cost_completeは金額の記録状況を別々に示す。runが費用を記録せず `--total-budget-usd` が停止した場合は、利用者が請求記録で確認した金額を `record-usage --task <ID> --run <番号> --usage-file cost.json --actor <担当> --reason <確認元>` で申告する。金額を推測して申告しない。完了後もhistoryが最新の計測台帳であり、公開時のprovenance.jsonはその時点のスナップショットとして保持する。
- 修正予算と上限後の判断は [進行管理](orchestration.md) の「完了の確認」に従う。正式exportが拒否された場合は `status.drafts` の `draft/design.draft.md` と `draft/open-issues.json` を草稿として報告する。
- 入力に含めなかった資料は「除外した」と報告し、「存在しない」と書かない。Markdown・テキストの参照資料は `init --reference <ファイル>...`（差し替えは `update --reference`）で渡し、`read --task <ID>` のreferencesで読む。出典・evidenceにはならず、矛盾や用語の判断根拠として message/reason に `ref:<パス>` で引用する。
- 初回抽出は文書単位。指示・schema・原文・文脈を含むタスクのUTF-8サイズが既定96 KiBを超える場合にシート、さらにsource範囲へ分割する。`init --extract-max-bytes` で調整できる。文字数・source数制限は既定無効で、必要な場合だけ明示指定する。
