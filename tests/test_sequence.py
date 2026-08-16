"""表示 ID の採番と書式の照合。

**書式は生成にしか使われていなかった。** 宣言した ``format`` を採番のときしか
見ないと、既に入っている値が別系統の ID であることに機械も人も気づかない ――
実際 arp4 自身の正本で message 34 件が 34 件とも書式外のまま座っていた
（`E000` / `W030` / `B012` …。これは実装が発している Finding コードで、
機械が振り直したら仕様と実装が食い違う）。

だから機械は**検知だけ**する。直すのは ``--fix-format`` を打った人である。
"""

from __future__ import annotations

from typing import Any

import pytest

from arp4 import metamodel as mm
from arp4 import sequence
from arp4.spec import Spec
from conftest import codes


def _message(item_id: str, message_id: str, severity: str = "エラー",
             **extra: Any) -> dict[str, Any]:
    return {"id": item_id, "type": "message", "status": "review",
            "name": f"{item_id} のメッセージ", "message_id": message_id,
            "severity": severity, "body": "対象がありません", **extra}


def _spec(model: mm.Metamodel, *items: dict[str, Any]) -> Spec:
    return Spec(metamodel=model, items=list(items), relations=[])


# ── 書式の照合 ──────────────────────────────────────────────────
@pytest.mark.parametrize("value, ok", [
    ("E-0001", True),
    ("E-0000", True),
    ("E-00001", True),       # 桁が増えても書式外にしない（下限としてだけ見る）
    ("E-001", False),        # 桁が足りない
    ("E0001", False),        # 区切りが無い
    ("E-0001a", False),      # 後ろに付いている
    ("E000", False),         # 別系統の ID
])
def test_桁は下限としてだけ見る(value: str, ok: bool) -> None:
    """``format(1000, "03d")`` は 4 桁になる。

    ``\\d{4}`` に固定すると **1000 件を超えた瞬間に既存の番号が全部書式外**になり、
    検出が「増えすぎたから直せ」という直しようのない指示に変わる。
    """
    assert sequence.conforms(value, "E-{:04d}") is ok


def test_接頭辞つきの書式も照合できる() -> None:
    """``prefix_from`` で ``CORE-FR-001`` の形にしたときも同じ規則で見る。"""
    assert sequence.conforms("CORE-FR-001", "FR-{:03d}", "CORE")
    assert not sequence.conforms("FR-001", "FR-{:03d}", "CORE")
    assert sequence.expected("FR-{:03d}", "CORE") == "CORE-FR-{:03d}"


def test_読めない書式は通す() -> None:
    """解釈できないものを error にすると、消費側の拡張で直しようのない検出が出る。"""
    assert sequence.conforms("REQ-  12", "REQ-{:>4}")


# ── 検知 ────────────────────────────────────────────────────────
def test_書式外はE028(model: mm.Metamodel) -> None:
    """**現状は無検出だった。** 空欄は E010 が言うが、別系統の値は誰も見ていない。"""
    findings = sequence.nonconforming(_spec(model, _message("msg-1", "E000")))

    assert codes(findings) == ["E028"]
    assert findings[0].level == "error"
    assert "E000" in findings[0].message and "E-{:04d}" in findings[0].message


def test_書式どおりなら何も出ない(model: mm.Metamodel) -> None:
    findings = sequence.nonconforming(_spec(model, _message("msg-1", "E-0001")))

    assert findings == []


def test_空欄は言わない(model: mm.Metamodel) -> None:
    """打ち手が 2 つに割れる。空欄の打ち手は ``arp4 number`` で、E010 が言う。"""
    findings = sequence.nonconforming(_spec(model, _message("msg-1", "")))

    assert findings == []


def test_overriddenに理由があればwarnへ落ちる(model: mm.Metamodel) -> None:
    """E027 → W032（known_gaps）と同じ落とし方。

    元資料の番号は顧客との対応表であることがある。**固定したという判断は残す。**
    """
    findings = sequence.nonconforming(_spec(model, _message(
        "msg-1", "E000", overridden={"message_id": {
            "was": "E-0001", "reason": "実装が発しているコード（validate.py）",
            "at": "2026-08-09"}})))

    assert codes(findings) == ["W042"]
    assert findings[0].level == "warn"
    assert "validate.py" in findings[0].message


def test_理由の無いoverriddenでは落ちない(model: mm.Metamodel) -> None:
    """空を許すと E028 を黙らせるだけのキーになる（理由の欠落は E017 が言う）。"""
    findings = sequence.nonconforming(_spec(model, _message(
        "msg-1", "E000", overridden={"message_id": {"was": "E-0001"}})))

    assert codes(findings) == ["E028"]


# ── 採番 ────────────────────────────────────────────────────────
def test_書式外は番号を予約しない(model: mm.Metamodel) -> None:
    """``W030`` が ``W-{:04d}`` の 30 番を食うと、正規の ``W-0030`` が理由なく飛ぶ。"""
    assignments, _ = sequence.assign(_spec(
        model,
        _message("msg-1", "W030", "警告"),
        _message("msg-2", "", "警告")))

    assert [a.value for a in assignments] == ["W-0001"]


def test_書式に合う番号は予約する(model: mm.Metamodel) -> None:
    assignments, _ = sequence.assign(_spec(
        model,
        _message("msg-1", "W-0001", "警告"),
        _message("msg-2", "", "警告")))

    assert [a.value for a in assignments] == ["W-0002"]


# ── 宣言外の値（extensible enum）の束 ──────────────────────────
# **`by` の生値で数えると、`default` に落ちた値どうしが番号を食い合わない。**
# 実測（r001）で `test-case.level` の宣言外の値 2 つ（点検 10 件・PoC 2 件）が
# 別々に 1 から採番され、`TC-0001` / `TC-0002` が二重になった。`number` は 1 件も
# 鳴らさず（書式には合っているので `E028` も出ない）、`check --strict` の `E012`
# で初めて出た。`message.severity` も extensible + default なので同じ形が待っている。
def test_宣言外の値どうしはdefaultの1つの束で数える(model: mm.Metamodel) -> None:
    assignments, _ = sequence.assign(_spec(
        model,
        _message("msg-1", "", "監査"), _message("msg-2", "", "監査"),
        _message("msg-3", "", "デバッグ")))

    # 3 件とも default（`M-{:04d}`）なので、通しの連番になる。
    assert [a.value for a in assignments] == ["M-0001", "M-0002", "M-0003"]


def test_宣言外の値の既存番号は他の宣言外の値からも予約されている(
        model: mm.Metamodel) -> None:
    """束が割れていると、既に使われている番号を別の値がもう一度取る。"""
    assignments, _ = sequence.assign(_spec(
        model,
        _message("msg-1", "M-0001", "監査"),
        _message("msg-2", "", "デバッグ")))

    assert [a.value for a in assignments] == ["M-0002"]


def test_renumberでも宣言外の値は衝突しない(model: mm.Metamodel) -> None:
    """``--renumber`` は同じ束の切り方を通るので、**衝突をそのまま再生産していた。**"""
    assignments, _ = sequence.assign(_spec(
        model,
        _message("msg-1", "M-0001", "監査"), _message("msg-2", "M-0002", "監査"),
        _message("msg-3", "M-0001", "デバッグ")), renumber=True)

    assert sorted(a.value for a in assignments) == ["M-0001", "M-0002", "M-0003"]


def test_宣言された値は従来どおり別の束(model: mm.Metamodel) -> None:
    """畳むのは**書式が同じもの**だけ ―― `エラー` と `警告` は別の体系のままである。"""
    assignments, _ = sequence.assign(_spec(
        model,
        _message("msg-1", "", "エラー"), _message("msg-2", "", "警告"),
        _message("msg-3", "", "エラー")))

    assert {a.item_id: a.value for a in assignments} == {
        "msg-1": "E-0001", "msg-3": "E-0002", "msg-2": "W-0001"}


# ── 採番が作った重複（E012） ────────────────────────────────────
def test_採番が作った重複はE012(model: mm.Metamodel) -> None:
    """**書き込む前に止める。** 気づくのが `check` では、もう正本に入っている。"""
    spec = _spec(model, _message("msg-1", "M-0001", "監査"),
                 _message("msg-2", "", "デバッグ"))
    fake = [sequence.Assignment("msg-2", "message", "message_id", "M-0001")]

    findings = sequence.collisions(spec, fake)

    assert codes(findings) == ["E012"]
    assert "M-0001" in findings[0].message and "msg-1" in findings[0].message


def test_元からある重複は採番の責任ではない(model: mm.Metamodel) -> None:
    """ここで一緒に止めると、**その重複を直すために打った ``number`` が進めない。**"""
    spec = _spec(model, _message("msg-1", "M-0001", "監査"),
                 _message("msg-2", "M-0001", "デバッグ"),
                 _message("msg-3", "", "デバッグ"))
    assignments, _ = sequence.assign(spec)

    assert [a.value for a in assignments] == ["M-0002"]
    assert sequence.collisions(spec, assignments) == []


def test_振り直す相手の番号は塞がっていない(model: mm.Metamodel) -> None:
    """``--renumber`` は全件を明け渡すので、**自分の元の番号と衝突しない。**"""
    spec = _spec(model, _message("msg-1", "M-0002", "監査"))
    assignments, _ = sequence.assign(spec, renumber=True)

    assert [a.value for a in assignments] == ["M-0001"]
    assert sequence.collisions(spec, assignments) == []


def test_既定では書式外を振り直さない(model: mm.Metamodel) -> None:
    """**黙って直しはしない。** 元資料の番号は機械が書き換えたら復元できない。"""
    assignments, _ = sequence.assign(_spec(model, _message("msg-1", "E000")))

    assert assignments == []


def test_fix_formatで書式外だけ振り直す(model: mm.Metamodel) -> None:
    assignments, _ = sequence.assign(_spec(
        model,
        _message("msg-1", "E000"),
        _message("msg-2", "E-0009")), fix_format=True)

    assert [(a.item_id, a.previous, a.value) for a in assignments] == [
        ("msg-1", "E000", "E-0001")]          # 書式どおりの E-0009 は動かない


def test_fix_formatはoverriddenを触らずW041(model: mm.Metamodel) -> None:
    """契約書や既存資料と突き合わせるために固定した番号を機械が動かさない。"""
    assignments, findings = sequence.assign(_spec(model, _message(
        "msg-1", "E000", overridden={"message_id": {
            "was": "E-0001", "reason": "実装が発しているコード"}})), fix_format=True)

    assert assignments == []
    assert codes(findings) == ["W041"]


def test_fix_formatは既存番号順に振る(model: mm.Metamodel) -> None:
    """id（ハッシュ）順に振ると、資料の並びと無関係な対応表ができる。"""
    assignments, _ = sequence.assign(_spec(
        model,
        _message("msg-a", "E017"),
        _message("msg-b", "E000"),
        _message("msg-c", "")), fix_format=True)

    assert {a.item_id: a.value for a in assignments} == {
        "msg-b": "E-0001", "msg-a": "E-0002", "msg-c": "E-0003"}


def test_renumberは既存番号の順を保つ(model: mm.Metamodel) -> None:
    """振り直しても**無用な入れ替わりを起こさない**（穴を詰めるだけ）。"""
    assignments, _ = sequence.assign(_spec(
        model,
        _message("msg-a", "E-0009"),
        _message("msg-b", "E-0002")), renumber=True)

    assert {a.item_id: a.value for a in assignments} == {
        "msg-b": "E-0001", "msg-a": "E-0002"}


# ── 節ごとの採番（prefix_from ＋ abbrev） ──────────────────────────
#
# 採番の束（`by: kind`）と設計書の節（`group_by: category`）は別の軸なので、
# 既定では節の中の番号が歯抜けになる（実測 ―― 機能要件 88 件の 1 本の連番が
# 7 節に割れ、`5.1 全体` は FR-030 / 039 / 082 / 084 の 4 件だった）。
# 節ごとに 001 から振り直す経路が `prefix_from` だが、**通しでは 1 度も
# 検査していなかった** ―― 宣言（`prefix_from: category`）だけが標準パックに
# あり、それを効かせる `abbrev` は正本にも試験にも一度も現れていない。
def _req(item_id: str, category: str, **extra: Any) -> dict[str, Any]:
    return {"id": item_id, "type": "requirement", "status": "review",
            "name": f"{item_id} の要件", "kind": "機能", "category": category,
            "statement": "…すること", **extra}


_ABBREV = {"パース": "PARSE", "構築": "BUILD"}


def test_略号を与えた分類は節ごとに001から振る(model: mm.Metamodel) -> None:
    assignments, _ = sequence.assign(_spec(
        model,
        _req("req-1", "パース"), _req("req-2", "構築"),
        _req("req-3", "パース"), _req("req-4", "構築")), abbrev=_ABBREV)

    assert {a.item_id: a.value for a in assignments} == {
        "req-1": "PARSE-FR-001", "req-3": "PARSE-FR-002",
        "req-2": "BUILD-FR-001", "req-4": "BUILD-FR-002"}


def test_略号の無い分類は1つの束に落ちる(model: mm.Metamodel) -> None:
    """**一部だけ書くと残り全部が混ざる。** 分けるなら全部の値に与える。

    接頭辞が空になったものは同じ束なので、分類が違っても番号を食い合う ――
    節に割ったとき、略号を書かなかった節にだけ歯抜けが残る。
    """
    assignments, _ = sequence.assign(_spec(
        model,
        _req("req-1", "パース"),
        _req("req-2", "整理"), _req("req-3", "検証")), abbrev=_ABBREV)

    assert {a.item_id: a.value for a in assignments} == {
        "req-1": "PARSE-FR-001", "req-2": "FR-001", "req-3": "FR-002"}


def test_略号を足すと既存の番号は書式外になる(model: mm.Metamodel) -> None:
    """移行の代償を**検出として見せる**（黙って振り直さない）。"""
    findings = sequence.nonconforming(_spec(
        model, _req("req-1", "パース", req_id="FR-001")), abbrev=_ABBREV)

    assert codes(findings) == ["E028"]
    assert "PARSE-FR-{:03d}" in findings[0].message      # 何であるべきかまで言う


def test_fix_formatが節ごとの採番への移行路になる(model: mm.Metamodel) -> None:
    """`--renumber` ではない ―― **既存番号の順を保ったまま**接頭辞を付け替える。"""
    assignments, _ = sequence.assign(_spec(
        model,
        _req("req-1", "パース", req_id="FR-001"),
        _req("req-2", "構築", req_id="FR-002"),
        _req("req-3", "パース", req_id="FR-003")),
        fix_format=True, abbrev=_ABBREV)

    assert {a.item_id: (a.previous, a.value) for a in assignments} == {
        "req-1": ("FR-001", "PARSE-FR-001"),
        "req-2": ("FR-002", "BUILD-FR-001"),
        "req-3": ("FR-003", "PARSE-FR-002")}


def test_採番の一覧は名前を出す(model: mm.Metamodel) -> None:
    """**内部 ID だけでは正しさを判断できない。**

    `mtd-033bd43e8bc1: method_id = MTD-0001` を 131 行並べても、人は 1 行も
    検算できない ―― id は内容ハッシュなので**名前も種別も読み取れない**。
    番号に業務的な意味を持たせないと決めた（決定 17）以上、採番が妥当かを見る
    手がかりは名称しか無い。
    """
    spec = Spec(metamodel=model, items=[
        {"id": "mtd-033bd43e8bc1", "type": "method", "status": "review",
         "name": "整理結果を読む", "statement": "load は organized/ を読むこと"},
    ], relations=[])
    assignments, _ = sequence.assign(spec)

    assert len(assignments) == 1
    assert assignments[0].render() == \
        "mtd-033bd43e8bc1（整理結果を読む）: method_id = MTD-0001"
