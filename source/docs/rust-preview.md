# ARP Rust試験版

このZIPはWindows x64向けのRust試験版（4.0.0-alpha.3）です。正式リリースではありません。
Pythonなしで、スキル導入・Excel取り込み・YAML編集・差分・成形記録・採用・レビュー・通常セルの書き戻しを実行できます。
今回の配布先は `C:/arp4-publish` です。GitHubへの公開は行いません。

## ZIPからの実行

配布物には `source/` と `audit/` を含めます。ソース・テスト・テストデータの確認、ハッシュ検証、オフライン再ビルド用の追加資材については、配布物の `audit/TESTING.md`（ソースでは [監査手順](audit-testing.md)）を参照してください。

Windows x64で展開したフォルダーから実行します。本体にスキルとスキーマを埋め込み、これらの操作にはPythonもRustも不要です。

```powershell
.\arp4.exe doctor
.\arp4.exe doctor --format json
.\arp4.exe skills install --root C:/my-project --agent github
.\arp4.exe documents schema mappings
```

プロジェクトのフォルダーは先に作成してください。`--agent` は `all`（既定）、`claude`、`github`、`none` を選べます。
`none` は何も書き込みません。導入するスキルは Rust 版専用です。

## Excelの取り込みから書き戻し

以下では、原本を `C:/my-project/docs/基本設計.xlsx` に置き、配布フォルダーで実行します。

```powershell
.\arp4.exe documents init --root C:/my-project
.\arp4.exe skills install --root C:/my-project --agent github
$candidate = .\arp4.exe documents import docs/基本設計.xlsx --id design --root C:/my-project | ConvertFrom-Json
.\arp4.exe documents check --proposal $candidate.proposal_id --root C:/my-project
.\arp4.exe documents diff $candidate.proposal_id --root C:/my-project
```

`$candidate.proposal` の `content/<シート名>.yml` をAgentまたは人が原本と照合して成形します。
本文の値は `blocks/table-1/rows/r<行番号>/<列名>` にあり、型・ID・セル対応を保って編集します。
機械生成した候補をLLMの成形済みとみなさず、実際に成形した後に作業のモデル・担当・プロンプトを記録してください。

```powershell
# 実際に成形した作業の情報を指定する
.\arp4.exe documents record $candidate.proposal_id --model <モデル名> --actor <担当> --prompt C:/my-project/prompt.txt --root C:/my-project
.\arp4.exe documents adopt $candidate.proposal_id --reviewer <レビュー担当> --root C:/my-project
```

採用後の本文は `knowledge/documents/docs/基本設計.xlsx/content/`、管理情報は `.arp/documents/docs/基本設計.xlsx/` に配置します。
本文の既存セル値を同じ型で編集した後は、以下の順に確認します。

```powershell
.\arp4.exe documents check design --root C:/my-project
.\arp4.exe documents diff --document design --root C:/my-project
.\arp4.exe documents review design --reviewer <レビュー担当> --root C:/my-project
.\arp4.exe documents export design --root C:/my-project
.\arp4.exe documents export design --out C:/my-project/.arp/out/基本設計-更新.xlsx --root C:/my-project
```

exportの `--out` 省略時は反映計画だけを出力します。出力時は新しいExcelと `.report.json` を作成します。
未レビュー・原本更新・未確定の対応・型違い・数式セルへの値上書き・署名付きブック・既存出力の上書きを拒否します。
通常セル更新では元のZIP部品を保持し、数式がある場合はキャッシュを無効化して次回Excel起動時の再計算を指定します。
Rust自身は数式を計算しません。`=...` で始まる通常セルの文字列は文字列のまま出力します。

`documents status` で候補と正本の検証状態を確認できます。`--root` 省略時は親の `.arp/config.yml` を探索します。
importの相対パスはプロジェクト基準、prompt・outの相対パスは実行ディレクトリ基準です。

## 対応範囲と互換性

- 対応する取り込みは `.xlsx` / `.xlsm` のセル値・数式原文・結合範囲です。図形・画像・コメント・印刷情報・OCRは未抽出で、候補と抽出JSONのR001に記録します。原本の外観も確認してください。
- 書き戻しは既存の通常セルの値のみです。行・図形・画像・数式の変更、Excel COMエンジンは未対応で、指定時は拒否します。マクロ部品は保持対象ですが、マクロ実行の検証はしていません。
- 見出し推定・空行ごとの表分割はまだ移植していません。元の行・列を保つ表を生成します。数式原文は別の表に保持します。
- `resume`、`edit-base/plan/apply`、`spec`、Word/PDF等、文書内のローカルリンクを持つ本文の検証は未対応です。
- 保存するスキーマ・出典・成形とレビューのハッシュはPython版の形式を使います。対象範囲の相互読込を試験しましたが、全既存文書への対応を保証するものではありません。
- diffは本文・対応表・除外理由・原本情報・画像ハッシュを比較します。画像の見た目は比較しません。
  `--format json` の `comparisons[].changes` はRust版の出力形式で、Python版の表示JSONと同一ではありません。`--format markdown --out <新規パス>` で保存できます。

スキルは `.claude/skills/` または `.github/skills/` へ導入し、
`.arp/installed-skills.json` に旧Pythonインストーラーと同じ形式のハッシュを保存します。
利用者が編集したファイルがある場合は、全スキルの更新前に停止します。CRLFとLFの差だけなら編集とみなしません。
同時に複数のRustスキル導入処理を実行できないようロックします。強制終了後にロックが残った場合は、
他の導入処理がないこととファイルの状態を確認してから `.arp/rust-skills-install.lock` を取り除きます。
更新中の外部編集は避けてください。
通常の書き込み失敗では変更済みファイルを復元しますが、電源断に対する複数ファイルの一括更新は保証しません。
文書の更新処理には `.arp/rust-documents.lock` を使います。同様に、残留ロックは処理が停止済みか確認して扱ってください。

`doctor` は現在の実装範囲を表示します。Excel・OCRの環境検出はまだ行いません。
JSONの `release_ready: false` と未実装機能を確認できます。未実装のコマンドは終了コード2で失敗します。

## ソースからの開発

Rustのツールチェーンは `rust-toolchain.toml`、依存は `Cargo.lock` で固定しています。
WindowsではMSVCのビルドツールが必要です。利用者向けの実行ファイルはCRTを静的リンクします。

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo run --locked --example sync_skills -- --check
./build/package_rust.ps1
./build/smoke_rust.ps1 -Zip target/preview-distribution/arp4-v4.0.0-alpha.3-windows-x64-preview.zip
./build/deploy_rust.ps1 -Zip target/preview-distribution/arp4-v4.0.0-alpha.3-windows-x64-preview.zip -Destination C:/arp4-publish
```

Python 版の実装と比較試験は削除しました。`contracts/document-schemas.json` と `contracts/excel-number-formats.json` を契約の正本として保守します。旧実装との比較結果は Git 履歴と verification.md に残しています。スキルは `surface/` からRustのビルド時に組み立てます。

ZIPの作成先に同名ファイルがある場合は上書きせず失敗します。別の `-OutputDirectory` を指定してください。
試験用ZIPには依存のライセンス表示・ライセンス本文を同梱し、SHA-256を隣接ファイルに出力します。
スモーク試験はZIPを一時フォルダーへ展開し、PATHを空にして実行・更新拒否を検証します。
この試験はPython未導入のOS、ネットワーク遮断、Agent上での選択、Excel実機の受入試験を代替しません。
deployは展開先の既存ファイルとの衝突を事前検査し、異なる内容を上書きせず停止します。
既存の `.git/` は保持し、Gitのコミット・push・Releases公開は実行しません。
展開内容のハッシュは `.arp/rust-distribution.json` に記録します。
