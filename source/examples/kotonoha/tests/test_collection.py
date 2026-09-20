"""コレクションの作成と参照。

**機密区分の継承がここで効く。** 埋め込みモデルは作成時に固定 ——
後から変えるには再インデックスが要る（次元とベクトル空間が変わる）。
"""

from __future__ import annotations

import pytest

from kotonoha.common.errors import (AlreadyExists, ClassificationViolation,
                                    InvalidInput, NotFound)


def test_作れる(services):
    collection = services.collections.create("cs-support", "manual")
    assert collection.collection_name == "manual"
    assert collection.status == "A"


def test_テナントの区分を継承する(services):
    assert services.collections.create(
        "legal-contract", "c1").classification == "30"
    assert services.collections.create(
        "cs-support", "c2").classification == "10"


def test_区分を厳しくできる(services):
    collection = services.collections.create("cs-support", "c1",
                                             classification="30")
    assert collection.classification == "30"


def test_区分を緩められない(services):
    with pytest.raises(ClassificationViolation):
        services.collections.create("legal-contract", "c1", classification="10")


def test_極秘は社内ホストのモデルになる(services):
    collection = services.collections.create("legal-contract", "c1")
    assert collection.embed_model == "voyage-4-nano"


def test_極秘に外部モデルを指定すると弾かれる(services):
    with pytest.raises(ClassificationViolation):
        services.collections.create("legal-contract", "c1",
                                    embed_model="voyage-law-2")


def test_同じ名前は作れない(services):
    services.collections.create("cs-support", "manual")
    with pytest.raises(AlreadyExists):
        services.collections.create("cs-support", "manual")


def test_テナントが違えば同じ名前を使える(services):
    services.collections.create("cs-support", "manual")
    services.collections.create("qa-defect", "manual")      # 例外が出ないこと


def test_名前の形が不正だと弾かれる(services):
    with pytest.raises(InvalidInput):
        services.collections.create("cs-support", "日本語の名前")
    with pytest.raises(InvalidInput):
        services.collections.create("cs-support", "-hyphen-start")


def test_知らないテナントでは作れない(services):
    with pytest.raises(NotFound):
        services.collections.create("nothing", "manual")


def test_別名が振られる(services):
    collection = services.collections.create("cs-support", "manual")
    assert collection.index_alias.startswith("col_")


def test_他テナントのものは引けない(services):
    """**「他テナントのもの」とは言わない** —— 存在を探れてしまう。"""
    collection = services.collections.create("cs-support", "manual")
    with pytest.raises(NotFound):
        services.collections.get(collection.collection_id, tenant_id="qa-defect")


def test_一覧は自分のぶんだけ(services):
    services.collections.create("cs-support", "a")
    services.collections.create("qa-defect", "b")
    assert [c.collection_name
            for c in services.collections.list_for("cs-support")] == ["a"]


def test_再構築中は書けないが読める(services):
    collection = services.collections.create("cs-support", "manual")
    services.collections.mark_rebuilding(collection.collection_id)
    reloaded = services.collections.get(collection.collection_id)
    assert not reloaded.writable
    assert reloaded.readable


def test_再構築を終えると戻る(services):
    collection = services.collections.create("cs-support", "manual")
    services.collections.mark_rebuilding(collection.collection_id)
    services.collections.mark_active(collection.collection_id)
    assert services.collections.get(collection.collection_id).writable
