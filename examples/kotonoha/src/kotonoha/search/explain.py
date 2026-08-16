"""点数の内訳。**調査用で、通常の応答には出さない。**

「なぜこの文書が上に来るのか」を利用側から聞かれたときに使う。
品質保証部からこの問い合わせが多く、そのために足した。

★ **この機能は API 仕様（openapi.yaml）に書かれているが、
   どの設計文書にも出てこない。** arp4 は .yaml を読まないので、
   資料だけを見ると存在しない機能になる（README の仕込み F1）。
"""

from __future__ import annotations

from dataclasses import dataclass

from kotonoha.search.fusion import RRF_K, Fused


@dataclass
class Explanation:
    """1 件ぶんの内訳。"""

    chunk_id: str
    final_score: float
    rrf_k: int
    vector_rank: int | None
    keyword_rank: int | None
    vector_contribution: float
    keyword_contribution: float
    reranked: bool
    rerank_score: float | None = None

    def to_dict(self) -> dict:
        return {
            "chunk_id": self.chunk_id,
            "final_score": round(self.final_score, 6),
            "rrf_k": self.rrf_k,
            "vector": {"rank": self.vector_rank,
                       "contribution": round(self.vector_contribution, 6)},
            "keyword": {"rank": self.keyword_rank,
                        "contribution": round(self.keyword_contribution, 6)},
            "reranked": self.reranked,
            "rerank_score": self.rerank_score,
        }


def build(fused: Fused, *, reranked: bool = False,
          rerank_score: float | None = None, k: int = RRF_K) -> Explanation:
    """1 件の内訳を組む。"""
    vector_rank = fused.ranks.get("vector")
    keyword_rank = fused.ranks.get("keyword")
    return Explanation(
        chunk_id=fused.chunk_id,
        final_score=fused.score,
        rrf_k=k,
        vector_rank=vector_rank,
        keyword_rank=keyword_rank,
        vector_contribution=1.0 / (k + vector_rank) if vector_rank else 0.0,
        keyword_contribution=1.0 / (k + keyword_rank) if keyword_rank else 0.0,
        reranked=reranked,
        rerank_score=rerank_score,
    )


def summarize(explanations: list[Explanation]) -> dict:
    """全体の傾向。**どちらの検索が効いているか**を見る。

    ベクトルだけ・全文だけ・両方に出た件数を数える。ハイブリッドにした
    意味があるかの点検に使う —— 片方だけの結果ばかりなら、融合の
    設計を見直す材料になる。
    """
    both = sum(1 for e in explanations if e.vector_rank and e.keyword_rank)
    vector_only = sum(1 for e in explanations if e.vector_rank and not e.keyword_rank)
    keyword_only = sum(1 for e in explanations if e.keyword_rank and not e.vector_rank)
    return {
        "total": len(explanations),
        "both": both,
        "vector_only": vector_only,
        "keyword_only": keyword_only,
    }
