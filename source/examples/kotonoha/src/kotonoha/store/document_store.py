"""``t_document`` ``t_chunk`` の読み書き。"""

from __future__ import annotations

import json

from kotonoha.ingest.models import Document, StoredChunk
from kotonoha.store.connection import Connection

_DOC_COLUMNS = (
    "document_id, collection_id, external_id, title, source_uri, "
    "content_type, content_hash, byte_size, chunk_count, metadata, "
    "ingested_at, deleted_at"
)

SELECT_DOCUMENT = f"SELECT {_DOC_COLUMNS} FROM t_document WHERE document_id = %s"

#: 再取り込みの判定で引く。**削除済みは除く。**
SELECT_DOCUMENT_BY_EXTERNAL = f"""
SELECT {_DOC_COLUMNS} FROM t_document
WHERE collection_id = %s AND external_id = %s AND deleted_at IS NULL
"""

SELECT_DOCUMENTS_BY_COLLECTION = f"""
SELECT {_DOC_COLUMNS} FROM t_document
WHERE collection_id = %s AND deleted_at IS NULL
ORDER BY ingested_at DESC
"""

UPSERT_DOCUMENT = f"""
INSERT INTO t_document ({_DOC_COLUMNS})
VALUES (%s, %s, %s, %s, %s, %s, %s, %s, %s, %s, %s, %s)
ON CONFLICT (document_id) DO UPDATE SET
    title        = EXCLUDED.title,
    content_hash = EXCLUDED.content_hash,
    byte_size    = EXCLUDED.byte_size,
    chunk_count  = EXCLUDED.chunk_count,
    metadata     = EXCLUDED.metadata,
    deleted_at   = EXCLUDED.deleted_at
"""

#: 論理削除。**行は消さない** —— 監査ログから辿れるようにする。
SOFT_DELETE_DOCUMENT = """
UPDATE t_document SET deleted_at = CURRENT_TIMESTAMP WHERE document_id = %s
"""

_CHUNK_COLUMNS = (
    "chunk_id, document_id, collection_id, seq_no, body, token_count, "
    "heading_path, char_start, char_end"
)

SELECT_CHUNK = f"SELECT {_CHUNK_COLUMNS} FROM t_chunk WHERE chunk_id = %s"

SELECT_CHUNKS_BY_DOCUMENT = (
    f"SELECT {_CHUNK_COLUMNS} FROM t_chunk WHERE document_id = %s ORDER BY seq_no")

SELECT_CHUNKS_BY_COLLECTION = (
    f"SELECT {_CHUNK_COLUMNS} FROM t_chunk WHERE collection_id = %s "
    f"ORDER BY document_id, seq_no")

INSERT_CHUNK = f"""
INSERT INTO t_chunk ({_CHUNK_COLUMNS})
VALUES (%s, %s, %s, %s, %s, %s, %s, %s, %s)
ON CONFLICT (document_id, seq_no) DO UPDATE SET
    body        = EXCLUDED.body,
    token_count = EXCLUDED.token_count
"""

#: **チャンクは物理削除する。** ベクトルは ON DELETE CASCADE で一緒に消える。
DELETE_CHUNKS_BY_DOCUMENT = "DELETE FROM t_chunk WHERE document_id = %s"

COUNT_CHUNKS_IN_COLLECTION = (
    "SELECT COUNT(*) AS n FROM t_chunk WHERE collection_id = %s")


class SqlDocumentRepository:
    """``t_document``。"""

    def __init__(self, conn: Connection) -> None:
        self._conn = conn

    def find(self, document_id: str) -> Document | None:
        row = self._conn.fetch_one(SELECT_DOCUMENT, (document_id,))
        return _to_document(row) if row else None

    def find_by_external_id(self, collection_id: str,
                            external_id: str) -> Document | None:
        row = self._conn.fetch_one(SELECT_DOCUMENT_BY_EXTERNAL,
                                   (collection_id, external_id))
        return _to_document(row) if row else None

    def list_by_collection(self, collection_id: str) -> list[Document]:
        rows = self._conn.fetch_all(SELECT_DOCUMENTS_BY_COLLECTION, (collection_id,))
        return [_to_document(r) for r in rows]

    def save(self, document: Document) -> None:
        self._conn.execute(UPSERT_DOCUMENT, (
            document.document_id, document.collection_id, document.external_id,
            document.title, document.source_uri, document.content_type,
            document.content_hash, document.byte_size, document.chunk_count,
            json.dumps(document.metadata, ensure_ascii=False),
            document.ingested_at, document.deleted_at,
        ))

    def delete(self, document_id: str) -> None:
        self._conn.execute(SOFT_DELETE_DOCUMENT, (document_id,))


class SqlChunkRepository:
    """``t_chunk``。"""

    def __init__(self, conn: Connection) -> None:
        self._conn = conn

    def find(self, chunk_id: str) -> StoredChunk | None:
        row = self._conn.fetch_one(SELECT_CHUNK, (chunk_id,))
        return _to_chunk(row) if row else None

    def list_by_document(self, document_id: str) -> list[StoredChunk]:
        rows = self._conn.fetch_all(SELECT_CHUNKS_BY_DOCUMENT, (document_id,))
        return [_to_chunk(r) for r in rows]

    def list_by_collection(self, collection_id: str) -> list[StoredChunk]:
        rows = self._conn.fetch_all(SELECT_CHUNKS_BY_COLLECTION, (collection_id,))
        return [_to_chunk(r) for r in rows]

    def save_many(self, chunks: list[StoredChunk]) -> None:
        self._conn.execute_many(INSERT_CHUNK, [
            (c.chunk_id, c.document_id, c.collection_id, c.seq_no, c.body,
             c.token_count, c.heading_path, c.char_start, c.char_end)
            for c in chunks
        ])

    def delete_by_document(self, document_id: str) -> int:
        self._conn.execute(DELETE_CHUNKS_BY_DOCUMENT, (document_id,))
        return 0

    def count_in_collection(self, collection_id: str) -> int:
        row = self._conn.fetch_one(COUNT_CHUNKS_IN_COLLECTION, (collection_id,))
        return int(row["n"]) if row else 0


def _to_document(row: dict) -> Document:
    metadata = row["metadata"]
    if isinstance(metadata, str):
        metadata = json.loads(metadata)
    return Document(
        document_id=row["document_id"], collection_id=row["collection_id"],
        title=row["title"] or "", source_uri=row["source_uri"],
        content_type=row["content_type"], content_hash=row["content_hash"],
        byte_size=row["byte_size"], external_id=row["external_id"],
        chunk_count=row["chunk_count"], metadata=metadata or {},
        ingested_at=row["ingested_at"], deleted_at=row["deleted_at"],
    )


def _to_chunk(row: dict) -> StoredChunk:
    return StoredChunk(
        chunk_id=row["chunk_id"], document_id=row["document_id"],
        collection_id=row["collection_id"], seq_no=row["seq_no"],
        body=row["body"], token_count=row["token_count"],
        char_start=row["char_start"], char_end=row["char_end"],
        heading_path=row["heading_path"] or "",
    )
