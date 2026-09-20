"""機密区分。**極秘を外部 API へ出さないことがこの資材の芯である。**

情報セキュリティ点検表（2026/03）の指摘に対する回答が、この検証にあたる。
"""

from __future__ import annotations

import pytest

from kotonoha.common.errors import ClassificationViolation, InvalidInput
from kotonoha.tenant import classification as cls


def test_3つの区分が定義されている():
    assert cls.all_codes() == ["10", "20", "30"]


def test_知らない区分は弾かれる():
    with pytest.raises(InvalidInput):
        cls.of("40")


def test_極秘だけが外部への送出を許さない():
    assert cls.allows_external("10")
    assert cls.allows_external("20")
    assert not cls.allows_external("30")


def test_強さは数値の大小と一致する():
    assert cls.of("10").rank < cls.of("20").rank < cls.of("30").rank


def test_保持期間():
    assert cls.of("10").retention_years is None      # 無期限
    assert cls.of("20").retention_years == 5
    assert cls.of("30").retention_years == 3


def test_区分を下げられない():
    with pytest.raises(ClassificationViolation):
        cls.ensure_not_lowered("30", "10")


def test_区分を上げるのは通る():
    assert cls.ensure_not_lowered("10", "30") == "30"


def test_同じ区分は通る():
    assert cls.ensure_not_lowered("20", "20") == "20"


def test_指定が無ければ継承がそのまま返る():
    assert cls.ensure_not_lowered("20", None) == "20"
    assert cls.ensure_not_lowered("20", "") == "20"


def test_極秘は外部経路を許さない():
    with pytest.raises(ClassificationViolation):
        cls.ensure_route_allowed("30", "external")


def test_極秘でも社内経路は通る():
    cls.ensure_route_allowed("30", "internal")       # 例外が出ないこと


def test_一般は外部経路が通る():
    cls.ensure_route_allowed("10", "external")
