"""単価。適用期間を持ち、月の初日で引く。

外部 API の請求は月末に確定するので、**単価は翌月に決まる**。
そのため締めは翌月 5 営業日目に回す（``docs/runbook/billing.md``）。
"""

from __future__ import annotations

from datetime import date
from decimal import Decimal

from kotonoha.common.errors import NotFound
from kotonoha.billing.models import Price

EMBED_CHUNK = "embed_chunk"
SEARCH_CALL = "search_call"
RERANK_CALL = "rerank_call"
GPU_SECOND = "gpu_second"

KINDS = (EMBED_CHUNK, SEARCH_CALL, RERANK_CALL, GPU_SECOND)

#: 日本語の品目名。請求書と Excel に出す。
LABELS = {
    EMBED_CHUNK: "エンベディング（チャンク）",
    SEARCH_CALL: "検索（呼び出し）",
    RERANK_CALL: "リランク（呼び出し）",
    GPU_SECOND: "社内GPU（占有秒）",
}


class PriceBook:
    """単価を引く。"""

    def __init__(self, price_repo) -> None:
        self._prices = price_repo

    def unit_price(self, kind: str, when: date) -> Decimal:
        """その日に有効な単価。

        :raises NotFound: その日に有効な単価が登録されていない
        """
        for price in self._prices.list_by_kind(kind):
            if price.applies_on(when):
                return price.unit_price
        raise NotFound(f"{kind} の単価がありません（{when}）", kind=kind, date=str(when))

    def all_on(self, when: date) -> dict[str, Decimal]:
        """その日の全単価。締めが一度に引く。"""
        return {kind: self.unit_price(kind, when) for kind in KINDS}

    def register(self, price: Price) -> None:
        """単価を登録する。**期間の重なりは弾かない** ——
        古い行の ``valid_to`` を先に閉じる運用（``docs/runbook/billing.md``）。
        """
        self._prices.save(price)
