"""待ち行列を回すワーカ。

本番は Kubernetes の Deployment で 4 ポッド（``settings.ingest_workers``）。
ここは同じプロセスで同期に回す —— 外部依存を持たない方針のため。
**呼び出しの形は同じ**なので、差し替えるのは待ち行列の実装だけで済む。
"""

from __future__ import annotations

from dataclasses import dataclass, field

from kotonoha.common import logging as applog
from kotonoha.ingest.models import SourceDocument
from kotonoha.ingest.queue import FairQueue, QueuedJob

log = applog.get(__name__)


@dataclass
class WorkerStats:
    """回した結果。運用の点検で読む。"""

    jobs: int = 0
    documents: int = 0
    failures: int = 0
    billed_chunks: int = 0
    cached_chunks: int = 0
    skipped: dict[str, int] = field(default_factory=dict)


class IngestWorker:
    """1 ジョブずつ取り出して通す。"""

    def __init__(self, queue: FairQueue, tracker, pipeline,
                 collection_service, source_store, meter=None) -> None:
        self._queue = queue
        self._tracker = tracker
        self._pipeline = pipeline
        self._collections = collection_service
        #: ジョブ ID から原文の配列を引く。本番はオブジェクトストア。
        self._sources = source_store
        #: 利用量の計測。**ここで数えないと取り込みが課金に乗らない。**
        self._meter = meter

    def run_once(self) -> bool:
        """1 ジョブ処理する。何も無ければ ``False``。"""
        item = self._queue.pop()
        if item is None:
            return False
        self._process(item)
        return True

    def drain(self, *, limit: int = 1_000) -> WorkerStats:
        """空になるまで回す。``limit`` は暴走よけ。"""
        stats = WorkerStats()
        while stats.jobs < limit and self.run_once():
            stats.jobs += 1
        return stats

    def _process(self, item: QueuedJob) -> None:
        sources: list[SourceDocument] = self._sources.take(item.job_id)
        collection = self._collections.get(item.collection_id)
        self._tracker.start(item.job_id)

        for seq_no, source in enumerate(sources):
            outcome = self._pipeline.run(source, collection)
            self._tracker.record(item.job_id, seq_no, outcome)
            if self._meter is not None:
                self._meter.record_embed(item.tenant_id,
                                         billed=outcome.billed_chunks,
                                         cached=outcome.cached_chunks)
            if outcome.status == "failed":
                log.warning("文書の取り込みに失敗 job=%s seq=%d reason=%s",
                            item.job_id, seq_no, outcome.error)

        self._tracker.close(item.job_id)
