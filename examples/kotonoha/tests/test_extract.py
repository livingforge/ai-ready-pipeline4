"""抽出（text / markdown / html / pdf / csv）。

★ **対応していない形式は黙って飛ばさない。** 取り込んだつもりで入って
   いないのがいちばん困るので、``UnsupportedType`` を上げる。
"""

from __future__ import annotations

import pytest

from kotonoha.ingest.extract import UnsupportedType, extract, registry


def test_知らない形式は弾かれる():
    with pytest.raises(UnsupportedType):
        extract("中身", "application/vnd.ms-excel")


def test_対応形式の一覧に主要な形式が入っている():
    supported = registry.supported()
    for kind in ("text/plain", "text/markdown", "text/html",
                 "application/pdf", "text/csv"):
        assert kind in supported


def test_文字集合の指定が付いていても引ける():
    result = extract("本文", "text/plain; charset=utf-8")
    assert result.text == "本文"


# ── text ─────────────────────────────────────────────────────────
def test_プレーンテキストはそのまま通る():
    result = extract("点検の手順\n2 行目", "text/plain")
    assert result.text == "点検の手順\n2 行目"
    assert result.notes["lines"] == 2


# ── markdown ─────────────────────────────────────────────────────
def test_markdownの見出しは残る():
    result = extract("# 章\n\n本文\n\n## 節\n\n本文\n", "text/markdown")
    assert "# 章" in result.text
    assert result.notes["headings"] == 2


def test_markdownのリンクは表題だけ残る():
    result = extract("詳細は [保守手順](https://example.invalid/a) を見る",
                     "text/markdown")
    assert "保守手順" in result.text
    assert "example.invalid" not in result.text


def test_markdownの画像は代替テキストだけ残る():
    result = extract("![配線図](https://example.invalid/z.png)", "text/markdown")
    assert result.text.strip() == "配線図"


def test_コードブロックの中は触らない():
    body = "```\n# これはコメント\n```\n"
    result = extract(body, "text/markdown")
    assert result.notes["headings"] == 0


def test_閉じていないコードブロックは部分的と申告する():
    result = extract("```\n中身\n", "text/markdown")
    assert result.partial


# ── html ─────────────────────────────────────────────────────────
def test_htmlの見出しはハッシュに寄る():
    result = extract("<h2>点検</h2><p>本文</p>", "text/html")
    assert "## 点検" in result.text
    assert result.notes["headings"] == 1


def test_htmlのタグは落ちる():
    result = extract("<p>点検の<b>手順</b></p>", "text/html")
    assert "<b>" not in result.text
    assert "点検の手順" in result.text


def test_scriptとstyleは落ちる():
    result = extract("<script>var a=1;</script><p>本文</p>", "text/html")
    assert "var a" not in result.text


def test_実体参照が戻る():
    result = extract("<p>A &amp; B</p>", "text/html")
    assert "A & B" in result.text


# ── pdf ──────────────────────────────────────────────────────────
def _pdf(text: str) -> bytes:
    return (b"%PDF-1.4\n/Type /Page\n"
            + b"BT (" + text.encode("utf-8") + b") Tj ET\n")


def test_pdfのテキスト層を起こせる():
    result = extract(_pdf("点検の手順を述べる"), "application/pdf")
    assert "点検の手順を述べる" in result.text
    assert not result.partial


def test_画像だけのpdfは部分的と申告する():
    """★ OCR を入れていない。既知の穴。"""
    result = extract(b"%PDF-1.4\n/Type /Page\n", "application/pdf")
    assert result.partial
    assert result.notes["image_only"]
    assert "OCR" in result.notes["hint"]


def test_pdfの頁数を数える():
    data = b"%PDF-1.4\n/Type /Page\n/Type /Page\nBT (abc) Tj ET\n"
    result = extract(data, "application/pdf")
    assert result.notes["pages"] == 2


# ── csv ──────────────────────────────────────────────────────────
def test_csvは1行が列名つきの塊になる():
    body = "報告番号,製品,内容\nQA-1,A-2210,異音\n"
    result = extract(body, "text/csv")
    assert "報告番号: QA-1" in result.text
    assert "製品: A-2210" in result.text
    assert result.notes["rows"] == 1


def test_csvの空の列は落ちる():
    body = "a,b,c\n1,,3\n"
    result = extract(body, "text/csv")
    assert "b:" not in result.text


def test_見出し行だけのcsvは空になる():
    result = extract("a,b,c\n", "text/csv")
    assert result.text == ""
    assert result.notes["rows"] == 0


def test_空のcsvは行数0():
    result = extract("", "text/csv")
    assert result.notes["rows"] == 0
