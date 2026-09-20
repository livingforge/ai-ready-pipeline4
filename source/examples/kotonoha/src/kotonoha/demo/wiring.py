"""依存を挿して組み立てる。**差し替え点はここ 1 か所。**

本番はここで ``store`` の SQL 実装と本物の Voyage クライアントを挿す。
資材ではメモリ実装と疑似のクライアントを挿す —— **業務層のコードは
どちらでも変わらない。**
"""

from __future__ import annotations

from dataclasses import dataclass

from kotonoha.billing.close import MonthlyClose
from kotonoha.billing.meter import UsageMeter
from kotonoha.billing.price import PriceBook
from kotonoha.common import audit
from kotonoha.demo import memory_store as mem
from kotonoha.embed.cache import EmbedCache
from kotonoha.embed.router import Router as EmbedRouter
from kotonoha.embed.selfhosted import SelfHostedProvider
from kotonoha.embed.service import EmbedService
from kotonoha.embed.voyage import VoyageProvider, VoyageReranker
from kotonoha.ingest.job import JobTracker
from kotonoha.ingest.pipeline import IngestPipeline
from kotonoha.ingest.queue import FairQueue
from kotonoha.ingest.service import IngestService
from kotonoha.ingest.worker import IngestWorker
from kotonoha.reindex.builder import IndexBuilder
from kotonoha.reindex.planner import Planner
from kotonoha.reindex.service import ReindexService
from kotonoha.reindex.switch import AliasSwitcher
from kotonoha.reindex.verifier import Verifier
from kotonoha.search.keyword import KeywordSearch
from kotonoha.search.rerank import RerankStage
from kotonoha.search.service import SearchService
from kotonoha.search.vector import VectorSearch
from kotonoha.tenant.apikey import ApiKeyService
from kotonoha.tenant.collection import CollectionService
from kotonoha.tenant.quota import QuotaChecker
from kotonoha.tenant.ratelimit import RateLimiter
from kotonoha.tenant.service import TenantService


@dataclass
class Services:
    """組み上がった一式。``api.app.build_app`` が受け取る。"""

    tenants: TenantService
    apikeys: ApiKeyService
    collections: CollectionService
    quota: QuotaChecker
    limiter: RateLimiter
    embed: EmbedService
    ingest: IngestService
    tracker: JobTracker
    worker: IngestWorker
    search: SearchService
    reindex: ReindexService
    meter: UsageMeter
    close: MonthlyClose
    prices: PriceBook
    # 保存先（テストと CLI が直に触る）
    documents: object
    chunks: object
    embeddings: object
    keywords: object
    audit: object


def build(*, quantize: bool = True, rerank: bool = True) -> Services:
    """一式を組む。

    :param quantize: int8 量子化を掛けるか（★ ADR-003 に無い）
    :param rerank: リランクを掛けるか（★ ADR-005 は第2次リリース）
    """
    tenant_repo = mem.MemoryTenantRepository()
    key_repo = mem.MemoryApiKeyRepository()
    collection_repo = mem.MemoryCollectionRepository()
    document_repo = mem.MemoryDocumentRepository()
    chunk_repo = mem.MemoryChunkRepository()
    vector_store = mem.MemoryVectorStore()
    keyword_store = mem.MemoryKeywordStore()
    cache_store = mem.MemoryCacheStore()
    job_repo = mem.MemoryJobRepository()
    reindex_repo = mem.MemoryReindexRepository()
    usage_repo = mem.MemoryUsageRepository()
    price_repo = mem.MemoryPriceRepository()
    audit_repo = mem.MemoryAuditRepository()
    source_store = mem.MemorySourceStore()
    object_store = mem.MemoryObjectStore()

    audit.bind(audit_repo)

    embed_router = EmbedRouter(external=VoyageProvider(),
                               internal=SelfHostedProvider())
    embed_service = EmbedService(embed_router, EmbedCache(cache_store),
                                 quantize=quantize)

    tenants = TenantService(tenant_repo)
    apikeys = ApiKeyService(key_repo)
    collections = CollectionService(collection_repo, tenant_repo)
    quota = QuotaChecker(usage_repo)
    limiter = RateLimiter()

    meter = UsageMeter(usage_repo)
    tracker = JobTracker(job_repo)
    pipeline = _KeywordAwarePipeline(embed_service, document_repo, chunk_repo,
                                     vector_store, keyword_store, object_store)
    queue = FairQueue()
    ingest = IngestService(collections, tenants, quota, embed_router,
                           queue, tracker, source_store)
    worker = IngestWorker(queue, tracker, pipeline, collections, source_store,
                          meter=meter)

    reranker = VoyageReranker() if rerank else None
    search = SearchService(
        collections,
        VectorSearch(vector_store, embed_service),
        KeywordSearch(keyword_store),
        RerankStage(reranker),
        chunk_repo, document_repo,
    )

    reindex = ReindexService(
        collections, Planner(chunk_repo),
        IndexBuilder(embed_service, chunk_repo, vector_store),
        Verifier(vector_store, chunk_repo, vector_store),
        AliasSwitcher(vector_store, keyword_store, collection_repo, vector_store),
        reindex_repo,
    )

    prices = PriceBook(price_repo)
    close = MonthlyClose(usage_repo, tenants, prices)

    return Services(
        tenants=tenants, apikeys=apikeys, collections=collections,
        quota=quota, limiter=limiter, embed=embed_service,
        ingest=ingest, tracker=tracker, worker=worker, search=search,
        reindex=reindex, meter=meter, close=close, prices=prices,
        documents=document_repo, chunks=chunk_repo, embeddings=vector_store,
        keywords=keyword_store, audit=audit_repo,
    )


class _KeywordAwarePipeline(IngestPipeline):
    """全文の索引にも入れる取り込み。

    本番は取り込みのたびに pgvector と OpenSearch の両方へ入れる。
    ``IngestPipeline`` はベクトルしか知らないので、ここで全文側を足す ——
    **本来はパイプライン自身が両方へ入れるべきで、この形は資材の都合**
    である。
    """

    def __init__(self, embed_service, document_repo, chunk_repo,
                 embedding_repo, keyword_store, object_store) -> None:
        super().__init__(embed_service, document_repo, chunk_repo,
                         embedding_repo, object_store)
        self._keywords = keyword_store

    def _run(self, source, collection):
        outcome = super()._run(source, collection)
        for chunk in outcome.chunks:
            self._keywords.index(collection.index_alias, chunk.chunk_id, chunk.body)
        return outcome
