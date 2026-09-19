"""Instructions distributed with the installed package, without copying its source tree."""

AGENT_GUIDE = """# ARP 文書正本の編集手順

このディレクトリの documents/*/content/**/*.md が文書の正本です。
通常検索はこの範囲に限定し、.arp/evidence、proposals、rounds、out は根拠確認時だけ読みます。
正本は人・Agentが直接編集できます。機械抽出JSONと原本スナップショットは編集しません。
原本は documents/<文書ID>/original/、整理した仕様データは spec/ にあります。
配置はプロジェクトの .arp/config.yml に記録します。原本更新後は original/ のファイルを再取り込みします。

## 編集

- 文書ID・節ID・型付き表の行IDは、見出しや表示順を変えても維持します。
- 正本本文を別のJSON/YAMLへ複製しません。mappings.ymlは参照と対応関係だけを持ちます。
- 本文は1つのH1の後、単独行の `<!-- arp:block id="stable-id" -->` で区切ります。
- 表は行頭・末尾に | を置きます。書き戻し項目は id / type / value の3列で、valueはJSONの値です。
- 新しい節・項目にはmappings.ymlを追加します。原本出典がなければreasonに追加理由を書きます。
- 削除した抽出内容はmappings.ymlのomissionsへ根拠を付けて記録します。
- 意味の補完、矛盾の解消、仕様変更は、単なる成形と区別して差分レビューに示します。
- 原本セルを指すtargetは根拠にもなりますが、originsと書き戻し先は別の役割です。
- 編集後は `arp4 documents check --root <project>`。レビューの権限があるときだけreviewを実行します。

## LLMによる成形

1. documents importの出力した候補と同名の.prompt.mdを読む。
2. 機械抽出JSONと画像を確認し、候補のcontent/とmappings.ymlを編集する。
3. 重複した機械プレビューは整理する。数値を本文にも複製した場合は整合性をレビューする。
4. `documents check --proposal <name> --root <project>` を通す。
5. `documents record <name> --model <実際のモデル> --actor <担当> --prompt <実際の手順ファイル> --root <project>`。
6. `documents diff <name> --root <project>` で差分を確認する。
7. 採用権限があるとき `documents adopt <name> --reviewer <担当> --root <project>`。

recordはLLMを呼び出しません。この手順を実行するAgent/人の実際の出力を保存する操作です。
モデル名は実際の実行情報を記録し、未知ならunknown、人の成形ならhumanとします。

## 再取り込み

採用済み正本を上書きしません。候補と、diffに示される基準・現在の正本を比較して編集を統合します。
前回の成形結果はformation.jsonが指す.arp/evidence/formed/<hash>/です。
セル番地が同じでも同じ業務項目とは限りません。行挿入・並べ替え後はIDとtargetを確認します。
原本にない追加記述も引き継ぎます。競合を機械的にどちらかへ寄せません。

## Excel書き戻し

まず `documents export <id> --root <project>` で計画を読みます。
pendingは未反映、excluded/omissionsは理由付きの対象外です。勝手にexcludedへ変えて通しません。
セル値更新はXML方式で動作します。行の挿入・削除、図形・画像・接続線、数式更新はExcel方式を自動選択します。
高度な更新にはWindows + Microsoft Excelと追加依存[writeback]が必要です。
対応表はschema_version: "2"、operations: []を持ちます（旧版1も読み込めます）。
行操作のatは原本の行番号です。既存セルのtargetも原本座標を維持し、移動は機械が計算します。
挿入セルはtarget: {sheet: ..., insertion: <操作ID>, offset: 0, column: B}で指定します。
数式は型付き表にstringの=式を書き、writeback: formulaで明示します。式中の座標は行操作後の位置です。
図形の値は型付き表へ置き、writeback: operationの対応を追加し、operationsのpropertiesから参照します。
`documents drawings <id>`（候補は--proposal）で原本の図形名を確認してください。
操作kindはinsert_rows/delete_rows/update_shape/add_shape/delete_shape/add_picture/replace_picture/add_connectorです。
図形位置・寸法・フォントサイズ・線幅はポイント、色は#RRGGBBまたはnoneです。
出力は `--out .arp/out/<new-name>.xlsx`（マクロ付き原本は.xlsm）。原本を上書きしません。
数式の計算は行わず、値・数式・行の変更時は計算キャッシュを無効化し、次にExcelで開く際の再計算を要求します。
"""

FORMATION_PROMPT = """# 文書成形の依頼

機械抽出結果を読み、採用候補のMarkdownを人とAgentが参照しやすい文書へ成形してください。
この依頼そのものはLLMの実行記録ではありません。実際の成形後にdocuments recordを実行します。

対象候補: {proposal}
入力JSON: {extraction}
現在の正本（初回は存在しません）: {authority}
手順: {guide}

- 見出し、段落、表を整理する。数値、否定、条件、未読取の申告を落とさない。
- JSONプレビューを自然な本文へ成形する。原本の画像が必要ならassetsを参照する。
- 意味を補完した箇所や仕様変更はreasonと差分で説明する。
- 元の情報を捨てるときはomissionsに理由を記録する。
- 原本出典とExcel書き戻し先を混同しない。
- 型付き表のセル値がExcelへ書き戻す値になる。説明文だけ変更してもセル値は変わらない。
- 再取り込み時は現在の正本と前回成形出力を読み、人・Agentによる追加修正を維持する。
- 同一性を確認して既存IDを引き継ぐ。行番号やセル番地だけで同一性を決めない。
- 未確定の対応はpendingのまま残す。検証を通すための架空の根拠や除外理由を書かない。
"""
