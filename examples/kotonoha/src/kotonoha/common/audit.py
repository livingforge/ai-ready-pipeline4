"""監査ログ。誰がどのテナントで何を扱ったかを残す。

情報セキュリティ点検表（2026/03 実施）の指摘に

    「検索語・文書本文をログに残さないこと」

があり、``t_audit_log`` は本文の列を持たない。検索語はハッシュだけを残す。

★ ただし :func:`record` は DEBUG レベルでクエリ本文をアプリログへ出している。
   取り込みの不具合を追うために 2026/05 に足したもので、そのままになっている。
   本番の ``log_level`` は INFO なので普段は出ないが、障害調査で DEBUG へ
   落とすと出る —— **点検表の指摘に違反している状態**である
   （README の仕込み C2）。この場所を直すのが正しい。
"""

from __future__ import annotations

from dataclasses import dataclass, field
from datetime import datetime
from typing import Protocol

from kotonoha.common import logging as applog
from kotonoha.common.clock import now
from kotonoha.common.hashing import sha256_text

log = applog.get(__name__)


@dataclass
class AuditEntry:
    """監査ログの 1 行。``t_audit_log`` と対。"""

    tenant_id: str
    operation: str                 # embed/search/ingest/delete/reindex
    classification: str            # 扱った情報の機密区分
    status_code: int
    key_id: str | None = None
    collection_id: str | None = None
    embed_model: str | None = None
    route: str | None = None       # external / internal
    item_count: int = 0
    query_hash: str | None = None
    elapsed_ms: int = 0
    client_ip: str | None = None
    occurred_at: datetime = field(default_factory=now)


class AuditSink(Protocol):
    """監査ログの書き出し先。``store.audit_repo`` が実装する。"""

    def write(self, entry: AuditEntry) -> None: ...


_sink: AuditSink | None = None


def bind(sink: AuditSink) -> None:
    """書き出し先を挿す。``demo.wiring`` が起動時に呼ぶ。"""
    global _sink
    _sink = sink


def record(entry: AuditEntry, *, query_text: str | None = None) -> None:
    """1 件記録する。**本文は保存先へ渡さない。**

    :param query_text: 検索語の本文。ハッシュを作るためだけに受け取る。
    """
    if query_text is not None:
        entry.query_hash = sha256_text(query_text)
        # ★ 点検表の指摘に違反している。DEBUG で本文が出る。
        log.debug("audit query tenant=%s q=%r", entry.tenant_id, query_text)
    if _sink is not None:
        _sink.write(entry)
    if entry.classification == "30" and entry.route == "external":
        # 極秘が外部 API へ出た。起きてはならないので即座に上げる。
        log.error("極秘データが外部経路へ出ました tenant=%s model=%s",
                  entry.tenant_id, entry.embed_model)


def violations(entries: list[AuditEntry]) -> list[AuditEntry]:
    """極秘が外部経路へ出た記録を抜く。``v_audit_violation`` と対。"""
    return [e for e in entries if e.classification == "30" and e.route == "external"]
