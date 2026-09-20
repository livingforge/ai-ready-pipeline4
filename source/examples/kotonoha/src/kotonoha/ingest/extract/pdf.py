"""PDF。**テキスト層だけ**を起こす。

★ **既知の穴。** スキャンした紙の PDF（画像だけで文字が入っていない）は
起こせない。OCR を入れていないためで、``docs/runbook/ingest.md`` に
書いてある。品質保証部の 2018 年より前の不具合報告がこれにあたり、
**取り込めないまま残っている**。

本番では ``pypdf`` を使う。ここは外部依存を持たない方針なので、
テキスト層を持つ最小限の PDF だけを読む簡易の実装にしてある ——
``BT ... ET`` の間の ``(...) Tj`` を拾うだけで、フォントの符号化も
配置も見ていない。**本番へ持っていってはならない。**
"""

from __future__ import annotations

import re

from kotonoha.ingest.extract.base import Extracted, registry

#: ``(文字列) Tj`` / ``[(文字列) ...] TJ``
_SHOW = re.compile(rb"\((?:[^()\\]|\\.)*\)")
#: テキストの区間
_TEXT_BLOCK = re.compile(rb"BT(.*?)ET", re.DOTALL)
#: 頁の区切り
_PAGE = re.compile(rb"/Type\s*/Page\b")

#: これを下回る頁は文字が入っていないとみなす（スキャンした紙）。
MIN_CHARS_PER_PAGE = 4


class PdfExtractor:
    """``application/pdf``。テキスト層のみ。"""

    content_types = ("application/pdf",)

    def extract(self, content: str | bytes) -> Extracted:
        data = content.encode("latin-1") if isinstance(content, str) else content
        pages = len(_PAGE.findall(data)) or 1

        pieces: list[str] = []
        for block in _TEXT_BLOCK.findall(data):
            for raw in _SHOW.findall(block):
                pieces.append(_unescape(raw[1:-1]))

        text = "\n".join(p for p in (s.strip() for s in pieces) if p)
        # 文字がほとんど取れなければ画像の PDF とみなす。**頁あたりで見る**
        # —— 総文字数で見ると、1 頁の短い通知が画像扱いになる。
        image_only = not text or (len(text) / pages) < MIN_CHARS_PER_PAGE
        return Extracted(
            text=text,
            notes={"pages": pages, "image_only": image_only,
                   "hint": "OCR が要ります（未対応）" if image_only else ""},
            partial=image_only,
        )


def _unescape(raw: bytes) -> str:
    out = bytearray()
    index = 0
    while index < len(raw):
        byte = raw[index]
        if byte == 0x5C and index + 1 < len(raw):        # バックスラッシュ
            index += 1
            nxt = raw[index]
            out.append({0x6E: 0x0A, 0x72: 0x0D, 0x74: 0x09}.get(nxt, nxt))
        else:
            out.append(byte)
        index += 1
    return bytes(out).decode("utf-8", errors="replace")


registry.register(PdfExtractor())
