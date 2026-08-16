"""人向けの一覧を畳む ―― **同じ文を何十回も読ませない。**

読み手はエージェントである。**標準出力は読み飛ばせない** ―― 端末なら人が目を
滑らせて済むところを、エージェントは 1 文字残らず文脈に載せる。だから資源は
行数ではなく**文字数**で、削るべきは「1 件ごとに違う情報」ではなく
**「何度も出てくる同じ文」**である。

実測（``arp4 check --strict``・警告 173 件・24,381 字）では

* ``→`` の 72 行に**7 種**しか無い（5,000 字が同じ文の再掲）
* ``known_gaps で承知している: …`` の 48 回に**12 種**しか無い（3,700 字）
* ``W031`` の 34 行は ID と名前以外まったく同じ文である

―― 4 割が同文の再掲だった。ここで畳むのは**その 4 割**と、**同じ種類の 6 件目
から先**である。

**畳むのは表示だけで、件数も終了コードも変えない。** 「畳んだから減った」が
起きると、人と CI の結論がずれる（→ :func:`arp4.cli._exit_code`）。
``--format json`` も間引かない ―― 機械が読むものを削る理由は無い
（→ :mod:`arp4.report`）。

**畳んだことは畳んだと言う。** 出力の末尾で「何を畳んだか」と「開き方」を必ず
出す。黙って減らすと、読み手は出ている行が全部だと読む。``freeze`` の作業キューが
先頭 20 件で切って「…ほか N 件（全部出すには ``--list``）」と言うのと同じ約束で
ある ―― こちらは**種別ごと**に切る（1 種類が画面を埋めても、ほかの種類が
下へ流れない）。
"""

from __future__ import annotations

import re
from typing import Iterable, Sequence

from arp4.finding import Finding, order

#: 同じ文が何件から「再掲」になるか。2 件で畳んでも 1 行しか減らない。
ROLLUP = 3

#: 1 つの ``code`` から出す最大の塊。**6 件目から先は種類が変わらない** ――
#: 直し方は同じで、違うのは対象だけである（対象は ``--code`` で全部出せる）。
#: **error は切らない。** あれは全部直すもので、選んで直すものではない。
CAP = 5

#: 承知済みの欠落（``known_gaps`` で理由を宣言してある）。**既定では件数だけ。**
#: 人が「資料に無いと確かめた」と書いたものを毎回全文で読み返す必要は無い ――
#: 理由の本文は正本と ``0_この設計書の穴.md`` の側にある。
KNOWN = "W032"

#: 畳んだ行の続き（対象の一覧）の折り返し。全角と半角が混じるので緩めに取る。
WIDTH = 76

#: 内訳（``--summary``）に添える本文の長さ。**種類が分かればよい。**
BRIEF = 38

#: 1 つの ``code`` の内訳として出す**言い分の形**の数。ここで切るのは、形が
#: 割れるのは対象名が違うだけのことがあるためである（``列「備考」…`` と
#: ``列「補足」…``）。切ったことは「ほか N 形」と言う ―― 黙って減らさない。
SHAPES = 3


def matches(code: str, wanted: Sequence[str]) -> bool:
    """``--code`` の照合。**前方一致を許す。**

    コードは体系そのものが接頭辞である（``W0`` は正本の警告、``P1`` は出来上がった
    設計書の形）ので、``--code P1`` で系ごと開けるほうが、4 桁を並べるより速い。
    """
    return any(code == one or code.startswith(one) for one in wanted)


def select(findings: Iterable[Finding],
           wanted: Sequence[str]) -> tuple[list[Finding], list[Finding]]:
    """``--code`` で絞る。**隠したほうも返す** ―― 黙って消さないため。"""
    if not wanted:
        return list(findings), []
    kept, hidden = [], []
    for finding in findings:
        (kept if matches(finding.code, wanted) else hidden).append(finding)
    return kept, hidden


def tally(findings: Iterable[Finding]) -> dict[str, int]:
    """``code`` ごとの件数。順序は :func:`arp4.finding.order` に合わせる。"""
    result: dict[str, int] = {}
    for finding in order(findings):
        result[finding.code] = result.get(finding.code, 0) + 1
    return result


def lines(findings: Iterable[Finding], *, show_known: bool = False,
          codes: Sequence[str] = (), summary: bool = False) -> list[str]:
    """人が読む一覧。**1 件 1 行ではなく、1 つの言い分に 1 行。**

    ``codes`` を渡すと、その系だけを**切らずに**出す（深掘り）。``summary`` は
    内訳だけ ―― 「どれから手を付けるか」を決める 1 回目の呼び出し用である。
    """
    ordered = order(findings)
    if summary:
        return _summary(ordered)

    kept, hidden = select(ordered, codes)
    if codes and matches(KNOWN, codes):
        show_known = True                 # 名指しで呼ばれたものを畳まない
    if show_known:
        shown, known = kept, []
    else:
        shown = [f for f in kept if f.code != KNOWN]
        known = [f for f in kept if f.code == KNOWN]

    # 名指しされた系は全部出す ―― 深掘りの途中で切ると、開くために
    # もう 1 度同じことを言うことになる。
    body, folded, capped = _rolled(shown, cap=None if codes else CAP)
    if known:
        body.append(f"[{known[0].level}] {KNOWN} "
                    f"known_gaps で承知している欠落: {len(known)} 件")

    return body + _footer(shown, folded, capped, len(known), hidden)


def _summary(findings: list[Finding]) -> list[str]:
    """``code`` ごとに 1 行 ―― ただし **1 つの言い分に 1 行**（:func:`lines` と同じ規律）。

    **``code`` は 1 種類とは限らない。** サンプル 1 件の本文を代表に立てると、
    多数派が見えないまま最初の 1 枚が出る ―― 実測（sales-corpus r001）で

    * ``W031`` 71 件が「realizes が 1 本もありません」と名乗った
      （内訳は **constrains 60** / realizes 11。**代表が少数派**）
    * ``W032`` 44 件が「has-column の多重度違反です」と名乗った
      （内訳は 必須属性がありません 23 / has-column 14 / displays 6 / disputes 1）
    * ``P111`` 12 件が「正本に **1 件**ありますが」と名乗った
      （その 1 件は 12 通りのうちの 1 つで、実体は 177・157・52…）

    ―― 手順書はこの ``--summary`` を「どれから手を付けるかを決める 1 回目の
    呼び出し」に置いているので、**最初に読む 1 枚が一番外していた。**

    形が 1 つで**文まで全員同じ**なら 1 行のまま（本文は ``:`` の先まで見せる）。
    割れているときだけ件数つきで展開する ―― 割れていることを黙っていると、
    読み手は出ている文が全部だと読む。
    """
    if not findings:
        return []
    out = [f"指摘の内訳 ― 全 {len(findings)} 件"]
    for code, count in tally(findings).items():
        same = [f for f in findings if f.code == code]
        shapes = _shapes(same)
        head = f"  [{same[0].level}] {code} {count} 件"
        if len(shapes) == 1:
            # 形が 1 つなら 1 行のまま。文まで全員同じときだけ `:` の先も見せる
            # ―― 数が割れているのに 1 件目の数を出すと、それが全件の数に見える。
            only = next(iter(shapes))
            whole = only == _head(same[0].message)
            out.append(f"{head} ― {_brief(same[0].message if whole else only)}")
            continue
        out.append(head)
        for shape, number in list(shapes.items())[:SHAPES]:
            out.append(f"      {number} 件 ― {_brief(shape)}")
        if len(shapes) > SHAPES:
            out.append(f"      （ほか {len(shapes) - SHAPES} 形）")
    out.append("")
    out.append("（1 種類だけ開くには --code <コード>。"
               "前方一致するので --code W0 で系ごとも出せます）")
    return out


def _shapes(findings: list[Finding]) -> dict[str, int]:
    """同じ ``code`` の中の **言い分の形**ごとの件数（多い順）。

    畳む鍵は「``:`` の手前の 1 文から数を落としたもの」である。``正本に 177 件``と
    ``正本に 3 件``は同じ言い分（対象の数が違うだけ）だが、``constrains が 1 本も
    ありません``と``realizes が 1 本もありません``は**別の言い分**で、直す先も違う。

    **見せるのは、その形の全員が同じ文のときだけ畳む前の文**である。数が割れて
    いるのに 1 件目を代表に立てると、``P111 12 件 ― 正本に 1 件ありますが`` の
    ように読み手が 12 × 1 と読む（実測の中身は 177・157・52… で 500 件を超える）。
    割れているときは数のところを ``N`` にして、**数は 1 つに決まらないと言う。**
    """
    counted: dict[str, int] = {}
    heads: dict[str, set[str]] = {}
    for finding in findings:
        head = _head(finding.message)
        key = re.sub(r"\d+", "N", head)
        counted[key] = counted.get(key, 0) + 1
        heads.setdefault(key, set()).add(head)
    ordered = sorted(counted.items(), key=lambda kv: -kv[1])
    return {(next(iter(heads[key])) if len(heads[key]) == 1 else key): number
            for key, number in ordered}


def _head(message: str) -> str:
    """本文の 1 文目（``:`` の手前）。**言い分はここまでで決まる。**"""
    return re.split(r"[:：]", message, maxsplit=1)[0].strip()


def _brief(text: str) -> str:
    return text if len(text) <= BRIEF else text[:BRIEF] + "…"


def _rolled(findings: list[Finding],
            cap: int | None) -> tuple[list[str], int, int]:
    """同じ ``(level, code, message)`` を 1 行へ。**対象は 1 つも落とさない。**

    畳んだ行は「文 → 対象」の順にする（1 件ずつのときと前後が逆になる）。
    共通なのは文のほうなので、先に出したほうが読み手は 1 度で済む。

    **位置（``file``）を持つ指摘は畳まない。** あれはエディタで開くためのもので、
    畳むと開けなくなる ―― 減るのは字数だが、失われるのは打ち手である。
    """
    groups: dict[tuple[str, str, str], list[Finding]] = {}
    for finding in findings:
        groups.setdefault(
            (finding.level, finding.code, finding.message), []).append(finding)

    # 塊を code ごとにまとめ直す（切るのは種別ごと ―― 1 種類が画面を埋めても
    # ほかの種類が下へ流れない）。`order` 済みなので並びは決まっている。
    blocks: dict[tuple[str, str], list[tuple[list[str], int]]] = {}
    folded = 0
    for (level, code, message), members in groups.items():
        if len(members) < ROLLUP or any(f.file for f in members):
            rows = [([f.head], 1) for f in members]
        else:
            rows = [([f"[{level}] {code} {message}: {len(members)} 件"]
                     + _wrapped(f.target for f in members), len(members))]
            folded += len(members)
        blocks.setdefault((level, code), []).extend(rows)

    out: list[str] = []
    capped = 0
    for (level, code), rows in blocks.items():
        limit = len(rows) if cap is None or level == "error" else cap
        for block, _ in rows[:limit]:
            out += block
        rest = sum(n for _, n in rows[limit:])
        if rest:
            out.append(f"    …ほか {rest} 件（全部出すには --code {code}）")
            capped += rest
    return out, folded, capped


def _wrapped(targets: Iterable[str], indent: str = "    ") -> list[str]:
    """畳んだ行の対象。**全件を出す**（「ほか 32 件」は打ち手を消す）。"""
    out: list[str] = []
    row = ""
    for target in targets:
        if row and len(row) + len(target) + 1 > WIDTH:
            out.append(indent + row)
            row = target
        else:
            row = f"{row}、{target}" if row else target
    if row:
        out.append(indent + row)
    return out


def _footer(shown: list[Finding], folded: int, capped: int, known: int,
            hidden: list[Finding]) -> list[str]:
    """畳んだ量と、``hint`` の凡例。

    ``hint`` は指摘 1 件ごとではなく**規則ごと**の定数である（同じ ``code`` なら
    同じ文が出る）。1 件ずつ添えると、7 種の文が 72 回出る。
    """
    out: list[str] = []
    if hidden:
        # **絞ったことを黙らない**（``freeze --path`` と同じ約束）。判定は
        # 絞っていないので、件数を言わないと「warn 0 なのに exit 2」に見える。
        breakdown = "・".join(f"{code} {n}" for code, n in tally(hidden).items())
        out.append("")
        out.append(f"（--code で {len(hidden)} 件を隠しています: {breakdown}）")

    parts = []
    if folded:
        parts.append(f"同じ文 {folded} 件")
    if capped:
        parts.append(f"種別ごとに {capped} 件")
    if known:
        parts.append(f"承知済み {known} 件（--show-known）")
    if parts:
        out.append("")
        out.append(f"（表示を省略しています。{'・'.join(parts)}。"
                   "1 種類を開くには --code <コード>）")

    legend: dict[tuple[str, str], None] = {}
    for finding in shown:
        if finding.hint:
            legend[(finding.code, finding.hint)] = None
    if legend:
        out.append("")
        out.append("直し方（code ごとに 1 度だけ出します）")
        out += [f"  {code} → {hint}" for code, hint in sorted(legend)]
    return out
