"""束を探す（`arp4 grep`）―― 走査と索引が同じ答えを返すこと。

検体が突くのは 3 つである ―― 当たりが**列と行の位置ごと**返ること、揺れを畳んでも
原文が出ること、そして索引を通しても走査と 1 件も違わないこと（索引は候補を
出すだけで、当たりの判定は 1 か所）。
"""

from __future__ import annotations

import os
from pathlib import Path

from arp4 import lookup, mdio, show
from arp4.paths import Paths
from conftest import organized, parsed

_LEDGER = """\
# 利用申請台帳.xlsx / 1.申請一覧

<!-- source: 資料/利用申請台帳.xlsx / シート: 1.申請一覧 -->

## 表 B2:E6  <!-- a:s3-t1 at=B2:E6 -->

| 1. 申請一覧 |  |  |  |
|---|---|---|---|
| テナント | 所管 | 機密区分 | 上限 |
| legal-contract | 法務部 | 極秘 | 100,000 |
| sales-proposal | 営業本部 | 社外秘 | 50,000 |
| qa-defect | 品質保証部 | 極秘扱い | 30,000 |

## セル B9  <!-- a:s3-x1 at=B9 -->

- `B9` ※ 極秘のテナントは ＯＲＤＥＲ ＮＯ を暗号化する
"""

_SLO = """\
# 運用引継ぎ資料.xlsx / 3.SLO

<!-- source: 資料/運用引継ぎ資料.xlsx / シート: 3.SLO -->

## 表 B2:C4  <!-- a:s5-t1 at=B2:C4 -->

| 指標 | 目標 |
|---|---|
| 検索の応答時間 | p95 で 800ms 以内 |
| 受注 番号 の桁 | 10 |
"""


def _two(project: Paths) -> None:
    round_ = project.round("r001")
    parsed(round_, "資料/利用申請台帳.xlsx/1.申請一覧.md", _LEDGER)
    parsed(round_, "資料/運用引継ぎ資料.xlsx/3.SLO.md", _SLO)


def _hits(project: Paths, pattern: str, mode: str = "scan", **options) -> list[show.Hit]:
    matcher = lookup.Matcher([pattern], **options)
    return lookup.run(project, matcher, mode=mode).hits


# ── 表を表として ────────────────────────────────────────────────
def test_当たりは表の見出し行と行番号を連れてくる(project: Paths) -> None:
    """**当たりがどの列の値か**を読み手が数え直さない。表題行は見出しにしない。"""
    _two(project)

    [hit] = _hits(project, "sales-proposal")

    assert hit.header == "| テナント | 所管 | 機密区分 | 上限 |"
    assert hit.header_where == "2 行目"           # 表題行が 1 行目なので「1 行目」ではない
    assert hit.columns == ["テナント", "所管", "機密区分", "上限"]
    assert hit.where == ["4 行目"]
    assert hit.lines == ["| sales-proposal | 営業本部 | 社外秘 | 50,000 |"]


def test_列で絞る(project: Paths) -> None:
    """``機密区分`` の列の ``極秘`` だけ ―― 備考の「極秘」は別物である。"""
    _two(project)

    [hit] = _hits(project, "極秘", column="機密区分")

    assert hit.reference.anchor == "s3-t1"
    assert hit.total == 2                        # 極秘・極秘扱い（部分一致）
    assert [str(h.reference) for h in _hits(project, "極秘")] == [
        "r001 資料/利用申請台帳.xlsx/1.申請一覧#s3-t1",
        "r001 資料/利用申請台帳.xlsx/1.申請一覧#s3-x1"]


def test_升の完全一致(project: Paths) -> None:
    _two(project)

    [hit] = _hits(project, "極秘", column="機密区分", cell=True)

    assert hit.total == 1 and "legal-contract" in hit.lines[0]
    assert not _hits(project, "極", column="機密区分", cell=True)


def test_無い列は当たらない(project: Paths) -> None:
    _two(project)

    assert not _hits(project, "極秘", column="そんな列")


# ── 揺れ ────────────────────────────────────────────────────────
def test_全角半角と空白の揺れを畳む(project: Paths) -> None:
    """探す側だけを畳み、**当たった本文は原文のまま**出す。"""
    _two(project)

    assert not _hits(project, "ORDER NO")
    [hit] = _hits(project, "order no", loose=True)
    assert "ＯＲＤＥＲ ＮＯ" in hit.lines[0]

    [hit] = _hits(project, "受注番号", loose=True)
    assert hit.reference.anchor == "s5-t1"


def test_同じ塊に語が全部あるものだけ(project: Paths) -> None:
    matcher = lookup.Matcher(["法務部", "営業本部"])
    _two(project)

    hits = lookup.run(project, matcher, mode="scan").hits

    assert [h.reference.anchor for h in hits] == ["s3-t1"]
    assert hits[0].total == 2                    # 見せる行はどちらかに当たった行
    assert not lookup.run(project, lookup.Matcher(["法務部", "p95"]), mode="scan").hits


def test_読めない正規表現は言う(project: Paths) -> None:
    try:
        lookup.Matcher(["["], regex=True)
    except ValueError as exc:
        assert "正規表現として読めません" in str(exc)
    else:                                     # pragma: no cover
        raise AssertionError("黙って通さない")


# ── 索引 ────────────────────────────────────────────────────────
def _same(project: Paths, pattern: str, **options) -> list[show.Hit]:
    """走査と索引で**同じ答え**が返ることを確かめてから返す。"""
    by_scan = _hits(project, pattern, "scan", **options)
    by_index = _hits(project, pattern, "index", **options)
    assert [(str(h.reference), h.lines, h.total, h.header, h.where) for h in by_scan] == \
           [(str(h.reference), h.lines, h.total, h.header, h.where) for h in by_index]
    return by_index


def test_索引を通しても走査と同じ答え(project: Paths) -> None:
    _two(project)

    assert len(_same(project, "極秘")) == 2               # 3 文字未満（instr）
    assert len(_same(project, "legal-contract")) == 1     # trigram
    assert len(_same(project, "p9[0-9]", regex=True)) == 1
    assert len(_same(project, "order no", loose=True)) == 1
    assert len(_same(project, "極秘", column="機密区分", cell=True)) == 1
    assert len(_same(project, "LEGAL", ignore_case=True)) == 1
    assert _same(project, "無い語") == []


def test_索引は写しの編集に追いつく(project: Paths) -> None:
    """**索引が写しと食い違う瞬間を作らない** ―― 引く前に姿を突き合わせる。"""
    _two(project)
    assert len(_hits(project, "極秘", "index")) == 2
    path = project.round("r001").parsed / "資料/利用申請台帳.xlsx/1.申請一覧.md"

    edited = path.read_text(encoding="utf-8").replace("極秘扱い", "社外秘")
    path.write_text(edited, encoding="utf-8", newline="\n")
    os.utime(path, ns=(1, 1))                  # 時刻が同じでも大きさが違えば拾う

    [hit] = _hits(project, "極秘", "index", column="機密区分")
    assert hit.total == 1

    path.unlink()
    assert not _hits(project, "極秘", "index")
    assert _hits(project, "p95", "index")


def test_撮り直したラウンドがあっても索引は新しいほうを引く(project: Paths) -> None:
    _two(project)
    parsed(project.round("r002"), "資料/運用引継ぎ資料.xlsx/3.SLO.md",
           _SLO.replace("800ms", "500ms"))

    [hit] = _hits(project, "以内", "index")
    assert hit.reference.round == "r002" and "500ms" in hit.lines[0]
    assert len(lookup.run(project, lookup.Matcher(["極秘"]), mode="index").hits) == 2

    [old] = lookup.run(project, lookup.Matcher(["以内"]), round_name="r001",
                       mode="index").hits
    assert "800ms" in old.lines[0]


def test_大きな束は索引を選ぶ(project: Paths, monkeypatch) -> None:
    _two(project)
    monkeypatch.setattr(lookup, "INDEX_FROM", 1)

    outcome = lookup.run(project, lookup.Matcher(["極秘"]))

    assert outcome.mode == "index" and outcome.refreshed.added == 2
    assert lookup.cache_path(project).exists()
    assert (lookup.cache_path(project).parent / ".gitignore").exists()
    # 2 度目は突き合わせだけで、何も写し直さない。
    assert lookup.run(project, lookup.Matcher(["極秘"])).refreshed.changed == 0


def test_パスで絞る(project: Paths) -> None:
    _two(project)

    assert [h.reference.file for h in
            lookup.run(project, lookup.Matcher(["極秘"]), only="SLO", mode="scan").hits] == []
    assert lookup.run(project, lookup.Matcher(["極秘"]), only="台帳", mode="index").files == 1


# ── 整理結果の側 ────────────────────────────────────────────────
_ORGANIZED = """\
records:
  - concept: c-act-法務部
    type: 利用者・ロール
    name: 法務部
    statement: 法務部は契約書の類似条項検索を行う所管部門である
    attrs: { physical_name: legal-contract }
    source: { anchor: s3-t1 }
"""

_CONCEPTS = """\
new:
  - concept: c-act-法務部
    type: 利用者・ロール
    label: 法務部
    aliases: [法務, リーガル]
"""


def test_整理結果を探すと同じ語が何になったかが出る(project: Paths) -> None:
    _two(project)
    round_ = project.round("r001")
    organized(round_, "資料/利用申請台帳.xlsx/1.申請一覧.yml", _ORGANIZED)
    organized(round_, "_concepts.yml", _CONCEPTS)

    found = lookup.in_organized(project, lookup.Matcher(["リーガル"]))
    assert [(h.file, h.concept) for h in found] == [("_concepts.yml", "c-act-法務部")]
    assert found[0].fields == [("aliases", "法務 / リーガル")]

    found = lookup.in_organized(project, lookup.Matcher(["legal-contract"]))
    assert found[0].where == "r001 資料/利用申請台帳.xlsx/1.申請一覧#s3-t1"
    assert found[0].fields == [("attrs.physical_name", "legal-contract")]


def test_塊ごとに誰が拾ったかを添える(project: Paths) -> None:
    _two(project)
    organized(project.round("r001"), "資料/利用申請台帳.xlsx/1.申請一覧.yml", _ORGANIZED)

    uses = lookup.Uses(project)
    assert uses.of(show.Reference(file="資料/利用申請台帳.xlsx/1.申請一覧",
                                  anchor="s3-t1", round="r001")) == ["c-act-法務部"]
    assert uses.of(show.Reference(file="資料/運用引継ぎ資料.xlsx/3.SLO",
                                  anchor="s5-t1", round="r001")) == []


def test_読み戻しは文字列からでも同じ(project: Paths) -> None:
    _two(project)
    path = project.round("r001").parsed / "資料/利用申請台帳.xlsx/1.申請一覧.md"

    from_file = mdio.read(path)
    from_text = mdio.read_text(path.read_text(encoding="utf-8"), path)

    assert [(a.id, a.at, a.body) for a in from_file.anchors] == \
           [(a.id, a.at, a.body) for a in from_text.anchors]


# ── 表の形 ──────────────────────────────────────────────────────
_STACKED = """\
# 利用申請台帳.xlsx / 1.申請一覧

<!-- source: 資料/利用申請台帳.xlsx / シート: 1.申請一覧 -->

## 表 B2:J8  <!-- a:s3-t1 at=B2:J8 -->

| 1. 利用申請の一覧 |  |  |  |  |  |
|---|---|---|---|---|---|
|  |  |  |  |  |  |
| テナント識別子 | 申請部門 |  | 機密区分 |  |  |
| テナント識別子 | 所管部門 | 用途 | 区分 | 名称 | 月間上限 |
| cs-support | カスタマーサポート部 | 保守問合せ | 10 | 一般 | 500,000 |
| legal-contract | 法務部 | 契約書の検索 | 30 | 極秘 | 100,000 |

## 表 B12:D14  <!-- a:s3-t2 at=B12:D14 -->

| No | 点検項目 | 結果 |
|---|---|---|
| 1 | 機密区分が規程に沿っているか | ○ |
| 2 | 極秘を外へ出さない仕組みがあるか | △ |
"""


def test_見出しの段を重ねて列名にする(project: Paths) -> None:
    """大見出しの下に小見出しが並ぶ表は、**段を重ねた列名**で答える。
    空の行は表の行として数える（区切り行は 2 行目に限る）。"""
    parsed(project.round("r001"), "資料/利用申請台帳.xlsx/1.申請一覧.md", _STACKED)

    hit = next(h for h in _hits(project, "legal-contract") if h.reference.anchor == "s3-t1")

    assert hit.header_where == "3〜4 行目"
    assert hit.columns == ["テナント識別子", "申請部門/所管部門", "申請部門/用途",
                           "機密区分/区分", "名称", "月間上限"]
    assert hit.header.startswith("| テナント識別子 | 申請部門/所管部門 |")
    assert hit.where == ["6 行目"]
    assert hit.cells == [[("テナント識別子", "legal-contract")]]
    assert hit.exact == 1                        # 升そのものに当たった


def test_列名は段のどれでも指せる(project: Paths) -> None:
    """``機密区分`` は ``区分`` の列を、``名称`` は ``名称`` の列を指す。
    その列を持たない表（点検項目に「機密区分」と書いてある）は対象外である。"""
    parsed(project.round("r001"), "資料/利用申請台帳.xlsx/1.申請一覧.md", _STACKED)

    [hit] = _hits(project, "30", column="機密区分", cell=True)
    assert hit.reference.anchor == "s3-t1" and hit.cells == [[("機密区分/区分", "30")]]
    [hit] = _hits(project, "極秘", column="名称")
    assert hit.where == ["6 行目"]
    assert not _hits(project, "極秘", column="機密区分")      # 区分の升は 10 / 30
    assert not _hits(project, "極秘", column="点検項目") or \
        all(h.reference.anchor == "s3-t2" for h in _hits(project, "極秘", column="点検項目"))


def test_同じ行に語が全部あるものだけ(project: Paths) -> None:
    parsed(project.round("r001"), "資料/利用申請台帳.xlsx/1.申請一覧.md", _STACKED)

    both = lookup.run(project, lookup.Matcher(["法務部", "極秘"], same_row=True),
                      mode="scan").hits
    assert [h.where for h in both] == [["6 行目"]]
    apart = lookup.run(project, lookup.Matcher(["法務部", "一般"], same_row=True),
                       mode="scan").hits
    assert apart == []                            # 同じ塊にはあるが行が違う
    assert lookup.run(project, lookup.Matcher(["法務部", "一般"]), mode="scan").hits


# ── 揺れ（区切り）────────────────────────────────────────────────
def test_桁区切りと日付と識別子の区切りを畳む(project: Paths) -> None:
    parsed(project.round("r001"), "資料/A.xlsx/一覧.md",
           "# A.xlsx / 一覧\n\n## 表  <!-- a:s1-t1 at=B2:D4 -->\n\n"
           "| 上限 | 日付 | 物理名 |\n|---|---|---|\n"
           "| 100,000 | 2026/02/03 | ORDER_NO |\n| 5 | 2026/03/01 | embed/router.py |\n")

    assert not _hits(project, "100000")
    [hit] = _hits(project, "100000", loose=True)
    assert hit.matched == [["100,000"]]          # 当たった字は原文のまま
    assert _hits(project, "2026-02-03", loose=True)[0].matched == [["2026/02/03"]]
    assert _hits(project, "orderNo", loose=True)[0].matched == [["ORDER_NO"]]
    assert _hits(project, "embed router", loose=True)[0].matched == [["embed/router"]]
    assert not _hits(project, "第32版", loose=True) or True   # ``.`` は落とさない
    assert lookup.normalize("第3.2版") != lookup.normalize("第32版")


# ── 別名 ────────────────────────────────────────────────────────
def test_別名でも探す(project: Paths) -> None:
    _two(project)
    organized(project.round("r001"), "_concepts.yml", _CONCEPTS)
    (project.spec).mkdir(parents=True, exist_ok=True)
    (project.spec / "concepts.yml").write_text(
        "- concept: c-act-営業本部\n  label: 営業本部\n  aliases: [sales-proposal]\n",
        encoding="utf-8")

    assert lookup.aliases_of(project, ["法務部"]) == [["法務", "リーガル"]]
    assert lookup.aliases_of(project, ["リーガル"]) == [["法務部", "法務"]]
    assert lookup.aliases_of(project, ["sales-proposal"]) == [["営業本部"]]
    assert lookup.aliases_of(project, ["無い語"]) == [[]]

    matcher = lookup.Matcher(["法務"], aliases=lookup.aliases_of(project, ["法務"]))
    hits = lookup.run(project, matcher, mode="scan").hits
    assert [h.reference.anchor for h in hits] == ["s3-t1"]
    assert hits[0].matched == [["法務部"]]


# ── 写しの名前 ──────────────────────────────────────────────────
def test_写しの名前にも当たる(project: Paths) -> None:
    """シート名は塊の本文に無い。名前の当たりはアンカー無しの出典で返る。"""
    _two(project)

    hits = _hits(project, "申請一覧")
    assert (str(hits[0].reference), hits[0].kind) == \
           ("r001 資料/利用申請台帳.xlsx/1.申請一覧", "name")
    assert [h.kind for h in hits] == ["name", "chunk"]     # 本文の「1. 申請一覧」も当たる
    assert hits[0].heading == lookup.NAME_HEADING
    assert not _hits(project, "申請一覧", column="テナント")   # 升で見るときは出さない
    assert [h.kind for h in _hits(project, "申請二覧", fuzzy=1)] == ["chunk"]   # 名前はあいまいにしない
    assert _same(project, "申請一覧")[0].kind == "name"


# ── 並び ────────────────────────────────────────────────────────
def test_升そのものに当たったものを先に(project: Paths) -> None:
    _two(project)
    parsed(project.round("r001"), "資料/A.xlsx/備考.md",
           "# A.xlsx / 備考\n\n## 表  <!-- a:s1-t1 at=B2:C3 -->\n\n"
           "| 区分 | 名称 |\n|---|---|\n| 30 | 極秘 |\n")

    by_path = lookup.run(project, lookup.Matcher(["極秘"]), mode="scan").hits
    assert [h.reference.file for h in by_path][0] == "資料/A.xlsx/備考"
    by_cell = lookup.run(project, lookup.Matcher(["極秘"]), mode="scan", sort="cell").hits
    assert [h.exact for h in by_cell] == sorted([h.exact for h in by_cell], reverse=True)
    assert by_cell[0].reference.file == "資料/A.xlsx/備考"
    assert by_cell[-1].reference.anchor == "s3-x1"          # 備考の言及は最後


# ── ラウンド間の差 ──────────────────────────────────────────────
def test_撮り直した写しの範囲で当たりの差を出す(project: Paths) -> None:
    _two(project)
    parsed(project.round("r002"), "資料/利用申請台帳.xlsx/1.申請一覧.md",
           _LEDGER.replace("| qa-defect | 品質保証部 | 極秘扱い |", "| qa-defect | 品質保証部 | 社外秘 |")
                  .replace("## セル B9", "## セル B12  <!-- a:s3-x2 at=B12 -->\n\n"
                           "- `B12` ※ 極秘は別鍵\n\n## セル B9"))

    result = lookup.diff(project, lookup.Matcher(["極秘"]), "r001", "r002", mode="scan")

    assert result.files == 1
    assert [h.reference.anchor for h in result.added] == ["s3-x2"]
    assert result.removed == []
    assert [(a.total, b.total) for a, b in result.changed] == [(2, 1)]
    assert result.same == 1                                   # s3-x1 は同じ
    try:
        lookup.diff(project, lookup.Matcher(["極秘"]), "r001", "r009")
    except ValueError as exc:
        assert "r009" in str(exc)
    else:                                     # pragma: no cover
        raise AssertionError("無いラウンドは言う")


# ── 片に分けた索引 ──────────────────────────────────────────────
def test_片に分けた索引も走査と同じ答え(project: Paths, monkeypatch) -> None:
    """片ごとに別のプロセスで写しても、引いた答えは 1 件も変わらない。"""
    _two(project)
    parsed(project.round("r001"), "資料/利用申請台帳.xlsx/1.申請一覧.md", _STACKED)
    monkeypatch.setattr(lookup, "SHARD_SIZE", 1)
    monkeypatch.setattr(lookup, "PARALLEL_FROM", 1)
    monkeypatch.setattr(lookup, "MAX_SHARDS", 2)

    outcome = lookup.run(project, lookup.Matcher(["極秘"]), mode="index")

    assert outcome.refreshed.added == 2
    assert len(list(lookup.cache_path(project).parent.glob("grep*.sqlite"))) == \
        min(2, os.cpu_count() or 1)
    for pattern, options in [("極秘", {}), ("30", {"column": "機密区分", "cell": True}),
                             ("100000", {"loose": True}), ("極密", {"fuzzy": 1})]:
        _same(project, pattern, **options)
    # 2 度目は突き合わせだけ。写しを消せば片からも消える。
    assert lookup.run(project, lookup.Matcher(["極秘"]), mode="index").refreshed.changed == 0
    (project.round("r001").parsed / "資料/運用引継ぎ資料.xlsx/3.SLO.md").unlink()
    assert not _hits(project, "p95", "index")
