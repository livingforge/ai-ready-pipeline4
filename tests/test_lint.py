"""``arp4 lint`` ―― **書いている最中に回せる検査**（``freeze`` の部分集合）。

``freeze --dry-run`` は 200 ファイルを読んでゲート 5 条件を見るので、1 ファイル
書いた直後には重く、しかも出てくるのは未整理数百件の山で**いま書いた 1 件が
埋もれる**。ここで見るのは「1 ファイルだけで決まるものを出すか」と
「**決まらないものを出さないか**」の両方である。
"""

from __future__ import annotations

import io
import json
import sys

from arp4 import cli, concepts as concepts_module, freeze
from arp4 import metamodel as mm
from arp4 import pack
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

## セル B2  <!-- a:s1-x1 at=B2 -->

- `B2` 受注テーブル定義書
"""

_GOOD = """\
records:
  - concept: c-受注番号
    type: データ項目
    name: 受注番号
    statement: 受注番号は文字列型の項目であること
    source: { anchor: s1-t1 }
out_of_scope:
  - { anchor: s1-x1, reason: 表題 }
"""


def _setup(round_: Round, body: str = _GOOD, name: str = "資料/a.xlsx/受注テーブル") -> None:
    parsed(round_, f"{name}.md", _PARSED)
    organized(round_, f"{name}.yml", body)


def test_1ファイルだけで決まるものは出す(round_: Round, model: Metamodel) -> None:
    _setup(round_, _GOOD.replace("    type: データ項目\n", "    type: 帳票レイアウト\n"))
    report = freeze.lint(round_, model, {})

    assert codes(report.findings) == ["G002"]
    assert report.findings[0].line == 3          # その欄の行を指す


def test_横断が要るものは出さない(round_: Round, model: Metamodel) -> None:
    """**出せないものを出せるふりをしない。** 未整理（``G001``）も concept の
    実在（``G003``）も 1 ファイルでは決まらない ―― ここで黙るのが正しい。
    """
    parsed(round_, "資料/a.xlsx/受注テーブル.md", _PARSED)   # 整理結果が無い
    parsed(round_, "資料/b.xlsx/顧客.md", _PARSED)
    organized(round_, "資料/a.xlsx/受注テーブル.yml", _GOOD.replace(
        "    source: { anchor: s1-t1 }",
        "    source: { anchor: s1-t1 }\n    refs: [{ rel: has-column, to: c-いない }]"))

    said = codes(freeze.lint(round_, model, {}).findings)

    assert "G001" not in said                    # 未整理は freeze の仕事
    assert "G003" not in said                    # concept の実在も同じ
    assert not said


def test_アンカーの実在は1対1なので見る(round_: Round, model: Metamodel) -> None:
    """相方のパース結果は名前で 1 つに決まるので、他のファイルを読まずに見られる。
    **幻覚の最頻形は「存在しない出典」**なので、ここで潰せるのは大きい。"""
    _setup(round_, _GOOD.replace("anchor: s1-t1", "anchor: s9-t9"))
    report = freeze.lint(round_, model, {})

    assert codes(report.findings) == ["G004"]
    assert report.findings[0].file.endswith("受注テーブル.yml")


def test_指定した1ファイルだけを読む(round_: Round, model: Metamodel) -> None:
    _setup(round_)
    _setup(round_, _GOOD.replace("    type: データ項目\n", "    type: 帳票レイアウト\n"),
           name="資料/b.xlsx/顧客")

    target = round_.organized / "資料/b.xlsx/顧客.yml"
    report = freeze.lint(round_, model, {}, only=[target])

    assert report.metrics["files"] == 1
    assert codes(report.findings) == ["G002"]


def test_freezeと同じ関数で判定する(round_: Round, model: Metamodel) -> None:
    """規則が 2 つあると、``G002`` が ``B013`` を取りこぼしていたのと同じ事故が
    lint と freeze のあいだで起きる。**lint が出したものは freeze も出す。**"""
    _setup(round_, _GOOD.replace("    type: データ項目\n", "    type: 帳票レイアウト\n"))

    linted = freeze.lint(round_, model, {}).findings
    gated = freeze.gate(round_, model, {}).findings

    assert {(f.code, f.file, f.line) for f in linted} <= {
        (f.code, f.file, f.line) for f in gated}


# ── CLI ────────────────────────────────────────────────────────
def _run(argv: list[str], monkeypatch) -> tuple[int, str, str]:
    out, err = io.StringIO(), io.StringIO()
    monkeypatch.setattr(sys, "stdout", out)
    monkeypatch.setattr(sys, "stderr", err)
    code = cli.main(argv)
    return code, out.getvalue(), err.getvalue()


def test_当たらないパスは黙って落とさない(project: Paths, round_: Round,
                                          monkeypatch) -> None:
    """黙って落とすと「lint が通った」と「そもそも検査していない」が同じ顔に
    なる ―― **打ち間違い 1 つで、直したファイルが検査されないまま緑になる。**"""
    _setup(round_)
    code, _, err = _run(["lint", "--root", str(project.root), "資料/そんな.yml"],
                        monkeypatch)

    assert code == 2
    assert "見つかりません" in err


def test_lintもJSONで出せる(project: Paths, round_: Round, monkeypatch) -> None:
    _setup(round_, _GOOD.replace("    type: データ項目\n", "    type: 帳票レイアウト\n"))
    code, out, _ = _run(["lint", "--root", str(project.root), "--format", "json"],
                        monkeypatch)
    body = json.loads(out)

    assert code == 1
    assert body["command"] == "lint"
    assert body["findings"][0]["code"] == "G002"
    assert body["metrics"]["files"] == 1


def test_正本を読まずに回る(project: Paths, round_: Round, monkeypatch) -> None:
    """**速さが要件である。** 正本（items / relations）は lint の判定に 1 つも
    使わないので、読むと 300 件のアイテムを毎回舐めることになる。"""
    _setup(round_)
    calls: list[str] = []
    monkeypatch.setattr(cli.spec_module, "load",
                        lambda *a, **k: calls.append("spec") or (_ for _ in ()).throw(
                            AssertionError("正本を読んではいけません")))

    code, _, _ = _run(["lint", "--root", str(project.root)], monkeypatch)

    assert code == 0
    assert not calls


def test_通っても凍結できるとは限らないと言う(project: Paths, round_: Round,
                                              monkeypatch) -> None:
    """lint は部分集合なので、**緑を凍結の合図と読ませない。**"""
    _setup(round_)
    _, out, _ = _run(["lint", "--root", str(project.root)], monkeypatch)

    assert "arp4 freeze --dry-run" in out


def test_メタモデルは読む(round_: Round) -> None:
    """語彙の検査（``G002``）に要るので、これだけは読む。"""
    model, findings = mm.load(round_.root / ".arp" / "spec" / "metamodel.yml")
    assert not [f for f in findings if f.level == "error"]
    assert model.item_types

    known, _ = concepts_module.load(Paths(round_.root))
    assert known == {}


def test_出典の欄の取りこぼしはlintでも出る(round_: Round, model: Metamodel) -> None:
    """**1 ファイルだけで決まる**（相方のパース結果は名前で 1 つに決まる）。

    凍結まで持ち越すと、出るのは未整理数百件の山の中の warn 1 件になる ――
    書いている最中に言えば、その場で資料を見直せる。
    """
    parsed(round_, "yamlio.py.md",
           "# yamlio.py\n\n<!-- source: yamlio.py -->\n\n"
           "## モジュール関数  <!-- a:m1 at=yamlio.py#L141 -->\n\n"
           "| メンバ | 種類 | 注釈 | シグネチャ | 戻り値 | 例外 | 行 |\n"
           "|---|---|---|---|---|---|---|\n"
           "| marked | 関数 |  | marked(text: str) | Any | _broken | 141 |\n")
    organized(round_, "yamlio.py.yml",
              "records:\n"
              "  - concept: c-mtd-yamlio-marked\n"
              "    type: メソッド\n"
              "    name: yamlio.marked\n"
              "    statement: marked は文字列の YAML を読み込むこと\n"
              "    attrs: { signature: \"yamlio.marked(text: str)\", returns: Any }\n"
              "    source: { anchor: m1 }\n")

    report = freeze.lint(round_, model, {}, only=[round_.organized / "yamlio.py.yml"])

    assert "G018" in codes(report.findings)



def test_閉じたenumの値が語彙外ならlintで言う(round_: Round, model: Metamodel) -> None:
    """**値の検査が ``check`` にしか無かった。**

    出るのは ``freeze`` の後 ―― そのときには整理結果はもう編集できない（直すには
    正本を ``overridden`` で上書きするか、ラウンドを起こし直すしかない）。
    属性の**名前**（``G016``）は書いている最中に言うのに、**値**だけが凍結の
    向こう側にあるのは筋が通らない。しかも取り違えは意味の判断ではなく綴りの
    問題なので、書いている最中に言えば直る。

    ``extensible`` でない enum に限る（→ 次のテスト）。
    """
    _setup(round_, _GOOD.replace("    type: データ項目\n",
                                 "    type: インデックス\n")
                        .replace("    statement: 受注番号は文字列型の項目であること\n",
                                 "    statement: 受注番号で一意に引けること\n"
                                 "    attrs: { uniqueness: ユニーク }\n"))

    report = freeze.lint(round_, model, {})

    assert "G028" in codes(report.findings)
    found = [f for f in report.findings if f.code == "G028"][0]
    assert found.level == "warn"          # build は落ちない ―― 段は上げない
    assert "ユニーク" in found.message and "一意" in found.message


def test_関係の属性の値も見る(round_: Round, model: Metamodel) -> None:
    """項目だけでなく **関係の属性**（``displays.io`` 等）も見る。

    画面項目表の「種別」列（表示 / 入力 / 選択）を ``io``（入力 / 出力 / 入出力）へ
    写す作業は分担の全員がやる ―― 資料の語をそのまま置くと、正本では
    ``build`` が黙って受け、``check`` が凍結の後で error にする。
    """
    _setup(round_, _GOOD.replace(
        "    source: { anchor: s1-t1 }\n",
        "    source: { anchor: s1-t1 }\n"
        "  - concept: c-scr-受注入力\n"
        "    type: 画面\n"
        "    name: 受注入力\n"
        "    statement: 受注を登録できること\n"
        "    source: { anchor: s1-t1 }\n"
        "    refs:\n"
        "      - { rel: displays, to: c-受注番号, attrs: { io: 表示 } }\n"))

    report = freeze.lint(round_, model, {})

    found = [f for f in report.findings if f.code == "G028"]
    assert found and "displays.io" in found[0].message


def test_extensibleなenumは語彙外でも言わない(round_: Round, model: Metamodel) -> None:
    """**寄せ先を増やしてよい語彙**では、宣言に無い値が正しい整理でありうる。

    ``data-item.data_type`` が ``extensible`` なのは、日本のテーブル定義書が書く
    ``CHAR`` / ``DECIMAL`` をそのまま採れるようにするためである ―― ここを鳴らすと、
    規律（資料が言っていない値を書かない）を守った側が鳴らされる。
    """
    _setup(round_, _GOOD.replace(
        "    statement: 受注番号は文字列型の項目であること\n",
        "    statement: 受注番号は文字列型の項目であること\n"
        "    attrs: { data_type: CHAR }\n"))

    report = freeze.lint(round_, model, {})

    assert "G028" not in codes(report.findings)


def test_フォルダを渡せる(project: Paths, round_: Round, monkeypatch) -> None:
    """**分担で配る単位は 1 ブック（＝1 フォルダ）である。**

    「1 ファイル書くたびに打て」という手順書に対して、書き上げた 1 冊をまとめて
    確かめる手段が無かった（実測で 8 ロット中 6 つが ``exit 2`` を報告した）。
    ファイルを 1 本ずつ並べる回避策は、日本の設計書のシート名に**空白が普通に
    入る**ためシェルで壊れやすく、**壊れても「lint は通った」と同じ顔で終わる。**
    """
    _setup(round_, name="資料/a.xlsx/受注テーブル")
    _setup(round_, name="資料/a.xlsx/受注明細")

    code, out, _ = _run(["lint", "--root", str(project.root), "資料/a.xlsx"],
                        monkeypatch)

    assert code == 0
    assert "整理結果 2 ファイル" in out


# ── 台帳への提案（`_concepts.yml`）を検査したことが分かるか ────
#
# `_concepts.yml` は**レコードではない**（整理②の出力）ので、`files` にも
# `records` にも `out_of_scope` にも入らない ―― それだけを渡すと出力が
# 「整理結果 0 ファイル / レコード 0 / 対象外 0 / error 0 / warn 0」になり、
# **検査した結果が白なのか、そもそも読まれなかったのかが打った人から区別
# できなかった。** しかもフォルダを渡す打ち方（手順書の既定）では、本当に
# 一度も読まれていなかった。
_BAD_CONCEPTS = "new: [{ concept: c-x, type: 存在しない型 }]\n"


def test__concepts_ymlだけを渡しても検査したと言う(project: Paths, round_: Round,
                                                    monkeypatch) -> None:
    """**0 が並ぶ画面に「検査した」と書いていなかった。**

    ここが無いと、`docs/reconcile.md` が言う「書いたら `arp4 lint _concepts.yml`
    で検査できる」を実行した人が、検査されたのかどうか判断できない。
    """
    _setup(round_)
    organized(round_, "_concepts.yml", "new: []\n")

    code, out, _ = _run(["lint", "--root", str(project.root), "_concepts.yml"],
                        monkeypatch)

    assert code == 0
    assert "_concepts.yml を検査しました" in out


def test_フォルダを渡すと_concepts_ymlも検査する(project: Paths, round_: Round,
                                                  monkeypatch) -> None:
    """**手順書の既定の打ち方で、一度も読まれていなかった。**

    フォルダの展開が :func:`arp4.organized.yaml_files`（``--fix`` が書き換えて
    よい対象）を使っていたので予約名が落ち、`G002`（`new` の型が語彙に無い）も
    `G021`（`assign` の相手が台帳に無い）も**黙って出なかった** ―― 出ないことと
    通ったことが同じ顔になる、いちばん追えない壊れ方である。
    """
    _setup(round_)
    organized(round_, "_concepts.yml", _BAD_CONCEPTS)

    code, out, _ = _run(["lint", "--root", str(project.root),
                         str(round_.organized)], monkeypatch)

    assert code == 1
    assert "G002" in out
    assert "_concepts.yml を検査しました" in out


def test__concepts_ymlはfixが書き換えない(project: Paths, round_: Round,
                                           monkeypatch) -> None:
    """**検査の対象と、書き換えてよい対象は別物である。**

    予約名を検査へ入れたぶん、``--fix`` へ流れ込まないことを対で押さえる ――
    :func:`arp4.fix.repair` が知っているのは ``records:`` の形だけである。
    """
    _setup(round_)
    path = organized(round_, "_concepts.yml", _BAD_CONCEPTS)

    _run(["lint", "--root", str(project.root), str(round_.organized), "--fix"],
         monkeypatch)

    assert path.read_text(encoding="utf-8") == _BAD_CONCEPTS


def test_中身が空のフォルダは当たらなかった側にする(project: Paths, round_: Round,
                                                    monkeypatch) -> None:
    """**0 件を黙って通さない。** フォルダ名を打ち間違えたときに
    「レコード 0・error 0」で緑になるのは、当たらないパスを黙って落とすのと同じ。
    """
    _setup(round_)
    (round_.organized / "資料" / "b.xlsx").mkdir(parents=True)

    code, _, err = _run(["lint", "--root", str(project.root), "資料/b.xlsx"],
                        monkeypatch)

    assert code == 2
    assert "見つかりません" in err


def test_declareは当たったファイルを全部出せる(project: Paths, round_: Round,
                                                monkeypatch) -> None:
    """**一括で仕様の外へ出す操作なのに、全部を確かめる手が無かった。**

    既定の先頭 20 件は打ち間違いの確認には足りる（実測 54 ファイル / 189 アンカーの
    うち 34 ファイルが「…ほか」に畳まれた）が、**パターンが余計なシートに当たって
    いても畳まれた側は見えない** ―― 落とした側は誰にも見えないので、`freeze` の
    未整理からも消える。
    """
    for n in range(22):
        parsed(round_, f"資料/b{n}.xlsx/表紙.md",
               f"# b{n}.xlsx / 表紙\n\n<!-- source: 資料/b{n}.xlsx / シート: 表紙 -->\n\n"
               "## セル B2  <!-- a:s1-x1 at=B2 -->\n\n- `B2` 設計書\n")

    _, out, _ = _run(["declare", "--root", str(project.root), "表紙",
                      "--reason", "表紙（仕様ではない）", "--dry-run"], monkeypatch)
    assert "…ほか 2 ファイル（全部出すには --list）" in out

    _, out, _ = _run(["declare", "--root", str(project.root), "表紙",
                      "--reason", "表紙（仕様ではない）", "--dry-run", "--list"],
                     monkeypatch)
    assert "…ほか" not in out
    assert out.count("[新規]") == 22


def test_freezeの指摘は担当ぶんに絞れる(project: Paths, round_: Round,
                                        monkeypatch) -> None:
    """**判定は絞らない**（ゲートの条件はファイルをまたぐ）が、出す指摘は絞れる。

    実測（11 ロットの分担）で、1 人あたり数百行の他人の指摘を `grep` で除けてから
    自分の 1 行を探していた ―― しかもパース結果のファイル名に空白が入るので、
    素直に grep すると件数が化ける。
    """
    _setup(round_, name="資料/a.xlsx/受注テーブル")
    # 別ロットのぶん（整理していない ＝ 未整理が出る）
    parsed(round_, "資料/z.xlsx/在庫テーブル.md",
           "# z.xlsx / 在庫テーブル\n\n<!-- source: 資料/z.xlsx / シート: 在庫テーブル -->\n\n"
           "## セル B2  <!-- a:s1-x1 at=B2 -->\n\n- `B2` 在庫\n")

    _, out, _ = _run(["freeze", "--root", str(project.root), "--dry-run"],
                     monkeypatch)
    assert "z.xlsx" in out

    _, out, _ = _run(["freeze", "--root", str(project.root), "--dry-run",
                      "--path", "資料/a.xlsx"], monkeypatch)
    assert "z.xlsx" not in out
    # **絞ったことを黙らない**（隠した中に error があっても凍結は止まる）。
    assert "--path 資料/a.xlsx のぶん: error 0 件 / warn 0 件" in out
    assert "--path で隠したぶん（ほかの担当）: error 1 件 / warn 0 件" in out


def test_担当ぶんがerror0なら自分の失敗と読ませない(project: Paths, round_: Round,
                                                    monkeypatch) -> None:
    """**「上の error」が上に無い。**

    自分の担当が error 0 でも、末尾は必ず「凍結できません（上の error を潰して
    ください）」で終わっていた ―― その error は `--path` で隠した他担当ぶんで、
    画面には 1 件も出ていない。実測で 3 人が独立に「自分の失敗と読んだ」と報告した。
    """
    _setup(round_, name="資料/a.xlsx/受注テーブル")
    parsed(round_, "資料/z.xlsx/在庫テーブル.md",
           "# z.xlsx / 在庫テーブル\n\n<!-- source: 資料/z.xlsx / シート: 在庫テーブル -->\n\n"
           "## セル B2  <!-- a:s1-x1 at=B2 -->\n\n- `B2` 在庫\n")

    code, _, err = _run(["freeze", "--root", str(project.root), "--dry-run",
                         "--path", "資料/a.xlsx"], monkeypatch)

    # 凍結できないこと自体は変わらない（判定はラウンド全体で行う）。
    assert code == 1
    assert "あなたの担当（--path 資料/a.xlsx）は凍結の条件を満たしています" in err
    assert "他の担当ぶんに error が 1 件残っている" in err
    assert "上の error" not in err


def test_担当ぶんにerrorがあれば従来どおり言う(project: Paths, round_: Round,
                                              monkeypatch) -> None:
    """自分のぶんに error があるなら「上の error」は本当に上にある。"""
    _setup(round_, _GOOD.replace("anchor: s1-t1", "anchor: s9-t9"),
           name="資料/a.xlsx/受注テーブル")

    code, _, err = _run(["freeze", "--root", str(project.root), "--dry-run",
                         "--path", "資料/a.xlsx"], monkeypatch)

    assert code == 1
    assert "凍結できません（上の error を潰してください）" in err


def test_当たらないpathはexit2(project: Paths, round_: Round, monkeypatch) -> None:
    """**0 件と区別する。** いまは打ち間違いが「担当ぶんは全部きれい」に見える
    ―― 絞り込みは指摘を消す仕組みなので、消えたのか無かったのかが打った人から
    区別できない（`lint` は当たらないパスで exit 2 になる。それに揃える）。
    """
    _setup(round_, name="資料/a.xlsx/受注テーブル")

    code, _, err = _run(["freeze", "--root", str(project.root), "--dry-run",
                         "--path", "資料/A.xlsx"], monkeypatch)

    assert code == 2
    assert "--path がどのファイルにも当たりません: 資料/A.xlsx" in err


def test_当たるがきれいなpathは通す(project: Paths, round_: Round,
                                    monkeypatch) -> None:
    """当たったうえで指摘が 0 件なのは正常 ―― **exit 2 は打ち間違いの合図**である。"""
    _setup(round_, name="資料/a.xlsx/受注テーブル")

    code, out, _ = _run(["freeze", "--root", str(project.root), "--dry-run",
                         "--path", "資料/a.xlsx"], monkeypatch)

    assert code == 0
    assert "--path 資料/a.xlsx のぶん: error 0 件 / warn 0 件" in out


# ── description への逃がし（G029 / G030） ──────────────────────
_TABLE_PARSED = """\
# a.xlsx / 受注テーブル列定義

<!-- source: 資料/a.xlsx / シート: 受注テーブル列定義 -->

## 表 B5:H8  <!-- a:s1-t1 at=B5:H8 -->

| 論理名 | 物理名 | 既定値 | 備考 |
|---|---|---|---|
| 受注番号 | ORDER_NO |  | 排他制御に使う |
"""


def _column_yaml(told: str, attrs: str = "") -> str:
    """列 1 本ぶんの整理結果。``description``（＝ refs の note）に何を書くかだけ変える。"""
    return f"""\
records:
  - concept: c-ent-受注
    type: エンティティ
    name: 受注ヘッダ
    statement: 受注ヘッダは受注 1 件を保持すること
    source: {{ anchor: s1-t1 }}
    refs:
      - {{ rel: has-column, to: c-受注番号, note: "{told}"{attrs} }}
  - concept: c-受注番号
    type: データ項目
    name: 受注番号
    statement: 受注番号は受注を一意に識別する文字列であること
    source: {{ anchor: s1-t1 }}
"""


def _column_lint(round_: Round, model: Metamodel, body: str) -> list[str]:
    parsed(round_, "資料/a.xlsx/受注テーブル列定義.md", _TABLE_PARSED)
    organized(round_, "資料/a.xlsx/受注テーブル列定義.yml", body)
    return codes(freeze.lint(round_, model, {}).findings)


def test_宣言済みの欄があるのにdescriptionへ流したらG029(round_: Round,
                                                         model: Metamodel) -> None:
    """**予約キーはスキーマ検査を素通りする。**

    `description` は :data:`arp4.metamodel.RELATION_RESERVED` なので、宣言なしに
    どの関係へも書ける ―― `G016`（宣言に無い属性名）にも `G028`（enum 外）にも
    当たらない。実測（r001）で `displays` 154 本の初期値と物理名がここへ流れ、
    **error も warn も 1 件も出ないまま設計書から消えた。**
    """
    said = _column_lint(round_, model,
                        _column_yaml("物理名 ORDER_NO ／ 既定値 0 ／ 備考 排他制御に使う"))

    assert "G029" in said


def test_G029は写す先を名指しする(round_: Round, model: Metamodel) -> None:
    """**「description に書くな」では直せない。** どの欄へ写すかまで言う。"""
    parsed(round_, "資料/a.xlsx/受注テーブル列定義.md", _TABLE_PARSED)
    organized(round_, "資料/a.xlsx/受注テーブル列定義.yml",
              _column_yaml("物理名 ORDER_NO ／ 既定値 0"))

    found = [f for f in freeze.lint(round_, model, {}).findings if f.code == "G029"]

    assert len(found) == 1
    assert "physical_name" in found[0].message and "default_value" in found[0].message
    assert found[0].line                          # その関係の行を指す


def test_受け皿の無い見出しが重なったらG030(round_: Round,
                                            model: Metamodel) -> None:
    """**語彙の穴の申告漏れ。** 受け皿が無いなら `_metamodel-add` へ提案する。"""
    parsed(round_, "資料/a.xlsx/受注テーブル列定義.md", _TABLE_PARSED)
    body = _column_yaml("物理名 ORDER_NO")
    refs = "".join(
        f'      - {{ rel: has-column, to: c-受注番号, note: "取得元 T_ORDER.C{i}" }}\n'
        for i in range(4))
    organized(round_, "資料/a.xlsx/受注テーブル列定義.yml",
              body.replace("    refs:\n", "    refs:\n" + refs))

    found = [f for f in freeze.lint(round_, model, {}).findings if f.code == "G030"]

    assert len(found) == 1
    assert "取得元" in found[0].message and "has-column" in found[0].target


# ── その文言が事実と食い違っていないか（G029 / G030） ──────────
#
# **ヒントは規則である。** 実測（8 分担）で、`G029` / `G030` が言っていた
# 「description は設計書のどの列にも出ません」を読んだ **5 人が独立に**
# 「description に置くと消える」と解し、資料にあった値（単価の根拠・SLO の
# 測り方・要員の工数・申請日）を `statement` の文中へ畳んだ ―― **畳むと列と
# しては二度と引けない。** 事実のほうは逆で、様式（パックの documents/*.yml）
# は節ごとに `description` を「補足」列として持っている。
def test_descriptionは様式の列になりうる() -> None:
    """**G029 / G030 の文言が寄りかかっている事実**を、様式の側から押さえる。

    ここが崩れたら（`description` を出す節が 1 つも無くなったら）文言のほうを
    見直す ―― 逆に、崩れていないのに「どの列にも出ません」と書き足されたら、
    それは事実に反する規則である。

    **1 つでも出す節があれば十分**にしてある。全節が持つことまで要求すると、
    様式は案件ごとに変わるものなので、パックを畳んだだけでここが赤くなる。
    """
    chain, findings = pack.resolve_chain("jp-sier-std")
    assert not [f for f in findings if f.level == "error"]
    出す = [(str(document.get("name")), section.get("heading"))
            for document in pack.documents(chain)
            for section in document.get("sections") or []
            if "description" in (section.get("columns") or [])]

    assert 出す, "description を列に出す節が 1 つもありません（文言を見直すこと）"


def test_G029は欄の列が空になると言う(round_: Round, model: Metamodel) -> None:
    """**言えるのは「書かなかった欄の列が空になる」ことだけ。**

    :func:`arp4.freeze._descriptions` が受け取るのは ``Metamodel`` だけで、様式を
    読んでいない ―― `description` が出るか出ないかは様式の側でしか決まらないので、
    ここから断定してはいけない。
    """
    parsed(round_, "資料/a.xlsx/受注テーブル列定義.md", _TABLE_PARSED)
    organized(round_, "資料/a.xlsx/受注テーブル列定義.yml",
              _column_yaml("物理名 ORDER_NO ／ 既定値 0"))

    found = [f for f in freeze.lint(round_, model, {}).findings if f.code == "G029"]

    assert len(found) == 1
    assert "その欄の列は空のまま" in found[0].message
    # **事実に反する断定を書き戻さない。**
    assert "どの列にも出ません" not in found[0].message


def test_G030はstatementへ畳めと言わない(round_: Round, model: Metamodel) -> None:
    """**畳むほうを既定の助言にしない。** 受け皿が無いのは語彙の穴である。

    ここは受け皿が無い側なので `description` が出るとも出ないとも言えない
    （「補足」列を持つ章なら残り、持たない章なら残らない ―― 様式次第）。
    言えるのは **`statement` へ畳めば列にならない**ことのほうで、そちらは
    様式に依らず決まる。
    """
    parsed(round_, "資料/a.xlsx/受注テーブル列定義.md", _TABLE_PARSED)
    body = _column_yaml("物理名 ORDER_NO")
    refs = "".join(
        f'      - {{ rel: has-column, to: c-受注番号, note: "取得元 T_ORDER.C{i}" }}\n'
        for i in range(4))
    organized(round_, "資料/a.xlsx/受注テーブル列定義.yml",
              body.replace("    refs:\n", "    refs:\n" + refs))

    hint = [f for f in freeze.lint(round_, model, {}).findings
            if f.code == "G030"][0].hint or ""

    assert "_metamodel-add.yml" in hint                     # 受け皿を足すのが本筋
    assert "statement へ畳まないこと" in hint
    assert "二度と" in hint                                  # 畳むと列にならない
    assert "statement へまとめます" not in hint              # 畳むほうを勧めない
    assert "どの列にも出ません" not in hint


def test_一致が1つだけならG029は鳴らない(round_: Round, model: Metamodel) -> None:
    """**補足の 1 文が偶然に欄の名前で始まることはある。**

    2 つ並んで初めて、欄に割って書けたものを 1 つの散文に畳んだと言える ――
    1 つだけで鳴らすと、正しい補足のほうが件数で埋もれる。
    """
    said = _column_lint(round_, model, _column_yaml("備考 排他制御に使う"))

    assert "G029" not in said


def test_対応する欄が埋まっていればG029は鳴らない(round_: Round,
                                                  model: Metamodel) -> None:
    """**言い直しは誤りではない。** 欄が埋まっているなら設計書に出ている。"""
    said = _column_lint(round_, model, _column_yaml(
        "物理名 ORDER_NO ／ 既定値 0",
        attrs=', attrs: { physical_name: ORDER_NO, default_value: "0" }'))

    assert "G029" not in said


def test_見出しの重なりが閾値未満ならG030は鳴らない(round_: Round,
                                                    model: Metamodel) -> None:
    """**資料 1 枚の言い回しでありうる。** 3 件までは語彙の穴とは言わない。"""
    parsed(round_, "資料/a.xlsx/受注テーブル列定義.md", _TABLE_PARSED)
    body = _column_yaml("物理名 ORDER_NO")
    refs = "".join(
        f'      - {{ rel: has-column, to: c-受注番号, note: "取得元 T_ORDER.C{i}" }}\n'
        for i in range(2))
    organized(round_, "資料/a.xlsx/受注テーブル列定義.yml",
              body.replace("    refs:\n", "    refs:\n" + refs))

    assert "G030" not in codes(freeze.lint(round_, model, {}).findings)
