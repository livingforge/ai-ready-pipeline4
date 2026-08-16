"""取り込みの値。``t_document`` ``t_ingest_job`` と対。"""

from __future__ import annotations

from dataclasses import dataclass, field
from datetime import datetime

from kotonoha.common.clock import now


@dataclass
class SourceDocument:
    """取り込みを頼まれた 1 件。まだ切っていない。"""

    external_id: str | None
    title: str
    content: str
    content_type: str = "text/plain"
    source_uri: str = ""
    metadata: dict = field(default_factory=dict)


@dataclass
class Document:
    """取り込み済みの文書。``t_document`` と対。"""

    document_id: str
    collection_id: str
    title: str
    source_uri: str
    content_type: str
    content_hash: str
    byte_size: int
    external_id: str | None = None
    chunk_count: int = 0
    metadata: dict = field(default_factory=dict)
    ingested_at: datetime = field(default_factory=now)
    deleted_at: datetime | None = None

    @property
    def alive(self) -> bool:
        return self.deleted_at is None


@dataclass
class StoredChunk:
    """格納したチャンク。``t_chunk`` と対。"""

    chunk_id: str
    document_id: str
    collection_id: str
    seq_no: int
    body: str
    token_count: int
    char_start: int
    char_end: int
    heading_path: str = ""


@dataclass
class IngestJob:
    """取り込みジョブ。``t_ingest_job`` と対。"""

    job_id: str
    collection_id: str
    tenant_id: str
    status: str = "queued"
    total_count: int = 0
    done_count: int = 0
    failed_count: int = 0
    chunk_count: int = 0
    cached_count: int = 0
    error_message: str | None = None
    queued_at: datetime = field(default_factory=now)
    started_at: datetime | None = None
    finished_at: datetime | None = None

    @property
    def finished(self) -> bool:
        return self.status in ("succeeded", "failed", "canceled")

    @property
    def progress(self) -> float:
        if not self.total_count:
            return 0.0
        return (self.done_count + self.failed_count) / self.total_count


@dataclass
class JobItem:
    """ジョブの明細 1 行。``t_ingest_job_item`` と対。"""

    job_id: str
    seq_no: int
    external_id: str | None = None
    document_id: str | None = None
    status: str = "pending"          # pending/done/skipped/failed
    skip_reason: str | None = None   # same_hash / empty / unsupported_type
    error_message: str | None = None
    finished_at: datetime | None = None
