"""ベクトル検索と全文検索（BM25）の順位を融合する。RRF。

★ **定数 k = 60 の根拠はどこにも書かれていない。**

ADR にも docs/ にも Excel にも無く、ここが唯一の正本である
（README の仕込み A2）。RRF の原論文が挙げている値をそのまま使っており、
社内のデータで調整したわけではない。**調整したほうがよいかもしれないが、
誰も試していない。**

RRF（Reciprocal Rank Fusion）は点数ではなく**順位**だけを使う。
ベクトルのコサイン類似度（0〜1）と BM25 のスコア（上限なし）は
そのままでは足せないので、順位に落としてから足す ——
正規化の仕方を決めずに済むのが利点である。

    score(d) = Σ 1 / (k + rank_i(d))

``k`` が大きいほど上位と下位の差が縮み、下位の結果が拾われやすくなる。
"""

from __future__ import annotations

from dataclasses import dataclass, field

#: RRF の定数。★ 根拠は記録されていない。
RRF_K = 60

#: どちらかの検索にしか出てこない結果に掛ける重み。1.0 は補正なし。
#: **これも根拠が無い。** 片側にしか出ない結果を下げたくなったときのために
#: 置いてあるが、1.0 のまま一度も動かしていない。
SINGLE_SOURCE_WEIGHT = 1.0


@dataclass
class Ranked:
    """1 本の検索結果。順位は 1 始まり。"""

    chunk_id: str
    rank: int
    raw_score: float = 0.0


@dataclass
class Fused:
    """融合したあとの 1 件。"""

    chunk_id: str
    score: float
    #: どの検索から来たか（``vector`` / ``keyword``）と、その順位
    ranks: dict[str, int] = field(default_factory=dict)

    @property
    def sources(self) -> int:
        return len(self.ranks)


def fuse(results: dict[str, list[Ranked]], *, k: int = RRF_K,
         top_k: int | None = None) -> list[Fused]:
    """順位の列を融合する。

    :param results: 検索の名前（``vector`` / ``keyword``）→ 順位の列
    :param k: RRF の定数
    :param top_k: 返す件数。省略すると全部
    """
    accumulated: dict[str, Fused] = {}
    for source, ranked in results.items():
        for item in ranked:
            entry = accumulated.get(item.chunk_id)
            if entry is None:
                entry = Fused(chunk_id=item.chunk_id, score=0.0)
                accumulated[item.chunk_id] = entry
            entry.score += 1.0 / (k + item.rank)
            entry.ranks[source] = item.rank

    fused = list(accumulated.values())
    if SINGLE_SOURCE_WEIGHT != 1.0:
        for entry in fused:
            if entry.sources == 1:
                entry.score *= SINGLE_SOURCE_WEIGHT

    # 点数が同じときは chunk_id で決める。**順序を安定させるため** ——
    # 実行のたびに順位が入れ替わると、精度の比較ができなくなる。
    fused.sort(key=lambda e: (-e.score, e.chunk_id))
    return fused[:top_k] if top_k else fused


def to_ranked(chunk_ids: list[str], scores: list[float] | None = None) -> list[Ranked]:
    """並んだ ID の列を順位付きへ変える。"""
    scores = scores or [0.0] * len(chunk_ids)
    return [Ranked(chunk_id=cid, rank=index + 1, raw_score=score)
            for index, (cid, score) in enumerate(zip(chunk_ids, scores))]


def explain(fused: Fused, *, k: int = RRF_K) -> dict:
    """1 件の点数の内訳。``search.explain`` が使う。"""
    return {
        "chunk_id": fused.chunk_id,
        "score": round(fused.score, 6),
        "k": k,
        "contributions": {
            source: round(1.0 / (k + rank), 6)
            for source, rank in sorted(fused.ranks.items())
        },
        "ranks": dict(sorted(fused.ranks.items())),
    }
