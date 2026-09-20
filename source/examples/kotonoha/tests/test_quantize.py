"""int8 量子化。

★ **ADR-003 は「1024 次元の float」と書いており、量子化に触れていない。**
   実装だけが先に進んでいる（README の仕込み B1）。この検証は実装の側の
   仕様であって、設計文書と突き合わせることはできない。
"""

from __future__ import annotations

from kotonoha.embed.models import Vector
from kotonoha.embed.quantize import (dequantize, error_ratio, from_bytes,
                                     quantize, to_bytes)


def _vector(values: list[float]) -> Vector:
    return Vector(values=values, model="voyage-4")


def test_量子化すると印が立つ():
    result = quantize(_vector([0.5, -0.25, 0.125]))
    assert result.quantized
    assert result.codes


def test_二重に掛からない():
    once = quantize(_vector([0.5, -0.25]))
    twice = quantize(once)
    assert twice is once


def test_符号が保たれる():
    result = quantize(_vector([0.5, -0.5, 0.0]))
    assert result.codes[0] > 0
    assert result.codes[1] < 0
    assert result.codes[2] == 0


def test_最大値が127になる():
    result = quantize(_vector([1.0, 0.5, -0.25]))
    assert max(abs(c) for c in result.codes) == 127


def test_ゼロベクトルは落ちない():
    result = quantize(_vector([0.0, 0.0, 0.0]))
    assert result.codes == [0, 0, 0]
    assert result.scale == 1.0


def test_復元すると元に近い():
    original = _vector([0.5, -0.25, 0.125, 0.9])
    restored = dequantize(quantize(original))
    assert not restored.quantized
    for before, after in zip(original.values, restored.values):
        assert abs(before - after) < 0.01


def test_誤差が小さい():
    """1024 次元での実測は 0.004 前後。**この数字を残す場所が無い。**"""
    values = [((i * 37) % 100 - 50) / 100 for i in range(1024)]
    original = _vector(values)
    ratio = error_ratio(original, quantize(original))
    assert ratio < 0.01


def test_量子化していないものの誤差は0():
    original = _vector([0.5, 0.25])
    assert error_ratio(original, original) == 0.0


def test_バイト列にすると1次元1バイト():
    original = _vector([0.5, -0.25, 0.125] * 8)
    data = to_bytes(original)
    assert len(data) == len(original.values)
    assert isinstance(data, bytes)


def test_バイト列から戻せる():
    original = quantize(_vector([0.5, -0.25, 0.125]))
    restored = from_bytes(to_bytes(original), original.scale, "voyage-4")
    assert restored.codes == original.codes


def test_量子化していなくてもバイト列にできる():
    data = to_bytes(_vector([0.5, -0.25]))
    assert len(data) == 2


def test_保管量が4分の1になる():
    """float32 が 4 バイト、int8 が 1 バイト。**これが入れた理由。**"""
    original = _vector([0.1] * 1024)
    assert len(to_bytes(original)) * 4 == len(original.values) * 4
