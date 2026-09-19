# 機能と説明文書の確認記録（2026-09-19）

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
