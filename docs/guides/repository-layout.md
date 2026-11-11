# 導入先リポジトリのフォルダ管理

対象はARPを導入する開発リポジトリです。ARP本体のソース配置や配布ZIPの構成とは区別します。ARP自身の開発リポジトリも同じ設定を使います。

## 保存先と正本

```text
repository/
  src/・tests/                      既存コード・テスト
  docs/                             文書原本（sources。例: docs/xxx/yyy/仕様.xlsx）
    .gitattributes                  改行を変換させない（* -text）
  .arp/
    config.yml                      共有設定
    .gitignore
    .gitattributes                  改行を変換させない（* -text）
    documents/<文書ID>/             文書IDは原本のsourcesからの相対パス（例: xxx/yyy/仕様.xlsx/）
      document.yml                  原本の相対パス・ハッシュ（この文書の記録で唯一の原本ハッシュ）
      extraction.json               原本の抽出結果（documents extraction-read で読む。原本のパス・ハッシュは持たない）
      content/*.yml                 AIが参照・編集する内容
      layout.yml                    本文要素と原本位置の対応（documents elements-read で読む）
      mappings.yml                  構造操作・構造修正と、既定と異なる原本位置への対応
      formation.json・prompt.txt    成形時の判断記録
      review.json                   現在内容のレビュー
    changes/<文書ID>/               取込・再取込候補。文書ごとに1つで、採用時に削除
    registry/
      registry.json                 現在状態・ID・現在の変更理由
      records/*.json                要件・仕様・関連・受入条件
      evidence/*.json               現在項目・判断が参照する抽出入力
      archive/judgments.json        初回抽出時の除外・監査・判断
    cache/                          出力・閲覧用文書（Git対象外）
    work/                           実行状態・一時処理（Git対象外）
```

文書本文の正本は元のOffice等のファイルです。ARPはそのコピーを保存しません。文書IDはsourcesフォルダからの原本の相対パス（拡張子込み）で、`.arp/documents/` と `.arp/changes/` は原本のフォルダ構造をそのまま映します。フォルダ名やファイル名に識別用のハッシュは付けません。原本ハッシュは `document.yml` の中で変更検出に使います。`extraction.json`・`layout.yml`・本文・`mappings.yml` は原本ハッシュを持たないため（構造解釈の修正記録があればその対象の版だけを持つ）、内容を変えずに保存し直した原本を再importしても、変わるのは `document.yml` の原本ハッシュと成形記録（`formation.json`）の内容ハッシュの2行だけです。

原本の移動・リネームは、新しいパスの文書の追加と、旧パスの文書の削除です。`documents import <フォルダ>` は新しいパスを取り込み、原本が見つからない文書を `missing` として返します。`documents remove <文書IDまたはフォルダ>` はそれらを削除し、原本が残っている文書の削除は拒否します。旧文書の根拠・承認は引き継がず、新しい抽出結果からcaptureとレビューをやり直します。

Windowsで使えない名前（末尾の `.`・空白、`CON` 等の予約名）と、既存の文書と大文字小文字だけが異なるパスは取込時に拒否します。

パスの長さは、その clone の実際の場所で判定します。ARP自体は長さの制限なく読み書きします。制限するのは、WindowsでGit管理下にあり、Gitが長いパスを有効にしていない（`core.longpaths` が `true` でない）場合だけです。このとき、原本または記録（`.arp/documents/<文書ID>/` 以下）の絶対パスがファイル258文字・フォルダ247文字（UTF-16の単位数）を超える原本は取込を拒否し、シート・スライドの追加も `path_too_long` で拒否します。これはGit for Windows 2.45で測った、`git add` とチェックアウトが扱える長さです。超えたファイルは警告1行だけでコミットから漏れるか、取り出せません。Gitの管理外の `.arp/cache/` 等は判定しません。

判定は今の clone の場所によるため、より長いフォルダに clone する利用者は取り出せないことがあります。Windowsで共有するプロジェクトでは、各 clone で `git config core.longpaths true` を設定してください（Git for Windowsのインストール時に長いパスを有効にした場合は、システム設定で有効になっています）。Officeでの描画と数式の計算は書き出し候補を一時フォルダへコピーして開くため、プロジェクトの場所に左右されません。IRM・秘密度ラベルの解除は原本を `.arp/work/` にコピーしてOfficeで開くため、プロジェクトが深い場所にあると、Officeが開けるパスの長さを超えて失敗することがあります。

抽出結果と編集用YAMLは原本位置の検証と変更案の表現に用途を分けています。原本と独立した二つの正本として編集し続ける運用にはしません。
要件・仕様の解釈、除外理由、レビューは機械的に再生成できるとは限らないため、共有データとして保持します。

原本・コード・`.arp/` の共有データをGit管理します。過去版の全体コピー、原本スナップショット、独自の変更履歴配列は保存しません。現在の項目または判断が参照する抽出入力は根拠として必要なため保持します。過去状態の根拠はそのGitコミットで確認します。
ARPは自動コミットしません。`work/` の実行ログ・再開情報はローカル作業状態で、共有すべき判断の唯一の保存先にしないでください。

## 設定とパス

```yaml
schema_version: '1'
sources: docs
agent_read_format: toon
```

`arp4 documents init` が `.arp/config.yml` と内部の `.gitignore` を作成します。原本の配置は `documents init --sources specifications` のように変更できます。初期化で既存の原本、AGENTS.md、エディタ設定は変更しません。

HTMLのページが相対パスで指す画像ファイルは、取り込み元フォルダ（sources）の中にあるものだけを読みます。画像を別のフォルダに置くときは、そのフォルダを `picture_folders` に書きます。プロジェクト内のフォルダは `.arp/config.yml` に、プロジェクトの外のフォルダ（共有ドライブなど）はGit管理外の `.arp/local.yml` に書きます。`local.yml` はそのマシンだけの設定で、絶対パスや `../` も書けます。存在しないフォルダは無視するので、ほかのマシンではその画像がassetsに入りません。

```yaml
# .arp/config.yml
picture_folders:
- shared/images
```

```yaml
# .arp/local.yml（Git管理外）
picture_folders:
- D:/share/manual-images
```

読む範囲を広げると、そのフォルダの画像が取込時に `.arp/documents/` へ写され、Gitで共有されます。利用者が判断して設定し、Agentには書き換えさせません。設定を変えても原本が変わらない文書は再取込されないので、`documents import --force` で取り込み直します。`.arp/.gitignore` の `/local.yml` は `documents init` が書き、以前に作ったプロジェクトでも `local.yml` を初めて読むときに追加します。

ARPは原本と `.arp/` の記録をバイト列のハッシュで照合します。Gitが改行を変換すると、コミットやcloneの時点で原本のCRLFがLFに変わる、または記録ファイルがCRLFになり、原本の変更や記録の不整合として検出されます。Windowsの既定の `core.autocrlf=true` や、リポジトリの `* text=auto eol=lf` が該当します。このため `documents init` はsourcesフォルダと `.arp/` の `.gitattributes` に `* -text` を追記します。下位フォルダの `.gitattributes` はリポジトリルートの指定より優先されるため、コード等の改行設定は変わりません。既存の `.gitattributes` の内容は残し、行が無い場合だけ追記します。削除しないでください。

設定と保存済み原本参照はリポジトリルートからの相対パスです。絶対パスや `../` による外部の参照は設定に保存しません（マシンごとの `.arp/local.yml` は例外で、Gitで共有しません）。リポジトリを移動しても内部の参照が成立します。CLIに直接指定する実行ファイル等には絶対パスも使えます。

`documents`・`spec workflow`・`spec registry` の `--root` はリポジトリルートです。省略時は `.arp/config.yml` を親へ探索し、Gitルート（worktreeの `.git` ファイルも含む）を越えません。
importの相対パスはリポジトリ基準で、そこに存在しない場合は設定のsourcesを基準に探します。原本はsourcesの中に置きます。sourcesの外のファイルは取り込めません。prompt・outやspecの入力引数は起動位置が基準です。管理対象のパスはリンクや外部への脱出を拒否します。

workflowの作業先は `.arp/work/workflow/<run-id>/`、既定IDはcurrentです。出力は `.arp/cache/output/` に固定します。別worktreeとは作業状態を共有しません。

## 原本への変更反映

1. importで原本を抽出し、`.arp/changes/<文書ID>/` の候補のYAMLと対応表を確認・編集します。
2. record・adopt後は `.arp/documents/<文書ID>/` を使用します。採用した候補は削除します。採用時点では未レビューで、確認後にreviewを記録します。
3. 現在データをGitへコミットし、以後の文書差分は `documents diff --document <ID>` でHEADと比較します。候補のdiffは現在データとの比較です。
4. YAMLの変更をreviewし、`documents export <ID>` で反映計画を確認します。`--out` で出力し、出力を開いてレイアウトを確認したら `documents confirm-export <ID>` で記録します。
5. `documents apply <ID>` で確認した候補を元ファイルへ反映し（未確認なら拒否）、再抽出候補を作ります。候補を確認してrecord・adoptします。出力が原本と同じなら原本を書き換えず候補も作らず、`unchanged` を返します。applyだけでは再抽出内容をレビュー済みにしません。

原本ハッシュが変わっていたら古い対応で書き戻しません。未対応・曖昧な対応は未反映として扱います。原本を変えずに出力する場合は `export --out <repo>/.arp/cache/export/<新規名>` を使います。
要件・仕様の現在版は `spec registry apply --change <JSON>` で更新し、Gitへコミットします。生成した閲覧文書は `spec registry render` で `.arp/cache/registry/` に出力します。

## 契約の版

製品は `1.0.0-alpha.1` です。各保存形式の版は契約の `schema_version` が正本で、本文（`content`）は7、`layout.yml`（`document-layout`）は14、抽出結果（`extraction`）は4、構造解釈の修正記録は2、ほかの保存形式・入力・モデル・workflow状態・Agent向け契約は1です。一覧は [文書保存形式の契約](../reference/document-contracts.md) を参照してください。共有設定は `.arp/config.yml` を使用します。semantic finalizeの直接採用は `--registry --project <名称>` を使い、実行ディレクトリから探索したプロジェクトの `.arp/registry/` に保存します。

## 対応範囲

Office/PDFの変換・書き戻しの形式上の制約は [文書操作と対応範囲](documents.md) を参照してください。テキスト・Markdown・CSV・TSVは原本を直接編集して再取込します。共通処理用の確認用YAMLは派生データであり、本文の編集や書き戻しには使いません。[文書操作](documents.md)を参照してください。
registryの変更からOfficeへの変更案を自動生成する機能、コード・テストとの自動対応は含みません。文書YAMLで対応変更を作り、review・applyで反映します。
