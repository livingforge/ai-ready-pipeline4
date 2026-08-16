"""外部 API（Voyage AI）のクライアント。

**本物の HTTP は投げない。** 本番では ``voyageai`` パッケージか
``POST https://api.voyageai.com/v1/embeddings`` を叩くが、この資材は
外部ライブラリとネットワークに依存しない方針なので、決定的な疑似ベクトルを
返す ``_FakeTransport`` を既定にしてある。**呼び出しの形と再試行と
``input_type`` の扱いは本番と同じ**にしてあるので、差し替えるのは
``Transport`` の実装 1 つだけで済む。

``input_type`` は retrieval では必ず渡す —— 検索語と文書で前置きが変わり、
省くと精度が落ちる。ここを省いた実装が PoC 時代にあり、精度が出ない
原因を 2 週間探した経緯がある（``docs/adr/0002-embedding-provider.md``）。
"""

from __future__ import annotations

import hashlib
import math
from typing import Protocol

from kotonoha.common import logging as applog
from kotonoha.common.errors import ProviderError
from kotonoha.common.retry import call as retry_call
from kotonoha.embed.models import EmbedModel, Vector

log = applog.get(__name__)

#: 1 回の呼び出しで投げられる最大件数（提供元の制限）。
MAX_BATCH = 128

#: retrieval で使える ``input_type``。
INPUT_TYPES = ("query", "document")


class Transport(Protocol):
    """HTTP のやり取り。差し替え点。"""

    def post(self, path: str, payload: dict) -> dict: ...


class _FakeTransport:
    """決定的な疑似ベクトルを返す。**本番では使わない。**

    同じ文字列には必ず同じベクトルを返し、似た文字列には近いベクトルを
    返す（文字 3-gram のハッシュを足し込む）。検索の順位が意味を持つ
    程度には似ていて、外部依存はゼロ。
    """

    def post(self, path: str, payload: dict) -> dict:
        model = payload["model"]
        dim = payload.get("output_dimension", 1024)
        prefix = "q:" if payload.get("input_type") == "query" else "d:"
        data = []
        for index, text in enumerate(payload["input"]):
            data.append({"embedding": _pseudo_vector(prefix + text, dim, model),
                         "index": index})
        return {"object": "list", "data": data, "model": model,
                "usage": {"total_tokens": sum(len(t) for t in payload["input"])}}


def _pseudo_vector(text: str, dim: int, model: str) -> list[float]:
    """文字 3-gram から決定的なベクトルを作り、長さ 1 に正規化する。"""
    acc = [0.0] * dim
    grams = [text[i:i + 3] for i in range(max(1, len(text) - 2))] or [text]
    for gram in grams:
        digest = hashlib.sha256((model + gram).encode("utf-8")).digest()
        for offset in range(0, len(digest), 2):
            slot = (digest[offset] << 8 | digest[offset + 1]) % dim
            acc[slot] += 1.0
    norm = math.sqrt(sum(v * v for v in acc)) or 1.0
    return [v / norm for v in acc]


class VoyageProvider:
    """外部 API の提供元。"""

    route = "external"

    def __init__(self, transport: Transport | None = None,
                 api_key: str = "") -> None:
        self._transport = transport or _FakeTransport()
        self._api_key = api_key

    def supports(self, model: EmbedModel) -> bool:
        return model.route == "external"

    def embed(self, texts: list[str], model: EmbedModel,
              input_type: str = "document") -> list[Vector]:
        """``texts`` を埋め込む。

        :raises ProviderError: 件数超過・不正な ``input_type``・提供元の失敗
        """
        if not texts:
            return []
        if len(texts) > MAX_BATCH:
            raise ProviderError(
                f"一度に投げられるのは {MAX_BATCH} 件までです（{len(texts)} 件）",
                count=len(texts))
        if input_type not in INPUT_TYPES:
            raise ProviderError(
                f"input_type は {'/'.join(INPUT_TYPES)} のいずれかです: {input_type}",
                input_type=input_type)

        payload = {
            "input": texts,
            "model": model.name,
            "input_type": input_type,
            "output_dimension": model.dim,
        }

        def once() -> dict:
            try:
                return self._transport.post("/v1/embeddings", payload)
            except ProviderError:
                raise
            except Exception as exc:                      # 提供元の事情はここで包む
                raise ProviderError(f"埋め込みの提供元が失敗しました: {exc}") from exc

        body = retry_call(once, label=f"voyage:{model.name}")
        rows = sorted(body["data"], key=lambda r: r["index"])
        if len(rows) != len(texts):
            raise ProviderError(
                f"返ってきた件数が合いません（要求 {len(texts)} / 応答 {len(rows)}）")
        return [Vector(values=row["embedding"], model=model.name) for row in rows]


class VoyageReranker:
    """リランクの提供元。``rerank-2.5``。

    ★ ADR-005 は「リランクは第2次リリース」と書いているが、品質保証部の
    精度要求で前倒しして入れた。ADR は更新していない
    （README の仕込み B2）。
    """

    def __init__(self, transport: Transport | None = None,
                 model: str = "rerank-2.5") -> None:
        self._transport = transport or _FakeRerankTransport()
        self.model = model

    def rerank(self, query: str, documents: list[str],
               top_k: int) -> list[tuple[int, float]]:
        if not documents:
            return []
        body = self._transport.post("/v1/rerank", {
            "query": query, "documents": documents,
            "model": self.model, "top_k": min(top_k, len(documents)),
        })
        rows = body["data"]
        return [(r["index"], r["relevance_score"]) for r in rows]


class _FakeRerankTransport:
    """語の重なりで点を付けるだけの疑似リランカ。**本番では使わない。**"""

    def post(self, path: str, payload: dict) -> dict:
        query_chars = set(payload["query"])
        scored = []
        for index, doc in enumerate(payload["documents"]):
            overlap = len(query_chars & set(doc))
            scored.append({"index": index,
                           "relevance_score": overlap / (len(query_chars) or 1)})
        scored.sort(key=lambda r: (-r["relevance_score"], r["index"]))
        return {"data": scored[:payload["top_k"]], "model": payload["model"]}
