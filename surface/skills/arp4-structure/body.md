# ARPの構造解釈と画像照合

原本の表・本文・図・画像をまとめて確認し、意味抽出に渡す構造解釈を整える。文書の値や書式を直す依頼、要件・仕様の抽出は別の作業とする。操作の詳細と契約は利用環境の `arp4 spec structure --help`、`arp4 documents structure-read --help`、`arp4 spec structure schema`、`arp4 spec structure read` から取得する。ARPのソースリポジトリで作業する場合は `docs/guides/document-structure.md` も参照できる。

## 進め方

1. 対象文書と作業ルートを確定し、採用済み文書は `documents structure-read <文書ID> --out <整理YAML>`、単独の抽出結果は `spec structure init --extraction <抽出JSON> --out <整理YAML>` で作業ビューを用意する。原本・抽出結果の版と既存の解釈・レビュー状態を確認する。
2. `spec structure read` で原文セル、表の推定、図形、OCR、検証済み画像パスを読む。各シートで表と本文の境界、見出し・単位・注記、図形群、画像を対象にする。OCRの精度確認対象となる画像は空結果・unavailableを含めてすべて実際に開く。表や図の見た目が必要な箇所は原本または `render`・`region` で得た画像を開く。Excel以外の画像は利用者が用意した原本由来PNGを `region` に登録する。
3. `arp4-structure-worker` に整理YAMLの作成・修正を担当させる。変更後に `spec structure check` を実行し、未確認箇所と判断理由を報告させる。作成担当はレビューを記録しない。
4. 作成に参加していない `arp4-structure-reviewer` に原本・画像と整理結果の照合を担当させる。指摘があれば作成担当へ戻して再確認する。実際に確認した範囲が受理条件を満たす場合だけ `spec structure review --decision accepted` を記録する。
5. 採用済み文書または候補への反映が依頼範囲に含まれる場合、レビュー済みビューを `documents structure-save` で保存する。後続の意味抽出が必要なら、その後に `spec capture` を実行する。構造変更後は以前のcaptureや進行中workflowへ自動反映されないため、再captureして更新する。

担当を起動できない環境では同じ手順を順番に実行し、作成者自身の点検を独立レビューとして記録しない。原本や画像を確認できない箇所は未確認として残し、文書・シート・セルまたはvisual IDと理由を報告する。CLIの `check` 成功は内容の正しさを保証しない。

## 照合の基準

- **表と本文**: `kind: text` の初期値を確定済み分類とみなさない。抽出済みセルを重複なく一つのelementに所属させ、表題・説明・表本体・多段見出し・単位・注記を原本に合わせる。表に対応する別text elementは `descriptions` で結び、同じセルを表へ複製しない。
- **複数図形の図**: 図形の文字・形状・位置・明示された接続を先に読み、線の方向、包含、順序、凡例は図全体の文脈で確認する。必要なら `visual.graph` のnodes・edgesへ関係と根拠を記録する。OCR文字列だけで接続や階層を推測しない。
- **OCR画像**: `available` の空文字はOCR実行済みの結果として扱う。このスキルがOCRの精度確認を担う場合は実画像を開き、誤読・欠落・空結果を一件ずつ照合する。補正時は `ocr.text`、`ocr.reason`、`ocr.llm_correction` と画像の `evidence` を契約どおり記録する。画像だけの文字を原本セルの正確な引用に追加しない。

`reading.method: vision`、画像確認、OCR補正、レビューの記録は実施した作業だけに付ける。原本セルの文字列・式・結合・書式は構造解釈で書き換えない。
