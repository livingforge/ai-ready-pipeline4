"""① パース結果の**疑い**を出す ―― 機械が切った塊が、資料では 1 つの表かもしれない。

塊の区切りは**意味の判断ではなく提示上の都合**である（→ ``docs/parsed.md`` の
規律 ④）。区切りを間違えても値は 1 つも落ちず、番地は全部付いているので読み直せる
―― そう決めたので、**間違えたことを誰も言わない**ままだった。

黙って割れているのが困るのは、読むのが人ではなく整理層だからである。3 つに割れた
表は 3 つの表として読まれ、``項番 21`` から先に列があることに気づかれない。
:func:`arp4.parse._big_note` は「大きい塊は大きいと言う」、
:func:`arp4.parse._sparse_note` は「表の形で出していないことを言う」―― 提示上の
都合はどれも申告しているのに、**割ったことだけが黙っていた。**

**ここで出すのは疑いであって誤りではない。** 空列を挟んで横に並べた 2 つの表は
実物の一覧シートにごく普通にあり、それが 1 つの表か 2 つの表かは**資料を開かないと
決まらない** ―― 決めるのは整理層である。だから ``error`` は 1 つも出さない。
出すのは ``warn`` と、手番が人に移ったことを言う ``exit 3`` だけである。
"""

from __future__ import annotations

import re
from typing import NamedTuple

from arp4 import mdio
from arp4.finding import Finding
from arp4.paths import Round

#: 見るアンカー。**セルの塊だけである** ―― 印刷設定・図形・画像・グラフ・
#: コメントは番地の代わりに件数を ``at`` に持つので、範囲としては読めない。
_CELLS = re.compile(r"^s\d+-[tx]\d+$")

#: ``B2:D11`` / ``A1``。
_RANGE = re.compile(r"^(?P<c1>[A-Z]{1,3})(?P<r1>\d+)"
                    r"(?::(?P<c2>[A-Z]{1,3})(?P<r2>\d+))?$")

#: 見出しの繰り返しを探す深さ。**1 行目とは限らない** ―― ページごとに
#: ``テーブル定義書（2/3）`` を打ち直す書き方が実物では普通で、そのときは
#: 2 行目が見出しになる。
_HEAD = 2

#: 見出しの行とみなす最小の埋まり具合。1 列しか埋まっていない行は**表題**で
#: あって見出しではない ―― それが一致しても「同じ表題の別の表」でしかない。
_HEAD_CELLS = 2

#: 一覧に並べるアンカーの数。超えたぶんは件数で畳む。
_LISTED = 6


class Block(NamedTuple):
    """セルの塊 1 つ。**番地の範囲と中身だけ**を持つ。"""

    anchor: str
    at: str
    top: int
    left: int
    bottom: int
    right: int
    rows: list[list[str]]

    @property
    def width(self) -> int:
        return self.right - self.left + 1

    @property
    def height(self) -> int:
        return self.bottom - self.top + 1


def look(round_: Round, only: str = "") -> tuple[list[Finding], int, int]:
    """ラウンドの ``parsed/`` を走る。戻すのは ``(疑い, 見た写し, 見た塊)``。

    見た数を一緒に返すのは、**0 件が「無い」なのか「見ていない」なのかを
    読み手が区別できるようにする**ためである（``--path`` で絞ると簡単に 0 本に
    なる）。数えないと、絞り間違えたときにも「疑いはありません」と出る。
    """
    findings: list[Finding] = []
    files = chunks = 0
    for path in sorted(mdio.scan(round_.parsed)):
        if only and only not in path.relative_to(round_.parsed).as_posix():
            continue
        where = path.relative_to(round_.root).as_posix()
        document = mdio.read(path)
        files += 1
        chunks += len(document.anchors)
        blocks = _blocks(document)
        findings += _sideways(blocks, where) + _paged(blocks, where)
    return findings, files, chunks


# ── 塊を番地の範囲にする ────────────────────────────────────────
def _blocks(document: mdio.ParsedFile) -> list[Block]:
    """**番地を持つ塊だけ**を拾う（＝いまのところ Excel のパース結果だけ）。

    ``at`` が範囲として読めるかどうかで切る ―― アンカーの形だけで切らないのは、
    番地を持つ形式が将来増えたときに**同じ検査がそのまま効く**ようにするため
    である。逆に、印刷設定（``at=印刷設定 2 件``）や図形（``at=図形 1 個``）は
    ここで自然に落ちる。
    """
    found: list[Block] = []
    for anchor in document.anchors:
        if not _CELLS.match(anchor.id):
            continue
        span = _span(anchor.at)
        if span is None:
            continue
        found.append(Block(anchor.id, anchor.at, *span, mdio.rows(anchor)))
    return found


def _span(at: str) -> tuple[int, int, int, int] | None:
    """``B2:D11`` → ``(2, 2, 11, 4)``。読めなければ ``None``。"""
    found = _RANGE.match(at.strip())
    if not found:
        return None
    top, left = int(found["r1"]), _column(found["c1"])
    bottom = int(found["r2"]) if found["r2"] else top
    right = _column(found["c2"]) if found["c2"] else left
    if bottom < top or right < left:
        return None
    return top, left, bottom, right


def _column(name: str) -> int:
    """``A`` → 1、``AA`` → 27（:func:`arp4.parse.column_name` の逆）。"""
    index = 0
    for letter in name:
        index = index * 26 + (ord(letter) - ord("A") + 1)
    return index


# ── 左右（S001 / S003）──────────────────────────────────────────
def _sideways(blocks: list[Block], where: str) -> list[Finding]:
    """**行の範囲が 1 行も違わない塊が、横に並んでいる。**

    ぴったり一致だけを見るのは、**そこが偶然では起きにくい**からである ――
    1 つの表を左右に切れば両側の行数は必ず揃うが、たまたま隣に置いた別の表が
    1 行も違わないことは多くない。重なり具合で緩めると、方眼紙のシートは
    どこもかしこも「重なっている」ので全部が疑いになる。
    """
    findings: list[Finding] = []
    for group in _rows_alike(blocks):
        if len(group) < 2 or not _apart(group):
            continue
        thin = [one for one in group if one.width == 1]
        if thin:
            findings.append(_shattered_note(group, thin, where))
        else:
            findings.append(_sideways_note(group, where))
    return findings


def _rows_alike(blocks: list[Block]) -> list[list[Block]]:
    """行の範囲が同じものを束ねる。**並びは左から右**（番地の順）。"""
    groups: dict[tuple[int, int], list[Block]] = {}
    for block in blocks:
        groups.setdefault((block.top, block.bottom), []).append(block)
    return [sorted(group, key=lambda one: one.left) for group in groups.values()]


def _apart(group: list[Block]) -> bool:
    """列の範囲が重なっていない（＝横に並んでいる）。"""
    return all(a.right < b.left for a, b in zip(group, group[1:]))


def _shattered_note(group: list[Block], thin: list[Block],
                    where: str) -> Finding:
    """**方眼紙の横結合で砕けた残骸。** ここだけは撮り直せば直る。"""
    return Finding(
        "warn", "S001", _named(group),
        f"行の範囲が 1 行も違わない塊が {len(group)} つ横に並んでいて、"
        f"うち {len(thin)} つが幅 1 列です（{_ats(group)}）。日本の設計書は"
        "論理列 1 本を数列ぶん横結合して作るので、**古い arp4 はここで "
        "1 つの表を列 1 本ずつに砕いていました**。",
        file=where,
        hint="いまの arp4 は砕きません ―― `arp4 parse` で撮り直すと 1 つの表に"
             "戻ります（手を入れたパース結果は守られるので、選んで入れ替える"
             "手順はスキル arp4-repair にあります）。")


def _sideways_note(group: list[Block], where: str) -> Finding:
    return Finding(
        "warn", "S003", _named(group),
        f"行の範囲が 1 行も違わない塊が {len(group)} つ横に並んでいます"
        f"（{_ats(group)}）。**左右に切られた 1 つの表**かもしれません ―― "
        "横に長い表を紙に収めるとき、右半分を別の場所へ置く書き方が実物に"
        "あります。隣に別の表を置いただけのこともあるので、機械には"
        "決められません。",
        file=where,
        hint="原本を開いて 1 つの表かどうかを確かめてください（→ スキル "
             "arp4-repair）。撮り直しでは変わりません ―― 資料の側に空列が"
             "あるので、機械にはそこが切れ目に見えます。")


# ── 上下（S002）────────────────────────────────────────────────
def _paged(blocks: list[Block], where: str) -> list[Finding]:
    """**列の範囲が同じ塊が縦に並び、見出しの行が繰り返されている。**

    ページごとに見出しを打ち直し、あいだを 2 行空ける ―― 綴じて配る設計書で
    いちばん多い書き方である。空行が 2 行あるので機械にはそこが切れ目に見え、
    :data:`arp4.parse._GAP` を超えて別の塊になる。**資料の側に隙間がある**ので、
    ここは撮り直しでは変わらない。
    """
    findings: list[Finding] = []
    for run, head in _runs(blocks):
        findings.append(Finding(
            "warn", "S002", _named(run),
            f"列の範囲が同じ塊が {len(run)} つ縦に並んでいて、見出しの行"
            f"（{' | '.join(head)}）が繰り返されています（{_ats(run)}）。"
            "**ページごとに切られた 1 つの表**かもしれません ―― ページの"
            "変わり目に空行が 2 行あると、機械にはそこが切れ目に見えます。",
            file=where,
            hint="原本を開いて 1 つの表かどうかを確かめてください（→ スキル "
                 "arp4-repair）。撮り直しでは変わりません ―― 資料の側に空行が"
                 "あるので、機械にはそこが切れ目に見えます。"))
    return findings


def _runs(blocks: list[Block]) -> list[tuple[list[Block], list[str]]]:
    """縦に繋がる塊をひと続きにする。**3 ページの表は 1 件で報せる。**

    対ごとに出していたころ、6 ページの表は 5 件になっていた ―― 同じ 1 つの
    ことを 5 回言われると、読み手は件数で重さを測れなくなる。
    """
    found: list[tuple[list[Block], list[str]]] = []
    run: list[Block] = []
    head: list[str] = []
    for previous, block in zip(blocks, blocks[1:]):
        repeated = _repeated(previous, block)
        if repeated is None:
            if len(run) > 1:
                found.append((run, head))
            run, head = [], []
            continue
        if not run:
            run, head = [previous], repeated
        run.append(block)
    if len(run) > 1:
        found.append((run, head))
    return found


def _repeated(above: Block, below: Block) -> list[str] | None:
    """繰り返された見出しの行。**縦に並んでいないなら見ない。**"""
    if above.left != below.left or above.right != below.right:
        return None
    if below.top <= above.bottom:
        return None
    heads = _heads(below)
    for row in _heads(above):
        if row in heads:
            return row
    return None


def _heads(block: Block) -> list[list[str]]:
    """見出しになりうる行。**表題の行（1 列だけ）は数えない。**"""
    return [row for row in block.rows[:_HEAD]
            if sum(1 for cell in row if cell.strip()) >= _HEAD_CELLS]


# ── 出力の体裁 ──────────────────────────────────────────────────
def _named(group: list[Block]) -> str:
    listed = " ".join(one.anchor for one in group[:_LISTED])
    if len(group) <= _LISTED:
        return listed
    return f"{listed} ほか {len(group) - _LISTED}"


def _ats(group: list[Block]) -> str:
    listed = "・".join(one.at for one in group[:_LISTED])
    if len(group) <= _LISTED:
        return listed
    return f"{listed} ほか {len(group) - _LISTED}"
