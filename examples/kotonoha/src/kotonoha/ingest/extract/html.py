"""HTML。社内 wiki からの書き出しがこれで来る。

``<h1>``〜``<h6>`` を Markdown の ``#`` へ寄せる —— チャンク分割が
見出しを境界に使うので、形式が違っても同じ規則に乗せるためである。
``<script>`` ``<style>`` ``<nav>`` は落とす。
"""

from __future__ import annotations

import html as htmllib
import re

from kotonoha.ingest.extract.base import Extracted, as_text, registry

_DROP = re.compile(r"<(script|style|nav|header|footer)\b.*?</\1>",
                   re.IGNORECASE | re.DOTALL)
_HEADING = re.compile(r"<h([1-6])\b[^>]*>(.*?)</h\1>", re.IGNORECASE | re.DOTALL)
_BLOCK = re.compile(r"</(p|div|li|tr|table|section|article|blockquote)>",
                    re.IGNORECASE)
_BR = re.compile(r"<br\s*/?>", re.IGNORECASE)
_TAG = re.compile(r"<[^>]+>")
_BLANK_RUN = re.compile(r"\n{3,}")


class HtmlExtractor:
    """``text/html``。"""

    content_types = ("text/html", "application/xhtml+xml")

    def extract(self, content: str | bytes) -> Extracted:
        body = as_text(content)
        body = _DROP.sub("", body)

        headings = 0

        def to_hash(match: re.Match) -> str:
            nonlocal headings
            headings += 1
            level = int(match.group(1))
            title = _TAG.sub("", match.group(2)).strip()
            return f"\n\n{'#' * level} {title}\n\n"

        body = _HEADING.sub(to_hash, body)
        body = _BR.sub("\n", body)
        body = _BLOCK.sub("\n", body)
        body = _TAG.sub("", body)
        body = htmllib.unescape(body)
        body = "\n".join(line.strip() for line in body.split("\n"))
        body = _BLANK_RUN.sub("\n\n", body).strip()
        return Extracted(text=body, notes={"headings": headings})


registry.register(HtmlExtractor())
