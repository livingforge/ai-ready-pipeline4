"""当たった箇所の切り出し。

**強調のタグは付けない。** 位置だけを返し、見せ方は利用側が決める ——
HTML を返すとエスケープ漏れの事故になる。
"""

from __future__ import annotations

from kotonoha.search.highlight import SNIPPET_CHARS, snippet


def test_空の本文は空を返す():
    text, spans = snippet("", "異音")
    assert text == ""
    assert spans == []


def test_当たった語の周りを切り出す():
    body = "前置き。" * 40 + "軸受から異音がする。" + "後書き。" * 40
    text, spans = snippet(body, "異音")
    assert "異音" in text
    assert len(text) <= SNIPPET_CHARS + 2      # 前後の三点リーダぶん


def test_当たらなければ先頭から切る():
    """**空を返さない** —— 利用側が「何も当たらなかった」と誤解する。"""
    body = "点検の手順を述べる。" * 40
    text, spans = snippet(body, "存在しない語")
    assert text.startswith("点検の手順")
    assert spans == []


def test_短い本文はそのまま返る():
    text, _ = snippet("軸受から異音がする。", "異音")
    assert text == "軸受から異音がする。"


def test_先頭で当たれば前の三点リーダは付かない():
    body = "異音がする。" + "後書き。" * 40
    text, _ = snippet(body, "異音")
    assert not text.startswith("…")


def test_途中で当たれば前に三点リーダが付く():
    body = "前置き。" * 40 + "異音がする。"
    text, _ = snippet(body, "異音")
    assert text.startswith("…")


def test_当たった位置が返る():
    text, spans = snippet("軸受から異音がする。", "異音")
    assert spans
    assert text[spans[0].start:spans[0].end] == "異音"


def test_複数の語が当たる():
    text, spans = snippet("異音と過熱を点検する。", "異音 過熱")
    assert len(spans) == 2


def test_重なった範囲は畳まれる():
    text, spans = snippet("異音異音", "異音 異音")
    assert len(spans) <= 2


def test_1文字の語は当てにいかない():
    """短すぎる語で切り出すと、どこでも当たって意味を持たない。"""
    body = "点検の手順を述べる。" * 40
    text, spans = snippet(body, "の")
    assert spans == []
