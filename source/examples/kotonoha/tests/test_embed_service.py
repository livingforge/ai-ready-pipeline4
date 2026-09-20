"""エンベディング生成の入口。

**課金に数えるのは提供元へ投げた件数だけ。** キャッシュに当たった分は
数えない（業務ルール「利用量の数え方」）。
"""

from __future__ import annotations

import pytest

from kotonoha.common.errors import ClassificationViolation, InvalidInput
from kotonoha.embed.models import EmbedRequest


def _request(texts, classification="10", model=None, input_type="document"):
    return EmbedRequest(texts=texts, model=model,
                        classification=classification,
                        input_type=input_type, tenant_id="cs-support")


def test_件数どおりのベクトルが返る(services):
    result = services.embed.embed(_request(["点検", "手順", "異音"]))
    assert len(result.vectors) == 3


def test_同じ文字列は同じベクトルになる(plain_services):
    """疑似の提供元は決定的。**順位の比較ができる程度には安定している。**"""
    first = plain_services.embed.embed(_request(["軸受"]))
    second = plain_services.embed.embed(_request(["軸受"], classification="20"))
    assert first.vectors[0].values == second.vectors[0].values


def test_ベクトルの長さは1に正規化されている(plain_services):
    result = plain_services.embed.embed(_request(["点検の手順"]))
    norm = sum(v * v for v in result.vectors[0].values) ** 0.5
    assert abs(norm - 1.0) < 1e-6


def test_検索語と文書でベクトルが違う(plain_services):
    """``input_type`` を省くと精度が落ちる。PoC で 2 週間探した原因。"""
    doc = plain_services.embed.embed(_request(["異音"], input_type="document"))
    query = plain_services.embed.embed(_request(["異音"], input_type="query"))
    assert doc.vectors[0].values != query.vectors[0].values


def test_1回目は課金され2回目は蓄積になる(services):
    first = services.embed.embed(_request(["キャッシュの確認"]))
    second = services.embed.embed(_request(["キャッシュの確認"]))
    assert (first.billed_count, first.cached_count) == (1, 0)
    assert (second.billed_count, second.cached_count) == (0, 1)


def test_一部だけ当たる(services):
    services.embed.embed(_request(["ある"]))
    result = services.embed.embed(_request(["ある", "ない"]))
    assert result.billed_count == 1
    assert result.cached_count == 1
    assert len(result.vectors) == 2


def test_量子化が掛かる(services):
    """★ ADR-003 は 1024 次元 float のまま。"""
    result = services.embed.embed(_request(["量子化の確認"]))
    assert result.vectors[0].quantized


def test_量子化を切れる(plain_services):
    result = plain_services.embed.embed(_request(["量子化なし"]))
    assert not result.vectors[0].quantized


def test_極秘は社内経路になる(services):
    result = services.embed.embed(_request(["契約の条項"], classification="30"))
    assert result.route == "internal"
    assert result.model == "voyage-4-nano"


def test_極秘に外部モデルを指定すると弾かれる(services):
    with pytest.raises(ClassificationViolation):
        services.embed.embed(_request(["条項"], classification="30",
                                      model="voyage-law-2"))


def test_長すぎるテキストは弾かれる(services):
    """**先にチャンクへ分割してから来ること。**"""
    with pytest.raises(InvalidInput):
        services.embed.embed(_request(["あ" * 60_000]))


def test_空の入力は空を返す(services):
    result = services.embed.embed(_request([]))
    assert result.vectors == []


def test_128件を超えても分割して通る(services):
    """提供元の上限は 128 件。``batch.split`` が分ける。"""
    result = services.embed.embed(_request([f"文書{i}" for i in range(200)]))
    assert len(result.vectors) == 200


def test_社内GPUは32件ずつに分かれる(services):
    result = services.embed.embed(
        _request([f"条項{i}" for i in range(100)], classification="30"))
    assert len(result.vectors) == 100
    assert result.route == "internal"


def test_監査ログに残る_本文は残らない(services):
    services.embed.embed(_request(["監査の確認"]))
    entry = services.audit.entries[-1]
    assert entry.operation == "embed"
    assert entry.route == "external"
    assert not hasattr(entry, "body")


def test_検索語は1本だけ埋め込める(services):
    vector = services.embed.embed_query("異音", "voyage-4", "10")
    assert vector.dim == 1024
