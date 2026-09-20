"""取り込み前の正規化。

社内文書は Word からの貼り付けが多く、見えない文字が混ざる。
落とさないとチャンクの境界がずれる。
"""

from __future__ import annotations

from kotonoha.ingest.normalizer import is_empty, normalize, normalize_title


def test_改行コードが揃う():
    assert normalize("一行目\r\n二行目\r三行目") == "一行目\n二行目\n三行目"


def test_見えない文字が落ちる():
    assert normalize("点検​の﻿手順") == "点検の手順"


def test_制御文字が落ちる():
    assert normalize("点検\x00の\x08手順") == "点検の手順"


def test_全角空白が半角になる():
    assert normalize("点検　の　手順") == "点検 の 手順"


def test_行末の空白が落ちる():
    assert normalize("点検   \n手順\t\t") == "点検\n手順"


def test_3行以上の空行は2行に畳まれる():
    assert normalize("前\n\n\n\n\n後") == "前\n\n後"


def test_前後の空白が落ちる():
    assert normalize("\n\n  点検  \n\n") == "点検"


def test_空文字はそのまま():
    assert normalize("") == ""


def test_表題は全角英数が半角に寄る():
    assert normalize_title("Ａ－２２１０ 型") == "A-2210 型"


def test_空判定は記号だけも空とみなす():
    assert is_empty("")
    assert is_empty("   \n\t ")
    assert is_empty("---===___")
    assert not is_empty("点検")


def test_見出しの記号だけの行は空とみなす():
    assert is_empty("### ")
    assert not is_empty("### 点検")
