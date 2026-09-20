"""新旧の突合。**張り替える前に必ず通す。**

見るのは 2 つ。

1. **件数が合っているか。** 新が旧より少なければ埋め漏れがある。
2. **検索の上位が大きく変わっていないか。** 抜き取りで同じ検索語を
   両方へ投げ、上位 10 件の重なりを見る。

2 の閾値 0.6 は**根拠が薄い。** モデルを変えれば結果は変わるのが当然で、
どれくらいまでなら「変わりすぎ」でないのかを決める材料が無い。
いまは「経験的にこれ以下だと苦情が来る」という線で置いてある ——
``docs/runbook/reindex.md`` にそう書いてある。
"""

from __future__ import annotations

from kotonoha.common import logging as applog
from kotonoha.reindex.models import Verification

log = applog.get(__name__)

#: 抜き取る検索語の数。
SAMPLE_SIZE = 30

#: 上位の重なりがこれを下回ったら止める。★ 根拠は経験則。
MIN_AGREEMENT = 0.6

#: 見る上位の件数。
TOP_N = 10


class Verifier:
    """突き合わせる。"""

    def __init__(self, embedding_repo, chunk_repo, vector_index) -> None:
        self._embeddings = embedding_repo
        self._chunks = chunk_repo
        self._index = vector_index

    def verify(self, job, collection, *,
               sample_queries: list[str] | None = None) -> Verification:
        """新旧を見比べる。

        :param sample_queries: 抜き取りに使う検索語。省略するとチャンクの
            先頭から作る（**実際の検索語は使わない** —— 監査ログに本文が
            残っていないため）
        """
        old_count = self._embeddings.count_in_index(job.from_index)
        new_count = self._embeddings.count_in_index(job.to_index)

        notes: list[str] = []
        if new_count < old_count:
            notes.append(f"新インデックスが少ないです（旧 {old_count} / 新 {new_count}）")

        queries = sample_queries or self._sample_queries(collection)
        agreement = self._agreement(job, queries) if queries else 1.0
        if agreement < MIN_AGREEMENT:
            notes.append(
                f"上位 {TOP_N} 件の重なりが {agreement:.0%} で、"
                f"閾値 {MIN_AGREEMENT:.0%} を下回りました")

        passed = new_count >= old_count and agreement >= MIN_AGREEMENT
        verification = Verification(
            old_count=old_count, new_count=new_count,
            sampled=len(queries), agreement=agreement,
            passed=passed, notes=notes,
        )
        log.info("突合しました job=%s 旧=%d 新=%d 重なり=%.0f%% 判定=%s",
                 job.job_id, old_count, new_count, agreement * 100,
                 "可" if passed else "否")
        return verification

    def _sample_queries(self, collection) -> list[str]:
        """チャンクの先頭 40 字を検索語に使う。"""
        chunks = self._chunks.list_by_collection(collection.collection_id)
        step = max(1, len(chunks) // SAMPLE_SIZE)
        return [c.body[:40] for c in chunks[::step][:SAMPLE_SIZE]]

    def _agreement(self, job, queries: list[str]) -> float:
        """上位の重なりの平均。"""
        ratios: list[float] = []
        for text in queries:
            old_top = self._top_ids(job.from_index, text)
            new_top = self._top_ids(job.to_index, text)
            if not old_top:
                continue
            overlap = len(set(old_top) & set(new_top))
            ratios.append(overlap / len(old_top))
        return sum(ratios) / len(ratios) if ratios else 1.0

    def _top_ids(self, index_name: str, text: str) -> list[str]:
        hits = self._index.search_text(index_name, text, TOP_N)
        return [h.chunk_id for h in hits]
