"""チャンク分割の検証。

★ **分割規則の正本はコードにしかない**（``ingest/chunker.py``）ので、
   この検証が事実上の仕様書になっている。設計文書と突き合わせることが
   できない —— それが README の仕込み A1 である。
"""

from __future__ import annotations

from kotonoha.common.tokenizer import count
from kotonoha.ingest.chunker import (MAX_TOKENS, OVERLAP_TOKENS, TARGET_TOKENS,
                                     chunk_text, split_sections)


def test_短い文書は1つのチャンクになる():
    chunks = chunk_text("軸受の劣化が疑われる。回転数を落として異音を見る。")
    assert len(chunks) == 1
    assert chunks[0].seq_no == 0


def test_空文字は1つも出ない():
    assert chunk_text("") == []


def test_見出しごとに切れる():
    text = ("# 保守手順\n\n本文です。\n\n"
            "## 1. 定期点検\n\n6 か月ごとに実施する。\n\n"
            "## 2. 異音\n\n軸受を疑う。\n")
    chunks = chunk_text(text)
    paths = [c.heading_path for c in chunks]
    assert "保守手順 > 1. 定期点検" in paths
    assert "保守手順 > 2. 異音" in paths


def test_見出しの階層がパスに積まれる():
    text = "# 章\n\n本文\n\n## 節\n\n本文\n\n### 項\n\n本文\n"
    sections = split_sections(text)
    assert sections[-1].heading_path == "章 > 節 > 項"


def test_見出しが無ければ全体で1区間():
    sections = split_sections("見出しのない本文だけの文書。")
    assert len(sections) == 1
    assert sections[0].heading_path == ""


def test_見出しの前の前書きも区間になる():
    sections = split_sections("前書きです。\n\n# 章\n\n本文\n")
    assert sections[0].heading_path == ""
    assert "前書き" in sections[0].text


def test_長い文書は目標の長さで切れる():
    body = "点検の手順を順に述べる。" * 400
    chunks = chunk_text(body)
    assert len(chunks) > 1
    # 段落の切れ目まで伸ばすので TARGET は超えうるが MAX は超えない
    assert all(c.token_count <= MAX_TOKENS for c in chunks)


def test_通し番号が0から連番になる():
    body = "点検の手順を順に述べる。" * 400
    chunks = chunk_text(body)
    assert [c.seq_no for c in chunks] == list(range(len(chunks)))


def test_隣のチャンクと本文が重なる():
    """オーバーラップ。境界に跨った文が引けなくなるのを防ぐ。"""
    body = "".join(f"第{i}文です。異音と過熱を点検する。" for i in range(200))
    chunks = chunk_text(body, overlap=OVERLAP_TOKENS)
    assert len(chunks) >= 2
    # 前のチャンクの末尾が次のチャンクの先頭に現れる
    tail = chunks[0].body[-20:]
    assert tail in chunks[1].body or chunks[1].char_start < chunks[0].char_end


def test_オーバーラップを0にすると重ならない():
    body = "".join(f"第{i}文です。点検する。" for i in range(200))
    chunks = chunk_text(body, overlap=0)
    assert all(a.char_end <= b.char_start + 1
               for a, b in zip(chunks, chunks[1:]))


def test_短すぎる末尾は手前へ吸われる():
    """1 語だけのチャンクを作らない。"""
    body = "点検の手順。" * 300 + "\n\n以上。"
    chunks = chunk_text(body)
    assert all(c.token_count > 8 for c in chunks[-1:])


def test_見出しをまたぐ末尾は吸わせない():
    text = "# 章1\n\n" + "本文。" * 300 + "\n\n# 章2\n\n短い。\n"
    chunks = chunk_text(text)
    last, prev = chunks[-1], chunks[-2]
    assert last.heading_path != prev.heading_path


def test_位置が原文の範囲に収まる():
    text = "# 章\n\n" + "本文である。" * 200
    for chunk in chunk_text(text):
        assert 0 <= chunk.char_start <= chunk.char_end <= len(text)


def test_トークン数が本文と整合する():
    for chunk in chunk_text("点検の手順。" * 300):
        assert chunk.token_count == count(chunk.body)


def test_目標の長さを変えると個数が変わる():
    body = "点検の手順を述べる。" * 400
    few = chunk_text(body, target=TARGET_TOKENS * 2, maximum=MAX_TOKENS * 2)
    many = chunk_text(body, target=128, maximum=192)
    assert len(many) > len(few)
