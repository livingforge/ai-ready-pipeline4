"""Markdown。**見出しを残す** —— チャンク分割がそれを境界に使う。

コードブロックの中は触らない（``#`` がコメントのことがある）。
表はそのまま残す —— 崩すと意味が失われるが、トークン数は増える。
"""

from __future__ import annotations

import re

from kotonoha.ingest.extract.base import Extracted, as_text, registry

_FENCE = re.compile(r"^\s*(```|~~~)")
_HEADING = re.compile(r"^(#{1,6})\s+")
#: 参照リンク ``[表題](url)`` は表題だけ残す（URL は検索の邪魔になる）。
_LINK = re.compile(r"\[([^\]]+)\]\([^)]*\)")
#: 画像は代替テキストだけ残す。
_IMAGE = re.compile(r"!\[([^\]]*)\]\([^)]*\)")


class MarkdownExtractor:
    """``text/markdown``。"""

    content_types = ("text/markdown", "text/x-markdown")

    def extract(self, content: str | bytes) -> Extracted:
        body = as_text(content)
        out: list[str] = []
        in_fence = False
        headings = 0
        for line in body.split("\n"):
            if _FENCE.match(line):
                in_fence = not in_fence
                out.append(line)
                continue
            if in_fence:
                out.append(line)
                continue
            if _HEADING.match(line):
                headings += 1
            line = _IMAGE.sub(r"\1", line)
            line = _LINK.sub(r"\1", line)
            out.append(line)
        return Extracted(
            text="\n".join(out),
            notes={"headings": headings, "fenced": in_fence},
            # 閉じていないコードブロックは崩れている疑いがある
            partial=in_fence,
        )


registry.register(MarkdownExtractor())
