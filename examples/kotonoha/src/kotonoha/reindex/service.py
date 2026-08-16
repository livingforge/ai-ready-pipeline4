"""再インデックスの入口。

**API では公開していない。** 運用が CLI から回す（``demo.cli reindex``）。
利用部門が勝手に走らせると社内 GPU が埋まるためで、その判断は
``docs/adr/0007-reindex-strategy.md`` にある。
"""

from __future__ import annotations

from kotonoha.common import ids
from kotonoha.common import logging as applog
from kotonoha.common.clock import now
from kotonoha.common.errors import IndexBusy, NotFound
from kotonoha.reindex.models import ReindexJob

log = applog.get(__name__)


class ReindexService:
    """再インデックスを回す。"""

    def __init__(self, collection_service, planner, builder, verifier,
                 switcher, reindex_repo) -> None:
        self._collections = collection_service
        self._planner = planner
        self._builder = builder
        self._verifier = verifier
        self._switcher = switcher
        self._jobs = reindex_repo

    def plan(self, collection_id: str, to_model: str):
        """見積もる。まだ何もしない。"""
        collection = self._collections.get(collection_id)
        return self._planner.plan(collection, to_model)

    def start(self, collection_id: str, to_model: str) -> ReindexJob:
        """始める。コレクションを再構築中にして取り込みを止める。

        :raises IndexBusy: 既に走っている再インデックスがある
        """
        collection = self._collections.get(collection_id)
        running = self._jobs.find_running(collection_id)
        if running is not None:
            raise IndexBusy(f"既に再インデックスが走っています: {running.job_id}",
                            collection_id=collection_id, job_id=running.job_id)

        plan = self._planner.plan(collection, to_model)
        generation = self._jobs.next_generation(collection_id)
        job = ReindexJob(
            job_id=ids.new_id(),
            collection_id=collection_id,
            from_model=collection.embed_model,
            to_model=plan.to_model,
            from_index=self._current_index(collection),
            to_index=ids.index_name(collection_id, generation),
            status="building",
            total_chunks=plan.total_chunks,
        )
        self._jobs.save(job)
        self._collections.mark_rebuilding(collection_id)
        log.info("再インデックスを始めました job=%s %s -> %s 件数=%d 見積=%d分",
                 job.job_id, job.from_model, job.to_model,
                 plan.total_chunks, plan.estimated_minutes)
        return job

    def step(self, job_id: str, *, limit: int | None = None) -> ReindexJob:
        """一区切りぶん進める。バッチが繰り返し呼ぶ。"""
        job = self.get(job_id)
        collection = self._collections.get(job.collection_id)
        progress = self._builder.build(job, collection,
                                       resume_from=job.done_chunks, limit=limit)
        job.done_chunks += progress.done
        if job.done_chunks >= job.total_chunks:
            job.status = "verifying"
        self._jobs.save(job)
        return job

    def finish(self, job_id: str, *, force_by: str | None = None) -> ReindexJob:
        """突合して張り替え、コレクションを戻す。

        :param force_by: 突合を飛ばすときの承認者
        """
        job = self.get(job_id)
        collection = self._collections.get(job.collection_id)

        if force_by:
            job = self._switcher.force_switch(job, approved_by=force_by)
        else:
            verification = self._verifier.verify(job, collection)
            job = self._switcher.switch(job, verification)

        job.finished_at = now()
        self._jobs.save(job)
        self._collections.mark_active(job.collection_id)
        return job

    def abort(self, job_id: str, reason: str) -> ReindexJob:
        """やめる。**新インデックスは残す**（調査のため）。"""
        job = self.get(job_id)
        job.status = "failed"
        job.error_message = reason
        job.finished_at = now()
        self._jobs.save(job)
        self._collections.mark_active(job.collection_id)
        log.warning("再インデックスを中止しました job=%s reason=%s", job_id, reason)
        return job

    def get(self, job_id: str) -> ReindexJob:
        job = self._jobs.find(job_id)
        if job is None:
            raise NotFound(f"再インデックスのジョブがありません: {job_id}", job_id=job_id)
        return job

    def _current_index(self, collection) -> str:
        latest = self._jobs.latest_switched(collection.collection_id)
        return latest.to_index if latest else ids.index_name(collection.collection_id, 1)
