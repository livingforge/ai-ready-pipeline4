"""1 文書ぶんの流れ。抽出 → 正規化 → 分割 → 埋め込み → 格納。

**ここが取り込みの本体である。** ジョブの受付（``service``）や待ち行列
（``queue`` ``worker``）は運び方の話で、何をするかはこの 1 本に書いてある。

途中で落ちたときは**その文書だけ**を失敗にして次へ進む。1 件の壊れた PDF で
5,000 件のジョブ全体を止めない、というのが運用と決めたことである
（``docs/runbook/ingest.md``）。
"""

from __future__ import annotations

from dataclasses import dataclass, field

from kotonoha.common import ids
from kotonoha.common import logging as applog
from kotonoha.common.errors import KotonohaError
from kotonoha.embed.models import EmbedRequest
from kotonoha.ingest import dedupe, normalizer
from kotonoha.ingest.chunker import chunk_text
from kotonoha.ingest.extract import extract
from kotonoha.ingest.models import Document, SourceDocument, StoredChunk

log = applog.get(__name__)


@dataclass
class Outcome:
    """1 文書を通した結果。"""

    status: str                      # done / skipped / failed
    document: Document | None = None
    chunks: list[StoredChunk] = field(default_factory=list)
    billed_chunks: int = 0
    cached_chunks: int = 0
    skip_reason: str | None = None
    error: str | None = None
    notes: dict = field(default_factory=dict)


class IngestPipeline:
    """1 文書を取り込む。"""

    def __init__(self, embed_service, document_repo, chunk_repo,
                 embedding_repo, object_store=None) -> None:
        self._embed = embed_service
        self._documents = document_repo
        self._chunks = chunk_repo
        self._embeddings = embedding_repo
        self._objects = object_store

    def run(self, source: SourceDocument, collection) -> Outcome:
        """通す。**例外は外へ出さない** —— 結果に畳んで返す。"""
        try:
            return self._run(source, collection)
        except KotonohaError as exc:
            log.warning("取り込みに失敗しました external_id=%s reason=%s",
                        source.external_id, exc.message)
            return Outcome(status="failed", error=exc.message,
                           notes=dict(getattr(exc, "detail", {})))
        except Exception as exc:                     # 想定外も 1 件で止める
            log.exception("取り込みで想定外の失敗 external_id=%s", source.external_id)
            return Outcome(status="failed", error=str(exc))

    def _run(self, source: SourceDocument, collection) -> Outcome:
        extracted = extract(source.content, source.content_type)
        body = normalizer.normalize(extracted.text)
        if normalizer.is_empty(body):
            return Outcome(status="skipped", skip_reason="empty",
                           notes=extracted.notes)

        prepared = SourceDocument(
            external_id=source.external_id,
            title=normalizer.normalize_title(source.title),
            content=body,
            content_type=source.content_type,
            source_uri=source.source_uri,
            metadata=source.metadata,
        )

        existing = None
        if prepared.external_id:
            existing = self._documents.find_by_external_id(
                collection.collection_id, prepared.external_id)
        decision = dedupe.decide(prepared, existing)

        if decision.action == "skip":
            return Outcome(status="skipped", skip_reason=decision.reason,
                           document=decision.existing, notes=extracted.notes)
        if decision.action == "metadata_only":
            decision.existing.metadata = prepared.metadata
            self._documents.save(decision.existing)
            return Outcome(status="done", document=decision.existing,
                           skip_reason=decision.reason, notes=extracted.notes)

        if decision.existing is not None:
            self._purge(decision.existing)

        pieces = chunk_text(prepared.content)
        if not pieces:
            return Outcome(status="skipped", skip_reason="empty",
                           notes=extracted.notes)

        result = self._embed.embed(EmbedRequest(
            texts=[p.body for p in pieces],
            model=collection.embed_model,
            classification=collection.classification,
            input_type="document",
            tenant_id=collection.tenant_id,
        ))

        document = Document(
            document_id=ids.new_id(),
            collection_id=collection.collection_id,
            title=prepared.title,
            source_uri=prepared.source_uri or self._store_source(prepared),
            content_type=prepared.content_type,
            content_hash=dedupe.content_hash(prepared),
            byte_size=len(prepared.content.encode("utf-8")),
            external_id=prepared.external_id,
            chunk_count=len(pieces),
            metadata=prepared.metadata,
        )
        self._documents.save(document)

        stored: list[StoredChunk] = []
        pairs: list[tuple[str, object]] = []
        for piece, vector in zip(pieces, result.vectors):
            chunk = StoredChunk(
                chunk_id=ids.new_id(),
                document_id=document.document_id,
                collection_id=collection.collection_id,
                seq_no=piece.seq_no,
                body=piece.body,
                token_count=piece.token_count,
                char_start=piece.char_start,
                char_end=piece.char_end,
                heading_path=piece.heading_path,
            )
            stored.append(chunk)
            pairs.append((chunk.chunk_id, vector))
        self._chunks.save_many(stored)
        self._embeddings.save_many(collection.index_alias, pairs)

        return Outcome(
            status="done", document=document, chunks=stored,
            billed_chunks=result.billed_count,
            cached_chunks=result.cached_count,
            notes=extracted.notes,
        )

    def _purge(self, document: Document) -> None:
        """古い版のチャンクとベクトルを消す。"""
        for chunk in self._chunks.list_by_document(document.document_id):
            self._embeddings.delete_by_chunk(chunk.chunk_id)
        self._chunks.delete_by_document(document.document_id)

    def _store_source(self, source: SourceDocument) -> str:
        """原文をオブジェクトストアへ置き、その位置を返す。"""
        if self._objects is None:
            return f"inline://{source.external_id or 'anonymous'}"
        return self._objects.put(source.content.encode("utf-8"),
                                 content_type=source.content_type)
