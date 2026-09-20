"""経路の振り分けとモデルの台帳。

★ **極秘に外部モデルを名指しされたときは黙って倒さずに弾く。**
   利用側が「voyage-law-2 を使っている」と思い込んだまま別のモデルで
   埋め込まれると、精度の議論が噛み合わなくなる。
"""

from __future__ import annotations

import pytest

from kotonoha.common.errors import ClassificationViolation, InvalidInput
from kotonoha.embed import registry
from kotonoha.embed.router import Router
from kotonoha.embed.selfhosted import SelfHostedProvider
from kotonoha.embed.voyage import VoyageProvider


@pytest.fixture
def router():
    return Router(external=VoyageProvider(), internal=SelfHostedProvider())


# ── 台帳 ─────────────────────────────────────────────────────────
def test_既定はvoyage4():
    assert registry.DEFAULT_MODEL == "voyage-4"


def test_極秘の既定は社内ホストのモデル():
    assert registry.INTERNAL_MODEL == "voyage-4-nano"
    assert registry.get("voyage-4-nano").route == "internal"


def test_知らないモデルは弾かれる():
    with pytest.raises(InvalidInput):
        registry.get("text-embedding-9")


def test_次元はどのモデルも1024():
    assert all(m.dim == 1024 for m in registry.all_models())


def test_極秘で使えるのは社内ホストのモデルだけ():
    usable = registry.usable_for("30")
    assert [m.name for m in usable] == ["voyage-4-nano"]


def test_一般はすべて使える():
    assert len(registry.usable_for("10")) == len(registry.all_models())


# ── 解決 ─────────────────────────────────────────────────────────
def test_省略すると区分に応じた既定になる():
    assert registry.resolve(None, "10").name == "voyage-4"
    assert registry.resolve(None, "30").name == "voyage-4-nano"


def test_極秘に外部モデルを指定すると弾かれる():
    with pytest.raises(ClassificationViolation):
        registry.resolve("voyage-law-2", "30")


def test_極秘に社内モデルを指定するのは通る():
    assert registry.resolve("voyage-4-nano", "30").name == "voyage-4-nano"


# ── 振り分け ─────────────────────────────────────────────────────
def test_一般は外部の提供元へ回る(router):
    model = router.model_for(None, "10")
    provider = router.provider_for(model, "10")
    assert provider.route == "external"


def test_極秘は社内の提供元へ回る(router):
    model = router.model_for(None, "30")
    provider = router.provider_for(model, "30")
    assert provider.route == "internal"


def test_経路だけを知る(router):
    assert router.route_of("10") == "external"
    assert router.route_of("30") == "internal"


def test_事前の点検で弾ける(router):
    """取り込みを始める前に確かめる。**始めてからでは遅い。**"""
    router.check("10", "voyage-4")
    with pytest.raises(ClassificationViolation):
        router.check("30", "voyage-4")


def test_提供元を取り違えると弾かれる(router):
    """外部モデルを社内の提供元へ渡そうとしても通らない。"""
    external_model = registry.get("voyage-4")
    with pytest.raises(ClassificationViolation):
        router.provider_for(external_model, "30")
