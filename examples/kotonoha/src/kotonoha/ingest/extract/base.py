"""抽出の共通部分。"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Protocol

from kotonoha.common.errors import InvalidInput


class UnsupportedType(InvalidInput):
    """対応していない形式。**黙って飛ばさずにこれを上げる。**"""

    code = "unsupported_type"


@dataclass
class Extracted:
    """起こした結果。"""

    text: str
    #: 起こす過程で分かったこと（頁数・行数・文字化けの疑いなど）
    notes: dict = field(default_factory=dict)
    #: 起こせなかった部分があるか。**真なら取り込みの結果に残す**
    partial: bool = False


class Extractor(Protocol):
    """形式ごとの起こし方。"""

    content_types: tuple[str, ...]

    def extract(self, content: str | bytes) -> Extracted: ...


class _Registry:
    """``content_type`` から抽出器を引く。"""

    def __init__(self) -> None:
        self._by_type: dict[str, Extractor] = {}

    def register(self, extractor: Extractor) -> Extractor:
        for content_type in extractor.content_types:
            self._by_type[content_type] = extractor
        return extractor

    def get(self, content_type: str) -> Extractor:
        key = (content_type or "").split(";")[0].strip().lower()
        extractor = self._by_type.get(key)
        if extractor is None:
            raise UnsupportedType(
                f"対応していない形式です: {content_type}",
                content_type=content_type, supported=sorted(self._by_type),
            )
        return extractor

    def supported(self) -> list[str]:
        return sorted(self._by_type)


registry = _Registry()


def as_text(content: str | bytes, encoding: str = "utf-8") -> str:
    """バイト列なら文字列へ。**文字化けは落とさず置換で残す。**

    化けた文字を落とすと本文の位置がずれ、``char_start`` が信用できなく
    なる。置換文字を残しておけば、あとから化けに気づける。
    """
    if isinstance(content, str):
        return content
    return content.decode(encoding, errors="replace")
