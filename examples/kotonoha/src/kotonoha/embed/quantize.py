"""int8 量子化。ベクトルの保管量を 1/4 に落とす。

★ **ADR-003 に反映されていない。**

ADR-003（2025/12/08）は「埋め込みは 1024 次元の float で持つ」と決めている。
2026/05、品質保証部の取り込み量が試算の 3 倍になり、ベクトルの保管費が
予算を超えたため、運用の場で int8 量子化を入れる判断をした。ADR は
書き換えていない（README の仕込み B1）。

判断の経緯は ``docs/runbook/reindex.md`` の末尾に 3 行だけ残っている。
検索の精度は社内評価で 0.82 → 0.81 に落ちたが、許容と判断した ——
**その評価結果もどこにも記録されていない。**

量子化は対称スケーリング（ゼロ点を持たない）。Voyage のベクトルは
長さ 1 に正規化されているので、絶対値の最大でスケールを取れば足りる。
"""

from __future__ import annotations

from kotonoha.embed.models import Vector

#: int8 の表現範囲。対称なので -127〜127 を使う（-128 は使わない）。
_MAX_CODE = 127


def quantize(vector: Vector) -> Vector:
    """float の列を int8 へ落とす。復元値を ``values`` に入れて返す。

    既に量子化済みならそのまま返す（二重に掛けない）。
    """
    if vector.quantized:
        return vector
    peak = max((abs(v) for v in vector.values), default=0.0)
    if peak == 0.0:
        return Vector(values=list(vector.values), model=vector.model,
                      quantized=True, codes=[0] * len(vector.values), scale=1.0)
    scale = peak / _MAX_CODE
    codes = [max(-_MAX_CODE, min(_MAX_CODE, round(v / scale))) for v in vector.values]
    return Vector(
        values=[c * scale for c in codes],
        model=vector.model,
        quantized=True,
        codes=codes,
        scale=scale,
    )


def dequantize(vector: Vector) -> Vector:
    """int8 の列から float へ戻す。量子化していなければそのまま。"""
    if not vector.quantized:
        return vector
    return Vector(
        values=[c * vector.scale for c in vector.codes],
        model=vector.model,
        quantized=False,
    )


def to_bytes(vector: Vector) -> bytes:
    """``t_embedding.vec_i8`` へ入れる形。1 次元 1 バイト。"""
    if not vector.quantized:
        vector = quantize(vector)
    return bytes((c + 128) & 0xFF for c in vector.codes)


def from_bytes(data: bytes, scale: float, model: str) -> Vector:
    """``t_embedding.vec_i8`` から戻す。"""
    codes = [(b - 128) for b in data]
    return Vector(
        values=[c * scale for c in codes],
        model=model,
        quantized=True,
        codes=codes,
        scale=scale,
    )


def error_ratio(before: Vector, after: Vector) -> float:
    """量子化で失われた割合。評価に使う（0 に近いほど良い）。

    社内評価では 1024 次元で 0.004 前後。**この数字を残す場所が
    どこにも無い**ので、毎回計り直している。
    """
    if not before.values:
        return 0.0
    diff = sum((a - b) ** 2 for a, b in zip(before.values, after.values))
    norm = sum(v ** 2 for v in before.values)
    return (diff / norm) ** 0.5 if norm else 0.0
