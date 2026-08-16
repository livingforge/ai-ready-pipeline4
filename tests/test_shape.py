"""形の検査 ―― **宣言（`schemas/*.yml`）が正で、実装は実行するだけ。**

手続きで書いていたころ、整理結果の契約は `organized.py` の関数の中と
`docs/organize.md` の例示の 2 か所にあった。**例示は実行されない**ので、古く
なっても誰も気づかない。ここで見るのは「宣言だけを直せば検査が変わるか」
―― 変わらないなら、規則はまだ 2 か所にある。
"""

from __future__ import annotations

import io
import sys

import pytest

from arp4 import cli, organized as organized_module, shape, yamlio
from arp4.paths import Round
from conftest import codes, organized


# ── 宣言が正であること ──────────────────────────────────────────
def test_宣言を差し替えれば検査が変わる(round_: Round, monkeypatch) -> None:
    """**これが「正である」の意味である。** 実装に触れずに規則を変えられないなら、
    どこかにもう 1 つ規則が残っている。"""
    organized(round_, "a.yml", """\
records:
  - concept: c-A
    type: エンティティ
    name: 受注
    statement: x
    source: { anchor: s1-t1 }
""")
    assert not organized_module.load(round_)[1]        # いまは通る

    tightened = shape.load()
    record = {**tightened["shapes"]["record"]}
    record["required"] = [*record["required"],
                          {"keys": ["owner"], "code": "G006",
                           "message": "必須の欄がありません: {missing}"}]
    monkeypatch.setitem(shape._CACHE, "organized",
                        {**tightened, "shapes": {**tightened["shapes"],
                                                 "record": record}})

    _, findings = organized_module.load(round_)
    assert codes(findings) == ["G006"]
    assert "owner" in findings[0].message


def test_コードも文言も宣言が持つ() -> None:
    """実装が持っているとスキーマを読んでも「破ると何が出るか」が分からない。"""
    schema = shape.load()
    assert schema["shapes"]["out_of_scope"]["keys"]["kind"]["code"] == "G011"
    assert schema["shapes"]["record"]["unknown"] == {
        "code": "G008", "level": "warn",
        "message": "レコード直下の見慣れない欄です: {keys}（属性は attrs に入れてください）"}


def test_arp4_schemaは畳まず原文を出す(monkeypatch) -> None:
    """**なぜそう決めたかはコメントにしか無い。** 値だけ出すと「何を書けばよいか」
    は分かっても「なぜ 3 つ揃えるのか」が落ちる。"""
    out = io.StringIO()
    monkeypatch.setattr(sys, "stdout", out)
    assert cli.main(["schema"]) == 0

    text = out.getvalue()
    assert "3 つ揃えるか" in text                       # コメントが残っている
    assert "together: true" in text
    assert yamlio.dumps(shape.load())                   # 値としても読める


# ── error は落とし、warn は落とさない ───────────────────────────
def test_errorを出した要素は組み立てない(round_: Round) -> None:
    organized(round_, "a.yml", """\
records:
  - concept: c-A
    source: { anchor: s1-t1 }
  - type: エンティティ
    name: 受注
    statement: x
    source: { anchor: s1-t2 }
""")
    result, findings = organized_module.load(round_)

    assert codes(findings) == ["G006"]                  # concept が無い 2 件目
    assert [r.anchor for r in result.records] == ["s1-t1"]


def test_warnを出した要素は落とさない(round_: Round) -> None:
    """「書けているが置き場所が違う」ものを捨てると、**資料が黙って消える。**"""
    organized(round_, "a.yml", """\
records:
  - concept: c-A
    type: エンティティ
    name: 受注
    statement: x
    source: { anchor: s1-t1 }
    physical_name: T_ORDER
""")
    result, findings = organized_module.load(round_)

    assert codes(findings) == ["G008"]
    assert [r.anchor for r in result.records] == ["s1-t1"]


def test_落ちた要素のぶんを詰めない(round_: Round) -> None:
    """``records[0]`` が落ちたときに 2 番目が 0 番になると、指摘の位置と
    整理結果の行がずれて、直す人が**別のレコードを見る**ことになる。"""
    organized(round_, "a.yml", """\
records:
  - source: { anchor: s1-t1 }
  - concept: c-B
    source: { anchor: s1-t2 }
""")
    result, _ = organized_module.load(round_)

    assert [r.index for r in result.records] == [1]
    assert result.records[0].line == 3


# ── 契約そのもの ────────────────────────────────────────────────
def test_3つ揃えるか3つとも省くか(round_: Round) -> None:
    body = """\
records:
  - concept: c-A
    source: { anchor: s1-t1 }
"""
    organized(round_, "a.yml", body)
    assert not organized_module.load(round_)[1]         # 3 つとも省く ―― 参照だけ

    organized(round_, "a.yml", body.replace(
        "    source:", "    type: エンティティ\n    source:"))
    _, findings = organized_module.load(round_)
    assert codes(findings) == ["G006"]
    assert "name" in findings[0].message and "statement" in findings[0].message


def test_配列でないものを配列として読まない(round_: Round) -> None:
    """手続き版は ``out_of_scope`` が配列かを見ておらず、連想配列を渡すと
    **キーを 1 件ずつ回して**「書式が不正」を並べていた。"""
    organized(round_, "a.yml", "out_of_scope:\n  s1-x1: 表紙\n")
    _, findings = organized_module.load(round_)

    assert codes(findings) == ["G000"]
    assert findings[0].message == "out_of_scope は配列でなければなりません"


def test_根の見慣れないキーは黙って通す(round_: Round) -> None:
    """整理結果に注記を書く人がいて、**それは資料の側の都合**である
    （レコードの中とは事情が違う）。"""
    organized(round_, "a.yml", "note: この資料は旧版\nrecords: []\n")
    assert not organized_module.load(round_)[1]


# ── 位置 ────────────────────────────────────────────────────────
def test_指摘は宣言どおりの欄を指す(round_: Round) -> None:
    organized(round_, "a.yml", """\
records:
  - concept: c-A
    source: { anchor: s1-t1 }
    description: 直下に書いた
out_of_scope:
  - { anchor: s1-x1, reason: 表紙, kind: そんな区分 }
""")
    _, findings = organized_module.load(round_)
    where = {f.code: (f.line, f.target) for f in findings}

    assert where["G008"] == (4, "s1-t1")               # その欄の行
    assert where["G011"] == (6, "s1-x1")               # kind の行


def test_関係の指摘はレコードの話として出す(round_: Round) -> None:
    """関係 1 本に名前は無い。読み手が次に開くのはパース結果のアンカーである。"""
    organized(round_, "a.yml", """\
records:
  - concept: c-A
    type: エンティティ
    name: 受注
    statement: x
    source: { anchor: s1-t1 }
    refs:
      - { rel: has-column, to: c-x }
      - { rel: has-column }
""")
    result, findings = organized_module.load(round_)

    assert [(f.code, f.target, f.line) for f in findings] == [("G000", "s1-t1", 9)]
    assert [r.to for r in result.records[0].refs] == ["c-x"]


def test_スキーマは1度だけ読む() -> None:
    """200 ファイルの読み込みで 200 回開かない。"""
    shape.load()
    assert "organized" in shape._CACHE
    missing = shape.SCHEMAS / "organized.yml"
    assert missing.is_file()                            # 梱包されている


def test_知らないスキーマ名は落ちる() -> None:
    with pytest.raises((FileNotFoundError, OSError, yamlio.YamlError)):
        shape.text("そんなスキーマ")


def test_declareの選択肢もスキーマから引く() -> None:
    """検査の表と `arp4 declare --kind` の選択肢が別々に書かれていると、
    **CLI が自分の書いたものを自分で拒否する**（`G032` と同じ形の事故）。"""
    declared = shape.load()["shapes"]["out_of_scope"]["keys"]["kind"]["values"]

    assert list(organized_module.SCOPE_KINDS) == declared
    assert organized_module.SCOPE_DEFAULT == declared[0]


def test_根が連想配列でなければ中身を読みに行かない(round_: Round) -> None:
    """落ちた根の中を読むと、既に壊れているものへ重ねて指摘を出すだけになる。"""
    organized(round_, "a.yml", "- これは配列\n")
    result, findings = organized_module.load(round_)

    assert codes(findings) == ["G000"]
    assert findings[0].message.startswith("整理結果は records / out_of_scope")
    assert not result.records
