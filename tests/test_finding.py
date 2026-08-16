"""指摘の位置 ―― **どこを開くか**が構造として載っているか。

``target`` に ``資料/A/受注テーブル[3]`` と書いていた頃は、レコードの添字・
アンカー・内部 ID が同じ欄に混ざっていて**どれもエディタから開けなかった**。
ここで見るのは「``file:line`` が正しく付くか」と「付いたものが機械可読か」である。
"""

from __future__ import annotations

from pathlib import Path

import pytest

from arp4 import yamlio
from arp4.finding import Finding, order
from arp4.yamlio import YamlError
from conftest import write


# ── Finding ────────────────────────────────────────────────────
def test_位置が無ければこれまでどおりtargetを出す() -> None:
    """**既存の指摘を壊さない。** 位置を取れない検査（語彙・メタモデル）は残る。"""
    finding = Finding("error", "M001", "エンティティ", "属性がありません")
    assert finding.where == "エンティティ"
    assert finding.render() == "[error] M001 エンティティ: 属性がありません"


def test_位置があればfile_lineで出す() -> None:
    finding = Finding("error", "G006", "s1-t1", "必須の欄がありません",
                      file=".arp/rounds/r001/organized/a.yml", line=47)
    assert finding.where == ".arp/rounds/r001/organized/a.yml:47 s1-t1"
    assert ".arp/rounds/r001/organized/a.yml:47" in finding.render()


def test_何の話かが無ければ位置だけ出す() -> None:
    """ファイル全体の指摘（``G014`` / ``G000``）は target を持たない。"""
    finding = Finding("error", "G014", "", "YAML として壊れています",
                      file=".arp/rounds/r001/organized/a.yml", line=12)
    assert finding.where == ".arp/rounds/r001/organized/a.yml:12"


def test_行が取れなければファイルだけ出す() -> None:
    finding = Finding("error", "G001", "s1-t1", "整理されていません",
                      file=".arp/rounds/r001/parsed/a.md")
    assert finding.where == ".arp/rounds/r001/parsed/a.md s1-t1"


def test_ヒントは2行目に出る() -> None:
    """**次の一手は本文と混ぜない。** 混ぜると何が悪いのかが読み取れなくなる。"""
    finding = Finding("error", "G014", "", "壊れています", hint="引用符で囲む")
    assert finding.render().splitlines() == ["[error] G014 : 壊れています",
                                             "    → 引用符で囲む"]


def test_機械可読の1件は空の欄を並べない() -> None:
    """``null`` が並ぶと、読む側が「位置が取れなかった」と「位置が無い検査」を
    区別するために値を見に行くことになる。**欄ごと落とす。**"""
    assert Finding("warn", "W030", "ent-1", "x").as_dict() == {
        "level": "warn", "code": "W030", "target": "ent-1", "message": "x"}
    assert Finding("warn", "W030", "ent-1", "x", file="a.yml", line=3,
                   hint="h").as_dict() == {
        "level": "warn", "code": "W030", "target": "ent-1", "message": "x",
        "file": "a.yml", "line": 3, "hint": "h"}


def test_同じファイルの指摘は行の順に並ぶ() -> None:
    """直す人はファイルを開いた順に潰す。**並びがファイルを行き来しない。**"""
    findings = [Finding("error", "G006", "z", "x", file="b.yml", line=1),
                Finding("error", "G006", "y", "x", file="a.yml", line=9),
                Finding("error", "G006", "x", "x", file="a.yml", line=2)]
    assert [(f.file, f.line) for f in order(findings)] == [
        ("a.yml", 2), ("a.yml", 9), ("b.yml", 1)]


def test_atで位置を後から付ける() -> None:
    """指摘を組み立てる場所と、位置を知っている場所が離れていることがある。"""
    finding = Finding("error", "G002", "s1-t1", "語彙にありません").at("a.yml", 5)
    assert (finding.file, finding.line) == ("a.yml", 5)
    assert finding.code == "G002"


# ── yamlio.Marks ───────────────────────────────────────────────
_SAMPLE = """\
records:
  - concept: c-A
    type: エンティティ
    source: { anchor: s1-t1 }
  - concept: c-B
    statement: |
      複数行の
      本文
    refs:
      - { rel: has-column, to: c-X }
      - { rel: has-column, to: c-Y }
out_of_scope:
  - { anchor: s9-x1, reason: 表紙 }
"""


def test_要素ごとの行が引ける(tmp_path: Path) -> None:
    path = write(tmp_path / "a.yml", _SAMPLE)
    data, marks = yamlio.load_marked(path)

    assert data == yamlio.load(path)          # 読んだ値には手を入れない
    assert marks.line("records") == 1
    assert marks.line("records", 0) == 2
    assert marks.line("records", 1) == 5
    assert marks.line("records", 1, "refs", 1) == 11
    assert marks.line("out_of_scope", 0) == 13


def test_複数行の値はキーの行を指す(tmp_path: Path) -> None:
    """値の行を採ると 2 行目以降を指し、**どの欄かを探しに上へ戻ることになる。**"""
    _, marks = yamlio.load_marked(write(tmp_path / "a.yml", _SAMPLE))
    assert marks.line("records", 1, "statement") == 6


def test_引けなければ親へ遡る(tmp_path: Path) -> None:
    """深い位置が取れないことは普通にある。**そこで位置を捨てない。**"""
    _, marks = yamlio.load_marked(write(tmp_path / "a.yml", _SAMPLE))
    assert marks.line("records", 0, "attrs", "physical_name") == 2
    assert marks.line("そんな欄は無い") == 1


def test_空ファイルでも落ちない(tmp_path: Path) -> None:
    data, marks = yamlio.load_marked(write(tmp_path / "a.yml", ""))
    assert data is None
    assert marks.line("records") is None


def test_壊れたYAMLは行を持って上がる(tmp_path: Path) -> None:
    """PyYAML は位置を**文章の中**に書く。呼び出し側に正規表現を書かせない。"""
    path = write(tmp_path / "a.yml", "records:\n  - { a: 1 }\n  - { b: (固定: 1) }\n")
    with pytest.raises(YamlError) as caught:
        yamlio.load_marked(path)
    assert caught.value.line == 3

    with pytest.raises(YamlError) as same:
        yamlio.load(path)                     # load も同じ位置を持つ
    assert same.value.line == 3


def test_読んだものは書き戻せる(tmp_path: Path) -> None:
    """行番号を値の側へ埋め込む実装だと ``safe_dump`` がここで落ちる
    ―― ``plan_declare`` は読んだものを書き戻す。"""
    data, _ = yamlio.load_marked(write(tmp_path / "a.yml", _SAMPLE))
    assert yamlio.dumps(data)
