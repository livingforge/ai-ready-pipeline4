# 配布ソースの監査と検証

標準 ZIP の source/ は実行ファイルをビルドしたソースのコピーです。本体、テスト、テストデータ、スキル原稿、契約、サンプル、ビルドスクリプトを含みます。audit/release.json に元のコミットと作業ツリーの変更状態、ビルド条件を記録します。未コミット変更がある場合、コミットだけでは配布ソースを再現できません。source/ と SHA256SUMS を確認してください。

## ハッシュの確認

配布フォルダーで以下を実行します。ハッシュ一覧自体は自己参照のため対象外です。ハッシュは改変検出用で、発行元の署名ではありません。

```powershell
./source/build/verify_audit.ps1 -Distribution .
```

## テストの実行

Windows x64、rust-toolchain.toml 指定の Rust、MSVC Build Tools と Windows SDK が必要です。実行ファイルの利用だけならこれらは不要です。

source/ に移動して実行します。通常は依存クレート取得に通信が必要です。

```powershell
cargo test --workspace --locked
cargo run --locked --example sync_skills -- --check
cargo build --release --locked --target x86_64-pc-windows-msvc
```

| 対象 | 確認する内容・データ |
|---|---|
| crates/arp4-cli/tests/contracts.rs | YAML/JSON の拒否条件、数値・Unicode、シート名・セル範囲 |
| crates/arp4-cli/tests/installation.rs | 埋め込みスキル導入、既存編集の保護、ロック、失敗時の復元 |
| crates/arp4-cli/tests/excel.rs | コード内で合成する Excel ZIP を用いた書き戻し、部品保持、不正操作の拒否 |
| build/smoke_rust.ps1 | .NET で合成する Excel を用いた CLI 操作一式。PATH を空にして実行 |
| build/test_deployment.ps1 | ZIP の展開、衝突時の保護、チェックサム不一致の拒否 |
| tests/dataset/ | 文書の検体定義。現在の cargo test では実行しない |
| examples/ | 文書・別アプリ・生成コードの見本。現在の Rust テストの合格範囲には含めない |

Rust と PowerShell の上記テストはコード内の合成データを使います。examples/ の資料の来歴・配布可否は、各 README と資料・サンプルコードを確認し、機密情報や第三者の権利がないことをリリース担当者が確認してください。ソースのライセンスは source/LICENSE、依存クレートの表示は配布ルートの licenses/ と THIRD-PARTY-NOTICES.md を参照してください。

## オフライン再ビルド用の追加 ZIP

配布担当者が、標準 ZIP を展開したフォルダーを指定して生成します。この準備段階では通信を使用できます。

```powershell
./build/package_offline.ps1 -Distribution C:/path/to/distribution -OutputDirectory C:/path/to/output
```

追加 ZIP を標準配布フォルダーへ展開すると offline/ ができます。まず標準配布のハッシュを検証し、offline/ 配下でも verify_audit.ps1 を実行して追加資材を検証してください。offline/release.json に対応する標準配布の audit/release.json の SHA-256 を記録しています。

source/ に移動して以下を実行します。vendor 設定は既存の CRT 静的リンク設定に追加されます。

```powershell
cargo --config ../offline/vendor-config.toml test --workspace --frozen
cargo --config ../offline/vendor-config.toml build --release --frozen --target x86_64-pc-windows-msvc
```

追加 ZIP は依存ソースを提供します。Rust、MSVC、Windows SDK、Agent、Excel のインストーラーは含みません。ツールチェーンは事前導入が必要です。--frozen による依存取得の禁止と、OS 自体のネットワーク遮断試験は別です。空の Windows、Excel/Agent 実機、バイト単位の再現可能ビルドは未検証です。

配布担当者は `build/test_offline.ps1 -Zip <標準ZIP> -OfflineZip <追加ZIP>` で、ZIP の展開と対応関係の検証、空の Cargo キャッシュを使った `--frozen` のテスト・release ビルドを実行できます。
