"""別名の張り替え。**再インデックスの唯一の切替点。**

検索は常に別名（``col_xxxxxxxx``）を見ている。実インデックスは
``idx_xxxxxxxx_v001`` のように世代を持ち、張り替えは別名が指す先を
差し替えるだけ —— **一瞬で終わり、検索は止まらない。**

旧インデックスは 7 日残す。切り戻しは同じ関数で逆へ張り替える。
7 日経ったら ``drop_old`` が消す（日次のバッチが呼ぶ）。

★ **この「7 日」は runbook にしかない。** ADR にも稟議書にも無く、
   コードでは ``RETENTION_DAYS`` にだけ現れる。
"""

from __future__ import annotations

from kotonoha.common import logging as applog
from kotonoha.common.clock import now
from kotonoha.common.errors import InvalidInput
from kotonoha.reindex.models import ReindexJob, Verification

log = applog.get(__name__)

#: 旧インデックスを残す日数。★ 根拠は runbook のみ。
RETENTION_DAYS = 7


class AliasSwitcher:
    """張り替える。"""

    def __init__(self, vector_index, keyword_index, collection_repo,
                 embedding_repo) -> None:
        self._vector = vector_index
        self._keyword = keyword_index
        self._collections = collection_repo
        self._embeddings = embedding_repo

    def switch(self, job: ReindexJob, verification: Verification) -> ReindexJob:
        """新インデックスへ張り替える。

        **突合に通っていなければ張り替えない。** 人が明示的に
        :meth:`force_switch` を呼ぶ必要がある。

        :raises InvalidInput: 突合に通っていない
        """
        if not verification.passed:
            raise InvalidInput(
                "突合に通っていないため張り替えられません: "
                + "／".join(verification.notes),
                job_id=job.job_id, notes=verification.notes)
        return self._apply(job, forced=False)

    def force_switch(self, job: ReindexJob, *, approved_by: str) -> ReindexJob:
        """突合を飛ばして張り替える。**運用の判断で行う。**

        誰が決めたかを必ずログに残す —— あとで結果が悪かったときに
        経緯を辿れるようにするため。
        """
        log.warning("突合を飛ばして張り替えます job=%s 承認=%s", job.job_id, approved_by)
        return self._apply(job, forced=True)

    def rollback(self, job: ReindexJob) -> ReindexJob:
        """旧へ戻す。旧がまだ残っていることが前提。

        :raises InvalidInput: 旧インデックスが既に消えている
        """
        if self._embeddings.count_in_index(job.from_index) == 0:
            raise InvalidInput(
                f"旧インデックス（{job.from_index}）が残っていないため戻せません",
                job_id=job.job_id, from_index=job.from_index)
        collection = self._collections.find(job.collection_id)
        self._vector.alias(collection.index_alias, job.from_index)
        self._keyword.alias(collection.index_alias, job.from_index)
        job.switched_at = None
        job.status = "failed"
        job.error_message = "切り戻しました"
        log.warning("切り戻しました job=%s -> %s", job.job_id, job.from_index)
        return job

    def drop_old(self, job: ReindexJob) -> int:
        """保持期間を過ぎた旧インデックスを消す。

        :raises InvalidInput: まだ保持期間内／張り替えていない
        """
        if job.switched_at is None:
            raise InvalidInput("まだ張り替えていません", job_id=job.job_id)
        elapsed = (now() - job.switched_at).days
        if elapsed < RETENTION_DAYS:
            raise InvalidInput(
                f"保持期間内です（{elapsed}/{RETENTION_DAYS} 日）",
                job_id=job.job_id, elapsed_days=elapsed)
        removed = self._embeddings.delete_index(job.from_index)
        job.old_dropped_at = now()
        log.info("旧インデックスを削除しました job=%s index=%s 件数=%d",
                 job.job_id, job.from_index, removed)
        return removed

    def _apply(self, job: ReindexJob, *, forced: bool) -> ReindexJob:
        collection = self._collections.find(job.collection_id)
        self._vector.alias(collection.index_alias, job.to_index)
        self._keyword.alias(collection.index_alias, job.to_index)
        collection.embed_model = job.to_model
        self._collections.save(collection)
        job.switched_at = now()
        job.status = "switched"
        log.info("別名を張り替えました job=%s %s -> %s forced=%s",
                 job.job_id, job.from_index, job.to_index, forced)
        return job
