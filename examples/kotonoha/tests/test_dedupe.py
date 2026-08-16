"""同じ文書の再取り込みを飛ばす。

保守マニュアルの全件同期では 9 割以上がここで飛ぶ。
**これが無いと課金が 10 倍になる。**
"""

from __future__ import annotations

from conftest import make_source

from kotonoha.ingest import dedupe
from kotonoha.ingest.models import Document


def _existing(source, **kw) -> Document:
    defaults = dict(
        document_id="doc-1", collection_id="col-1", title=source.title,
        source_uri="s3://x/y", content_type=source.content_type,
        content_hash=dedupe.content_hash(source), byte_size=len(source.content),
        external_id=source.external_id, metadata=dict(source.metadata),
    )
    defaults.update(kw)
    return Document(**defaults)


def test_初回は取り込む():
    decision = dedupe.decide(make_source("本文"), None)
    assert decision.action == "ingest"
    assert decision.reason == "new"


def test_同じ本文は飛ぶ():
    source = make_source("軸受から異音がする。")
    decision = dedupe.decide(source, _existing(source))
    assert decision.action == "skip"
    assert decision.reason == "same_hash"
    assert decision.skipped


def test_本文が変われば取り込む():
    source = make_source("直した本文。")
    old = _existing(make_source("古い本文。"))
    decision = dedupe.decide(source, old)
    assert decision.action == "ingest"
    assert decision.reason == "content_changed"
    assert decision.existing is old


def test_メタデータだけならメタデータだけ直す():
    """**埋め込みは本文からしか作らない。** 作り直す意味が無い。"""
    source = make_source("同じ本文。", metadata={"年度": "2026"})
    old = _existing(make_source("同じ本文。", metadata={"年度": "2025"}))
    decision = dedupe.decide(source, old)
    assert decision.action == "metadata_only"
    assert decision.skipped


def test_表題が変わっても本文が同じなら飛ぶ():
    source = make_source("同じ本文。", title="新しい表題")
    old = _existing(make_source("同じ本文。", title="古い表題"))
    assert dedupe.decide(source, old).action == "skip"


def test_削除済みなら取り込み直す():
    from kotonoha.common.clock import now
    source = make_source("本文")
    old = _existing(source, deleted_at=now())
    assert dedupe.decide(source, old).action == "ingest"


def test_ハッシュは本文だけから取る():
    a = dedupe.content_hash(make_source("同じ本文", title="A"))
    b = dedupe.content_hash(make_source("同じ本文", title="B"))
    assert a == b


def test_本文が違えばハッシュも違う():
    assert dedupe.content_hash(make_source("A")) != \
        dedupe.content_hash(make_source("B"))


def test_内訳を数えられる():
    decisions = [
        dedupe.Decision(action="ingest"),
        dedupe.Decision(action="skip"),
        dedupe.Decision(action="skip"),
    ]
    assert dedupe.summarize(decisions) == {"ingest": 1, "skip": 2}
