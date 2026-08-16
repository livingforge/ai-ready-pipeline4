"""**取りこぼしカタログ** ―― 資料にあった事実が、無言で消えた現象を 1 件 1 本で固定する。

ここは**現象の側**で並べる。検査ごとのテスト（`test_publish` / `test_audit` /
`test_lint`）に混ぜると、「この現象は誰が拾うのか」が実装の都合で移動したときに
追えなくなる ―― 拾う担当が ``W043`` から ``P111`` へ移っても、ここは期待値の
1 行を書き換えるだけで済む。

根拠は sales-corpus（原本 30 冊 / 写し 201 枚）を r001 として通した実測である。
正本の ``description`` 628 件のうち **528 件（84%）がどの生成物の本文にも
現れなかった** ―― error も warn も 1 件も出ないまま消えた。

============  ==================================================================
``W043``      列は解決できるのに全行空で、値は**同名の反対側の欄**にある
              （指す先が 1 つに決まる）
``W047``      列は解決できるのに全行空で、母集合が ``description`` を使って
              いる ―― **中身は照合していない**ので、まだ割れていない
``W046``      様式が持つ列を**全行空で畳んだ**（脚注にしか残っていなかった）
``P111``      正本に値があるのに、**どの設計書の列にも出ない**属性
``G029``      宣言済みの欄があるのに ``description`` へ流した（整理層への直接の指摘）
``G030``      同じ見出しの ``description`` が種別内で重なる（**語彙の穴の申告漏れ**）
============  ==================================================================

期待値は**コードと主語だけ**を見る。文言の全文一致はしない ―― hint の推敲で
テストが壊れると、カタログのほうが先に捨てられる。
"""

from __future__ import annotations

from typing import Any

import pytest

from arp4 import freeze
from arp4 import metamodel as mm
from arp4 import publish as publish_module
from arp4.audit import audit
from arp4.metamodel import Metamodel
from arp4.paths import Round
from arp4.spec import Spec
from conftest import codes, organized, parsed


def _documents(monkeypatch: pytest.MonkeyPatch,
               documents: list[dict[str, Any]]) -> None:
    """文書定義を差し替える（パックの様式に依存させない）。"""
    monkeypatch.setattr(publish_module.pack_module, "documents",
                        lambda _chain: documents)


def _doc(monkeypatch: pytest.MonkeyPatch, sections: list[dict[str, Any]],
         title: str = "設計書") -> None:
    _documents(monkeypatch, [{"name": "doc", "title": title, "sections": sections}])


def _targets(findings, code: str) -> list[str]:
    return [f"{f.target} {f.message}" for f in findings if f.code == code]


# ── TC-A 値が別の欄にある（W043 / W046） ────────────────────────
def _displays_spec(model: mm.Metamodel, note: str = "") -> Spec:
    """画面項目 3 本。**初期値と物理名が `note` ではなく `description` に入っている。**

    実測（r001）で `displays` 164 本すべてが `note` 空のまま、初期値
    （「受注日 + 2 営業日」）と画面側の物理名が `description` へ流れた ――
    `description` は :data:`arp4.metamodel.RELATION_RESERVED` の予約キーなので
    宣言なしにどの関係へも書けてしまい、スキーマ検査を素通りする。
    """
    items: list[dict[str, Any]] = [
        {"id": "scr-1", "type": "screen", "status": "review",
         "screen_id": "SCR-001", "name": "受注入力",
         "statement": "受注入力画面は受注を登録すること"}]
    relations: list[dict[str, Any]] = []
    for index, (name, told) in enumerate(
            [("受注番号", "物理名 orderNo ／ 初期値 採番"),
             ("受注日", "物理名 orderDate ／ 初期値 システム日付"),
             ("納品予定日", "物理名 dueDate ／ 初期値 受注日 + 2 営業日")], start=1):
        items.append({"id": f"itm-{index}", "type": "data-item", "status": "review",
                      "name": name, "data_type": "文字列",
                      "statement": f"{name}は受注の項目であること"})
        relation: dict[str, Any] = {
            "type": "displays", "from": "scr-1", "to": f"itm-{index}",
            "status": "review", "order": index, "io": "入力",
            "description": told}
        if note:
            relation["note"] = note
        relations.append(relation)
    return Spec(metamodel=model, items=items, relations=relations)


_DISPLAYS_SECTION = {"heading": "画面帳票項目", "kind": "relation",
                     "relation": "displays", "group_by": "from",
                     "columns": ["order", "to.name", "io", "note"]}


def test_A1_関係の欄が空で同じ関係のdescriptionに値があればW047(
        model: mm.Metamodel, monkeypatch: pytest.MonkeyPatch) -> None:
    """**同じ側の逃がし先を誰も見ていなかった。**

    ``_elsewhere`` が探していたのは「**同名**の属性が関係の**反対側**にある」
    場合だけで、`note`（空）も `description`（値あり）も同じ側なので ``None`` を
    返し、列は「全行が空だったので省略した列: 備考」と畳まれていた。

    拾うのは ``W047`` である ―― ここで言えているのは「`note` は空」と
    「`description` に何か入っている」の 2 つだけで、**その `description` が
    `note` の値かどうかは見ていない**（この検体では実際に初期値と物理名だが、
    機械はそれを確かめていない）。
    """
    _doc(monkeypatch, [_DISPLAYS_SECTION])

    found = _targets(publish_module.lint(_displays_spec(model)), "W047")
    assert len(found) == 1
    assert "note" in found[0] and "description" in found[0]


def test_A1_欄が埋まっていればW047は鳴らない(
        model: mm.Metamodel, monkeypatch: pytest.MonkeyPatch) -> None:
    """**偽陽性の確認。** 宣言どおり `note` へ写してあれば、何も言うことは無い。"""
    _doc(monkeypatch, [_DISPLAYS_SECTION])

    said = codes(publish_module.lint(_displays_spec(model, note="採番")))
    assert "W043" not in said and "W047" not in said


def test_A2_値が別の欄にある列は畳まずに両方を名指しする(
        model: mm.Metamodel, monkeypatch: pytest.MonkeyPatch) -> None:
    """**「資料に無い」と「別の欄に入っている」を同じ顔で出さない。**

    畳んでしまうと脚注は「全行が空だったので省略した列: 備考」としか言わない ――
    その `備考` が `displays.note` で、整理の手順書が「初期値・物理名はそこへ
    写す」と名指ししている欄だとは、その 1 行からは辿れない。**畳まずに残し、
    見に行く先（`displays.description`）まで書く。**

    ここでは ``W046`` を出さない ―― 同じ列を 2 回言わない（``W043`` が既に
    言っている）。``broken`` と ``W043`` の間にある規律と同じである。
    """
    _doc(monkeypatch, [_DISPLAYS_SECTION])
    spec = _displays_spec(model)

    blocks = publish_module._blocks(spec, {"sections": [_DISPLAYS_SECTION]})
    tables = [b for b in blocks if b.rows]
    note = "\n".join(n for b in tables for n in b.notes)
    assert "displays.note" in note and "displays.description" in note
    assert "省略した列" not in note              # 畳んでいない
    assert "W046" not in codes(audit(spec))


def test_A2_逃がし先が無い列は畳んでW046で数える(
        model: mm.Metamodel, monkeypatch: pytest.MonkeyPatch) -> None:
    """**畳んだことが `_gate.json` に残っていなかった。**

    脚注は章の末尾に 1 行出るだけで、束としては数えられない ―― 実測（r001）で
    23 列が畳まれたが、ゲートの件数にも穴の 1 枚のコード表にも 1 件も現れなかった。
    """
    section = {"heading": "画面一覧", "type": "screen",
               "columns": ["screen_id", "name", "route", "statement"]}
    _doc(monkeypatch, [section])
    spec = Spec(metamodel=model, items=[
        {"id": f"scr-{i}", "type": "screen", "status": "review",
         "screen_id": f"SCR-00{i}", "name": f"画面{i}",
         "statement": f"画面 {i} は受注を扱うこと"} for i in range(1, 4)],
        relations=[])

    found = _targets(audit(spec), "W046")
    assert len(found) == 1
    assert "screen.route" in found[0] and "画面一覧" in found[0]


def test_A3_アイテムの章でも値が別の欄にあればW047(
        model: mm.Metamodel, monkeypatch: pytest.MonkeyPatch) -> None:
    """この検査は ``kind: relation`` の章にしか走っていなかった。

    `_lint_section` の ``if kind == "relation":`` が外の 3 種別を素通りさせて
    いたので、一覧・升目・トレースの章で同じことが起きても誰も言わなかった。
    """
    section = {"heading": "画面一覧", "type": "screen",
               "columns": ["screen_id", "name", "route", "description"]}
    _doc(monkeypatch, [section])
    spec = Spec(metamodel=model, items=[
        {"id": f"scr-{i}", "type": "screen", "status": "review",
         "screen_id": f"SCR-00{i}", "name": f"画面{i}",
         "statement": f"画面 {i} は受注を扱うこと",
         "description": f"画面パス /order/{i}"} for i in range(1, 4)], relations=[])

    found = _targets(publish_module.lint(spec), "W047")
    assert len(found) == 1 and "route" in found[0]


def test_A4_トレースの章でも値が別の欄にあればW047(
        model: mm.Metamodel, monkeypatch: pytest.MonkeyPatch) -> None:
    """トレースの章も同じ ―― ただし ``linked`` は計算列なので数えない。"""
    section = {"heading": "要件 → 設計要素", "kind": "trace", "type": "requirement",
               "relation": "realizes", "where": {"kind": "非機能"},
               "columns": ["req_id", "name", "metric", "linked"]}
    _doc(monkeypatch, [section])
    spec = Spec(metamodel=model, items=[
        {"id": f"req-{i}", "type": "requirement", "status": "review",
         "req_id": f"NFR-00{i}", "kind": "非機能", "name": f"要件{i}",
         "statement": f"要件 {i} を満たすこと",
         "description": "確認方法 負荷試験 ／ 確認する工程 総合テスト"}
        for i in range(1, 4)], relations=[])

    found = _targets(publish_module.lint(spec), "W047")
    assert len(found) == 1 and "metric" in found[0]
    assert not [f for f in found if "linked" in f]


# ── TC-B 正本にあるのに、どの列にも出ない（P111） ───────────────
def _interfaces(model: mm.Metamodel) -> Spec:
    """外部インターフェース 5 件。**異常時の扱い・再送/再実行が `description` にある。**"""
    return Spec(metamodel=model, items=[
        {"id": f"eif-{i}", "type": "external-interface", "status": "review",
         "if_id": f"IF-00{i}", "name": f"連携{i}", "direction": "送信",
         "statement": f"連携 {i} は日次で送ること",
         "description": "異常時の扱い 翌日再送 ／ 再実行 手動"} for i in range(1, 6)],
        relations=[])


def test_B1_正本に値があるのにどの列にも出ない属性はP111(
        model: mm.Metamodel, monkeypatch: pytest.MonkeyPatch) -> None:
    """**``P110`` の粒度を関係型から属性へ下げる。**

    ``P110`` が見ているのは ``section.relation`` の集合だけで、``columns`` を
    1 つも見ていない ―― **節さえあれば属性が全部落ちても黙る。** 実測（r001）で
    外部インターフェース 5 件すべての「異常時の扱い」「再送/再実行」が
    `description` に入り、束のどこにも出なかった。
    """
    _doc(monkeypatch, [{"heading": "外部インターフェース一覧",
                        "type": "external-interface",
                        "columns": ["if_id", "name", "direction", "statement"]}])

    found = [f for f in audit(_interfaces(model)) if f.code == "P111"]
    assert [f.target for f in found] == ["external-interface.description"]
    assert "5 件" in found[0].message


def test_B2_同じ種別でも節が出していなければP111(
        model: mm.Metamodel, monkeypatch: pytest.MonkeyPatch) -> None:
    """**種別で見ると取りこぼす。** 出ているかは**そのレコードが出たか**で決まる。

    実測（r001）で非機能要件 15 件の「確認方法」「確認する工程」は
    `description` にあり、非機能要件の節にその列が無かった ―― 機能要件の節には
    `description` 列があるので、種別×属性で数えると「出ている」ことになる。
    """
    _doc(monkeypatch, [
        {"heading": "機能要件", "type": "requirement", "where": {"kind": "機能"},
         "columns": ["req_id", "name", "statement", "description"]},
        {"heading": "非機能要件", "type": "requirement", "where": {"kind": "非機能"},
         "columns": ["req_id", "name", "statement", "metric"]}])
    items = [{"id": f"req-f{i}", "type": "requirement", "status": "review",
              "req_id": f"FR-00{i}", "kind": "機能", "name": f"機能要件{i}",
              "statement": f"機能 {i} を提供すること",
              "description": f"補足 {i}"} for i in range(1, 4)]
    items += [{"id": f"req-n{i}", "type": "requirement", "status": "review",
               "req_id": f"NFR-00{i}", "kind": "非機能", "name": f"非機能要件{i}",
               "statement": f"非機能 {i} を満たすこと",
               "description": "確認方法 負荷試験 ／ 確認する工程 総合テスト"}
              for i in range(1, 6)]

    found = [f for f in audit(Spec(metamodel=model, items=items, relations=[]))
             if f.code == "P111"]
    assert [f.target for f in found] == ["requirement.description"]
    assert "5 件" in found[0].message


def test_B3_値が1件も無い属性ではP111は鳴らない(
        model: mm.Metamodel, monkeypatch: pytest.MonkeyPatch) -> None:
    """**様式の穴だけを見る。** 正本が値を持たない属性は、様式の問題ではない。"""
    _doc(monkeypatch, [{"heading": "外部インターフェース一覧",
                        "type": "external-interface",
                        "columns": ["if_id", "name", "direction", "statement"]}])
    spec = _interfaces(model)
    for item in spec.items:
        item.pop("description")

    assert "P111" not in codes(audit(spec))


def test_B4_group_byやwhereに使われている属性は出ているとみなす(
        model: mm.Metamodel, monkeypatch: pytest.MonkeyPatch) -> None:
    """**節の見出しも出稿である。** `group_by` の値は節名として印字される。"""
    _doc(monkeypatch, [{"heading": "画面一覧", "type": "screen",
                        "group_by": "subsystem",
                        "columns": ["screen_id", "name", "statement"]}])
    spec = Spec(metamodel=model, items=[
        {"id": f"scr-{i}", "type": "screen", "status": "review",
         "screen_id": f"SCR-00{i}", "name": f"画面{i}",
         "subsystem": "受注管理" if i % 2 else "在庫管理",
         "statement": f"画面 {i} は受注を扱うこと"} for i in range(1, 5)], relations=[])

    assert "screen.subsystem" not in [f.target for f in audit(spec)]


def test_B_段はwarnに留める(model: mm.Metamodel,
                            monkeypatch: pytest.MonkeyPatch) -> None:
    """``P111`` は初回に数十種鳴る ―― **error にすると `--force` の理由を作る。**"""
    _doc(monkeypatch, [{"heading": "外部インターフェース一覧",
                        "type": "external-interface",
                        "columns": ["if_id", "name", "direction", "statement"]}])

    assert all(f.level == "warn" for f in audit(_interfaces(model)))


# ── TC-C / D 整理層への直接のフィードバック（G029 / G030） ──────
_PARSED = """\
# a.xlsx / 画面項目

<!-- source: 資料/a.xlsx / シート: 画面項目 -->

## 表 B5:H8  <!-- a:s1-t1 at=B5:H8 -->

| 論理名 | 物理名 | 初期値 |
|---|---|---|
| 受注番号 | orderNo | 採番 |
"""


def _lint(round_: Round, model: Metamodel, body: str,
          name: str = "資料/a.xlsx/画面項目") -> list[str]:
    parsed(round_, f"{name}.md", _PARSED)
    organized(round_, f"{name}.yml", body)
    return codes(freeze.lint(round_, model, {}).findings)


def _permission_yaml(count: int, told: str) -> str:
    """権限マトリクスの △ を `description` へ逃がした整理結果。"""
    head = """\
records:
  - concept: c-act-営業
    type: 利用者・ロール
    name: 営業
    statement: 営業は受注を登録するロールであること
    source: { anchor: s1-t1 }
    refs:
"""
    body = "".join(
        f"      - {{ rel: operates, to: c-scr-{i}, note: \"{told}\" }}\n"
        for i in range(1, count + 1))
    tail = "".join(f"""\
  - concept: c-scr-{i}
    type: 画面
    name: 受注入力{i}
    statement: 受注入力{i} は受注を登録すること
    source: {{ anchor: s1-t1 }}
""" for i in range(1, count + 1))
    return head + body + tail


def test_D1_宣言済みの欄があるのにdescriptionへ流したらG029(
        round_: Round, model: Metamodel) -> None:
    """**権限マトリクスの △ は語彙を持っている。**

    決定 71 で ``permission: 条件付き`` と ``condition`` を足したのに、実測
    （r001）では 56 本の `operates` がどちらも空のまま、元の機能行名と条件が
    `description` に流れた ―― 予約キーなので ``G016``（宣言に無い属性名）にも
    ``G028``（enum 外）にも当たらず、**スキーマ検査を素通りする。**
    """
    said = _lint(round_, model,
                 _permission_yaml(1, "権限 条件付き ／ 条件 部長職のみ可"))

    assert "G029" in said


def test_D1_逃がした値はどの列にも出ないのでP111も鳴る(
        model: mm.Metamodel, monkeypatch: pytest.MonkeyPatch) -> None:
    """整理層（``G029``）と様式（``P111``）の両側から同じ穴を塞ぐ。"""
    _doc(monkeypatch, [{"heading": "権限マトリクス", "kind": "matrix",
                        "relation": "operates", "row_header": "ロール",
                        "rows": ["actor"], "cols": ["screen"],
                        "cell": "permission"}], title="権限マトリクス")
    spec = Spec(metamodel=model, items=[
        {"id": "act-1", "type": "actor", "status": "review", "actor_id": "ACT-01",
         "name": "営業", "statement": "営業は受注を登録するロールであること"},
        {"id": "scr-1", "type": "screen", "status": "review",
         "screen_id": "SCR-001", "name": "受注入力",
         "statement": "受注入力画面は受注を登録すること"},
    ], relations=[{"type": "operates", "from": "act-1", "to": "scr-1",
                   "status": "review", "description": "権限 条件付き ／ 条件 部長職のみ可"}])

    assert "operates.description" in [f.target for f in audit(spec) if f.code == "P111"]


def test_D2_受け皿の無い見出しが重なったらG030(round_: Round,
                                                model: Metamodel) -> None:
    """**語彙の穴の申告漏れ。**

    受け皿が無くて `description` に逃げたものは ``_metamodel-add.yml`` へ提案
    されるべきだった（``G007`` の相手）。実測（r001）で外部インターフェース
    5 件の「異常時の扱い」がそろって `description` にあり、提案は 1 件も無かった。
    """
    said = _lint(round_, model, _permission_yaml(5, "異常時の扱い 翌日再送"))

    assert "G030" in said


def test_D3_出典の欄の取りこぼしは既存のG018が拾う(round_: Round,
                                                   model: Metamodel) -> None:
    """**既存の検査が生きていることの確認。**

    ``G018`` は「出典の欄は埋まっているのに属性が空」を照合だけで言う ――
    ただし見られるのは**メタモデルが ``column:`` を宣言している属性**に限る
    （実測でコード資産の ``method`` だけが宣言を持つ）。メッセージ定義書の
    「出力条件」のように受け皿の宣言が無い欄は、ここではなく ``G030`` が拾う。
    """
    parsed(round_, "src/x.py.md", """\
# src/x.py

<!-- source: src/x.py -->

## クラス: Foo  <!-- a:m1 at=src/x.py#L1-L10 -->

| メンバ | 種類 | 注釈 | シグネチャ | 戻り値 | 例外 | 行 |
|---|---|---|---|---|---|---|
| run | メソッド |  | run() | int | ValueError | 5 |
""")
    organized(round_, "src/x.py.yml", """\
records:
  - concept: c-mtd-src.x.Foo.run
    type: メソッド
    name: run
    statement: run は受注を登録して件数を返すこと
    source: { anchor: m1 }
    attrs: { signature: "run()", returns: int }
""")

    assert "G018" in codes(freeze.lint(round_, model, {}).findings)


# ── TC-E 標準パックの様式そのものを検体にする ───────────────────
#: 全種別が持つ属性（`common_attributes`）。**様式は種別ごとに出す/出さないを
#: 選んでいる**（→ 決定 75 と `basic-design.yml` の注記）ので、埋まっていれば
#: `P111` が鳴るのは正しい ―― ここで潰す対象ではない。
_COMMON = {"owner", "priority", "subsystem"}

#: トレースの章にしか出ない関係。**関係そのものの補足を置く列が作れない**
#: （トレースはアイテムを行にするので、関係の属性を持てない）。ここは様式では
#: なく整理層の側で閉じる ―― `realizes` の「対応機能の種別」は関係ではなく
#: 設計要素の属性である（→ `organize.md`）。
_TRACE_ONLY = {"contributes-to.description", "disputes.description",
               "realizes.description", "resolves.description",
               "supports.description", "uses-specimen.description"}


def _every_attribute(model: mm.Metamodel,
                     documents: list[dict[str, Any]]) -> Spec:
    """**宣言済みの属性が全部埋まった 1 件**を種別ごとに置いた検体。

    正本の代わりにこれを通すと、鳴った ``P111`` は**データの欠落ではなく
    様式の穴**になる ―― 「この属性を書いても、どの設計書にも出ない」が
    コーパスを待たずに分かる。
    """
    def value(attribute: dict[str, Any], name: str) -> Any:
        kind = str(attribute.get("kind") or "string")
        if kind == "enum":
            values = attribute.get("values") or ["値"]
            return [values[0]] if attribute.get("multi") else values[0]
        return {"int": 1, "bool": True}.get(kind, f"{name}の値")

    def wheres(type_name: str) -> list[dict[str, Any]]:
        # **絞りのある種別に「絞りに当たらない 1 件」を混ぜない** ―― `requirement`
        # の `kind` は必須なので、空の 1 件はデータでは起こらない（E010 が言う）。
        found: list[dict[str, Any]] = []
        for document in documents:
            for section in document.get("sections") or []:
                if (str(section.get("type") or "") != type_name
                        or str(section.get("kind") or "items") != "items"):
                    continue
                where = section.get("where")
                if isinstance(where, dict) and where not in found:
                    found.append(dict(where))
        return found or [{}]

    def concrete(where: dict[str, Any],
                 definition: dict[str, Any]) -> dict[str, Any]:
        """``where`` を**その節に当たる 1 件ぶんの実値**にする。

        絞りは 3 通り書ける（→ :func:`arp4.conform.matches`）―― 単一値・配列
        （どれか）・``{not: [...]}``（残り全部）。**そのまま item へ写すと
        配列や連想配列が属性値になり、どの節にも当たらない検体ができる** ――
        1 つの種別を複数の設計書へ振り分ける様式（`business-rule` /
        `constraint`）で、種別まるごとが P111 に落ちて見えた。
        """
        fixed: dict[str, Any] = {}
        for key, expected in where.items():
            if isinstance(expected, dict):
                excluded = expected.get("not") or []
                values = [v for v in ((definition.get("attributes") or {})
                                      .get(key) or {}).get("values") or []
                          if v not in excluded]
                if values:
                    fixed[key] = values[0]
                continue        # enum でないなら生成した値がそのまま当たる
            fixed[key] = expected[0] if isinstance(expected, list) else expected
        return fixed

    items: list[dict[str, Any]] = []
    by_type: dict[str, list[dict[str, Any]]] = {}
    for type_name, definition in model.item_types.items():
        for index, where in enumerate(wheres(type_name)):
            item: dict[str, Any] = {"id": f"{type_name}-{index}", "type": type_name,
                                    "status": "review"}
            for name, attribute in (definition.get("attributes") or {}).items():
                item[name] = value(attribute or {}, name)
            item.update({"name": f"{type_name}{index}",
                         "statement": f"{type_name} は検体であること",
                         **concrete(where, definition)})
            items.append(item)
            by_type.setdefault(type_name, []).append(item)

    def resolve(names: list[str]) -> list[str]:
        return [t for name in names for t in model.groups.get(name, (name,))]

    relations: list[dict[str, Any]] = []
    for relation_type, definition in model.relation_types.items():
        for source in resolve([str(t) for t in (definition.get("from") or [])]):
            for target in resolve([str(t) for t in (definition.get("to") or [])]):
                left = (by_type.get(source) or [None])[0]
                right = (by_type.get(target) or [None])[0]
                if left is None or right is None or left is right:
                    continue
                relation: dict[str, Any] = {
                    "type": relation_type, "from": left["id"], "to": right["id"],
                    "status": "review", "description": "補足", "order": 1}
                for name, attribute in (definition.get("attributes") or {}).items():
                    relation[name] = value(attribute or {}, name)
                relations.append(relation)
    return Spec(metamodel=model, items=items, relations=relations)


def test_E_標準パックの様式は宣言済みの属性を出す(
        model: mm.Metamodel, monkeypatch: pytest.MonkeyPatch) -> None:
    """**コーパスを待たずに様式の穴を数える。**

    実測（r001）で消えた 528 件の受け皿は、ほとんどが「その節に列が無い」
    ことだけが原因だった ―― `displays.note` も `data-item.description` も
    `operates.condition` も、正本には入っていて列が無かった。**列が無いことは
    データを見なくても分かる。**

    残す例外は 2 つだけで、どちらも様式では閉じられない（:data:`_COMMON` /
    :data:`_TRACE_ONLY`）。増やすときは**理由をそこへ書く** ―― 黙って足すと
    この検査は「いま鳴っていないもの」の記録に変わる。
    """
    from arp4 import pack as pack_module
    chain, findings = pack_module.resolve_chain("jp-sier-std")
    assert not [f for f in findings if f.level == "error"]
    documents = pack_module.documents(chain)
    _documents(monkeypatch, documents)

    found = {f.target for f in audit(_every_attribute(model, documents))
             if f.code == "P111"}
    unexpected = sorted(t for t in found
                        if t.rsplit(".", 1)[-1] not in _COMMON
                        and t not in _TRACE_ONLY)

    assert unexpected == [], (
        "様式（documents/*.yml）に出す先がありません: " + "・".join(unexpected))


def test_E_例外の一覧が古くなっていないか見る(
        model: mm.Metamodel, monkeypatch: pytest.MonkeyPatch) -> None:
    """**反対側も見る**（`holes._CODES` と同じ規律）。

    様式に列を足して鳴らなくなったものが例外の一覧に残ると、**次に落ちたとき
    黙る。** 例外は「いま本当に閉じられないもの」だけにする。
    """
    from arp4 import pack as pack_module
    chain, _ = pack_module.resolve_chain("jp-sier-std")
    documents = pack_module.documents(chain)
    _documents(monkeypatch, documents)

    found = {f.target for f in audit(_every_attribute(model, documents))
             if f.code == "P111"}

    assert sorted(_TRACE_ONLY - found) == []


# ── 件数で埋めない ―― 打ち手が 1 つなら指摘も 1 件 ──────────────
def test_F_畳んだ列は章1件にまとめる(model: mm.Metamodel,
                                      monkeypatch: pytest.MonkeyPatch) -> None:
    """**様式が持つ列にデータが追いつくのは正常な途中経過である。**

    1 列 1 件で出すと、アイテム 5 件の例でも 18 件鳴ってゲートの件数がそれだけで
    埋まり、**本物の指摘が件数に埋もれる**（`P104` から出典列を外したのと同じ
    判断）。直す単位は章なので、章 1 件にまとめて列を全部名指しする ――
    1 件ずつは `0_この設計書の穴.md` の表にある。
    """
    section = {"heading": "画面一覧", "type": "screen",
               "columns": ["screen_id", "name", "route", "screen_type",
                           "statement"]}
    _doc(monkeypatch, [section])
    spec = Spec(metamodel=model, items=[
        {"id": f"scr-{i}", "type": "screen", "status": "review",
         "screen_id": f"SCR-00{i}", "name": f"画面{i}",
         "statement": f"画面 {i} は受注を扱うこと"} for i in range(1, 4)],
        relations=[])

    found = _targets(audit(spec), "W046")
    assert len(found) == 1
    assert "2 列" in found[0]
    assert "screen.route" in found[0] and "screen.screen_type" in found[0]


def test_F_値が別の欄にある列も章1件にまとめる(
        model: mm.Metamodel, monkeypatch: pytest.MonkeyPatch) -> None:
    """**直す先は 1 つの節の定義である。** 列の数だけ行が並んでも打ち手は増えない。"""
    section = {**_DISPLAYS_SECTION,
               "columns": ["order", "to.name", "io", "note", "control"]}
    _doc(monkeypatch, [section])
    spec = _displays_spec(model)

    found = _targets(publish_module.lint(spec), "W047")
    assert len(found) == 1
    assert "2 列" in found[0]
    assert "`note` → `description`" in found[0] and "`control`" in found[0]
    # 脚注も 1 本にまとめる（章末に同じ文が列の数だけ並ぶと、どれも読まれない）。
    blocks = publish_module._blocks(spec, {"sections": [section]})
    notes = [n for b in blocks for n in b.notes if "displays.note" in n]
    assert len(notes) == 1


def test_F_descriptionへの逃がしはファイル1件で言う(round_: Round,
                                                    model: Metamodel) -> None:
    """**レコード 1 件ずつ並べない**（`G004` をファイル単位にしたのと同じ規律）。

    実測（r001）で `displays` は 1 ファイルに 154 本あり、直す操作は「その欄へ
    写す」1 つしか無い ―― 154 行に割ると、他の指摘がその中に埋もれる。
    """
    parsed(round_, "資料/a.xlsx/画面項目.md", _PARSED)
    organized(round_, "資料/a.xlsx/画面項目.yml",
              _permission_yaml(5, "権限 条件付き ／ 条件 部長職のみ可"))

    found = [f for f in freeze.lint(round_, model, {}).findings
             if f.code == "G029"]

    assert len(found) == 1
    assert "5 件" in found[0].message and found[0].target == "operates"
    assert "permission" in found[0].message and "condition" in found[0].message
