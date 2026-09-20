"""社内 GPU の推論サーバ。極秘（機密区分 30）専用。

``voyage-4-nano`` はオープンウェイト（Apache-2.0）なので社内に置ける。
**これが極秘データを扱える唯一の経路である。**

外部 API と違うのは 3 つ。

1. **容量が有限**である。A100 が 2 枚しかないので、詰まると待たされる。
   待ち行列の長さを見て、閾値を超えたら受け付けを断る（``ProviderError``）。
2. **占有秒を数える**。チャージバックの按分に使う（``t_usage_daily.gpu_seconds``）。
3. **バッチの上限が小さい**。VRAM に載る範囲で 32 件。

精度は外部 API のモデルに劣る（PoC の評価で 0.71 / voyage-4 は 0.82）。
それを承知の上で極秘に使う、というのが法務部と合意した線である。
"""

from __future__ import annotations

import hashlib
import math
from typing import Protocol

from kotonoha.common import logging as applog
from kotonoha.common.clock import now
from kotonoha.common.errors import ProviderError
from kotonoha.embed.models import EmbedModel, Vector

log = applog.get(__name__)

#: VRAM に載る 1 回ぶんの件数。外部 API（128）より小さい。
MAX_BATCH = 32

#: 待ち行列がこれを超えたら受け付けない。待たせるより断ったほうがよい。
MAX_QUEUE_DEPTH = 64


class InferenceClient(Protocol):
    """推論サーバとのやり取り。差し替え点。"""

    def infer(self, texts: list[str], model: str, input_type: str) -> list[list[float]]: ...
    def queue_depth(self) -> int: ...


class _LocalInference:
    """疑似の推論。**本番では GPU のサーバを叩く。**"""

    def __init__(self) -> None:
        self._depth = 0

    def infer(self, texts: list[str], model: str, input_type: str) -> list[list[float]]:
        prefix = "q:" if input_type == "query" else "d:"
        return [_local_vector(prefix + t, model) for t in texts]

    def queue_depth(self) -> int:
        return self._depth


def _local_vector(text: str, model: str, dim: int = 1024) -> list[float]:
    acc = [0.0] * dim
    for index in range(max(1, len(text) - 1)):
        gram = text[index:index + 2]
        digest = hashlib.blake2b((model + gram).encode("utf-8"), digest_size=16).digest()
        for offset in range(0, len(digest), 2):
            slot = (digest[offset] << 8 | digest[offset + 1]) % dim
            acc[slot] += 1.0
    norm = math.sqrt(sum(v * v for v in acc)) or 1.0
    return [v / norm for v in acc]


class SelfHostedProvider:
    """社内 GPU の提供元。"""

    route = "internal"

    def __init__(self, client: InferenceClient | None = None) -> None:
        self._client = client or _LocalInference()
        #: 累計の占有秒。締めのときに読んで 0 に戻す。
        self.gpu_seconds = 0.0

    def supports(self, model: EmbedModel) -> bool:
        return model.route == "internal"

    def embed(self, texts: list[str], model: EmbedModel,
              input_type: str = "document") -> list[Vector]:
        """``texts`` を埋め込む。

        :raises ProviderError: 件数超過・待ち行列が深すぎる・推論の失敗
        """
        if not texts:
            return []
        if len(texts) > MAX_BATCH:
            raise ProviderError(
                f"社内 GPU は一度に {MAX_BATCH} 件までです（{len(texts)} 件）",
                count=len(texts), limit=MAX_BATCH)
        depth = self._client.queue_depth()
        if depth > MAX_QUEUE_DEPTH:
            raise ProviderError(
                f"推論サーバが混んでいます（待ち {depth}）。時間を空けて再度お試しください",
                queue_depth=depth)

        started = now().timestamp()
        try:
            rows = self._client.infer(texts, model.name, input_type)
        except Exception as exc:
            raise ProviderError(f"社内 GPU の推論が失敗しました: {exc}") from exc
        self.gpu_seconds += max(0.0, now().timestamp() - started)

        if len(rows) != len(texts):
            raise ProviderError(
                f"返ってきた件数が合いません（要求 {len(texts)} / 応答 {len(rows)}）")
        return [Vector(values=row, model=model.name) for row in rows]

    def take_gpu_seconds(self) -> int:
        """累計の占有秒を取り出して 0 に戻す。``billing.meter`` が呼ぶ。"""
        seconds = int(self.gpu_seconds)
        self.gpu_seconds -= seconds
        return seconds
