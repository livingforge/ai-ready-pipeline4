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


# ── プログラム設計（3.17.0）──────────────────────────────────────
#: 揺れを止めるために足した種別（fact 種別 → アイテム種別）。
_PROGRAM = {
    "実装規約": "implementation-standard",
    "引数": "parameter",
    "問い合わせ": "query",
    "エンドポイント": "endpoint",
}


def test_プログラム設計の語彙は整理層から書ける(model: mm.Metamodel) -> None:
    """**入口の無い種別は永久に 0 件で通る**（`decision` / `test-run` と同じ形）。

    この段は「資料から起こす」段ではなく「決める」段だが、それでも写像は要る
    ―― 資料がコードのラウンドでは引数も問い合わせも全部書いてあり、そこを
    塞ぐと**実装が先にある案件**（`examples/kotonoha`）が何も書けなくなる。
    """
    for fact_type, item_type in _PROGRAM.items():
        mapped = model.for_fact(fact_type)
        assert mapped is not None, fact_type
        assert mapped[0] == item_type
        assert model.layer_of(item_type) == "プログラム設計"


def test_プログラム設計は詳細設計とテストの間に置く(model: mm.Metamodel) -> None:
    """**工程の並びは publish の章立てと「持ち主」の判定に効く。**

    同じアイテムが 2 冊に出るとき、表示 ID を持つのは**工程が先の設計書**である
    （`publish._owner`）―― プログラム設計をテストより後ろに置くと、実装の座標を
    テスト仕様書が先に名乗る。
    """
    layers = list(model.layers)
    assert layers.index("詳細設計") < layers.index("プログラム設計")
    assert layers.index("プログラム設計") < layers.index("テスト")


def test_呼び出しの境界が語彙で決まる(model: mm.Metamodel) -> None:
    """**ここが空白だと、生成のたびに違うコードが出る。**

    詳細設計の語彙が持っていたのは `signature` / `returns` / `raises` の
    3 つの文字列だけで、`signature` は「資料にそう書いてあった」ものである
    ―― 資料が Excel の詳細設計書なら 1 件も入らない。呼ぶ名前・引数・引数の型・
    戻り値の型が語彙にあって初めて、隣のモジュールと噛み合う。
    """
    assert "method_name" in model.item_types["method"]["attributes"]
    assert "source_path" in model.item_types["module"]["attributes"]

    # 並びが仕様である（呼び出しの順）。
    assert model.relation_types["has-parameter"]["ordered"] is True
    assert model.relation_types["has-parameter"]["to"] == ["parameter"]

    # **型は正本の語彙で指す** ―― 実装型を文字列で書くと 4 言語ぶん揺れる。
    for relation in ("typed-as", "returns-type"):
        assert model.relation_types[relation]["to"] == [
            "data-item", "entity", "code-master"]
    assert model.relation_types["typed-as"]["from"] == ["parameter"]
    assert model.relation_types["returns-type"]["from"] == ["method"]

    # いつ出すかは出す側との組にしか無い（同じ `E-0007` を 3 つが別の理由で出す）。
    assert "condition" in model.relation_types["raises"]["attributes"]


def test_実装言語はモジュールごとに持つ(model: mm.Metamodel) -> None:
    """**1 案件が単一言語である保証はどこにも無い。**

    対象は C / Java / JS・TS / Python で、画面が TypeScript・バッチが Java と
    いう構成は普通にある ―― 実装規約を 1 件に縛ると、どちらかの規約が正本から
    消える。規約の側は**言語が空欄なら全言語に効く**（例外の方針やログの粒度は
    案件で 1 つに決まるので、共通の 1 条を 4 回書かせない）。
    """
    language = model.item_types["module"]["attributes"]["language"]
    assert set(language["values"]) == {"C", "Java", "JavaScript",
                                       "TypeScript", "Python"}
    assert language.get("required") is not True

    standard = model.item_types["implementation-standard"]["attributes"]
    assert standard["language"].get("required") is not True
    assert standard["standard_kind"]["kind"] == "enum"


def test_論理は疑似コードで止める(model: mm.Metamodel) -> None:
    """**1 文 = 1 レコードにすると、正本が YAML で書いたソースコードになる。**

    揺れは消えるが、設計書として読めるものが出せなくなる（この束の存在理由が
    消える）。境界を固めれば揺れはメソッドの内側に閉じるので、論理はここで止める
    ―― 反復と例外処理に関係を足さないのも同じ理由で、**疑似コードと関係の
    2 か所に同じ枝を書くと必ず片方が古くなる。** 分岐だけは `proceeds-to` が
    既に持っているので、そちらは関係が正である。
    """
    assert "pseudo" in model.item_types["process-step"]["attributes"]
    assert "repeats" not in model.relation_types
    assert "handles-error" not in model.relation_types
    assert "condition" in model.relation_types["proceeds-to"]["attributes"]


def test_プログラム設計の欄はapprovedでだけ必須になる(
        model: mm.Metamodel, documents: dict[str, Any]) -> None:
    """**必須にすると整理層が推測で埋める**（3.2.0 / 3.4.0 / 3.5.0 / 3.8.0）。

    「資料が言っているところまで」書いた draft は通し、実装に渡す approved の
    ときだけ欠けを error にする ―― **既存の案件には 1 件も鳴らない。** 新しい
    4 種別はレコードが 0 件ならルールが当たらず、`module` の 1 本は実装言語を
    宣言したものにだけ掛かる。
    """
    common = set(mm.load_pack("jp-sier-std").get("common_attributes") or {})
    for name in _PROGRAM.values():
        for key, attribute in model.item_types[name]["attributes"].items():
            if key in common:            # name / statement は全種別で必須である
                continue
            assert attribute.get("required") is not True, f"{name}.{key}"

    chain, _ = pack.resolve_chain("jp-sier-std")
    rules = pack.rules(chain).get("attribute_rules") or []
    program = {(r.get("type"), r.get("attribute")): r for r in rules}

    for key in (("implementation-standard", "rule"), ("parameter", "param_name"),
                ("query", "operation"), ("endpoint", "path"),
                ("endpoint", "http_method"), ("module", "source_path")):
        assert program[key]["when_status"] == ["approved"], key
        assert program[key]["level"] == "error", key

    # 実装言語を宣言したモジュールにだけ掛ける（`{not: [~]}` = 値が入っている）。
    assert program[("module", "source_path")]["where"] == {
        "language": {"not": [None]}}


def test_プログラム設計書は必須文書にしない(documents: dict[str, Any]) -> None:
    """**設計書だけを作る案件は、この段に 1 行も書かないという形で降りられる。**

    空で出ることが「この工程の語彙が正本に無い」の申告になる（計画書と同じ
    判断 ―― 決定 55）。落ちた冊子は誰にも見えないが、空の冊子は自分で言う。
    """
    chain, _ = pack.resolve_chain("jp-sier-std")
    required = pack.rules(chain).get("require_documents") or []

    assert "program-design" in documents
    assert "program-design" not in required
    assert documents["program-design"]["phase"] == "プログラム設計"


def test_詳細設計書の欄をプログラム設計書へ写さない(
        documents: dict[str, Any]) -> None:
    """**同じ表が 2 つの工程で別々に承認されると、工程を単位にした意味が消える**
    （`P106` ―― 詳細設計書がメッセージの定義について書いている規律と同じ）。

    ここが出すのは実装の座標と契約だけで、仕様・クラス名・パッケージ・
    シグネチャは詳細設計書の側にある。
    """
    module = _section(documents["program-design"], "モジュールの実装")
    assert module["columns"] == ["module_id", "name", "language", "source_path"]

    method = _section(documents["program-design"], "メソッドの契約")
    for 写さない in ("signature", "returns", "raises", "statement", "description"):
        assert 写さない not in method["columns"]
