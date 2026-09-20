"""CSV。品質保証部の不具合報告がこの形で来る。

**1 行を 1 つのまとまりとして起こす。** 見出し行を各行に付けて
``列名: 値`` の並びにする —— そうしないと「発生日 2026-03-12」が
どの列の値なのか分からなくなり、検索で当たらない。

行数が多いので、チャンク分割は行の境界を跨がない（1 行が 512 トークンを
超えることは実質無い）。
"""

from __future__ import annotations

import csv
import io

from kotonoha.ingest.extract.base import Extracted, as_text, registry

#: これを超える行数は起こさない。分割して入れ直してもらう。
MAX_ROWS = 20_000


class CsvExtractor:
    """``text/csv``。"""

    content_types = ("text/csv", "application/csv")

    def extract(self, content: str | bytes) -> Extracted:
        body = as_text(content)
        reader = csv.reader(io.StringIO(body))
        try:
            header = next(reader)
        except StopIteration:
            return Extracted(text="", notes={"rows": 0})

        header = [h.strip() for h in header]
        blocks: list[str] = []
        rows = 0
        truncated = False
        for row in reader:
            if rows >= MAX_ROWS:
                truncated = True
                break
            pairs = [f"{name}: {value.strip()}"
                     for name, value in zip(header, row) if value.strip()]
            if pairs:
                blocks.append("\n".join(pairs))
                rows += 1

        return Extracted(
            text="\n\n".join(blocks),
            notes={"rows": rows, "columns": len(header), "truncated": truncated},
            partial=truncated,
        )


registry.register(CsvExtractor())
