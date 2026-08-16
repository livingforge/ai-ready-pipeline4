"""④ 設計書 ―― **空欄が支配的な表を出さない**ことと、**空の設計書が理由を言う**こと。

r001 の CRUD 図は 22 × 16 の升のうち埋まったのが 7.2%、権限マトリクスは 30.3%
だった。空欄の大半は資料に画面 × テーブルのアクセス表が無いことの正しい反映
なのだが、升からは「資料に無い」と「関係を張り忘れた」が見分けられない。

畳むこと自体より、**畳んだものを名前つきで見せること**のほうが大事である
（黙って落とすなら空欄のほうがまだ正直だった）。
"""

from __future__ import annotations

from pathlib import Path

import pytest

from arp4 import metamodel as mm
from arp4 import pack
from arp4 import publish as publish_module
from arp4.spec import Spec


def _spec(model: mm.Metamodel, items: list[dict], relations: list[dict]) -> Spec:
    return Spec(metamodel=model, items=items, relations=relations)


def _phase(model: mm.Metamodel, name: str) -> str:
    """``out/<番号>_<工程>/`` の番号は ``layers`` の並びで決まる。

    **パックに工程を足すと繰り下がる**（3.11.0 で `企画` が入り `3_詳細設計` →
    `4_詳細設計`）。ここで見たいのは飛び先が 1 冊に定まることなので、番号を
    期待値に焼き付けない。
    """
    return f"{list(model.layers).index(name) + 1}_{name}"


@pytest.fixture
def crud(model: mm.Metamodel) -> Spec:
    """画面 2 枚 × テーブル 2 本。**関係があるのは 1 組だけ。**"""
    return _spec(model, [
        {"id": "scr-1", "type": "screen", "name": "受注入力", "status": "review"},
        {"id": "scr-2", "type": "screen", "name": "ログイン", "status": "review"},
        {"id": "ent-1", "type": "entity", "name": "受注", "status": "review"},
        {"id": "ent-2", "type": "entity", "name": "単価", "status": "review"},
    ], [
        {"type": "accesses", "from": "scr-1", "to": "ent-1", "crud": ["C", "R"],
         "status": "review"},
    ])


_MATRIX = {"heading": "CRUD 図", "kind": "matrix", "relation": "accesses",
           "row_header": "機能", "rows": ["screen"], "cols": ["entity"],
           "cell": "crud"}
_DOC = {"name": "crud-matrix", "title": "CRUD図", "phase": "基本設計",
        "sections": [_MATRIX]}


def test_関係の無い行と列は畳む(crud: Spec) -> None:
    block = publish_module._blocks(crud, _DOC)[0]

    assert block.columns == ["機能", "受注"]           # 単価の列は消える
    assert block.rows == [["受注入力", "CR"]]          # ログインの行も消える


def test_畳んだ行と列は名前つきで脚注に出る(crud: Spec) -> None:
    """**黙って落とさない。** 件数だけだと正本を引き直すことになる。"""
    block = publish_module._blocks(crud, _DOC)[0]

    notes = "\n".join(block.notes)
    assert "ログイン" in notes and "単価" in notes
    assert "行" in notes and "列" in notes
    assert "--full" in notes                          # 戻し方まで書く


def test_fullなら畳まない(crud: Spec) -> None:
    block = publish_module._blocks(crud, _DOC, full=True)[0]

    assert set(block.columns[1:]) == {"受注", "単価"}
    assert {row[0] for row in block.rows} == {"ログイン", "受注入力"}
    # 畳んでいないので**省略の脚注**は出ない。凡例は畳む・畳まないと無関係に出る
    # ―― 空欄の読み違えは full でも起きる（→ `_legend`）。
    assert not [n for n in block.notes if "省略" in n]


def test_脚注はmarkdownにもhtmlにも出る(crud: Spec) -> None:
    blocks = publish_module._blocks(crud, _DOC)
    text = publish_module._markdown(crud, _DOC, blocks, {})
    page = publish_module._html(crud, _DOC, blocks, {})

    assert "ログイン" in text and "単価" in text
    assert "ログイン" in page and "単価" in page


def test_廃止した関係では升が埋まらない(crud: Spec) -> None:
    crud.relations[0]["status"] = "deprecated"
    block = publish_module._blocks(crud, _DOC)[0]

    assert block.rows == []                           # 全部畳まれる
    assert "受注入力" in "\n".join(block.notes)        # それでも名前は出る


# ── 対象データが 0 件の設計書 ───────────────────────────────────
_TEST_DOC = {"name": "test-spec", "title": "テスト仕様書", "phase": "テスト",
             "sections": [
                 {"heading": "テストケース", "type": "test-case",
                  "columns": ["test_id", "name"]},
                 {"heading": "検証対象", "kind": "relation", "relation": "verifies",
                  "columns": ["from.name", "to.name"]}]}


def test_対象が0件の設計書は不足している種別を書く(model: mm.Metamodel) -> None:
    """「作ったが空」と「作っていない」がレビューで区別できるようにする。"""
    empty = _spec(model, [], [])
    blocks = publish_module._blocks(empty, _TEST_DOC)
    assert publish_module.barren(blocks)

    text = publish_module._markdown(empty, _TEST_DOC, blocks, {})
    assert "test-case" in text and "verifies" in text
    assert "テスト" in text                            # 想定される原因（工程）
    assert "test-case" in publish_module._html(empty, _TEST_DOC, blocks, {})


def test_升が1つも残らないなら省略した行を並べない(crud: Spec) -> None:
    """**母集合そのものを「省略した」と言わない。**

    1 本も関係が無ければ升は 1 つも残らず、脚注は母集合の名前を全部並べる
    だけになる ―― 実測で空の CRUD 図は 2.6KB のうち大半をモジュール 111 件の
    名前に使っていた。比べる相手がいないので、名指しは何も伝えない。

    見るのは**関係が 1 本も無いこと**であって升が残らなかったことではない
    ―― 全部 `deprecated` でも升は 0 になるが、あちらは「廃止したので消えた」
    であって「語彙がまだ無い」ではないので名前を出す
    （→ `test_廃止した関係では升が埋まらない`）。
    """
    crud.relations.clear()
    block = publish_module._blocks(crud, _DOC)[0]

    assert not block.rows and not block.notes
    text = publish_module._markdown(crud, _DOC, [block], {})
    assert "省略した行" not in text and "ログイン" not in text


def test_足りないのは種別とは限らない(crud: Spec) -> None:
    """CRUD 図はエンティティが揃っていても ``accesses`` が 0 本なら空になる。

    そこで「アイテムがありません」と書くと、**正本を見た人が食い違いに悩む。**
    """
    crud.relations.clear()
    blocks = publish_module._blocks(crud, _DOC)
    text = publish_module._markdown(crud, _DOC, blocks, {})

    assert "accesses" in text and "アイテムが正本にありません" not in text


def test_中身のある設計書には断りを書かない(crud: Spec) -> None:
    blocks = publish_module._blocks(crud, _DOC)
    assert not publish_module.barren(blocks)
    assert "正本にありません" not in publish_module._markdown(crud, _DOC, blocks, {})


def test_目次に対象データなしが付く(crud: Spec, tmp_path: Path) -> None:
    """目次だけ見たレビュアーにも「中身が無い」ことが伝わる。"""
    md, page = publish_module._index(crud, tmp_path, [
        ("基本設計", "CRUD図", tmp_path / "2_基本設計" / "CRUD図.md", False),
        ("テスト", "テスト仕様書", tmp_path / "4_テスト" / "テスト仕様書.md", True),
    ])

    index = md.read_text(encoding="utf-8")
    assert "- [CRUD図](2_基本設計/CRUD図.md)\n" in index
    assert "- [テスト仕様書](4_テスト/テスト仕様書.md)（対象データなし）" in index

    # **HTML にも入口を出す。** 束を HTML で渡された人にも読み始めがある。
    body = page.read_text(encoding="utf-8")
    assert '<a href="2_基本設計/CRUD図.html" target="_blank">CRUD図</a>' in body
    assert '<a href="4_テスト/テスト仕様書.html" target="_blank">テスト仕様書</a>' in body
    assert "（対象データなし）" in body


# ── 束をまたいだ番号のリンク ────────────────────────────────────
#: モジュール 1 件とテスト 1 件。**同じ番号が複数の設計書に出る**最小の形。
_LINKED = [
    {"id": "mod-1", "type": "module", "module_id": "MOD-027",
     "name": "arp4.yamlio", "statement": "YAML を読み書きすること",
     "status": "review"},
    {"id": "tcs-1", "type": "test-case", "test_id": "TC-0001",
     "name": "読んだものは書き戻せる", "expected": "戻ること", "status": "review"},
]
_TESTED = [{"type": "verifies", "from": "tcs-1", "to": "mod-1",
            "status": "review"}]


@pytest.fixture(scope="module")
def packed() -> mm.Metamodel:
    """**様式まで含めたメタモデル。** 束のリンクは 1 冊では確かめられない。

    `model` は素の辞書から解いているのでパックの鎖を持たず、`catalog` が
    文書定義を 1 つも返さない ―― ここだけは `extends` で解く。
    """
    resolved, findings = mm.resolve({"extends": "jp-sier-std", "version": 3})
    assert not [f for f in findings if f.level == "error"]
    return resolved


def test_番号は束の中で1冊だけが持つ(packed: mm.Metamodel, tmp_path: Path) -> None:
    """**両方が `id` を名乗ると `#MOD-027` の飛び先が読み手から見て 2 つになる。**

    `MOD-027` は詳細設計書の一覧にもトレーサビリティ・マトリクスにも出る。
    持つのは工程が先の 1 冊（V 字の並び）で、残りはそこへ飛ぶ。
    """
    written = publish_module.publish(_spec(packed, _LINKED, _TESTED),
                                     tmp_path, meta={})
    pages = {p.name: p.read_text(encoding="utf-8")
             for p in written if p.suffix == ".html"}

    assert 'id="MOD-027"' in pages["詳細設計書.html"]
    assert 'id="MOD-027"' not in pages["トレーサビリティ・マトリクス.html"]
    assert (f'<a href="../{_phase(packed, "詳細設計")}/詳細設計書.html#MOD-027" '
            'target="_blank">MOD-027</a>'
            in pages["トレーサビリティ・マトリクス.html"])
    # テスト側も同じ ―― 596 個のアンカーが誰からも参照されていなかった
    assert 'id="TC-0001"' in pages["テスト仕様書.html"]
    assert (f'<a href="../{_phase(packed, "テスト")}/テスト仕様書.html#TC-0001" '
            'target="_blank">TC-0001</a>'
            in pages["トレーサビリティ・マトリクス.html"])


def test_目次から入って目次へ戻れる(packed: mm.Metamodel, tmp_path: Path) -> None:
    """HTML 一式を渡された人にも読み始めと戻り道がある。"""
    written = publish_module.publish(_spec(packed, _LINKED, _TESTED),
                                     tmp_path, meta={})
    pages = {p.name: p.read_text(encoding="utf-8") for p in written}

    assert (f'<a href="{_phase(packed, "詳細設計")}/詳細設計書.html" '
            'target="_blank">詳細設計書</a>') in pages["目次.html"]
    assert '<a href="../目次.html" target="_blank">' in pages["詳細設計書.html"]


def test_1冊だけ書き出すときは目次へ戻る導線を付けない(
        packed: mm.Metamodel, tmp_path: Path) -> None:
    """**存在しないページを指さない。** 目次を出していないなら戻り道も無い。"""
    written = publish_module.publish(_spec(packed, _LINKED, _TESTED), tmp_path,
                                     names=["詳細設計書"], meta={})

    assert not [p for p in written if p.name == "目次.html"]
    page = [p for p in written if p.suffix == ".html"][0]
    assert "目次.html" not in page.read_text(encoding="utf-8")


def test_見た目が似ているだけの語はリンクにしない(model: mm.Metamodel) -> None:
    """**本文を舐めると、リンクが本文の意味を語り出す。**

    仕様の文中の `G025` のような語まで拾うと、読み手には「番号が張ってある
    ＝正本にその番号のアイテムがある」と読める。探すのはこちらが組み立てた
    形の先頭（`、` で区切った各片の最初の語）だけにする。
    """
    owners = {"MOD-027": Path("out/3_詳細設計/詳細設計書.html")}
    here = Path("out/5_管理/トレーサビリティ・マトリクス.html")

    assert publish_module._linkify("規約 MOD-027 に従うこと", owners, here,
                                   False) == "規約 MOD-027 に従うこと"
    assert publish_module._linkify("MOD-027 arp4.yamlio", owners, here, False) == (
        '<a href="../3_詳細設計/詳細設計書.html#MOD-027" target="_blank">MOD-027</a>'
        ' arp4.yamlio')


# ── 一覧の主軸（区分の 2 段） ────────────────────────────────────
#: 元資料の要件一覧は「区分」を結合セルで束ね、下にサブシステム・分類の 2 列を置く。
_2段の区分 = [
    {"id": "req-1", "type": "requirement", "name": "受注登録", "req_id": "FR-001",
     "kind": "機能", "subsystem": "受注管理", "category": "受注業務",
     "statement": "受注を登録できること", "status": "review"},
    {"id": "req-2", "type": "requirement", "name": "出荷指示", "req_id": "FR-002",
     "kind": "機能", "subsystem": "受注管理", "category": "出荷業務",
     "statement": "倉庫へ出荷指示を出せること", "status": "review"},
    {"id": "req-3", "type": "requirement", "name": "入金消込", "req_id": "FR-003",
     "kind": "機能", "subsystem": "請求管理", "category": "入金業務",
     "statement": "入金を消し込めること", "status": "review"},
]


def test_区分は上段が節で下段が列(packed: mm.Metamodel, tmp_path: Path) -> None:
    """**主軸（`subsystem`）が設計書に出るか。** ―― 決定 75 の再発検知。

    受け皿が無かった段では、整理層はサブシステムを `refines` の相手や
    `description` の散文へ逃がすしかなく、**設計書に区分が 1 文字も出なかった**
    （実測 sales-corpus 30 冊: 37 件が `description`・機能要件 21 件が `refines`）。
    パース結果には縦結合を展開した全行に入っていたので、**上流を見ても気づけない。**

    見るのは 2 つ ―― 上段が節になっていること、下段が列として残っていること。
    節を下段（`category`）で割ると `5.1 入金業務` と `5.2 受注業務` が同じ高さに
    並び、**どのサブシステムの話かが目次から消える。**
    """
    written = publish_module.publish(_spec(packed, _2段の区分, []), tmp_path,
                                     names=["要件定義書"], meta={})
    page = [p for p in written if p.suffix == ".md"][0].read_text(encoding="utf-8")

    # 章番号は 1 ―― 他の章は対象データが無いので畳まれる（節は上段で 2 つに割れる）
    assert "### 1.1 受注管理" in page and "### 1.2 請求管理" in page
    assert "| 要件ID | 名称 | 大分類 | 仕様 |" in page   # 下段は列として残る
    assert "受注業務" in page and "出荷業務" in page
    # 分類を節にしていたときの形（サブシステムが消えた形）に戻っていないこと
    assert "### 1.1 受注業務" not in page


# ── 空の列 ──────────────────────────────────────────────────────
_REQUIREMENTS = [
    {"id": "req-1", "type": "requirement", "name": "凍結は 4 条件で止める",
     "req_id": "FR-001", "kind": "機能", "category": "凍結",
     "statement": "4 条件を満たすときだけ通すこと", "status": "review"},
    {"id": "req-2", "type": "requirement", "name": "既にある番号を動かさない",
     "req_id": "FR-002", "kind": "機能", "category": "採番",
     "statement": "空いているものだけに割り当てること", "status": "review"},
]
_SECTION = {"heading": "機能要件", "type": "requirement",
            "where": {"kind": "機能"}, "group_by": "category",
            "columns": ["req_id", "name", "statement", "description"]}
_REQ_DOC = {"name": "requirement-spec", "title": "要件定義書",
            "phase": "要件定義", "sections": [_SECTION]}


def test_全行が空の列は畳む(model: mm.Metamodel) -> None:
    """**読み手に何も伝えないのに、伝わるものを狭める。**

    列が増えるほど 1 列の幅は減り、紙にすると左右に割れる ―― 升目に当てている
    規律（畳んで、名前つきで見せて、`--full` で戻す）を列にも当てるだけである。
    """
    blocks = publish_module._blocks(_spec(model, _REQUIREMENTS, []), _REQ_DOC)
    tables = [b for b in blocks if b.rows]

    assert [b.columns for b in tables] == [["要件ID", "名称", "仕様"]] * 2
    notes = "\n".join(n for b in tables for n in b.notes)
    assert "省略した列: 補足" in notes and "--full" in notes


def test_空の列を畳む判定は節ごとにする(model: mm.Metamodel) -> None:
    """**塊ごとに決めると、同じ列がある表と無い表に割れる。**

    `group_by` で 17 の表に割れる節では比べられなくなり、しかも脚注が 17 回出る。
    片方の組にだけ値があるなら、**両方とも残す**のが正しい。
    """
    items = [*_REQUIREMENTS]
    items[1] = {**items[1], "description": "凍結の 4 条件と対で読む"}
    blocks = publish_module._blocks(_spec(model, items, []), _REQ_DOC)
    tables = [b for b in blocks if b.rows]

    assert all(b.columns == ["要件ID", "名称", "仕様", "補足"] for b in tables)
    assert not [n for b in tables for n in b.notes]
    # 脚注は節に 1 回だけ（表の数だけ繰り返さない）
    blocks = publish_module._blocks(_spec(model, _REQUIREMENTS, []), _REQ_DOC)
    assert len([n for b in blocks for n in b.notes]) == 1


def test_fullなら空の列も残す(model: mm.Metamodel) -> None:
    blocks = publish_module._blocks(_spec(model, _REQUIREMENTS, []),
                                    _REQ_DOC, full=True)
    tables = [b for b in blocks if b.rows]

    assert all(b.columns == ["要件ID", "名称", "仕様", "補足"] for b in tables)
    assert not [n for b in tables for n in b.notes]


_TRACE = {"heading": "要件 → テストケース", "kind": "trace",
          "type": "requirement", "relation": "verifies",
          "link_label": "対応するテストケース",
          "columns": ["req_id", "name", "linked"]}
_TRACE_DOC = {"name": "traceability-matrix",
              "title": "トレーサビリティ・マトリクス",
              "phase": "管理", "sections": [_TRACE]}
_CASE = {"id": "tcs-9", "type": "test-case", "status": "review",
         "test_id": "TC-0009", "name": "凍結が止まる",
         "expected": "止まること"}


def test_トレースの空欄は畳まない(model: mm.Metamodel) -> None:
    """**あちらは空欄そのものが結論である。**

    「対応するテストケース」が `―` の行はテスト漏れそのもの ―― 畳むと
    **いちばん読ませたい列が消える**。関係が使われてさえいれば残す。
    """
    spec = _spec(model, [*_REQUIREMENTS, _CASE],
                 [{"type": "verifies", "from": "tcs-9", "to": "req-1",
                   "status": "review"}])
    blocks = publish_module._blocks(spec, _TRACE_DOC)

    assert blocks[0].columns == ["要件ID", "名称", "対応するテストケース"]
    assert [row[-1] for row in blocks[0].rows] == ["TC-0009 凍結が止まる", "―"]
    assert not blocks[0].notes


def test_関係が1本も無いトレースは理由を書いて畳む(model: mm.Metamodel) -> None:
    """**「対象データが無い」では嘘になる。** 母集合はある ―― 無いのは関係である。

    実測（r001）で、前向きの対応表は全行 `―` の母集合 101 行を出しながら、
    対になるギャップ表は*対象データが無いので省略*されていた。同じ事実から
    逆の判断が 2 つ出ていたので、**どちらも同じ理由で畳む**ことにした。
    """
    # **文書ぜんぶが空なら畳まない**（あちらは `_no_data` が理由を書く）ので、
    # 実際の構成と同じく中身のある章と対で置く。
    doc = {**_TRACE_DOC, "sections": [_SECTION, _TRACE]}
    blocks = publish_module._blocks(_spec(model, _REQUIREMENTS, []), doc)

    assert "要件 → テストケース" not in [b.heading for b in blocks]
    note = "\n".join(n for b in blocks for n in b.notes)
    assert "`verifies`" in note and "1 本も無い" in note
    assert "要件 → テストケース" in note
    assert "対象データが無い" not in note


def test_fullなら畳まずに母集合を出す(model: mm.Metamodel) -> None:
    """母集合そのものを見たい場面はある ―― `--full` は畳みを全部止める。"""
    blocks = publish_module._blocks(
        _spec(model, _REQUIREMENTS, []), _TRACE_DOC, full=True)

    assert len(blocks[0].rows) == len(_REQUIREMENTS)
    assert all(row[-1] == "―" for row in blocks[0].rows)


# ── gap ―― 関係が 1 本も無いときの「未対応の一覧」 ──────────────
_GAP = {"heading": "未実施のテストケース", "kind": "trace", "type": "test-case",
        "relation": "executes", "gap": True,
        "columns": ["test_id", "level", "name", "expected"]}
_GAP_DOC = {"name": "test-result", "title": "テスト結果報告書", "phase": "テスト",
            "sections": [_GAP]}
_CASES = [
    {"id": "tcs-1", "type": "test-case", "status": "review", "name": "受注できる",
     "test_id": "TC-0001", "expected": "登録されること"},
    {"id": "tcs-2", "type": "test-case", "status": "review", "name": "与信で止まる",
     "test_id": "TC-0002", "expected": "止まること"},
]


def test_関係が1本も無いgapは母集合を並べない(model: mm.Metamodel) -> None:
    """**「実施していない」と「実施記録を取り込んでいない」を混ぜない。**

    漏れの一覧は「張られている中に、張られていないものが混じっている」から
    意味を持つ。1 本も無ければ残るのは母集合そのもので、それは調べた結果では
    なく語彙をまだ取り込んでいないということ ―― 自身の資産ではテスト結果
    報告書が 596 件を「未実施」として並べ、116KB かけてテスト仕様書の再掲に
    なっていた。次の一手が正反対（テストを流す／実施記録を取り込む）である。
    """
    spec = _spec(model, _CASES, [])
    blocks = publish_module._blocks(spec, _GAP_DOC)

    assert publish_module.barren(blocks)
    text = publish_module._markdown(spec, _GAP_DOC, blocks, {})
    assert "TC-0001" not in text
    # 空の理由は「テストの資料が無い」ではなく「実施記録が無い」
    assert "`test-run` が 0 件" in text
    assert "`test-case` は正本にあります" in text
    assert "丸ごと欠けているわけではありません" in text
    assert "「テスト」工程の資料が含まれていない" not in text


def test_1本でも張られていればgapは漏れの一覧になる(model: mm.Metamodel) -> None:
    """張られたものがある中の未対応は、調べた結果である ―― こちらは並べる。"""
    spec = _spec(model, [*_CASES,
                         {"id": "run-1", "type": "test-run", "status": "review",
                          "name": "1 回目", "result": "OK"}],
                 [{"type": "executes", "from": "run-1", "to": "tcs-1",
                   "status": "review"}])
    blocks = publish_module._blocks(spec, _GAP_DOC)

    assert [row[0] for row in blocks[0].rows] == ["TC-0002"]


def test_fullなら母集合も出す(model: mm.Metamodel) -> None:
    """母集合そのものなので、見たい場面はある。**戻す道は必ず残す。**"""
    spec = _spec(model, _CASES, [])
    blocks = publish_module._blocks(spec, _GAP_DOC, full=True)

    assert [row[0] for row in blocks[0].rows] == ["TC-0001", "TC-0002"]


def _mod(ident: str, name: str, file: str) -> dict:
    return {"id": ident, "type": "module", "status": "review", "name": name,
            "module_id": f"MOD-{ident[-1]}0",
            "source": [{"round": "r001", "file": file, "anchor": "m1"}]}


_MISS = {"heading": "テストの無いモジュール（テスト漏れ）", "kind": "trace",
         "type": "module", "relation": "verifies", "gap": True,
         "exclude_sources_of": "test-case",
         "columns": ["module_id", "name", "status"]}
_MISS_DOC = {"name": "traceability-matrix", "title": "トレーサビリティ・マトリクス",
             "phase": "管理", "sections": [_MISS]}
def _case(ident: str, test_id: str, file: str) -> dict:
    return {"id": ident, "type": "test-case", "status": "review",
            "test_id": test_id, "name": f"{test_id} の観点",
            "source": [{"round": "r001", "file": file, "anchor": "t1"}]}


#: `arp4.spec` は検証済み。**1 本でも張られている**ので漏れの一覧が成り立つ。
_MIXED = [
    _mod("mod-1", "arp4.paths", "src/arp4/paths.py"),
    _mod("mod-2", "tests.test_paths", "tests/test_paths.py"),
    _mod("mod-3", "tests.conftest", "tests/conftest.py"),
    _mod("mod-4", "arp4.spec", "src/arp4/spec.py"),
    _case("tcs-1", "TC-0001", "tests/test_paths.py"),
]
_VERIFIES = [{"type": "verifies", "from": "tcs-1", "to": "mod-4",
              "status": "review"}]


def test_テストファイル自身をテスト漏れに数えない(model: mm.Metamodel) -> None:
    """**テストファイルにテストが無いのは漏れではない。** そのファイルがテスト。

    実測で「テストの無いモジュール」85 件のうち 31 件が `tests.test_paths` の
    ようなテスト側で、漏れの一覧として使えなかった。
    """
    spec = _spec(model, _MIXED, _VERIFIES)
    rows = publish_module._blocks(spec, _MISS_DOC)[0].rows

    assert [row[1] for row in rows] == ["arp4.paths", "tests.conftest"]


def test_外した行は名前つきで脚注に出る(model: mm.Metamodel) -> None:
    """黙って落とさない ―― 除外の規律は畳んだ行・列と同じにする。"""
    notes = "\n".join(publish_module._blocks(
        _spec(model, _MIXED, _VERIFIES), _MISS_DOC)[0].notes)

    assert "外した行: 1 件" in notes and "tests.test_paths" in notes
    assert "--full" in notes


def test_fullなら除外もしない(model: mm.Metamodel) -> None:
    blocks = publish_module._blocks(_spec(model, _MIXED, _VERIFIES),
                                    _MISS_DOC, full=True)

    assert "tests.test_paths" in [row[1] for row in blocks[0].rows]
    assert not [n for n in blocks[0].notes if "外した行" in n]


def test_除外は名前ではなく出典で決める(model: mm.Metamodel) -> None:
    """名前を見ると意味の判断になり、命名規約の違う資産で外れる。

    `SpecOfPaths` という名前でも、そのファイルからテストが起きていれば
    テストである ―― 逆に `test_utils.py` でもテストが起きていなければ残す。
    """
    items = [_mod("mod-1", "SpecOfPaths", "src/SpecOfPaths.java"),
             _mod("mod-2", "tests.test_utils", "tests/test_utils.py"),
             _mod("mod-4", "arp4.spec", "src/arp4/spec.py"),
             _case("tcs-1", "TC-0001", "src/SpecOfPaths.java")]
    rows = publish_module._blocks(_spec(model, items, _VERIFIES),
                                  _MISS_DOC)[0].rows

    assert [row[1] for row in rows] == ["tests.test_utils"]


def test_gapの空の列は畳む(model: mm.Metamodel) -> None:
    """**gap は行の選び方が結論で、列はただの属性である。**

    トレースを `_trim` から外しているのは「対応するテストケース」が全行 `―`
    ならそれが結論だから ―― `gap` の「レベル」が全行 `―` なのは何の結論でも
    なく、免除の巻き添えで 596 行ぶん残っていた。
    """
    spec = _spec(model, [*_CASES,
                         {"id": "run-1", "type": "test-run", "status": "review",
                          "name": "1 回目", "result": "OK"}],
                 [{"type": "executes", "from": "run-1", "to": "tcs-1",
                   "status": "review"}])
    blocks = publish_module._blocks(spec, _GAP_DOC)

    assert "レベル" not in blocks[0].columns
    assert "省略した列: レベル" in "\n".join(blocks[0].notes)


# ── 読み口（並び・飛び先・空章） ────────────────────────────────
def test_表は表示ID順に並べる(model: mm.Metamodel) -> None:
    """採番は id（ハッシュ）順に走り、節は別の属性で割る。

    名前順に並べると節の中の番号が `FR-006, FR-024, FR-004` と散る ――
    **読み手は番号順に読む**ので、番号を動かさずに並びだけを合わせる。
    """
    items = [{**r, "category": "共通"} for r in _REQUIREMENTS]
    items[0] = {**items[0], "req_id": "FR-010", "name": "あ"}
    items[1] = {**items[1], "req_id": "FR-009", "name": "い"}
    blocks = publish_module._blocks(_spec(model, items, []), _REQ_DOC)

    # 分類が 1 つしか無いので節には割れない（→ 束が 1 つなら節に割らない）。
    assert [row[0] for row in blocks[0].rows] == ["FR-009", "FR-010"]


def test_桁を揃えていない表示IDも数として比べる(model: mm.Metamodel) -> None:
    """資料由来の ID は `E9` と `E10` が混ざる。文字列順だと `E10` が先に来る。"""
    from arp4 import sequence

    assert sorted(["E10", "E9", "E100"], key=sequence.sort_key) == [
        "E9", "E10", "E100"]


def test_表示IDを持たない種別は名前順のまま(model: mm.Metamodel) -> None:
    """用語・エンティティは番号を持たない。**無い番号で並べようとしない。**

    id 順（`ent-1` → `ent-2`）とは逆になる並びを選んでいる ―― 同じ結果が
    「並べていない」でも出るなら、テストが何も言っていないことになる。
    """
    items = [{"id": "ent-1", "type": "entity", "name": "受注", "status": "review"},
             {"id": "ent-2", "type": "entity", "name": "単価", "status": "review"}]
    doc = {"name": "d", "title": "t", "phase": "基本設計",
           "sections": [{"heading": "エンティティ一覧", "type": "entity",
                         "columns": ["name"]}]}
    blocks = publish_module._blocks(_spec(model, items, []), doc)

    assert [row[0] for row in blocks[0].rows] == ["単価", "受注"]   # 単 U+5358 < 受 U+53D7


def _nf(req_id: str, name: str, nf_category: str | None) -> dict:
    item = {"id": f"req-{req_id}", "type": "requirement", "kind": "非機能",
            "req_id": req_id, "name": name, "statement": f"{name}であること",
            "status": "review"}
    return {**item, "nf_category": nf_category} if nf_category else item


_NF_DOC = {"name": "requirement-spec", "title": "要件定義書", "phase": "要件定義",
           "sections": [{"heading": "非機能要件", "type": "requirement",
                         "where": {"kind": "非機能"}, "group_by": "nf_category",
                         "columns": ["req_id", "name", "statement"]}]}


def test_節は宣言順に並べる(model: mm.Metamodel) -> None:
    """`sorted()` だけだと**文字コード順**になり、宣言した順序を捨てる。

    `nf_category` は IPA 非機能要求グレードの 6 大項目を**順序のある enum として
    宣言してある**のに、出ていたのは `システム環境・エコロジー → 性能・拡張性 →
    運用・保守性` だった（カタカナが先、あとは漢字のコードポイント順）。読み手には
    工程順にも重要度順にも見えず、**目次から現在地を掴めない。**
    """
    items = [_nf("NFR-001", "撮れない環境では劣化させない", "システム環境・エコロジー"),
             _nf("NFR-002", "しきい値はピクセルで持つ", "性能・拡張性"),
             _nf("NFR-003", "向きの規則を 1 箇所に持つ", "運用・保守性")]
    blocks = publish_module._blocks(_spec(model, items, []), _NF_DOC)

    assert [b.heading for b in blocks if b.level == 3] == [
        "性能・拡張性", "運用・保守性", "システム環境・エコロジー"]


def test_宣言に無い分類は末尾へ回す(model: mm.Metamodel) -> None:
    """`extensible` な enum を殺さない ―― 資料にある分類を落とさず後ろへ並べる。

    固定すると整理層が「近いほう」へ寄せる判断を強いられ、**資料に無い分類が
    正本に残る**（→ `metamodel.yml` の nf_category）。
    """
    items = [_nf("NFR-001", "あ", "ユーザビリティ"),        # 宣言に無い
             _nf("NFR-002", "い", "移行性"),
             _nf("NFR-003", "う", "可用性"),
             _nf("NFR-004", "え", "アクセシビリティ")]      # 宣言に無い
    blocks = publish_module._blocks(_spec(model, items, []), _NF_DOC)

    assert [b.heading for b in blocks if b.level == 3] == [
        "可用性", "移行性", "アクセシビリティ", "ユーザビリティ"]


def test_束が1つなら節に割らない(model: mm.Metamodel) -> None:
    """分類が宣言されていない正本では、全件が「未分類」の 1 節に集まる。

    節は**他と比べる単位**なので、比べる相手が 1 つも無い `4.1 未分類` は
    見出しの水増しにしかならない（目次だけが 1 行伸びる）。
    """
    blocks = publish_module._blocks(
        _spec(model, [_nf("NFR-001", "あ", None), _nf("NFR-002", "い", None)], []),
        _NF_DOC)

    assert [b.level for b in blocks] == [2]
    assert len(blocks[0].rows) == 2


def test_関係の表も表示ID順に並べる(model: mm.Metamodel) -> None:
    """内部 ID（ハッシュ）順は、**読み手には順不同にしか見えない。**

    id 順（`fs-a` → `fs-b`）とは逆になる並びを選んでいる ―― 同じ結果が
    「並べていない」でも出るなら、テストが何も言っていないことになる。
    """
    steps = [{"id": "fs-b", "type": "flow-step", "step_id": "FS-001",
              "name": "組み立てる", "status": "review"},
             {"id": "fs-a", "type": "flow-step", "step_id": "FS-002",
              "name": "検証する", "status": "review"},
             {"id": "fs-c", "type": "flow-step", "step_id": "FS-003",
              "name": "生成する", "status": "review"}]
    relations = [{"type": "leads-to", "from": "fs-a", "to": "fs-c", "status": "review"},
                 {"type": "leads-to", "from": "fs-b", "to": "fs-a", "status": "review"}]
    doc = {"name": "d", "title": "t", "phase": "要件定義",
           "sections": [{"heading": "業務フローの流れ", "kind": "relation",
                         "relation": "leads-to",
                         "columns": ["from.step_id", "to.step_id"]}]}
    blocks = publish_module._blocks(_spec(model, steps, relations), doc)

    assert blocks[0].rows == [["FS-001", "FS-002"], ["FS-002", "FS-003"]]


def test_関係の相手は指すだけにできる(model: mm.Metamodel) -> None:
    """**N 対 1 の関係で相手の属性を写すと、辺の数だけ複製される。**

    自身の資産では `verifies` の相手の仕様文が `arp4.paths` で 392 行・
    `arp4.parse` で 311 行に並び、テスト仕様書 657KB のうち 525KB（8 割）が
    この章だった。`to` を属性なしで書くと**表示 ID 付きで指すだけ**になり、
    相手の仕様は相手の設計書で読む。
    """
    items = [{"id": "mod-1", "type": "module", "module_id": "MOD-027",
              "name": "arp4.yamlio", "statement": "YAML の読み書きを担うこと",
              "status": "review"},
             {"id": "tcs-1", "type": "test-case", "test_id": "TC-0001",
              "name": "壊れたYAMLは行を持って上がる", "status": "review"},
             {"id": "tcs-2", "type": "test-case", "test_id": "TC-0002",
              "name": "読んだものは書き戻せる", "status": "review"}]
    relations = [{"type": "verifies", "from": "tcs-1", "to": "mod-1",
                  "status": "review"},
                 {"type": "verifies", "from": "tcs-2", "to": "mod-1",
                  "status": "review"}]
    doc = {"name": "d", "title": "t", "phase": "テスト",
           "sections": [{"heading": "検証対象の対応", "kind": "relation",
                         "relation": "verifies", "labels": {"to": "検証対象"},
                         "columns": ["from.test_id", "to"]}]}
    blocks = publish_module._blocks(_spec(model, items, relations), doc)

    assert blocks[0].columns == ["テストID", "検証対象"]
    assert blocks[0].rows == [["TC-0001", "MOD-027 arp4.yamlio"],
                              ["TC-0002", "MOD-027 arp4.yamlio"]]
    # 仕様文は 1 度も複製しない（相手の設計書で読む）
    assert "YAML の読み書き" not in publish_module._markdown(
        _spec(model, items, relations), doc, blocks, {})


def test_指す先が正本に無ければ空欄にする(model: mm.Metamodel) -> None:
    """指せないことを `―` で言う ―― 内部 ID を素で出しても読み手には引けない。"""
    items = [{"id": "mod-1", "type": "module", "module_id": "MOD-027",
              "name": "arp4.yamlio", "status": "review"},
             {"id": "tcs-1", "type": "test-case", "test_id": "TC-0001",
              "name": "あ", "status": "review"},
             {"id": "tcs-2", "type": "test-case", "test_id": "TC-0002",
              "name": "い", "status": "review"}]
    relations = [{"type": "verifies", "from": "tcs-1", "to": "mod-消えた",
                  "status": "review"},
                 {"type": "verifies", "from": "tcs-2", "to": "mod-1",
                  "status": "review"}]
    doc = {"name": "d", "title": "t", "phase": "テスト",
           "sections": [{"heading": "検証対象の対応", "kind": "relation",
                         "relation": "verifies",
                         "columns": ["from.test_id", "to"]}]}
    blocks = publish_module._blocks(_spec(model, items, relations), doc)

    assert blocks[0].rows == [["TC-0001", "―"],
                              ["TC-0002", "MOD-027 arp4.yamlio"]]


def _sourced(count: int) -> dict:
    return {**_nf("NFR-001", "あ", None),
            "source": [{"round": "r001", "file": "docs/parsed.md",
                        "anchor": f"h{i}"} for i in range(1, count + 1)]}


_SRC_DOC = {"name": "d", "title": "t", "phase": "要件定義",
            "sections": [{"heading": "非機能要件", "type": "requirement",
                          "columns": ["req_id", "source"]}]}


def test_出典は先頭だけ出して残りは件数で言う(model: mm.Metamodel) -> None:
    """出典が仕様の本文より長くなる（実測 `FR-004` の出典 9 件）。

    Markdown の表は幅を**いちばん長いセル**が決めるので、出典を全部出すと
    読ませたい「仕様」の列が潰れる。辿るのに要るのは 1 つで足りる ―― どれも
    同じ concept を指しているからである。
    """
    cell = publish_module._blocks(
        _spec(model, [_sourced(9)], []), _SRC_DOC)[0].rows[0][1]

    assert "#h1" in cell and "#h2" in cell and "#h3" not in cell
    assert "ほか 7 件" in cell                 # 黙って落とさない（数で言う）


def test_fullなら出典を全部出す(model: mm.Metamodel) -> None:
    cell = publish_module._blocks(
        _spec(model, [_sourced(9)], []), _SRC_DOC, full=True)[0].rows[0][1]

    assert "#h9" in cell and "ほか" not in cell


def test_上限までの出典は畳まない(model: mm.Metamodel) -> None:
    cell = publish_module._blocks(
        _spec(model, [_sourced(2)], []), _SRC_DOC)[0].rows[0][1]

    assert "#h1" in cell and "#h2" in cell and "ほか" not in cell


def test_両端が同じ種別なら列見出しを名指しできる(model: mm.Metamodel) -> None:
    """1 つの表に畳むと `ステップID` が 2 列並び、**どちらが分岐元か読めない。**

    節に割って回避していた頃は起きなかった問題なので、畳む側で名前を与える。
    """
    steps = [{"id": "fs-1", "type": "flow-step", "step_id": "FS-001",
              "name": "組み立てる", "status": "review"},
             {"id": "fs-2", "type": "flow-step", "step_id": "FS-002",
              "name": "検証する", "status": "review"}]
    doc = {"name": "d", "title": "t", "phase": "要件定義",
           "sections": [{"heading": "業務フローの流れ", "kind": "relation",
                         "relation": "leads-to",
                         "labels": {"from.step_id": "分岐元",
                                    "to.step_id": "分岐先"},
                         "columns": ["from.step_id", "to.step_id"]}]}
    blocks = publish_module._blocks(
        _spec(model, steps,
              [{"type": "leads-to", "from": "fs-1", "to": "fs-2",
                "status": "review"}]), doc)

    assert blocks[0].columns == ["分岐元", "分岐先"]


def test_参照は表示IDで書き種別ラベルを重ねない(model: mm.Metamodel) -> None:
    """整理層が付ける名前は `同一性の台帳（concepts）` のように括弧で終わる。

    そこへ `（モジュール・クラス）` を足すと**括弧が 2 つ並んで読めない**
    （実測 42 セル）。接頭辞 `MOD` が種別を語るのでラベルは要らない。
    """
    module = {"id": "mod-1", "type": "module", "module_id": "MOD-003",
              "name": "同一性の台帳（concepts）", "status": "review"}

    assert publish_module._reference(model and _spec(model, [module], []),
                                     module) == "MOD-003 同一性の台帳（concepts）"


def test_表示IDを持たない参照はラベルで補う(model: mm.Metamodel) -> None:
    entity = {"id": "ent-1", "type": "entity", "name": "受注", "status": "review"}

    assert publish_module._reference(_spec(model, [entity], []),
                                     entity) == "受注（エンティティ）"


def test_節見出しも識別子を重ねない(model: mm.Metamodel) -> None:
    """`_reference` と同じ規律を**節見出しにも**効かせる。

    r001 の module は 57 件すべてが `表示 ID の採番（sequence）` の書式で、
    詳細設計書の節見出しが `（sequence）（sequence）` になっていた
    （目次と本文で実測 42 行）。

    見るのは**末尾の一致だけ**である。`画面（一覧）` のように別の理由で
    括弧が付いた名前から識別子まで落とすと、同名のモジュールを見分けられない
    ―― 括弧が並ぶことより**どれの節か分からない**ほうが重い。

    **名前が識別子そのもの**ということもある（`Anchor` / `class_name: Anchor`）。
    論理名と物理名が一致する資産 ―― コード・DDL ―― ではこれが普通で、実測で
    テーブル定義書の全 33 節が `Anchor（Anchor）` になった。足しても見分けは
    1 つも増えないので足さない。
    """
    modules = [{"id": "mod-1", "type": "module", "module_id": "MOD-001",
                "name": "表示 ID の採番（sequence）", "class_name": "sequence",
                "status": "review"},
               {"id": "mod-2", "type": "module", "module_id": "MOD-002",
                "name": "画面（一覧）", "class_name": "screens",
                "status": "review"},
               {"id": "mod-3", "type": "module", "module_id": "MOD-003",
                "name": "Anchor", "class_name": "Anchor",
                "status": "review"}]
    methods = [{"id": "mtd-1", "type": "method", "method_id": "MTD-0001",
                "name": "割り当てる", "status": "review"},
               {"id": "mtd-2", "type": "method", "method_id": "MTD-0002",
                "name": "並べる", "status": "review"},
               {"id": "mtd-3", "type": "method", "method_id": "MTD-0003",
                "name": "読む", "status": "review"}]
    doc = {"name": "d", "title": "t", "phase": "詳細設計",
           "sections": [{"heading": "メソッド定義", "kind": "relation",
                         "relation": "has-method", "group_by": "from",
                         "columns": ["to.method_id", "to.name"]}]}
    blocks = publish_module._blocks(
        _spec(model, modules + methods,
              [{"type": "has-method", "from": "mod-1", "to": "mtd-1",
                "status": "review"},
               {"type": "has-method", "from": "mod-2", "to": "mtd-2",
                "status": "review"},
               {"type": "has-method", "from": "mod-3", "to": "mtd-3",
                "status": "review"}]), doc)

    assert [b.heading for b in blocks if b.level == 3] == [
        "表示 ID の採番（sequence）", "画面（一覧）（screens）", "Anchor"]


def test_1行も出なかった章は畳んで名前を出す(model: mm.Metamodel) -> None:
    """様式は固定なので「（該当なし）」の章が毎回並ぶ（実測 144 中 20）。

    目次は「この設計書に何があるか」を見るところで、**無いものが過半を占めると
    あるものを探せない**。ただし黙って落とさない ―― 章名を必ず並べる。
    """
    doc = {"name": "d", "title": "t", "phase": "基本設計", "sections": [
        {"heading": "画面一覧", "type": "screen", "columns": ["name"]},
        _SECTION]}
    blocks = publish_module._blocks(_spec(model, _REQUIREMENTS, []), doc)

    assert "画面一覧" not in [b.heading for b in blocks]
    notes = "\n".join(n for b in blocks for n in b.notes)
    assert "省略した章: 「画面一覧」" in notes and "--full" in notes


def test_省略した章の脚注に番号を出さない(model: mm.Metamodel) -> None:
    """**畳む前の番号は、畳んだ瞬間に嘘になる。**

    このテストは以前「省略した章: 1 画面一覧」を期待していた ―― 生き残った章は
    振り直されるので、**同じ番号が本文と脚注の両方に出る**。実測で詳細設計書に
    `3 呼出関係` と `省略した章: 3 バッチ構成` が同時に載っていた。

    いまは :class:`Block` が番号を持たないので、脚注に積めるのは見出しだけで、
    この壊れ方は**書こうとしても書けない**（→ :func:`publish._numbering`）。
    """
    doc = {"name": "d", "title": "t", "phase": "基本設計", "sections": [
        {"heading": "画面一覧", "type": "screen", "columns": ["name"]},
        _SECTION]}
    blocks = publish_module._blocks(_spec(model, _REQUIREMENTS, []), doc)
    notes = "\n".join(n for b in blocks for n in b.notes)

    assert not hasattr(blocks[0], "number")
    dropped = notes.split("省略した章: ")[1]
    assert not any(char.isdigit() for char in dropped.split("。")[0])


def test_畳んだあとの章番号は詰め直す(model: mm.Metamodel) -> None:
    """**章番号は文書内の位置であって識別子ではない。**

    識別子は表示 ID で、HTML ではそこへ直接飛べる ―― 番号を飛ばしたまま
    「2 機能要件」から始まる設計書のほうが「1 はどこへ行った」と探させる。
    """
    doc = {"name": "d", "title": "t", "phase": "基本設計", "sections": [
        {"heading": "画面一覧", "type": "screen", "columns": ["name"]},
        _SECTION]}
    blocks = publish_module._blocks(_spec(model, _REQUIREMENTS, []), doc)

    assert publish_module._numbering(blocks) == ["1", "1.1", "1.2"]


def test_省略した章は鉤括弧で括る(model: mm.Metamodel) -> None:
    """区切りの `・` は、**名前の中の `・` と見分けが付かない**。

    実測で要件定義書が「省略した章: … 7 利用者・ロール・9 用語」と出ていた ――
    畳んだのは 8 つなのに、`・` で繋ぐと何個畳んだのかが読めない。
    """
    assert publish_module._listed(["業務フロー", "利用者・ロール", "用語"]) == (
        "「業務フロー」「利用者・ロール」「用語」")


def test_fullなら空の章も残す(model: mm.Metamodel) -> None:
    doc = {"name": "d", "title": "t", "phase": "基本設計", "sections": [
        {"heading": "画面一覧", "type": "screen", "columns": ["name"]},
        _SECTION]}
    blocks = publish_module._blocks(_spec(model, _REQUIREMENTS, []), doc, full=True)

    assert "画面一覧" in [b.heading for b in blocks]
    assert not [n for b in blocks for n in b.notes]


def test_全部が空の設計書は畳まない(model: mm.Metamodel) -> None:
    """畳むと真っ白になる。**あちらは「なぜ空か」を書くほうが仕事である。**"""
    doc = {"name": "d", "title": "t", "phase": "基本設計", "sections": [
        {"heading": "画面一覧", "type": "screen", "columns": ["name"]},
        {"heading": "帳票一覧", "type": "report", "columns": ["name"]}]}
    blocks = publish_module._blocks(_spec(model, [], []), doc)

    assert [b.heading for b in blocks] == ["画面一覧", "帳票一覧"]


def _page(model: mm.Metamodel, items: list[dict], doc: dict) -> str:
    spec = _spec(model, items, [])
    return publish_module._html(spec, doc, publish_module._blocks(spec, doc), {})


def test_表示IDへ飛べる(model: mm.Metamodel) -> None:
    """番号に業務的な意味を持たせない代わりに、**番号で引けるようにする。**

    レビューで `FR-001` と書けばその行が開く ―― 番号が意味を持たないまま実用になる。

    表示 ID の列は**固定する列**でもある（`class="k"` ―― Excel の「先頭列の固定」）。
    行番号の溝を付けなかったのはこのためで、固定するなら**再生成で動かない値**の
    ほうを固定する（→ `arp4.page`）。
    """
    page = _page(model, _REQUIREMENTS, _REQ_DOC)

    assert '<td class="k" id="FR-001">FR-001</td>' in page


def test_同じ番号に飛び先を2つ作らない(model: mm.Metamodel) -> None:
    """同じ番号は複数の章に出る（`FS-001` は手順一覧にも流れの表にも）。

    `id` が重なると飛び先が壊れるので、**最初に出たところ**＝一覧の章を正とする。
    """
    twice = {**_REQ_DOC, "sections": [_SECTION, {**_SECTION, "heading": "再掲"}]}
    page = _page(model, _REQUIREMENTS, twice)

    assert page.count('id="FR-001"') == 1
    assert page.count("FR-001") == 3          # 見出しではなく本文に 2 回出ている


def test_空欄には飛び先を作らない(model: mm.Metamodel) -> None:
    """未採番の `―` に `id="―"` を付けると、番号でない飛び先ができる。"""
    items = [{**_REQUIREMENTS[0], "req_id": ""}]
    page = _page(model, items, _REQ_DOC)

    assert 'id="―"' not in page


def test_標準パックの表に同じ列見出しが2つ並ばない(model: mm.Metamodel) -> None:
    """**両端が同じ種別の関係を 1 表に畳むと、見出しは必ずぶつかる。**

    `labels` で名指しする規律は決めてあったのに、詳細設計書の「呼出関係」と
    「メソッドが出すメッセージ」へ当て忘れていた ―― 出力は `| 名称 | … | 名称 |`
    になり、**どちらが呼ぶ側かを列見出しから読めない**（実測 77 行）。

    節ごとに書き忘れを見張るのではなく、**パック全体を 1 つの検査で覆う** ――
    文書を足すたびに同じ穴が開くので、当て忘れは仕組みで止めるほうが確実である。
    """
    chain, findings = pack.resolve_chain("jp-sier-std")
    assert not [f for f in findings if f.level == "error"]

    duplicated: list[str] = []
    for definition in pack.documents(chain):
        for section in definition.get("sections") or []:
            columns = section.get("columns") or []
            override = section.get("labels") or {}
            headers = [override.get(c) or override.get(publish_module._leaf(c))
                       or model.label(publish_module._leaf(c)) for c in columns]
            for header in sorted({h for h in headers if headers.count(h) > 1}):
                duplicated.append(
                    f"{definition['name']}「{section.get('heading')}」: {header}")

    assert duplicated == []


# ── md と html の突き合わせ ──────────────────────────────────────
def _text_cells(page: str) -> list[str]:
    """HTML の升目を**画面に見える文字**へ。標準ライブラリだけで剥がす。

    ここが要点である ―― `<td>a<text>b</td>` のように**タグとして読まれた部分は
    落ちる**ので、ブラウザで見える字がそのまま出る。正規表現で `<[^>]*>` を消すと
    「消えた」ことに気づけない（消したのは自分なので）。
    """
    import html.parser

    class Reader(html.parser.HTMLParser):
        def __init__(self) -> None:
            super().__init__()
            self.cells: list[str] = []
            self.buffer: list[str] | None = None

        def handle_starttag(self, tag: str, attrs: object) -> None:
            if tag in ("td", "th"):
                self.buffer = []
            elif tag == "br" and self.buffer is not None:
                self.buffer.append("\n")

        def handle_endtag(self, tag: str) -> None:
            if tag in ("td", "th") and self.buffer is not None:
                self.cells.append("".join(self.buffer))
                self.buffer = None

        def handle_data(self, data: str) -> None:
            if self.buffer is not None:
                self.buffer.append(data)

    reader = Reader()
    reader.feed(page)
    return reader.cells


#: **意地悪な値**。設計書に実際に出たもの（`arp4.__init__` / `'<text>'`）と、
#: 出てもおかしくないもの（Windows のパス・正規表現・HTML の実体参照）を混ぜる。
_HOSTILE = [
    "arp4.__init__", "__version__", "node_modules・__pycache__",
    "yamlio.marked(text: str, where: Any = '<text>')", "a:<id>",
    "dict[str, str] | None", r"C:\arp4\out", r"a\|b", "x|y",
    "1 & 2", "&lt;", "`code`", "*em*", "[link](x)", "~~del~~",
    "<!-- a:s9-t9 -->", "a<br>b", "第 1 行\n第 2 行", "―",
]


def test_markdownの升目は人が見ても元の字のままである() -> None:
    """**書き出したら、人が見るのと同じ規則で読み戻す。**

    `|` と改行しか逃がしていなかったあいだ、実測で **2071 升のうち 12 升**が
    HTML 版と違う字を出していた ―― `arp4.__init__` が `arp4.init`、
    `where: Any = '<text>'` が `where: Any = ''`。**実在しないモジュール名**を
    設計書が名乗るので、それを読んで書いた `python -m arp4.init` は動かない。

    読み戻す側を自前で書かないのが肝心である。逃がし漏れを見つける相手が自前の
    実装だと**同じ思い込みを 2 度書くだけ**で、何も検査したことにならない。
    """
    md = pytest.importorskip(
        "markdown_it", reason="pip install -e \".[check]\"").MarkdownIt(
        "commonmark").enable("table")

    body = "\n".join("| " + publish_module._md_cell(v) + " |" for v in _HOSTILE)
    source = "| 値 |\n|---|\n" + body + "\n"
    rendered = _text_cells(md.render(source))[1:]

    assert rendered == _HOSTILE


def test_同じ正本から出したmdとhtmlは同じ字を出す(model: mm.Metamodel,
                                                 tmp_path: Path) -> None:
    """**2 つの形が違うことを言うなら、どちらかが嘘である。**

    正本 → 設計書は 1 つの写像なので、Markdown と HTML は**同じ升目に同じ字**を
    出さなければならない。片方だけを見ていると、逃がし漏れは「片方が正しい」まま
    静かに残る（実際 HTML は最初から正しく、Markdown だけが壊れていた）。

    升の端の空白は比べない ―― Markdown も HTML も描画のときに落とすので、
    **どちらの形でも読み手には届かない**（これは逃がしで直せるものではない）。
    """
    md = pytest.importorskip(
        "markdown_it", reason="pip install -e \".[check]\"").MarkdownIt(
        "commonmark").enable("table")

    items = [{"id": f"cst-{i}", "type": "constraint", "constraint_id": f"CST-{i:03}",
              "name": value, "statement": value, "category": "技術",
              "status": "review"}
             for i, value in enumerate(_HOSTILE, start=1)]
    doc = {"name": "d", "title": "t", "phase": "要件定義", "sections": [
        {"heading": "制約", "type": "constraint",
         "columns": ["constraint_id", "name", "statement"]}]}

    spec = _spec(model, items, [])
    blocks = publish_module._blocks(spec, doc)
    markdown = publish_module._markdown(spec, doc, blocks, {})
    page = publish_module._html(spec, doc, blocks, {})

    from_md = [c.strip() for c in _text_cells(md.render(markdown))]
    from_html = [c.strip() for c in _text_cells(page)]

    assert from_md == from_html
    assert all(value in from_md for value in _HOSTILE)


# ── 文書定義の検査（lint: E040 / W043） ─────────────────────────
def _lint_doc(sections: list[dict]) -> dict:
    return {"name": "d", "title": "t", "phase": "基本設計", "sections": sections}


def _lint(model: mm.Metamodel, items: list[dict], relations: list[dict],
          sections: list[dict]) -> list:
    """パックの documents を通さず、任意の章定義だけを検査する。"""
    spec = _spec(model, items, relations)
    findings: list = []
    for section in sections:
        findings += publish_module._lint_section(spec, "d", section)
    return findings


def test_標準パックの全列がメタモデルで解決できる(packed: mm.Metamodel) -> None:
    """**回帰の網。** 解決できない列はエラーにならず全行空の列になり、
    「全行が空だったので省略した列」と畳まれて誰も気づかない ―― 実測 r001 で
    コード定義の `value`（正しくは `to.value`）がそうなっていた。文書定義を
    足す・直すたびに、この 1 本が全列を照合する。
    """
    spec = _spec(packed, [], [])
    assert publish_module.lint(spec) == []


def test_関係に無い列はエラーになり反対側を提案する(model: mm.Metamodel) -> None:
    """実測 r001 の形そのもの ―― `value` は has-value（関係）の属性ではなく、
    code-value（to 側）の属性である。"""
    findings = _lint(model, [], [], [
        {"heading": "コード定義", "kind": "relation", "relation": "has-value",
         "columns": ["order", "value", "to.name"]}])

    assert [f.code for f in findings] == ["E040"]
    assert "value" in findings[0].message
    assert "to.value" in (findings[0].hint or "")


def test_相手側に無い列はエラーになる(model: mm.Metamodel) -> None:
    findings = _lint(model, [], [], [
        {"heading": "列定義", "kind": "relation", "relation": "has-column",
         "columns": ["order", "to.physical_name"]}])   # 物理名は関係側にある

    assert [f.code for f in findings] == ["E040"]
    assert "physical_name" in findings[0].message
    assert "関係の属性" in (findings[0].hint or "")


def test_アイテムに無い列はエラーになる(model: mm.Metamodel) -> None:
    findings = _lint(model, [], [], [
        {"heading": "画面一覧", "type": "screen",
         "columns": ["screen_id", "screen_route"]}])

    assert [f.code for f in findings] == ["E040"]
    assert "screen_route" in findings[0].message


def test_未知の種別と関係はエラーになる(model: mm.Metamodel) -> None:
    findings = _lint(model, [], [], [
        {"heading": "a", "type": "tst-case", "columns": ["name"]},
        {"heading": "b", "kind": "relation", "relation": "has-values",
         "columns": ["order"]}])

    assert [f.code for f in findings] == ["E040", "E040"]
    assert "tst-case" in findings[0].message
    assert "has-values" in findings[1].message


def test_文書定義に書けない鍵はエラーになる(model: mm.Metamodel) -> None:
    """`colums` と打ち間違えると**列の無い表**が黙って出る。鍵の語彙を閉じる。"""
    findings = _lint(model, [], [], [
        {"heading": "画面一覧", "type": "screen", "colums": ["screen_id"]}])

    assert "E040" in [f.code for f in findings]
    assert any("colums" in f.message for f in findings)


def test_labelsの死んだ鍵はエラーになる(model: mm.Metamodel) -> None:
    findings = _lint(model, [], [], [
        {"heading": "呼出関係", "kind": "relation", "relation": "calls",
         "columns": ["from.name", "to"], "labels": {"from.nam": "呼ぶ側"}}])

    assert [f.code for f in findings] == ["E040"]
    assert "from.nam" in findings[0].message


def test_全行が空で反対側に値がある列は警告になる(model: mm.Metamodel) -> None:
    """列は解決できる（physical_name は has-column の属性）が、値を持って
    いるのは from 側の entity ―― 静的には判定できないので、データを見て言う。"""
    items = [
        {"id": "ent-1", "type": "entity", "name": "受注", "status": "review",
         "physical_name": "T_ORDER", "statement": "受注を保持すること"},
        {"id": "itm-1", "type": "data-item", "name": "受注番号",
         "statement": "受注を識別すること", "data_type": "文字列",
         "status": "review"}]
    relations = [{"type": "has-column", "from": "ent-1", "to": "itm-1",
                  "order": 1, "status": "review"}]
    findings = _lint(model, items, relations, [
        {"heading": "列定義", "kind": "relation", "relation": "has-column",
         "columns": ["order", "to.name", "physical_name"]}])

    assert [f.code for f in findings] == ["W043"]
    assert "physical_name" in findings[0].message
    assert "from.physical_name" in findings[0].message


def test_値のある列は警告にならない(model: mm.Metamodel) -> None:
    items = [
        {"id": "ent-1", "type": "entity", "name": "受注", "status": "review",
         "physical_name": "T_ORDER", "statement": "受注を保持すること"},
        {"id": "itm-1", "type": "data-item", "name": "受注番号",
         "statement": "受注を識別すること", "data_type": "文字列",
         "status": "review"}]
    relations = [{"type": "has-column", "from": "ent-1", "to": "itm-1",
                  "order": 1, "physical_name": "ORDER_NO", "status": "review"}]
    findings = _lint(model, items, relations, [
        {"heading": "列定義", "kind": "relation", "relation": "has-column",
         "columns": ["order", "to.name", "physical_name"]}])

    assert findings == []


def test_全行が空でも反対側に値のある列は畳まれず脚注が言う(
        model: mm.Metamodel) -> None:
    """**「全行が空」と「対応づけの誤り」を混ぜない。** 畳んでしまうと
    「資料に無い」と同じ顔になる ―― 残して脚注で言う。"""
    items = [
        {"id": "ent-1", "type": "entity", "name": "受注", "status": "review",
         "physical_name": "T_ORDER", "statement": "受注を保持すること"},
        {"id": "itm-1", "type": "data-item", "name": "受注番号",
         "statement": "受注を識別すること", "data_type": "文字列",
         "status": "review"}]
    relations = [{"type": "has-column", "from": "ent-1", "to": "itm-1",
                  "order": 1, "status": "review"}]
    doc = _lint_doc([
        {"heading": "列定義", "kind": "relation", "relation": "has-column",
         "columns": ["order", "to.name", "physical_name"]}])
    spec = _spec(model, items, relations)

    blocks = publish_module._blocks(spec, doc)
    notes = "\n".join(n for b in blocks for n in b.notes)
    assert "物理名" in blocks[0].columns              # 畳まれていない
    assert "from.physical_name" in notes             # 値のある場所を名指しする
    assert "省略した列" not in notes


# ── 様式の回帰（r001 レビューで見つけた列漏れを固定する） ───────
def _publish_one(model: mm.Metamodel, items: list[dict], relations: list[dict],
                 name: str, tmp_path: Path) -> str:
    spec = _spec(model, items, relations)
    written = publish_module.publish(spec, tmp_path, names=[name])
    md = [p for p in written if p.suffix == ".md"]
    assert len(md) == 1
    return md[0].read_text(encoding="utf-8")


def test_テーブル定義書に列の意味が出る(packed: mm.Metamodel,
                                        tmp_path: Path) -> None:
    """列の意味（「10=受付、20=与信保留…」）は data-item の statement にしか
    無い。出さないと、コード値の意味が設計書一式のどこにも残らない（実測 r001）。"""
    items = [
        {"id": "ent-1", "type": "entity", "name": "受注ヘッダ", "status": "review",
         "physical_name": "T_ORDER", "statement": "受注を保持すること"},
        {"id": "itm-1", "type": "data-item", "name": "受注ステータス",
         "statement": "10=受付、20=与信保留、90=取消であること",
         "data_type": "文字列", "length": 2, "status": "review"}]
    relations = [{"type": "has-column", "from": "ent-1", "to": "itm-1",
                  "order": 1, "physical_name": "ORDER_STATUS",
                  "status": "review"}]

    text = _publish_one(packed, items, relations, "table-spec", tmp_path)
    assert "10=受付、20=与信保留、90=取消であること" in text


def test_基本設計書のコード定義にコード値が出る(packed: mm.Metamodel,
                                                tmp_path: Path) -> None:
    """コード値は code-value の value にある。列を `value` と書いていたあいだ
    全行空で畳まれ、No 連番のせいで「1=受付」と読めた（実際は 10）。"""
    items = [
        {"id": "cdm-1", "type": "code-master", "name": "受注ステータス",
         "code_id": "CD-001", "statement": "受注の状態を表すこと",
         "status": "review"},
        {"id": "cdv-1", "type": "code-value", "name": "受付", "value": "10",
         "statement": "受注ステータス 10（受付）は登録直後の状態であること",
         "status": "review"}]
    relations = [{"type": "has-value", "from": "cdm-1", "to": "cdv-1",
                  "order": 1, "status": "review"}]

    text = _publish_one(packed, items, relations, "basic-design", tmp_path)
    row = next(line for line in text.splitlines() if "受付" in line and "|" in line)
    assert "| 10 |" in row                            # No 連番の 1 ではなく値の 10


def test_基本設計書の一覧に仕様が出る(packed: mm.Metamodel,
                                      tmp_path: Path) -> None:
    items = [{"id": "scr-1", "type": "screen", "screen_id": "SCR-001",
              "name": "受注入力", "screen_type": "入力",
              "statement": "受注入力画面は得意先・商品・数量を入力して受注を登録すること",
              "status": "review"}]

    text = _publish_one(packed, items, [], "basic-design", tmp_path)
    assert "受注を登録すること" in text


def test_課題管理表に課題の状態が出る(packed: mm.Metamodel,
                                      tmp_path: Path) -> None:
    """`status` は arp4 の査読状態であって課題の状態ではない ―― 全行「レビュー中」
    になり、資料が「完了」と言う課題を読者が追いかけ続ける（実測 r001・22 件）。
    課題の状態は整理層が description に写した資料の値を出す。"""
    items = [{"id": "iss-1", "type": "open-issue", "issue_id": "ISS-001",
              "name": "在庫引当タイミングの未確定",
              "statement": "引当のタイミングが確定していないこと",
              "description": "起票日 2025-08-22 / 工程 基本設計 / 区分 仕様 / 状態 対応中",
              "assignee": "鈴木", "status": "review"}]

    text = _publish_one(packed, items, [], "issue-register", tmp_path)
    assert "対応中" in text
    assert "レビュー中" not in text


# ── known_gaps の露出（黙って無い節を作らない） ─────────────────
_KG_REASON = "共通基盤の画面にはサブシステム別の画面仕様書が無い。先方へ提供を依頼する"


def _kg_spec(model: mm.Metamodel) -> Spec:
    """画面 2 枚。1 枚は項目あり、1 枚は known_gaps で「資料に無い」を宣言済み。"""
    return _spec(model, [
        {"id": "scr-1", "type": "screen", "screen_id": "SCR-002",
         "name": "受注入力", "statement": "受注を登録すること", "status": "review"},
        {"id": "scr-2", "type": "screen", "screen_id": "SCR-003",
         "name": "ログイン", "statement": "利用者を認証すること", "status": "review",
         "known_gaps": {"displays": {"reason": _KG_REASON, "at": "2026-08-11"}}},
        {"id": "scr-3", "type": "screen", "screen_id": "SCR-004",
         "name": "在庫照会", "statement": "在庫を照会すること", "status": "review"},
        {"id": "itm-1", "type": "data-item", "name": "受注番号",
         "statement": "受注を識別すること", "data_type": "文字列",
         "status": "review"},
    ], [
        {"type": "displays", "from": "scr-1", "to": "itm-1", "order": 1,
         "io": "入力", "status": "review"},
        {"type": "displays", "from": "scr-3", "to": "itm-1", "order": 1,
         "io": "出力", "status": "review"},
    ])


_KG_SECTION = {"heading": "画面帳票項目", "kind": "relation", "relation": "displays",
             "group_by": "from",
             "columns": ["order", "to.name", "io", "to.data_type"]}
_KG_DOC = {"name": "d", "title": "t", "phase": "基本設計",
            "sections": [_KG_SECTION]}


def test_known_gapsを宣言した画面は脚注に出る(model: mm.Metamodel) -> None:
    """節が黙って存在しないと「項目が無い画面」と「資料が無い画面」が読み手に
    区別できない（実測 r001・画面 3 枚と帳票 3 本が黙って消えていた）。"""
    blocks = publish_module._blocks(_kg_spec(model), _KG_DOC)

    notes = "\n".join(n for b in blocks for n in b.notes)
    assert "ログイン" in notes
    assert _KG_REASON in notes
    assert "known_gaps" in notes


def test_関係のあるアイテムはknown_gapsでも脚注に出ない(
        model: mm.Metamodel) -> None:
    """節が出ているものまで並べると、宣言の消し忘れ（W033 の仕事）まで
    こちらが言うことになる。"""
    spec = _kg_spec(model)
    spec.items[0]["known_gaps"] = {"displays": {"reason": "消し忘れの宣言",
                                                "at": "2026-08-11"}}
    blocks = publish_module._blocks(spec, _KG_DOC)

    notes = "\n".join(n for b in blocks for n in b.notes)
    assert "受注入力" not in notes
    assert "消し忘れの宣言" not in notes


def test_束が1つでもknown_gapsは脚注に出る(model: mm.Metamodel) -> None:
    """節に割らない（束が 1 つの）表でも、宣言は同じように見せる。"""
    spec = _kg_spec(model)
    spec.relations.pop()                    # scr-3 の関係を消して束を 1 つにする
    blocks = publish_module._blocks(spec, _KG_DOC)

    notes = "\n".join(n for b in blocks for n in b.notes)
    assert "ログイン" in notes and _KG_REASON in notes


# ── 1 つの種別を複数の設計書へ振り分ける（where の「残り全部」） ────
_SPLIT_RULES = [
    {"id": "rul-1", "type": "business-rule", "status": "review",
     "rule_id": "RUL-001", "name": "与信枠", "rule_kind": "business",
     "statement": "与信枠を超える受注は保留すること"},
    {"id": "rul-2", "type": "business-rule", "status": "review",
     "rule_id": "RUL-002", "name": "必須入力", "rule_kind": "validation",
     "statement": "受注数量は未入力でないこと"},
    {"id": "rul-3", "type": "business-rule", "status": "review",
     "rule_id": "RUL-003", "name": "区分の無いルール",
     "statement": "区分を書き忘れても落とさないこと"},
]


def test_whereで振り分けた種別は片方にしか出ない(model: mm.Metamodel) -> None:
    """**同じ表が 2 つの工程で別々に承認される**のを止める（実測 r001 で
    業務ルール 55 件が基本設計書と詳細設計書に全文で重複し、詳細設計書に固有の
    ルールは 0 件だった ―― どちらが正かが読めない）。"""
    spec = _spec(model, _SPLIT_RULES, [])
    business = publish_module._blocks(spec, _lint_doc([
        {"heading": "業務ルール", "type": "business-rule",
         "where": {"rule_kind": ["business", "calculation"]},
         "columns": ["rule_id", "name"]}]))

    assert [row[0] for row in business[0].rows] == ["RUL-001"]


def test_残り全部を引き受ける節は絞りに漏れを作らない(model: mm.Metamodel) -> None:
    """``{not: [...]}`` は**残り全部**。白列挙どうしで振り分けると、
    ``extensible: true`` の enum に値が生えた（あるいは未設定の）ときに
    **どちらの設計書にも出ないアイテム**ができる。"""
    spec = _spec(model, _SPLIT_RULES, [])
    rest = publish_module._blocks(spec, _lint_doc([
        {"heading": "実装ルール", "type": "business-rule",
         "where": {"rule_kind": {"not": ["business", "calculation"]}},
         "columns": ["rule_id", "name"]}]))

    assert [row[0] for row in rest[0].rows] == ["RUL-002", "RUL-003"]


# ── trace は関係を配列で書ける（被覆は和集合） ──────────────────
_NFR = {"id": "req-nf", "type": "requirement", "status": "review",
        "req_id": "NFR-001", "kind": "非機能", "name": "稼働率",
        "statement": "99.5% 以上とすること"}
_FR = {"id": "req-fn", "type": "requirement", "status": "review",
       "req_id": "FR-001", "kind": "機能", "name": "受注登録",
       "statement": "受注を登録できること"}
_METHOD = {"id": "cst-1", "type": "constraint", "status": "review",
           "constraint_id": "CST-001", "name": "冗長化方式", "category": "技術",
           "statement": "AP サーバを 2 重化すること"}
_SCREEN = {"id": "scr-1", "type": "screen", "status": "review",
           "screen_id": "SCR-001", "name": "受注入力"}
_COVER = [{"type": "realizes", "from": "scr-1", "to": "req-fn",
           "status": "review"},
          {"type": "constrains", "from": "cst-1", "to": "req-nf",
           "status": "review"}]
_MULTI = {"heading": "要件 → 設計要素", "kind": "trace", "type": "requirement",
          "relation": ["realizes", "constrains"],
          "columns": ["req_id", "kind", "name", "linked"]}
_MULTI_GAP = {**_MULTI, "heading": "未実現の要件（設計漏れ）", "gap": True,
              "columns": ["req_id", "kind", "name", "status"]}


def test_被覆は複数の関係の和集合で見る(model: mm.Metamodel) -> None:
    """``realizes.from`` は ``設計要素`` グループで ``constraint`` を含まない
    ―― 方式が非機能要件に応える経路は ``constrains`` にしかない。1 本に決め
    打つと、整理層が正しく張った関係が「設計漏れ」の誤警報になる（実測 r001 で
    非機能 15 件が全件並び、うち 7 件は方式が応えていた）。"""
    blocks = publish_module._blocks(
        _spec(model, [_FR, _NFR, _METHOD, _SCREEN], _COVER),
        _lint_doc([_MULTI]))

    linked = {row[0]: row[3] for row in blocks[0].rows}
    assert linked["FR-001"] == "SCR-001 受注入力"
    assert linked["NFR-001"] == "CST-001 冗長化方式"


def test_どれか1本あればgapに残らない(model: mm.Metamodel) -> None:
    """3 章（対応表）と 4 章（漏れの一覧）で母集合も関係も揃える ―― 違うと
    同じ問いに 2 つの答えが出る。"""
    blocks = publish_module._blocks(
        _spec(model, [_FR, _NFR, _METHOD, _SCREEN], _COVER),
        _lint_doc([_MULTI_GAP]))

    assert [b.rows for b in blocks if b.rows] == []


def test_配列で書いた関係はE040にならない(model: mm.Metamodel) -> None:
    """配列をそのまま文字列にすると ``['realizes', 'constrains']`` という
    関係型を探して誤検出する。**未知の名前だけを 1 本ずつ言う。**"""
    assert _lint(model, [], [], [_MULTI]) == []

    broken = _lint(model, [], [], [{**_MULTI, "relation": ["realizes", "realize"]}])
    assert [f.code for f in broken] == ["E040"]
    assert "realize" in broken[0].message


# ── 同名のアイテムに所有元の修飾子を付ける ──────────────────────
def _same_name(model: mm.Metamodel) -> Spec:
    """「得意先コード」が 2 つのテーブルの列として、それぞれ 1 件ずつある。"""
    return _spec(model, [
        {"id": "ent-1", "type": "entity", "status": "review", "name": "受注ヘッダ",
         "physical_name": "T_ORDER"},
        {"id": "ent-2", "type": "entity", "status": "review", "name": "請求ヘッダ",
         "physical_name": "T_BILL"},
        {"id": "di-1", "type": "data-item", "status": "review",
         "name": "得意先コード", "data_type": "文字列"},
        {"id": "di-2", "type": "data-item", "status": "review",
         "name": "得意先コード", "data_type": "文字列"},
        {"id": "di-3", "type": "data-item", "status": "review",
         "name": "請求金額", "data_type": "数値"},
        {"id": "cst-1", "type": "constraint", "status": "review",
         "constraint_id": "CST-008", "name": "得意先コードのドメイン",
         "statement": "6 桁の数字とすること"},
    ], [
        {"type": "has-column", "from": "ent-1", "to": "di-1", "order": 1,
         "physical_name": "CUST_CD", "status": "review"},
        {"type": "has-column", "from": "ent-2", "to": "di-2", "order": 1,
         "physical_name": "CUST_CD", "status": "review"},
        {"type": "has-column", "from": "ent-2", "to": "di-3", "order": 2,
         "physical_name": "AMOUNT", "status": "review"},
        {"type": "constrains", "from": "cst-1", "to": "di-1", "status": "review"},
        {"type": "constrains", "from": "cst-1", "to": "di-2", "status": "review"},
    ])


_CONSTRAINS = {"heading": "制約が縛るもの", "kind": "relation",
               "relation": "constrains",
               "labels": {"from": "制約・業務ルール", "to": "縛る先"},
               "columns": ["from", "to"]}


def test_同名のアイテムは所有元で修飾する(model: mm.Metamodel) -> None:
    """実測（r001）で `data-item` 226 件のうち 40 名称・131 件が同名で、
    要件定義書「制約が縛るもの」のデータ項目 113 行のうち 100 行は表示名だけでは
    どれのことか決まらなかった ―― 正本は正しく、出すときに親を辿っていない。"""
    blocks = publish_module._blocks(_same_name(model), _lint_doc([_CONSTRAINS]))

    assert sorted(row[1] for row in blocks[0].rows) == [
        "受注ヘッダ.得意先コード（データ項目）",
        "請求ヘッダ.得意先コード（データ項目）"]


def test_同名が1件しか無いなら修飾しない(model: mm.Metamodel) -> None:
    """**無駄に長くしない。** 区別が要らないところで修飾子を付けると、
    区別のための語が行の中でいちばん長くなる。"""
    assert publish_module._qualifiers(_same_name(model)).get("di-3") is None


def test_親が2つ以上あるなら並べない(model: mm.Metamodel) -> None:
    """同じデータ項目を複数のテーブル・画面が指すことはある ―― そのときは
    件数だけ言って、表示 ID から辿らせる。"""
    spec = _same_name(model)
    spec.relations.append({"type": "has-column", "from": "ent-1", "to": "di-2",
                           "order": 2, "physical_name": "CUST_CD",
                           "status": "review"})

    assert publish_module._qualifiers(spec)["di-2"] == "得意先コード（親が 2 件）"


# ── 目次の付録（決定記録・stakeholder 向け） ────────────────────
def test_目次の付録から決定記録とstakeholderへ行ける(
        crud: Spec, tmp_path: Path) -> None:
    """どちらも様式（`documents/*.yml`）から出ないので ``placed`` に入らない
    ―― 実測（r001）で 7 本が目次のどこからも指されておらず、**束を渡された人は
    そこへ到達できなかった**。付録は工程の節の外に置く（束全体の記録である）。"""
    (tmp_path / "決定記録.md").write_text("x", encoding="utf-8")
    (tmp_path / "stakeholder").mkdir()
    (tmp_path / "stakeholder" / "用語集.md").write_text("x", encoding="utf-8")

    md, page = publish_module._index(crud, tmp_path, [
        ("基本設計", "CRUD図", tmp_path / "2_基本設計" / "CRUD図.md", False)])

    index = md.read_text(encoding="utf-8")
    assert "## 付録" in index
    assert index.index("## 基本設計") < index.index("## 付録")
    assert "- [決定記録](決定記録.md)" in index
    assert "- [用語集](stakeholder/用語集.md)" in index

    # HTML の目次からも辿れる。付録は HTML を持たないので `.md` を指す。
    body = page.read_text(encoding="utf-8")
    assert '<a href="決定記録.md" target="_blank">決定記録</a>' in body
    assert '<a href="stakeholder/用語集.md" target="_blank">用語集</a>' in body


def test_付録が無ければ節ごと出さない(crud: Spec, tmp_path: Path) -> None:
    md, _page = publish_module._index(crud, tmp_path, [
        ("基本設計", "CRUD図", tmp_path / "2_基本設計" / "CRUD図.md", False)])

    assert "## 付録" not in md.read_text(encoding="utf-8")
