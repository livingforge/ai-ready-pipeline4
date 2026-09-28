# ARP4

ARP4 は、文書の内容を確認・更新するためのコマンドラインツールです。Excel・Word・PowerPoint・PDF を取り込み、変更箇所の確認、レビュー、元と同じ形式への書き戻しができます。また、原文を根拠に要件や仕様を作成・更新できます。UTF-8 の TXT・Markdown・CSV・TSV も取り込めますが、これらの本文編集と書き戻しには対応していません。

この配布物は `1.0.0-alpha.1` のソースコードです。Windows または Linux でビルドして使います。文書形式ごとに扱える内容と操作は異なります。詳しくは [文書操作](docs/guides/documents.md) を参照してください。Excel の指定範囲を画像にして確認する機能には、Windows とデスクトップ版 Excel が必要です。

## はじめる

ソース ZIP を展開するか、指定された Git のタグまたはコミットを取得します。Rust と OS に必要なビルドツールを用意し、ソースコードの最上位フォルダーで次を実行します。

```text
cargo install --path crates/arp4-cli --locked
arp4 doctor
arp4 skills install --root <project>
arp4 documents init --root <project>
```

`<project>` は、文書を管理するプロジェクトのパスに置き換えます。`skills install` は、標準では Codex・Claude Code・GitHub Copilot 向けのスキルをまとめて導入します。いずれか一つだけ導入する場合は、`--agent codex`、`--agent claude`、`--agent github` のいずれかを付けます。

AI エージェントに導入を任せる場合は、[セットアップ手順](surface/skills/arp4-setup/body.md) を渡してください。必要なツールや PATH の設定、更新方法もそこに記載しています。導入後の使い方は [基本操作ガイド](docs/guides/rust-preview.md) を参照してください。

ソース ZIP には、ビルドと利用に必要なファイルだけが入っています。同梱ファイルの一覧は `build/source-files.txt`、梱包時のコミットと未コミットの変更の有無は `SOURCE.txt`、各ファイルの SHA-256 値は `SHA256SUMS` で確認できます。開発・検証用の資料を読む場合は、`SOURCE.txt` に記されたコミットのリポジトリを参照してください。

## やりたいことから探す

| 目的 | 文書 |
|---|---|
| 元の文書と ARP の管理データをどこに置くか決める | [導入先リポジトリのフォルダ管理](docs/guides/repository-layout.md) |
| 文書を取り込み、変更を確認する | [文書操作](docs/guides/documents.md) |
| Excel の表や図を原本と照合して整理する | [構造解釈](docs/guides/document-structure.md) |
| 要件・仕様を作成・レビューし、中断した作業を再開する | [進行管理ワークフロー](docs/guides/semantic-workflow.md) |
| AI 実行先を設定する | [AI 実行先との接続](docs/guides/semantic-runners.md) |
| カスタム Agent に作業を任せる | [カスタム Agent による委任](docs/guides/custom-agents.md) |
| 要件・仕様を更新し続ける | [正本管理](docs/guides/registry.md) |
| CLI のコマンドと応答を確認する | [コマンド一覧](docs/reference/commands.md)・[CLI 応答仕様](docs/reference/cli.md) |
| 対応機能を確認する | [機能一覧](docs/reference/capabilities.md) |
