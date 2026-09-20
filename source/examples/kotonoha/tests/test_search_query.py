"""検索語の正規化。

**取り込み側の正規化とは規則が違う。** こちらは照合のために踏み込んで
揃える —— 全角半角・大文字小文字・末尾の長音。

同義語の展開は入れていない（``docs/adr/0006-hybrid-search.md`` で見送り）。
"""

from __future__ import annotations

import pytest

from kotonoha.common.errors import InvalidInput
from kotonoha.search.query import (MAX_QUERY_CHARS, is_phrase, normalize,
                                   strip_quotes, terms)


def test_空の検索語は弾かれる():
    with pytest.raises(InvalidInput):
        normalize("")
    with pytest.raises(InvalidInput):
        normalize("   ")


def test_長すぎる検索語は弾かれる():
    with pytest.raises(InvalidInput):
        normalize("あ" * (MAX_QUERY_CHARS + 1))


def test_全角英数が半角になる():
    assert normalize("Ａ－２２１０") == "a-2210"


def test_大文字が小文字になる():
    assert normalize("SERVO Motor") == "servo motor"


def test_末尾の長音が落ちる():
    """工業分野の慣習。「モーター」と「モータ」を同じに寄せる。"""
    assert normalize("サーボモーター") == "サーボモータ"


def test_語中の長音は落ちない():
    assert "ー" in normalize("モーター音")


def test_記号が落ちる():
    assert normalize("異音（軸受）") == "異音 軸受"


def test_連続する空白が畳まれる():
    assert normalize("異音   と    過熱") == "異音 と 過熱"


def test_語に割れる():
    assert terms("異音 と 過熱") == ["異音", "と", "過熱"]


def test_空白だけの語は落ちる():
    assert terms("異音    過熱") == ["異音", "過熱"]


def test_引用符で囲むと完全一致の指定になる():
    assert is_phrase('"A-2210"')
    assert not is_phrase("A-2210")


def test_片側だけの引用符は完全一致ではない():
    assert not is_phrase('"A-2210')


def test_引用符を外せる():
    assert strip_quotes('"A-2210"') == "A-2210"
    assert strip_quotes("A-2210") == "A-2210"
