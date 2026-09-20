---
name: arp4-setup
description: ARP の Windows ZIP から実行ファイルを起動し、スキル導入と文書プロジェクトの初期化を行う。
license: MIT
---

# ARPの導入と起動

Rust試験版では `arp4 doctor` で実装済み機能を確認する。
`arp4 skills install --root <project> --agent all` はPythonなしでスキルを導入する。
Rust版は `documents init/import/check/status/record/diff/adopt/review/export/schema` に対応する。
Excelの通常セルを対象とし、図形・画像・コメント・OCRの抽出と、行・図形・数式変更の書き戻しは未対応である。
`documents init --root <project>` で初期化し、arp4スキルのRust版手順に従う。Python版のランタイム導入は不要。

ZIP を展開したフォルダーの arp4.exe を絶対パスで実行する。プロジェクトのフォルダーを先に作成する。既存の設定・ユーザーが編集したスキルを上書きしない。文書操作は [arp4](../arp4/SKILL.md) に従う。
