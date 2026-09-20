"""提供元の上限に合わせた分割。

外部 API は 128 件、社内 GPU は 32 件まで。**長すぎるテキストはここでは
切らない** —— 切る判断はチャンク分割の仕事なので、ここへ来た時点で
長すぎるものはエラーにする。
"""

from __future__ import annotations

import pytest

from kotonoha.common.errors import InvalidInput
from kotonoha.embed import registry
from kotonoha.embed.batch import estimate_calls, split

EXTERNAL = registry.get("voyage-4")
INTERNAL = registry.get("voyage-4-nano")


def test_空の入力は塊が出ない():
    assert split([], EXTERNAL) == []


def test_上限以内なら1つの塊():
    assert split(["a"] * 10, EXTERNAL) == [list(range(10))]


def test_外部APIは128件で割れる():
    batches = split(["a"] * 200, EXTERNAL)
    assert [len(b) for b in batches] == [128, 72]


def test_社内GPUは32件で割れる():
    batches = split(["a"] * 100, INTERNAL)
    assert [len(b) for b in batches] == [32, 32, 32, 4]


def test_値ではなく添字を返す():
    """**キャッシュの当たり外れと突き合わせる**ので、元の位置が要る。"""
    batches = split(["a", "b", "c"], EXTERNAL)
    assert batches == [[0, 1, 2]]


def test_添字が抜けない():
    batches = split(["a"] * 300, EXTERNAL)
    assert sorted(i for b in batches for i in b) == list(range(300))


def test_上限を明示できる():
    assert [len(b) for b in split(["a"] * 10, EXTERNAL, max_batch=3)] == [3, 3, 3, 1]


def test_長すぎるテキストは弾かれる():
    with pytest.raises(InvalidInput) as caught:
        split(["あ" * 60_000], EXTERNAL)
    assert caught.value.detail["index"] == 0
    assert "チャンクへ分割" in caught.value.message


def test_何件目が長すぎるか分かる():
    with pytest.raises(InvalidInput) as caught:
        split(["短い", "あ" * 60_000], EXTERNAL)
    assert caught.value.detail["index"] == 1


def test_呼び出し回数を見積もれる():
    assert estimate_calls(0, EXTERNAL) == 0
    assert estimate_calls(1, EXTERNAL) == 1
    assert estimate_calls(128, EXTERNAL) == 1
    assert estimate_calls(129, EXTERNAL) == 2


def test_社内GPUのほうが呼び出し回数が多い():
    assert estimate_calls(1000, INTERNAL) > estimate_calls(1000, EXTERNAL)
