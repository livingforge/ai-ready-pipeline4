"""データベース接続の約束。本番は psycopg のカーソルを挿す。

**プレースホルダは ``%s``**（psycopg の書き方）。文字列連結で SQL を
組んではならない —— 検索語やメタデータの鍵がそのまま入る場所があり、
連結すると注入の穴になる。
"""

from __future__ import annotations

from typing import Any, Protocol, Sequence


class Connection(Protocol):
    """1 本の接続。トランザクションは呼ぶ側が張る。"""

    def execute(self, sql: str, params: Sequence[Any] = ()) -> None:
        """更新系。"""
        ...

    def execute_many(self, sql: str, rows: Sequence[Sequence[Any]]) -> None:
        """まとめて更新。取り込みのチャンク投入で使う。"""
        ...

    def fetch_one(self, sql: str, params: Sequence[Any] = ()) -> dict | None:
        """1 行。無ければ ``None``。"""
        ...

    def fetch_all(self, sql: str, params: Sequence[Any] = ()) -> list[dict]:
        """全行。**件数の上限は SQL 側で付ける。**"""
        ...


class Transaction(Protocol):
    """トランザクション。``with`` で使う。"""

    def __enter__(self) -> Connection: ...
    def __exit__(self, *args) -> bool: ...


def placeholders(count: int) -> str:
    """``%s, %s, %s`` を組む。``IN`` 句に使う。

    **数だけを組み立てる。** 値は必ずパラメータで渡す。
    """
    return ", ".join(["%s"] * count)
