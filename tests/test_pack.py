"""標準パック（jp-sier-std）の語彙が持つべき不変条件と、3.11 系で足したもの。

**受け皿の無い語彙は、資料が届いていても永久に 0 件で通る。** ここはその 1 点を
いろいろな角度から見張る ―― 種別が無い（企画レイヤ）・写像が無い（`fact_types`）・
関係が無い（ER の関連）・列が無い（文書定義）。どれも error にはならず、
**生成物が空になるだけ**なので、成果物からは「資料に書いていない」と見分けが付かない。

## 企画レイヤ（3.11.0）―― 「なぜ作るのか」に受け皿が無かった

実測（r001 のレビュー）で「受注受付の手入力の対象を 6 割減らす」が正本にも
生成物にも残らなかった。語彙に無いものは整理層が書けず、行き先は
``out_of_scope: 対象外``（holes.py が拾わないので**痕跡が残らない**）か
``description`` への散文（設計書に出ない）しか無い。

同じ穴のもう 1 つの顔が**重複**である ―― sales-corpus 30 冊で制約 102 件のうち
6 組が二重登録で、6 組とも「要件定義書とプロジェクト計画書の両方に書かれ、
concept 名が別々に付いた」ものだった（→ :func:`validate._near_duplicates`）。
**受け皿の無い情報は、消えるか、近い種別へ滲み出して重複になるかのどちらか。**

ここで見張るのは章立てや列の並びそのものではなく、**その並びが持っている理由**
である ―― 目標は現状値と対でなければ測れない、目標と要件は片側だけでは意味を
持たない、計画書を持たない案件が 1 冊落とすことにならない。
"""

from __future__ import annotations

from typing import Any

import pytest

from arp4 import metamodel as mm
from arp4 import pack

#: 3.11.0 で足した「整理層が資料から書ける」種別（fact 種別 → アイテム種別）。
_PLANNING = {
    "プロジェクト概要": "project-overview",
    "事業目標": "business-goal",
    "現状課題": "current-issue",
    "リスク": "risk",
    "マイルストーン": "milestone",
}


@pytest.fixture(scope="module")
def documents() -> dict[str, dict[str, Any]]:
    chain, findings = pack.resolve_chain("jp-sier-std")
    assert not [f for f in findings if f.level == "error"]
    return {str(d.get("name")): d for d in pack.documents(chain)}


def _section(document: dict[str, Any], heading: str) -> dict[str, Any]:
    for section in document.get("sections") or []:
        if section.get("heading") == heading:
            return section
    raise AssertionError(f"章がありません: {heading}")


# ── 語彙の不変条件 ──────────────────────────────────────────────
def test_標準パックの全種別が整理結果から書ける(model: mm.Metamodel) -> None:
    """**入口の無い種別は、資料が届いても永久に 0 件のままになる。**

    `decision` / `test-run` が 3.11.0 までそうだった ―― 属性は `decided_by`
    （会議体）・`tester`・`executed_on` と**資料を写す欄しか無い**のに写像
    （`fact_types`）が無く、正本を人が手で書く以外の入口が無かった。手順書は
    「正本側で人が起こす」と書いていたが、実測では議事録の決定事項が**1 件も
    入らなかった** ―― 「人が起こす」は、誰も起こさないので何も残らないという
    形で失敗する（→ 決定 77）。

    しかも error にはならない。テスト結果報告書は「必要な語彙: `test-run`」と
    自己申告し、読み手は資料を探しに行くが、**見つけて parse に渡しても整理層が
    書けない。** 供給経路の無い自己申告は、穴の申告として機能しない。

    **消費側パックでは orphan を許す**（`arp4 model` の
    `[整理結果からは書けない種別]` はその検出口として残してある）。標準パックが
    配る語彙には入口を必ず用意する、という約束をここで固定する。
    """
    orphans = sorted(name for name, definition in model.item_types.items()
                     if not definition.get("fact_types"))

    assert orphans == []


def test_計画書の語彙は整理層から書ける(model: mm.Metamodel) -> None:
    """**写像（fact_types）を足し忘れた種別は、永遠に 0 件で通る。**

    メタモデルの検査（M021）は宣言の妥当性しか見ないので、「種別は足したが
    整理結果からは書けない」状態はエラーにならない ―― 生成物が空でも
    「資料に書いていない」と見分けが付かない。
    """
    for fact_type, item_type in _PLANNING.items():
        mapped = model.for_fact(fact_type)
        assert mapped is not None, fact_type
        assert mapped[0] == item_type
        assert model.layer_of(item_type) == "企画"


def test_企画は要件の手前の工程に置く(model: mm.Metamodel) -> None:
    """**要件定義に混ぜると、業務要件という段が潰れる。**

    `realizes`（設計要素 → 要件）の相手にできてしまうと、画面が事業目標を
    直接「実現する」ことになる。目標に届く道は業務要件を経由する
    `contributes-to` の 1 本だけである。
    """
    assert list(model.layers)[:2] == ["企画", "要件定義"]
    assert "business-goal" not in (model.relation_types["realizes"].get("to") or [])
    assert model.relation_types["contributes-to"]["to"] == ["business-goal"]


def test_現状課題と課題は属性名から見分けられる(model: mm.Metamodel) -> None:
    """段が違う ―― 現状課題は「システム化の理由」、課題は「決着していない争点」。

    同じ `issue_id` にすると、整理層はどちらへ書くかを名前から判断できず、
    資料の「課題一覧」が両方に散る。
    """
    assert "problem_id" in model.item_types["current-issue"]["attributes"]
    assert "issue_id" not in model.item_types["current-issue"]["attributes"]
    assert model.layer_of("current-issue") == "企画"
    assert model.layer_of("open-issue") == "管理"


def test_リスクは課題と別の種別にする(model: mm.Metamodel) -> None:
    """課題は「もう起きている」、リスクは「まだ起きていない」。

    混ぜると課題管理表に未発生の想定が混ざり、`due` の意味が変わる
    （課題は回答期限、リスクは再評価の時期）。
    """
    assert "リスク" in model.fact_types and "課題" in model.fact_types
    assert model.for_fact("リスク")[0] != model.for_fact("課題")[0]
    # 相手を書けないリスクは「気を付ける」以上の意味を持てない。
    assert model.relation_types["threatens"]["from"] == ["risk"]
    assert "milestone" in model.relation_types["threatens"]["to"]


def test_決定は何を定めたかを指せる(model: mm.Metamodel) -> None:
    """**`resolves` だけでは、議事録から起こした決定の大半が孤児になる。**

    登録済みの課題に紐づく決定は少数で、残りは「決めた」としか言っていない
    アイテムとして課題管理表に並ぶ ―― あとから来た人は何が変わったのかを
    本文から推測するしかない。

    相手は `disputes`（課題 → 争点）と**同じ集合**にしてある。争点になれるものと
    決定が確定させられるものが違うと、課題 → 決定 の流れが途中で相手を変える。
    """
    establishes = model.relation_types["establishes"]
    assert establishes["from"] == ["decision"]
    assert set(establishes["to"]) == set(model.relation_types["disputes"]["to"])


def test_エンティティ間の関連は多重度と外部キーを持つ(model: mm.Metamodel) -> None:
    """**ER 図が 1 本も描けなかった。**

    entity → entity を張れる関係は `refines`（同一種別の階層化）しか無く、
    多重度も外部キーも持てない ―― 外部キー欄は `has-column` の `note` へ
    逃げ、設計書のどの列にも出ていなかった。
    """
    references = model.relation_types["references"]
    assert references["from"] == ["entity"] and references["to"] == ["entity"]
    assert set(references["attributes"]) >= {"cardinality", "fk_columns",
                                             "required_flag"}
    # **bool にしない** ―― `false` は升に空文字で出るので、「任意参照では
    # ない」と「資料が何も言っていない」が同じ見た目になる（決定 71）。
    assert references["attributes"]["required_flag"]["kind"] == "enum"
    assert not references.get("same_type_only")     # 階層化ではなく関連


def test_ロールと目標は階層を張れる(model: mm.Metamodel) -> None:
    """権限マトリクスは「営業部 → 営業担当」の階層を前提に読む。

    受け皿が無いあいだ、階層は `description` の散文に入るしかなかった
    （決定 75 でサブシステムが同じ形で消えたのと同型）。
    """
    refines = model.relation_types["refines"]
    for side in ("from", "to"):
        assert "actor" in refines[side] and "business-goal" in refines[side]
    assert refines.get("same_type_only")            # 階層化であることは変えない


# ── 文書 ────────────────────────────────────────────────────────
def test_効果目標は現状値と対で出る(documents: dict[str, Any]) -> None:
    """**「6 割減」は分母が並んでいて初めて測れる。**

    目標値だけの列は、読み手には標語と区別が付かない。
    """
    columns = _section(documents["project-charter"], "事業目標")["columns"]
    assert columns.index("baseline") < columns.index("metric")


def test_目標と要件は対応と漏れを対で出す(documents: dict[str, Any]) -> None:
    """トレースは埋まっている部分より**空いている部分に価値がある**。

    要件に支えられていない目標は「掲げただけ」で、それは対の章でしか見えない。
    """
    matrix = documents["traceability-matrix"]
    linked = _section(matrix, "事業目標 → 要件")
    gap = _section(matrix, "要件の無い事業目標（要件化漏れ）")

    assert linked["relation"] == gap["relation"] == "contributes-to"
    assert linked["type"] == gap["type"] == "business-goal"
    assert gap["gap"] is True and not linked.get("gap")


def test_計画書は必須文書にしない(documents: dict[str, Any]) -> None:
    """**空で出ることが「計画書を取り込んでいない」の可視化**である（決定 55）。

    必須にすると、資産がコードだけの案件が恒久的に 1 冊落とす ―― 落ちた冊子は
    誰にも見えないが、空の冊子は「必要な語彙: 事業目標・現状課題…」と自分で言う。
    """
    chain, _ = pack.resolve_chain("jp-sier-std")
    required = pack.rules(chain).get("require_documents") or []

    assert "project-charter" in documents
    assert "project-charter" not in required


# ── オンライン処理の処理フロー（3.13.0）──────────────────────────
def test_オンライン処理の処理フローに受け皿がある(
        model: mm.Metamodel, documents: dict[str, Any]) -> None:
    """**バッチにしかステップの受け皿が無かった。**

    ``has-step`` は batch → batch-step、``has-flow-step`` は
    business-flow → flow-step に縛られており、処理仕様書の「3.処理フロー」を
    構造として置く場所が無い ―― 図形 13 個・接続 13 本がパース結果に取れている
    のに、である。実測（sales-corpus・r001／処理仕様書 4 冊）で 4 ロットが独立に
    散文へ逃がし、詳細設計書のステップ表はバッチ 6 件だけで出た。
    """
    assert "処理ステップ" in model.item_types["process-step"]["fact_types"]
    assert model.for_fact("処理ステップ")[0] == "process-step"
    assert model.layer_of("process-step") == "詳細設計"

    holder = model.relation_types["has-process-step"]
    assert holder["from"] == ["module", "method"] and holder["to"] == ["process-step"]

    # **分岐が表せること**が batch-step を流用しなかった理由である。
    arrow = model.relation_types["proceeds-to"]
    assert arrow["from"] == arrow["to"] == ["process-step"]
    assert "condition" in arrow["attributes"]

    # 受け皿があっても、出す章が無ければ生成物は空のままになる。
    assert _section(documents["detail-design"], "処理フロー")["relation"] \
        == "has-process-step"
    assert _section(documents["detail-design"], "処理フローの分岐")["relation"] \
        == "proceeds-to"


def test_処理フローは業務フローと別の種別に置く(model: mm.Metamodel) -> None:
    """**要件定義書へ漏らさない。**

    要件定義書の「業務フローの手順」「業務フローの流れ」は種別で絞らずに
    ``has-flow-step`` / ``leads-to`` を全件出す（関係の章は ``where`` を
    適用しない）―― `flow-step` に相乗りすると、詳細設計のステップと分岐が
    要件定義書に丸ごと並ぶ。
    """
    assert model.relation_types["has-flow-step"]["from"] == ["business-flow"]
    assert model.relation_types["leads-to"]["from"] == ["flow-step"]
    assert "process-step" not in model.relation_types["leads-to"]["to"]
