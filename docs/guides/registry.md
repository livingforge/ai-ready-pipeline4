# 要件・仕様を正本として管理する

`spec registry` は抽出モデルを継続保守するための正本へ登録する。
正本は項目ごとの JSON と現在状態のマニフェストであり、生成 Markdown は閲覧用である。
原資料の証拠を残しつつ、原資料にない新しい要件や設計判断も変更理由とともに直接追加できる。

## 正本・根拠・閲覧用文書

```text
.arp/registry/
  registry.json                 現在状態、項目ハッシュ、発行済ID、現在の変更理由
  records/REQ-000001.json        1項目1ファイル
  records/SPEC-000001.json
  evidence/<hash>.json          現在項目・判断が参照する抽出入力
  archive/judgments.json        抽出時の除外、AI監査、判断
.arp/cache/registry/
  README.md
  modules/<module>.md
  evidence/<module>.md
  appendices/traceability.md
  appendices/change.json        現在の変更理由
```

先に `arp4 documents init` で `.arp/config.yml` を作成する。`--root` はリポジトリルートを指し、省略時はGit境界内で探索する。
`.arp/registry/` と原本をGit管理し、過去版はGitで確認する。生成Markdownはキャッシュである。
原本バイナリや元モデル・ID台帳の全体コピーはregistryに保存しない。現在の項目が必要とする根拠と判断を保持する。
ハッシュ照合は記録の整合検査であり、署名・アクセス制御ではない。actor は申告した担当者名であり、本人認証しない。
承認権限の制御はリポジトリのレビュー・アクセス権等で行う。

## 初回登録

検証済み・採番済みの抽出モデルとその `.ids.json` が必要。
`catalog.json` では全項目の分類と機能、対応要件、関連項目、分類理由を明示する。

```json
{
  "actor": "実際の分類担当者",
  "reason": "実績と設計上の制約を区別して初回登録",
  "modules": {"backup": "バックアップ"},
  "open_issues": ["原資料の保存期間の矛盾について確認が必要"],
  "open_issue_documents": {"原資料の保存期間の矛盾について確認が必要": []},
  "entries": {
    "SPEC-000001": {
      "category": "specification", "module": "backup",
      "requirements": [], "related": [],
      "reason": "原文は具体的な保管仕様。対応要件は資料に未記載"
    }
  }
}
```

分類は requirement（要件）／specification（仕様）／observation（実績）／estimate（試算）／reference（参考）。
元の抽出時の kind に従う必要はない。既存IDは分類変更時も維持するため、IDの接頭辞だけで現在の分類を判断しない。
`requirements` は仕様から要件への参照で、`related` は当該項目が依存・参照する項目への方向付きリンクである。
関連が明示されない項目への変更影響は自動推論しない。対応要件を創作してリンクを埋めない。

```powershell
arp4 spec registry init --input input-all.json --model model.json --catalog catalog.json --project kotonoha
arp4 spec registry check
arp4 spec registry render
```

初回登録で拒否理由を確認したい場合は、書き込みを行わない `preflight` を先に実行する。

```powershell
arp4 spec registry preflight --input input-all.json --model model.json --catalog catalog.json --project kotonoha
```

`preflight` はモデル検証、ID台帳、分類キー、分類メタデータ、正本importの各ゲートを個別に返す。`registry init` は最初の不整合で停止するため、既存の自動処理や運用確認では `preflight` の結果を保存する。

保存先は `.arp/registry/` に固定する。初期状態は全件 proposed（提案）。AI監査や元の ready を承認に転用しない。
既存の verification は原文照合手順として保持し、実装受入条件 acceptance は別に記述する。
未解決事項は `open_issues` に残し、各事項の対象文書を `open_issue_documents` に必ず指定する。全体共通なら空配列、事項がなければ空オブジェクトを指定する。これらが残る間は承認操作を拒否する。

## 新しい要件・仕様を追加する

`check` が返す `summary.base_hash` を変更ファイルに記す。新規IDは `new:<英小文字のキー>` とし、CLI が永続IDを発行する。
原資料の引用がない新しい設計判断は `evidence: []` とし、rationale と変更理由を必須とする。
それを原文から抽出した事実とは表示しない。新たに追加する根拠があれば、`apply --input <capture.json>` で保管し、
evidence に `{"snapshot":"入力ハッシュ","span":{"source":"出典ID","start":0,"end":3,"quote":"引用"}}` を指定する。
condition/value の形式は [抽出モデル](../reference/specifications.md) と共通。原文引用のある項目は数量原文照合も継続する。

```json
{
  "base_hash": "checkで取得した64桁のハッシュ",
  "actor": "設計担当者", "reason": "復元確認を新たな運用要件として提案",
  "entries": [{
    "id": "new:restore", "name": "復元可能性の確認",
    "category": "requirement", "module": "backup", "status": "proposed",
    "subject": "バックアップ", "property": "復元確認",
    "condition": {"basis": "unspecified"},
    "value": {"kind": "text", "text": "リストア試験"},
    "statement": "バックアップからデータを復元できることを確認する。",
    "verification": "復元試験の結果を確認する。",
    "acceptance": ["復元したデータが保存時のデータと一致すること"],
    "requirements": [], "related": [], "evidence": [],
    "rationale": "新しい設計判断。実施方法と受入条件をレビュー対象とする。",
    "approval": null
  }]
}
```

同時に新規仕様を追加する場合、`requirements: ["new:restore"]` の参照も採番後のIDへ置き換わる。
新規は REQ / SPEC / OBS / EST / REF の系列で採番し、廃止したIDを再利用しない。

```powershell
arp4 spec registry apply --change change.json
git diff -- .arp/registry
arp4 spec registry render
```

## 修正・承認・廃止

修正は対象 records ファイルを entries へコピーし、修正内容と `status: proposed`, `approval: null` を設定する。
一部フィールドだけのパッチではなく項目全体を指定する。既存IDを保つ。
修正・廃止した項目への依存先を requirements/related から推移的に列挙し、承認済みの影響項目を proposed に戻す。
項目の本文を自動修正したり、古い承認を自動継承したりはしない。

承認は別の変更で `approve: [{"id":"REQ-000005","reason":"実際のレビュー結果"}]` と指定する。
要件・仕様には非空の acceptance が必要。リンク先も承認済みである必要がある（同じ変更での同時承認は可能）。
仮定の条件が残る項目は承認できない。受入条件の妥当性やレビューの実施自体は機械では証明しない。
対象・属性・条件を正規化した同一論点の値の不一致は conflict_candidates として表示し、矛盾する項目の同時承認を拒否する。
意味上の同義語や条件の包含関係まで推論するものではない。
未解決事項は `resolve_issues: {"既存の課題文":"確認者と確認結果を含む解決理由"}` で解決を記録する。
`add_issues` で新たな課題を登録すると、既存の承認済み項目を再確認待ちへ戻す。

廃止は `retire: ["REQ-000005"]`。廃止項目は削除せず残し、同じIDを復活させない。
分割・統合は新規項目と旧項目の廃止を同じ変更に含め、理由を記録する。
旧項目を参照する仕様のリンク変更も明示する。廃止項目へ依存する仕様は再承認できない。

## 運用上の範囲

- 現在版を同じ場所で更新する。過去版のディレクトリを増やさず、変更をGitへコミットする。ARPは自動コミットしない。
- 一時ディレクトリで新しい現在版を作成し、切り替える。通常完了時は旧コピーを削除する。
- `base_hash` と保存時の再照合、更新ロックで古い基準からの変更を拒否する。管理外ファイルを含むregistryは置き換えない。
- `render` は固定キャッシュを再生成する。生成物が手編集されている場合や管理外ファイルがある場合は拒否する。
- 参照されなくなった抽出入力は現在版から削除する。過去の根拠は過去のGitコミットで確認する。
- 原本の変更は自動で要件・仕様に反映しない。再取込し、影響項目の根拠・値・関係を明示的に更新する。
- registryの変更をOfficeへ自動変換する機能はない。対応文書のYAMLに変更案を作り、review後の `documents apply` で原本へ反映する。
- 要件・仕様間の関連と分類、状態は管理する。コード・テストとの自動対応や設計図の更新は対象外。
