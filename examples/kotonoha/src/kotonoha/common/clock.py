"""時刻。**直接 ``datetime.now()`` を呼ばない。**

テストが時刻を固定できるようにここを通す。締め処理と保持期間の判定が
時刻に依存するので、そこを再現できないと検証が書けない。
"""

from __future__ import annotations

from datetime import date, datetime, timedelta

_frozen: datetime | None = None


def now() -> datetime:
    """現在時刻。凍結されていればその値。"""
    return _frozen or datetime.now()


def today() -> date:
    """今日の日付。"""
    return now().date()


def year_month(when: date | None = None) -> str:
    """``YYYYMM``。利用量の締めの単位。"""
    target = when or today()
    return f"{target.year:04d}{target.month:02d}"


def freeze(at: datetime) -> None:
    """時刻を固定する。**テスト専用。**"""
    global _frozen
    _frozen = at


def unfreeze() -> None:
    """固定を解く。"""
    global _frozen
    _frozen = None


def expires_in(days: int, *, base: datetime | None = None) -> datetime:
    """``days`` 日後。キャッシュと保持期間の計算に使う。"""
    return (base or now()) + timedelta(days=days)
