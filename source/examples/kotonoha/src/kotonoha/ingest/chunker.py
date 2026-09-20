"""文書をチャンク（分割単位・断片）に切る。

★ **この規則はどの設計文書にも書かれていない。**

ADR にも docs/ にも Excel にも無く、ここが唯一の正本である
（README の仕込み A1）。PoC のときに手で試して決めた値がそのまま本番へ
入っており、根拠を知っているのは当時いた 2 人だけである。

規則は 3 つ。

1. **512 トークンごとに切る**（``TARGET_TOKENS``）。voyage-4 の文脈長は
   32,000 あるが、長い塊は検索の粒度が粗くなって当たらなくなる。
2. **64 トークンを隣と重ねる**（``OVERLAP_TOKENS``）。境界に跨った文が
   どちらのチャンクからも引けなくなるのを防ぐ。
3. **見出しの境界があればそちらを優先する**。512 に達していなくても
   見出しが来たら切る。逆に、見出しの直後で 512 に達しても、
   段落の途中では切らずに ``MAX_TOKENS`` まで伸ばす。

3 が効くのは Markdown と HTML から起こしたものだけで、PDF から起こした
テキストには見出しが無いので 1 と 2 だけで切れる。**その差が検索の質に
出ている**が、測ってはいない。
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field

from kotonoha.common.tokenizer import count, truncate

#: 目標のチャンク長。
TARGET_TOKENS = 512

#: 隣と重ねる長さ。
OVERLAP_TOKENS = 64

#: 見出しを跨がずに伸ばせる上限。ここを超えたら段落の途中でも切る。
MAX_TOKENS = 768

#: これより短い末尾の切れ端は手前へ吸わせる（1 語だけのチャンクを作らない）。
MIN_TOKENS = 48

#: Markdown の見出し。
_HEADING = re.compile(r"^(#{1,6})\s+(.+?)\s*$", re.MULTILINE)

#: 段落の区切り。
_PARAGRAPH = re.compile(r"\n\s*\n")


@dataclass
class Chunk:
    """切り出した 1 つ。``t_chunk`` と対。"""

    seq_no: int
    body: str
    token_count: int
    char_start: int
    char_end: int
    heading_path: str = ""


@dataclass
class Section:
    """見出しで区切った区間。切る前の中間表現。"""

    heading_path: str
    text: str
    char_start: int
    #: 見出し行そのものは本文に含めるが、トークン数には数える
    level: int = 0


def split_sections(text: str) -> list[Section]:
    """見出しで区間に割る。見出しが無ければ全体で 1 区間。

    見出しの階層を ``heading_path``（``"1章 > 1.2 節"``）に積む。
    検索結果に文脈を出すために使う。
    """
    matches = list(_HEADING.finditer(text))
    if not matches:
        return [Section(heading_path="", text=text, char_start=0)]

    sections: list[Section] = []
    stack: list[tuple[int, str]] = []

    if matches[0].start() > 0:
        head = text[: matches[0].start()].strip()
        if head:
            sections.append(Section(heading_path="", text=head, char_start=0))

    for index, match in enumerate(matches):
        level = len(match.group(1))
        title = match.group(2).strip()
        while stack and stack[-1][0] >= level:
            stack.pop()
        stack.append((level, title))
        path = " > ".join(t for _, t in stack)

        start = match.start()
        end = matches[index + 1].start() if index + 1 < len(matches) else len(text)
        body = text[start:end].strip()
        if body:
            sections.append(Section(heading_path=path, text=body,
                                    char_start=start, level=level))
    return sections


def chunk_text(text: str, *, target: int = TARGET_TOKENS,
               overlap: int = OVERLAP_TOKENS,
               maximum: int = MAX_TOKENS) -> list[Chunk]:
    """文書全体を切る。

    :param target: 目標のチャンク長（トークン）
    :param overlap: 隣と重ねる長さ（トークン）
    :param maximum: 段落の切れ目を探して伸ばせる上限
    """
    chunks: list[Chunk] = []
    for section in split_sections(text):
        for piece in _chunk_section(section, target, overlap, maximum):
            piece.seq_no = len(chunks)
            chunks.append(piece)
    return _absorb_tail(chunks)


def _chunk_section(section: Section, target: int, overlap: int,
                   maximum: int) -> list[Chunk]:
    """1 区間を切る。**段落の切れ目を優先して境界にする。**

    **残りが上限に収まったらそこで打ち切る。** 打ち切らずに重なりを
    取り続けると、末尾で歩幅が 0 になって同じ断片を繰り返す。
    """
    pieces: list[Chunk] = []
    text = section.text
    cursor = 0
    while cursor < len(text):
        window = text[cursor:]
        last = count(window) <= maximum
        head = window if last else _extend_to_boundary(
            window, truncate(window, target), maximum)
        piece = _make_chunk(section, cursor, head)
        if piece is not None:
            pieces.append(piece)
        if last or cursor + len(head) >= len(text):
            break
        cursor += max(1, len(head) - _overlap_chars(head, overlap))
    return pieces


def _make_chunk(section: Section, cursor: int, head: str) -> Chunk | None:
    """切り出した範囲を 1 つのチャンクにする。空なら ``None``。"""
    body = head.strip()
    if not body:
        return None
    offset = cursor + (len(head) - len(head.lstrip()))
    return Chunk(
        seq_no=0, body=body, token_count=count(body),
        char_start=section.char_start + offset,
        char_end=section.char_start + offset + len(body),
        heading_path=section.heading_path,
    )


def _extend_to_boundary(window: str, head: str, maximum: int) -> str:
    """段落の切れ目まで伸ばす。``maximum`` を超えるなら伸ばさない。"""
    match = _PARAGRAPH.search(window, len(head))
    if match is None:
        return head
    extended = window[: match.start()]
    return extended if count(extended) <= maximum else head


def _overlap_chars(head: str, overlap_tokens: int) -> int:
    """末尾から ``overlap_tokens`` ぶんに相当する文字数。"""
    if overlap_tokens <= 0:
        return 0
    low, high = 0, len(head)
    while low < high:
        mid = (low + high + 1) // 2
        if count(head[-mid:]) <= overlap_tokens:
            low = mid
        else:
            high = mid - 1
    return low


def _absorb_tail(chunks: list[Chunk]) -> list[Chunk]:
    """短すぎる末尾を手前へ吸わせる。**見出しを跨ぐときは吸わせない。**"""
    if len(chunks) < 2:
        return chunks
    result = list(chunks)
    last = result[-1]
    prev = result[-2]
    if last.token_count < MIN_TOKENS and last.heading_path == prev.heading_path:
        merged = Chunk(
            seq_no=prev.seq_no,
            body=prev.body + "\n" + last.body,
            token_count=count(prev.body + "\n" + last.body),
            char_start=prev.char_start,
            char_end=last.char_end,
            heading_path=prev.heading_path,
        )
        result = result[:-2] + [merged]
    for index, chunk in enumerate(result):
        chunk.seq_no = index
    return result
