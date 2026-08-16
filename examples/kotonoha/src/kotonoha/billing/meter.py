"""利用量を数える。

★ **「キャッシュに当たった分は数えない」という規則はここにしかない。**

利用申請の説明資料には「取り込んだチャンク数で課金します」としか書いて
おらず、キャッシュの扱いに触れていない。実装はキャッシュ分を
``cached_chunks`` に分けて課金対象から外している（業務ルール
「利用量の数え方」／README の仕込み A）。

各部門から「思ったより請求が少ない」と言われることがあり、その都度
口頭で説明している —— **文書化されていないため。**
"""

from __future__ import annotations

from kotonoha.common import logging as applog
from kotonoha.common.clock import today
from kotonoha.billing.models import DailyUsage

log = applog.get(__name__)


class UsageMeter:
    """使ったぶんを日次の表へ足す。"""

    def __init__(self, usage_repo) -> None:
        self._usage = usage_repo

    def record_embed(self, tenant_id: str, *, billed: int, cached: int,
                     gpu_seconds: int = 0) -> None:
        """埋め込みの実績。**``billed`` だけが課金対象。**"""
        self._add(DailyUsage(
            tenant_id=tenant_id, usage_date=today(),
            embed_chunks=billed, cached_chunks=cached, gpu_seconds=gpu_seconds,
        ))

    def record_search(self, tenant_id: str, *, reranked: bool = False) -> None:
        """検索 1 回。リランクが掛かったら別に数える。"""
        self._add(DailyUsage(
            tenant_id=tenant_id, usage_date=today(),
            search_calls=1, rerank_calls=1 if reranked else 0,
        ))

    def _add(self, delta: DailyUsage) -> None:
        current = self._usage.find_daily(delta.tenant_id, delta.usage_date)
        if current is None:
            current = DailyUsage(tenant_id=delta.tenant_id,
                                 usage_date=delta.usage_date)
        current.add(delta)
        self._usage.save_daily(current)

    def month_to_date(self, tenant_id: str, year_month: str) -> DailyUsage:
        """当月の合計。上限判定と利用照会が読む。"""
        total = DailyUsage(tenant_id=tenant_id, usage_date=today())
        for row in self._usage.list_daily_in_month(tenant_id, year_month):
            total.add(row)
        return total
