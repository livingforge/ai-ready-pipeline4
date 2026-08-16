"""アプリログ。標準ライブラリの ``logging`` を薄く包む。

**監査ログとは別物である。** 監査ログは ``common.audit`` で、消してはならない
記録として ``t_audit_log`` に入る。こちらは運用が読む普通のログで、
14 日で回る。
"""

from __future__ import annotations

import logging
import sys

from kotonoha.common.settings import SETTINGS

_CONFIGURED = False


def setup(level: str | None = None) -> None:
    """1 プロセスに一度だけ呼ぶ。

    二度目以降は組み立て直さないが、**``level`` を明示したときは
    水準だけ入れ替える** —— モジュールの読み込みで先に既定の水準で
    組まれてしまうので、``main`` から下げられないと困る。
    """
    global _CONFIGURED
    if _CONFIGURED:
        if level:
            logging.getLogger("kotonoha").setLevel(
                getattr(logging, level.upper(), logging.INFO))
        return
    handler = logging.StreamHandler(sys.stderr)
    handler.setFormatter(logging.Formatter(
        "%(asctime)s %(levelname)-5s [%(name)s] %(message)s",
        datefmt="%Y-%m-%dT%H:%M:%S",
    ))
    root = logging.getLogger("kotonoha")
    root.handlers.clear()
    root.addHandler(handler)
    root.setLevel(getattr(logging, (level or SETTINGS.log_level).upper(), logging.INFO))
    root.propagate = False
    _CONFIGURED = True


def get(name: str) -> logging.Logger:
    """モジュール用のロガーを返す。``get(__name__)`` で使う。"""
    setup()
    return logging.getLogger(name if name.startswith("kotonoha") else f"kotonoha.{name}")
