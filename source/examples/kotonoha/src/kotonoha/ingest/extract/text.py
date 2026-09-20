"""プレーンテキスト。そのまま通す。"""

from __future__ import annotations

from kotonoha.ingest.extract.base import Extracted, as_text, registry


class TextExtractor:
    """``text/plain``。"""

    content_types = ("text/plain", "text/x-log", "")

    def extract(self, content: str | bytes) -> Extracted:
        body = as_text(content)
        return Extracted(text=body, notes={"lines": body.count("\n") + 1})


registry.register(TextExtractor())
