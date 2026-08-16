"""ジョブの進捗を記録する。``t_ingest_job`` ``t_ingest_job_item`` と対。

**明細を残すのが要点である。** 5,000 件のうち 3 件が失敗したとき、
どの 3 件かが分からないと入れ直せない。``skip_reason`` も残す ——
「飛ばした」と「失敗した」を混ぜると、取り込めていない文書に気づけない。
"""

from __future__ import annotations

from kotonoha.common import ids
from kotonoha.common import logging as applog
from kotonoha.common.clock import now
from kotonoha.common.errors import NotFound
from kotonoha.ingest.models import IngestJob, JobItem, SourceDocument
from kotonoha.ingest.pipeline import Outcome

log = applog.get(__name__)


class JobTracker:
    """ジョブの起票と更新。"""

    def __init__(self, job_repo) -> None:
        self._jobs = job_repo

    def open(self, tenant_id: str, collection_id: str,
             sources: list[SourceDocument]) -> IngestJob:
        """受け付けたところ。明細も同時に起こす。"""
        job = IngestJob(
            job_id=ids.new_id(),
            collection_id=collection_id,
            tenant_id=tenant_id,
            total_count=len(sources),
        )
        self._jobs.save(job)
        self._jobs.save_items([
            JobItem(job_id=job.job_id, seq_no=index,
                    external_id=source.external_id)
            for index, source in enumerate(sources)
        ])
        log.info("取り込みを受け付けました job=%s tenant=%s 件数=%d",
                 job.job_id, tenant_id, len(sources))
        return job

    def start(self, job_id: str) -> IngestJob:
        job = self.get(job_id)
        job.status = "running"
        job.started_at = now()
        self._jobs.save(job)
        return job

    def record(self, job_id: str, seq_no: int, outcome: Outcome) -> IngestJob:
        """1 件ぶんの結果を書く。"""
        job = self.get(job_id)
        item = self._jobs.find_item(job_id, seq_no)
        if item is None:
            raise NotFound(f"ジョブの明細がありません: {job_id}#{seq_no}",
                           job_id=job_id, seq_no=seq_no)

        item.status = outcome.status
        item.skip_reason = outcome.skip_reason
        item.error_message = outcome.error
        item.document_id = outcome.document.document_id if outcome.document else None
        item.finished_at = now()
        self._jobs.save_item(item)

        if outcome.status == "failed":
            job.failed_count += 1
        else:
            job.done_count += 1
        job.chunk_count += outcome.billed_chunks
        job.cached_count += outcome.cached_chunks
        self._jobs.save(job)
        return job

    def close(self, job_id: str, *, error: str | None = None) -> IngestJob:
        """締める。**1 件でも失敗があれば ``failed`` にはしない** ——
        全部失敗したときだけ ``failed``、それ以外は ``succeeded`` として
        明細で内訳を見せる。
        """
        job = self.get(job_id)
        if error is not None:
            job.status = "failed"
            job.error_message = error
        elif job.failed_count and job.done_count == 0:
            job.status = "failed"
            job.error_message = "すべての文書で失敗しました"
        else:
            job.status = "succeeded"
        job.finished_at = now()
        self._jobs.save(job)
        log.info("取り込みを終えました job=%s status=%s 成功=%d 失敗=%d チャンク=%d",
                 job_id, job.status, job.done_count, job.failed_count, job.chunk_count)
        return job

    def cancel(self, job_id: str) -> IngestJob:
        job = self.get(job_id)
        if job.finished:
            return job
        job.status = "canceled"
        job.finished_at = now()
        self._jobs.save(job)
        return job

    def get(self, job_id: str) -> IngestJob:
        job = self._jobs.find(job_id)
        if job is None:
            raise NotFound(f"ジョブがありません: {job_id}", job_id=job_id)
        return job

    def failures(self, job_id: str) -> list[JobItem]:
        """失敗した明細だけ。入れ直しの手がかり。"""
        return [i for i in self._jobs.list_items(job_id) if i.status == "failed"]
