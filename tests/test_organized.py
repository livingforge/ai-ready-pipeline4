"""② 整理結果 ―― **契約違反はここで全部数える**（黙って落とさない）。"""

from __future__ import annotations

from arp4 import organized as organized_module
from arp4.paths import Round
from conftest import codes, organized

_GOOD = """\
records:
  - concept: c-T_ORDER
    type: エンティティ
    name: 受注
    statement: 受注テーブル T_ORDER は受注 1 件を保持すること
    attrs: { physical_name: T_ORDER }
    source: { anchor: s1-t1 }
    refs:
      - { rel: has-column, to: c-受注番号, attrs: { physical_name: ORDER_NO } }
out_of_scope:
  - { anchor: s1-x1, reason: 表紙 }
"""


def test_読めて対応が1対1(round_: Round) -> None:
    organized(round_, "資料/a.xlsx/受注.yml", _GOOD)
    result, findings = organized_module.load(round_)

    assert not findings
    assert len(result.records) == 1
    record = result.records[0]
    assert record.file == "資料/a.xlsx/受注"       # ファイルは対応から決まる
    assert record.anchor == "s1-t1"
    assert record.refs[0].to == "c-受注番号"
    assert result.claimed == {("資料/a.xlsx/受注", "s1-t1"),
                              ("資料/a.xlsx/受注", "s1-x1")}


def test_必須の欄が無ければG006(round_: Round) -> None:
    organized(round_, "a.yml", """\
records:
  - type: エンティティ
    name: 受注
    statement: x
    source: { anchor: s1-t1 }
""")
    _, findings = organized_module.load(round_)
    assert codes(findings) == ["G006"]
    assert "concept" in findings[0].message


def test_対象外に理由が無ければG006(round_: Round) -> None:
    """**黙って飛ばすのを防ぐための宣言**なので、理由の無い宣言は意味がない。"""
    organized(round_, "a.yml", "out_of_scope:\n  - { anchor: s1-x1 }\n")
    _, findings = organized_module.load(round_)
    assert codes(findings) == ["G006"]


def test_refsの書式違反はG000(round_: Round) -> None:
    organized(round_, "a.yml", """\
records:
  - concept: c-a
    type: エンティティ
    name: 受注
    statement: x
    source: { anchor: s1-t1 }
    refs: [{ rel: has-column }]
""")
    _, findings = organized_module.load(round_)
    assert codes(findings) == ["G000"]


def test_見慣れない欄はG008で警告(round_: Round) -> None:
    organized(round_, "a.yml", """\
records:
  - concept: c-a
    type: エンティティ
    name: 受注
    statement: x
    source: { anchor: s1-t1 }
    physical_name: T_ORDER
""")
    result, findings = organized_module.load(round_)
    assert codes(findings) == ["G008"]
    assert len(result.records) == 1          # 落とさない（属性だけ拾えない）


# ── known_gaps（「調べたうえで相手がいない」の宣言） ────────────
_WITH_GAPS = """\
records:
  - concept: c-cst-チャンク数の推移
    type: 制約・前提
    name: 品質保証部のチャンク数の月次推移
    statement: チャンク数は 4 月から 9 月にかけて単調に増えること
    source: { anchor: s4-t1 }
    known_gaps:
      constrains:
        reason: 規模の想定で、縛る先の列がそもそも無い（8 冊に一覧が無い）
        at: 2026-08-16
"""


def test_known_gapsを読む(round_: Round) -> None:
    """**「調べたうえで相手がいない」と「まだ調べていない」を分ける唯一の場所。**

    正本の `known_gaps` は `build` を打った人の欄で、分担しているとき配る側は
    `build` を禁じる ―― 実測（8 冊 / 8 分担 × 2 周）で 80 件超の `G020` が
    2 周とも warn のまま親へ渡り、8 人中 4 人が「宣言する場所が無い」と報告した。
    """
    organized(round_, "a.yml", _WITH_GAPS)
    result, findings = organized_module.load(round_)

    assert not findings
    assert result.records[0].known_gaps == {
        "constrains": {"reason": "規模の想定で、縛る先の列がそもそも無い（8 冊に一覧が無い）",
                       "at": "2026-08-16"}}


def test_known_gapsに理由が無ければG006(round_: Round) -> None:
    """`out_of_scope` の `reason` と同じ規律（正本側の `E019` とも同じ考え方）。

    **理由の無い宣言は、黙って飛ばすのと区別が付かない** ―― `G020` を消すためだけ
    のキーになる。
    """
    organized(round_, "a.yml", _WITH_GAPS.replace(
        "        reason: 規模の想定で、縛る先の列がそもそも無い（8 冊に一覧が無い）\n"
        "        at: 2026-08-16\n", "        at: 2026-08-16\n"))
    result, findings = organized_module.load(round_)

    assert codes(findings) == ["G006"]
    assert findings[0].level == "error"
    assert findings[0].target == "s4-t1"           # 指摘はレコードの話として出す
    # **理由の無い宣言だけを落とす。** レコードは落とさない（資料が黙って消える）。
    assert len(result.records) == 1
    assert result.records[0].known_gaps == {}


def test_known_gapsが連想配列でなければG000(round_: Round) -> None:
    organized(round_, "a.yml", _WITH_GAPS.replace(
        """    known_gaps:
      constrains:
        reason: 規模の想定で、縛る先の列がそもそも無い（8 冊に一覧が無い）
        at: 2026-08-16
""", "    known_gaps: [constrains]\n"))
    _, findings = organized_module.load(round_)

    assert codes(findings) == ["G000"]


_PARSED = """\
# a.xlsx / 表紙

<!-- source: 資料/a.xlsx / シート: 表紙 -->

## セル B2  <!-- a:s1-x1 at=B2 -->

- `B2` 受注管理システム 基本設計書

## セル B4  <!-- a:s1-x2 at=B4 -->

- `B4` 第 1.2 版
"""


def test_一括で対象外を宣言できる(round_: Round) -> None:
    """表紙・改訂履歴は資料の数だけ同じ宣言が要る。**作業は機械にやらせる。**"""
    from conftest import parsed
    parsed(round_, "資料/a.xlsx/表紙.md", _PARSED)
    parsed(round_, "資料/a.xlsx/受注.md", _PARSED.replace("表紙", "受注"))

    plans, findings = organized_module.plan_declare(
        round_, ["表紙"], "表紙（仕様ではない）")
    organized_module.write_declarations(plans)

    assert not findings
    assert [p.file for p in plans] == ["資料/a.xlsx/表紙"]
    result, _ = organized_module.load(round_)
    assert {o.anchor for o in result.out_of_scope} == {"s1-x1", "s1-x2"}
    assert all(o.kind == "対象外" for o in result.out_of_scope)


def test_宣言済みのアンカーは二重に宣言しない(round_: Round) -> None:
    from conftest import parsed
    parsed(round_, "資料/a.xlsx/表紙.md", _PARSED)
    organized(round_, "資料/a.xlsx/表紙.yml",
              "out_of_scope:\n  - { anchor: s1-x1, reason: 表題 }\n")

    plans, _ = organized_module.plan_declare(
        round_, ["*/表紙"], "本体は図形でパース結果に出ていない", kind="未読取")
    organized_module.write_declarations(plans)
    result, _ = organized_module.load(round_)

    assert [p.anchors for p in plans] == [["s1-x2"]]
    assert {(o.anchor, o.kind) for o in result.out_of_scope} == {
        ("s1-x1", "対象外"), ("s1-x2", "未読取")}


def test_参照だけのレコードを書ける(round_: Round) -> None:
    """**日本の設計書は同じ対象を複数シートに書く。**

    アンカーはレコードでしか解決できない（G001）ので、3 つを必須にしていた頃は
    シートの数だけ statement を書かされ、食い違うのが当たり前になっていた。
    """
    organized(round_, "a.yml", """\
records:
  - concept: c-受注入力
    source: { anchor: s6-t1 }
    refs: [{ rel: displays, to: c-受注番号 }]
""")
    result, findings = organized_module.load(round_)

    assert not findings
    record = result.records[0]
    assert not record.complete and record.concept == "c-受注入力"
    assert record.refs[0].to == "c-受注番号"
    assert result.claimed == {("a", "s6-t1")}


def test_中途半端なレコードはG006(round_: Round) -> None:
    """3 つ揃えるか 3 つとも省くか。**書き忘れと「参照だけのつもり」を区別する。**"""
    organized(round_, "a.yml", """\
records:
  - concept: c-a
    type: エンティティ
    source: { anchor: s1-t1 }
""")
    result, findings = organized_module.load(round_)

    assert codes(findings) == ["G006"]
    assert "name" in findings[0].message and "statement" in findings[0].message
    assert result.records == []


def test_壊れたYAMLは全ファイルまとめて出す(round_: Round) -> None:
    """1 件ずつ止めると「直す → また落ちる」を 200 回繰り返すことになる。"""
    organized(round_, "壊れ1.yml",
              "records:\n  - statement: 借方は売掛金（固定: 130010）とすること\n")
    organized(round_, "壊れ2.yml", "records: [{ a: b\n")
    organized(round_, "無事.yml", _GOOD)
    result, findings = organized_module.load(round_)

    assert codes(findings) == ["G014", "G014"]
    assert len(result.records) == 1              # 壊れていないファイルは読めている


def test_横断整理の出力はレコードとして読まない(round_: Round) -> None:
    organized(round_, "_concepts.yml", "new: [{ concept: c-a, type: データ項目 }]\n")
    organized(round_, "_metamodel-add.yml", "add_item_types: [{ name: 帳票レイアウト }]\n")
    result, findings = organized_module.load(round_)

    assert not findings
    assert result.records == []
    assert result.concepts["new"][0]["concept"] == "c-a"
    assert result.metamodel_add["add_item_types"][0]["name"] == "帳票レイアウト"


def test_concepts側も形を検査する(round_: Round) -> None:
    """**記録ファイルとして素通りさせない。**

    レコードには schema と lint があるのに `_concepts.yml` だけ検査手段が無く、
    書いた内容が効いたか分かるのは build を打った後だった。契約の正本は
    `arp4 schema concepts`（schemas/concepts.yml）である。
    """
    organized(round_, "_concepts.yml", "new: [{ type: データ項目 }]\n")   # concept 無し
    _, findings = organized_module.load(round_)

    assert "G006" in codes(findings)


def test_conceptsの打ち間違いは黙って飛ばさない(round_: Round) -> None:
    """build は宣言に無い節を読み飛ばす ―― `asign:` は**守っているつもりで
    守られていない**を作る（known_gaps の E018 と同じ理屈）。"""
    organized(round_, "_concepts.yml", "asign: [{ concept: c-a, aliases_add: [別名] }]\n")
    _, findings = organized_module.load(round_)

    said = [f for f in findings if "asign" in f.message]
    assert said and said[0].level == "warn"


def test_矛盾には争点と両論が要る(round_: Round) -> None:
    """両論の無い矛盾は摘出になっていない（片論は課題に書く）。"""
    organized(round_, "_concepts.yml", """\
contradictions:
  - subject: c-保持期間
""")
    _, findings = organized_module.load(round_)

    said = [f for f in findings if f.code == "G006"]
    assert said and "positions" in said[0].message


def test_アンダースコアで始まるソースの整理結果を読み飛ばさない(round_: Round) -> None:
    """**書いた本人からは正しく書いたようにしか見えない。**

    予約名（`_concepts` / `_metamodel-add`）を避けるための「`_` で始まるものは
    飛ばす」が深いところまで効いていたので、`__main__.py` や `__init__.py` の
    整理結果が**黙って読み飛ばされていた** ―― アンカーは永久に未整理のまま残り、
    `freeze` は「整理も対象外宣言もされていません」と言い続ける。
    """
    organized(round_, "src/arp4/__main__.py.yml", """\
out_of_scope:
  - { anchor: i1, reason: 起動の入口だけで仕様を持たない }
""")
    result, findings = organized_module.load(round_)

    assert not findings
    assert result.claimed == {("src/arp4/__main__.py", "i1")}


def test_直下でもソースの整理結果なら読む(round_: Round) -> None:
    """**`sources/` に `__main__.py` を 1 本だけ入れると直下に来る。**

    深いところの読み飛ばしは直したが、直下は「予約名ではない」と言って飛ばす
    ままだった ―― 整理結果はパース結果と名前で 1:1 なので相方は
    `organized/__main__.py.yml` しかありえず、**書きようが無くなる**。
    判定は名前の形ではなく**相方のパース結果があるか**で決める。
    """
    from conftest import parsed
    parsed(round_, "__main__.py.md", _PARSED)
    organized(round_, "__main__.py.yml", """\
out_of_scope:
  - { anchor: s1-x1, reason: 起動の入口だけで仕様を持たない }
""")
    result, findings = organized_module.load(round_)

    assert not findings
    assert result.claimed == {("__main__.py", "s1-x1")}


def test_declareが書いた整理結果をloadが読む(round_: Round) -> None:
    """**CLI が自分の書いたものを自分で拒否しない。**

    `declare` は `organized/<資料名>.yml` を書くので、資料名が `_` で始まると
    「1 ファイルを書きました」と報告した直後に `load` が読み飛ばしていた ――
    成功報告のあとに無効になるので、何が悪いのか追う手がかりが無い。
    """
    from conftest import parsed
    parsed(round_, "__init__.py.md", _PARSED)

    plans, _ = organized_module.plan_declare(
        round_, ["__init__.py"], "版番号だけで仕様を持たない")
    organized_module.write_declarations(plans)
    result, findings = organized_module.load(round_)

    assert [p.file for p in plans] == ["__init__.py"]
    assert not findings
    assert {o.anchor for o in result.out_of_scope} == {"s1-x1", "s1-x2"}


def test_直下の見慣れない予約名は飛ばすが必ず言う(round_: Round) -> None:
    """打ち間違い（`_concept.yml`）は黙って消える側に倒さない ―― 整理②の
    横断結果がまるごと無かったことになり、気づく手がかりが 1 つも残らない。

    **相方のパース結果が無い**ので資料の整理結果ではない、と分かる。
    """
    organized(round_, "_concept.yml", "records: []")
    result, findings = organized_module.load(round_)

    assert codes(findings) == ["G032"]
    assert not result.records


# ── 指摘の位置 ──────────────────────────────────────────────────
_POSITIONS = """\
records:
  - concept: c-A
    type: エンティティ
    name: 受注
    statement: 受注は 1 件を保持すること
    source: { anchor: s1-t1 }
    refs:
      - { rel: has-column, to: c-X }
      - { rel: has-column, to: c-Y }
  - concept: c-B
    source: { anchor: s1-t2 }
    description: 直下に書いてしまった欄
out_of_scope:
  - { anchor: s1-x1, kind: 対象外外, reason: 表紙 }
"""


def test_レコードは開ける位置を持つ(round_: Round) -> None:
    """``file`` は 1:1 対応の鍵、``path`` は**開くため**の位置。畳まない。"""
    organized(round_, "資料/a.xlsx/受注.yml", _POSITIONS)
    result, _ = organized_module.load(round_)

    first, second = result.records
    assert first.file == "資料/a.xlsx/受注"          # 拡張子なし（出典の鍵）
    assert first.path == ".arp/rounds/2026-08-02/organized/資料/a.xlsx/受注.yml"
    assert (first.line, second.line) == (2, 10)
    assert [r.line for r in first.refs] == [8, 9]


def test_見慣れない欄はその欄の行を指す(round_: Round) -> None:
    """レコードの先頭行を指すと、20 行のレコードでは目で探し直すことになる。"""
    organized(round_, "a.yml", _POSITIONS)
    _, findings = organized_module.load(round_)

    stray = [f for f in findings if f.code == "G008"]
    assert (stray[0].line, stray[0].target) == (12, "s1-t2")


def test_対象外の区分違反はkindの行を指す(round_: Round) -> None:
    organized(round_, "a.yml", _POSITIONS)
    _, findings = organized_module.load(round_)

    kind = [f for f in findings if f.code == "G011"]
    assert (kind[0].line, kind[0].target) == (14, "s1-x1")


def test_壊れたYAMLは行と直し方を出す(round_: Round) -> None:
    """実測でいちばん多いのは ``:``＋空白（「（固定: 130010）」）である。
    **どこが壊れているかだけでは直せない**ので、原因の候補まで言う。"""
    organized(round_, "a.yml", """\
records:
  - concept: c-A
    attrs: { note: （固定: 130010） }
""")
    _, findings = organized_module.load(round_)

    assert codes(findings) == ["G014"]
    assert findings[0].line == 3
    assert findings[0].file == ".arp/rounds/2026-08-02/organized/a.yml"
    assert "ブロック記法" in (findings[0].hint or "")


def test_語彙の追加提案も形を検査する(round_: Round) -> None:
    """**隣のファイルだけが素通りしていた。**

    ``freeze`` が読むのは ``add_item_types`` の 1 節だけで、``add_attributes:`` /
    ``add_relations:`` と書いたものは error も warn も無いまま読み飛ばされる。
    「既存種別に属性を足したい」「関係の組み合わせを足したい」は語彙の穴として
    最も普通の形なのに、書ける場所が無いことがどこにも書いていなかった ――
    実測（sales-corpus・11 ロットの分担）で 3 つのロットが独立に踏み、うち 1 人は
    ``add_item_types`` に**嘘の種別名**で載せて機械に数えさせていた。

    **受けられない欄はコメントではなく機械が数える場所へ**、とこの道具は繰り返し
    言っている。その道具自身が、機械が数えない書き方を黙って通していた。
    """
    organized(round_, "_metamodel-add.yml", """\
add_item_types:
  - name: 宿題事項
add_attributes:
  - target: open-issue
""")
    result, findings = organized_module.load(round_)

    warned = [f for f in findings if f.level == "warn"]
    assert warned and "add_attributes" in warned[0].message
    # **読める節はそのまま読める**（提案 1 件は生きている）。
    assert result.metamodel_add["add_item_types"][0]["name"] == "宿題事項"
