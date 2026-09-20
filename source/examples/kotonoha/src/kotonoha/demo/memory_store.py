"""すべての保存先のメモリ実装。**本番では使わない。**

``store/`` の SQL 実装と同じ約束（``*/repository.py``）に従う。
検索は総当たり —— pgvector の HNSW も OpenSearch の BM25 も無いので、
コサイン類似度と語の重なりを全件で計算する。**件数が増えたら遅い**が、
資材として動かすぶんには足りる。
"""

from __future__ import annotations

import math
from collections import defaultdict
from datetime import date

from kotonoha.billing.models import DailyUsage, MonthlyUsage, Price
from kotonoha.common.audit import AuditEntry
from kotonoha.common.clock import now
from kotonoha.embed.cache import CacheEntry
from kotonoha.embed.models import Vector
from kotonoha.ingest.models import Document, IngestJob, JobItem, StoredChunk
from kotonoha.reindex.models import ReindexJob
from kotonoha.search import query as querylib
from kotonoha.search.keyword import KeywordHit
from kotonoha.search.vector import VectorHit
from kotonoha.tenant.models import ApiKey, Collection, Tenant


class MemoryTenantRepository:
    def __init__(self) -> None:
        self._rows: dict[str, Tenant] = {}

    def find(self, tenant_id): return self._rows.get(tenant_id)
    def save(self, tenant): self._rows[tenant.tenant_id] = tenant
    def all(self): return list(self._rows.values())


class MemoryApiKeyRepository:
    def __init__(self) -> None:
        self._rows: dict[str, ApiKey] = {}

    def find(self, key_id): return self._rows.get(key_id)
    def save(self, key): self._rows[key.key_id] = key
    def all(self): return list(self._rows.values())

    def list_by_tenant(self, tenant_id):
        return [k for k in self._rows.values() if k.tenant_id == tenant_id]


class MemoryCollectionRepository:
    def __init__(self) -> None:
        self._rows: dict[str, Collection] = {}

    def find(self, collection_id): return self._rows.get(collection_id)
    def save(self, collection): self._rows[collection.collection_id] = collection
    def all(self): return list(self._rows.values())

    def find_by_name(self, tenant_id, name):
        for row in self._rows.values():
            if row.tenant_id == tenant_id and row.collection_name == name \
                    and row.status != "D":
                return row
        return None

    def list_by_tenant(self, tenant_id):
        return [c for c in self._rows.values() if c.tenant_id == tenant_id]


class MemoryDocumentRepository:
    def __init__(self) -> None:
        self._rows: dict[str, Document] = {}

    def find(self, document_id): return self._rows.get(document_id)
    def save(self, document): self._rows[document.document_id] = document

    def find_by_external_id(self, collection_id, external_id):
        for row in self._rows.values():
            if (row.collection_id == collection_id
                    and row.external_id == external_id and row.alive):
                return row
        return None

    def list_by_collection(self, collection_id):
        return [d for d in self._rows.values()
                if d.collection_id == collection_id and d.alive]

    def delete(self, document_id):
        row = self._rows.get(document_id)
        if row is not None:
            row.deleted_at = now()


class MemoryChunkRepository:
    def __init__(self) -> None:
        self._rows: dict[str, StoredChunk] = {}

    def find(self, chunk_id): return self._rows.get(chunk_id)

    def save_many(self, chunks):
        for chunk in chunks:
            self._rows[chunk.chunk_id] = chunk

    def list_by_document(self, document_id):
        return sorted((c for c in self._rows.values()
                       if c.document_id == document_id),
                      key=lambda c: c.seq_no)

    def list_by_collection(self, collection_id):
        return sorted((c for c in self._rows.values()
                       if c.collection_id == collection_id),
                      key=lambda c: (c.document_id, c.seq_no))

    def delete_by_document(self, document_id):
        victims = [c.chunk_id for c in self._rows.values()
                   if c.document_id == document_id]
        for chunk_id in victims:
            del self._rows[chunk_id]
        return len(victims)

    def count_in_collection(self, collection_id):
        return len(self.list_by_collection(collection_id))


class MemoryVectorStore:
    """ベクトルの置き場と検索。**総当たりでコサインを取る。**"""

    def __init__(self) -> None:
        self._rows: dict[tuple[str, str], Vector] = {}
        self._aliases: dict[str, str] = {}

    # ── 書き込み ──────────────────────────────────────
    def save(self, chunk_id, index_name, vector):
        self._rows[(chunk_id, self._resolve(index_name))] = vector

    def upsert(self, index_name, chunk_id, vector):
        self.save(chunk_id, index_name, vector)

    def save_many(self, index_name, items):
        for chunk_id, vector in items:
            self.save(chunk_id, index_name, vector)

    def find(self, chunk_id, index_name):
        return self._rows.get((chunk_id, self._resolve(index_name)))

    def delete(self, index_name, chunk_id):
        self._rows.pop((chunk_id, self._resolve(index_name)), None)

    def delete_by_chunk(self, chunk_id):
        victims = [k for k in self._rows if k[0] == chunk_id]
        for key in victims:
            del self._rows[key]
        return len(victims)

    def delete_index(self, index_name):
        target = self._resolve(index_name)
        victims = [k for k in self._rows if k[1] == target]
        for key in victims:
            del self._rows[key]
        return len(victims)

    def count_in_index(self, index_name):
        target = self._resolve(index_name)
        return sum(1 for k in self._rows if k[1] == target)

    # ── 検索 ─────────────────────────────────────────
    def search(self, index_name, vector, limit, chunk_ids=None, ef_search=100):
        target = self._resolve(index_name)
        scored: list[VectorHit] = []
        allowed = set(chunk_ids) if chunk_ids else None
        for (chunk_id, index), stored in self._rows.items():
            if index != target:
                continue
            if allowed is not None and chunk_id not in allowed:
                continue
            scored.append(VectorHit(chunk_id=chunk_id,
                                    similarity=cosine(vector.values, stored.values)))
        scored.sort(key=lambda h: (-h.similarity, h.chunk_id))
        return scored[:limit]

    def search_text(self, index_name, text, limit):
        """突合（``reindex.verifier``）が使う。本文は持っていないので
        ベクトルを持たないまま空を返す —— **突合は資材では意味を持たない。**
        """
        return []

    def alias(self, alias, index_name):
        self._aliases[alias] = index_name

    def _resolve(self, name: str) -> str:
        return self._aliases.get(name, name)


class MemoryKeywordStore:
    """全文検索。**語の重なりで点を付けるだけ。** BM25 ではない。"""

    def __init__(self) -> None:
        self._rows: dict[tuple[str, str], str] = {}
        self._aliases: dict[str, str] = {}

    def index(self, index_name, chunk_id, body):
        self._rows[(chunk_id, self._resolve(index_name))] = body

    def delete(self, index_name, chunk_id):
        self._rows.pop((chunk_id, self._resolve(index_name)), None)

    def alias(self, alias, index_name):
        self._aliases[alias] = index_name

    def search(self, index_name, terms, limit, chunk_ids=None, phrase=None):
        target = self._resolve(index_name)
        allowed = set(chunk_ids) if chunk_ids else None
        scored: list[KeywordHit] = []
        for (chunk_id, index), body in self._rows.items():
            if index != target:
                continue
            if allowed is not None and chunk_id not in allowed:
                continue
            lowered = body.lower()
            if phrase:
                score = 5.0 if phrase.lower() in lowered else 0.0
            else:
                score = sum(lowered.count(t) for t in terms)
            if score > 0:
                scored.append(KeywordHit(chunk_id=chunk_id, score=float(score)))
        scored.sort(key=lambda h: (-h.score, h.chunk_id))
        return scored[:limit]

    def _resolve(self, name: str) -> str:
        return self._aliases.get(name, name)


class MemoryCacheStore:
    def __init__(self) -> None:
        self._rows: dict[str, CacheEntry] = {}

    def get(self, key): return self._rows.get(key)
    def put(self, entry): self._rows[entry.text_hash] = entry

    def purge_expired(self):
        stamp = now()
        victims = [k for k, v in self._rows.items()
                   if v.expires_at is not None and v.expires_at <= stamp]
        for key in victims:
            del self._rows[key]
        return len(victims)


class MemoryJobRepository:
    def __init__(self) -> None:
        self._jobs: dict[str, IngestJob] = {}
        self._items: dict[tuple[str, int], JobItem] = {}

    def find(self, job_id): return self._jobs.get(job_id)
    def save(self, job): self._jobs[job.job_id] = job
    def save_item(self, item): self._items[(item.job_id, item.seq_no)] = item

    def save_items(self, items):
        for item in items:
            self.save_item(item)

    def find_item(self, job_id, seq_no): return self._items.get((job_id, seq_no))

    def list_items(self, job_id):
        return sorted((i for i in self._items.values() if i.job_id == job_id),
                      key=lambda i: i.seq_no)

    def list_by_tenant(self, tenant_id, limit=50):
        rows = [j for j in self._jobs.values() if j.tenant_id == tenant_id]
        rows.sort(key=lambda j: j.queued_at, reverse=True)
        return rows[:limit]


class MemoryReindexRepository:
    def __init__(self) -> None:
        self._rows: dict[str, ReindexJob] = {}

    def find(self, job_id): return self._rows.get(job_id)
    def save(self, job): self._rows[job.job_id] = job

    def find_running(self, collection_id):
        for row in self._rows.values():
            if (row.collection_id == collection_id
                    and row.status in ("queued", "building", "verifying")):
                return row
        return None

    def latest_switched(self, collection_id):
        rows = [r for r in self._rows.values()
                if r.collection_id == collection_id and r.switched_at]
        rows.sort(key=lambda r: r.switched_at, reverse=True)
        return rows[0] if rows else None

    def next_generation(self, collection_id):
        return sum(1 for r in self._rows.values()
                   if r.collection_id == collection_id) + 2

    def list_by_collection(self, collection_id):
        return [r for r in self._rows.values() if r.collection_id == collection_id]

    def list_droppable(self):
        return [r for r in self._rows.values()
                if r.switched_at and not r.old_dropped_at
                and (now() - r.switched_at).days >= 7]


class MemoryUsageRepository:
    def __init__(self) -> None:
        self._daily: dict[tuple[str, date], DailyUsage] = {}
        self._monthly: dict[tuple[str, str], MonthlyUsage] = {}

    def find_daily(self, tenant_id, usage_date):
        return self._daily.get((tenant_id, usage_date))

    def save_daily(self, usage):
        self._daily[(usage.tenant_id, usage.usage_date)] = usage

    def list_daily_in_month(self, tenant_id, year_month):
        return [u for (t, d), u in sorted(self._daily.items())
                if t == tenant_id and f"{d.year:04d}{d.month:02d}" == year_month]

    def embed_chunks_in_month(self, tenant_id, year_month):
        return sum(u.embed_chunks
                   for u in self.list_daily_in_month(tenant_id, year_month))

    def find_monthly(self, tenant_id, year_month):
        return self._monthly.get((tenant_id, year_month))

    def save_monthly(self, usage):
        self._monthly[(usage.tenant_id, usage.year_month)] = usage


class MemoryPriceRepository:
    def __init__(self) -> None:
        self._rows: list[Price] = []

    def save(self, price): self._rows.append(price)
    def all(self): return list(self._rows)

    def list_by_kind(self, kind):
        return sorted((p for p in self._rows if p.price_kind == kind),
                      key=lambda p: p.valid_from, reverse=True)


class MemoryAuditRepository:
    """監査ログ。**本文は持たない**（``t_audit_log`` と同じ）。"""

    def __init__(self) -> None:
        self.entries: list[AuditEntry] = []

    def write(self, entry): self.entries.append(entry)

    def violations(self, since=None):
        return [e for e in self.entries
                if e.classification == "30" and e.route == "external"]


class MemorySourceStore:
    """受け付けた原文の一時置き場。ジョブが終わったら捨てる。"""

    def __init__(self) -> None:
        self._rows: dict[str, list] = defaultdict(list)

    def put(self, job_id, sources): self._rows[job_id] = list(sources)
    def take(self, job_id): return self._rows.pop(job_id, [])


class MemoryObjectStore:
    """原文の置き場。**機密区分ごとに分けていない**（メモリなので）。"""

    def __init__(self) -> None:
        self._rows: dict[str, bytes] = {}

    def put(self, data: bytes, *, content_type: str = "text/plain") -> str:
        from kotonoha.common.hashing import sha256_bytes
        key = sha256_bytes(data)
        self._rows[key] = data
        return f"s3://memory/{key}"

    def get(self, uri: str) -> bytes:
        return self._rows[uri.rsplit("/", 1)[-1]]


def cosine(left: list[float], right: list[float]) -> float:
    """コサイン類似度。長さが違えば 0。"""
    if len(left) != len(right):
        return 0.0
    dot = sum(a * b for a, b in zip(left, right))
    norm = math.sqrt(sum(a * a for a in left)) * math.sqrt(sum(b * b for b in right))
    return dot / norm if norm else 0.0
