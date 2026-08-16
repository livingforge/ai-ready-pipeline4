"""``arp4 audit`` ―― **出来上がった設計書のほうを検査する（``P0xx``）。**

ここに並ぶのは、実測（sales-corpus 30 冊・r001）で**出荷されてしまった形**である。
規則はいずれも先に書かれていた ―― `publish.md` の「列見出しは日本語」も
「母集合そのものを並べない」も。足りなかったのは規則ではなく、規則を確かめる者。
"""

from __future__ import annotations

from typing import Any

import pytest

from arp4 import metamodel as mm
from arp4.audit import audit
from arp4.spec import Spec
from conftest import codes


def _requirement(ident: str, req_id: str) -> dict[str, Any]:
    return {"id": ident, "type": "requirement", "status": "review",
            "req_id": req_id, "kind": "機能", "name": f"要件{req_id}",
            "statement": f"{req_id} は満たされること"}


def _trace_doc(monkeypatch: pytest.MonkeyPatch, sections: list[dict[str, Any]],
               title: str = "トレーサビリティ・マトリクス") -> None:
    """文書定義を差し替える（パックの定義に依存させない）。"""
    from arp4 import publish as publish_module
    monkeypatch.setattr(publish_module.pack_module, "documents",
                        lambda _chain: [{"name": "doc", "title": title,
                                         "sections": sections}])


# ── P101 母集合をそのまま並べた表 ───────────────────────────────
def test_トレースの結論が全行空ならP101(model: mm.Metamodel,
                                        monkeypatch: pytest.MonkeyPatch) -> None:
    """**100% 空は結論ではない。**

    実測でトレーサビリティ・マトリクスの「要件 → テストケース」80 行と
    「モジュール → テストケース」18 行が全行 `―` で出た。同じ文書の末尾では
    「未検証の要件」が*対象データが無いので省略*されており、**同じ事実から
    逆の判断が 2 つ出ている。**

    **見るのは「関係はあるのに、この母集合の誰にも繋がっていない」場合である。**
    関係が 1 本も無いときは章ごと畳まれるので、ここには来ない ―― あちらは
    「調べたら 0 だった」ではなく**その語彙をまだ取り込んでいない**であり、
    理由つきで畳むのが正しい（→ :func:`publish._trace_blocks`）。
    """
    _trace_doc(monkeypatch, [{"heading": "要件 → テストケース", "kind": "trace",
                              "type": "requirement", "relation": "verifies",
                              "where": {"kind": "機能"},
                              "columns": ["req_id", "kind", "name", "linked"]}])
    # `verifies` は使われているが、繋がっているのは母集合の外（非機能）だけ。
    spec = Spec(metamodel=model,
                items=[_requirement("req-1", "FR-001"),
                       _requirement("req-2", "FR-002"),
                       {**_requirement("req-3", "NFR-001"), "kind": "非機能"},
                       {"id": "tc-1", "type": "test-case", "status": "review",
                        "test_id": "TC-0001", "name": "応答時間",
                        "expected": "3 秒以内であること"}],
                relations=[{"type": "verifies", "from": "tc-1", "to": "req-3",
                            "status": "review"}])

    found = [f for f in audit(spec) if f.code == "P101"]
    assert len(found) == 1
    assert "2 行すべて" in found[0].message


def test_関係が1本も無ければ章ごと畳まれてP101は出ない(
        model: mm.Metamodel, monkeypatch: pytest.MonkeyPatch) -> None:
    """**「調べたら 0 だった」と「まだ取り込んでいない」は次の一手が正反対。**

    実測（r001）で、前向きの対応表は 101 行を全行 `―` で出しながら、対になる
    「未検証の要件（テスト漏れ）」は*対象データが無いので省略*されていた ――
    **同じ事実から逆の判断が 2 つ**出ていた。いまはどちらも同じ理由で畳む。
    """
    _trace_doc(monkeypatch, [{"heading": "要件 → テストケース", "kind": "trace",
                              "type": "requirement", "relation": "verifies",
                              "columns": ["req_id", "kind", "name", "linked"]}])
    spec = Spec(metamodel=model,
                items=[_requirement("req-1", "FR-001"),
                       _requirement("req-2", "FR-002")], relations=[])

    assert "P101" not in codes(audit(spec))


def test_一部でも埋まっていればP101は出ない(model: mm.Metamodel,
                                            monkeypatch: pytest.MonkeyPatch) -> None:
    """トレースは**空欄そのものが結論**なので、埋まりかけを咎めない。"""
    _trace_doc(monkeypatch, [{"heading": "要件 → テストケース", "kind": "trace",
                              "type": "requirement", "relation": "verifies",
                              "columns": ["req_id", "kind", "name", "linked"]}])
    spec = Spec(metamodel=model,
                items=[_requirement("req-1", "FR-001"),
                       _requirement("req-2", "FR-002"),
                       {"id": "tc-1", "type": "test-case", "status": "review",
                        "test_id": "TC-001", "name": "受注登録",
                        "expected": "登録できること",
                        "statement": "受注を登録できること"}],
                relations=[{"type": "verifies", "from": "tc-1", "to": "req-1",
                            "status": "review"}])

    assert "P101" not in codes(audit(spec))


# ── P102 升目の凡例 ─────────────────────────────────────────────
def _matrix_spec(model: mm.Metamodel) -> Spec:
    return Spec(metamodel=model, items=[
        {"id": "act-1", "type": "actor", "status": "review", "actor_id": "ACT-01",
         "name": "営業", "statement": "営業は権限ロールの 1 つであること"},
        {"id": "scr-1", "type": "screen", "status": "review", "screen_id": "SCR-001",
         "name": "受注入力", "statement": "受注入力画面は受注を登録すること"},
    ], relations=[{"type": "operates", "from": "act-1", "to": "scr-1",
                   "status": "review"}])


def test_升目には空欄の意味を書く(model: mm.Metamodel,
                                  monkeypatch: pytest.MonkeyPatch) -> None:
    """**× と「記載が無い」が同じ空欄になる。**

    原典は ``○ / △ / ×`` の 3 値だが、関係の有無に写した時点で区別が消える
    ―― 実測（r001 の権限マトリクス）で ``△＝部長職のみ可`` は正本からも消え、
    ``×`` は空欄になり、凡例そのものも落ちていた。
    """
    from arp4 import publish as publish_module
    _trace_doc(monkeypatch, [{"heading": "権限マトリクス", "kind": "matrix",
                              "relation": "operates", "row_header": "ロール",
                              "rows": ["actor"], "cols": ["screen"],
                              "cell": "permission"}], title="権限マトリクス")
    spec = _matrix_spec(model)

    # 既定では凡例が付くので P102 は出ない。
    assert "P102" not in codes(audit(spec))
    note = "\n".join(publish_module._blocks(spec, {
        "sections": [{"heading": "権限マトリクス", "kind": "matrix",
                      "relation": "operates", "row_header": "ロール",
                      "rows": ["actor"], "cols": ["screen"],
                      "cell": "permission"}]})[0].notes)
    # **空欄が何を意味するかを必ず言う。** ただしこの spec の operates は
    # permission を持たないので、`不可` を持ち出して言い切ってはいけない。
    assert "空欄" in note
    assert "空欄とは別である" not in note
    assert "1 件もない" in note
    # 1 件も無い属性を「出どころ」に挙げない（読み手はそこを見に行く）。
    assert "operates.permission" not in note


def test_否定が升に出て初めて空欄との違いを言い切る(
        model: mm.Metamodel, monkeypatch: pytest.MonkeyPatch) -> None:
    """**語彙にあることと、この表に出ていることは違う。**

    決定 71 で ``不可`` を足したあとの実測 ―― ``operates`` 38 本すべてが
    ``permission`` を持たないのに、凡例は「不可 は…空欄とは別である」と
    **1 升も無い値の説明**を出し、出どころとして **1 件も無い属性**を名指しした。
    読み手はそこに ``不可`` が無いことを「禁止が無い」と読むが、原典は同じ升を
    ``×`` と書いている。
    """
    from arp4 import publish as publish_module
    section = {"heading": "権限マトリクス", "kind": "matrix",
               "relation": "operates", "row_header": "ロール",
               "rows": ["actor"], "cols": ["screen"], "cell": "permission"}
    spec = _matrix_spec(model)
    spec.relations[0]["permission"] = ["不可"]

    note = "\n".join(
        publish_module._blocks(spec, {"sections": [section]})[0].notes)
    assert "不可 は資料が明示的に禁じている" in note
    assert "空欄とは別である" in note
    # 値が実在するので、出どころを名指ししてよい。
    assert "operates.permission" in note


def test_禁止を書ける升に禁止が1件も無いとP109(
        model: mm.Metamodel, monkeypatch: pytest.MonkeyPatch) -> None:
    """**正直な凡例は「落ちた」とは言わない。** 落ちたことを言うのは検査である。"""
    _trace_doc(monkeypatch, [{"heading": "権限マトリクス", "kind": "matrix",
                              "relation": "operates", "row_header": "ロール",
                              "rows": ["actor"], "cols": ["screen"],
                              "cell": "permission"}], title="権限マトリクス")
    spec = _matrix_spec(model)

    assert "P109" in codes(audit(spec))

    spec.relations[0]["permission"] = ["不可"]
    assert "P109" not in codes(audit(spec))


def test_否定の語彙が無い升ではP109は鳴らない(
        model: mm.Metamodel, monkeypatch: pytest.MonkeyPatch) -> None:
    """**語彙の穴と写し漏れを同じ音にしない。**

    ``accesses.crud`` は否定を宣言していない ―― 凡例が既に「升では表せない」と
    断っているので、ここで重ねて鳴らすと直しようのない warn になる。
    """
    _trace_doc(monkeypatch, [{"heading": "CRUD 図", "kind": "matrix",
                              "relation": "accesses", "row_header": "機能",
                              "rows": ["screen"], "cols": ["entity"],
                              "cell": "crud"}], title="CRUD図")
    spec = Spec(metamodel=model, items=[
        {"id": "scr-1", "type": "screen", "status": "review",
         "screen_id": "SCR-001", "name": "受注入力",
         "statement": "受注入力画面は受注を登録すること"},
        {"id": "ent-1", "type": "entity", "status": "review",
         "physical_name": "T_ORDER", "name": "受注ヘッダ",
         "statement": "受注ヘッダは受注を保持すること"},
    ], relations=[{"type": "accesses", "from": "scr-1", "to": "ent-1",
                   "status": "review", "crud": ["R"]}])

    assert "P109" not in codes(audit(spec))


def test_否定の語彙が無い升は表せないと断る(model: mm.Metamodel,
                                            monkeypatch: pytest.MonkeyPatch) -> None:
    """**語彙が無いうちは言い切らない。**

    ``negative`` を宣言していない関係では、升は「関係がある」しか言えない ――
    そこで「不可とは別」と書くと、書けないものを書けるかのように読ませる。
    """
    from arp4 import publish as publish_module
    section = {"heading": "CRUD 図", "kind": "matrix", "relation": "accesses",
               "row_header": "機能", "rows": ["screen"], "cols": ["entity"],
               "cell": "crud"}
    spec = Spec(metamodel=model, items=[
        {"id": "scr-1", "type": "screen", "status": "review",
         "screen_id": "SCR-001", "name": "受注入力",
         "statement": "受注入力画面は受注を登録すること"},
        {"id": "ent-1", "type": "entity", "status": "review",
         "physical_name": "T_ORDER", "name": "受注ヘッダ",
         "statement": "受注ヘッダは受注を保持すること"},
    ], relations=[{"type": "accesses", "from": "scr-1", "to": "ent-1",
                   "status": "review", "crud": ["R"]}])

    note = "\n".join(
        publish_module._blocks(spec, {"sections": [section]})[0].notes)
    assert "升では表せない" in note


def test_凡例を落とすとP102(model: mm.Metamodel,
                            monkeypatch: pytest.MonkeyPatch) -> None:
    """凡例の生成を止めたら検査が鳴る ―― **退行したことが分かる。**"""
    from arp4 import publish as publish_module
    _trace_doc(monkeypatch, [{"heading": "権限マトリクス", "kind": "matrix",
                              "relation": "operates", "row_header": "ロール",
                              "rows": ["actor"], "cols": ["screen"],
                              "cell": "permission"}], title="権限マトリクス")
    monkeypatch.setattr(publish_module, "_legend", lambda *a, **k: [])

    assert "P102" in codes(audit(_matrix_spec(model)))


# ── P103 節の見出しが ASCII だけ ────────────────────────────────
def test_enumの生値が見出しに出たらP103(model: mm.Metamodel,
                                        monkeypatch: pytest.MonkeyPatch) -> None:
    """実測で基本設計書が ``7.1 business`` … ``7.7 未分類`` と混在で出た。

    ``publish.md`` の「列見出しは日本語」は**列しか見ていなかった** ――
    ``group_by`` が作る節の見出しは素通りしていた。
    """
    _trace_doc(monkeypatch, [{"heading": "業務ルール", "type": "business-rule",
                              "group_by": "rule_kind",
                              "columns": ["rule_id", "name", "statement"]}])

    def rule(ident: str, rule_id: str, kind: str) -> dict[str, Any]:
        return {"id": ident, "type": "business-rule", "status": "review",
                "rule_id": rule_id, "rule_kind": kind, "name": f"ルール{rule_id}",
                "statement": f"{rule_id} を守ること"}

    # 語彙に無い値（value_labels にも無い）は訳せないので生値のまま出る。
    spec = Spec(metamodel=model, items=[rule("rul-1", "RUL-001", "business"),
                                        rule("rul-2", "RUL-002", "lifecycle")],
                relations=[])

    found = [f for f in audit(spec) if f.code == "P103"]
    assert len(found) == 1 and "lifecycle" in found[0].message


def test_value_labelsがあればP103は出ない(model: mm.Metamodel,
                                          monkeypatch: pytest.MonkeyPatch) -> None:
    """値は訳さず**見出しだけ**差し替える ―― 値を訳すと ``E011`` が自分の出力を弾く。"""
    _trace_doc(monkeypatch, [{"heading": "業務ルール", "type": "business-rule",
                              "group_by": "rule_kind",
                              "columns": ["rule_id", "name", "statement"]}])

    def rule(ident: str, rule_id: str, kind: str) -> dict[str, Any]:
        return {"id": ident, "type": "business-rule", "status": "review",
                "rule_id": rule_id, "rule_kind": kind, "name": f"ルール{rule_id}",
                "statement": f"{rule_id} を守ること"}

    spec = Spec(metamodel=model, items=[rule("rul-1", "RUL-001", "business"),
                                        rule("rul-2", "RUL-002", "calculation")],
                relations=[])

    assert "P103" not in codes(audit(spec))


# ── P104 同じ本文の繰り返し ─────────────────────────────────────
def _screen(ident: str, screen_id: str, note: str) -> dict[str, Any]:
    return {"id": ident, "type": "screen", "status": "review",
            "screen_id": screen_id, "name": f"画面{screen_id}",
            "statement": f"{screen_id} は画面であること", "description": note}


_BOILERPLATE = ("利用権限は営業・管理者。方式は排他制御が楽観的排他"
                "（更新日時が一致しない場合は排他エラー）、表示件数は一覧 1 ページ"
                " 50 件のページ送り ―― 方式の 2 行は本冊子の 5 画面すべてに"
                "同じ文が置かれている")


def test_定型文の繰り返しはP104(model: mm.Metamodel,
                                monkeypatch: pytest.MonkeyPatch) -> None:
    """実測で画面一覧の補足が 12 回出た（しかも「N 画面」の N が 4 と 5 で揺れた）。"""
    _trace_doc(monkeypatch, [{"heading": "画面一覧", "type": "screen",
                              "columns": ["screen_id", "name", "description"]}])
    spec = Spec(metamodel=model,
                items=[_screen(f"scr-{i}", f"SCR-00{i}", _BOILERPLATE)
                       for i in range(1, 6)], relations=[])

    found = [f for f in audit(spec) if f.code == "P104"]
    assert len(found) == 1 and "5 回" in found[0].message


def test_出典の繰り返しは数えない(model: mm.Metamodel,
                                  monkeypatch: pytest.MonkeyPatch) -> None:
    """**同じシートから来た行が同じアンカーを出すのは当たり前。**

    数えたときの 54 件のうち 45 件が出典列で、本物（補足の定型文 4 件）が
    件数に埋もれていた。
    """
    _trace_doc(monkeypatch, [{"heading": "画面一覧", "type": "screen",
                              "columns": ["screen_id", "name", "source"]}])
    where = [{"round": "r001",
              "file": "資料/00_全体/02_基本設計/画面一覧.xlsx/画面一覧",
              "anchor": "s3-t1"}]
    spec = Spec(metamodel=model,
                items=[{**_screen(f"scr-{i}", f"SCR-00{i}", ""), "source": where}
                       for i in range(1, 6)], relations=[])

    assert "P104" not in codes(audit(spec))


# ── P105 争点のあるアイテムに印が無い ───────────────────────────
def test_争点のあるアイテムに印が無ければP105(model: mm.Metamodel,
                                              monkeypatch: pytest.MonkeyPatch) -> None:
    """実測で、基本設計書には**互いに矛盾する 4 組**が並列に載った。

    課題管理表では拾えているのに、当の基本設計書ではただの規則である ――
    読み手は両方を確定仕様として受け取る。関係は既に張ってあるので、
    足りないのは印を出すことだけ。

    印を出す側を止めたら鳴る ―― **退行したことが分かる。**
    """
    from arp4 import publish as publish_module
    _trace_doc(monkeypatch, [{"heading": "業務ルール", "type": "business-rule",
                              "columns": ["rule_id", "name", "statement"]}])
    monkeypatch.setattr(publish_module, "_dispute_marks", lambda *a, **k: None)

    found = [f for f in audit(_disputed_spec(model)) if f.code == "P105"]
    assert len(found) == 1 and "RUL-136" in found[0].message


def test_publishが印を出すのでP105は出ない(model: mm.Metamodel,
                                            monkeypatch: pytest.MonkeyPatch) -> None:
    """**新しい判断はしていない。** 正本にある `disputes` の相手を行に出すだけ。"""
    from arp4 import publish as publish_module
    section = {"heading": "業務ルール", "type": "business-rule",
               "columns": ["rule_id", "name", "statement"]}
    _trace_doc(monkeypatch, [section])
    spec = _disputed_spec(model)

    assert "P105" not in codes(audit(spec))

    block = publish_module._blocks(spec, {"sections": [section]})[0]
    assert block.columns[-1] == "課題"
    assert block.rows[0][-1] == "ISS-017"


def test_争点が1件も無い表に課題の列を足さない(model: mm.Metamodel,
                                                monkeypatch: pytest.MonkeyPatch) -> None:
    """**印は多いほど目に入らなくなる。** ほとんどが `―` の列を増やさない。"""
    from arp4 import publish as publish_module
    section = {"heading": "業務ルール", "type": "business-rule",
               "columns": ["rule_id", "name", "statement"]}
    spec = _disputed_spec(model)
    spec.relations.clear()

    block = publish_module._blocks(spec, {"sections": [section]})[0]
    assert "課題" not in block.columns


def _disputed_spec(model: mm.Metamodel) -> Spec:
    """争点 1 組 ―― 課題 `ISS-017` が業務ルール `RUL-136` を争っている。"""
    return Spec(metamodel=model, items=[
        {"id": "rul-1", "type": "business-rule", "status": "review",
         "rule_id": "RUL-136", "name": "在庫引当を実行する時点",
         "statement": "在庫の引当は出荷指示の実行時に行うこと"},
        {"id": "iss-1", "type": "open-issue", "status": "review",
         "issue_id": "ISS-017", "name": "在庫引当を行う時点",
         "statement": "受注確定時か出荷指示時かを決めること"},
    ], relations=[{"type": "disputes", "from": "iss-1", "to": "rul-1",
                   "status": "review"}])


# ── P106 同じアイテムが複数の設計書に全文で出る ─────────────────
def test_列の顔ぶれまで同じならP106(model: mm.Metamodel,
                                    monkeypatch: pytest.MonkeyPatch) -> None:
    """実測で業務ルール 186 件が基本設計書と詳細設計書に全文で重複していた
    （詳細設計書だけにある規則は 0 件 ―― 完全な部分集合）。"""
    from arp4 import publish as publish_module
    columns = ["rule_id", "name", "condition", "action", "statement"]
    monkeypatch.setattr(publish_module.pack_module, "documents", lambda _c: [
        {"name": "a", "title": "基本設計書",
         "sections": [{"heading": "業務ルール", "type": "business-rule",
                       "columns": columns}]},
        {"name": "b", "title": "詳細設計書",
         "sections": [{"heading": "実装ルール", "type": "business-rule",
                       "columns": columns}]}])
    spec = Spec(metamodel=model, items=[
        {"id": f"rul-{i}", "type": "business-rule", "status": "review",
         "rule_id": f"RUL-00{i}", "name": f"ルール{i}",
         "statement": f"ルール {i} を守ること"} for i in range(1, 6)], relations=[])

    found = [f for f in audit(spec) if f.code == "P106"]
    assert len(found) == 1 and "5 件" in found[0].message


def test_索引はP106にしない(model: mm.Metamodel,
                            monkeypatch: pytest.MonkeyPatch) -> None:
    """**同じ ID が 2 冊に出ること自体は咎めない。**

    トレーサビリティ・マトリクスは要件 80 件を並べるのが仕事で、あれは重複では
    なく索引である ―― ID と名称しか共有しないので当たらない。
    """
    from arp4 import publish as publish_module
    monkeypatch.setattr(publish_module.pack_module, "documents", lambda _c: [
        {"name": "a", "title": "要件定義書",
         "sections": [{"heading": "機能要件", "type": "requirement",
                       "columns": ["req_id", "name", "statement", "source"]}]},
        {"name": "b", "title": "トレーサビリティ・マトリクス",
         "sections": [{"heading": "要件 → 設計要素", "kind": "trace",
                       "type": "requirement", "relation": "realizes",
                       "columns": ["req_id", "name"]}]}])
    spec = Spec(metamodel=model,
                items=[_requirement(f"req-{i}", f"FR-00{i}") for i in range(1, 6)],
                relations=[])

    assert "P106" not in codes(audit(spec))


# ── P107 出典列の有無が揺れている ───────────────────────────────
def test_出典列の有無が揃っていなければP107(model: mm.Metamodel,
                                            monkeypatch: pytest.MonkeyPatch) -> None:
    """**同じアイテムなのに、開いた設計書によって追跡できたりできなかったりする。**"""
    from arp4 import publish as publish_module
    monkeypatch.setattr(publish_module.pack_module, "documents", lambda _c: [
        {"name": "a", "title": "基本設計書",
         "sections": [{"heading": "画面一覧", "type": "screen",
                       "columns": ["screen_id", "name", "source"]}]},
        {"name": "b", "title": "画面帳票項目定義書",
         "sections": [{"heading": "画面一覧", "type": "screen",
                       "columns": ["screen_id", "name"]}]}])
    spec = Spec(metamodel=model,
                items=[_screen("scr-1", "SCR-001", "")], relations=[])

    found = [f for f in audit(spec) if f.code == "P107"]
    assert len(found) == 1 and "screen" in found[0].target


def test_段はすべてwarn(model: mm.Metamodel,
                        monkeypatch: pytest.MonkeyPatch) -> None:
    """**error にしない。** ここが見ているのは読み手が誤読しうる形であって
    データの不備ではない ―― error にすると ``--force`` を押す理由をまた作り、
    ``arp4.gate`` が塞いだ穴を別の場所に開け直すことになる。
    """
    _trace_doc(monkeypatch, [{"heading": "要件 → テストケース", "kind": "trace",
                              "type": "requirement", "relation": "verifies",
                              "columns": ["req_id", "kind", "name", "linked"]}])
    spec = Spec(metamodel=model,
                items=[_requirement("req-1", "FR-001"),
                       _requirement("req-2", "FR-002")], relations=[])

    assert all(f.level == "warn" for f in audit(spec))


# ── P108 「未分類」の節が大きすぎる ─────────────────────────────
def test_未分類が章の1割4分を超えたらP108(model: mm.Metamodel,
                                          monkeypatch: pytest.MonkeyPatch) -> None:
    """**プロンプトを強めても直らない類の問題。**

    ``organize.md`` は既に「未分類のまま残すのは資料に区分が無いときだけ」と書いて
    いて、整理層はそれに従っている ―― 資料の側に区分が無いのは事実である。それでも
    実測（r001）で制約 70 件中 21 件（30%）が「8.4 未分類」に入り、8.1 技術（40 件）に
    次ぐ 2 番目の節になった。中身の 12 件は「〜のドメイン」で括れるものだった。
    """
    _trace_doc(monkeypatch, [{"heading": "制約・前提", "type": "constraint",
                              "group_by": "category",
                              "columns": ["constraint_id", "name", "statement"]}])

    def constraint(i: int, category: str | None) -> dict[str, Any]:
        item = {"id": f"cst-{i}", "type": "constraint", "status": "review",
                "constraint_id": f"CST-{i:03d}", "name": f"制約{i}",
                "statement": f"制約 {i} を守ること"}
        if category:
            item["category"] = category
        return item

    spec = Spec(metamodel=model, items=(
        [constraint(i, "技術") for i in range(1, 6)]
        + [constraint(i, None) for i in range(6, 10)]), relations=[])

    found = [f for f in audit(spec) if f.code == "P108"]
    assert len(found) == 1
    assert "4 件" in found[0].message and "44%" in found[0].message


def test_未分類が少なければP108は出ない(model: mm.Metamodel,
                                        monkeypatch: pytest.MonkeyPatch) -> None:
    """**資料に区分が無いものは実在する。** 少数なら未分類のままが正しい。"""
    _trace_doc(monkeypatch, [{"heading": "制約・前提", "type": "constraint",
                              "group_by": "category",
                              "columns": ["constraint_id", "name", "statement"]}])

    def constraint(i: int, category: str | None) -> dict[str, Any]:
        item = {"id": f"cst-{i}", "type": "constraint", "status": "review",
                "constraint_id": f"CST-{i:03d}", "name": f"制約{i}",
                "statement": f"制約 {i} を守ること"}
        if category:
            item["category"] = category
        return item

    spec = Spec(metamodel=model, items=(
        [constraint(i, "技術") for i in range(1, 10)]
        + [constraint(10, None)]), relations=[])

    assert "P108" not in codes(audit(spec))


def test_章をまたいで数えない(model: mm.Metamodel,
                              monkeypatch: pytest.MonkeyPatch) -> None:
    """**目次で隣に並ぶのは同じ章の節だけである。**

    文書全体を分母にすると、要件定義書は制約 21 件を 200 行超で割ることになり、
    2 番目に大きい節が未分類でも 1 割を切って黙る。
    """
    _trace_doc(monkeypatch, [
        {"heading": "制約・前提", "type": "constraint", "group_by": "category",
         "columns": ["constraint_id", "name", "statement"]},
        {"heading": "業務ルール", "type": "business-rule", "group_by": "rule_kind",
         "columns": ["rule_id", "name", "statement"]}])

    items: list[dict[str, Any]] = []
    for i in range(1, 4):                            # 制約 3 件（うち 2 件が未分類）
        item: dict[str, Any] = {
            "id": f"cst-{i}", "type": "constraint", "status": "review",
            "constraint_id": f"CST-{i:03d}", "name": f"制約{i}",
            "statement": f"制約 {i} を守ること"}
        if i == 1:
            item["category"] = "技術"
        items.append(item)
    for i in range(1, 30):                           # 業務ルール 29 件（全部分類済み）
        items.append({"id": f"rul-{i}", "type": "business-rule", "status": "review",
                      "rule_id": f"RUL-{i:03d}", "name": f"ルール{i}",
                      "rule_kind": "business" if i % 2 else "calculation",
                      "statement": f"ルール {i} を守ること"})

    found = [f for f in audit(Spec(metamodel=model, items=items, relations=[]))
             if f.code == "P108"]
    assert len(found) == 1 and "制約・前提" in found[0].target


# ── P110 正本にあるのに、どの様式も出していない関係型 ───────────
def test_様式が拾っていない関係型をP110が言う(model: mm.Metamodel,
                                              monkeypatch: pytest.MonkeyPatch) -> None:
    """**穴の 1 枚も元資料の対応表も、この面を言わない。**

    実測（r001）で 293 本が誰にも読めなかった ―― いちばん多い `constrains`
    227 本が出ないのに、縛る先を**持っていない**制約 45 件だけが穴の 1 枚に
    理由つきで並んでいた。**丁寧に張ったほうが静かに消える。**
    """
    _trace_doc(monkeypatch, [{"heading": "業務ルール", "type": "business-rule",
                              "columns": ["rule_id", "name", "statement"]}])
    spec = Spec(metamodel=model, items=[
        {"id": "cst-1", "type": "constraint", "status": "review",
         "constraint_id": "CST-001", "name": "文字コード",
         "statement": "すべて UTF-8 とすること"},
        {"id": "rul-1", "type": "business-rule", "status": "review",
         "rule_id": "RUL-001", "name": "消費税の計算",
         "statement": "明細単位に計算すること"},
    ], relations=[{"type": "constrains", "from": "cst-1", "to": "rul-1",
                   "status": "review"}])

    found = [f for f in audit(spec) if f.code == "P110"]
    assert len(found) == 1
    assert "constrains" in found[0].target and "1 本" in found[0].message


def test_使われていない語彙はP110で言わない(model: mm.Metamodel,
                                            monkeypatch: pytest.MonkeyPatch) -> None:
    """**様式の問題だけを見る。** 正本に 1 本も無い関係型は、様式の穴ではない。"""
    _trace_doc(monkeypatch, [{"heading": "業務ルール", "type": "business-rule",
                              "columns": ["rule_id", "name", "statement"]}])
    spec = Spec(metamodel=model, items=[
        {"id": "rul-1", "type": "business-rule", "status": "review",
         "rule_id": "RUL-001", "name": "消費税の計算",
         "statement": "明細単位に計算すること"}], relations=[])

    assert "P110" not in codes(audit(spec))


def test_節が無くても別の形で出ている関係は言わない(model: mm.Metamodel,
                                                    monkeypatch: pytest.MonkeyPatch) -> None:
    """`disputes` は「課題」列として出る ―― 節が無いことは穴ではない。"""
    _trace_doc(monkeypatch, [{"heading": "業務ルール", "type": "business-rule",
                              "columns": ["rule_id", "name", "statement"]}])

    found = [f for f in audit(_disputed_spec(model)) if f.code == "P110"]
    assert not found


def test_節に割れても列で数える(model: mm.Metamodel,
                                monkeypatch: pytest.MonkeyPatch) -> None:
    """**塊ごとに数えると、節に割れた表では閾値に届かない。**

    実測（r001）で「利用権限 営業・管理者」は 13 画面の `補足` 列にあったのに
    `P104` は 1 件しか数えなかった ―― `group_by` が節に割ると、同じ定型文が
    節の数だけ分散する。列は節をまたいで 1 本なので、数える単位も列である。
    しかも 60 字に届かない短い定型句は、受け皿（`補足`）でも落ちていた。
    """
    _trace_doc(monkeypatch, [{"heading": "画面一覧", "type": "screen",
                              "group_by": "subsystem",
                              "columns": ["screen_id", "name", "description"]}])
    spec = Spec(metamodel=model, items=[
        {**_screen(f"scr-{i}", f"SCR-00{i}", "利用権限は営業・管理者"),
         "subsystem": "受注管理" if i % 2 else "在庫管理"}
        for i in range(1, 7)], relations=[])

    found = [f for f in audit(spec) if f.code == "P104"]
    assert len(found) == 1
    assert "6 回" in found[0].message and "補足" in found[0].message


def test_短い値の繰り返しは自由記述の列でだけ数える(
        model: mm.Metamodel, monkeypatch: pytest.MonkeyPatch) -> None:
    """**enum の値が並ぶのは当たり前。** 閾値を下げるのは受け皿の列だけである。"""
    _trace_doc(monkeypatch, [{"heading": "画面一覧", "type": "screen",
                              "columns": ["screen_id", "name", "screen_type"]}])
    spec = Spec(metamodel=model, items=[
        {**_screen(f"scr-{i}", f"SCR-00{i}", ""), "screen_type": "入力画面"}
        for i in range(1, 7)], relations=[])

    assert "P104" not in codes(audit(spec))
