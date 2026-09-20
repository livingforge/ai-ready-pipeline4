"""当たった箇所の切り出し。

チャンクは 512 トークン（日本語で 700 字前後）あり、そのまま返すと
利用側が読めない。検索語に近いところを 160 字ほど切り出して返す。

**強調のタグは付けない。** 利用側がどう見せるかを決められるように、
位置だけを返す —— HTML を返すと、そのまま画面へ流し込まれて
エスケープ漏れの事故になる。
"""

from __future__ import annotations

from dataclasses import dataclass

from kotonoha.search import query as querylib

#: 切り出す長さ（文字）。
SNIPPET_CHARS = 160

#: 当たった箇所の前に付ける文脈の長さ。
LEAD_CHARS = 40


@dataclass
class Span:
    """当たった範囲。チャンク本文中の位置。"""

    start: int
    end: int


def snippet(body: str, text: str) -> tuple[str, list[Span]]:
    """切り出しと、その中で当たった範囲を返す。

    語が 1 つも見つからなければ先頭から切る（**空を返さない** ——
    利用側が「何も当たらなかった」と誤解する）。
    """
    if not body:
        return "", []
    words = [w for w in querylib.terms(querylib.strip_quotes(text)) if len(w) >= 2]
    lowered = body.lower()

    first = -1
    for word in words:
        found = lowered.find(word)
        if found >= 0 and (first < 0 or found < first):
            first = found

    if first < 0:
        cut = body[:SNIPPET_CHARS]
        return cut + ("…" if len(body) > SNIPPET_CHARS else ""), []

    start = max(0, first - LEAD_CHARS)
    end = min(len(body), start + SNIPPET_CHARS)
    cut = body[start:end]
    prefix = "…" if start > 0 else ""
    suffix = "…" if end < len(body) else ""

    spans: list[Span] = []
    cut_lower = cut.lower()
    for word in words:
        offset = 0
        while True:
            found = cut_lower.find(word, offset)
            if found < 0:
                break
            spans.append(Span(start=found + len(prefix),
                              end=found + len(word) + len(prefix)))
            offset = found + len(word)

    spans.sort(key=lambda s: (s.start, s.end))
    return prefix + cut + suffix, _merge(spans)


def _merge(spans: list[Span]) -> list[Span]:
    """重なった範囲を畳む。"""
    merged: list[Span] = []
    for span in spans:
        if merged and span.start <= merged[-1].end:
            merged[-1].end = max(merged[-1].end, span.end)
        else:
            merged.append(Span(span.start, span.end))
    return merged
