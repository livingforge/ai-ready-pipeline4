"""監査ログ。

★ **情報セキュリティ点検表の指摘に違反している箇所がある。**

指摘は「検索語・文書本文をログに残さないこと」。``t_audit_log`` は
本文の列を持たず、``AuditEntry`` も持たないので**保存先は正しい**。
ところが :func:`kotonoha.common.audit.record` が DEBUG レベルで
クエリ本文をアプリログへ出しており、指摘に反している
（README の仕込み C2）。

この検証は**違反していることを確かめる**。直したらここを直す ——
「直したつもりで直っていない」を防ぐため、いまの状態を固定しておく。
"""

from __future__ import annotations

import logging

from kotonoha.common import audit
from kotonoha.common.audit import AuditEntry, record
from kotonoha.common.hashing import sha256_text


def test_保存先は本文の列を持たない():
    entry = AuditEntry(tenant_id="t", operation="search",
                       classification="10", status_code=200)
    assert not hasattr(entry, "query_text")
    assert not hasattr(entry, "body")


def test_検索語はハッシュになる(services):
    record(AuditEntry(tenant_id="cs-support", operation="search",
                      classification="10", status_code=200),
           query_text="軸受の異音")
    entry = services.audit.entries[-1]
    assert entry.query_hash == sha256_text("軸受の異音")
    assert "軸受" not in entry.query_hash


def test_検索語を渡さなければハッシュも残らない(services):
    record(AuditEntry(tenant_id="cs-support", operation="embed",
                      classification="10", status_code=200))
    assert services.audit.entries[-1].query_hash is None


class _Capture(logging.Handler):
    """``kotonoha`` ロガーへ直に挿す。

    ``common.logging`` は ``propagate = False`` にしているので、pytest の
    ``caplog``（根のロガーに挿さる）では拾えない。**運用でログを根へ
    流さないようにしてある**ためで、その性質ごと確かめたい。
    """

    def __init__(self) -> None:
        super().__init__()
        self.messages: list[str] = []

    def emit(self, record_):
        self.messages.append(record_.getMessage())

    def __enter__(self):
        self._logger = logging.getLogger("kotonoha")
        self._level = self._logger.level
        self._logger.addHandler(self)
        return self

    def __exit__(self, *args):
        self._logger.removeHandler(self)
        self._logger.setLevel(self._level)
        return False

    def at(self, level: int) -> "_Capture":
        self._logger.setLevel(level)
        self.setLevel(level)
        return self

    def contains(self, text: str) -> bool:
        return any(text in m for m in self.messages)


def test_DEBUGで本文が漏れる(services):
    """★ **点検表の指摘に違反している状態。** 直すべきはここ。

    本番の ``log_level`` は INFO なので普段は出ないが、障害調査で
    DEBUG へ落とすと出る。
    """
    with _Capture() as captured:
        captured.at(logging.DEBUG)
        record(AuditEntry(tenant_id="cs-support", operation="search",
                          classification="10", status_code=200),
               query_text="社外秘の検索語")
    assert captured.contains("社外秘の検索語"),         "★ ここが False になったら仕込み C2 が直っている"


def test_INFOでは本文が出ない(services):
    """普段の水準では漏れない。**だから気づかれていない。**"""
    with _Capture() as captured:
        captured.at(logging.INFO)
        record(AuditEntry(tenant_id="cs-support", operation="search",
                          classification="10", status_code=200),
               query_text="社外秘の検索語")
    assert not captured.contains("社外秘の検索語")


def test_極秘が外部経路へ出たら拾える():
    """``v_audit_violation`` と対。**常に 0 件であるべき。**"""
    entries = [
        AuditEntry(tenant_id="a", operation="embed", classification="30",
                   status_code=200, route="internal"),
        AuditEntry(tenant_id="b", operation="embed", classification="30",
                   status_code=200, route="external"),
        AuditEntry(tenant_id="c", operation="embed", classification="10",
                   status_code=200, route="external"),
    ]
    violations = audit.violations(entries)
    assert [e.tenant_id for e in violations] == ["b"]


def test_極秘が外部へ出るとエラーログが出る(services):
    with _Capture() as captured:
        captured.at(logging.ERROR)
        record(AuditEntry(tenant_id="legal-contract", operation="embed",
                          classification="30", status_code=200,
                          route="external", embed_model="voyage-4"))
    assert captured.contains("外部経路")


def test_通常の運用では違反が出ない(services, ingested):
    """極秘のコレクションを含めて回しても 0 件であること。"""
    from conftest import make_source
    from kotonoha.search.models import SearchQuery

    secret = services.collections.create("legal-contract", "contract")
    services.ingest.submit("legal-contract", secret.collection_id,
                           [make_source("秘密保持の条項。")])
    services.worker.drain()
    services.search.search(SearchQuery(text="秘密保持",
                                       collection_id=secret.collection_id,
                                       tenant_id="legal-contract"))
    assert services.audit.violations() == []


def test_極秘の埋め込みは社内経路で記録される(services, secret_collection):
    from conftest import make_source
    services.ingest.submit("legal-contract", secret_collection.collection_id,
                           [make_source("条項。")])
    services.worker.drain()
    embeds = [e for e in services.audit.entries if e.operation == "embed"]
    assert embeds[-1].route == "internal"
    assert embeds[-1].embed_model == "voyage-4-nano"
