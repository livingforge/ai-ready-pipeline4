"""L2（パックの準拠ルール）―― **メタモデルでは表せない工程の約束**を検査する。

ここが見るのは値ではなく**文書の形**である。``group_by`` で節を割りすぎている
かどうかは、メタモデルからは分からない ―― 分類が細かいこと自体は悪ではなく
（``nf_category`` は 6 値ある）、悪いのは**その分類で割った節が痩せる**ことだから、
文書定義を知っている層でしか判定できない。凍結（``freeze``）は文書を見ないので、
節を数えられない。
"""

from __future__ import annotations

from typing import Any

import pytest

from arp4 import conform as conform_module
from arp4 import metamodel as mm
from arp4 import pack as pack_module
from arp4.spec import Spec
from conftest import codes


@pytest.fixture(scope="session")
def chain() -> list[pack_module.Pack]:
    resolved, findings = pack_module.resolve_chain("jp-sier-std")
    assert not [f for f in findings if f.level == "error"]
    return resolved


def _requirements(categories: list[str]) -> list[dict[str, Any]]:
    """区分を 1 件ずつ与えて要件を起こす（同じ区分を並べれば節が太る）。

    入れるのは要件定義書「機能要件」が **いま** ``group_by`` に使っている属性で
    ある（決定 75 で ``category`` → ``subsystem`` に移った）。ここが様式と食い違うと
    値が 1 つの束に落ちて検査が黙り、**痩せた節を作れないテストになる。**
    """
    return [{"id": f"req-{index}", "type": "requirement", "kind": "機能",
             "req_id": f"FR-{index:03d}", "name": f"要件 {index}",
             "statement": "こうであること", "subsystem": category,
             "status": "review"}
            for index, category in enumerate(categories, start=1)]


def _findings(model: mm.Metamodel, chain: list[pack_module.Pack],
              categories: list[str]) -> list:
    spec = Spec(metamodel=model, items=_requirements(categories), relations=[])
    return conform_module._document_rules(spec, chain, pack_module.rules(chain))


def test_節を割りすぎたらC205(model: mm.Metamodel, chain) -> None:
    """**実測の再現。** arp4 自身を通したとき 31 件が 17 節に割れた。

    17 分類のうち 15 が ``src/arp4/`` のモジュール名と 1 対 1 で、要件定義書の
    目次がソースのファイル一覧になっていた ―― 整理層に寄せ先（enum の値）を
    配っていなかったので、1 ファイルずつ読む整理層は目の前のファイル名しか
    書きようが無かった。
    """
    categories = [f"分類{i}" for i in range(1, 18)] + ["分類1"] * 14

    findings = _findings(model, chain, categories)

    assert codes(findings) == ["C205"]
    assert findings[0].level == "warn"
    assert "17 節" in findings[0].message and "31 件" in findings[0].message


def test_節が太っていれば出ない(model: mm.Metamodel, chain) -> None:
    """12 件を 3 節に割るのは分類である（1 節あたり 4 件）。

    ``nf_category`` は IPA 非機能要求グレードの 6 大項目を宣言してあるので、
    同じ整理層・同じラウンドでも 12 件が 3 節に落ち着いた。**差は判断の質では
    なく、寄せ先を配ったかどうかである。**
    """
    assert _findings(model, chain, ["A"] * 4 + ["B"] * 4 + ["C"] * 4) == []


def test_件数が少ないうちは見ない(model: mm.Metamodel, chain) -> None:
    """中身が 5 件なら 5 節に割れて当たり前 ―― まだ形が決まっていない。

    ここで出すと、**書き始めの正本に必ず出る警告**になり、読まれなくなる。
    """
    assert _findings(model, chain, ["A", "B", "C", "D", "E"]) == []


def test_分類が1つなら見ない(model: mm.Metamodel, chain) -> None:
    """節に割れていない（＝ publish も 1 つの表として出す）ので、水増しが無い。"""
    assert _findings(model, chain, ["A"] * 10) == []
