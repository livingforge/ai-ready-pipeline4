"""取り込みの受付。``POST /v1/collections/{id}/documents`` が呼ぶ。

**受け付ける前に 3 つ確かめる。**

1. コレクションが書き込みを受けられるか（再構築中は受けない）
2. 機密区分とモデルの組み合わせが許されるか（``embed.router.check``）
3. 月間の上限に収まるか（``tenant.quota``）

3 は**見積りで弾く**。チャンク数は切ってみるまで分からないので、
トークン数から概算する —— 途中まで埋め込んでから止めると、課金だけされて
使えないものが残るためである。
"""

from __future__ import annotations

from kotonoha.common import logging as applog
from kotonoha.common.errors import IndexBusy, InvalidInput
from kotonoha.common.settings import SETTINGS
from kotonoha.common.tokenizer import count
from kotonoha.ingest.chunker import TARGET_TOKENS
from kotonoha.ingest.models import IngestJob, SourceDocument
from kotonoha.ingest.queue import QueuedJob

log = applog.get(__name__)


class IngestService:
    """受け付ける。"""

    def __init__(self, collection_service, tenant_service, quota_checker,
                 embed_router, queue, tracker, source_store) -> None:
        self._collections = collection_service
        self._tenants = tenant_service
        self._quota = quota_checker
        self._router = embed_router
        self._queue = queue
        self._tracker = tracker
        self._sources = source_store

    def submit(self, tenant_id: str, collection_id: str,
               sources: list[SourceDocument]) -> IngestJob:
        """取り込みを受け付け、ジョブを返す。**まだ処理していない。**

        :raises InvalidInput: 件数が多すぎる／空
        :raises IndexBusy: 再構築中で受けられない
        :raises ClassificationViolation: 区分に許されないモデル
        :raises QuotaExceeded: 月間の上限を超える
        """
        if not sources:
            raise InvalidInput("取り込む文書がありません")
        if len(sources) > SETTINGS.max_documents_per_job:
            raise InvalidInput(
                f"1 回に投げられるのは {SETTINGS.max_documents_per_job} 件までです"
                f"（{len(sources)} 件）。分割してください",
                count=len(sources), limit=SETTINGS.max_documents_per_job)

        tenant = self._tenants.get(tenant_id)
        collection = self._collections.get(collection_id, tenant_id=tenant_id)
        if not collection.writable:
            raise IndexBusy(
                "再インデックス中のため取り込みを受け付けられません。"
                "検索は続けてお使いいただけます",
                collection_id=collection_id, status=collection.status)

        self._router.check(collection.classification, collection.embed_model)
        estimated = estimate_chunks(sources)
        self._quota.ensure_can_ingest(tenant, estimated)

        job = self._tracker.open(tenant_id, collection_id, sources)
        self._sources.put(job.job_id, sources)
        self._queue.push(QueuedJob(job_id=job.job_id, tenant_id=tenant_id,
                                   collection_id=collection_id))
        log.info("取り込みを受け付けました job=%s 件数=%d 見積チャンク=%d",
                 job.job_id, len(sources), estimated)
        return job

    def status(self, job_id: str, tenant_id: str) -> IngestJob:
        """進捗を返す。**他テナントのジョブは見せない。**"""
        job = self._tracker.get(job_id)
        if job.tenant_id != tenant_id:
            from kotonoha.common.errors import NotFound
            raise NotFound(f"ジョブがありません: {job_id}", job_id=job_id)
        return job


def estimate_chunks(sources: list[SourceDocument]) -> int:
    """チャンク数の概算。**切らずにトークン数から割る。**

    実際の分割は見出しの境界で早めに切るので、この見積りは**やや少なめ**に
    出る。上限判定には少なめのほうが安全側ではないが、多めに見積もって
    受け付けを断るほうが困る、という判断でこうしてある ——
    **この判断はどこにも書かれていない。**
    """
    total = 0
    for source in sources:
        tokens = count(source.content)
        total += max(1, -(-tokens // TARGET_TOKENS))
    return total
