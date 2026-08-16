"""新インデックスへ埋め直す。

チャンクの本文は変えない —— 分割は取り込み時のままで、**埋め直すのは
ベクトルだけ**である。分割規則を変えたいときは再インデックスでは足りず、
取り込みからやり直す必要がある（``docs/runbook/reindex.md``）。

途中で落ちても、済んだところまでは新インデックスに残る。再開すると
残りだけを埋める（``done_chunks`` から続き）—— 12 時間掛かる処理を
最初からやり直すのは現実的でない。
"""

from __future__ import annotations

from dataclasses import dataclass

from kotonoha.common import logging as applog
from kotonoha.common.errors import ProviderError
from kotonoha.embed.models import EmbedRequest

log = applog.get(__name__)

#: 一度に処理するチャンク数。提供元の上限より大きくてよい
#: （``embed.service`` が更に分ける）。
BATCH_SIZE = 256


@dataclass
class BuildProgress:
    """一区切りぶんの進み。"""

    done: int
    failed: int
    billed: int
    cached: int


class IndexBuilder:
    """埋め直す。"""

    def __init__(self, embed_service, chunk_repo, embedding_repo) -> None:
        self._embed = embed_service
        self._chunks = chunk_repo
        self._embeddings = embedding_repo

    def build(self, job, collection, *, resume_from: int = 0,
              limit: int | None = None) -> BuildProgress:
        """``job.to_index`` へ埋める。

        :param resume_from: 何件目から続けるか
        :param limit: 一度に処理する上限（``None`` なら最後まで）
        """
        chunks = self._chunks.list_by_collection(collection.collection_id)
        chunks = chunks[resume_from:]
        if limit is not None:
            chunks = chunks[:limit]

        progress = BuildProgress(done=0, failed=0, billed=0, cached=0)
        for start in range(0, len(chunks), BATCH_SIZE):
            group = chunks[start:start + BATCH_SIZE]
            try:
                result = self._embed.embed(EmbedRequest(
                    texts=[c.body for c in group],
                    model=job.to_model,
                    classification=collection.classification,
                    input_type="document",
                    tenant_id=collection.tenant_id,
                ))
            except ProviderError as exc:
                # **その塊だけ失敗にして進む。**全体を止めない。
                log.warning("再インデックスの一部が失敗しました job=%s offset=%d reason=%s",
                            job.job_id, resume_from + start, exc.message)
                progress.failed += len(group)
                continue

            self._embeddings.save_many(
                job.to_index,
                [(c.chunk_id, v) for c, v in zip(group, result.vectors)])
            progress.done += len(group)
            progress.billed += result.billed_count
            progress.cached += result.cached_count

        log.info("再インデックスを進めました job=%s 済=%d 失敗=%d",
                 job.job_id, progress.done, progress.failed)
        return progress
