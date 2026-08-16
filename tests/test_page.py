"""④の見た目 ―― **Excel で読まれてきたものを、Excel として読める形で出す。**

ここが見ているのは体裁そのものではなく、**体裁が事実を歪めないこと**である。

* 出典のリンクは**実在する写しにだけ**張る（辿れない飛び先を作らない）
* 概要は md と html で**同じ事実**を出す（片方だけが言うことを作らない）
* 「出ていない写し」は**3 通りに言い分ける**（次の一手が正反対のものを混ぜない）
* 見取り図の線は**正本にある参照の数**と一致する（絵が資料より多くを語らない）
"""

from __future__ import annotations

import re
from pathlib import Path

import pytest

from arp4 import figure as figure_module
from arp4 import holes as holes_module
from arp4 import metamodel as mm
from arp4 import origins as origins_module
from arp4 import page as page_module
from arp4 import publish as publish_module
from arp4.paths import Paths
from arp4.spec import Spec

from conftest import write

_DOC = {"name": "req", "title": "要件定義書", "phase": "要件定義",
        "sections": [{"heading": "機能要件", "type": "requirement",
                      "where": {"kind": "機能"},
                      "columns": ["req_id", "name", "statement", "source"]}]}


def _source(file: str, anchor: str = "s1-t1") -> list[dict]:
    return [{"round": "r001", "file": file, "anchor": anchor}]


def _phase(spec: Spec) -> str:
    """``out/<番号>_<工程>/`` の番号は ``layers`` の並びで決まる。

    **パックに工程を足すと繰り下がる**（3.11.0 で `企画` が入り `1_要件定義` →
    `2_要件定義`）。番号を期待値に焼き付けると、ここで見たいこと（出典のリンク・
    タブの開き方）と関係のない書き換えが毎回発生する。
    """
    return f"{publish_module.phases(spec).index('要件定義') + 1}_要件定義"


@pytest.fixture(scope="module")
def packed() -> mm.Metamodel:
    """**様式まで含めたメタモデル。** `model` はパックの鎖を持たないので
    `catalog` が文書定義を 1 つも返さない ―― 束を出す試験はこちらで解く。"""
    resolved, findings = mm.resolve({"extends": "jp-sier-std", "version": 3})
    assert not [f for f in findings if f.level == "error"]
    return resolved


@pytest.fixture
def project_with_copies(project: Paths) -> Paths:
    """写しが 3 枚。**1 枚は使われ、1 枚は対象外宣言、1 枚は未整理。**"""
    round_ = project.round("r001")
    for name in ("資料/A.xlsx/受注", "資料/A.xlsx/表紙", "資料/B.py"):
        write(round_.parsed / f"{name}.md", f"# {name}\n\n<!-- a:s1-t1 -->\n")
    write(round_.organized / "資料/A.xlsx/受注.yml",
          "records:\n  - anchor: s1-t1\n    type: requirement\n"
          "    concept: c-1\n    name: 受注する\n")
    write(round_.organized / "資料/A.xlsx/表紙.yml",
          "records: []\n\nout_of_scope:\n  - anchor: s1-t1\n"
          "    reason: 表紙・改訂履歴（仕様ではない）\n")
    return project


@pytest.fixture
def spec_with_copies(packed: mm.Metamodel, project_with_copies: Paths) -> Spec:
    return Spec(metamodel=packed, paths=project_with_copies, items=[
        {"id": "req-1", "type": "requirement", "req_id": "FR-001",
         "kind": "機能", "name": "受注する", "statement": "受注できること",
         "status": "review", "source": _source("資料/A.xlsx/受注")},
        {"id": "req-2", "type": "requirement", "req_id": "FR-002",
         "kind": "機能", "name": "消えた資料", "statement": "写しが残っていない",
         "status": "review", "source": _source("資料/消えた.xlsx/シート")},
    ], relations=[])


# ── 出典のリンク ────────────────────────────────────────────────
def test_出典は実在する写しへのリンクになる(spec_with_copies: Spec) -> None:
    """升には最初から**辿れる形**が出ていたのに、文字列のままだった（実測 580 セル）。"""
    out = spec_with_copies.paths.out
    publish_module.publish(spec_with_copies, out, names=["要件定義書"])
    page = (out / _phase(spec_with_copies) / "要件定義書.html").read_text(encoding="utf-8")

    assert ('<a href="../../rounds/r001/parsed/資料/A.xlsx/受注.md" '
            'target="_blank">' in page)
    # 飛び先が実在すること ―― リンクの形だけ合っていても意味が無い
    target = (out / _phase(spec_with_copies)
              / "../../rounds/r001/parsed/資料/A.xlsx/受注.md")
    assert target.resolve().is_file()


def test_写しが無い出典はリンクにしない(spec_with_copies: Spec) -> None:
    """**辿れない飛び先を作らない。** 「資料に無い」と「機械が出していない」を混ぜない。"""
    out = spec_with_copies.paths.out
    publish_module.publish(spec_with_copies, out, names=["要件定義書"])
    page = (out / _phase(spec_with_copies) / "要件定義書.html").read_text(encoding="utf-8")

    assert "資料/消えた.xlsx/シート" in page          # 文字は消さない
    assert "消えた.xlsx/シート.md" not in page        # リンクにはしない


def test_アンカーは飛び先に付けない(spec_with_copies: Spec) -> None:
    """写しのアンカーは HTML コメントなので、断片識別子としては動かない。"""
    out = spec_with_copies.paths.out
    publish_module.publish(spec_with_copies, out, names=["要件定義書"])
    page = (out / _phase(spec_with_copies) / "要件定義書.html").read_text(encoding="utf-8")

    assert 'href="../../rounds/r001/parsed/資料/A.xlsx/受注.md#s1-t1"' not in page
    assert "受注#s1-t1</a>" in page                   # 升の文字にはアンカーが残る


# ── 概要 ────────────────────────────────────────────────────────
def test_概要はmdとhtmlで同じ事実を出す(spec_with_copies: Spec) -> None:
    """片方だけが言うことを作らない（升目の突き合わせと同じ理由）。

    **同じ升目にはしない** ―― md は 1 行の引用、html は ``<dl>`` である。
    揃えるのは事実であって描画ではない（→ :meth:`publish.Brief.facts`）。
    """
    blocks = publish_module._blocks(spec_with_copies, _DOC)
    brief = publish_module._brief(spec_with_copies, blocks)
    markdown = publish_module._markdown(spec_with_copies, _DOC, blocks, {},
                                        brief=brief)
    page = publish_module._html(spec_with_copies, _DOC, blocks, {}, brief=brief)

    assert brief.rows == 2 and brief.books == 2
    for _label, value, _note in brief.facts():
        assert value in markdown and value in page


def test_概要は本文を要約しない(spec_with_copies: Spec) -> None:
    """出典の無い文章を、設計書のいちばん目立つところに置かない。"""
    blocks = publish_module._blocks(spec_with_copies, _DOC)
    brief = publish_module._brief(spec_with_copies, blocks)
    page = publish_module._html(spec_with_copies, _DOC, blocks, {}, brief=brief)

    band = page[page.index('<div class="brief">'):page.index("</dl>")]
    assert "受注できること" not in band                # 本文は帯に出さない
    assert "本文の要約ではありません" in page


def test_畳んだ数は脚注の文面からではなく畳んだ場所から数える() -> None:
    """文面を直した日に静かに 0 件になる数え方をしない。"""
    block = publish_module.Block(2, "章", ["a"], [["x"]])
    block.folded_columns = 3
    brief = publish_module.Brief(folded_columns=block.folded_columns)

    assert "列 3" in "".join(v for _l, v, _n in brief.facts())


# ── 元資料と設計書の対応 ────────────────────────────────────────
def test_出ていない写しを3通りに言い分ける(spec_with_copies: Spec) -> None:
    """**同じ「出ていない」でも次の一手が正反対**である。

    言い分けていなかったころ、実測（r001）で出ていない 67 枚が 67 枚とも
    対象外の宣言だったのに、表は全部に「正本まで届いていません」と書いていた
    ―― 手を打つところが 1 つも無いものを 67 件の宿題として見せていた。
    """
    out = spec_with_copies.paths.out
    origins_module.write(spec_with_copies, out)
    body = (out / f"{origins_module.STEM}.md").read_text(encoding="utf-8")

    rows = [line for line in body.splitlines() if line.startswith("| r001 ")]
    assert len(rows) == 2                              # 受注は使われたので出ない
    assert [line for line in rows if "表紙" in line
            and origins_module._DECLARED in line]
    assert [line for line in rows if "B.py" in line
            and origins_module._UNORGANIZED in line]


def test_原本ごとの使われ方はブックで束ねる(spec_with_copies: Spec) -> None:
    """Excel は 1 冊にシートが何枚も入る ―― **束ねる単位は原本側の事実**である。"""
    assert origins_module.origin_of("資料/A.xlsx/受注") == ("資料/A.xlsx", "受注")
    assert origins_module.origin_of("src/arp4/build.py") == ("src/arp4", "build.py")

    out = spec_with_copies.paths.out
    origins_module.write(spec_with_copies, out)
    body = (out / f"{origins_module.STEM}.md").read_text(encoding="utf-8")

    assert "| 資料/A.xlsx | r001 | 2 | 1 |" in body     # 写し 2 枚 / 出たのは 1 枚


def test_どの設計書に出たかは渡されたときだけ列にする(spec_with_copies: Spec) -> None:
    """**無いものを空欄で出すと「どの設計書にも出ていない」と読める。**"""
    out = spec_with_copies.paths.out
    origins_module.write(spec_with_copies, out)
    without = (out / f"{origins_module.STEM}.md").read_text(encoding="utf-8")
    origins_module.write(spec_with_copies, out,
                         {("r001", "資料/A.xlsx/受注"): {"要件定義書"}})
    with_docs = (out / f"{origins_module.STEM}.md").read_text(encoding="utf-8")

    assert "出た設計書" not in without
    assert "出た設計書" in with_docs and "要件定義書" in with_docs


# ── 見取り図 ────────────────────────────────────────────────────
def test_見取り図は正本に無い線を引かない() -> None:
    """近さ・名前の似かたで線を足さない（`parse` の「座標から線を復元しない」と同じ）。"""
    nodes = [figure_module.Node("要件定義書", "要件定義書"),
             figure_module.Node("詳細設計書", "詳細設計書")]
    svg = figure_module.documents([("要件定義", nodes[:1]), ("詳細設計", nodes[1:])],
                                  {("詳細設計書", "要件定義書")})

    assert svg.count('class="edge"') == 1
    assert "線は 1 本" in svg


def test_箱が無ければ図を出さない() -> None:
    """線の無い図は「参照が無い」と「図が壊れている」を見分けられない。"""
    assert figure_module.documents([], set()) == ""


def test_自分への線は引かない() -> None:
    node = [figure_module.Node("要件定義書", "要件定義書")]
    svg = figure_module.documents([("要件定義", node)], {("要件定義書", "要件定義書")})

    assert svg.count('class="edge"') == 0


# ── 表の囲い ────────────────────────────────────────────────────
def test_横長の表だけ窓にする() -> None:
    """縦長の表を窓に入れると、**見出しが 1 度も張り付かなくなる**（→ `arp4.page`）。"""
    assert 'class="sheet"' in page_module.sheet(7)
    assert "sheet pan" in page_module.sheet(17)


def test_長い升だけ畳む() -> None:
    short, long = "あ" * 10, "あ" * 200

    assert page_module.cell(short, short) == short
    assert page_module.cell(long, long).startswith('<div class="clip">')


# ── 穴の 1 枚 ───────────────────────────────────────────────────
def test_穴の1枚にも同じ体裁が付く(spec_with_copies: Spec) -> None:
    """空の ``<link rel="stylesheet">`` を出していたので、**いちばん先に読ませたい
    1 枚だけ**が罫線も色も無い素の HTML だった。"""
    out = spec_with_copies.paths.out
    holes_module.write(spec_with_copies, out, [])
    page = (out / f"{holes_module.STEM}.html").read_text(encoding="utf-8")

    assert 'rel="stylesheet"' not in page
    assert "<style>" in page and "table.grid" in page
    assert '<a href="目次.html" target="_blank">' in page              # 束へ戻る導線


# ── 配色の切り替え ──────────────────────────────────────────────
def test_どのページのヘッダーにも配色の切り替えが出る(spec_with_copies: Spec) -> None:
    """表を持たない目次にも出す ―― **束の入口だけ地の色が変わらない**と浮く。"""
    out = spec_with_copies.paths.out
    publish_module.publish(spec_with_copies, out)

    for name in ("目次.html", f"{holes_module.STEM}.html",
                 f"{origins_module.STEM}.html",
                 f"{_phase(spec_with_copies)}/要件定義書.html"):
        page = (out / name).read_text(encoding="utf-8")
        assert 'id="arp-theme"' in page, name
        assert '<div class="bar">' in page, name

    # 目次は絞り込む表を持たないので、そちらは出さない
    assert 'id="arp-q"' not in (out / "目次.html").read_text(encoding="utf-8")


def test_既定は画面の設定に従う(spec_with_copies: Spec) -> None:
    """開いた人が何も選んでいないうちから、こちらの好みを押し付けない。"""
    out = spec_with_copies.paths.out
    publish_module.publish(spec_with_copies, out)
    page = (out / "目次.html").read_text(encoding="utf-8")

    assert "data-theme=" not in page.split("<style>")[0].split("<script>")[0]
    assert "@media (prefers-color-scheme: dark)" in page


def test_暗い側の配色は2か所に出るが中身は1つから作る() -> None:
    """「画面が暗い」と「暗いに固定した」の両方で要る ―― **手で 2 度書くと、
    片方だけ直した日から 2 つの暗い画面が別物になる。**"""
    fixed = page_module._dark(':root[data-theme="dark"]')
    system = page_module._dark(':root:not([data-theme="light"])')

    assert page_module._DARK_VARS in fixed and page_module._DARK_VARS in system
    assert fixed.count("--paper: #1b1b1b") == system.count("--paper: #1b1b1b")
    assert fixed in page_module.STYLE and system in page_module.STYLE


def test_選んだ配色は描く前に当てる(spec_with_copies: Spec) -> None:
    """``<body>`` の中で当てると、暗い画面で「明るい」を選んだ人に**一瞬だけ
    暗い画面が出る**（ページを開くたびに光る）。"""
    out = spec_with_copies.paths.out
    publish_module.publish(spec_with_copies, out)
    page = (out / "目次.html").read_text(encoding="utf-8")

    assert page.index(page_module.THEME_KEY) < page.index("<body>")
    assert "catch(e){}" in page                        # 覚えられなくても止まらない


# ── 飛び先の開き方 ──────────────────────────────────────────────
def test_ページの外へ出るリンクは新しいタブで開く(spec_with_copies: Spec) -> None:
    """出典を 1 つ確かめるたびに戻るボタンを押させると、**どの行を見ていたかを
    読み手が覚えていること**になる（Excel でも他のブックへの飛び先は別で開く）。"""
    out = spec_with_copies.paths.out
    publish_module.publish(spec_with_copies, out)
    page = (out / _phase(spec_with_copies) / "要件定義書.html").read_text(encoding="utf-8")
    index = (out / "目次.html").read_text(encoding="utf-8")

    assert ('<a href="../../rounds/r001/parsed/資料/A.xlsx/受注.md" '
            'target="_blank">' in page)
    assert '<a href="../目次.html" target="_blank">' in page
    assert (f'<a href="{_phase(spec_with_copies)}/要件定義書.html" '
            'target="_blank">') in index
    assert f'{origins_module.STEM}.html" target="_blank"' in index


def test_ページの中の飛び先は同じタブのまま(spec_with_copies: Spec) -> None:
    """目次から章へ、シート見出しから表へ ―― これは Excel でいうシートの移動で、
    別のタブで開いたら**同じ文書が 2 つ開く**。

    `<base target="_blank">` で一括指定できないのはこのためである（あれは
    同一ページの断片にも掛かる）。
    """
    out = spec_with_copies.paths.out
    publish_module.publish(spec_with_copies, out)
    page = (out / _phase(spec_with_copies) / "要件定義書.html").read_text(encoding="utf-8")

    assert "<base " not in page
    for fragment in re.findall(r'<a href="(#[^"]*)"([^>]*)>', page):
        assert "target" not in fragment[1], fragment


def test_持ち主が自分なら番号のリンクにtargetを付けない(
        packed: mm.Metamodel, tmp_path: Path) -> None:
    """同じ番号でも、飛び先が同じページなら新しいタブで開く理由が無い。"""
    here = Path("out/3_詳細設計/詳細設計書.html")
    same = publish_module._linkify("MOD-027 arp4.paths", {"MOD-027": here},
                                   here, False)
    away = publish_module._linkify("MOD-027 arp4.paths", {"MOD-027": Path(
        "out/5_管理/トレーサビリティ・マトリクス.html")}, here, False)

    assert same.startswith('<a href="#MOD-027">')
    assert 'target="_blank"' in away


def test_目次から穴と元資料の両方へ行ける(spec_with_copies: Spec) -> None:
    out = spec_with_copies.paths.out
    publish_module.publish(spec_with_copies, out)
    index = (out / "目次.html").read_text(encoding="utf-8")

    assert f'href="{holes_module.STEM}.html"' in index
    assert f'href="{origins_module.STEM}.html"' in index
    assert "<svg" in index                             # 束の見取り図


def test_出典に引かれたことと本文に出たことを分ける(spec_with_copies: Spec) -> None:
    """**引かれたことと、読めることは別である。**

    実測（r001）で `処理仕様書_請求締め.xlsx/7.締め期間の例` は「設計書に出た」
    側に数えられていたが、その表の中身（締め実行日・締め期間・支払期日の例）は
    **全生成物に 1 文字も無かった** ―― 1 つの列でしか言っていなかったので、
    資料を渡した側には「届いた」としか読めなかった。
    """
    out = spec_with_copies.paths.out
    # 出典には引かれているが、どの設計書の行にもなっていない写し。
    origins_module.write(spec_with_copies, out, {})
    body = (out / f"{origins_module.STEM}.md").read_text(encoding="utf-8")

    assert "出典に引かれた写し" in body and "本文に出た写し" in body
    assert "| 資料/A.xlsx | r001 | 2 | 1 | 0 |" in body

    origins_module.write(spec_with_copies, out,
                         {("r001", "資料/A.xlsx/受注"): {"要件定義書"}})
    shown = (out / f"{origins_module.STEM}.md").read_text(encoding="utf-8")

    assert "| 資料/A.xlsx | r001 | 2 | 1 | 1 |" in shown
