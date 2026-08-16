"""段の境界と、**逆向きの検査**（案 5・案 6）。

arp4 の検査は順方向に偏っていた ―― パース結果 → 整理結果（``G001``）、整理結果
→ 台帳（``G003``）、正本 → パース結果（``G004`` / ``G010``）は見ているのに、
その逆はどこも見ていない。逆向きに落ちるものは**黙って通る**ので、
「静かに消える」種類の欠落だけが残っていた。

段の境界も同じ形の穴である。``G002`` が「語彙にある関係か」しか見なかったせいで
``build`` が ``B013`` で落ちていたのと同じことが、``build`` → ``check`` →
``publish`` のあいだにも残っていた。
"""

from __future__ import annotations

import dataclasses

from arp4 import concepts as concepts_module
from arp4 import freeze, pack, publish as publish_module
from arp4.concepts import Concept
from arp4.metamodel import Metamodel
from arp4.paths import Round
from arp4.spec import Spec
from conftest import codes, organized, parsed

_PARSED = """\
# a.xlsx / 受注テーブル

<!-- source: 資料/a.xlsx / シート: 受注テーブル -->

## 表 B5:H8  <!-- a:s1-t1 at=B5:H8 -->

| 論理名 | 物理名 |
|---|---|
| 受注番号 | ORDER_NO |
"""

_ORGANIZED = """\
records:
  - concept: c-受注番号
    type: データ項目
    name: 受注番号
    statement: 受注番号は文字列型の項目であること
    source: { anchor: s1-t1 }
"""


def _with_documents(model: Metamodel, items: list[dict]) -> Spec:
    """文書定義を引けるようにした正本（``pending`` は様式を舐める）。"""
    chain, findings = pack.resolve_chain("jp-sier-std")
    assert not [f for f in findings if f.level == "error"]
    return Spec(metamodel=dataclasses.replace(model, chain=tuple(chain)),
                items=items, relations=[])


# ── 案 5: 段の境界 ──────────────────────────────────────────────
def test_宣言に無い属性は凍結の前に言う(round_: Round, model: Metamodel) -> None:
    """``build`` は宣言に無い属性を**捨てる**（``B021``）。それが出るのは凍結した
    あとで、そのときには**整理結果はもう編集できない。**"""
    parsed(round_, "資料/a.xlsx/受注テーブル.md", _PARSED)
    organized(round_, "資料/a.xlsx/受注テーブル.yml", _ORGANIZED.replace(
        "    source: { anchor: s1-t1 }",
        "    attrs: { 桁数: 10, data_type: 文字列 }\n    source: { anchor: s1-t1 }"))

    said = [f for f in freeze.gate(round_, model, {}).findings if f.code == "G016"]

    assert len(said) == 1
    assert said[0].level == "warn"            # B021 と同じ重み
    assert "桁数" in said[0].message
    assert "data_type" not in said[0].message  # 宣言にある属性は言わない


def test_関係の属性も見る(round_: Round, model: Metamodel) -> None:
    """関係の属性は ``build`` が**黙って捨てる**（``B024`` は食い違いしか言わない）
    ので、言われなければ気づく手がかりが無い。"""
    parsed(round_, "資料/a.xlsx/受注テーブル.md", _PARSED)
    organized(round_, "資料/a.xlsx/受注テーブル.yml", _ORGANIZED.replace(
        "    type: データ項目", "    type: エンティティ").replace(
        "    source: { anchor: s1-t1 }",
        "    source: { anchor: s1-t1 }\n"
        "    refs: [{ rel: has-column, to: c-x, attrs: { primary_key: true } }]"))

    said = [f for f in freeze.gate(round_, model, {}).findings if f.code == "G016"]

    assert [f.message for f in said] == \
        ["has-column に無い属性です: primary_key（build が捨てます）"]


def test_資料は届いているのに空になる設計書をcheckで言う(model: Metamodel) -> None:
    """実測では CRUD 図が空だと分かるのが**生成して人が目で見たとき**で、しかも
    設計書には「資料が無いのかもしれない」と出ていた。"""
    items = [{"id": "mod-1", "type": "module", "name": "受注登録",
              "statement": "受注を登録すること"},
             {"id": "ent-1", "type": "entity", "name": "受注",
              "statement": "受注を保持すること"}]
    spec = _with_documents(model, items)

    said = [f for f in publish_module.pending(spec) if f.code == "W034"]

    # トレーサビリティも挙がる ―― `refines` は module どうしにも張れるので、
    # **材料はある**。ここで見たいのは CRUD 図が挙がることなので、含まれるかで見る
    # （様式に節が足されるたびに期待値を書き換えるのは、検査ではなく写経になる）。
    crud = [f for f in said if f.target == "CRUD図"]
    assert crud, [f.target for f in said]
    assert "資料は届いているのに空になります" in crud[0].message
    assert "起点 1 件 / 終点 1 件" in crud[0].message


def test_資料が無いだけのものは言わない(model: Metamodel) -> None:
    """11 種のうち 8 種が「正しい空」なので、全部並べると本当に困っている
    ものが埋もれる。**それは publish が本文に書けば足りる。**"""
    spec = _with_documents(model, [])
    assert [f.target for f in publish_module.pending(spec)] == []


def test_空の理由を設計書が言い分ける(model: Metamodel) -> None:
    items = [{"id": "mod-1", "type": "module", "name": "受注登録", "statement": "x"},
             {"id": "ent-1", "type": "entity", "name": "受注", "statement": "y"}]
    spec = Spec(metamodel=model, items=items, relations=[])

    reasons = dict((name, kind) for name, kind, _ in
                   publish_module.unmet(spec, ["accesses", "screen"]))

    assert reasons["accesses"] == "関係"     # 資料は届いている
    assert reasons["screen"] == "資料"       # 本当に無い


# ── 案 6: 逆向きの対応 ──────────────────────────────────────────
def test_パース結果の無い整理結果は孤児として言う(round_: Round,
                                                model: Metamodel) -> None:
    """資料が改訂されてシート名が変わると、整理結果は前の名前のまま残り、
    **中身は 1 件も正本に入らない**のに凍結が通っていた。"""
    parsed(round_, "資料/a.xlsx/受注テーブル.md", _PARSED)
    organized(round_, "資料/a.xlsx/受注テーブル.yml", _ORGANIZED)
    organized(round_, "資料/a.xlsx/古いシート名.yml", _ORGANIZED)

    said = [f for f in freeze.gate(round_, model, {}).findings if f.code == "G004"]

    assert len(said) == 1
    assert said[0].file.endswith("古いシート名.yml")


def test_孤児はレコードの数だけ並べない(round_: Round, model: Metamodel) -> None:
    """50 レコードあるファイルが孤児になると 50 件並ぶ ―― 直す操作はどのみち
    「ファイルの名前を直す」1 つしかない。"""
    body = "records:\n" + "".join(
        f"  - concept: c-{i}\n    type: データ項目\n    name: x{i}\n"
        f"    statement: x{i} は項目であること\n    source: {{ anchor: s1-t1 }}\n"
        for i in range(10))
    organized(round_, "資料/a.xlsx/相方が無い.yml", body)

    said = [f for f in freeze.gate(round_, model, {}).findings if f.code == "G004"]
    assert len(said) == 1


def test_対象外宣言だけのファイルも拾う(round_: Round, model: Metamodel) -> None:
    """レコードが 0 件だと ``G004`` のレコード側では 1 件も出ない ――
    **いちばん静かに消えるのがこの形**である。"""
    organized(round_, "資料/a.xlsx/表紙.yml",
              "out_of_scope:\n  - { anchor: s1-x1, reason: 表紙 }\n")

    assert codes(freeze.gate(round_, model, {}).findings) == ["G004"]


def test_台帳が指すアイテムが消えていたらerror() -> None:
    """切れたまま次のラウンドを組むと、同じ concept が**別のアイテムとして
    起こし直され**、統合したはずのものが 2 つに割れる。"""
    known = {"c-受注": Concept(concept="c-受注", item="ent-消えた")}
    said = concepts_module.check(known, {"ent-1"})

    assert [f.code for f in said] == ["E029"]
    assert said[0].level == "error"


def test_写像が空なのはwarnに留める() -> None:
    """次のラウンドで使われる予定かもしれない ―― **機械には決められない。**"""
    known = {"c-受注": Concept(concept="c-受注")}
    said = concepts_module.check(known, set())

    assert [(f.code, f.level) for f in said] == [("W035", "warn")]


def test_写像が生きていれば黙る() -> None:
    known = {"c-受注": Concept(concept="c-受注", item="ent-1")}
    assert concepts_module.check(known, {"ent-1"}) == []
