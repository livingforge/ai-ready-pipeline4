"""1 文書ぶんの取り込み。

**1 件の壊れた PDF で 5,000 件のジョブ全体を止めない** ——
例外は結果に畳んで返す（``docs/runbook/ingest.md``）。
"""

from __future__ import annotations

from conftest import make_source


def _run(services, collection, source):
    return services.worker._pipeline.run(source, collection)


def test_取り込むとチャンクが出る(services, manual_collection):
    outcome = _run(services, manual_collection,
                   make_source("軸受から異音がする。回転数を落として確かめる。"))
    assert outcome.status == "done"
    assert outcome.chunks
    assert outcome.document is not None


def test_空の文書は飛ばす(services, manual_collection):
    outcome = _run(services, manual_collection, make_source("   \n\n  "))
    assert outcome.status == "skipped"
    assert outcome.skip_reason == "empty"


def test_記号だけの文書も飛ばす(services, manual_collection):
    outcome = _run(services, manual_collection, make_source("---===---"))
    assert outcome.status == "skipped"


def test_知らない形式は失敗として残る(services, manual_collection):
    """**黙って飛ばさない。** 取り込んだつもりで入っていないのが困る。"""
    outcome = _run(services, manual_collection,
                   make_source("中身", content_type="application/zip"))
    assert outcome.status == "failed"
    assert "対応していない" in outcome.error


def test_同じ本文は2回目に飛ぶ(services, manual_collection):
    source = make_source("軸受から異音がする。")
    assert _run(services, manual_collection, source).status == "done"
    second = _run(services, manual_collection, source)
    assert second.status == "skipped"
    assert second.skip_reason == "same_hash"


def test_本文が変わると入れ直す(services, manual_collection):
    _run(services, manual_collection, make_source("最初の本文。"))
    outcome = _run(services, manual_collection, make_source("直した本文。"))
    assert outcome.status == "done"
    assert outcome.chunks


def test_メタデータだけ変わると本文は作り直さない(services, manual_collection):
    _run(services, manual_collection,
         make_source("同じ本文。", metadata={"年度": "2025"}))
    outcome = _run(services, manual_collection,
                   make_source("同じ本文。", metadata={"年度": "2026"}))
    assert outcome.status == "done"
    assert outcome.skip_reason == "metadata_changed"
    assert outcome.billed_chunks == 0        # 埋め直していない


def test_入れ直すと古いチャンクは消える(services, manual_collection):
    _run(services, manual_collection, make_source("点検の手順。" * 200))
    before = services.chunks.count_in_collection(manual_collection.collection_id)
    _run(services, manual_collection, make_source("短い本文。"))
    after = services.chunks.count_in_collection(manual_collection.collection_id)
    assert after < before


def test_markdownの見出しがチャンクに残る(services, manual_collection):
    outcome = _run(services, manual_collection, make_source(
        "# 章\n\n本文\n\n## 節\n\n本文\n", content_type="text/markdown"))
    assert any(c.heading_path for c in outcome.chunks)


def test_極秘は社内経路で埋め込まれる(services, secret_collection):
    outcome = _run(services, secret_collection, make_source("契約の条項。"))
    assert outcome.status == "done"
    assert services.audit.entries[-1].route == "internal"


def test_画像だけのpdfは部分的と申告される(services, manual_collection):
    """★ OCR 未対応。**既知の穴**として結果に残る。"""
    outcome = _run(services, manual_collection, make_source(
        "%PDF-1.4\n/Type /Page\n", content_type="application/pdf"))
    assert outcome.status == "skipped"
    assert outcome.notes.get("image_only")


def test_チャンクの通し番号が連番(services, manual_collection):
    outcome = _run(services, manual_collection,
                   make_source("点検の手順。" * 300))
    assert [c.seq_no for c in outcome.chunks] == list(range(len(outcome.chunks)))


def test_全文の索引にも入る(services, manual_collection):
    """本番は pgvector と OpenSearch の両方へ入れる。"""
    _run(services, manual_collection, make_source("異音の点検手順。"))
    hits = services.keywords.search(manual_collection.index_alias,
                                    ["異音"], 10)
    assert hits
