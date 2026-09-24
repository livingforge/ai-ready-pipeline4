# カスタムAgentによる委任

`arp4 skills install --root <project> --agent codex` で、親が使うarp4スキル、操作リファレンス、委任先のカスタムAgentを配置します。Claude Codeは `--agent claude`、GitHub Copilotは `--agent github`、全形式は `--agent all` です。設定を再読込したホストで「arp4スキルを使って既存workflowを続行」と依頼します。モデル・権限・ホストの同時実行設定は変更しません。

| ホスト | 親の入口 | 委任先 |
| --- | --- | --- |
| Codex | `.agents/skills/arp4/SKILL.md` | `.codex/agents/arp4-worker.toml`、`arp4-reviewer.toml` |
| Claude Code | `.claude/skills/arp4/SKILL.md` | `.claude/agents/arp4-worker.md`、`arp4-reviewer.md` |
| GitHub Copilot | `.github/skills/arp4/SKILL.md` | `.github/agents/arp4-worker.agent.md`、`arp4-reviewer.agent.md` |

親をさらにサブAgentにする階層は増やしません。親のスキルから、作成・修正担当と独立レビュー担当を直接呼びます。CLIの `handoff` は `agent` に委任先名を返します。親はホストのカスタムAgent呼出し機能でその名前を選択し、handoffと担当固有のactorを渡します。Agent定義を手作業で毎回プロンプトに貼り付ける必要はありません。

## スキルと担当定義の配置方針

スキルは利用者の目的から操作手順を選ぶ入口、カスタムAgentは委任する仕事の責任境界として分けます。通常の文書操作は親が行い、workflowの意味判断に作成・修正担当と独立レビュー担当を使います。ARP本体のコード開発はこの利用者向けスキルの対象外です。

| 配布原稿 | 維持する内容 |
| --- | --- |
| `surface/skills/arp4/body.md` | 用途別の入口、親と担当の責務 |
| `surface/skills/arp4/references/orchestration.md` | workflow実行時の割当・再利用・独立性・完了判断、必要時の計測 |
| `surface/skills/arp4/references/operations.md` | 原本・設計書・台帳の操作手順とCLI応答の読み方 |
| `surface/skills/arp4-setup/body.md` | 利用環境の導入・更新 |
| `surface/agents/*/body.md` | 各担当の判断基準と担当範囲 |
| `surface/agents/common.md` | handoffからの読取・検証・提出・結果報告 |

担当は親のスキル全文を引き継がず、handoffを受けてreadから原文と返信契約を取得します。共通手順は生成時に各ホストのAgent定義へ組み込むため、担当が別のスキルを発見・読み込みできることには依存しません。契約の一覧はCLIから供給し、Agent原稿では判断基準を維持します。

この分け方は、[OpenAIのスキル設計](https://learn.chatgpt.com/docs/build-skills)にある必要時の詳細読み込みと、[カスタムAgentの設計](https://learn.chatgpt.com/docs/agent-configuration/subagents)にある限定した役割・責務に沿ったものです。小さな手順ごとにスキルや担当を増やす必要はありません。

## タスクと担当数

文書・シート・範囲のレビュータスク、および関連する指摘をまとめた修正タスクはCLIの計画を維持します。親は既定で最大3担当を同時に動かし、ユーザー指定とホストの利用可能枠に合わせて調整します。これはスキルによる配分方針であり、CLIがホストのAgentを起動・予約する実装ではありません。自動runnerのプロセス再利用を追加するものでもありません。

再開可能なホストでは空いた同じ役割の担当へ次のhandoffを渡します。再開できないホストでは、小さい同段階タスクのhandoffを少数ずつ渡し、担当は1件を読取・検証・提出してから次を読みます。各タスクの返信・監査記録は分離したままです。大きいタスクや文脈混同の兆候があれば担当を更新します。実際の割当手順・上限調整・失敗時の停止条件は配布されたarp4スキルを参照します。

作成・修正に参加した担当を、このrunの独立レビューへ転用しません。レビュー担当は指摘を提出し、モデルの修正は作成・修正担当へ戻します。本文と詳細理由は担当側に置き、親は短い報告とreceiptで確認します。handoffは予約操作ではないため、親が重複割当を防ぎます。

## 配布と確認

正本は `surface/skills/` と `surface/agents/`。build.rsがCLIに埋め込み、`cargo run --locked --example sync_docs` がホスト別のファイルを生成します。生成先を直接編集しません。導入先で編集されたスキル・Agent・参照ファイルは、既存の所有記録による保護の対象です。

ホスト側でカスタムAgentと委任ツールが利用可能であることを確認してください。新規配置の反映方法と再開操作はホストに依存します。委任できない場合は独立セッションへ引き継ぎ、レビューを実施済みと扱いません。

対応形式の根拠: [Codex custom agents](https://learn.chatgpt.com/docs/agent-configuration/subagents)、[Claude Code subagents](https://code.claude.com/docs/en/sub-agents)、[GitHub custom agents](https://docs.github.com/en/copilot/reference/custom-agents-configuration)、[VS Code subagents](https://code.visualstudio.com/docs/agents/run/subagents)。

効率の比較では同じ入力・モデル・監査範囲を使い、経過時間、Agent起動数、ホストの総トークン/費用、拒否と修正・再レビューの往復を記録します。CLIへ提出されたusageとホスト全体の消費は区別し、未計測をゼロ扱いしません。

2026-09-23の確認では、Windowsでinstallation/workflowの78テストが成功し、保存済み実行データを必要とする1テストは未実施です。生成物の整合、Agent定義のTOML/YAML構文、スキル検証、配布入力を確認しました。独立した机上検証ではレビュー20件・担当枠2・提出拒否・修正指摘を使い、再開可能/非対応の両方を確認しました。各ホストでの実際のAgent呼出し、実案件の時間・トークン削減率は未検証です。

2026-09-24の構成見直しでは、Windowsのinstallationテスト15件、3ホスト分のスキル6件の検証、ローカル参照リンク36件、Agent定義6件のTOML/YAML構文、sync_docsの整合、互換方針、配布入力を確認しました。独立した机上評価では本体コード開発・取り込みのみ・25件のレビューと担当枠2・再開非対応ホストでの提出拒否・修正上限到達を扱いました。提出拒否の訂正と残件停止の境界を明確にし、再評価で整合を確認しました。実案件での抽出精度や時間・トークン削減率を測定したものではありません。
