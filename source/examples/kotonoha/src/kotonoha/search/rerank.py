"""リランク。融合した上位を並べ直す。

★ **ADR-005 と食い違っている。**

ADR-005（2026/01/14）は「リランクは第2次リリース（2026年10月）で入れる」と
決めているが、2026/06 に品質保証部から精度の要求が出て前倒しで入れた。
ADR は書き換えていない（README の仕込み B2）。

前倒しの経緯は ``docs/runbook/search-tuning.md`` に 2 行あるだけで、
**誰が決裁したのかは記録されていない。**

リランクは融合の上位 50 件（``settings.rerank_candidates``）に掛ける。
全件に掛けると遅すぎるし、下位まで並べ直す意味も薄い。
極秘（機密区分 30）のコレクションでは**掛けない** ——
リランクの提供元は外部 API しかないためである。
"""

from __future__ import annotations

from dataclasses import dataclass

from kotonoha.common import logging as applog
from kotonoha.common.errors import ProviderError
from kotonoha.common.settings import SETTINGS
from kotonoha.embed.provider import Reranker
from kotonoha.search.fusion import Fused
from kotonoha.tenant import classification as cls

log = applog.get(__name__)


@dataclass
class RerankOutcome:
    """並べ直した結果。掛からなかったときは ``applied=False``。"""

    items: list[Fused]
    applied: bool
    reason: str = ""


class RerankStage:
    """並べ直す。**失敗しても検索は返す。**"""

    def __init__(self, reranker: Reranker | None,
                 candidates: int | None = None) -> None:
        self._reranker = reranker
        self._candidates = candidates or SETTINGS.rerank_candidates

    def apply(self, text: str, fused: list[Fused], bodies: dict[str, str],
              *, top_k: int, classification: str,
              enabled: bool | None = None) -> RerankOutcome:
        """上位を並べ直す。

        :param bodies: chunk_id → 本文
        :param enabled: 明示の指定。``None`` なら設定に従う
        """
        want = SETTINGS.rerank_enabled if enabled is None else enabled
        if not want:
            return RerankOutcome(items=fused[:top_k], applied=False,
                                 reason="disabled")
        if self._reranker is None:
            return RerankOutcome(items=fused[:top_k], applied=False,
                                 reason="no_provider")
        # 極秘は外部 API へ本文を出せないので掛けない。
        if not cls.allows_external(classification):
            return RerankOutcome(items=fused[:top_k], applied=False,
                                 reason="classification")
        if len(fused) <= 1:
            return RerankOutcome(items=fused[:top_k], applied=False,
                                 reason="too_few")

        head = fused[: self._candidates]
        tail = fused[self._candidates:]
        documents = [bodies.get(item.chunk_id, "") for item in head]

        try:
            ordered = self._reranker.rerank(text, documents, top_k=len(head))
        except (ProviderError, Exception) as exc:
            # **並べ直せなくても検索は成功させる。** 融合の順で返す。
            log.warning("リランクに失敗しました。融合の順で返します reason=%s", exc)
            return RerankOutcome(items=fused[:top_k], applied=False,
                                 reason="provider_error")

        reordered: list[Fused] = []
        for index, score in ordered:
            if 0 <= index < len(head):
                item = head[index]
                item.score = score
                reordered.append(item)
        reordered.extend(tail)
        return RerankOutcome(items=reordered[:top_k], applied=True)
