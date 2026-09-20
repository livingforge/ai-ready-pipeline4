# ARP4

Rust 製の Excel 文書管理 CLI です。Windows x64 用 ZIP を展開し、Python なしでスキル導入、Excel 取り込み、YAML 編集、差分確認、レビュー、既存セルへの書き戻しを行えます。

現在は 4.0.0-alpha.3 の試験版です。利用者は作成済みの `arp4.exe` を使います。開発時のみ Rust と MSVC Build Tools が必要です。

標準配布には監査用のソース・テスト・テストデータとビルド記録を含めます。依存ソースを含むオフライン再ビルド用 ZIP も別途作成できます。[監査・再検証手順](docs/audit-testing.md)を参照してください。

```powershell
.\arp4.exe doctor
.\arp4.exe skills install --root C:/my-project --agent github
.\arp4.exe documents init --root C:/my-project
```

プロジェクトのフォルダーを先に作成してください。詳しい手順と制限は [利用・開発ガイド](docs/rust-preview.md)、検証結果は [verification](docs/verification.md) を参照してください。

取り込みは `.xlsx` / `.xlsm` のセル値・数式原文・結合範囲、書き戻しは既存の通常セル値が対象です。図形・OCR・構造変更・他形式・仕様書生成は未対応です。

Python 版の本体・ビルド・専用テストは削除しました。旧実装は Git 履歴に残っています。`examples/` の別アプリのコードや資料生成用 Python はサンプル資産であり、ARP の利用・ビルド・検証には不要です。

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo run --locked --example sync_skills -- --check
./build/package_rust.ps1
./build/smoke_rust.ps1 -Zip target/preview-distribution/arp4-v4.0.0-alpha.3-windows-x64-preview.zip
```

スキル原稿は `surface/skills/` にあります。変更後は `cargo run --locked --example sync_skills` でリポジトリ内のスキルを更新します。本体にも同じ内容が埋め込まれます。
