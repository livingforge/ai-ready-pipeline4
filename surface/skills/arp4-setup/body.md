# ARP のソースからの導入

Windows／Linux でソースから ARP を導入・更新し、文書プロジェクトを初期化する。現在のAgentが実行し、専用の担当Agentの起動は不要。本体導入前はこの原稿をソースから直接読める。導入済みの文書操作・workflow再開には [arp4](../arp4/SKILL.md) を使う。

## 前提環境と取得

利用者が指定した配布元・版のソース ZIP または Git のタグ／コミットを使う。会話や指定された作業場所から導入対象を特定できない場合だけ確認する。ZIP は隣接する `.sha256` と照合し、新しいフォルダーへ展開する。`SOURCE.txt` の commit／dirty を確認し、dirty な配布物を正式版と扱わない。ソースと文書プロジェクトの場所を分ける。

OS・CPU、`rustup --version`、`cargo --version`、`git --version` を確認する。Rust がなければ https://rust-lang.org/tools/install/ の公式手順で導入する。rustup はユーザー単位で導入でき、管理者権限は不要。Windows は MSVC 用 C++ Build Tools と Windows SDK、Linux はディストリビューションに対応したリンカー／C コンパイラが必要。Linux のパッケージ管理コマンドを一律に仮定しない。履歴管理と採用後の文書差分には Git を使う。インストーラーが対話や再起動を要求した場合は、その状態を報告し、完了後に続ける。

Windows では Rust の導入前後に Build Tools の有無を確認する。`"${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe" -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath` が空、または vswhere がなければ未導入とみなす。Build Tools と Windows SDK の導入には管理者権限（UAC の昇格）が必要なため、Agent が昇格を試みたり、rustup-init の Visual Studio 自動導入を無断で選んだりしない。不足を報告し、利用者本人または端末の管理者に導入を依頼して待つ。依頼時は次のコマンドを示す。「C++ によるデスクトップ開発」ワークロードに Windows SDK が含まれる。

```text
winget install --id Microsoft.VisualStudio.2022.BuildTools --override "--passive --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
```

管理者権限を得られない場合は導入を進めず、その旨を報告する。導入後は新しいシェルで再確認してからビルドする。

ソースルートの `rust-toolchain.toml` と `Cargo.lock` を使い、導入のために変更しない。ツールチェーンと依存の取得にはネットワークが必要。利用目的の導入に Python、PowerShell 7、開発用 Git フックは不要。

## ビルドと初期化

ソースルートで実行する。この手順内の `docs/` と `surface/` のパスもソースルート基準である。

```text
cargo install --path crates/arp4-cli --locked
arp4 --version
arp4 doctor --format json
arp4 skills install --root <project> --agent <agent>
arp4 documents init --root <project>
```

`<project>` は文書プロジェクトの絶対パス。空白を含むパスはシェルに合わせて引用する。`<agent>` の選択肢は `arp4 skills install --help` で確認する。スキル・操作リファレンス・カスタムAgentをまとめて導入する。導入後はホストに設定を再読込させ、親がarp4スキルから担当Agentを呼び出す。対応形式と委任の仕組みは `docs/guides/custom-agents.md` を参照する。未対応ホストでは原稿を独立した担当セッションに読み込ませ、未対応のAgent名をCLIに渡さない。

導入後、Cargo のインストール先 `bin` がユーザーの PATH に登録されているか確認する。Windows は `[Environment]::GetEnvironmentVariable('Path','User')`、Linux はログインシェルの設定で確認する。未登録なら、Windows はユーザー環境変数の Path に追加し（管理者権限は不要）、Linux は `~/.cargo/env` を読み込む設定をシェルの起動ファイルに追加する。既存の PATH を上書きせず追記する。変更できない場合だけ実行ファイルを絶対パスで使う。既定は Windows の `%USERPROFILE%/.cargo/bin/arp4.exe`、Linux の `$HOME/.cargo/bin/arp4`。`CARGO_HOME` やインストール先を変更している場合は実際の場所を確認し、別バージョンを誤って実行しない。

各終了コードを確認し、失敗時は後続処理を止める。通常応答は JSON。`ok` と `error.code/message` を確認する。`--help` と `--version` はテキスト。`doctor` は実装範囲を示し、リンカー・Excel・Agent の環境診断を完了した証拠にはならない。

スキル導入と `documents init` は対象フォルダーがなければ作成する。スキル導入だけでは文書管理を初期化しない。初期化済みなら状態を確認して未完了操作だけ続ける。既存の設定や編集済みスキルを削除して再実行しない。標準の原本置き場は `docs/`、管理データは `.arp/`。

## 完了と更新

版、ソースの commit／dirty、実行ファイルの絶対パス、対象プロジェクト、実際に完了した確認と不足条件を報告する。PATH の変更は起動済みのターミナル・VS Code などのエディター・Agent ホストには反映されないため、`arp4` をコマンド名で使う前にそれらを再起動するよう利用者へ明示する。VS Code は統合ターミナルを開き直すだけでは反映されず、ウィンドウをすべて閉じて再起動する必要がある。再起動後に新しいシェルで `arp4 --version` を確認するよう案内する。Excel の画面描画は Windows とデスクトップ Excel が必要で、Linux では利用できない。Agent／モデル接続は別途設定する。

更新は指定版を別のソースフォルダーに取得して同じビルド手順で行う。別パッケージとの衝突に `--force` を自動適用しない。文書プロジェクトを保持し、必要に応じてスキル導入を再実行する。利用操作はソース内の `surface/skills/arp4/body.md`、詳細は `docs/guides/rust-preview.md` を参照する。
