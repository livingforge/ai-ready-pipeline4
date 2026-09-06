"""あいまい検索（`arp4 grep --fuzzy`）―― 編集距離 k 以内の部分一致。

検体が突くのは 2 つである ―― 展開した正規表現が距離 k **ちょうどまで**しか当たらない
こと、そして索引の篩（鳩の巣の片）が**当たるものを 1 つも落とさない**こと。
"""

from __future__ import annotations

import re

import pytest

from arp4 import fuzzy, lookup, show
from arp4.paths import Paths
from conftest import parsed


def _levenshtein(a: str, b: str) -> int:
    prev = list(range(len(b) + 1))
    for i, ca in enumerate(a, 1):
        cur = [i]
        for j, cb in enumerate(b, 1):
            cur.append(min(prev[j] + 1, cur[j - 1] + 1, prev[j - 1] + (ca != cb)))
        prev = cur
    return prev[-1]


def _within(text: str, key: str, k: int) -> bool:
    """**素朴な定義**（部分文字列のどれかが距離 k 以内）。展開の正解として使う。"""
    return any(_levenshtein(text[i:j], key) <= k
               for i in range(len(text) + 1) for j in range(i, len(text) + 1))


@pytest.mark.parametrize("k", [0, 1, 2])
def test_展開は素朴な定義と一致する(k: int) -> None:
    key = "受注番号"
    found = re.compile(fuzzy.pattern(key, k))
    samples = ["受注番号", "受注蕃号", "受注番号号", "受番号", "受注号", "注番",
               "受注", "番号", "受主番号ある", "xx受注番号xx", "受注番", "受注番",
               "受注番号の桁", "受注 番号", "受主蕃号", "全然違う", "", "番"]
    for text in samples:
        assert (found.search(text) is not None) == _within(text, key, k), (text, k)


def test_全部欠落した形は作らない() -> None:
    assert () not in fuzzy.variants("ab", 2)
    assert re.compile(fuzzy.pattern("ab", 2)).search("zz") is not None   # 距離 2 で当たる
    assert re.compile(fuzzy.pattern("ab", 1)).search("zz") is None


def test_展開が多すぎれば断る() -> None:
    with pytest.raises(ValueError, match="展開が多すぎます"):
        fuzzy.variants("これはとても長い語をあいまいに探そうとしたときの話である" * 3, 2)
    assert len(fuzzy.variants("受注番号一覧表", 2)) < fuzzy.LIMIT      # 普通の語は通る


def test_鳩の巣の片は当たるものを落とさない() -> None:
    """距離 k 以内なら、**どれか 1 片は無傷で本文にある。**"""
    key = "受注番号一覧"
    for k in (1, 2):
        parts = fuzzy.pieces(key, k)
        assert len(parts) == k + 1 and "".join(parts) == key
        found = re.compile(fuzzy.pattern(key, k))
        for text in ["受注番号一覧", "受注蕃号一覧", "受注番一覧", "受注番号一覧表",
                     "受主番号一", "注番号一覧", "受注番号覧", "受注番号号一覧"]:
            if found.search(text):
                assert any(p in text for p in parts), (text, k)
    assert fuzzy.pieces("ab", 2) == []           # 割れない（空の片は何にでも当たる）


# ── 束の中で ────────────────────────────────────────────────────
_SHEET = """\
# A.xlsx / 画面

<!-- source: 資料/A.xlsx / シート: 画面 -->

## 画像の中の文字（読み違えが混ざります）  <!-- a:s1-o1 at=画像 1 枚 -->

- 受注蕃号 ORDER-OOI
- 出荷番号 SHIP-NO

## 表 B2:C4  <!-- a:s1-t1 at=B2:C4 -->

| 項目 | 物理名 |
|---|---|
| 受注番号 | ORDER_NO |
| 受注日 | ORDER_DATE |
"""


def _hits(project: Paths, pattern: str, mode: str, **options) -> list[show.Hit]:
    return lookup.run(project, lookup.Matcher([pattern], **options), mode=mode).hits


def test_一字違いに届き近い順に並ぶ(project: Paths) -> None:
    parsed(project.round("r001"), "資料/A.xlsx/画面.md", _SHEET)

    assert [h.reference.anchor for h in _hits(project, "受注番号", "scan")] == ["s1-t1"]
    for mode in ("scan", "index"):
        hits = _hits(project, "受注番号", mode, fuzzy=1)
        assert [(h.reference.anchor, h.distance) for h in hits] == [("s1-t1", 0), ("s1-o1", 1)]
        assert hits[1].lines == ["- 受注蕃号 ORDER-OOI"]
        assert hits[1].total == 1                      # 出荷番号 は距離 2


def test_揺れを畳んだうえであいまいに(project: Paths) -> None:
    parsed(project.round("r001"), "資料/A.xlsx/画面.md", _SHEET)

    assert not _hits(project, "order-oo1", "scan", fuzzy=1)        # 大小が違う
    for mode in ("scan", "index"):
        [hit] = _hits(project, "order-oo1", mode, fuzzy=1, loose=True)
        assert hit.reference.anchor == "s1-o1" and hit.distance == 1
        assert hit.lines == ["- 受注蕃号 ORDER-OOI"]


def test_短い語は候補を絞らず全部確かめる(project: Paths) -> None:
    parsed(project.round("r001"), "資料/A.xlsx/画面.md", _SHEET)

    hits = _hits(project, "受注", "index", fuzzy=2)          # 片に割れない
    assert {h.reference.anchor for h in hits} == {"s1-o1", "s1-t1"}


def test_正規表現とは併用できない() -> None:
    with pytest.raises(ValueError, match="fuzzy"):
        lookup.Matcher(["a.b"], regex=True, fuzzy=1)
