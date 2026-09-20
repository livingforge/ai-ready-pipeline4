# 検証記録（2026-09-20、4.0.0-alpha.3）

- `cargo test --workspace --locked`: 18 件成功（契約 3、Excel 3、導入 12）。Excel 試験は文字列・数値・真偽値・null、数式キャッシュ無効化、未知の ZIP 部品とコメントの保持、型違い・数式・結合セル・外部参照・署名の拒否を確認。
- `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --locked -- -D warnings`: 成功。
- `cargo run --locked --example sync_skills -- --check`: 4 スキルと埋め込み原稿が一致。
- `build/smoke_rust.ps1`: 最終 ZIP を展開、PATH を空にして導入→取り込み→成形記録→採用→編集→差分→レビュー→書き戻しに成功。.NET XML による独立した読み戻し、未記録の採用・未レビュー出力・既存出力・変更済み管理原本の拒否を確認。
- `build/test_deployment.ps1`: 初回・反復展開、Git メタデータの保持、既存ファイル衝突と不正チェックサムの拒否に成功。
- `C:/arp4-publish` に alpha.3 を展開し、全管理ファイルの SHA-256 と実行バージョンを確認。GitHub への push・公開は実施していません。

空の Windows VM、Agent 上での導入、Excel 実機の受入試験は未実施です。
