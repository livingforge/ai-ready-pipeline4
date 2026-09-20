"""テナント 4 件と文書の見本。

★ **法務部の上限を 100,000 で入れてある** ——``資料/利用申請台帳.xlsx`` の
  値に合わせた。``tenant.quota.DEFAULT_QUOTA``（300,000）とは違う。
  実運用では 2026/06 に 300,000 へ直されているので、**この見本は台帳側**
  である（README の仕込み C1）。
"""

from __future__ import annotations

from datetime import date
from decimal import Decimal

from kotonoha.billing.models import Price
from kotonoha.ingest.models import SourceDocument

#: 利用申請台帳の 4 件。
TENANTS = [
    # (id, 名称, 部門, 区分, モデル, 上限, 原価センタ, 申請日)
    ("cs-support", "保守問合せ回答支援", "カスタマーサポート部",
     "10", "voyage-4", 500_000, "CC-4120", date(2025, 11, 12)),
    ("qa-defect", "不具合報告の類似検索", "品質保証部",
     "20", "voyage-4", 2_000_000, "CC-3310", date(2026, 1, 20)),
    ("legal-contract", "契約条項の検索", "法務部",
     "30", "voyage-4-nano", 100_000, "CC-1150", date(2026, 2, 3)),
    ("sales-proposal", "提案書の再利用", "営業本部",
     "20", "voyage-4", 300_000, "CC-2200", date(2026, 3, 16)),
]

#: 単価。外部 API の請求実績を按分して決めた値。
PRICES = [
    ("embed_chunk", Decimal("0.0180"), "voyage-4 の実績から按分"),
    ("search_call", Decimal("0.0400"), "計算資源と運用費の按分"),
    ("rerank_call", Decimal("0.1200"), "rerank-2.5 の実績から按分"),
    ("gpu_second", Decimal("0.0140"), "社内 GPU の減価償却と電力"),
]

#: 保守マニュアルの抜粋（カスタマーサポート部）。
MANUAL_DOCS = [
    SourceDocument(
        external_id="MAN-A2210-01",
        title="A-2210 型 サーボモータ 保守手順",
        content_type="text/markdown",
        metadata={"製品": "A-2210", "部署": "カスタマーサポート部", "年度": "2026"},
        content="""# A-2210 型 サーボモータ 保守手順

## 1. 定期点検

定期点検は 6 か月ごとに実施する。点検の前に必ず主電源を落とし、
残留電荷が抜けるまで 5 分待つこと。

## 2. 異音がするとき

軸受の劣化が疑われる。回転数を 500rpm まで落として異音の変化を見る。
回転数に比例して音が大きくなる場合は軸受を交換する。

## 3. 過熱するとき

冷却ファンの目詰まりをまず疑う。フィルタを外して圧縮空気で清掃する。
清掃後も 80 度を超える場合は制御基板の故障を疑い、部品交換を手配する。
""",
    ),
    SourceDocument(
        external_id="MAN-B4400-01",
        title="B-4400 型 減速機 取扱説明",
        content_type="text/markdown",
        metadata={"製品": "B-4400", "部署": "カスタマーサポート部", "年度": "2025"},
        content="""# B-4400 型 減速機 取扱説明

## 1. 潤滑油の交換

初回は稼働 500 時間で交換し、以後は 4,000 時間ごとに交換する。
指定油以外を使うと保証の対象外になる。

## 2. 異音がするとき

歯車の摩耗が疑われる。負荷を外した状態で手回しし、引っ掛かりを確かめる。

## 3. 油漏れ

シールの劣化である。稼働を止めてシールを交換する。
油量が規定を下回ったまま運転すると焼き付く。
""",
    ),
]

#: 不具合報告（品質保証部）。CSV で来る。
DEFECT_CSV = SourceDocument(
    external_id="QA-2026Q1",
    title="2026年度 第1四半期 不具合報告",
    content_type="text/csv",
    metadata={"部署": "品質保証部", "年度": "2026", "四半期": "Q1"},
    content=(
        "報告番号,製品,発生日,区分,内容,処置\n"
        "QA-26-0012,A-2210,2026-01-18,異音,"
        "低速回転時に軸受から断続的な異音,軸受を交換し再発なし\n"
        "QA-26-0031,A-2210,2026-02-05,過熱,"
        "連続運転 4 時間で 85 度に到達,冷却ファンのフィルタ清掃で改善\n"
        "QA-26-0044,B-4400,2026-02-22,油漏れ,"
        "出力軸シールから油の滲み,シール交換。ロット不良の疑いで調査中\n"
        "QA-26-0058,A-2210,2026-03-11,異音,"
        "高速回転時に金属音。軸受は正常,カップリングの緩みが原因\n"
    ),
)

#: 契約書の抜粋（法務部・極秘）。
CONTRACT_DOCS = [
    SourceDocument(
        external_id="CTR-2026-0112",
        title="部品供給基本契約書（ひな形）",
        content_type="text/markdown",
        metadata={"種別": "基本契約", "部署": "法務部"},
        content="""# 部品供給基本契約書

## 第5条（検査）

甲は乙から納入された物品について、納入後 10 営業日以内に検査を行い、
その結果を乙に通知する。期間内に通知がないときは合格とみなす。

## 第8条（瑕疵担保）

引渡し後 1 年以内に隠れた瑕疵が発見されたときは、乙は無償で修補し、
または代品を納入する。

## 第12条（秘密保持）

本契約により知り得た相手方の秘密情報を、事前の書面による承諾なく
第三者に開示してはならない。本条の効力は契約終了後 3 年間存続する。
""",
    ),
]


def install(services) -> dict:
    """テナント・API キー・単価を入れる。発行した鍵を返す。"""
    secrets: dict[str, str] = {}
    for tenant_id, name, dept, level, model, quota, cost, applied in TENANTS:
        services.tenants.register(
            tenant_id, name, dept, classification=level, cost_center=cost,
            embed_model=model, monthly_quota=quota, applied_at=applied)
        services.tenants.approve(tenant_id)
        issued = services.apikeys.issue(tenant_id, label="初期発行")
        secrets[tenant_id] = issued.secret

    for kind, unit, note in PRICES:
        services.prices.register(Price(
            price_kind=kind, valid_from=date(2026, 4, 1),
            valid_to=date(9999, 12, 31), unit_price=unit, note=note))

    return secrets
