"""同一性の台帳 ―― **既存の判断は上書きしない。**"""

from __future__ import annotations

from arp4 import concepts
from arp4.paths import Paths
from conftest import codes

_PROPOSAL = {
    "new": [{"concept": "c-受注番号", "type": "データ項目", "label": "受注番号",
             "aliases": ["受注No"], "reason": "物理名 ORDER_NO が一致"}],
    "assign": [{"concept": "c-顧客コード", "aliases_add": ["得意先コード"]}],
}


def test_提案を重ねて保存し読み戻せる(project: Paths) -> None:
    known = {"c-顧客コード": concepts.Concept(concept="c-顧客コード",
                                              label="顧客コード", since="2026-08-02")}
    added, findings = concepts.apply_proposal(known, _PROPOSAL, "2026-11-14")
    concepts.save(project, known)
    again, load_findings = concepts.load(project)

    assert added == ["c-受注番号"] and not findings and not load_findings
    assert again["c-受注番号"].since == "2026-11-14"      # 初出ラウンドが残る
    assert again["c-受注番号"].aliases == ["受注No"]
    assert again["c-顧客コード"].aliases == ["得意先コード"]


def test_既にある概念をnewで出しても壊さない(project: Paths) -> None:
    """前ラウンドの結果を見ていない形。**上書きせず警告する。**"""
    known = {"c-受注番号": concepts.Concept(concept="c-受注番号", label="受注番号",
                                            item="itm-既存", since="2026-08-02")}
    _, findings = concepts.apply_proposal(
        known, {"new": _PROPOSAL["new"]}, "2026-11-14")

    assert codes(findings) == ["B002"]
    assert known["c-受注番号"].item == "itm-既存"


def test_newを再適用しても別名は反映される(project: Paths) -> None:
    """初回 build が台帳へ自動登録するので、**やり直すと new が全部「既にある」**に
    なっていた ―― 別名が入ったのかを台帳の grep で確かめる羽目になる。"""
    known = {"c-受注番号": concepts.Concept(concept="c-受注番号", label="受注番号",
                                            since="2026-08-02")}
    _, findings = concepts.apply_proposal(
        known, {"new": _PROPOSAL["new"]}, "2026-08-02")

    assert known["c-受注番号"].aliases == ["受注No"]
    assert codes(findings) == ["B002"] and "別名" in findings[0].message


def test_同じラウンドの登録済みには何も言わない(project: Paths) -> None:
    """**凍結後は整理結果を編集できない。**

    「このラウンドで登録済みです（正常です）」を build のたびに出していたので、
    やり直すたびに new の件数ぶん warn が永久に出続けた ―― 本物の warn を
    見落とす訓練になる。言うべきは「前ラウンドの台帳を見ていない」ほうだけ。
    """
    entry = concepts.Concept(concept="c-受注番号", label="受注番号",
                             aliases=["受注No"], since="2026-08-02")
    _, same = concepts.apply_proposal({"c-受注番号": entry},
                                      {"new": _PROPOSAL["new"]}, "2026-08-02")
    _, later = concepts.apply_proposal({"c-受注番号": entry},
                                       {"new": _PROPOSAL["new"]}, "2026-11-14")

    assert same == []
    assert "台帳を見ていない" in later[0].message


def test_相手のいないassignはB003(project: Paths) -> None:
    _, findings = concepts.apply_proposal({}, {"assign": [{"concept": "c-いない"}]},
                                          "2026-08-02")
    assert codes(findings) == ["B003"]


def test_ensureは無ければ作る(project: Paths) -> None:
    """横断整理を回していないラウンドでも build が通るようにする保険。"""
    known: dict[str, concepts.Concept] = {}
    entry = concepts.ensure(known, "c-a", "データ項目", "受注番号", "2026-08-02")

    assert entry.since == "2026-08-02"
    assert concepts.ensure(known, "c-a", "x", "y", "2026-11-14") is entry
