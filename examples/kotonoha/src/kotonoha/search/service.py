"""検索の入口。``POST /v1/collections/{id}/search`` が呼ぶ。

流れ:

    正規化（query）
      → メタデータで候補を絞る（filters）
      → ベクトルと全文を並行に引く（vector / keyword）
      → 順位を融合する（fusion）★ k=60
      → 並べ直す（rerank）★ ADR-005 と食い違い
      → 本文を引いて切り出す（highlight）
      → 監査ログへ残す（本文は残さない）

**検索は上限に掛からない。** 取り込みだけが月間上限の対象で、検索は
止めない（業務が止まると困る、という利用申請時の合意）。
"""

from __future__ import annotations

from kotonoha.common import logging as applog
from kotonoha.common.audit import AuditEntry, record
from kotonoha.common.clock import now
from kotonoha.common.errors import IndexBusy, NotFound
from kotonoha.common.settings import SETTINGS
from kotonoha.search import filters as filterlib
from kotonoha.search import fusion, highlight
from kotonoha.search import query as querylib
from kotonoha.search.models import Hit, SearchQuery, SearchResult

log = applog.get(__name__)


class SearchService:
    """引く。"""

    def __init__(self, collection_service, vector_search, keyword_search,
                 rerank_stage, chunk_repo, document_repo) -> None:
        self._collections = collection_service
        self._vector = vector_search
        self._keyword = keyword_search
        self._rerank = rerank_stage
        self._chunks = chunk_repo
        self._documents = document_repo

    def search(self, request: SearchQuery) -> SearchResult:
        """検索する。

        :raises InvalidInput: 検索語が空／長すぎる／絞り込みが不正
        :raises NotFound: コレクションが無い（他テナントのものも同じ扱い）
        :raises IndexBusy: 削除済みのコレクション
        """
        started = now()
        collection = self._collections.get(request.collection_id,
                                           tenant_id=request.tenant_id)
        if not collection.readable:
            raise IndexBusy("このコレクションは検索できません",
                            collection_id=collection.collection_id,
                            status=collection.status)

        text = querylib.normalize(request.text)
        top_k = min(max(1, request.top_k), SETTINGS.max_top_k)
        conditions = filterlib.parse(request.filters)

        scoped: list[str] | None = None
        if conditions:
            candidates = self._chunks.list_by_collection(collection.collection_id)
            documents = {d.document_id: d
                         for d in self._documents.list_by_collection(
                             collection.collection_id)}
            kept = [c for c in candidates
                    if _document_matches(conditions, documents.get(c.document_id))]
            scoped = [c.chunk_id for c in kept]
            if not scoped:
                return SearchResult(hits=[], total_candidates=0,
                                    elapsed_ms=_elapsed(started))

        ranked = {
            "vector": self._vector.search(text, collection, top_k, chunk_ids=scoped),
            "keyword": self._keyword.search(text, collection, top_k, chunk_ids=scoped),
        }
        fused = fusion.fuse(ranked, top_k=None)

        bodies = self._bodies([f.chunk_id for f in fused[:SETTINGS.rerank_candidates]])
        outcome = self._rerank.apply(
            request.text, fused, bodies, top_k=top_k,
            classification=collection.classification, enabled=request.rerank)

        hits = self._build_hits(outcome.items, request, text)
        result = SearchResult(
            hits=hits,
            total_candidates=len(fused),
            elapsed_ms=_elapsed(started),
            reranked=outcome.applied,
            sources=[name for name, items in ranked.items() if items],
        )

        record(AuditEntry(
            tenant_id=request.tenant_id,
            operation="search",
            classification=collection.classification,
            status_code=200,
            collection_id=collection.collection_id,
            embed_model=collection.embed_model,
            item_count=len(hits),
            elapsed_ms=result.elapsed_ms,
        ), query_text=request.text)

        log.info("検索しました tenant=%s collection=%s 候補=%d 返却=%d rerank=%s %dms",
                 request.tenant_id, collection.collection_id,
                 result.total_candidates, len(hits), outcome.applied,
                 result.elapsed_ms)
        return result

    def _build_hits(self, items, request: SearchQuery, normalized: str) -> list[Hit]:
        hits: list[Hit] = []
        for item in items:
            chunk = self._chunks.find(item.chunk_id)
            if chunk is None:
                # 検索の途中で消された。**落とすだけで失敗にはしない。**
                log.debug("チャンクが見つかりません chunk=%s", item.chunk_id)
                continue
            document = self._documents.find(chunk.document_id)
            snippet, _ = highlight.snippet(chunk.body, request.text)
            hits.append(Hit(
                chunk_id=chunk.chunk_id,
                document_id=chunk.document_id,
                score=round(item.score, 6),
                body=chunk.body,
                title=document.title if document else "",
                heading_path=chunk.heading_path,
                seq_no=chunk.seq_no,
                snippet=snippet,
                metadata=document.metadata if document else {},
                detail=fusion.explain(item) if request.explain else None,
            ))
        return hits

    def _bodies(self, chunk_ids: list[str]) -> dict[str, str]:
        bodies: dict[str, str] = {}
        for chunk_id in chunk_ids:
            chunk = self._chunks.find(chunk_id)
            if chunk is not None:
                bodies[chunk_id] = chunk.body
        return bodies


def _document_matches(conditions, document) -> bool:
    if document is None:
        return False
    return all(c.matches(document.metadata or {}) for c in conditions)


def _elapsed(started) -> int:
    return int((now() - started).total_seconds() * 1000)
