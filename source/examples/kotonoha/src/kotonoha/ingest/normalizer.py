"""取り込み前の正規化。

**検索側の正規化（``search.query``）とは規則が違う。** こちらは原文の
見た目を保ちながら余計なものを落とす（改行コード・制御文字・全角空白）。
検索側は照合のために更に踏み込んで揃える。

社内文書は Word からの貼り付けが多く、見えない文字が混ざる ——
それをここで落とさないと、チャンクの境界がずれてトークン数の見積りも狂う。
"""

from __future__ import annotations

import re
import unicodedata

#: 制御文字（改行・タブは除く）。
_CONTROL = re.compile(r"[\x00-\x08\x0b\x0c\x0e-\x1f\x7f]")

#: 3 つ以上続く空行を 2 つに畳む。
_BLANK_RUN = re.compile(r"\n{3,}")

#: 行末の空白。
_TRAILING = re.compile(r"[ \t　]+$", re.MULTILINE)

#: Word からの貼り付けで混ざる見えない文字。
_INVISIBLE = dict.fromkeys(map(ord, "​‌‍﻿­"), None)


def normalize(text: str) -> str:
    """原文を整える。**内容は変えない。**"""
    if not text:
        return ""
    body = text.replace("\r\n", "\n").replace("\r", "\n")
    body = body.translate(_INVISIBLE)
    body = _CONTROL.sub("", body)
    body = body.replace("　", " ")
    body = _TRAILING.sub("", body)
    body = _BLANK_RUN.sub("\n\n", body)
    return body.strip()


def normalize_title(title: str) -> str:
    """表題を整える。全角英数を半角へ寄せる（検索の当たりを良くする）。"""
    return unicodedata.normalize("NFKC", title or "").strip()


def is_empty(text: str) -> bool:
    """中身が無いか。空白と記号だけのものも空とみなす。"""
    stripped = re.sub(r"[\s\-=_*#|]+", "", text or "")
    return not stripped
