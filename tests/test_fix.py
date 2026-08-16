"""``arp4 lint --fix`` ―― **機械的に確実なものだけを直す。**

自動修正は範囲を狭く切れるかどうかが全部である。ここで見るのは「直せるものを
直すか」よりも、**直せないものを直さないか**と、**直した結果を検算しているか**
のほうである ―― 越えてはいけない線（意味の判断は整理層だけ）を機械が越えると、
越えたことが誰にも見えない（直った結果しか残らない）。
"""

from __future__ import annotations

import io
import json
import sys

from arp4 import cli, fix, organized as organized_module, yamlio
from arp4.metamodel import Metamodel
from arp4.paths import Paths, Round
from conftest import codes, organized, parsed

_PARSED = """\
# a.xlsx / 受注テーブル

<!-- source: 資料/a.xlsx / シート: 受注テーブル -->

## 表 B5:H8  <!-- a:s1-t1 at=B5:H8 -->

| 論理名 | 物理名 |
|---|---|
| 受注番号 | ORDER_NO |
"""


def _repair(round_: Round, model: Metamodel, name: str = "受注.yml"):
    path = round_.organized / name
    return fix.repair(path, model, name), path


# ── 直すもの ────────────────────────────────────────────────────
def test_attrsが無ければその場で作る(round_: Round, model: Metamodel) -> None:
    organized(round_, "受注.yml", """\
records:
  - concept: c-A
    type: エンティティ
    name: 受注
    statement: 受注を保持すること
    physical_name: T_ORDER
    source: { anchor: s1-t1 }
""")
    (applied, refused), path = _repair(round_, model)

    assert not refused
    assert [f.code for f in applied] == ["G008"]
    assert applied[0].line == 6
    assert "attrs: { physical_name: T_ORDER }" in path.read_text(encoding="utf-8")


def test_フロー記法のattrsへ足す(round_: Round, model: Metamodel) -> None:
    organized(round_, "受注.yml", """\
records:
  - concept: c-A
    type: エンティティ
    name: 受注
    statement: 受注を保持すること
    attrs: { entity_kind: トランザクション }
    physical_name: T_ORDER
    source: { anchor: s1-t1 }
""")
    (applied, refused), path = _repair(round_, model)
    data = yamlio.load(path)

    assert not refused and len(applied) == 1
    assert data["records"][0]["attrs"] == {"entity_kind": "トランザクション",
                                           "physical_name": "T_ORDER"}
    assert "physical_name: T_ORDER }" in path.read_text(encoding="utf-8")


def test_ブロック記法のattrsへ足す(round_: Round, model: Metamodel) -> None:
    organized(round_, "受注.yml", """\
records:
  - concept: c-A
    type: エンティティ
    name: 受注
    statement: 受注を保持すること
    attrs:
      entity_kind: トランザクション
    physical_name: T_ORDER
    source: { anchor: s1-t1 }
""")
    (applied, refused), path = _repair(round_, model)

    assert not refused and len(applied) == 1
    assert yamlio.load(path)["records"][0]["attrs"]["physical_name"] == "T_ORDER"


def test_値は生のテキストのまま運ぶ(round_: Round, model: Metamodel) -> None:
    """読み直して書き戻すと、引用の有無やコメントが失われる ―― 整理結果は人と
    エージェントが読む面なので、**直した覚えのない差分を出さない。**"""
    organized(round_, "受注.yml", """\
# この資料は旧版から起こした
records:
  - concept: c-A
    type: エンティティ
    name: 受注
    statement: 受注を保持すること
    volume: "1,200,000/年"
    source: { anchor: s1-t1 }
""")
    (applied, _), path = _repair(round_, model)
    text = path.read_text(encoding="utf-8")

    assert len(applied) == 1
    assert '"1,200,000/年"' in text                 # 引用符ごと運ぶ
    assert "# この資料は旧版から起こした" in text    # コメントが残る
    assert yamlio.load(path)["records"][0]["attrs"]["volume"] == "1,200,000/年"


def test_複数あっても1つずつ直す(round_: Round, model: Metamodel) -> None:
    organized(round_, "受注.yml", """\
records:
  - concept: c-A
    type: エンティティ
    name: 受注
    statement: 受注を保持すること
    physical_name: T_ORDER
    volume: 月 3 万件
    source: { anchor: s1-t1 }
""")
    (applied, refused), path = _repair(round_, model)

    assert not refused and len(applied) == 2
    assert yamlio.load(path)["records"][0]["attrs"] == {
        "physical_name": "T_ORDER", "volume": "月 3 万件"}


# ── 直さないもの ────────────────────────────────────────────────
def test_語彙に無い名前は動かさない(round_: Round, model: Metamodel) -> None:
    """``attrs`` へ移しても ``build`` が捨てる先が変わるだけで、**直っていないのに
    指摘が消える。** 名前の取り違えは資料を読まないと直せない。"""
    organized(round_, "受注.yml", """\
records:
  - concept: c-A
    type: エンティティ
    name: 受注
    statement: 受注を保持すること
    桁数: 10
    source: { anchor: s1-t1 }
""")
    (applied, refused), path = _repair(round_, model)

    assert not applied and not refused
    assert "桁数: 10" in path.read_text(encoding="utf-8")


def test_種別が決まらないレコードは触らない(round_: Round, model: Metamodel) -> None:
    """参照だけのレコードは種別を名乗らないので、何が属性かが決まらない。"""
    organized(round_, "受注.yml", """\
records:
  - concept: c-A
    physical_name: T_ORDER
    source: { anchor: s1-t1 }
""")
    (applied, _), _ = _repair(round_, model)
    assert not applied


def test_壊れたYAMLは直さない(round_: Round, model: Metamodel) -> None:
    """**構文解析できないので、どんな直しも本文の推測になる。**
    引用符を当てる位置を 1 つ間違えると、資料の値が黙って別のものになる。"""
    organized(round_, "受注.yml", """\
records:
  - concept: c-A
    attrs: { note: （固定: 130010） }
""")
    before = (round_.organized / "受注.yml").read_text(encoding="utf-8")
    (applied, refused), path = _repair(round_, model)

    assert not applied and not refused
    assert path.read_text(encoding="utf-8") == before


def test_複数行の値は触らない(round_: Round, model: Metamodel) -> None:
    organized(round_, "受注.yml", """\
records:
  - concept: c-A
    type: エンティティ
    name: 受注
    statement: 受注を保持すること
    description: |
      複数行の
      補足
    source: { anchor: s1-t1 }
""")
    (applied, _), _ = _repair(round_, model)
    assert not applied


# ── 検算 ────────────────────────────────────────────────────────
def test_期待と違う結果は書かない(round_: Round, model: Metamodel,
                                  monkeypatch) -> None:
    """**賢さで安全を担保しない。** 行をいじる実装である以上、器用にやるほど
    壊し方も器用になる ―― 読み直して 1 文字でも違えば書かない。"""
    organized(round_, "受注.yml", """\
records:
  - concept: c-A
    type: エンティティ
    name: 受注
    statement: 受注を保持すること
    physical_name: T_ORDER
    source: { anchor: s1-t1 }
""")
    path = round_.organized / "受注.yml"
    before = path.read_text(encoding="utf-8")

    # 直し方だけを壊す（値を取り違える実装にすり替える）。
    original = fix._move

    def broken(text, marks, index, key, location):
        made = original(text, marks, index, key, location)
        if made is None:
            return None
        return made[0], made[1].replace("T_ORDER", "T_ORDERS")

    monkeypatch.setattr(fix, "_move", broken)
    applied, refused = fix.repair(path, model, "受注.yml")

    assert not applied
    assert [f.code for f in refused] == ["G017"]
    assert "期待した値と違います" in refused[0].message
    assert path.read_text(encoding="utf-8") == before      # 1 文字も変えない


def test_読めなくなる直しも書かない(round_: Round, model: Metamodel,
                                    monkeypatch) -> None:
    organized(round_, "受注.yml", """\
records:
  - concept: c-A
    type: エンティティ
    name: 受注
    statement: 受注を保持すること
    physical_name: T_ORDER
    source: { anchor: s1-t1 }
""")
    path = round_.organized / "受注.yml"
    monkeypatch.setattr(fix, "_move", lambda t, m, i, k, l:
                        (fix.Fix("G008", l, 1, "x"), "records: { 壊れた"))
    applied, refused = fix.repair(path, model, "受注.yml")

    assert not applied
    assert "YAML として読めません" in refused[0].message


def test_改行の種類を保つ(round_: Round, model: Metamodel) -> None:
    """`arp4 declare` が書いたファイルは Windows では CRLF ―― 素直に読み書き
    すると**直した覚えのない差分**が全行に出る。"""
    path = round_.organized / "受注.yml"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(
        "records:\r\n  - concept: c-A\r\n    type: エンティティ\r\n"
        "    name: 受注\r\n    statement: x\r\n    physical_name: T_ORDER\r\n"
        "    source: { anchor: s1-t1 }\r\n".encode("utf-8"))

    applied, _ = fix.repair(path, model, "受注.yml")
    raw = path.read_bytes().decode("utf-8")

    assert len(applied) == 1
    assert raw.count("\r\n") == raw.count("\n")            # 全行 CRLF のまま


# ── CLI ────────────────────────────────────────────────────────
def _run(argv: list[str], monkeypatch) -> tuple[int, str, str]:
    out, err = io.StringIO(), io.StringIO()
    monkeypatch.setattr(sys, "stdout", out)
    monkeypatch.setattr(sys, "stderr", err)
    return cli.main(argv), out.getvalue(), err.getvalue()


_STRAY = """\
records:
  - concept: c-A
    type: エンティティ
    name: 受注
    statement: 受注を保持すること
    physical_name: T_ORDER
    source: { anchor: s1-t1 }
"""


def test_凍結済みのラウンドは直さない(project: Paths, round_: Round,
                                      monkeypatch) -> None:
    """直すと `G009` でハッシュが合わなくなり、**「正本側で直す」という決まりを
    機械が破る**ことになる。"""
    parsed(round_, "受注.md", _PARSED)
    organized(round_, "受注.yml", _STRAY)
    round_.frozen.write_text("frozen_at: '2026-08-02'\nfiles: {}\n",
                             encoding="utf-8")
    before = (round_.organized / "受注.yml").read_text(encoding="utf-8")

    code, _, err = _run(["lint", "--fix", "--root", str(project.root)], monkeypatch)

    assert code == 2
    assert "凍結済み" in err
    assert (round_.organized / "受注.yml").read_text(encoding="utf-8") == before


def test_直したものを必ず言う(project: Paths, round_: Round, monkeypatch) -> None:
    parsed(round_, "受注.md", _PARSED)
    organized(round_, "受注.yml", _STRAY)

    code, out, _ = _run(["lint", "--fix", "--root", str(project.root)], monkeypatch)

    assert code == 0
    assert "[fixed] G008" in out
    assert "physical_name を attrs へ移しました" in out


def test_JSONにも直した一覧が載る(project: Paths, round_: Round,
                                  monkeypatch) -> None:
    parsed(round_, "受注.md", _PARSED)
    organized(round_, "受注.yml", _STRAY)

    _, out, _ = _run(["lint", "--fix", "--root", str(project.root),
                      "--format", "json"], monkeypatch)
    body = json.loads(out)

    assert body["metrics"]["fixed"][0]["code"] == "G008"
    assert body["counts"] == {"error": 0, "warn": 0}        # 直ったので残らない


def test_fixを付けなければ書き換えない(project: Paths, round_: Round,
                                      monkeypatch) -> None:
    parsed(round_, "受注.md", _PARSED)
    organized(round_, "受注.yml", _STRAY)
    before = (round_.organized / "受注.yml").read_text(encoding="utf-8")

    _, out, _ = _run(["lint", "--root", str(project.root)], monkeypatch)

    assert "G008" in out
    assert (round_.organized / "受注.yml").read_text(encoding="utf-8") == before


def test_指定した1ファイルだけ直す(project: Paths, round_: Round,
                                  monkeypatch) -> None:
    parsed(round_, "受注.md", _PARSED)
    parsed(round_, "顧客.md", _PARSED)
    organized(round_, "受注.yml", _STRAY)
    organized(round_, "顧客.yml", _STRAY)

    _run(["lint", "--fix", "--root", str(project.root), "顧客.yml"], monkeypatch)

    assert "attrs" not in (round_.organized / "受注.yml").read_text(encoding="utf-8")
    assert "attrs" in (round_.organized / "顧客.yml").read_text(encoding="utf-8")
