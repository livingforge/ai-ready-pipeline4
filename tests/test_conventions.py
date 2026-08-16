"""コード整理の規約（G022〜G025）―― **lint が許す自由度＝ブレの発生源**。

r001 で整理を 4 エージェントに分担したとき、規約に無い判断（calls の粒度・
dunder の起票・package の根拠・関数の塊が無いファイルの出典）が人数分に割れた。
割れた箇所は規則が未成文だった箇所と正確に一致するので、organize.md の文書化に
加えて機械の検査に落とす（→ 作業計画書 Phase 0-5）。

検体は**1 つの欠陥を 1 つのファイルで突く** ―― わざと規約を破った整理結果に
lint / freeze が指摘を出し、規約どおりの整理結果には出さないことを両方見る。
"""

from __future__ import annotations

from arp4 import freeze
from arp4.metamodel import Metamodel
from arp4.paths import Round
from conftest import codes, organized, parsed

#: コードのパース結果の検体。クラス 1 つ（dunder 入り）と取り込みの塊を持つ。
_CODE = """\
# src/x.py

<!-- source: src/x.py -->

## クラス: Foo  <!-- a:m1 at=src/x.py#L1-L10 -->

| メンバ | 種類 | 注釈 | シグネチャ | 戻り値 | 例外 | 行 |
|---|---|---|---|---|---|---|
| Foo | クラス |  | class Foo |  |  | 1 |
| __init__ | メソッド |  | __init__(self) |  |  | 2 |
| run | メソッド |  | run() | int |  | 5 |

## 取り込み  <!-- a:i1 at=src/x.py -->

| 取り込み | 元 | 名前 | 行 |
|---|---|---|---|
| from y import helper | y | helper | 1 |
"""


def _gate(round_: Round, model: Metamodel, body: str) -> list[str]:
    parsed(round_, "src/x.py.md", _CODE)
    organized(round_, "src/x.py.yml", body)
    return codes(freeze.gate(round_, model, {}).findings)


# ── ① calls はモジュールへ畳む（G022） ──────────────────────────
def test_取り込みから張るcallsの相手がメソッドならG022(
        round_: Round, model: Metamodel) -> None:
    said = _gate(round_, model, """\
records:
  - concept: c-mod-src.x
    type: モジュール
    name: x
    statement: x は取り込みの検体であること
    source: { anchor: i1 }
    refs:
      - { rel: calls, to: c-mtd-src.y.helper }
  - concept: c-mtd-src.y.helper
    type: メソッド
    name: helper
    statement: helper は補助の関数であること
    source: { anchor: m1 }
""")
    assert "G022" in said


def test_取り込みから張るcallsの相手がモジュールなら黙る(
        round_: Round, model: Metamodel) -> None:
    said = _gate(round_, model, """\
records:
  - concept: c-mod-src.x
    type: モジュール
    name: x
    statement: x は取り込みの検体であること
    source: { anchor: i1 }
    refs:
      - { rel: calls, to: c-mod-src.y }
  - concept: c-mod-src.y
    type: モジュール
    name: y
    statement: y は相手のモジュールであること
    source: { anchor: m1 }
    attrs: { class_name: src.y.Y }
""")
    assert "G022" not in said


# ── ② dunder は起こさない（G023） ───────────────────────────────
def test_dunderをメソッドに起こしたらG023(round_: Round, model: Metamodel) -> None:
    said = _gate(round_, model, """\
records:
  - concept: c-mtd-src.x.Foo.__init__
    type: メソッド
    name: __init__
    statement: __init__ は初期化すること
    source: { anchor: m1 }
  - concept: c-mod-src.x
    type: モジュール
    name: x
    statement: x は検体のモジュールであること
    source: { anchor: i1 }
""")
    assert "G023" in said


def test_公開メソッドはG023にしない(round_: Round, model: Metamodel) -> None:
    said = _gate(round_, model, """\
records:
  - concept: c-mtd-src.x.Foo.run
    type: メソッド
    name: run
    statement: run は処理を実行すること
    source: { anchor: m1 }
  - concept: c-mod-src.x
    type: モジュール
    name: x
    statement: x は検体のモジュールであること
    source: { anchor: i1 }
""")
    assert "G023" not in said


# ── ③ package は路から取る（G024） ──────────────────────────────
def test_路に無いpackageは根拠が無ければG024(round_: Round,
                                             model: Metamodel) -> None:
    said = _gate(round_, model, """\
records:
  - concept: c-mod-src.x
    type: モジュール
    name: x
    statement: x は検体のモジュールであること
    attrs: { package: arp9 }
    source: { anchor: i1 }
  - concept: c-mod-src.x-m1
    source: { anchor: m1 }
""")
    assert "G024" in said


def test_路にあるpackageは通る(round_: Round, model: Metamodel) -> None:
    said = _gate(round_, model, """\
records:
  - concept: c-mod-src.x
    type: モジュール
    name: x
    statement: x は検体のモジュールであること
    attrs: { package: src }
    source: { anchor: i1 }
  - concept: c-mod-src.x-m1
    source: { anchor: m1 }
""")
    assert "G024" not in said


def test_越境のpackageも根拠を書けば通る(round_: Round, model: Metamodel) -> None:
    """**越境そのものは禁じない**（0-5「package の越境根拠は可」）。"""
    said = _gate(round_, model, """\
records:
  - concept: c-mod-src.x
    type: モジュール
    name: x
    statement: x は検体のモジュールであること
    attrs: { package: arp9,
             description: 配布名は arp9（路の src は開発リポジトリの都合） }
    source: { anchor: i1 }
  - concept: c-mod-src.x-m1
    source: { anchor: m1 }
""")
    assert "G024" not in said


# ── ④ ファイル単位のモジュールの出典（G025） ────────────────────
def test_ファイル単位のモジュールがクラスの塊を指したらG025(
        round_: Round, model: Metamodel) -> None:
    said = _gate(round_, model, """\
records:
  - concept: c-mod-src.x
    type: モジュール
    name: x
    statement: x は検体のモジュールであること
    source: { anchor: m1 }
  - concept: c-mod-src.x
    source: { anchor: i1 }
""")
    assert "G025" in said


def test_ファイル単位のモジュールがi1を指せば通る(round_: Round,
                                                  model: Metamodel) -> None:
    said = _gate(round_, model, """\
records:
  - concept: c-mod-src.x
    type: モジュール
    name: x
    statement: x は検体のモジュールであること
    source: { anchor: i1 }
  - concept: c-mod-src.x
    source: { anchor: m1 }
""")
    assert "G025" not in said


def test_クラスのモジュールはクラスの塊を指してよい(round_: Round,
                                                    model: Metamodel) -> None:
    """``class_name`` を持つレコードはクラス単位 ―― 出典がクラスの塊なのは正しい。"""
    said = _gate(round_, model, """\
records:
  - concept: c-mod-src.x.Foo
    type: モジュール
    name: x.Foo
    statement: Foo は検体のクラスであること
    attrs: { class_name: src.x.Foo }
    source: { anchor: m1 }
  - concept: c-mod-src.x.Foo
    source: { anchor: i1 }
""")
    assert "G025" not in said


# ── Excel の資料では鳴らない ────────────────────────────────────
def test_シートのアンカーには規約を掛けない(round_: Round,
                                            model: Metamodel) -> None:
    parsed(round_, "資料/a.xlsx/一覧.md", """\
# a.xlsx / 一覧

<!-- source: 資料/a.xlsx / シート: 一覧 -->

## 表 B5:H8  <!-- a:s1-t1 at=B5:H8 -->

| 名前 |
|---|
| __init__ |
""")
    organized(round_, "資料/a.xlsx/一覧.yml", """\
records:
  - concept: c-mtd-x.__init__
    type: メソッド
    name: __init__
    statement: __init__ は初期化のメソッドとして資料に載っていること
    source: { anchor: s1-t1 }
""")
    said = codes(freeze.gate(round_, model, {}).findings)
    assert "G023" not in said

    # lint も同じ関数を通る（規則が 2 つあると同じ問題が形を変えて戻る）。
    assert "G023" not in codes(freeze.lint(round_, model, {}).findings)
