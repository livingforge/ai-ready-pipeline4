"""埋め込みの提供元の約束。

外部 API（``voyage``）も社内 GPU（``selfhosted``）も同じ形で呼べるように
する。**呼び分けは ``router`` の仕事**で、ここでは経路の違いを持たない。
"""

from __future__ import annotations

from typing import Protocol

from kotonoha.embed.models import EmbedModel, Vector


class EmbedProvider(Protocol):
    """埋め込みを作るもの。"""

    #: どの経路か（``external`` / ``internal``）。監査ログに残る。
    route: str

    def embed(self, texts: list[str], model: EmbedModel,
              input_type: str) -> list[Vector]:
        """``texts`` を埋め込む。順序は入力と同じ。

        :param input_type: ``query`` か ``document``。retrieval では必ず渡す
        :raises ProviderError: 提供元が失敗した（再試行の対象）
        """
        ...

    def supports(self, model: EmbedModel) -> bool:
        """そのモデルを扱えるか。"""
        ...


class Reranker(Protocol):
    """検索結果を並べ直すもの。"""

    def rerank(self, query: str, documents: list[str],
               top_k: int) -> list[tuple[int, float]]:
        """``(元の添字, 点数)`` を点数の降順で返す。"""
        ...
