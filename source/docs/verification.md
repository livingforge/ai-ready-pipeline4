# Rust 専用化の検証（2026-09-20、4.0.0-alpha.3）

削除前の実装はソースの `7f0b9e7`、配布先の `a286d10` にコミット済みです。Python 本体・wheel・旧ビルド・専用テストを削除し、スキルと CI を Rust 専用に更新しました。サンプルアプリと資料生成用の Python は本体と独立した資産として残しています。

- `cargo test --workspace --locked`: 18 件成功（契約 3、Excel 3、導入 12）。Excel 試験は文字列・数値・真偽値・null、数式キャッシュ無効化、未知の ZIP 部品とコメントの保持、型違い・数式・結合セル・外部参照・署名の拒否を確認。
- `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --locked -- -D warnings`: 成功。
- `cargo run --locked --example sync_skills -- --check`: 4 スキルと埋め込み原稿が一致。
- `build/smoke_rust.ps1`: 最終 ZIP を展開、PATH を空にして導入→取り込み→成形記録→採用→編集→差分→レビュー→書き戻しに成功。.NET XML による独立した読み戻し、未記録の採用・未レビュー出力・既存出力・変更済み管理原本の拒否を確認。
- `build/test_deployment.ps1`: 初回・反復展開、Git メタデータの保持、既存ファイル衝突と不正チェックサムの拒否に成功。
- `C:/arp4-publish` に alpha.3 を展開し、全管理ファイルの SHA-256 と実行バージョンを確認。Python bootstrap 配布を削除。GitHub への push・公開は実施していません。

Python 版との比較試験は削除前の履歴に保存しています。上記の Rust 試験は旧 Python 全機能の試験と同じ範囲ではありません。空の Windows VM、Agent 上での導入、Excel 実機の受入試験は未実施です。

以下は過去の検証記録です。削除済みスクリプトのコマンドは現在の実行手順ではありません。

# 機能と説明文書の確認記録

## RustのExcel移植とローカル展開（2026-09-20）

Windows x64 / Rust 1.98.1 / MSVCで `4.0.0-alpha.2` を作成し、`C:/arp4-publish` へ展開した。
今回の対象はPython不要のExcelセル取り込み・差分・成形記録・採用・レビュー・通常セルのXML書き戻し。
GitHubへの公開・push・コミットは実施していない。展開先の既存Python配布物とGit履歴は保持した。

| 確認 | 結果 |
| --- | --- |
| `cargo test --workspace --locked` | 15件成功（スキル導入12件、厳密なデータ読込・数値正規化・Excel境界3件） |
| `cargo fmt --all -- --check` / `cargo clippy --workspace --all-targets --locked -- -D warnings` | 成功 |
| 最終全体テスト `python -m pytest -q` | 359件成功、Excel COM実機用3件スキップ。`ARP_RUST_BIN=C:/arp4-publish/arp4.exe` と最終ZIPを指定して実行 |
| `tests/test_rust_documents.py` | 20件成功。日本語パス、Python版との相互読込、原本保持、型・数式キャッシュ・ZIP部品保持、差分、記録後編集・再取り込み競合、原本更新、未確定対応、署名、重複ZIP、外部シート、不正YAML、出力拒否を確認 |
| `tests/test_rust_deployment.py` | 3件成功。実ZIPの展開と再展開、既存Python配布物・Gitデータの保持、衝突時の事前拒否、チェックサム不一致の拒否 |
| 配布先バイナリとPython版の比較 | 11スキーマと4スキルの一致。`build/check_rust_compat.py --binary C:/arp4-publish/arp4.exe` |
| `build/smoke_rust.ps1` | 最終ZIPを一時フォルダーへ展開し、PATHを空にしてスキル導入・Excel生成（.NET）・取り込み・記録・採用・YAML編集・差分・レビュー・書き戻し・独立したXML再読込を確認。Pythonプロセスは使用しない |
| `build/deploy_rust.ps1` | 展開前に全ファイルの衝突を確認し、展開後のハッシュを検証。`.arp/rust-distribution.json` に内容を記録 |
| PE依存確認 | `arp4.exe` の直接依存はWindowsシステムDLLのみ。Python DLL・MSVCランタイムDLLへの直接依存なし |

最終配布ZIPは `target/local-distribution/arp4-v4.0.0-alpha.2-windows-x64-preview.zip`。
SHA-256: `dcc9bb673f88f868d45bb68b4dcb0f67e7d6a7f430ea680ff708c2c2838d4e0c`。
スキルとスキーマは実行ファイルに埋め込む。依存ライセンスを同梱し、crateに本文がない一部依存は同じリポジトリコミットの原文を収録した。

成形・レビューは試験用の `test-fixture` / `synthetic` として記録した。実際のLLMによる成形や業務レビューを実施した結果とは扱わない。
非セル情報（図形・画像・コメント・印刷情報）とOCRは未抽出で、R001と候補本文に明示する。
見出し推定・表分割、構造変更・数式変更の書き戻し、文書内ローカルリンク、他形式・仕様生成などの残作業は [試験版の対応範囲](rust-preview.md) を参照。
空のWindows VM、ネットワーク遮断、Agent製品上の自動スキル選択、Excel COM、マクロ実行・Office外観は未検証。
そのため `doctor` の `release_ready` は `false` を維持する。

## Rust移植の初期確認（2026-09-20）

Windows x64 / Rust 1.98.1 / MSVCで、スキル導入・文書スキーマ出力・機能一覧の試験実装を検証した。
Rust版のExcel取り込み・文書管理・差分・書き戻し・仕様生成は未実装である。

| 確認 | 結果 |
| --- | --- |
| `cargo test --workspace --locked` | 12件成功。新規導入、Agent選択、再導入、CRLF、編集保護、全件事前検査、不正パス、ロック、JSONスキーマ、未実装コマンドの拒否、Windowsジャンクション拒否、所有記録の更新失敗時の復元 |
| `cargo fmt --all -- --check` | 成功 |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | 成功 |
| `python build/rust_contracts.py --check` | 現行Python版と埋め込み用スキーマの一致 |
| `python build/check_rust_compat.py --binary target/debug/arp4.exe` | 11種類のスキーマと4つのスキルの一致 |
| `build/package_rust.ps1` | CRTを静的リンクしたWindows x64試験ZIP、依存ライセンス、SHA-256を作成 |
| `build/smoke_rust.ps1 -Zip <試験ZIP>` | ZIPを別フォルダーに展開し、PATHを空にして機能一覧・スキル導入・スキーマ出力・編集済みスキルの更新拒否を確認 |
| 既存Python版 `python -m pytest -q` | 336件成功、Excel実機用3件スキップ（移植作業開始時の基準） |

互換性試験でスキルのfrontmatter内コメントの除去差を検出し、Rust側の組み立てを修正して再試験した。
配布物のライセンス収集はWindows対象の依存グラフに限定した。
スキルは実行ファイルに埋め込み、`.arp/installed-skills.json` は既存Python版と同じハッシュ形式を使う。
試験ZIPは `target/preview-distribution/arp4-v4.0.0-alpha.1-windows-x64-preview.zip` に出力した。

PATHを空にした試験は、Python未導入のOSでの受入試験ではない。
ネットワーク遮断、Agent製品上のスキル利用、Excel COM実機、GitHub Actionsの実行・Releases公開は未確認・未実施。
試験版は `doctor --format json` で `release_ready: false` と未実装機能を明示する。
実行方法と制限は [Rust試験版](rust-preview.md) を参照。

## UX改善の追加確認（2026-09-20）

Windows / CPython 3.12で次を確認しました。

- 全体テスト335件成功、Excel実機用3件スキップ（追加の再開テストは下記UXテストで確認）。
- `tests/test_document_ux.py`: 取り込みから記録・編集・採用への状態遷移、読み取り専用の再開、原本更新・正本競合・検証エラーの区別、編集基準を使った再開案内。
- 差分の型・Excel対応先、対応表だけの変更、レビュー時の本文を基準にした比較、既存レポートの上書き拒否。
- スキル同時導入、未編集の配布スキルの更新、利用者が編集したスキルの保護、Agentの選択。
- 新しいwheelと依存wheelを含む配布物を構築し、空の独立プロジェクトへオフライン導入。インストールされたパッケージだけで取り込み・状態確認・差分・採用・XMLによるExcel書き戻し・仕様整理と設計書生成を確認。
- `build/build.py --check` とスキルのfrontmatter検証に成功。

`resume` は現在の証跡から次の操作を案内し、Agentを起動したり採用を自動実行したりしません。
今回はオンライン導入・別OS・Agent製品上での自動スキル選択・Excel COM実機は未検証です。
以下は過去の確認記録で、当時の件数と機能範囲を保持しています。

## 過去の確認（2026-09-19）

Windows 11 / CPython 3.12.10で、README、文書管理・仕様整理の説明、CLI、実装、
テストを照合した記録です。対応する全ファイル・全環境での動作保証ではありません。

## 実行結果

| 確認 | 結果 |
| --- | --- |
| `python -m pytest -q` | 108件成功、Excel実機用3件スキップ |
| `python build/build.py --check` | 展開済み4ファイルが生成元と一致 |
| 独立プロジェクトでのCLI実行 | 44回が期待した終了コードと一致。うち9回は未レビュー・未完了・既存出力などの拒否確認 |
| Excelセル更新 | 新規出力をopenpyxlで再読込し、10→12、直接編集・再レビュー後の13を確認。入力元の10は維持 |
| 仕様整理の見本 | `examples/specification-demo/build.py`を新しい出力先で実行し、18種類の成果物を生成 |

CLI検証では機械的な検証用データを使用した。成形記録は`test-fixture`、レビュー担当は
`synthetic-audit`とし、LLMによる意味の整理や業務上のレビューを実施したとは扱っていない。
既存の`work/parse-demo`は変更していない。

## 機能ごとの根拠

| 機能 | 確認内容・範囲 |
| --- | --- |
| 原本取り込み | Excel、実例のWord・PowerPoint・PDF・CSV、Markdown・テキスト・TSV・Python・Java・SQL・DDLのテスト成功。破損Officeの拒否も確認 |
| 文書の正本化 | CLIでinit→import→check→record→diff→adoptを実行。採用後のlist、drawings、watch --once、レビュー必須checkも成功 |
| 編集・出典・レビュー | 出典・対応漏れ、証跡改変、記録後編集の拒否をテスト。CLIで直接編集後のレビュー無効化と再レビューを確認 |
| 再取り込み | CLIで原本更新後の再取り込み・diffを実行。正本編集との競合保護は既存テストで確認 |
| Excelセル書き戻し | CLIで計画表示・出力・再読込。型、数式と書式、変更対象外ZIP内容、署名付きブック拒否は既存テストで確認 |
| 行・数式・図形・画像・接続線 | 操作契約・座標変換・Excelエンジン選択のテスト成功。実Excelでの保存・再読込は未実行 |
| 複数文書からの仕様整理 | 複数入力・除外理由・関係・出典の検証テスト成功。CLIでは採用済み文書から仕様を作成 |
| 設計書生成 | CLIで草稿・正式出力・未完了時の拒否・再生成時の出力先一致を確認。全種類の仕様を含む見本も生成 |
| 仕様更新 | CLIで入力変更の検出、refresh、影響項目のdraft化を確認。参照切れ・既存項目維持は既存テストで確認 |
| 導入・配布 | インストーラーのハッシュ・環境・既存ファイル保護等のテスト成功。今回、新規wheel配布物の構築とオンライン／オフライン導入は実行していない |

主なテストはソースリポジトリの`tests/test_documents.py`、`tests/test_current_workflow.py`、
`tests/test_document_operations.py`、`tests/test_specifications.py`、
`tests/test_project_installer.py`にある。テストコードは配布物には含まれない。

## 説明を修正した点

[文書管理の説明](documents.md)を次のように実装へ合わせた。

- 対応拡張子を列挙し、旧Office形式とプログラム実行が対象外であることを明記。
- `record --prompt`の相対パスは`--root`ではなく実行ディレクトリ基準と明記。
- VSCodeタスクはPATH上の`arp4`を使うため、専用環境を使う場合の実行パス設定を追記。
- 実装に存在しない「旧ラウンドの検索除外」を削除し、実際の除外対象を記載。
- `diff`の内容差分はMarkdownだけであり、対応表・画像の差分を含まないことを明記。
  対応表だけを編集してdiffの出力が変わらないことも実行確認した。
- `export`は計画表示でもレビュー済み・原本の版一致が必要と明記。
- 通常のセル更新先は抽出された既存セルに限定されることを明記。
  未使用セルZ99を指定した場合の`missing Excel target`も確認した。

## 残る確認

このPython環境にpywin32はなく、`Excel.Application`のCOM登録も見つからなかった。
そのため、行操作、数式変更、図形・画像・接続線の実機動作を「確認済み」とはしない。
Windows + Microsoft Excel + `writeback`依存のある環境で次を実行する。

```powershell
$env:ARP_TEST_EXCEL = '1'
python -m pytest tests/test_document_operations.py -q
```

通常実行では次の3件がスキップされる。

- `test_real_excel_rows_formulas_names_tables_and_literal_values`
- `test_real_excel_shape_edit_add_delete_connect_and_picture`
- `test_real_excel_invalid_formula_leaves_no_output`

VSCode上のProblems表示、Office上の外観、OCRの読取品質、実案件の意味の正しさは今回の確認対象外。
`.xlsm`は不透明なマクロ部品の保持テストがあるが、実マクロの実行は検証していない。
`.docm`・`.pptm`は対応拡張子と処理分岐を確認したのみで、今回の実ファイル試験は`.docx`・`.pptx`である。

ローカルの追加検証スクリプトは`work/capability-audit/run.py`。
成功した実行のログ・出力は`work/capability-audit/run-yrk8ct9v/`に保存した。
`cli-log.json`に各引数・期待終了コード・実際の終了コード・標準出力・標準エラーを記録している。
`work/`はGit対象外であり、これらのローカル証跡は配布物には含まれない。
