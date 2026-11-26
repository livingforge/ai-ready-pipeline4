# ARPの文書操作と進行管理

原本文書から要件・仕様を管理するためのスキル。利用者の依頼に応じて以下の手順を選ぶ。ARP本体のコード開発には適用しない。

## 作業に応じて読む資料

依頼に該当する行の資料だけを読む。資料の中から別の資料を指している場合も、その作業が必要になった時点で読む。

| 依頼 | 読む資料 |
| --- | --- |
| 原本の取り込み・採用・移動・削除 | [取り込みと採用](references/import.md)。形式ごとの抽出範囲と制約を確かめるときは [対応形式](references/formats.md) |
| 採用データの本文を読む、設計情報を検索する、原本の見た目で確認するページ（view）を作る | [本文の読み方と検索](references/reading.md) |
| 値・数式・フォントの変更 | [値・数式・フォントの編集](references/value-edits.md) |
| 行・列の追加・削除・移動、記録した操作の取り消し | [行・列の追加・削除と操作の取り消し](references/layout-edits.md) |
| Excelの書式・シート・セル範囲・図形の配置・リンク・入力規則の変更 | [Excelの書式・シート・セルの操作](references/excel-edits.md) |
| PowerPointのスライド・図形の変更 | [PowerPointのスライドと図形の操作](references/slide-edits.md) |
| 変更の原本への書き戻し | [原本への反映](references/writeback.md) |
| 構造解釈が必要かの判断、原本変更後の構造解釈の引継ぎ | [構造解釈の要否と引継ぎ](references/structure.md) |
| 表と本文の区分、複数図形の関係、画像OCRを原本と照合して整理 | [arp4-structure](../arp4-structure/SKILL.md) |
| capture、workflowを使わない個別の意味抽出・採番・check・render | [個別CLI操作](references/spec-cli.md) |
| 新規workflowの開始、自動runner、usageの記録 | [workflowのCLI操作](references/workflow-cli.md) と [進行管理](references/orchestration.md) |
| カスタムAgentを使えない担当セッションでのタスクの読取・返信 | [タスクの読取と返信](references/task-replies.md) |
| 既存workflowの再開・抽出・レビュー・修正 | [進行管理](references/orchestration.md)。同じroot・run-idのstatus --summaryから再開する |
| 採番済みの要件・仕様の更新・承認 | [正本の継続保守](references/registry.md) |
| CLI応答のページ送り・省略・エラー・状態の読み方 | [CLI応答の読み方](references/responses.md) |
| ARP実行ファイルがプロジェクトにもPATHにもない、または更新が必要 | ARPソースの `docs/guides/install.md`。ソースの場所は利用者に確認する |

資料にある `docs/guides/`・`docs/reference/` で始まる文書パスはARPソースルート基準で、導入先の文書プロジェクトに存在するとは限らない。原本フォルダの既定 `docs/` は文書プロジェクト側のフォルダで、これとは別。実行時の引数・対応機能・返信契約は、資料で足りないときだけCLIのhelp・doctor・readで確かめる。

原本操作や台帳保守だけなら、workflowの担当Agentを起動する必要はない。

## 使う実行ファイル

資料中の `arp4` は、次の順で最初に見つかった実行ファイルを指す。

1. 文書プロジェクトのルート（`.arp/` を置くフォルダ）にある `arp4.exe`（Linuxは `arp4`）。チームがGitで共有している版で、PATHに別の版があってもこちらを使う。
2. PATHの `arp4`。

ルートの実行ファイルはコマンド名では起動されないため、パスで呼ぶ（PowerShellは `& '<project>\arp4.exe' …`、bashは `'<project>/arp4.exe' …`）。一つの作業の中で実行ファイルを切り替えない。workflowは開始時と同じ実行ファイルでしか続行できない。担当へ委任するときは、handoffのexecutableまたはこの実行ファイルのパスを渡す。ルートの実行ファイルを、ビルドやインストール済みの版で置き換えるのは、利用者に依頼されたときだけにする。

## workflowの責務

このスキルを使う親AgentがCLIによる準備・割当・提出確認・段階の進行・成果物報告を行う。意味判断はhandoffが指定するカスタムAgentへ委任する。

- `arp4-worker` は作成・修正を担当する。
- `arp4-reviewer` は作成・修正に参加していない独立した担当として監査する。

担当にはCLIのhandoffとインスタンス固有のactorを渡す。親はworkflowのread（instructions・packet・contextを含む全pointerと既定の全節読取）を実行せず、内部ファイル経由でも作業資料を読まない。親の会話履歴やこのスキルの参照資料を渡さず、資料の要約も作らない。担当がreadからタスク固有の契約と資料を取得する。委任文はhandoffとactorを基本とし、必要な追加指示も短くする。

完了はCLIのstatusと出力成果物で確認し、未解決事項と草稿はその状態で報告する。提出受理は意味的正解や公開承認ではない。採用・承認・原本への反映は利用者から与えられた権限内で行う。
