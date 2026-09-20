"""再インデックス。見積り・組み直し・突合・張り替え。

**検索を止めない。** 旧を読ませたまま新を作り、別名の張り替えで切り替える。
旧は 7 日残す（★ この日数は runbook にしかない）。
"""

from __future__ import annotations

from datetime import timedelta

import pytest

from kotonoha.common.clock import freeze, now
from kotonoha.common.errors import ClassificationViolation, IndexBusy, InvalidInput
from kotonoha.reindex.models import Verification
from kotonoha.reindex.planner import SLO_MINUTES
from kotonoha.reindex.switch import RETENTION_DAYS


# ── 見積り ───────────────────────────────────────────────────────
def test_見積りが出る(services, ingested):
    plan = services.reindex.plan(ingested.collection_id, "voyage-4-large")
    assert plan.from_model == "voyage-4"
    assert plan.to_model == "voyage-4-large"
    assert plan.total_chunks > 0


def test_同じ次元なら索引を作り直さない(services, ingested):
    plan = services.reindex.plan(ingested.collection_id, "voyage-4-large")
    assert not plan.dimension_changes


def test_チャンクが無ければ警告が出る(services, manual_collection):
    plan = services.reindex.plan(manual_collection.collection_id, "voyage-4-large")
    assert any("必要がありません" in w for w in plan.warnings)


def test_社内GPUを使うと警告が出る(services, secret_collection):
    plan = services.reindex.plan(secret_collection.collection_id, "voyage-4-nano")
    assert any("社内 GPU" in w for w in plan.warnings)


def test_極秘に外部モデルは指定できない(services, secret_collection):
    with pytest.raises(ClassificationViolation):
        services.reindex.plan(secret_collection.collection_id, "voyage-4-large")


def test_SLOの上限は12時間():
    assert SLO_MINUTES == 12 * 60


# ── 実行 ─────────────────────────────────────────────────────────
def test_始めると再構築中になる(services, ingested):
    services.reindex.start(ingested.collection_id, "voyage-4-large")
    assert not services.collections.get(ingested.collection_id).writable


def test_再構築中も検索できる(services, ingested):
    from kotonoha.search.models import SearchQuery
    services.reindex.start(ingested.collection_id, "voyage-4-large")
    result = services.search.search(SearchQuery(
        text="異音", collection_id=ingested.collection_id,
        tenant_id="cs-support"))
    assert result.hits


def test_二重に始められない(services, ingested):
    services.reindex.start(ingested.collection_id, "voyage-4-large")
    with pytest.raises(IndexBusy):
        services.reindex.start(ingested.collection_id, "voyage-4-large")


def test_新旧のインデックス名が違う(services, ingested):
    job = services.reindex.start(ingested.collection_id, "voyage-4-large")
    assert job.from_index != job.to_index


def test_進めると済んだ件数が増える(services, ingested):
    job = services.reindex.start(ingested.collection_id, "voyage-4-large")
    job = services.reindex.step(job.job_id)
    assert job.done_chunks == job.total_chunks
    assert job.status == "verifying"


def test_中止すると元に戻る(services, ingested):
    job = services.reindex.start(ingested.collection_id, "voyage-4-large")
    services.reindex.abort(job.job_id, "検証で問題が出た")
    assert services.collections.get(ingested.collection_id).writable


# ── 張り替え ─────────────────────────────────────────────────────
def _passed() -> Verification:
    return Verification(old_count=8, new_count=8, sampled=3,
                        agreement=1.0, passed=True)


def _failed() -> Verification:
    return Verification(old_count=8, new_count=3, sampled=3,
                        agreement=0.2, passed=False, notes=["件数が合わない"])


def test_突合に通れば張り替えられる(services, ingested):
    job = services.reindex.start(ingested.collection_id, "voyage-4-large")
    services.reindex.step(job.job_id)
    switched = services.reindex._switcher.switch(
        services.reindex.get(job.job_id), _passed())
    assert switched.switched


def test_突合に通らなければ張り替えない(services, ingested):
    job = services.reindex.start(ingested.collection_id, "voyage-4-large")
    with pytest.raises(InvalidInput):
        services.reindex._switcher.switch(job, _failed())


def test_人が承認すれば飛ばして張り替えられる(services, ingested):
    """**誰が決めたかを必ずログに残す。**"""
    job = services.reindex.start(ingested.collection_id, "voyage-4-large")
    switched = services.reindex._switcher.force_switch(job, approved_by="佐藤")
    assert switched.switched


def test_張り替えるとモデルが更新される(services, ingested):
    job = services.reindex.start(ingested.collection_id, "voyage-4-large")
    services.reindex.step(job.job_id)
    services.reindex.finish(job.job_id, force_by="佐藤")
    assert services.collections.get(
        ingested.collection_id).embed_model == "voyage-4-large"


def test_終えると書けるようになる(services, ingested):
    job = services.reindex.start(ingested.collection_id, "voyage-4-large")
    services.reindex.step(job.job_id)
    services.reindex.finish(job.job_id, force_by="佐藤")
    assert services.collections.get(ingested.collection_id).writable


# ── 旧の保持 ─────────────────────────────────────────────────────
def test_保持期間は7日():
    """★ この日数は runbook にしかない。"""
    assert RETENTION_DAYS == 7


def test_保持期間内は消せない(services, ingested):
    job = services.reindex.start(ingested.collection_id, "voyage-4-large")
    services.reindex.step(job.job_id)
    job = services.reindex.finish(job.job_id, force_by="佐藤")
    with pytest.raises(InvalidInput):
        services.reindex._switcher.drop_old(job)


def test_張り替えていなければ消せない(services, ingested):
    job = services.reindex.start(ingested.collection_id, "voyage-4-large")
    with pytest.raises(InvalidInput):
        services.reindex._switcher.drop_old(job)


def test_7日経てば消せる(services, ingested):
    job = services.reindex.start(ingested.collection_id, "voyage-4-large")
    services.reindex.step(job.job_id)
    job = services.reindex.finish(job.job_id, force_by="佐藤")
    freeze(now() + timedelta(days=RETENTION_DAYS + 1))
    services.reindex._switcher.drop_old(job)     # 例外が出ないこと
    assert job.old_dropped_at is not None
