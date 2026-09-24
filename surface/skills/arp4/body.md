# ARPの文書操作と進行管理

原本文書から要件・仕様を管理するためのスキル。利用者の依頼に応じて以下の手順を選ぶ。ARP本体のコード開発には適用しない。

## 作業に応じて読む資料

| 依頼 | 使用する手順 |
| --- | --- |
| 文書の取り込み・成形・差分確認・書き戻し | [操作リファレンス](references/operations.md) の「文書ワークフロー」「CLI応答の読み方」 |
| 構造解釈・capture・新規の設計書生成 | [操作リファレンス](references/operations.md) の「メタモデルと設計書」。workflowを始める時点で下記の進行管理を読む |
| 既存workflowの再開・抽出・レビュー・修正 | [workflowの進行管理](references/orchestration.md)。同じroot・run-idのstatus --summaryから再開する |
| 採番済みの要件・仕様の更新・承認 | [操作リファレンス](references/operations.md) の「正本の継続保守」 |
| ARP実行ファイルの導入・更新が必要 | [arp4-setup](../arp4-setup/SKILL.md) |

参照資料は必要な節だけを読む。原本操作や台帳保守だけなら、workflowの担当Agentを起動する必要はない。

## workflowの責務

このスキルを使う親AgentがCLIによる準備・割当・提出確認・段階の進行・成果物報告を行う。意味判断はhandoffが指定するカスタムAgentへ委任する。

- `arp4-worker` は作成・修正を担当する。
- `arp4-reviewer` は作成・修正に参加していない独立した担当として監査する。

担当にはCLIのhandoffとインスタンス固有のactorを渡す。親はworkflowのread（instructions・packet・contextを含む全pointerと既定の全節読取）を実行せず、内部ファイル経由でも作業資料を読まない。親の会話履歴や操作リファレンス全体を渡さず、資料の要約も作らない。担当がreadからタスク固有の契約と資料を取得する。委任文はhandoffとactorを基本とし、必要な追加指示も短くする。

完了はCLIのstatusと出力成果物で確認し、未解決事項と草稿はその状態で報告する。提出受理は意味的正解や公開承認ではない。採用・承認・原本への反映は利用者から与えられた権限内で行う。
