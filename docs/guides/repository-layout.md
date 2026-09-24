# 導入先リポジトリのフォルダ管理

対象はARPを導入する開発リポジトリです。ARP本体のソース配置や配布ZIPの構成とは区別します。ARP自身の開発リポジトリも同じ設定を使います。

## 保存先と正本

```text
repository/
  src/・tests/                      既存コード・テスト
  docs/                             文書原本（既定の探索先）
  .arp/
    config.yml                      共有設定
    .gitignore
    documents/<文書ID>/
      document.yml                  原本の相対パス・ハッシュ
      extraction.json               原本の抽出結果
      content/*.yml                 AIが参照・編集する内容
      mappings.yml                  原本位置への対応
      formation.json・prompt.txt    成形時の判断記録
      review.json                   現在内容のレビュー
    changes/<文書ID>/<候補ID>/       取込・再取込候補（採用時に削除）
    registry/
      registry.json                 現在状態・ID・現在の変更理由
      records/*.json                要件・仕様・関連・受入条件
      evidence/*.json               現在項目・判断が参照する抽出入力
      archive/judgments.json        初回抽出時の除外・監査・判断
    cache/                          出力・閲覧用文書（Git対象外）
    work/                           実行状態・一時処理（Git対象外）
```

文書本文の正本は元のOffice等のファイルです。ARPはそのコピーを保存しません。文書IDはファイル名から独立し、同じIDで移動後の原本を再取込できます。
抽出結果と編集用YAMLは原本位置の検証と変更案の表現に用途を分けています。原本と独立した二つの正本として編集し続ける運用にはしません。
要件・仕様の解釈、除外理由、レビューは機械的に再生成できるとは限らないため、共有データとして保持します。

原本・コード・`.arp/` の共有データをGit管理します。過去版の全体コピー、原本スナップショット、独自の変更履歴配列は保存しません。現在の項目または判断が参照する抽出入力は根拠として必要なため保持します。過去状態の根拠はそのGitコミットで確認します。
ARPは自動コミットしません。`work/` の実行ログ・再開情報はローカル作業状態で、共有すべき判断の唯一の保存先にしないでください。

## 設定とパス

```yaml
schema_version: '1'
sources: docs
```

`arp4 documents init` が `.arp/config.yml` と内部の `.gitignore` を作成します。原本の配置は `documents init --sources specifications` のように変更できます。初期化で既存のdocs、AGENTS.md、エディタ設定は変更しません。

設定と保存済み原本参照はリポジトリルートからの相対パスです。絶対パスや `../` による外部の参照は設定に保存しません。リポジトリを移動しても内部の参照が成立します。CLIに直接指定する実行ファイル等には絶対パスも使えます。

`documents`・`spec workflow`・`spec registry` の `--root` はリポジトリルートです。省略時は `.arp/config.yml` を親へ探索し、Gitルート（worktreeの `.git` ファイルも含む）を越えません。
importの相対パスはリポジトリ基準で、そこに存在しない場合は設定のsourcesを基準に探します。prompt・outやspecの入力引数は起動位置が基準です。管理対象のパスはリンクや外部への脱出を拒否します。

workflowの作業先は `.arp/work/workflow/<run-id>/`、既定IDはcurrentです。出力は `.arp/cache/output/` に固定します。別worktreeとは作業状態を共有しません。

## 原本への変更反映

1. importで原本を抽出し、候補のYAMLと対応表を確認・編集します。
2. record・adopt後は `.arp/documents/<文書ID>/` を使用します。採用した候補は削除します。
3. 現在データをGitへコミットし、以後の文書差分は `documents diff --document <ID>` でHEADと比較します。候補のdiffは現在データとの比較です。
4. YAMLの変更をreviewし、`documents export <ID>` で反映計画を確認します。
5. `documents apply <ID>` で元ファイルへ反映し、再抽出候補を作ります。候補を確認してrecord・adoptします。applyだけでは再抽出内容をレビュー済みにしません。

原本ハッシュが変わっていたら古い対応で書き戻しません。未対応・曖昧な対応は未反映として扱います。原本を変えずに出力する場合は `export --out <repo>/.arp/cache/export/<新規名>` を使います。
要件・仕様の現在版は `spec registry apply --change <JSON>` で更新し、Gitへコミットします。生成した閲覧文書は `spec registry render` で `.arp/cache/registry/` に出力します。

## 契約と確認範囲

製品は `1.0.0-alpha.1`、保存形式・入力・モデル・workflow状態・Agent向け契約はv1です。共有設定は `.arp/config.yml` を使用します。semantic finalizeの直接採用は `--registry --project <名称>` を使います。

README、docs（AGENT_BRIEFを含む）、サンプル、buildの手順・スクリプト、スキル原稿・同期先・配布埋め込みを実装と照合します。原本・レビュー・履歴の説明と実行例も確認します。

## 対応範囲

Office/PDFの変換・書き戻しの形式上の制約は [文書操作と対応範囲](documents.md) を参照してください。テキスト・Markdown・CSV・TSVは原本を直接編集して再取込します。共通処理用の確認用YAMLは派生データであり、本文の編集や書き戻しには使いません。[文書操作](documents.md)を参照してください。
registryの変更からOfficeへの変更案を自動生成する機能、コード・テストとの自動対応は含みません。文書YAMLで対応変更を作り、review・applyで反映します。
