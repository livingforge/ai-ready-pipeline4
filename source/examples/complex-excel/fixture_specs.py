"""Shared identities for the three complex Excel fixtures."""
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SPECS = [
    ("screen-order", "01_受注画面詳細設計書.xlsx", "受注登録画面 SCR-001", "UI-ONLY-731：与信超過時は承認申請のみ可能"),
    ("inventory-if", "02_在庫引当外部連携設計書.xlsx", "在庫引当連携 IF-INV-001", "IF-ONLY-842：タイムアウト後は照会してから再送"),
    ("billing-batch", "03_請求締めバッチ設計書.xlsx", "請求締め処理 BAT-BIL-001", "BAT-ONLY-953：再開時は確定済み請求を再作成しない"),
]
