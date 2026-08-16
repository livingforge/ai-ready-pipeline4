"""``--force`` の痕跡 ―― **抜け道を選んだことが生成物に残る。**

実測（sales-corpus 30 冊・r001）で、``error 14 / warn 83`` のまま ``--force`` で
通した 12 文書には「アイテム 808 件 / 関係 1094 件」としか書かれていなかった。
要件 43 件が設計要素に 1 つも繋がっていないことも、権限マトリクスが原典と
逆になっていることも、**成果物を受け取った人からは見えない。**
"""

from __future__ import annotations

from pathlib import Path

from arp4 import gate as gate_module
from arp4.finding import Finding


def _findings() -> list[Finding]:
    return [Finding("error", "E010", "itm-1", "必須属性がありません: data_type"),
            Finding("warn", "W030", "req-1", "どの設計要素からも参照されていません"),
            Finding("warn", "W030", "req-2", "どの設計要素からも参照されていません")]


def test_内訳はコード順で出す() -> None:
    """**件数順にしない。** 版が変わるたびに並びが動くと差分が読めない。"""
    gate = gate_module.summarize(_findings(), forced=True, today="2026-08-11")

    assert gate.breakdown() == "E010 1・W030 2"
    assert gate.errors == 1 and gate.warns == 2


def test_forceで通したら帯が出る() -> None:
    lines = gate_module.banner(
        gate_module.summarize(_findings(), forced=True, today="2026-08-11"))

    assert lines, "帯が出ていない"
    joined = "\n".join(lines)
    assert "--force" in joined
    assert "error 1 件 / warn 2 件" in joined
    # 「書かれていない＝資料に無い」と読ませないための一文（決定の要点）。
    assert "資料に無い" in joined


def test_通常どおり通ったら帯は出ない() -> None:
    """うるさくするのが目的ではない ―― 通ったなら本文の前に何も足さない。"""
    gate = gate_module.summarize(_findings(), forced=False, today="2026-08-11")

    assert gate_module.banner(gate) == []
    assert gate_module.banner_html(gate) == ""


def test_forceでなくても未解決のwarnはフッタが言う() -> None:
    """**通ったことは「穴が無い」ことを意味しない。**

    W030 は error ではないが、トレーサビリティが繋がっていないという事実その
    ものである ―― 実測で 32 件あり、生成物には 1 文字も出ていなかった。
    """
    gate = gate_module.summarize(
        [f for f in _findings() if f.level == "warn"], forced=False)

    note = gate_module.footnote(gate)
    assert "warn 2 件" in note and "W030 2" in note
    assert "--force" not in note


def test_指摘が無ければフッタにも何も足さない() -> None:
    gate = gate_module.summarize([], forced=False)

    assert gate.clean
    assert gate_module.footnote(gate) == ""


def test_帯のリンクは工程フォルダの深さに合わせる() -> None:
    """設計書は ``out/2_基本設計/`` の下に置かれる ―― 穴の一覧は ``out/`` 直下。"""
    gate = gate_module.summarize(_findings(), forced=True)

    assert "](0_" in "\n".join(gate_module.banner(gate, depth=0))
    assert "](../0_" in "\n".join(gate_module.banner(gate, depth=1))


def test_記録は往復する(tmp_path: Path) -> None:
    gate = gate_module.summarize(_findings(), forced=True, today="2026-08-11")
    path = gate_module.record(tmp_path, gate)

    assert path.name == gate_module.FILENAME
    assert gate_module.load(tmp_path) == gate


def test_記録は毎回上書きする(tmp_path: Path) -> None:
    """**消し方が「直す」しか無い。** 手で消しても次の publish が書き直し、
    error を片付けて --force 無しで通せば痕跡そのものが消える。"""
    gate_module.record(tmp_path, gate_module.summarize(_findings(), forced=True))
    gate_module.record(tmp_path, gate_module.summarize([], forced=False))

    after = gate_module.load(tmp_path)
    assert after is not None and not after.forced and after.clean


def test_記録が無ければNone(tmp_path: Path) -> None:
    assert gate_module.load(tmp_path) is None


def test_壊れた記録はNone(tmp_path: Path) -> None:
    """読めないものを既定値で埋めない ―― 「通った」と誤読させない。"""
    (tmp_path / gate_module.FILENAME).write_text("{壊れている", encoding="utf-8")

    assert gate_module.load(tmp_path) is None


def test_日本語をエスケープして書く(tmp_path: Path) -> None:
    """``report`` と同じ理由 ―― cp932 の端末へ流れても JSON として壊れない。"""
    gate = gate_module.summarize(
        [Finding("error", "E010", "itm-1", "必須属性がありません")], forced=True)
    path = gate_module.record(tmp_path, gate)

    assert path.read_text(encoding="utf-8").isascii()
