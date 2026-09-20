"""取り込みの受付とジョブの記録。

**受け付ける前に 3 つ確かめる** —— 書けるか、区分とモデルが合うか、
上限に収まるか。上限は**見積りで弾く**（途中まで埋め込んでから止めると
課金だけされて使えないものが残る）。
"""

from __future__ import annotations

import pytest
from conftest import make_source

from kotonoha.common.errors import (IndexBusy, InvalidInput, NotFound,
                                    QuotaExceeded)
from kotonoha.common.settings import SETTINGS
from kotonoha.ingest.service import estimate_chunks


def test_受け付けるとジョブが返る(services, manual_collection):
    job = services.ingest.submit("cs-support", manual_collection.collection_id,
                                 [make_source("点検の手順。")])
    assert job.status == "queued"
    assert job.total_count == 1


def test_受付の時点ではまだ処理していない(services, manual_collection):
    job = services.ingest.submit("cs-support", manual_collection.collection_id,
                                 [make_source("点検の手順。")])
    assert job.done_count == 0
    assert services.chunks.count_in_collection(
        manual_collection.collection_id) == 0


def test_ワーカを回すと入る(services, manual_collection):
    job = services.ingest.submit("cs-support", manual_collection.collection_id,
                                 [make_source("点検の手順。")])
    services.worker.drain()
    assert services.tracker.get(job.job_id).status == "succeeded"
    assert services.chunks.count_in_collection(
        manual_collection.collection_id) > 0


def test_空の投入は弾かれる(services, manual_collection):
    with pytest.raises(InvalidInput):
        services.ingest.submit("cs-support", manual_collection.collection_id, [])


def test_件数が多すぎると弾かれる(services, manual_collection):
    sources = [make_source("本文", external_id=f"D{i}")
               for i in range(SETTINGS.max_documents_per_job + 1)]
    with pytest.raises(InvalidInput):
        services.ingest.submit("cs-support", manual_collection.collection_id,
                               sources)


def test_再構築中は受け付けない(services, manual_collection):
    """**検索は続けられる。**取り込みだけ止める。"""
    services.collections.mark_rebuilding(manual_collection.collection_id)
    with pytest.raises(IndexBusy):
        services.ingest.submit("cs-support", manual_collection.collection_id,
                               [make_source("本文")])


def test_他テナントのコレクションには入れられない(services, manual_collection):
    with pytest.raises(NotFound):
        services.ingest.submit("qa-defect", manual_collection.collection_id,
                               [make_source("本文")])


def test_上限を超える見積りは受け付けない(services, manual_collection):
    services.tenants.change_quota("cs-support", 1)
    with pytest.raises(QuotaExceeded):
        services.ingest.submit("cs-support", manual_collection.collection_id,
                               [make_source("点検の手順。" * 400)])


def test_見積りはトークン数から割る(services):
    """**切らずに割る。** 実際より少なめに出るのを承知で使っている。"""
    assert estimate_chunks([make_source("短い")]) == 1
    assert estimate_chunks([make_source("点検の手順。" * 400)]) > 1


def test_見積りは件数ぶん足し合わせる():
    sources = [make_source("点検の手順。" * 400, external_id=f"D{i}")
               for i in range(3)]
    assert estimate_chunks(sources) == estimate_chunks(sources[:1]) * 3


def test_進捗を引ける(services, manual_collection):
    job = services.ingest.submit("cs-support", manual_collection.collection_id,
                                 [make_source("本文")])
    assert services.ingest.status(job.job_id, "cs-support").job_id == job.job_id


def test_他テナントの進捗は引けない(services, manual_collection):
    job = services.ingest.submit("cs-support", manual_collection.collection_id,
                                 [make_source("本文")])
    with pytest.raises(NotFound):
        services.ingest.status(job.job_id, "qa-defect")


def test_失敗の明細が残る(services, manual_collection):
    """**どの文書が入らなかったか**が分からないと入れ直せない。"""
    services.ingest.submit("cs-support", manual_collection.collection_id, [
        make_source("良い本文", external_id="OK"),
        make_source("中身", external_id="NG", content_type="application/zip"),
    ])
    services.worker.drain()
    job = services.tracker.get(
        services.tracker._jobs.list_by_tenant("cs-support")[0].job_id)
    assert job.done_count == 1
    assert job.failed_count == 1
    failures = services.tracker.failures(job.job_id)
    assert [f.external_id for f in failures] == ["NG"]


def test_全部失敗したときだけ失敗になる(services, manual_collection):
    services.ingest.submit("cs-support", manual_collection.collection_id, [
        make_source("中身", external_id="NG", content_type="application/zip"),
    ])
    services.worker.drain()
    job = services.tracker._jobs.list_by_tenant("cs-support")[0]
    assert job.status == "failed"


def test_一部失敗なら成功として内訳を見せる(services, manual_collection):
    services.ingest.submit("cs-support", manual_collection.collection_id, [
        make_source("良い本文", external_id="OK"),
        make_source("中身", external_id="NG", content_type="application/zip"),
    ])
    services.worker.drain()
    job = services.tracker._jobs.list_by_tenant("cs-support")[0]
    assert job.status == "succeeded"


def test_テナントで均して取り出す(services, manual_collection):
    """品質保証部の大量投入で後ろが待たされないように。"""
    other = services.collections.create("qa-defect", "defect")
    services.ingest.submit("cs-support", manual_collection.collection_id,
                           [make_source("A")])
    services.ingest.submit("cs-support", manual_collection.collection_id,
                           [make_source("B", external_id="D2")])
    services.ingest.submit("qa-defect", other.collection_id,
                           [make_source("C")])
    order = []
    while True:
        item = services.worker._queue.pop()
        if item is None:
            break
        order.append(item.tenant_id)
    assert order[:2] == ["cs-support", "qa-defect"]
