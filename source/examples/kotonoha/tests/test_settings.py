"""設定。

★ **この検証は「設計文書と食い違っている値」を固定するためにある。**

``rate_limit_rps`` は 100 だが ``docs/runbook/rate-limit.md`` は 60。
``quantize`` は真だが ADR-003 は量子化に触れていない。
``rerank_enabled`` は真だが ADR-005 は「第2次リリース」。

いずれも**実装の側が正しいかどうかは分からない**。どちらへ寄せるかを
決めるのは人であって、この検証はいまの状態を記録しているだけである
（README の仕込み A3・B1・B2）。
"""

from __future__ import annotations

import os

from kotonoha.common.settings import SETTINGS, Settings


def test_レート制限は100rps():
    """★ runbook は 60 rps。**Ingress 側の設定はこのリポジトリに無い。**"""
    assert SETTINGS.rate_limit_rps == 100


def test_量子化が既定で入っている():
    """★ ADR-003 は「1024 次元の float」としか書いていない。"""
    assert SETTINGS.quantize is True


def test_リランクが既定で入っている():
    """★ ADR-005 は「第2次リリース（2026年10月）」と書いている。"""
    assert SETTINGS.rerank_enabled is True
    assert SETTINGS.rerank_model == "rerank-2.5"


def test_エンベディングの一度の上限は128件():
    assert SETTINGS.max_embed_batch == 128


def test_検索の件数の上限は100():
    assert SETTINGS.max_top_k == 100


def test_1ジョブの文書数の上限は1000():
    assert SETTINGS.max_documents_per_job == 1_000


def test_キャッシュの保持は30日():
    assert SETTINGS.cache_ttl_days == 30


def test_監査ログの保持は5年():
    """機密区分 20（社外秘）の保持期間に合わせた。"""
    assert SETTINGS.audit_retention_years == 5


def test_極秘のモデルはvoyage4nano():
    assert SETTINGS.internal_model == "voyage-4-nano"


def test_環境変数で上書きできる():
    os.environ["KOTONOHA_RATE_LIMIT_RPS"] = "60"
    try:
        assert Settings.from_env().rate_limit_rps == 60
    finally:
        del os.environ["KOTONOHA_RATE_LIMIT_RPS"]


def test_真偽値の環境変数を読める():
    os.environ["KOTONOHA_QUANTIZE"] = "false"
    try:
        assert Settings.from_env().quantize is False
    finally:
        del os.environ["KOTONOHA_QUANTIZE"]


def test_数でない値は既定にする():
    """**不正な設定で起動を止めない。** 起動できないほうが困る。"""
    os.environ["KOTONOHA_RATE_LIMIT_RPS"] = "たくさん"
    try:
        assert Settings.from_env().rate_limit_rps == 100
    finally:
        del os.environ["KOTONOHA_RATE_LIMIT_RPS"]
