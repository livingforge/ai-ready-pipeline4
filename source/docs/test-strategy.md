# 検証方針

Rust の契約・インストール試験を cargo test で実行します。build/smoke_rust.ps1 は配布 ZIP を展開し、PATH を空にしてスキル導入から Excel の書き戻しを検証します。Excel は .NET で作成し、出力も XML として独立に読み戻します。

空の Windows VM と Excel 実機の受入試験は別途必要です。
