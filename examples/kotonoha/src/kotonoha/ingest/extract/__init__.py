"""原文からテキストを起こす。形式ごとに 1 モジュール。

**対応していない形式は黙って飛ばさない。** ``UnsupportedType`` を上げて
ジョブの明細に残す —— 取り込んだつもりで入っていないのがいちばん困る。

対応:

=====================  ==========================================
content_type           起こすもの
=====================  ==========================================
``text/plain``         そのまま
``text/markdown``      見出しを残したまま整える
``text/html``          タグを落として見出しを ``#`` へ寄せる
``application/pdf``    テキスト層だけ。**画像の PDF は起こせない**
``text/csv``           1 行 1 段落。見出し行を各行に付ける
=====================  ==========================================

PDF の画像（スキャンした紙）は OCR が要るが、入れていない。
品質保証部の古い不具合報告にこれが混ざっていて、**取り込めないまま
残っている** —— ``docs/runbook/ingest.md`` に既知の穴として書いてある。
"""

from kotonoha.ingest.extract.base import Extracted, UnsupportedType, registry
from kotonoha.ingest.extract import csvdoc, html, markdown, pdf, text  # noqa: F401

__all__ = ["Extracted", "UnsupportedType", "registry", "extract"]


def extract(content: str | bytes, content_type: str) -> Extracted:
    """形式に合った起こし方を選ぶ。

    :raises UnsupportedType: 対応していない形式
    """
    return registry.get(content_type).extract(content)
