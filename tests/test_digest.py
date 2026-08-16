"""人向けの一覧の畳み方 ―― **同じ文を何十回も読ませない。**

読み手はエージェントで、標準出力は読み飛ばせない（1 文字残らず文脈に載る）。
実測では ``check --strict`` の 24,381 字のうち 4 割が同文の再掲だった ――
``→`` の 72 行に 7 種、``W031`` の 34 行は ID 以外まったく同じ文である。

ここで見るのは「**畳んだぶんの情報が消えていないか**」と「**件数と終了コードが
畳みで変わらないか**」の 2 つである。字数が減っても打ち手が消えるなら損である。
"""

from __future__ import annotations

from arp4 import digest
from arp4.finding import Finding


def _many(count: int, code: str = "W031", message: str = "相手がありません",
          **kw) -> list[Finding]:
    return [Finding("warn", code, f"cst-{i}（制約 {i}）", message, **kw)
            for i in range(count)]


# ── ① 同じ文は 1 度だけ ────────────────────────────────────────
def test_同じ文の指摘は1行に畳んで対象を並べる() -> None:
    said = digest.lines(_many(3))

    assert said[0] == "[warn] W031 相手がありません: 3 件"
    assert "cst-0（制約 0）" in said[1] and "cst-2（制約 2）" in said[1]


def test_畳んでも対象は1つも落とさない() -> None:
    """「ほか 32 件」は字数を減らすが、**打ち手（どれを直すか）を消す。**"""
    said = "\n".join(digest.lines(_many(40)))

    for i in range(40):
        assert f"cst-{i}（制約 {i}）" in said


def test_2件では畳まない() -> None:
    """畳んでも 1 行しか減らないのに、読み方だけ 2 通りになる。"""
    said = digest.lines(_many(2))

    assert said == ["[warn] W031 cst-0（制約 0）: 相手がありません",
                    "[warn] W031 cst-1（制約 1）: 相手がありません"]


def test_文が違えば畳まない() -> None:
    findings = (_many(3, message="constrains が 1 本もありません")
                + _many(3, message="realizes が 1 本もありません"))
    said = [l for l in digest.lines(findings) if l.startswith("[warn]")]

    assert len(said) == 2
    assert said[0].endswith(": 3 件") and said[1].endswith(": 3 件")


def test_位置のある指摘は畳まない() -> None:
    """``file:line`` はエディタで開くためのものである ―― 畳むと開けなくなる。
    減るのは字数だが、**失われるのは打ち手**である。"""
    findings = [f.at(f"a{i}.yml", 3) for i, f in enumerate(_many(5, "G030"))]
    said = digest.lines(findings)

    assert len([l for l in said if l.startswith("[warn]")]) == 5
    assert "a4.yml:3" in "\n".join(said)


# ── ② hint は規則ごとの定数 ────────────────────────────────────
def test_hintはcodeごとに1度だけ末尾に出る() -> None:
    """1 件ずつ添えると、7 種の文が 72 回出る（実測）。"""
    said = digest.lines(_many(3, "P111", hint="様式の列に足す"))

    assert said.count("  P111 → 様式の列に足す") == 1
    assert not [l for l in said if l.startswith("    → ")]


def test_違うcodeのhintは両方出る() -> None:
    said = digest.lines(_many(3, "P111", hint="列に足す")
                        + _many(3, "W046", "空です", hint="このままでよい"))

    assert "  P111 → 列に足す" in said
    assert "  W046 → このままでよい" in said


# ── ③ 承知済みの欠落 ───────────────────────────────────────────
def test_known_gapsで承知済みは既定では件数だけ() -> None:
    """人が「資料に無い」と確かめて書いたものを毎回全文で読み返す必要は無い
    ―― 理由の本文は正本と ``0_この設計書の穴.md`` の側にある。"""
    findings = [Finding("warn", digest.KNOWN, f"ent-{i}",
                        f"has-column の多重度違反です ―― known_gaps: 長い理由 {i}")
                for i in range(10)]
    said = digest.lines(findings)

    assert said[0] == "[warn] W032 known_gaps で承知している欠落: 10 件"
    assert "長い理由 3" not in "\n".join(said)


def test_show_knownで1件ずつ出す() -> None:
    findings = [Finding("warn", digest.KNOWN, f"ent-{i}", f"違反です ―― 理由 {i}")
                for i in range(10)]
    said = "\n".join(digest.lines(findings, show_known=True))

    assert "理由 3" in said                                  # 1 件ずつ出る
    assert "…ほか 5 件（全部出すには --code W032）" in said    # 種別の上限は効く


# ── ④ 畳んだことは畳んだと言う ─────────────────────────────────
def test_畳んだ件数と全件の出し方を必ず言う() -> None:
    """黙って減らすと、読み手は**出ている行が全部だと読む。**"""
    said = "\n".join(digest.lines(_many(5)))

    assert "同じ文 5 件" in said
    assert "--code <コード>" in said


# ── ⑤ 段階的に開く ─────────────────────────────────────────────
def test_同じcodeは先頭だけ出して残りは件数で言う() -> None:
    """**6 件目から先は種類が変わらない** ―― 直し方は同じで、違うのは対象だけ。"""
    findings = [Finding("warn", "W043", f"doc-{i}", f"{i} 列が全行空です")
                for i in range(12)]
    said = digest.lines(findings)

    assert len([l for l in said if l.startswith("[warn]")]) == digest.CAP
    assert "    …ほか 7 件（全部出すには --code W043）" in said


def test_errorは切らない() -> None:
    """error は全部直すものである ―― 選んで直すものではない。"""
    findings = [Finding("error", "E010", f"itm-{i}", f"{i} が要ります")
                for i in range(12)]
    said = digest.lines(findings)

    assert len([l for l in said if l.startswith("[error]")]) == 12
    assert not [l for l in said if "ほか" in l]


def test_codeで名指しすれば切らずに出す() -> None:
    findings = ([Finding("warn", "W043", f"doc-{i}", f"{i} 列が全行空です")
                 for i in range(12)]
                + _many(4, "P111", "受け皿がありません"))
    said = digest.lines(findings, codes=["W043"])

    assert len([l for l in said if l.startswith("[warn]")]) == 12
    assert not [l for l in said if l.startswith("[warn] P111")]


def test_codeは前方一致する() -> None:
    """コードの体系そのものが接頭辞である（``W0`` は正本の警告、``P1`` は設計書の形）。"""
    findings = _many(3, "W030") + _many(3, "P104", "同じ本文です")
    said = "\n".join(digest.lines(findings, codes=["P1"]))

    assert "[warn] P104" in said
    assert "[warn] W030" not in said            # 隠した件数としては出る


def test_絞ったことを黙らない() -> None:
    """判定は絞っていないので、件数を言わないと「warn 0 なのに exit 2」に見える。"""
    findings = _many(3, "W030") + _many(4, "P104", "同じ本文です")
    said = "\n".join(digest.lines(findings, codes=["W030"]))

    assert "--code で 4 件を隠しています: P104 4" in said


def test_show_knownを名指しで呼べば畳まない() -> None:
    """名指しで呼ばれたものを畳むと、開くために同じことを 2 度言うことになる。"""
    findings = [Finding("warn", digest.KNOWN, f"ent-{i}", f"違反です ―― 理由 {i}")
                for i in range(6)]
    said = "\n".join(digest.lines(findings, codes=[digest.KNOWN]))

    assert "理由 5" in said


def test_summaryはcodeごとに1行だけ() -> None:
    """「どれから手を付けるか」を決める 1 回目の呼び出し用。"""
    findings = _many(30) + _many(4, "P104", "同じ本文です")
    said = digest.lines(findings, summary=True)

    assert said[0] == "指摘の内訳 ― 全 34 件"
    assert "  [warn] W031 30 件 ― 相手がありません" in said
    assert "  [warn] P104 4 件 ― 同じ本文です" in said
    assert not [l for l in said if l.startswith("[warn]")]   # 1 件ずつは出ない


def test_summaryは1つのcodeが複数の言い分でも多数派を隠さない() -> None:
    """**代表が少数派になってはいけない。** 実測（sales-corpus r001）で `W031`
    71 件が「realizes が 1 本もありません」と名乗り、内訳は constrains 60 /
    realizes 11 だった ―― 最初に読む 1 枚が一番外していた。
    """
    findings = (_many(60, "W031", "constrains（制約する） が 1 本もありません")
                + _many(11, "W031", "realizes（実現する） が 1 本もありません"))
    said = digest.lines(findings, summary=True)

    assert "  [warn] W031 71 件" in said
    assert "      60 件 ― constrains（制約する） が 1 本もありません" in said
    assert "      11 件 ― realizes（実現する） が 1 本もありません" in said


def test_summaryは形が多いと切って切ったと言う() -> None:
    findings = [f for n, code in enumerate("abcd")
                for f in _many(n + 1, "W032", f"{code} の多重度違反です")]
    said = digest.lines(findings, summary=True)

    assert "  [warn] W032 10 件" in said
    assert "      4 件 ― d の多重度違反です" in said      # 多い順
    assert "      （ほか 1 形）" in said                   # 黙って減らさない


def test_summaryは数だけが違う言い分を1行に畳んで数を名乗らない() -> None:
    """`P111 12 件 ― 正本に 1 件ありますが` は 12 × 1 と読まれる。
    実測の中身は 177・157・52… で 500 件を超えていた ―― **1 件目の数は全体の数
    ではない。**
    """
    findings = [Finding("warn", "P111", f"t{n}",
                        f"正本に {n} 件ありますが、どの設計書にも出ていません")
                for n in (177, 157, 52, 1)]
    said = digest.lines(findings, summary=True)

    assert "  [warn] P111 4 件 ― 正本に N 件ありますが、どの設計書にも出ていません" in said
    assert not [l for l in said if "正本に 1 件" in l]


def test_畳まなければ断り書きも出さない() -> None:
    """畳んでいないのに「畳みました」と言うと、読み手は隠れた行を探しに行く。"""
    assert digest.lines(_many(2)) == [
        "[warn] W031 cst-0（制約 0）: 相手がありません",
        "[warn] W031 cst-1（制約 1）: 相手がありません"]
