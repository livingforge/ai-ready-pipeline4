"""トークン数の見積り。

**本物のトークナイザではない。** 埋め込みモデルの提供元が使う語彙を持って
いないので、日本語と英語で係数を変えた近似で数える。チャンク分割の境界を
決めるのに使う値なので、多少ずれても検索の質は落ちない —— ただし
**課金はトークンではなくチャンク数で数える**ので、ここのずれが請求に
効くことはない。

本番で厳密に数えるなら提供元のトークナイザを入れる。そのときここは
差し替えになる。
"""

from __future__ import annotations

#: 日本語 1 文字あたりのトークン数（実測の平均）。
_JA_RATE = 0.72
#: 英数 1 文字あたり。
_EN_RATE = 0.28


def _is_cjk(ch: str) -> bool:
    code = ord(ch)
    return (
        0x3040 <= code <= 0x30FF      # ひらがな・カタカナ
        or 0x4E00 <= code <= 0x9FFF   # 漢字
        or 0xFF00 <= code <= 0xFFEF   # 全角記号
    )


def count(text: str) -> int:
    """おおよそのトークン数。"""
    if not text:
        return 0
    cjk = sum(1 for ch in text if _is_cjk(ch))
    other = len(text) - cjk
    return max(1, round(cjk * _JA_RATE + other * _EN_RATE))


def truncate(text: str, max_tokens: int) -> str:
    """``max_tokens`` に収まるところで切る。

    二分探索で境界を出す。**文字の途中では切らない**が、単語や文の
    途中では切る —— 呼ぶ側（``ingest.chunker``）が境界を選ぶ前提。
    """
    if count(text) <= max_tokens:
        return text
    low, high = 0, len(text)
    while low < high:
        mid = (low + high + 1) // 2
        if count(text[:mid]) <= max_tokens:
            low = mid
        else:
            high = mid - 1
    return text[:low]


def fits(text: str, max_tokens: int) -> bool:
    return count(text) <= max_tokens
