"""derived 層（AI の解釈）と読者別 publish ―― Phase 3。

事実層の規律（資料に無いことを書かない）を緩めずに要約・抽象化を持つには、
**層を足して混ぜない**しかない。混ぜない担保が basis（根拠の実在を機械検証）と
confidence（推定であることの宣言）である。
"""

from __future__ import annotations

from arp4 import audience, decisions, derived, spec as spec_module, yamlio
from arp4.metamodel import Metamodel
from arp4.paths import Paths, Round
from arp4.spec import Spec
from conftest import parsed, write


def _spec(project: Paths, model: Metamodel,
          items: list | None = None, relations: list | None = None) -> Spec:
    return Spec(metamodel=model, items=items or [], relations=relations or [],
                paths=project)


def _derived(project: Paths, body: str) -> None:
    write(project.spec / "derived" / "解釈.yml", body)


_GOOD = """\
- id: drv-受注管理
  type: 機能グループ
  name: 受注管理
  statement: 受注の登録から出荷指示までを扱う機能のまとまり。
  confidence: 高
  basis:
    - { round: "2026-08-02", file: src/x.py, anchor: i1 }
"""


def _anchor(round_: Round) -> None:
    parsed(round_, "src/x.py.md", """\
# src/x.py

<!-- source: src/x.py -->

## 取り込み  <!-- a:i1 at=src/x.py -->

| 取り込み | 元 | 名前 | 行 |
|---|---|---|---|
""")


# ── 契約の検証 ──────────────────────────────────────────────────
def test_実在するbasisは通る(project: Paths, round_: Round,
                             model: Metamodel) -> None:
    _anchor(round_)
    _derived(project, _GOOD)

    assert derived.check(_spec(project, model)) == []


def test_実在しないアンカーのbasisはerror(project: Paths, round_: Round,
                                          model: Metamodel) -> None:
    """**幻覚の最頻形は存在しない根拠。** 全件を機械で潰す（検体テスト）。"""
    _anchor(round_)
    _derived(project, _GOOD.replace("anchor: i1", "anchor: s9-t9"))

    findings = derived.check(_spec(project, model))
    assert [f.code for f in findings] == ["D003"]
    assert findings[0].level == "error"


def test_basisの無いアイテムはerror(project: Paths, model: Metamodel) -> None:
    _derived(project, """\
- id: drv-x
  type: 概要
  name: 概要
  statement: 何かをするシステム。
  confidence: 高
""")
    assert [f.code for f in derived.check(_spec(project, model))] == ["D002"]


def test_confidenceの無いアイテムはerror(project: Paths, round_: Round,
                                         model: Metamodel) -> None:
    """「資料に無い」と「AI が推定した」を混ぜない ―― 確度は必須の宣言である。"""
    _anchor(round_)
    _derived(project, _GOOD.replace("  confidence: 高\n", ""))

    assert [f.code for f in derived.check(_spec(project, model))] == ["D004"]


def test_basisにはアイテムIDも書ける(project: Paths, model: Metamodel) -> None:
    _derived(project, """\
- id: drv-x
  type: 機能グループ
  name: 受注
  statement: 受注のまとまり。
  confidence: 中
  basis: [mod-001]
""")
    spec = _spec(project, model, items=[
        {"id": "mod-001", "type": "module", "status": "review", "name": "受注"}])

    assert derived.check(spec) == []
    # 指す先が消えれば error
    assert [f.code for f in derived.check(_spec(project, model))] == ["D003"]


# ── stakeholder 向け publish ────────────────────────────────────
def test_stakeholder一式が出る(project: Paths, round_: Round,
                               model: Metamodel) -> None:
    _anchor(round_)
    _derived(project, _GOOD + """\
- id: drv-summary
  type: 概要
  name: 概要
  statement: 受注を登録し出荷を指示する販売管理システム。
  confidence: 高
  basis:
    - { round: "2026-08-02", file: src/x.py, anchor: i1 }
- id: drv-flow
  type: 処理フロー
  name: 受注の流れ
  statement: 受注から出荷指示までの推定の流れ。
  confidence: 中
  basis:
    - { round: "2026-08-02", file: src/x.py, anchor: i1 }
  flow: [受注受付, 与信判定, 出荷指示]
""")
    spec = _spec(project, model, items=[
        {"id": "mod-001", "type": "module", "status": "review", "name": "受注",
         "statement": "受注を扱うこと"},
        {"id": "mod-002", "type": "module", "status": "review", "name": "出荷",
         "statement": "出荷を扱うこと"},
        {"id": "trm-001", "type": "glossary-term", "status": "review",
         "name": "与信", "reading": "よしん", "english": "credit",
         "statement": "掛け売りの上限のこと"},
        {"id": "tcs-001", "type": "test-case", "status": "review",
         "name": "受注できる", "statement": "受注できること", "expected": "OK"},
    ], relations=[
        {"type": "calls", "from": "mod-001", "to": "mod-002", "status": "review"},
        {"type": "verifies", "from": "tcs-001", "to": "mod-001",
         "status": "review"},
    ])
    loaded, findings = derived.load(project)
    assert not findings

    written = audience.publish_stakeholder(spec, loaded, project.out)
    names = {p.name for p in written}
    assert names == {"システム概要.md", "機能一覧.md", "用語集.md",
                     "テスト状況サマリ.md", "構成図.md", "処理フロー図.md"}

    overview = (project.out / audience.DIR / "システム概要.md") \
        .read_text(encoding="utf-8")
    assert "販売管理システム" in overview
    assert "確度: 高" in overview                # AI の解釈には確度を必ず添える

    diagram = (project.out / audience.DIR / "構成図.md") \
        .read_text(encoding="utf-8")
    assert "```mermaid" in diagram and "受注" in diagram

    flow = (project.out / audience.DIR / "処理フロー図.md") \
        .read_text(encoding="utf-8")
    assert "与信判定" in flow and "```mermaid" in flow

    tests = (project.out / audience.DIR / "テスト状況サマリ.md") \
        .read_text(encoding="utf-8")
    assert "1 / 1 件" in tests                   # verifies の紐付き件数


def test_構成図は描かなかったモジュールを数で言う(project: Paths,
                                                 model: Metamodel) -> None:
    """**描いた件数を「正本の全件」と言わない。**

    ``calls`` はファイルの取り込みから作るので、クラス由来の module は端に
    現れず、テストの取り込みは ``verifies`` へ回る ―― 絵に出るのは正本の
    モジュールの一部でしかない。自己仕様では 111 件中 33 件しか描いておらず、
    「モジュール 33 件（正本に登録された全件）」と読めた。落としたものは
    種別と件数で名指しする（→ `publish._omitted` と同じ規律）。
    """
    spec = _spec(project, model, items=[
        {"id": "mod-001", "type": "module", "status": "review", "name": "受注",
         "statement": "受注を扱うこと"},
        {"id": "mod-002", "type": "module", "status": "review", "name": "出荷",
         "statement": "出荷を扱うこと"},
        {"id": "mod-003", "type": "module", "status": "review", "name": "受注明細",
         "class_name": "受注.明細", "statement": "明細を持つこと"},
        {"id": "mod-004", "type": "module", "status": "review",
         "name": "tests.test_受注", "statement": "受注を見ること"},
    ], relations=[
        {"type": "calls", "from": "mod-001", "to": "mod-002", "status": "review"},
    ])
    audience.publish_stakeholder(spec, derived.Derived(), project.out)
    diagram = (project.out / audience.DIR / "構成図.md") \
        .read_text(encoding="utf-8")

    assert "呼出関係 1 本（正本に登録された全件）" in diagram
    assert "その両端に現れるモジュール 2 件" in diagram
    # 描かなかった 2 件は黙って落とさず、種別ごとに数で言う
    assert "正本のモジュールは全 4 件" in diagram
    assert "呼出関係を 1 本も持たない 2 件" in diagram
    assert "クラス由来 1 件・ファイル由来 1 件" in diagram
    # 全部描けたときは断りを足さない
    assert "受注明細" not in diagram and "tests.test_受注" not in diagram


def test_全部描けた構成図には断りを足さない(project: Paths,
                                            model: Metamodel) -> None:
    """落としたものが無いのに「描いていない」と書くと、逆に数を疑わせる。"""
    spec = _spec(project, model, items=[
        {"id": "mod-001", "type": "module", "status": "review", "name": "受注",
         "statement": "受注を扱うこと"},
        {"id": "mod-002", "type": "module", "status": "review", "name": "出荷",
         "statement": "出荷を扱うこと"},
    ], relations=[
        {"type": "calls", "from": "mod-001", "to": "mod-002", "status": "review"},
    ])
    audience.publish_stakeholder(spec, derived.Derived(), project.out)
    diagram = (project.out / audience.DIR / "構成図.md") \
        .read_text(encoding="utf-8")

    assert "呼出関係 1 本（正本に登録された全件）" in diagram
    assert "描いていない" not in diagram


def test_データが無い文書は自己申告する(project: Paths,
                                        model: Metamodel) -> None:
    """「作ったが空」と「作っていない」を目次から取り違えさせない（3-4）。"""
    written = audience.publish_stakeholder(
        _spec(project, model), derived.Derived(), project.out)

    for path in written:
        body = path.read_text(encoding="utf-8")
        assert "この文書に出せるデータが正本にありません" in body
        assert "必要な語彙" in body


# ── 決定記録 ────────────────────────────────────────────────────
def test_決定記録が付録として出る(project: Paths, round_: Round,
                                  model: Metamodel) -> None:
    decisions.append(round_, [decisions.entry(
        "draft", "tier=Common を付けた", "@dataclass の転記",
        decisions.SURE, ["src/x.py.md#m1"])])

    path = audience.decision_report(_spec(project, model), project.out)

    assert path is not None
    body = path.read_text(encoding="utf-8")
    assert "tier=Common を付けた" in body
    # **根拠（basis）はここには出ない。** 判断の型で畳んだ表なので、全件は
    # decisions.yml にある ―― そこを名指しする 1 行が導線である。
    assert "decisions.yml" in body
    assert "AI" not in body                     # 主体は機械だけ（draft/build/auto）


def test_決定が無ければ付録を出さない(project: Paths, model: Metamodel) -> None:
    assert audience.decision_report(_spec(project, model), project.out) is None


def test_決定記録は判断の型で畳む(project: Paths, round_: Round,
                                  model: Metamodel) -> None:
    """実測（r001）の明細 175 行は **ほぼ同一の文の反復**で、理由は 7 種の
    「書いた向きのまま入れた」に集中していた ―― 同じ文が 42 回並ぶことは、
    「止めたい判断を差し戻す」という目的に 1 度並ぶことの何も足さない。"""
    decisions.append(round_, [
        decisions.entry("build", f"leads-to（S{n} → S{n + 1}）を書いた向きのまま入れた",
                        "どちらの向きも宣言に合う", decisions.GUESS)
        for n in range(5)])

    path = audience.decision_report(_spec(project, model), project.out)

    assert path is not None
    body = path.read_text(encoding="utf-8")
    rows = [line for line in body.splitlines() if line.startswith("| build")]
    assert len(rows) == 1                       # 5 件が 1 型に畳まれる
    assert "| 5 |" in rows[0]
    assert "leads-toを書いた向きのまま入れた" in rows[0]
    assert "S0 → S1・S1 → S2・S2 → S3 ほか 2 件" in rows[0]   # 代表は 3 件まで
    assert "5 件を 1 型にまとめました" in body


def test_畳んでも入れ子の括弧で型を取り違えない(project: Paths, round_: Round,
                                                model: Metamodel) -> None:
    """主語のほうに括弧が入ることがある（`オーダー入力（代行入力）`）――
    非貪欲な正規表現で切ると、内側で切れて型が 1 件ずつに割れる。"""
    decisions.append(round_, [
        decisions.entry("build", "leads-to（オーダー入力（代行入力） → 与信）を"
                                 "書いた向きのまま入れた", "宣言に合う",
                        decisions.GUESS),
        decisions.entry("build", "leads-to（受注受付 → 与信）を"
                                 "書いた向きのまま入れた", "宣言に合う",
                        decisions.GUESS)])

    body = audience.decision_report(_spec(project, model),
                                    project.out).read_text(encoding="utf-8")

    rows = [line for line in body.splitlines() if line.startswith("| build")]
    assert len(rows) == 1
    assert "オーダー入力（代行入力） → 与信" in rows[0]
