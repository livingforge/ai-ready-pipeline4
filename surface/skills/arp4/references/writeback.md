# 原本への反映

採用済み文書の変更を export し、表示を確認して原本へ apply する。

- 採用後は `documents values <文書ID> --input <編集JSON>` で値の変更を保存し（行・列の追加削除は [専用コマンド](layout-edits.md) で行う。値の変更と構造操作は順序を問わず、未適用の操作があっても値を変更できる）、`documents check <文書ID>`、`documents diff --document <文書ID>`、
  `documents review <文書ID> --reviewer <担当>`、`documents export <文書ID> --out <プロジェクト>/.arp/cache/export/<新規名>.<原本と同じ拡張子>` の順で確認・反映する。数式を持つExcelは、WindowsのデスクトップExcelがあれば export に `--calculate` を付け、Excelが計算した数式の結果を候補に書き込む（付けないと編集で古くなる計算結果は空になり、Excelで開くまで表示されない）。
  原本へapplyする場合は、出力した文書を実際に開くか、`documents render <文書ID> --candidate <出力パス> --out <新規PNG> [--sheet <シート> --range <範囲> | --page <番号>]` で描画した画像を開き、変更箇所と周囲の折り返し・はみ出し・結合・印刷配置を確認する。renderはExcelのシート・範囲をExcelで、Word・PowerPointをOfficeでPDFにしたページを、PDFはそのページをWindowsで描画する（WindowsとデスクトップOfficeが必要。応答の `pages` で全ページを確認する）。未確認のままapplyしない。描画画像は確認の材料であり、描画の成功を確認済みとみなさない。出力と `.report.json` はapplyが終わるまで保持し、その後に利用者が成果物を求めた場合だけ残す。
- 原本へ反映する場合はreview→export→出力を開いたレイアウト確認→`documents confirm-export <文書ID> --candidate <出力パス> --output-sha256 <reportの出力ハッシュ> --reviewer <担当> --reason <実際に確認した範囲・結果> --layout-reviewed`→`documents apply <文書ID>`。レイアウト確認には `documents render` で描画した画像を使える。これは自動描画判定ではなく、実際に見た結果の記録である。出力を開けなければ未確認として止める。applyは確認した候補そのものを適用し、本文・原本・候補・reportが変わると拒否する。成功後は再抽出した候補がapplyによる成形（`formation.json` の `by: apply`）として記録され `ready_to_adopt` になるので、diffで確認してadoptする（recordは不要。候補を構造保存などで変えた場合は `needs_record` に戻るので、実際に作業した担当でrecordする）。出力が原本と同じバイト列（変更のない反映）なら原本を書き換えず、候補も作らずに `state: unchanged` を返す。`diff --document` はGitのHEADとの比較なので先に基準をコミットする。
