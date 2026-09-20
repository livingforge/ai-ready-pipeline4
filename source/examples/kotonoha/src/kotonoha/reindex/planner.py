"""再インデックスの見積り。

**始める前に利用部門へ知らせる**ための数字を出す。所要時間の見積りは
SLO の「全件 12 時間以内」に収まるかの判定にも使う。

見積りの係数（1 呼び出しあたりの秒数）は運用の実績から出したもので、
``docs/runbook/reindex.md`` に記録がある —— 外部 API で 0.9 秒／128 件、
社内 GPU で 2.4 秒／32 件。**社内 GPU は 10 倍以上遅い**ので、
法務部のコレクションだけは所要が桁違いになる。
"""

from __future__ import annotations

from kotonoha.embed import batch as batching
from kotonoha.embed import registry
from kotonoha.reindex.models import Plan

#: 1 呼び出しあたりの秒数（実測）。
SECONDS_PER_CALL = {"external": 0.9, "internal": 2.4}

#: SLO の上限（分）。これを超える見積りは警告を出す。
SLO_MINUTES = 12 * 60


class Planner:
    """見積もる。"""

    def __init__(self, chunk_repo) -> None:
        self._chunks = chunk_repo

    def plan(self, collection, to_model: str) -> Plan:
        """新モデルへ移すときの見積り。

        :raises InvalidInput: 台帳に無いモデル
        :raises ClassificationViolation: 区分に許されないモデル
        """
        target = registry.resolve(to_model, collection.classification)
        current = registry.get(collection.embed_model)
        total = self._chunks.count_in_collection(collection.collection_id)
        calls = batching.estimate_calls(total, target)
        seconds = calls * SECONDS_PER_CALL[target.route]
        minutes = int(seconds / 60) + 1

        plan = Plan(
            collection_id=collection.collection_id,
            from_model=collection.embed_model,
            to_model=target.name,
            total_chunks=total,
            estimated_calls=calls,
            estimated_minutes=minutes,
            dimension_changes=current.dim != target.dim,
        )

        if minutes > SLO_MINUTES:
            plan.warnings.append(
                f"見積り {minutes // 60} 時間は SLO（12 時間）を超えます。"
                f"分割して実施してください")
        if plan.dimension_changes:
            plan.warnings.append(
                f"次元が変わります（{current.dim} → {target.dim}）。"
                f"索引を作り直す必要があります")
        if target.route == "internal":
            plan.warnings.append(
                "社内 GPU を使います。ほかの極秘テナントの取り込みが遅くなります")
        if total == 0:
            plan.warnings.append("チャンクが 1 件もありません。実施の必要がありません")

        return plan
