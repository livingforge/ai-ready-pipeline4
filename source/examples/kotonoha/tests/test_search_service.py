"""検索の端から端まで。

**検索は月間上限に掛からない** —— 取り込みだけが対象で、検索は止めない
（利用申請時の合意）。
"""

from __future__ import annotations

import pytest
from conftest import make_source

from kotonoha.common.errors import InvalidInput, NotFound
from kotonoha.search.models import SearchQuery


def _query(collection, text, **kw):
    return SearchQuery(text=text, collection_id=collection.collection_id,
                       tenant_id=collection.tenant_id, **kw)


def test_当たる(services, ingested):
    result = services.search.search(_query(ingested, "異音", top_k=3))
    assert result.hits


def test_件数を絞れる(services, ingested):
    result = services.search.search(_query(ingested, "点検", top_k=1))
    assert len(result.hits) <= 1


def test_上限を超える件数指定は丸められる(services, ingested):
    result = services.search.search(_query(ingested, "点検", top_k=9999))
    assert len(result.hits) <= 100


def test_空の検索語は弾かれる(services, ingested):
    with pytest.raises(InvalidInput):
        services.search.search(_query(ingested, "   "))


def test_他テナントのコレクションは引けない(services, ingested):
    query = SearchQuery(text="異音", collection_id=ingested.collection_id,
                        tenant_id="qa-defect")
    with pytest.raises(NotFound):
        services.search.search(query)


def test_知らないコレクションは引けない(services):
    query = SearchQuery(text="異音", collection_id="nothing",
                        tenant_id="cs-support")
    with pytest.raises(NotFound):
        services.search.search(query)


def test_両方の経路が使われる(services, ingested):
    """ベクトルと全文の両方から候補が来る。"""
    result = services.search.search(_query(ingested, "異音"))
    assert "vector" in result.sources


def test_点数の順に並ぶ(services, ingested):
    result = services.search.search(_query(ingested, "点検", top_k=5))
    scores = [h.score for h in result.hits]
    assert scores == sorted(scores, reverse=True)


def test_切り出しが付く(services, ingested):
    result = services.search.search(_query(ingested, "異音", top_k=1))
    assert result.hits[0].snippet


def test_見出しのパスが付く(services, ingested):
    result = services.search.search(_query(ingested, "異音", top_k=3))
    assert any(h.heading_path for h in result.hits)


def test_文書の表題が付く(services, ingested):
    result = services.search.search(_query(ingested, "潤滑油", top_k=1))
    assert result.hits[0].title


def test_内訳を出せる(services, ingested):
    result = services.search.search(_query(ingested, "異音", top_k=1,
                                           explain=True))
    detail = result.hits[0].detail
    assert detail is not None
    assert detail["k"] == 60


def test_内訳は既定では出ない(services, ingested):
    result = services.search.search(_query(ingested, "異音", top_k=1))
    assert result.hits[0].detail is None


def test_メタデータで絞れる(services, ingested):
    hit = services.search.search(_query(ingested, "異音", top_k=10,
                                        filters={"年度": "2026"}))
    miss = services.search.search(_query(ingested, "異音", top_k=10,
                                         filters={"年度": "1999"}))
    assert hit.hits
    assert miss.hits == []


def test_絞り込みで候補が0なら空を返す(services, ingested):
    result = services.search.search(_query(ingested, "異音", top_k=10,
                                           filters={"存在しない鍵": "値"}))
    assert result.hits == []
    assert result.total_candidates == 0


def test_リランクを切れる(services, ingested):
    result = services.search.search(_query(ingested, "異音", rerank=False))
    assert not result.reranked


def test_極秘にはリランクが掛からない(services, secret_collection):
    """**本文を外部 API へ出せない。**"""
    services.ingest.submit("legal-contract", secret_collection.collection_id,
                           [make_source("秘密保持の条項について定める。")])
    services.worker.drain()
    result = services.search.search(_query(secret_collection, "秘密保持"))
    assert not result.reranked


def test_監査ログに残る(services, ingested):
    services.search.search(_query(ingested, "異音"))
    entry = services.audit.entries[-1]
    assert entry.operation == "search"
    assert entry.tenant_id == "cs-support"


def test_監査ログには検索語のハッシュしか残らない(services, ingested):
    """★ 情報セキュリティ点検表の指摘。**本文は残さない。**"""
    services.search.search(_query(ingested, "軸受の異音"))
    entry = services.audit.entries[-1]
    assert entry.query_hash
    assert "軸受" not in str(entry.query_hash)
    assert not hasattr(entry, "query_text")


def test_消えたチャンクは黙って落ちる(services, ingested):
    """検索の途中で消されても**失敗にはしない。**"""
    chunk = services.chunks.list_by_collection(ingested.collection_id)[0]
    services.chunks._rows.pop(chunk.chunk_id)
    result = services.search.search(_query(ingested, "点検", top_k=10))
    assert all(h.chunk_id != chunk.chunk_id for h in result.hits)
