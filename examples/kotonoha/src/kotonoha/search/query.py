"""検索語の正規化。

**取り込み側の正規化（``ingest.normalizer``）とは規則が違う。** あちらは
原文の見た目を保つが、こちらは照合のために踏み込んで揃える ——
全角半角・大文字小文字・長音・記号。

社内文書は表記が揺れる。「サーボモータ」「サーボモーター」「servo motor」が
同じものを指しており、揃えないと当たらない。**辞書は持っていない**ので、
機械的にできる範囲までしかやらない —— 同義語の展開は入れていない
（``docs/adr/0006-hybrid-search.md`` で見送りにした）。
"""

from __future__ import annotations

import re
import unicodedata

from kotonoha.common.errors import InvalidInput

#: 検索語の長さ。長すぎると全文検索が遅くなる。
MAX_QUERY_CHARS = 2_000

#: 落とす記号。検索の意味を持たないもの。
#:
#: **NFKC の後に掛かる**ので、半角へ寄ったあとの形で書く —— 全角の
#: ``（）`` は NFKC で ``()`` になるため、全角だけを並べても落ちない。
#: ピリオドとカンマは**落とさない**（``A-2210.5`` のような型番で意味を持つ）。
_NOISE = re.compile(r"[。、・「」『』()\[\]{}【】〈〉〔〕…‥]")

#: 連続する空白。
_SPACES = re.compile(r"\s+")

#: 末尾の長音。「モーター」→「モータ」に寄せる（工業分野の慣習）。
_TRAILING_CHOON = re.compile(r"ー(?=\s|$)")


def normalize(text: str) -> str:
    """照合用に揃える。

    :raises InvalidInput: 空／長すぎる
    """
    if not text or not text.strip():
        raise InvalidInput("検索語が空です")
    if len(text) > MAX_QUERY_CHARS:
        raise InvalidInput(
            f"検索語が長すぎます（{len(text)} 文字 / 上限 {MAX_QUERY_CHARS}）",
            length=len(text), limit=MAX_QUERY_CHARS)

    folded = unicodedata.normalize("NFKC", text).lower()
    folded = _NOISE.sub(" ", folded)
    folded = _TRAILING_CHOON.sub("", folded)
    return _SPACES.sub(" ", folded).strip()


def terms(text: str) -> list[str]:
    """全文検索へ渡す語に割る。

    **形態素解析はしない。** 本番は OpenSearch の kuromoji が行うので、
    ここは空白で割るだけ。この関数は疑似実装（``demo``）の全文検索が使う。
    """
    return [t for t in normalize(text).split(" ") if t]


def is_phrase(text: str) -> bool:
    """引用符で囲まれた完全一致の指定か。"""
    stripped = text.strip()
    return len(stripped) >= 2 and stripped[0] == stripped[-1] == '"'


def strip_quotes(text: str) -> str:
    stripped = text.strip()
    return stripped[1:-1] if is_phrase(stripped) else stripped
