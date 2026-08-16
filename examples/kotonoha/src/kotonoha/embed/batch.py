"""提供元の上限に合わせて分割する。

外部 API は 128 件、社内 GPU は 32 件までしか一度に受けない。加えて
1 件あたりの文脈長（32,000 トークン）もある。**長すぎるテキストは
ここでは切らない** —— 切る判断はチャンク分割（``ingest.chunker``）の
仕事なので、ここへ来た時点で長すぎるものはエラーにする。
"""

from __future__ import annotations

from kotonoha.common.errors import InvalidInput
from kotonoha.common.tokenizer import count
from kotonoha.embed.models import EmbedModel


def split(texts: list[str], model: EmbedModel, *,
          max_batch: int | None = None) -> list[list[int]]:
    """入力の添字を、提供元へ投げられる塊に分ける。

    **値ではなく添字を返す。**呼ぶ側がキャッシュの当たり外れと突き合わせる
    必要があり、値だけだと元の位置が分からなくなるため。

    :param max_batch: 提供元の件数上限。省略するとモデルの経路から決める
    :raises InvalidInput: 1 件で文脈長を超えるテキストがある
    """
    limit = max_batch or (32 if model.route == "internal" else 128)
    batches: list[list[int]] = []
    current: list[int] = []
    for index, text in enumerate(texts):
        tokens = count(text)
        if tokens > model.max_tokens:
            raise InvalidInput(
                f"{index} 番目のテキストが長すぎます"
                f"（{tokens} トークン / 上限 {model.max_tokens}）。"
                f"先にチャンクへ分割してください",
                index=index, tokens=tokens, limit=model.max_tokens,
            )
        current.append(index)
        if len(current) >= limit:
            batches.append(current)
            current = []
    if current:
        batches.append(current)
    return batches


def estimate_calls(count_of_texts: int, model: EmbedModel) -> int:
    """何回呼ぶことになるかの見積り。上限判定の前に使う。"""
    limit = 32 if model.route == "internal" else 128
    return (count_of_texts + limit - 1) // limit
