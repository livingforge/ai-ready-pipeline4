"""メタデータの絞り込み。

**機密区分の絞り込みは検索では起きない** —— コレクションが区分を持ち、
テナントは自分のコレクションしか引けないので、区分をまたぐ検索は
そもそも成立しない。ここが担うのはメタデータだけ。

メタデータの鍵の表記ゆれ（「部署」と「部門」）は基盤側で正規化しない ——
意味の判断になるためである。
"""

from __future__ import annotations

from dataclasses import dataclass, field

import pytest

from kotonoha.common.errors import InvalidInput
from kotonoha.search.filters import MAX_FILTERS, apply, describe, parse


@dataclass
class _Item:
    metadata: dict = field(default_factory=dict)


def test_空の指定は絞り込みなし():
    assert parse({}) == []
    assert parse(None) == []


def test_単一の値で絞れる():
    filters = parse({"部署": "品質保証部"})
    kept = apply(filters, [_Item({"部署": "品質保証部"}), _Item({"部署": "法務部"})])
    assert len(kept) == 1


def test_配列はORになる():
    filters = parse({"年度": ["2025", "2026"]})
    items = [_Item({"年度": "2025"}), _Item({"年度": "2026"}), _Item({"年度": "2024"})]
    assert len(apply(filters, items)) == 2


def test_複数の条件はANDになる():
    filters = parse({"部署": "品質保証部", "年度": "2026"})
    items = [
        _Item({"部署": "品質保証部", "年度": "2026"}),
        _Item({"部署": "品質保証部", "年度": "2025"}),
    ]
    assert len(apply(filters, items)) == 1


def test_値が数でも文字列として比べる():
    filters = parse({"年度": 2026})
    assert len(apply(filters, [_Item({"年度": "2026"})])) == 1


def test_メタデータ側が配列でも当たる():
    filters = parse({"製品": "A-2210"})
    assert len(apply(filters, [_Item({"製品": ["A-2210", "B-4400"]})])) == 1


def test_鍵が無ければ当たらない():
    """★ 表記ゆれ（「部署」と「部門」）はそのまま当たらなさになる。"""
    filters = parse({"部門": "品質保証部"})
    assert apply(filters, [_Item({"部署": "品質保証部"})]) == []


def test_メタデータが空でも落ちない():
    filters = parse({"部署": "品質保証部"})
    assert apply(filters, [_Item()]) == []


def test_条件が多すぎると弾かれる():
    with pytest.raises(InvalidInput):
        parse({f"k{i}": "v" for i in range(MAX_FILTERS + 1)})


def test_値が多すぎると弾かれる():
    with pytest.raises(InvalidInput):
        parse({"年度": [str(i) for i in range(33)]})


def test_空の値は弾かれる():
    with pytest.raises(InvalidInput):
        parse({"年度": []})


def test_鍵が空だと弾かれる():
    with pytest.raises(InvalidInput):
        parse({"": "値"})


def test_説明には鍵しか出さない():
    """**値は出さない** —— 監査ログとエラーに機密が乗るのを防ぐ。"""
    text = describe(parse({"部署": "法務部", "年度": "2026"}))
    assert "部署" in text and "年度" in text
    assert "法務部" not in text
