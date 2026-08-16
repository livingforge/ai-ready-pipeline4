"""トークン数の見積り。

**本物のトークナイザではない**（提供元の語彙を持っていない）。
チャンクの境界を決めるのに使う近似で、課金には使わない。
"""

from __future__ import annotations

from kotonoha.common.tokenizer import count, fits, truncate


def test_空文字は0():
    assert count("") == 0


def test_1文字でも1以上になる():
    assert count("あ") >= 1


def test_長いほど多くなる():
    assert count("点検" * 10) > count("点検")


def test_日本語のほうが1文字あたり多い():
    assert count("あ" * 100) > count("a" * 100)


def test_収まる範囲で切る():
    text = "点検の手順を述べる。" * 100
    cut = truncate(text, 50)
    assert count(cut) <= 50
    assert text.startswith(cut)


def test_収まっていれば切らない():
    text = "短い文。"
    assert truncate(text, 1000) == text


def test_fitsは境界で一致する():
    text = "点検の手順。" * 20
    assert fits(text, count(text))
    assert not fits(text, count(text) - 1)
