"""RRF による融合。

★ **定数 k=60 の根拠はコードにしかない**（``search/fusion.py``）。
   この検証も「そう実装されている」ことしか言えない —— 正しい値かどうかは
   誰も測っていない（README の仕込み A2）。
"""

from __future__ import annotations

from kotonoha.search.fusion import RRF_K, explain, fuse, to_ranked


def test_定数は60():
    assert RRF_K == 60


def test_順位づけは1始まり():
    ranked = to_ranked(["a", "b", "c"])
    assert [r.rank for r in ranked] == [1, 2, 3]


def test_両方に出た結果は片方だけより上に来る():
    """同じ順位なら、両方に出たほうが強い。"""
    fused = fuse({
        "vector": to_ranked(["both", "vec_only"]),
        "keyword": to_ranked(["both", "kw_only"]),
    })
    assert fused[0].chunk_id == "both"
    assert fused[0].sources == 2


def test_逆順に出ると両端が上に来る():
    """RRF は 1/(k+r) の和なので凸性が効く。

    ``1/61 + 1/63 > 2/62`` なので、**双方 2 位の b より、1 位と 3 位を
    取った a・c のほうが上**になる。直感に反するが RRF の性質である ——
    定数 k を大きくすると差は縮む。
    """
    fused = fuse({
        "vector": to_ranked(["a", "b", "c"]),
        "keyword": to_ranked(["c", "b", "a"]),
    })
    assert [f.chunk_id for f in fused] == ["a", "c", "b"]
    assert fused[0].score > fused[2].score


def test_片方にしか出ない結果も拾われる():
    fused = fuse({
        "vector": to_ranked(["a"]),
        "keyword": to_ranked(["z"]),
    })
    ids = {f.chunk_id for f in fused}
    assert ids == {"a", "z"}


def test_点数はRRFの式どおり():
    fused = fuse({"vector": to_ranked(["a"])})
    assert abs(fused[0].score - 1.0 / (RRF_K + 1)) < 1e-9


def test_kを大きくすると差が縮む():
    results = {"vector": to_ranked(["a", "b"])}
    tight = fuse(results, k=1)
    loose = fuse(results, k=1000)
    tight_gap = tight[0].score - tight[1].score
    loose_gap = loose[0].score - loose[1].score
    assert loose_gap < tight_gap


def test_順序が安定する():
    """点数が同じときは chunk_id で決める。

    **実行のたびに順位が入れ替わると、精度の比較ができなくなる。**
    """
    results = {"vector": to_ranked(["b", "a"]), "keyword": to_ranked(["a", "b"])}
    first = [f.chunk_id for f in fuse(results)]
    for _ in range(5):
        assert [f.chunk_id for f in fuse(results)] == first


def test_件数を絞れる():
    fused = fuse({"vector": to_ranked(["a", "b", "c", "d"])}, top_k=2)
    assert len(fused) == 2


def test_空の入力は空を返す():
    assert fuse({}) == []
    assert fuse({"vector": []}) == []


def test_どの検索から来たかが残る():
    fused = fuse({"vector": to_ranked(["a"]), "keyword": to_ranked(["a"])})
    assert fused[0].ranks == {"vector": 1, "keyword": 1}


def test_内訳を出せる():
    fused = fuse({"vector": to_ranked(["a"]), "keyword": to_ranked(["x", "a"])})
    detail = explain(fused[0])
    assert detail["k"] == RRF_K
    assert set(detail["contributions"]) == {"vector", "keyword"}
    assert abs(sum(detail["contributions"].values()) - detail["score"]) < 1e-5
