# ソースからの導入と基本操作

Windows／Linux でソースをビルドして使う試験版です。Rust と OS のビルドツールを準備し、ソース ZIP の展開先または固定版のチェックアウトで実行します。前提環境・Agent 経由の導入・更新の手順は [導入原稿](../../surface/skills/arp4-setup/body.md) を参照してください。

```text
cargo install --path crates/arp4-cli --locked
arp4 --version
arp4 doctor --format json
arp4 skills install --root <project> --agent github
arp4 documents init --root <project>
```

`<project>` を対象プロジェクトのパスに置き換えます。通常操作のために Python や PowerShell 7 は不要です。履歴管理と `documents diff --document` には Git を使います。Excel の画面描画は Windows とデスクトップ Excel が必要です。

カスタム Agent はソース内の `surface/skills/arp4/body.md` を直接読み込めます。モデルの自動実行には [AI実行先との接続](semantic-runners.md) を別途設定します。

`skills install` は対象フォルダーがなければ親フォルダーも含めて作成します。対象パスがファイルの場合や、必要な書き込み権限がない場合はエラーになります。
`--agent` は `all`（既定）、`claude`、`github`、`none` を選べます。
`none` はフォルダーも作成せず、何も書き込みません。導入するスキル・カスタムAgentは Rust 版専用です。
スキル導入は文書管理を初期化しないため、Excelの取り込み前に `documents init` も実行してください。

## Excelの取り込みから書き戻し

以下では、原本を `C:/my-project/docs/基本設計.xlsx` に置き、任意の作業フォルダーで実行します。この節の JSON 取り出し例は PowerShell 用です。Linux の Agent は JSON 応答から proposal_id を読み、同じ CLI 引数で操作します。

```powershell
arp4 documents init --root C:/my-project
arp4 skills install --root C:/my-project --agent github
$candidate = arp4 documents import docs/基本設計.xlsx --id design --root C:/my-project | ConvertFrom-Json
arp4 documents check --proposal $candidate.proposal_id --root C:/my-project
arp4 documents diff $candidate.proposal_id --root C:/my-project
```

`$candidate.proposal` の `content/<シート名>.yml` をAgentまたは人が原本と照合して成形します。
本文の値は `blocks/table-1/rows/r<行番号>/<列名>` にあり、型・ID・セル対応を保って編集します。
行・列を追加または削除する場合は、候補の管理側 `mappings.yml` に構造操作を記録します。`insert_rows` / `delete_rows` / `insert_columns` / `delete_columns` の `sheet`、`at`、`count`、`reason` を指定してください。追加する行・列は本文に `<操作ID>-<番号>`（例: `add-1`）のキーで書きます。既存の `r<行番号>` と列名は原本の位置を指すため、追加した行・列の値には使いません。追加した本文の行・列に対応するmapping entryは、CLIが現在の本文と構造操作から再生成します。
機械生成した候補をLLMの成形済みとみなさず、実際に成形した後に作業のモデル・担当・プロンプトを記録してください。

```powershell
# 実際に成形した作業の情報を指定する
arp4 documents record $candidate.proposal_id --model <モデル名> --actor <担当> --prompt C:/my-project/prompt.txt --root C:/my-project
arp4 documents adopt $candidate.proposal_id --reviewer <レビュー担当> --root C:/my-project
```

採用後の本文・管理情報は `.arp/documents/design/` に集約します。原本は `docs/基本設計.xlsx` を相対パスで参照し、コピーを保存しません。原本と `.arp/` の共有データをGitへコミットしてから `diff --document` でHEADと比較します。
本文の既存セル値を同じ型で編集した後は、以下の順に確認します。

```powershell
arp4 documents check design --root C:/my-project
arp4 documents diff --document design --root C:/my-project
arp4 documents review design --reviewer <レビュー担当> --root C:/my-project
arp4 documents export design --root C:/my-project
arp4 documents export design --out C:/my-project/.arp/cache/export/基本設計-更新.xlsx --root C:/my-project
```

exportの `--out` 省略時は反映計画だけを出力します。出力時は新しいExcelと `.report.json` を作成します。原本への反映は `documents apply design --root C:/my-project` を使います。反映後は再抽出された候補を確認し、record・adoptしてください。
未レビュー・原本更新・未確定の対応・型違い・数式セルへの値上書き・書き戻し対象外の値（結合セルの左上以外の値、数式の結果と原文）の編集・署名付きブック・既存出力の上書きを拒否します。
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
