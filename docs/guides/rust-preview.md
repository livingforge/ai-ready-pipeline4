# ソースからの導入と基本操作

Windows／Linux でソースをビルドして使う試験版です。Rust と OS のビルドツールを準備し、ソース ZIP の展開先または固定版のチェックアウトで実行します。前提環境・Agent 経由の導入・更新の手順は [導入原稿](../../surface/skills/arp4-setup/body.md) を参照してください。

```text
cargo install --path crates/arp4-cli --locked
arp4 --version
arp4 doctor
arp4 skills install --root <project> --agent github
arp4 documents init --root <project>
```

`<project>` を対象プロジェクトのパスに置き換えます。通常操作のために Python や PowerShell 7 は不要です。履歴管理と `documents diff --document` には Git を使います。Excel の画面描画は Windows とデスクトップ Excel が必要です。

カスタム Agent はソース内の `surface/skills/arp4/body.md` を直接読み込めます。モデルの自動実行には [AI実行先との接続](semantic-runners.md) を別途設定します。

`skills install` は対象フォルダーがなければ親フォルダーも含めて作成します。対象パスがファイルの場合や、必要な書き込み権限がない場合はエラーになります。
`--agent` は `all`（既定）、`claude`、`github`、`codex`、`none` を選べます。
`none` はフォルダーも作成せず、何も書き込みません。導入するスキル・カスタムAgentは Rust 版専用です。
スキル導入は文書管理を初期化しないため、Excelの取り込み前に `documents init` も実行してください。

## Excelの取り込みから書き戻し

取り込みをAIに委任する場合、利用者は原本またはフォルダと作業範囲を伝えます。AIはimport、候補の成形と検査、record、adoptを進め、実際に内容を確認した文書だけreviewを記録します。結果は取り込み件数、採用件数、未レビュー件数、確認が必要な文書の箇所と理由をまとめて報告します。原本でしか確認できない情報、曖昧な対応、失敗やblockersがあれば、該当文書について利用者の判断を求めます。採用はレビュー完了を意味しません。

以下では、原本を `C:/my-project/docs/基本設計.xlsx` に置き、任意の作業フォルダーで実行します。この節の JSON 取り出し例は PowerShell 用です。文書IDは `docs/` からの相対パス `基本設計.xlsx` です。Linux の Agent は JSON 応答の document_id を読み、同じ CLI 引数で操作します。

```powershell
arp4 documents init --root C:/my-project
arp4 skills install --root C:/my-project --agent github
$candidate = arp4 documents import docs/基本設計.xlsx --root C:/my-project | ConvertFrom-Json
arp4 documents check --proposal $candidate.document_id --root C:/my-project
arp4 documents diff $candidate.document_id --root C:/my-project
```

`$candidate.proposal` の `content/<シート名>.yml` をAgentまたは人が原本と照合して成形します。
本文の値は `blocks/table-1/rows/r<行番号>/<列名>` にあり、型・ID・セル対応を保って編集します。
行・列を追加または削除する場合は、`documents rows` / `documents columns` を使います。`mappings.yml` の構造操作と本文の `<操作ID>-<番号>` キーを、1回の実行でまとめて書き込みます（仕様は [CLI応答仕様](../reference/cli.md) の「行・列の構造変更」）。

```powershell
# 課題一覧の最終行の後に1行追加する（先に --dry-run で位置と値の変換を確認する）
Set-Content -Encoding utf8 row.yml '- {B: "8", C: 2025/11/21, E: 仕様, F: 課題内容, J: 未着手}'
arp4 documents rows insert --proposal $candidate.document_id --sheet 課題一覧 --after last --id add-issue8 --reason 課題No.8を追加 --values row.yml --dry-run --root C:/my-project
arp4 documents rows insert --proposal $candidate.document_id --sheet 課題一覧 --after last --id add-issue8 --reason 課題No.8を追加 --values row.yml --root C:/my-project
```
PowerPointのスライドは `documents slides insert --from slide-1 --after slide-2 --id <操作ID>` で複製し、`documents slides delete --slide slide-3` で削除します。複製したスライドの本文は `content/<操作ID>.yml` に作られます（仕様は [CLI応答仕様](../reference/cli.md) の「スライドの追加・削除」）。

機械生成した候補をLLMの成形済みとみなさず、実際に成形した後に作業のモデル・担当・プロンプトを記録してください。

```powershell
# 実際に成形した作業の情報を指定する
arp4 documents record $candidate.document_id --model <モデル名> --actor <担当> --prompt C:/my-project/prompt.txt --root C:/my-project
arp4 documents adopt $candidate.document_id --root C:/my-project
arp4 documents review $candidate.document_id --reviewer <確認担当> --root C:/my-project
```

`adopt` は候補を現在版へ移しますが、レビュー記録は作りません。採用直後は `needs_review` で、内容を確認した担当者が `review` すると `reviewed` になります。`record`・`adopt`・`review` は文書IDの代わりにフォルダIDを渡すと配下の文書を、`--all` を付けると全文書をまとめて処理します。対象はその操作を待っている状態（recordは `needs_record`、adoptは `ready_to_adopt`、reviewは `needs_review`）の文書だけで、他の状態の文書は `skipped` に状態・blockersを付けて返します。1件が失敗しても他の文書は処理し、`failed` があれば終了コード2です。処理済みの文書は対象の状態から外れるので、同じコマンドの再実行で残りだけを処理します。

確認した内容だけを処理するには、先に `--dry-run --out <計画.json>` で対象と各文書のcontentハッシュを保存し、確認後に `--expect <計画.json>` を付けて実行します。計画にない文書は `not_planned`、計画後に内容が変わった文書は `changed_since_plan` として処理せず `skipped` に返します。

```powershell
# 資料フォルダ配下の候補のうち、recordを待つものを確認してから記録する
arp4 documents record 資料 --model <モデル名> --actor <担当> --prompt C:/my-project/prompt.txt --dry-run --out record-plan.json --root C:/my-project
arp4 documents record 資料 --model <モデル名> --actor <担当> --prompt C:/my-project/prompt.txt --expect record-plan.json --root C:/my-project
# 記録済みの全候補を採用する
arp4 documents adopt --all --dry-run --out adopt-plan.json --root C:/my-project
arp4 documents adopt --all --expect adopt-plan.json --root C:/my-project
```

同じ `--prompt` を全件に記録するため、全件で実際に同じ成形・確認を行った場合だけまとめて記録してください。

採用後の本文・管理情報は `.arp/documents/基本設計.xlsx/` に集約します。原本は `docs/基本設計.xlsx` を相対パスで参照し、コピーを保存しません。原本と `.arp/` の共有データをGitへコミットしてから `diff --document` でHEADと比較します。
本文の既存セル値を同じ型で編集した後は、以下の順に確認します。

```powershell
arp4 documents check 基本設計.xlsx --root C:/my-project
arp4 documents diff --document 基本設計.xlsx --root C:/my-project
arp4 documents review 基本設計.xlsx --reviewer <レビュー担当> --root C:/my-project
arp4 documents export 基本設計.xlsx --root C:/my-project
arp4 documents export 基本設計.xlsx --out C:/my-project/.arp/cache/export/基本設計-更新.xlsx --root C:/my-project
```

exportの `--out` 省略時は反映計画だけを出力します。出力時は新しいExcelと `.report.json` を作成します。原本への反映は `documents apply 基本設計.xlsx --root C:/my-project` を使います。反映後は再抽出された候補を確認し、record・adoptしてください。
未レビュー・原本更新・未確定の対応・型違い・数式セルへの値上書き・書き戻し対象外の値（結合セルの左上以外の値、数式の計算結果）の編集・署名付きブック・既存出力の上書きを拒否します。
通常セル更新では元のZIP部品を保持し、数式がある場合はキャッシュを無効化して次回Excel起動時の再計算を指定します。
Rust自身は数式を計算しません。`=...` で始まる通常セルの文字列は文字列のまま出力します。

`documents status` で候補と正本の検証状態を確認できます。`--root` 省略時は親の `.arp/config.yml` を探索します。
importの相対パスはプロジェクト基準、prompt・outの相対パスは実行ディレクトリ基準です。

CLIは既定でAgent向けJSONを返します。成否・ページ送り・詳細取得は [CLI応答仕様](../reference/cli.md) を参照してください。

## 対応範囲

形式別の取り込み・編集・書き戻しの可否と制限は [文書操作](documents.md) を参照してください。

## スキルの更新とロック

スキル・参照ファイル・カスタムAgentをホスト別の場所へ導入します。Codexも選択できます。配置と委任の手順は [カスタムAgentによる委任](custom-agents.md) を参照してください。
`.arp/installed-skills.json` に導入したファイルのハッシュを保存します。
利用者が編集したファイルがある場合は、全配布ファイルの更新前に停止します。CRLFとLFの差だけなら編集とみなしません。
同時に複数のRustスキル導入処理を実行できないようロックします。強制終了後にロックが残った場合は、
他の導入処理がないこととファイルの状態を確認してから `.arp/rust-skills-install.lock` を取り除きます。
更新中の外部編集は避けてください。
通常の書き込み失敗では変更済みファイルを復元しますが、電源断に対する複数ファイルの一括更新は保証しません。
文書の更新処理には `.arp/rust-documents.lock` を使います。同様に、残留ロックは処理が停止済みか確認して扱ってください。

`doctor` は現在の実装範囲を表示します。Excel・OCRの環境検出はまだ行いません。
JSONの `release_ready: false` と未実装機能を確認できます。未実装のコマンドは終了コード2で失敗します。

## 次に読む文書

- [設計書生成の進行管理](semantic-workflow.md): 抽出・修正・独立レビュー・再開。
- [正本管理](registry.md): 要件・仕様の継続保守。
- [開発・配布手順](../development/development.md): ソースの変更とZIP作成。
- [文書一覧](../index.md): 仕様、検証記録、設計案。
