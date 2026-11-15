# 正本の継続保守

採番済みの要件・仕様のregistryへの登録、更新、承認。

- 抽出モデルと `.ids.json` を `spec registry init --input <入力> --model <採番済みモデル> --catalog <分類表> --project <名称>` で登録する。分類表には全IDの category（requirement/specification/observation/estimate/reference）、module、requirements、related と、actor/reason/modules/open_issues/open_issue_documents を指定する。open_issue_documentsは各事項の対象文書を列挙し、全体共通は空配列、事項なしは空オブジェクトとする。各項目の分類理由reasonとverificationは任意で、省略は検証済みを意味しない。曖昧な分類には理由を添える。初回抽出のリンクは省略でき、workflowのlink段階で補完後に独立レビューする。参考・実績・試算は原表型value.kind=tableで保持できるが、設計条件の抽出や数量照合の回避には使わない。初回登録は全件 proposed であり承認ではない。
- 正本は `.arp/registry/` の registry.json・records/*.json・evidence/・archive/。`spec registry check` で検証し、`spec registry render` で機能別本文と根拠・対応表を生成する。Markdownを直接正本として編集しない。
- 更新は `spec registry apply --change <変更JSON>`。変更には check の base_hash、実際の actor、reason を必須とし、entries（全項目内容、proposed/approval:null）、retire、approve、modules、resolve_issues、add_issues を指定できる。新規IDは new:<キー>、既存IDは維持。原資料がない新規判断は evidence:[] と根拠理由を明記する。追加根拠の入力は --input で保存する。取り込み直した入力が、既存の記録の引用する出典を同じ内容・同じ文書の版で持つ場合、その記録の evidence は新しい入力へ自動で付け替わる（承認は維持、`last_change.repointed` に記録ID）。
- 承認には実際の確認者と理由、要件・仕様の acceptance を記録する。open_issues を勝手に解決しない。修正・廃止した項目を参照する項目（依存元）は推移的に再確認待ちになる。`git diff -- .arp/registry` で確認する。現在版を更新し、過去版はGitで管理する。生成物は `.arp/cache/registry/` にある。
