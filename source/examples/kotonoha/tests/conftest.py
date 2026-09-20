"""テストの共通の足場。

**時刻は必ず固定する。** 締めと保持期間の判定が時刻に依存するので、
固定しないと日付をまたいだときに落ちる。
"""

from __future__ import annotations

import sys
from datetime import datetime
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

from kotonoha.common import clock  # noqa: E402
from kotonoha.common import logging as applog  # noqa: E402
from kotonoha.demo import fixtures  # noqa: E402
from kotonoha.demo.wiring import build  # noqa: E402
from kotonoha.ingest.models import SourceDocument  # noqa: E402

#: テストで使う固定時刻。2026 年 8 月 15 日（本稼働から 4 か月半）。
FIXED_NOW = datetime(2026, 8, 15, 10, 30, 0)


@pytest.fixture(autouse=True)
def frozen_clock():
    """時刻を固定する。全テストに掛ける。"""
    clock.freeze(FIXED_NOW)
    yield
    clock.unfreeze()


@pytest.fixture(autouse=True)
def quiet_logs():
    """ログを黙らせる。**DEBUG を見たいテストは自分で上げる。**"""
    applog.setup("ERROR")


@pytest.fixture
def services():
    """一式を組んでテナント 4 件を入れたもの。"""
    built = build()
    built.secrets = fixtures.install(built)
    return built


@pytest.fixture
def plain_services():
    """量子化とリランクを切ったもの。素の挙動を見るとき。"""
    built = build(quantize=False, rerank=False)
    built.secrets = fixtures.install(built)
    return built


@pytest.fixture
def manual_collection(services):
    """カスタマーサポート部（一般）のコレクション。"""
    return services.collections.create("cs-support", "maintenance-manual")


@pytest.fixture
def secret_collection(services):
    """法務部（極秘）のコレクション。"""
    return services.collections.create("legal-contract", "contract-clause")


@pytest.fixture
def ingested(services, manual_collection):
    """保守マニュアル 2 件を取り込んだ状態。"""
    services.ingest.submit("cs-support", manual_collection.collection_id,
                           fixtures.MANUAL_DOCS)
    services.worker.drain()
    return manual_collection


def make_source(content: str, *, external_id: str = "D1",
                content_type: str = "text/plain",
                title: str = "見本", metadata: dict | None = None) -> SourceDocument:
    """取り込む 1 件を組む。"""
    return SourceDocument(
        external_id=external_id, title=title, content=content,
        content_type=content_type, metadata=metadata or {},
    )
