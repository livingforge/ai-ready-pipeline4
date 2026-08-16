"""③ 構築 ―― **意味の判断を含まない**こと、**冪等**であることを確かめる。"""

from __future__ import annotations

from typing import Any

from arp4 import build, concepts as concepts_module, organized as organized_module
from arp4 import spec as spec_module
from arp4.paths import Paths, Round
from conftest import codes, organized

_RECORDS = """\
records:
  - concept: c-T_ORDER
    type: エンティティ
    name: 受注
    statement: 受注テーブル T_ORDER は受注 1 件を 1 レコードで保持すること
    attrs: { physical_name: T_ORDER, entity_kind: トランザクション }
    source: { anchor: s1-t1 }
    refs:
      - { rel: has-column, to: c-受注番号,
          attrs: { physical_name: ORDER_NO, pk: true } }
  - concept: c-受注番号
    type: データ項目
    name: 受注番号
    statement: 受注番号は文字列型（10 桁）の項目であること
    attrs: { data_type: 文字列, length: 10 }
    source: { anchor: s1-t1 }
"""


def _plan(project: Paths, round_: Round, body: str = _RECORDS,
          known: dict[str, Any] | None = None):
    organized(round_, "資料/a.xlsx/受注.yml", body)
    spec, findings = spec_module.load(project)
    assert not [f for f in findings if f.level == "error"]
    result, load_findings = organized_module.load(round_)
    assert not [f for f in load_findings if f.level == "error"]
    return spec, build.plan(spec, result, known if known is not None else {}, round_.name)


def test_conceptでマージしアイテムになる(project: Paths, round_: Round) -> None:
    spec, plan = _plan(project, round_)

    assert plan.metrics["created"] == 2
    entity = [i for i in plan.created if i["type"] == "entity"][0]
    assert entity["physical_name"] == "T_ORDER"
    assert entity["status"] == "review"          # 承認は人だけ
    assert entity["source"] == [{"round": "2026-08-02",
                                 "file": "資料/a.xlsx/受注", "anchor": "s1-t1"}]


def test_アイテムIDはconceptから決まる(project: Paths, round_: Round) -> None:
    """**ラウンドが増えても実行順が変わっても動かない。**"""
    _, first = _plan(project, round_)
    ids = sorted(i["id"] for i in first.created)

    _, second = _plan(project, project.round("2026-11-14"))
    assert sorted(i["id"] for i in second.created) == ids


def test_台帳の写像があればそれを使う(project: Paths, round_: Round) -> None:
    """item を分割・統合しても**凍結物は無傷**でいられる、の要になる性質。"""
    known = {"c-T_ORDER": concepts_module.Concept(
        concept="c-T_ORDER", item="ent-手で決めた")}
    _, plan = _plan(project, round_, known=known)

    assert any(i["id"] == "ent-手で決めた" for i in plan.created)


def test_関係の向きは補正される(project: Paths, round_: Round) -> None:
    """抽出は「自分は誰に属するか」の向きで出がちだが、宣言は親 → 子である。"""
    body = _RECORDS.replace(
        """    refs:
      - { rel: has-column, to: c-受注番号,
          attrs: { physical_name: ORDER_NO, pk: true } }
""", "").replace(
        """    attrs: { data_type: 文字列, length: 10 }
    source: { anchor: s1-t1 }""",
        """    attrs: { data_type: 文字列, length: 10 }
    source: { anchor: s1-t1 }
    refs: [{ rel: has-column, to: c-T_ORDER, attrs: { physical_name: ORDER_NO } }]""")
    _, plan = _plan(project, round_, body)

    relation = plan.relations[0]
    assert relation["from"].startswith("ent-")   # データ項目側から書かれていても
    assert relation["to"].startswith("itm-")     # 親 → 子に直る


def test_相手がいない参照はB012で警告(project: Paths, round_: Round) -> None:
    body = _RECORDS.replace("to: c-受注番号", "to: c-いない")
    _, plan = _plan(project, round_, body)

    assert "B012" in codes(plan.findings)


_NEXT_ROUND = """\
records:
  - concept: c-T_ORDER_HISTORY
    type: エンティティ
    name: 受注履歴
    statement: 受注履歴 T_ORDER_HISTORY は受注の変更を 1 件 1 レコードで残すこと
    attrs: { physical_name: T_ORDER_HISTORY, entity_kind: トランザクション }
    source: { anchor: s1-t1 }
    refs: [{ rel: has-column, to: c-受注番号 }]
"""


def test_前のラウンドで確立したconceptも関係の相手になる(
        project: Paths, round_: Round) -> None:
    """**更新の無い資料は前のラウンドの整理結果を指し続ける**（正常な状態）。

    `item_of` はこのラウンドのレコードから作った辞書なので、これだけを見ていた
    あいだ、前のラウンドの concept を指す関係は `B012` の warn 1 行で落ちていた
    ―― エラーではないのでトレースの穴として静かに残る。ラウンドが増えるほど
    増える壊れ方である。
    """
    spec, first = _plan(project, round_)
    build.apply(spec, first)
    spec_module.save_in_place(spec)
    concepts_module.save(project, first.concepts)

    # 次のラウンドで**新しい表**が起き、列として前のラウンドの項目を指す。
    second_round = project.round("2026-11-14")
    organized(second_round, "資料/b.xlsx/受注履歴.yml", _NEXT_ROUND)
    spec2, _ = spec_module.load(project)
    result, _ = organized_module.load(second_round)
    known, _ = concepts_module.load(project)
    plan = build.plan(spec2, result, known, second_round.name)

    assert "B012" not in codes(plan.findings)
    assert [(r["from"], r["to"]) for r in plan.relations] == [
        (plan.item_of["c-T_ORDER_HISTORY"], first.item_of["c-受注番号"])]


def test_台帳が指すアイテムが正本に無ければB012のまま(
        project: Paths, round_: Round) -> None:
    """台帳と正本がずれている（アイテムを消した）なら、相手が無いのと同じである。"""
    spec, first = _plan(project, round_)
    concepts_module.save(project, first.concepts)     # 台帳だけ書いて正本は書かない

    second_round = project.round("2026-11-14")
    organized(second_round, "資料/b.xlsx/受注履歴.yml", _NEXT_ROUND)
    spec2, _ = spec_module.load(project)
    result, _ = organized_module.load(second_round)
    known, _ = concepts_module.load(project)
    plan = build.plan(spec2, result, known, second_round.name)

    assert "B012" in codes(plan.findings)
    assert plan.relations == []


# ── known_gaps の引き継ぎ ──────────────────────────────────────
_GAP_RECORDS = """\
records:
  - concept: c-cst-規模
    type: 制約・前提
    name: 品質保証部のチャンク数の月次推移
    statement: チャンク数は 4 月から 9 月にかけて単調に増えること
    attrs: { category: 業務 }
    source: { anchor: s4-t1 }
    known_gaps:
      constrains:
        reason: 規模の想定で、縛る先の列がそもそも無い
        at: 2026-08-16
"""


def test_整理層のknown_gapsを正本へ引き継ぐ(project: Paths, round_: Round) -> None:
    """**正本側で書いたものと同じ扱いになる。** `check` では `W032` として出る。

    分担しているとき配る側は `build` を禁じるので、正本の欄しか無いあいだ整理層は
    「調べたうえで相手がいない」を宣言できなかった。
    """
    from arp4 import validate as validate_module

    _, plan = _plan(project, round_, _GAP_RECORDS)
    item = plan.created[0]

    assert item["known_gaps"] == {"constrains": {
        "reason": "規模の想定で、縛る先の列がそもそも無い", "at": "2026-08-16"}}

    # 正本に入れば `W031`（相手が 0 本）ではなく `W032`（承知している欠落）になる。
    spec, _ = spec_module.load(project)
    build.apply(spec, plan)
    said = [f for f in validate_module.validate(spec) if f.target.endswith("規模")
            or "constrains" in f.message]
    assert [f.code for f in said if f.code in ("W031", "W032")] == ["W032"]
    assert "known_gaps で承知している" in [f for f in said
                                          if f.code == "W032"][0].message


def test_正本の宣言は上書きしない(project: Paths, round_: Round) -> None:
    """**人が凍結後に書いた理由を、整理結果の側から書き換えない**（重複させない）。

    :func:`arp4.auto.declare_gaps` の「人が既に宣言している（上書きしない）」と
    同じ規律である。
    """
    spec, first = _plan(project, round_, _GAP_RECORDS)
    build.apply(spec, first)
    spec.items[0]["known_gaps"]["constrains"]["reason"] = "先方へ依頼済み（人が追記）"
    spec_module.save_in_place(spec)

    spec2, second = _plan(project, round_, _GAP_RECORDS)
    item = spec2.by_id[first.created[0]["id"]]

    assert second.metrics["created"] == 0
    assert item["known_gaps"]["constrains"]["reason"] == "先方へ依頼済み（人が追記）"


def test_同じconceptで理由が食い違えばB027(project: Paths, round_: Round) -> None:
    """どちらが正かは意味の判断なので機械が決めない（`B022` / `B024` と同じ扱い）。"""
    body = _GAP_RECORDS + """\
  - concept: c-cst-規模
    source: { anchor: s4-t1 }
    known_gaps:
      constrains:
        reason: 別の理由
"""
    _, plan = _plan(project, round_, body)

    assert "B027" in codes(plan.findings)
    assert plan.created[0]["known_gaps"]["constrains"]["reason"] == \
        "規模の想定で、縛る先の列がそもそも無い"


def test_受け皿の無い属性は捨てて数える(project: Paths, round_: Round) -> None:
    body = _RECORDS.replace("attrs: { data_type: 文字列, length: 10 }",
                            "attrs: { data_type: 文字列, 桁: 10 }")
    _, plan = _plan(project, round_, body)

    assert "B021" in codes(plan.findings)


def test_統合で属性が食い違えば数える(project: Paths, round_: Round) -> None:
    body = _RECORDS + """\
  - concept: c-受注番号
    type: データ項目
    name: 受注番号
    statement: 受注番号は文字列型（12 桁）
    attrs: { data_type: 文字列, length: 12 }
    source: { anchor: s1-t1 }
"""
    _, plan = _plan(project, round_, body)
    assert "B022" in codes(plan.findings)
    # 本文も食い違っている。**文字数で選んだことを黙っていない。**
    assert "B023" in codes(plan.findings)


_TWO_SIDES = """\
records:
  - concept: c-ORDER_NO
    type: データ項目
    name: 受注番号
    statement: 受注番号は文字列型（10 桁）の項目であること
    attrs: { data_type: 文字列, length: 10 }
    source: { anchor: s1-t1 }
    refs:
      - { rel: has-column, to: c-T_ORDER, attrs: { physical_name: ORDER_NO } }
  - concept: c-T_ORDER
    type: エンティティ
    name: 受注
    statement: 受注テーブル T_ORDER は受注 1 件を 1 レコードで保持すること
    attrs: { physical_name: T_ORDER }
    source: { anchor: s1-t1 }
    refs:
      - { rel: has-column, to: c-ORDER_NO,
          attrs: { physical_name: ORDER_NO, pk: true } }
"""


def test_同じ関係を別々の資料が張っても属性を落とさない(
        project: Paths, round_: Round) -> None:
    """項目一覧（PK 列なし）を先に読んでも、**テーブル定義書の pk が消えない。**"""
    _, plan = _plan(project, round_, _TWO_SIDES)

    assert len(plan.relations) == 1
    assert plan.relations[0]["pk"] is True
    assert "B024" not in codes(plan.findings)


def test_関係の属性が食い違えばB024(project: Paths, round_: Round) -> None:
    body = _TWO_SIDES.replace(
        "attrs: { physical_name: ORDER_NO } }",
        "attrs: { physical_name: ORDER_CD } }")
    _, plan = _plan(project, round_, body)

    assert "B024" in codes(plan.findings)


_REFERENCE = _RECORDS + """\
  - concept: c-T_ORDER
    source: { anchor: s1-t1 }
    refs: [{ rel: has-column, to: c-受注番号, attrs: { not_null: true } }]
"""


def test_参照だけのレコードは本文を上書きしない(project: Paths, round_: Round) -> None:
    """**B023 が構造的に量産されるのを止める**（同じ対象が複数シートに出るため）。"""
    _, plan = _plan(project, round_, _REFERENCE)

    entity = [i for i in plan.created if i["type"] == "entity"][0]
    assert entity["statement"].startswith("受注テーブル T_ORDER は")
    assert "B023" not in codes(plan.findings) and "B020" not in codes(plan.findings)
    # 属性はマージされ、**アンカーは出典として残る**（本物の出典だから）。
    assert plan.relations[0]["not_null"] is True and plan.relations[0]["pk"] is True
    assert len(entity["source"]) == 2


def test_完全なレコードがどこにも無ければB014(project: Paths, round_: Round) -> None:
    """凍結ゲート（G013）を通していれば起きないが、``--force`` の保険として要る。"""
    _, plan = _plan(project, round_, """\
records:
  - concept: c-T_ORDER
    source: { anchor: s1-t1 }
""")
    assert "B014" in codes(plan.findings)
    assert plan.created == []


_CRUD = """\
records:
  - concept: c-受注登録
    type: モジュール
    name: 受注登録サービス
    statement: 受注登録サービスは受注を登録すること
    attrs: { module_kind: サービス }
    source: { anchor: s1-t1 }
    refs: [{ rel: accesses, to: c-T_ORDER, attrs: { crud: [C] } }]
  - concept: c-受注登録
    source: { anchor: s2-t1 }
    refs: [{ rel: accesses, to: c-T_ORDER, attrs: { crud: [R, C] } }]
  - concept: c-T_ORDER
    type: エンティティ
    name: 受注
    statement: 受注テーブル T_ORDER は受注 1 件を保持すること
    attrs: { physical_name: T_ORDER }
    source: { anchor: s1-t1 }
"""


def test_複数値の属性は和集合になる(project: Paths, round_: Round) -> None:
    """先勝ちだった頃は、モジュール一覧の ``[C]`` が処理仕様書の ``[C, R]`` を
    押しのけ、**CRUD 図から R が消えていた。**"""
    _, plan = _plan(project, round_, _CRUD)

    assert plan.relations[0]["crud"] == ["C", "R"]     # 並びは enum の宣言順
    assert "B024" not in codes(plan.findings)          # 落ちるものが無いので黙る


_REFINES = """\
records:
  - concept: c-BR
    type: 業務要件
    name: オーダーの登録
    statement: オーダーを登録できること
    source: { anchor: s1-t1 }
    refs: [{ rel: refines, to: c-FR }]
  - concept: c-FR
    type: 機能要件
    name: 受注登録
    statement: 受注を登録できること
    source: { anchor: s1-t1 }
"""


def test_同じ種別どうしの関係は向きを直せないと言う(project: Paths, round_: Round) -> None:
    """``refines`` は from と to が同じ種別なので、**書いた向きがそのまま残る。**

    「向きは機械が直す」と読んで逆に書くと、トレース表から丸ごと落ちる。
    """
    _, plan = _plan(project, round_, _REFINES)
    unsure = [f for f in plan.findings if f.code == "B026"]

    assert len(unsure) == 1                     # 1 本ずつではなく組み合わせごとに 1 件
    assert "向きは機械が直せません" in unsure[0].message
    assert plan.relations[0]["from"] == plan.item_of["c-BR"]   # 書いたまま入る


_REFINES_MULTI = """\
records:
  - concept: c-BR
    type: 業務要件
    name: オーダーの登録
    statement: オーダーを登録できること
    source: { anchor: s1-t1 }
    refs:
      - { rel: refines, to: c-FR1 }
      - { rel: refines, to: c-FR2 }
  - concept: c-FR1
    type: 機能要件
    name: 受注登録
    statement: 受注を登録できること
    source: { anchor: s1-t1 }
  - concept: c-FR2
    type: 機能要件
    name: 受注取消
    statement: 受注を取り消せること
    source: { anchor: s1-t1 }
"""


def test_B026の場所は重複を畳んで出す(project: Paths, round_: Round) -> None:
    """1 レコードが同じ組み合わせを 2 本持っても、**開ける場所は 1 つ**である。

    畳まずに出すと ``a.yml[0]、a.yml[0]`` と同じ場所が並び、**何本あるかの
    手がかりにも、どこを開くかの手がかりにもならない。** 本数は本数で言う。
    """
    _, plan = _plan(project, round_, _REFINES_MULTI)
    unsure = [f for f in plan.findings if f.code == "B026"]

    assert len(unsure) == 1
    assert "2 本 / 1 レコード" in unsure[0].message
    places = unsure[0].message.split("確かめてください: ")[1]
    assert places == places.split("、")[0]      # 同じ場所を並べない


def test_向きを直せる関係ではB026を出さない(project: Paths, round_: Round) -> None:
    _, plan = _plan(project, round_, _TWO_SIDES)

    assert "B026" not in codes(plan.findings)


_CALLS_FROM_IMPORT = """\
records:
  - concept: c-mod-src.a
    type: モジュール
    name: a
    statement: a は b を取り込んで呼ぶこと
    source: { anchor: i1 }
    refs:
      - { rel: calls, to: c-mod-src.b }
  - concept: c-mod-src.b
    type: モジュール
    name: b
    statement: b は呼ばれる側であること
    source: { anchor: i1 }
"""


def test_取り込み由来のcallsはB026にしない(project: Paths, round_: Round) -> None:
    """出典が取り込みの塊（``i1``）なら、向きは資料に書いてある（0-3）。

    r001 では 232 本が一括警告され、実質読み飛ばされる量だった ―― 確認済みの
    ものに警告を出すと、本当に確かめてほしい ``refines`` が山に埋もれる。
    """
    _, plan = _plan(project, round_, _CALLS_FROM_IMPORT)

    assert "B026" not in codes(plan.findings)
    assert len(plan.relations) == 1              # 関係そのものは入る


def test_シート由来のcallsはB026のまま(project: Paths, round_: Round) -> None:
    """向きの事実が出典に無いもの（シートの呼出関係表）は従来どおり申告する。"""
    _, plan = _plan(project, round_, _CALLS_FROM_IMPORT.replace(
        "source: { anchor: i1 }", "source: { anchor: s1-t1 }"))

    assert "B026" in codes(plan.findings)


def test_適用は冪等(project: Paths, round_: Round) -> None:
    spec, plan = _plan(project, round_)
    build.apply(spec, plan)
    spec_module.save_in_place(spec)

    spec2, findings = spec_module.load(project)
    result, _ = organized_module.load(round_)
    known, _ = concepts_module.load(project)
    again = build.plan(spec2, result, known, round_.name)

    assert again.empty


def test_overriddenは再構築で守られる(project: Paths, round_: Round) -> None:
    spec, plan = _plan(project, round_)
    build.apply(spec, plan)
    entity = [i for i in spec.items if i["type"] == "entity"][0]
    entity["physical_name"] = "T_ORDERS"
    entity["overridden"] = {"physical_name": {"was": "T_ORDER", "reason": "実装に合わせた"}}
    spec_module.save_in_place(spec)

    spec2, _ = spec_module.load(project)
    result, _ = organized_module.load(round_)
    known, _ = concepts_module.load(project)
    again = build.plan(spec2, result, known, round_.name)

    assert again.protected                      # 黙って守らない（数えて報告する）
    assert spec2.by_id[entity["id"]]["physical_name"] == "T_ORDERS"


_RULE = """\
  - concept: c-保持期間
    type: 業務ルール
    name: 受注データの保持期間
    statement: 受注データは 13 か月でアーカイブすること
    attrs: { rule_kind: processing }
    source: { anchor: s1-t1 }
"""

_CONTRADICTION = """\
contradictions:
  - subject: c-保持期間
    positions:
      - { statement: 5 年間保存する }
      - { statement: 13 か月でアーカイブする }
"""


def _issues(project: Paths, round_: Round, concepts_body: str, body: str):
    organized(round_, "_concepts.yml", concepts_body)
    spec, plan = _plan(project, round_, body)
    result, _ = organized_module.load(round_)
    return spec, plan, build.issues(spec, result, {}, plan)


def test_矛盾は課題になり争点へdisputesを張る(project: Paths, round_: Round) -> None:
    """張らないと ``disputes`` の多重度を必ず破り、E027 が積み上がっていた。"""
    _, plan, found = _issues(project, round_, _CONTRADICTION, _RECORDS + _RULE)

    assert len(found) == 1
    assert found.items[0]["type"] == "open-issue"
    assert [(r["type"], r["to"]) for r in found.relations] == [
        ("disputes", plan.item_of["c-保持期間"])]


def test_両論がそれぞれ別のアイテムを指せば全部に張る(
        project: Paths, round_: Round) -> None:
    body = _RECORDS + _RULE + """\
  - concept: c-保持期間-運用
    type: 業務ルール
    name: 受注データの退避
    statement: 受注データは 5 年間保存すること
    attrs: { rule_kind: processing }
    source: { anchor: s1-t1 }
"""
    proposal = _CONTRADICTION.replace(
        "      - { statement: 5 年間保存する }",
        "      - { statement: 5 年間保存する, concept: c-保持期間-運用 }").replace(
        "      - { statement: 13 か月でアーカイブする }",
        "      - { statement: 13 か月でアーカイブする, concept: c-保持期間 }")
    _, plan, found = _issues(project, round_, proposal, body)

    assert sorted(r["to"] for r in found.relations) == sorted(
        {plan.item_of["c-保持期間"], plan.item_of["c-保持期間-運用"]})


def test_同じsubjectの2件目は畳んで両論を足す(project: Paths, round_: Round) -> None:
    """**黙って捨てない。** 課題 ID は ``subject`` から決まるので 1 subject : 1 課題
    だが、2 件目を ``continue`` していたころは error も warn も出ないまま消えていた
    ―― 実測（sales-corpus・13 ロット / 争点 59 件）で 6 組が同一 subject に集まり、
    整理②で気づかなければ 6 件以上が正本に入らなかった。
    """
    proposal = _CONTRADICTION + """\
  - subject: c-保持期間
    name: 保持期間の起算日が資料間で食い違う
    positions:
      - { statement: 受注日を起算日にする }
      - { statement: 締め日を起算日にする }
"""
    _, _, found = _issues(project, round_, proposal, _RECORDS + _RULE)

    assert len(found) == 1                       # 課題は 1 件のまま
    positions = found.items[0]["positions"]
    assert "受注日を起算日にする" in positions   # ★ 2 件目の両論が残っている
    assert "5 年間保存する" in positions
    assert "\n" not in positions                 # 課題管理表の 1 升に出る
    assert "B017" in codes(found.findings)


def test_課題の仕様文はnameから作る(project: Paths, round_: Round) -> None:
    """定型文（``<label> について、資料が n 通りのことを言っている``）は、争点が
    違っても label しか変わらない ―― 実測（sales-corpus・r001）で課題 28 件のうち
    9 件が ``W044``（仕様文がほぼ同一）で鳴り、本物の二重登録が埋もれた。
    """
    proposal = _CONTRADICTION.replace(
        "  - subject: c-保持期間",
        "  - subject: c-保持期間\n    name: 保持期間が資料間で食い違う")
    _, _, found = _issues(project, round_, proposal, _RECORDS + _RULE)

    assert found.items[0]["statement"].startswith("保持期間が資料間で食い違う")


def test_矛盾からの起票は決定記録に残る(project: Paths, round_: Round) -> None:
    """Excel だけのラウンドは ``draft`` を通らないので、build が残さないと
    ``decisions.yml`` がそもそも作られず ``out/決定記録.md`` が出ない。"""
    _, _, found = _issues(project, round_, _CONTRADICTION, _RECORDS + _RULE)

    assert [e["by"] for e in found.logged] == ["build"]
    assert found.items[0]["id"] in found.logged[0]["what"]


def test_決定の根拠は出典アンカーの形にする(project: Paths, round_: Round) -> None:
    """``positions[].source`` は ``{file, anchor}`` である ―― そのまま文字列に
    すると ``{'file': …}`` が決定記録の「根拠」の欄に出る。"""
    proposal = _CONTRADICTION.replace(
        "      - { statement: 5 年間保存する }",
        "      - { statement: 5 年間保存する,"
        " source: { file: 資料/a.xlsx/受注, anchor: s1-t1 } }")
    _, _, found = _issues(project, round_, proposal, _RECORDS + _RULE)

    assert found.logged[0]["basis"] == ["資料/a.xlsx/受注.md#s1-t1"]


def test_subjectが課題なら課題を起こさない(project: Paths, round_: Round) -> None:
    """``iss-<digest(subject)>`` が既存の課題と衝突して E002 / E012 になっていた。"""
    body = _RECORDS + """\
  - concept: c-課題1
    type: 課題
    name: 消費税の計算単位が未合意
    statement: 消費税の計算単位が資料間で決まっていないこと
    source: { anchor: s1-t1 }
"""
    _, _, found = _issues(
        project, round_, _CONTRADICTION.replace("c-保持期間", "c-課題1"), body)

    assert not found.items
    assert "B015" in codes(found.findings)


def test_争点が指せない種別ならB016(project: Paths, round_: Round) -> None:
    """用語は disputes の相手になれない。**黙って多重度違反にしない。**

    見張っているのは ``B016`` そのもの（＝ **語彙が許していない相手へは張らず、
    張らなかったことを言う**）であって、どの種別が相手になれないかではない ――
    相手の集合は資料の実例が出るたびに広がる。**試しに使う種別は、広がった
    ときに取り替える。**

    **エンティティでは試せない。** データ項目・エンティティ・コード値は
    ``disputes`` の相手に足した ―― 争点そのものがデータ項目であることは普通にある
    （実測で「消費税の計算単位」の証拠が 2 つの「消費税額」だった）。

    **利用者・ロールでも試せなくなった**（jp-sier-std 3.16.0）。体制図の凡例は
    実線（指揮命令）と破線（委託）しか描き分けていないのに、実線 7 本のうち
    点検依頼・提供・月次締め の 3 本はどちらでもない ―― 争点が組織どうしの線に
    なるので `actor` を相手に足した。用語に取り替えてあるが、用語の定義の
    食い違いを課題にしたい実例が出たら、また別の種別へ取り替えることになる。
    """
    body = _RECORDS + """  - concept: c-trm-受注残
    type: 用語
    name: 受注残
    statement: 受注残は受注のうち出荷の済んでいないものを指すこと
    source: { anchor: s1-t1 }
"""
    _, _, found = _issues(
        project, round_, _CONTRADICTION.replace("c-保持期間", "c-trm-受注残"), body)

    assert len(found) == 1 and not found.relations
    assert "B016" in codes(found.findings)


def test_データ項目も争点になれる(project: Paths, round_: Round) -> None:
    """**争点そのものがデータ項目であることは普通にある。**

    相手に無かったころは業務ルール・制約へ迂回するしかなく、課題管理表から
    証拠のデータ項目へ辿れなかった（実測・sales-corpus「消費税の計算単位」）。
    """
    _, plan, found = _issues(
        project, round_, _CONTRADICTION.replace("c-保持期間", "c-受注番号"), _RECORDS)

    assert [r["to"] for r in found.relations] == [plan.item_of["c-受注番号"]]
    assert "B016" not in codes(found.findings)


def test_必須属性を埋めない(project: Paths, round_: Round) -> None:
    """``E010`` は「人が埋めよ」の正しい信号であって、機械が消すものではない。"""
    body = _RECORDS.replace("attrs: { physical_name: T_ORDER, entity_kind: トランザクション }",
                            "attrs: { entity_kind: トランザクション }")
    _, plan = _plan(project, round_, body)
    entity = [i for i in plan.created if i["type"] == "entity"][0]

    assert "physical_name" not in entity


_FLOW = """\
records:
  - concept: c-bf-受注受付
    type: 業務フロー
    name: 受注受付の流れ
    statement: 受注は与信の枠内かを判定したうえで在庫の引き当てへ進めること
    source: { anchor: s1-c1 }
    refs:
      - { rel: has-flow-step, to: c-fs-与信判定 }
      - { rel: has-flow-step, to: c-fs-在庫引き当て }
      - { rel: has-flow-step, to: c-fs-保留 }
  - concept: c-fs-与信判定
    type: 業務フローのステップ
    name: 与信の枠内？
    statement: 受注の金額が得意先の与信の枠内であるかを判定すること
    attrs: { step_kind: 判定 }
    source: { anchor: s1-g1 }
    refs:
      - { rel: leads-to, to: c-fs-在庫引き当て,
          attrs: { condition: 与信の枠内であるとき } }
      - { rel: leads-to, to: c-fs-保留, attrs: { condition: 与信の枠を超えるとき } }
  - concept: c-fs-在庫引き当て
    type: 業務フローのステップ
    name: 在庫の引き当て
    statement: 受注した品目の在庫を引き当てること
    source: { anchor: s1-g1 }
  - concept: c-fs-保留
    type: 業務フローのステップ
    name: 保留
    statement: 与信の枠を超える受注は承認を得るまで保留とすること
    source: { anchor: s1-g1 }
"""


def test_業務フローの手順と分岐が正本に入る(project: Paths, round_: Round) -> None:
    """図形の接続（`c1`）が、文章ではなく**関係**として正本へ入ること。

    `steps`（文字列）に流し込むとトレースの対象にならず、分岐条件は**どちらの枝の
    ものか**が分からなくなる。
    """
    _, plan = _plan(project, round_, _FLOW)

    steps = [i for i in plan.created if i["type"] == "flow-step"]
    assert {s["name"] for s in steps} == {"与信の枠内？", "在庫の引き当て", "保留"}

    members = [r for r in plan.relations if r["type"] == "has-flow-step"]
    assert len(members) == 3

    # **分岐条件は関係側にある**（ステップ側ではない）
    edges = [r for r in plan.relations if r["type"] == "leads-to"]
    assert len(edges) == 2
    assert {e.get("condition") for e in edges} == {
        "与信の枠内であるとき", "与信の枠を超えるとき"}


# ── 出典どうしの食い違い（決定 70） ─────────────────────────────
_APPEND = """\
records:
  - concept: c-act-営業
    type: 利用者・ロール
    name: 営業
    statement: 営業は社員に割り当てる権限ロールの 1 つであること
    attrs: { actor_kind: 利用者, description: 営業部 120 名。受注の登録を行う }
    source: { anchor: s7-t1 }
  - concept: c-act-営業
    attrs: { description: 権限マトリクスでは「与信保留の解除」が △（部長職のみ可）である }
    source: { anchor: s8-t1 }
"""


def test_補足は足し合わせて両方残す(project: Paths, round_: Round) -> None:
    """**相補的な補足は、片方を捨てると事実そのものが消える。**

    実測（sales-corpus 30 冊）で衝突した 6 件は全部これで、`5.権限マトリクス` が
    書いた △（部長職のみ可）は `4.セキュリティ方式` の「営業部 120 名。…」に
    上書きされ、**正本からも生成物からも消えた**（`arp4 publish` の権限マトリクス
    には △ が 0 件）。B022 は 6 件鳴っていたが build の出力は端末に流れて消える。
    """
    _spec, plan = _plan(project, round_, _APPEND)

    item = next(i for i in plan.created if i.get("name") == "営業")
    assert "営業部 120 名" in item["description"]
    assert "△（部長職のみ可）" in item["description"]
    # 失っていないので食い違いの警告は出さない。
    assert "B022" not in codes(plan.findings)
    assert "conflicts" not in item


def test_同じ補足を2度足さない(project: Paths, round_: Round) -> None:
    """参照だけのレコードが同文を持つことはある ―― 足すと同じ段落が 2 つ並ぶ。"""
    same = _APPEND.replace(
        "権限マトリクスでは「与信保留の解除」が △（部長職のみ可）である",
        "営業部 120 名。受注の登録を行う")
    _spec, plan = _plan(project, round_, same)

    item = next(i for i in plan.created if i.get("name") == "営業")
    assert item["description"].count("営業部 120 名") == 1


_SCALAR = """\
records:
  - concept: c-受注番号
    type: データ項目
    name: 受注番号
    statement: 受注番号は受注を識別する番号であること
    attrs: { data_type: 文字列, length: 12 }
    source: { anchor: s1-t1 }
  - concept: c-受注番号
    attrs: { data_type: 数値 }
    source: { anchor: s2-t1 }
"""


def test_足し合わせられない食い違いは捨てた値を正本に残す(
        project: Paths, round_: Round) -> None:
    """**build の warn は端末に流れて消える。**

    次に `check` を回した人には衝突があったことが見えない ―― 残せば `W045` が
    言い続け、穴の一覧にも出る。どちらが正かは意味の判断なので機械は決めない。
    """
    _spec, plan = _plan(project, round_, _SCALAR)

    item = next(i for i in plan.created if i.get("name") == "受注番号")
    assert "B022" in codes(plan.findings)          # 端末にも出す
    dropped = item["conflicts"]["data_type"]
    assert len(dropped) == 1
    assert dropped[0]["value"] == "数値"
    assert dropped[0]["source"]["anchor"] == "s2-t1"
    assert item["data_type"] == "文字列"            # 採った値は先に読んだほう


# ── 関係の注記（決定 72） ───────────────────────────────────────
_NOTE = """\
records:
  - concept: c-mod-BillingCloseBatch
    type: モジュール
    name: 請求締めバッチ
    statement: 請求締めバッチは締め対象の得意先ごとに売上を集計すること
    attrs: { module_id: MOD-001 }
    source: { anchor: s5-t2 }
    refs:
      - { rel: calls, to: c-mod-TaxCalculator, note: 請求単位の消費税額を計算する }
  - concept: c-mod-TaxCalculator
    type: モジュール
    name: 消費税計算部品
    statement: 消費税計算部品は税区分と税率から消費税額を計算すること
    attrs: { module_id: MOD-002 }
    source: { anchor: s5-t2 }
"""


def test_関係の注記が正本まで届く(project: Paths, round_: Round) -> None:
    """資料は呼出 1 本ごとに「呼び出す目的」を書いているのに、**書く場所が無くて
    11 本が丸ごと落ちていた** ―― しかもこの 1 本は課題 ISS-016（消費税の計算単位）
    の証拠そのものである。"""
    _spec, plan = _plan(project, round_, _NOTE)

    call = next(r for r in plan.relations if r["type"] == "calls")
    assert call["description"] == "請求単位の消費税額を計算する"


def test_B026の決定は関係1本ずつ積む(project: Paths, round_: Round) -> None:
    """**警告は畳んでも、決定は畳まない。**

    事後拒否権（``out/決定記録.md``）は矢印 1 本ごとに要る ―― 「どの 100 本か」が
    消えると、差し戻す対象を名指しできない。

    ここは実際に畳んでいた（→ 決定 78）。出典アンカーの集合で積んでいたため、
    実測（sales-corpus・r001）で同型の関係 111 本に対し決定が **11 件**しか
    残らず、1 シートから起こした ``leads-to`` 42 本が 1 件に化けていた ――
    :func:`arp4.build._unsure` の docstring は当時から「畳まずに積む」と
    書いており、**文書とコードが逆を言っていた。**
    """
    _, plan = _plan(project, round_, _REFINES_MULTI)

    # 警告は組み合わせごとに 1 件のまま（これは畳んでよい）。
    assert len([f for f in plan.findings if f.code == "B026"]) == 1

    said = [d for d in plan.logged if "向きのまま入れた" in d["what"]]
    assert len(said) == 2, "関係 2 本に対して決定が 2 件ありません"

    # **どの 1 本かが名指しされている。** 表示 ID は number が build のあとに
    # 振るので、この時点で読み手が引ける手掛かりは名前しかない。
    assert {d["what"] for d in said} == {
        "refines（オーダーの登録 → 受注登録）を書いた向きのまま入れた",
        "refines（オーダーの登録 → 受注取消）を書いた向きのまま入れた"}


_CONTRADICTION_EXT = """\
contradictions:
  - subject: c-保持期間
    positions:
      - { statement: 5 年間保存する,
          source: { file: 資料/運用設計.xlsx/保持方針.md, anchor: s4-t2 } }
      - { statement: 13 か月でアーカイブする,
          source: { file: 資料/基本設計書.xlsx/業務ルール, anchor: s8-t1 } }
"""


def test_矛盾の根拠は拡張子を二重にしない(project: Paths, round_: Round) -> None:
    """``positions[].source.file`` は**人が手で書く値**である。

    手順書の例が organize.md（``.md`` あり）と reconcile.md（なし）で割れており、
    実測（sales-corpus・r001）で矛盾由来の決定 13 行すべてが
    ``…/3.機能要件一覧.md.md#s7-t1`` になって、**根拠パスが 1 本も辿れなかった。**
    規約は「拡張子を書かない」に揃えたが、手で書く値である以上ここでも受ける。
    """
    _, _, found = _issues(project, round_, _CONTRADICTION_EXT, _RECORDS + _RULE)

    basis = [b for d in found.logged for b in d["basis"]]
    assert basis, "矛盾からの起票が根拠を残していません"
    assert not [b for b in basis if ".md.md" in b], basis
    # 付いていても・付いていなくても、行き先は同じ 1 つの形になる。
    assert set(basis) == {"資料/運用設計.xlsx/保持方針.md#s4-t2",
                          "資料/基本設計書.xlsx/業務ルール.md#s8-t1"}
