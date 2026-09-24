# ARP4

Rust製の文書管理CLIです。Excel・Word・PowerPoint・PDFの取り込み、編集、差分確認、レビュー、同形式への書き戻しと、原文を根拠にした要件・仕様の生成・継続保守を支援します。UTF-8のTXT・Markdown・CSV・TSVも抽出元として利用できます。

`1.0.0-alpha.1` のソース配布です。Windows／Linux で各自ビルドします。形式ごとの制限は [文書操作](docs/guides/documents.md) を参照してください。Excel の画面描画には Windows とデスクトップ Excel が必要です。

## はじめる

ソース ZIP を展開するか、指定タグ／コミットを取得します。Rust と OS のビルドツールを準備し、ソースルートで実行します。

```text
cargo install --path crates/arp4-cli --locked
arp4 doctor
arp4 skills install --root <project> --agent github
arp4 documents init --root <project>
```

`<project>` は文書プロジェクトのパスに置き換えます。カスタム Agent には [導入原稿](surface/skills/arp4-setup/body.md) を読ませてセットアップを依頼できます。前提ツール、PATH、更新方法もこの原稿を参照してください。

ソース ZIP はビルドと利用に必要なファイルだけを含みます。正確な一覧は `build/source-files.txt`、出自は `SOURCE.txt`、ハッシュは `SHA256SUMS` にあります。以下の開発・監査・文書一覧へのリンクはリポジトリ向けです。

対象フォルダーがなければ作成します。`skills install --agent none` は作成しません。導入後の操作は [導入ガイド](docs/guides/rust-preview.md) を参照してください。

## 目的から読む

| 目的 | 文書 |
|---|---|
| 原本とARPデータの配置を決める | [リポジトリ内の管理方針](docs/guides/repository-layout.md) |
| 文書を取り込み、変更を確認する | [文書操作](docs/guides/documents.md) |
| 要件・仕様を抽出し、レビュー・再開する | [進行管理ワークフロー](docs/guides/semantic-workflow.md) |
| カスタムAgentへ委任し、担当を再利用する | [カスタムAgentによる委任](docs/guides/custom-agents.md) |
| 要件・仕様を継続保守する | [正本管理](docs/guides/registry.md) |
| CLI応答を扱う | [CLI応答仕様](docs/reference/cli.md) |
| 実装を変更し、検証・配布する | [Rust実装の構成](docs/development/rust-architecture.md)・[開発手順](docs/development/development.md) |
| 配布ソースを監査・再検証する | [監査手順](docs/development/audit-testing.md) |

仕様、検証記録、今後の設計案は [文書一覧](docs/index.md) から辿れます。
