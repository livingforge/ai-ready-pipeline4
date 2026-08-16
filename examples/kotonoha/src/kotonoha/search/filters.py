"""絞り込み。メタデータと機密区分。

**機密区分の絞り込みは検索では起きない。** コレクションが区分を持ち、
テナントは自分のコレクションしか引けないので、区分をまたぐ検索は
そもそも成立しない —— この設計にしたのは、検索のたびに区分を突き合わせる
と漏れが出るためである。ここが担うのはメタデータの絞り込みだけ。

メタデータのスキーマは決めていない。テナントが自由に付けた鍵で絞る。
**そのため鍵の表記ゆれがそのまま当たらなさになる**（「部署」と「部門」）が、
基盤側で正規化はしない —— 意味の判断になるためである。
"""

from __future__ import annotations

from dataclasses import dataclass

from kotonoha.common.errors import InvalidInput

#: 一度に指定できる絞り込みの数。
MAX_FILTERS = 16

#: 値の配列で指定できる数（``{"年度": ["2025", "2026"]}``）。
MAX_VALUES = 32


@dataclass
class Filter:
    """1 つぶんの絞り込み。"""

    key: str
    values: tuple[str, ...]

    def matches(self, metadata: dict) -> bool:
        actual = metadata.get(self.key)
        if actual is None:
            return False
        if isinstance(actual, (list, tuple)):
            return any(str(a) in self.values for a in actual)
        return str(actual) in self.values


def parse(raw: dict) -> list[Filter]:
    """入力の辞書を絞り込みに変える。

    値は文字列か文字列の配列。``{"部署": "品質保証部"}`` と
    ``{"年度": ["2025", "2026"]}`` の両方を受ける。

    :raises InvalidInput: 数が多すぎる／型が合わない
    """
    if not raw:
        return []
    if len(raw) > MAX_FILTERS:
        raise InvalidInput(
            f"絞り込みは {MAX_FILTERS} 個までです（{len(raw)} 個）",
            count=len(raw), limit=MAX_FILTERS)

    filters: list[Filter] = []
    for key, value in raw.items():
        if not isinstance(key, str) or not key:
            raise InvalidInput(f"絞り込みの鍵が不正です: {key!r}", key=key)
        if isinstance(value, (list, tuple)):
            if len(value) > MAX_VALUES:
                raise InvalidInput(
                    f"{key} の値は {MAX_VALUES} 個までです（{len(value)} 個）",
                    key=key, count=len(value))
            values = tuple(str(v) for v in value)
        else:
            values = (str(value),)
        if not values:
            raise InvalidInput(f"{key} の値が空です", key=key)
        filters.append(Filter(key=key, values=values))
    return filters


def apply(filters: list[Filter], candidates: list) -> list:
    """絞り込みに合うものだけ残す。``candidates`` は ``metadata`` を持つ。

    **すべての条件に合うもの**（AND）を残す。OR は値の配列で表す。
    """
    if not filters:
        return list(candidates)
    return [c for c in candidates
            if all(f.matches(getattr(c, "metadata", {}) or {}) for f in filters)]


def describe(filters: list[Filter]) -> str:
    """人が読む形。監査ログとエラーメッセージに使う（**値は入れない**）。"""
    return ", ".join(sorted(f.key for f in filters))
