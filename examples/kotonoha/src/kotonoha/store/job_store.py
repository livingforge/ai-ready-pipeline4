"""``t_ingest_job`` ``t_ingest_job_item`` ``t_reindex_job`` の読み書き。"""

from __future__ import annotations

from kotonoha.ingest.models import IngestJob, JobItem
from kotonoha.reindex.models import ReindexJob
from kotonoha.store.connection import Connection

_JOB_COLUMNS = (
    "job_id, collection_id, tenant_id, status, total_count, done_count, "
    "failed_count, chunk_count, cached_count, error_message, "
    "queued_at, started_at, finished_at"
)

SELECT_JOB = f"SELECT {_JOB_COLUMNS} FROM t_ingest_job WHERE job_id = %s"

SELECT_JOBS_BY_TENANT = (
    f"SELECT {_JOB_COLUMNS} FROM t_ingest_job WHERE tenant_id = %s "
    f"ORDER BY queued_at DESC LIMIT %s")

UPSERT_JOB = f"""
INSERT INTO t_ingest_job ({_JOB_COLUMNS})
VALUES (%s, %s, %s, %s, %s, %s, %s, %s, %s, %s, %s, %s, %s)
ON CONFLICT (job_id) DO UPDATE SET
    status        = EXCLUDED.status,
    done_count    = EXCLUDED.done_count,
    failed_count  = EXCLUDED.failed_count,
    chunk_count   = EXCLUDED.chunk_count,
    cached_count  = EXCLUDED.cached_count,
    error_message = EXCLUDED.error_message,
    started_at    = EXCLUDED.started_at,
    finished_at   = EXCLUDED.finished_at
"""

_ITEM_COLUMNS = ("job_id, seq_no, external_id, document_id, status, "
                 "skip_reason, error_message, finished_at")

SELECT_ITEM = (f"SELECT {_ITEM_COLUMNS} FROM t_ingest_job_item "
               f"WHERE job_id = %s AND seq_no = %s")

SELECT_ITEMS = (f"SELECT {_ITEM_COLUMNS} FROM t_ingest_job_item "
                f"WHERE job_id = %s ORDER BY seq_no")

UPSERT_ITEM = f"""
INSERT INTO t_ingest_job_item ({_ITEM_COLUMNS})
VALUES (%s, %s, %s, %s, %s, %s, %s, %s)
ON CONFLICT (job_id, seq_no) DO UPDATE SET
    document_id   = EXCLUDED.document_id,
    status        = EXCLUDED.status,
    skip_reason   = EXCLUDED.skip_reason,
    error_message = EXCLUDED.error_message,
    finished_at   = EXCLUDED.finished_at
"""

_REINDEX_COLUMNS = (
    "job_id, collection_id, from_model, to_model, from_index, to_index, "
    "status, total_chunks, done_chunks, switched_at, old_dropped_at, "
    "queued_at, finished_at"
)

SELECT_REINDEX = f"SELECT {_REINDEX_COLUMNS} FROM t_reindex_job WHERE job_id = %s"

SELECT_REINDEX_RUNNING = f"""
SELECT {_REINDEX_COLUMNS} FROM t_reindex_job
WHERE collection_id = %s AND status IN ('queued', 'building', 'verifying')
ORDER BY queued_at DESC LIMIT 1
"""

SELECT_REINDEX_LATEST_SWITCHED = f"""
SELECT {_REINDEX_COLUMNS} FROM t_reindex_job
WHERE collection_id = %s AND switched_at IS NOT NULL
ORDER BY switched_at DESC LIMIT 1
"""

#: 保持期間（7 日）を過ぎて旧インデックスを消せるもの。
SELECT_REINDEX_DROPPABLE = f"""
SELECT {_REINDEX_COLUMNS} FROM t_reindex_job
WHERE switched_at IS NOT NULL
  AND old_dropped_at IS NULL
  AND switched_at < CURRENT_TIMESTAMP - INTERVAL '7 days'
"""

COUNT_GENERATIONS = ("SELECT COUNT(*) AS n FROM t_reindex_job "
                     "WHERE collection_id = %s")

UPSERT_REINDEX = f"""
INSERT INTO t_reindex_job ({_REINDEX_COLUMNS})
VALUES (%s, %s, %s, %s, %s, %s, %s, %s, %s, %s, %s, %s, %s)
ON CONFLICT (job_id) DO UPDATE SET
    status         = EXCLUDED.status,
    done_chunks    = EXCLUDED.done_chunks,
    switched_at    = EXCLUDED.switched_at,
    old_dropped_at = EXCLUDED.old_dropped_at,
    finished_at    = EXCLUDED.finished_at
"""


class SqlJobRepository:
    """``t_ingest_job`` ``t_ingest_job_item``。"""

    def __init__(self, conn: Connection) -> None:
        self._conn = conn

    def find(self, job_id: str) -> IngestJob | None:
        row = self._conn.fetch_one(SELECT_JOB, (job_id,))
        return _to_job(row) if row else None

    def list_by_tenant(self, tenant_id: str, limit: int = 50) -> list[IngestJob]:
        rows = self._conn.fetch_all(SELECT_JOBS_BY_TENANT, (tenant_id, limit))
        return [_to_job(r) for r in rows]

    def save(self, job: IngestJob) -> None:
        self._conn.execute(UPSERT_JOB, (
            job.job_id, job.collection_id, job.tenant_id, job.status,
            job.total_count, job.done_count, job.failed_count, job.chunk_count,
            job.cached_count, job.error_message, job.queued_at,
            job.started_at, job.finished_at,
        ))

    def find_item(self, job_id: str, seq_no: int) -> JobItem | None:
        row = self._conn.fetch_one(SELECT_ITEM, (job_id, seq_no))
        return _to_item(row) if row else None

    def list_items(self, job_id: str) -> list[JobItem]:
        return [_to_item(r) for r in self._conn.fetch_all(SELECT_ITEMS, (job_id,))]

    def save_item(self, item: JobItem) -> None:
        self._conn.execute(UPSERT_ITEM, _item_params(item))

    def save_items(self, items: list[JobItem]) -> None:
        self._conn.execute_many(UPSERT_ITEM, [_item_params(i) for i in items])


class SqlReindexRepository:
    """``t_reindex_job``。"""

    def __init__(self, conn: Connection) -> None:
        self._conn = conn

    def find(self, job_id: str) -> ReindexJob | None:
        row = self._conn.fetch_one(SELECT_REINDEX, (job_id,))
        return _to_reindex(row) if row else None

    def find_running(self, collection_id: str) -> ReindexJob | None:
        row = self._conn.fetch_one(SELECT_REINDEX_RUNNING, (collection_id,))
        return _to_reindex(row) if row else None

    def latest_switched(self, collection_id: str) -> ReindexJob | None:
        row = self._conn.fetch_one(SELECT_REINDEX_LATEST_SWITCHED, (collection_id,))
        return _to_reindex(row) if row else None

    def next_generation(self, collection_id: str) -> int:
        row = self._conn.fetch_one(COUNT_GENERATIONS, (collection_id,))
        return (int(row["n"]) if row else 0) + 2

    def list_by_collection(self, collection_id: str) -> list[ReindexJob]:
        return []

    def list_droppable(self) -> list[ReindexJob]:
        return [_to_reindex(r) for r in self._conn.fetch_all(SELECT_REINDEX_DROPPABLE)]

    def save(self, job: ReindexJob) -> None:
        self._conn.execute(UPSERT_REINDEX, (
            job.job_id, job.collection_id, job.from_model, job.to_model,
            job.from_index, job.to_index, job.status, job.total_chunks,
            job.done_chunks, job.switched_at, job.old_dropped_at,
            job.queued_at, job.finished_at,
        ))


def _item_params(item: JobItem):
    return (item.job_id, item.seq_no, item.external_id, item.document_id,
            item.status, item.skip_reason, item.error_message, item.finished_at)


def _to_job(row: dict) -> IngestJob:
    return IngestJob(
        job_id=row["job_id"], collection_id=row["collection_id"],
        tenant_id=row["tenant_id"], status=row["status"],
        total_count=row["total_count"], done_count=row["done_count"],
        failed_count=row["failed_count"], chunk_count=row["chunk_count"],
        cached_count=row["cached_count"], error_message=row["error_message"],
        queued_at=row["queued_at"], started_at=row["started_at"],
        finished_at=row["finished_at"],
    )


def _to_item(row: dict) -> JobItem:
    return JobItem(
        job_id=row["job_id"], seq_no=row["seq_no"],
        external_id=row["external_id"], document_id=row["document_id"],
        status=row["status"], skip_reason=row["skip_reason"],
        error_message=row["error_message"], finished_at=row["finished_at"],
    )


def _to_reindex(row: dict) -> ReindexJob:
    return ReindexJob(
        job_id=row["job_id"], collection_id=row["collection_id"],
        from_model=row["from_model"], to_model=row["to_model"],
        from_index=row["from_index"], to_index=row["to_index"],
        status=row["status"], total_chunks=row["total_chunks"],
        done_chunks=row["done_chunks"], switched_at=row["switched_at"],
        old_dropped_at=row["old_dropped_at"], queued_at=row["queued_at"],
        finished_at=row["finished_at"],
    )
