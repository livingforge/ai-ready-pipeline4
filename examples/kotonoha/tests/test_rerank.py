"""リランク。

★ **ADR-005 は「第2次リリース（2026年10月）で入れる」と書いているが、
   実装済みである**（README の仕込み B2）。この検証は実装の側の仕様で、
   ADR とは突き合わせられない。

**極秘には掛けない** —— リランクの提供元は外部 API しかないため。
"""

from __future__ import annotations

from kotonoha.common.errors import ProviderError
from kotonoha.search.fusion import fuse, to_ranked
from kotonoha.search.rerank import RerankStage


class _Reverse:
    """順序をひっくり返すだけの見本。"""

    def rerank(self, query, documents, top_k):
        return [(i, float(len(documents) - i))
                for i in reversed(range(len(documents)))]


class _Broken:
    """必ず失敗する見本。"""

    def rerank(self, query, documents, top_k):
        raise ProviderError("提供元が落ちています")


def _fused(ids):
    return fuse({"vector": to_ranked(ids)})


def _bodies(ids):
    return {i: f"{i} の本文" for i in ids}


def test_掛かると順序が変わる():
    ids = ["a", "b", "c"]
    stage = RerankStage(_Reverse())
    outcome = stage.apply("問い", _fused(ids), _bodies(ids),
                          top_k=3, classification="10")
    assert outcome.applied
    assert [i.chunk_id for i in outcome.items] == ["c", "b", "a"]


def test_切っていれば掛からない():
    ids = ["a", "b", "c"]
    outcome = RerankStage(_Reverse()).apply(
        "問い", _fused(ids), _bodies(ids), top_k=3,
        classification="10", enabled=False)
    assert not outcome.applied
    assert outcome.reason == "disabled"


def test_提供元が無ければ掛からない():
    ids = ["a", "b"]
    outcome = RerankStage(None).apply("問い", _fused(ids), _bodies(ids),
                                      top_k=2, classification="10")
    assert not outcome.applied
    assert outcome.reason == "no_provider"


def test_極秘には掛からない():
    """**本文を外部 API へ出せない。**"""
    ids = ["a", "b"]
    outcome = RerankStage(_Reverse()).apply(
        "問い", _fused(ids), _bodies(ids), top_k=2, classification="30")
    assert not outcome.applied
    assert outcome.reason == "classification"


def test_1件以下なら掛からない():
    outcome = RerankStage(_Reverse()).apply(
        "問い", _fused(["a"]), _bodies(["a"]), top_k=1, classification="10")
    assert not outcome.applied
    assert outcome.reason == "too_few"


def test_提供元が落ちても検索は返る():
    """**並べ直せなくても検索は成功させる。** 融合の順で返す。"""
    ids = ["a", "b", "c"]
    outcome = RerankStage(_Broken()).apply(
        "問い", _fused(ids), _bodies(ids), top_k=3, classification="10")
    assert not outcome.applied
    assert outcome.reason == "provider_error"
    assert [i.chunk_id for i in outcome.items] == ids


def test_件数が絞られる():
    ids = ["a", "b", "c", "d"]
    outcome = RerankStage(_Reverse()).apply(
        "問い", _fused(ids), _bodies(ids), top_k=2, classification="10")
    assert len(outcome.items) == 2


def test_候補の上限を超えた分は末尾に残る():
    ids = [f"c{i:02d}" for i in range(10)]
    stage = RerankStage(_Reverse(), candidates=3)
    outcome = stage.apply("問い", _fused(ids), _bodies(ids),
                          top_k=10, classification="10")
    assert outcome.applied
    # 先頭 3 件だけが並べ直され、残りは元の順で続く
    assert [i.chunk_id for i in outcome.items[:3]] == ["c02", "c01", "c00"]
    assert [i.chunk_id for i in outcome.items[3:]] == ids[3:]
