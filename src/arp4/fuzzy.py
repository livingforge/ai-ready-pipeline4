"""あいまい検索 ―― 編集距離 k 以内の部分一致を、**正規表現に展開して**当てる。

OCR の読み（``o1``）と手打ちの設計書には 1 字違いが普通にある ―― ``受注番号`` が
``受注蕃号`` や ``受注番号号`` になっていて、部分一致では届かない。欲しいのは
「1 文字の置換・欠落・混入まで許す部分一致」で、これは編集距離（Levenshtein）
k 以内の部分文字列の探索である。

**動的計画法で 1 字ずつ見ると Python では遅い**（1 塊 2 KB × 数十万塊）。代わりに
「距離 k 以内で当たる形」を全部並べた正規表現にする ―― ``受注`` の 1 字違いは
``.注`` ``受.`` ``注`` ``受`` ``.受注`` ``受.注`` ``受注.`` の 7 通りで、これを ``|`` で
繋いだ 1 本の正規表現は C の速さで走る。展開の数は語長 L に対して k=1 で 3L+1、
k=2 でその 2 乗ほどなので、上限を置いて超えたら断る（黙って遅くならない）。

索引を引くときは鳩の巣原理で候補を絞る ―― 語を k+1 個に割れば、**どれか 1 片は
無傷で本文に残っている**。片を OR で引いてから、ここで作った正規表現で確かめる。
"""

from __future__ import annotations

import re

#: 展開の上限。超えると正規表現が長くなりすぎて、当たり判定が篩より遅くなる。
LIMIT = 4000

#: 任意の 1 文字（置換・混入）。文字そのものと区別するために sentinel で持つ。
_ANY = None


def variants(key: str, k: int) -> set[tuple[str | None, ...]]:
    """距離 k 以内で当たる形の集合。要素は文字か :data:`_ANY` の並び。"""
    start: tuple[str | None, ...] = tuple(key)
    results = {start}
    frontier = {start}
    for _ in range(k):
        grown: set[tuple[str | None, ...]] = set()
        for shape in frontier:
            for i in range(len(shape)):
                grown.add(shape[:i] + shape[i + 1:])             # 欠落
                grown.add(shape[:i] + (_ANY,) + shape[i + 1:])   # 置換
            for i in range(len(shape) + 1):
                grown.add(shape[:i] + (_ANY,) + shape[i:])       # 混入
        grown.discard(())                        # 全部欠落した形は何にでも当たる
        results |= grown
        frontier = grown
        if len(results) > LIMIT:
            raise ValueError(
                f"あいまい検索の展開が多すぎます（{key}、k={k}）。"
                "語を短くするか --fuzzy の数を小さくしてください")
    return results


def pattern(key: str, k: int) -> str:
    """距離 k 以内の部分一致を表す正規表現（``search`` で使う）。"""
    if k <= 0:
        return re.escape(key)
    shapes = sorted(variants(key, k), key=lambda s: (-len(s), [c or "" for c in s]))
    return "|".join("".join("." if c is _ANY else re.escape(c) for c in shape)
                    for shape in shapes)


def pieces(key: str, k: int) -> list[str]:
    """鳩の巣原理の片 ―― k+1 個に割った語。**どれか 1 片は無傷で残る。**

    語が k+1 文字に満たなければ割れない（空の片は何にでも当たる）ので空を返す。
    そのときは候補を絞れず、束を全部確かめることになる。
    """
    n = k + 1
    if len(key) < n:
        return []
    size, extra = divmod(len(key), n)
    out: list[str] = []
    at = 0
    for i in range(n):
        width = size + (1 if i < extra else 0)
        out.append(key[at:at + width])
        at += width
    return out
