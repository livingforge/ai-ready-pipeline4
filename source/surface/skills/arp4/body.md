# ARP 文書正本の管理

## Excel ワークフロー

最初に `arp4 doctor --format json` を確認する。`implementation: rust` の場合はこの節の範囲で作業する。
起動できない場合は [arp4-setup](../arp4-setup/SKILL.md) を参照する。

- 初期化は `documents init --root <project>`、取り込みは `documents import <原本> --id <文書ID>`。
  対象は `.xlsx` / `.xlsm` のセル値・数式原文・結合範囲。図形・画像・コメント・印刷情報・OCRは未抽出で、R001に記録する。
  未抽出の情報は原本で確認し、読み取れたと扱わない。
- importのJSONにある `proposal_id` と `proposal` を使い、本文YAMLと対応表を確認して実際の成形を行う。
  型、page_id、行・列、field ID、セル対応を維持する。構造変更や対応表の自動更新は未対応。
- `documents check --proposal <候補ID>` と `documents diff <候補ID>` で確認し、
  実際に作業したactor/model/promptを `documents record <候補ID> --model <モデル> --actor <担当> --prompt <ファイル>` で記録する。
  実施していない成形を記録しない。権限のある担当者が `documents adopt <候補ID> --reviewer <担当>` を行う。
- 採用後は既存の本文の値を同じ型で編集し、`documents check <文書ID>`、`documents diff --document <文書ID>`、
  `documents review <文書ID> --reviewer <担当>`、`documents export <文書ID> --out <プロジェクト>/.arp/out/<新規名>.xlsx` の順で確認・反映する。
  文字列の先頭が `=` でも数式化しない。既存数式の変更、行追加削除、図形・画像操作は拒否される。
- 中断時は `documents status`。`resume`、`edit-base/plan/apply`、`spec`、他形式の取り込みは未対応なので呼び出さない。
  型や構造の変更が必要なら未対応と報告し、検証を回避しない。
- `--root` は全コマンドで指定できる。importの相対パスはプロジェクト基準、prompt・outの相対パスは実行ディレクトリ基準。
  diffは既定のMarkdown表示または `--format json`。JSONはRust版の `comparisons[].changes` 形式。
