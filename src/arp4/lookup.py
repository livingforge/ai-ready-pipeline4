"""束を探す ―― 写しを 1 本ずつ塊に割らずに、当たりを塊で返す。

:func:`arp4.show.search` は長いあいだ、写しを全部 :func:`arp4.mdio.read` で塊に
割ってから 1 行ずつ探していた。47 本なら 0.2 秒で済むが、**写しが数万本・
1 GB になると 1 回の検索が分単位**になる ―― 塊に割る仕事は Python の速さで、
しかも当たらない写しにも同じだけ掛かる。

やることは 2 つに割れる。

============  ==================================================================
走査          写しを 1 本ずつ読み、**ファイル全体を先に篩にかけて**、当たった
              写しだけを塊に割る。索引を持たない。束が小さいときの既定
索引          塊を SQLite（FTS5 の trigram）に写しておき、探すときは索引を引く。
              **毎回、写しの姿（大きさ・更新時刻）と突き合わせて**ずれた分だけ
              写し直す。束が大きいときの既定
============  ==================================================================

どちらも**当たりの判定は同じ 1 か所**（:class:`Matcher`）でやる。索引が返すのは
候補の塊で、本文をもう一度同じ判定に通してから当たりにする ―― 索引の作りが
変わっても、当たる・当たらないは走査と 1 件も違わない。

**索引は「保存した索引」ではなく「照合する写し」である。** parse.md は長く
「索引はファイルに保存しない（保存すると `parsed/` を編集した瞬間に古くなり、
写しと索引のどちらが本当かを読み手が決められなくなる）」と書いてきた。理由は
いまも正しい。ここが守るのは結論ではなく理由のほうで、**索引が写しと食い違う
瞬間を作らない** ―― 引く前に必ず写しの姿と突き合わせ、違えば写し直してから
引く。人が `parsed/` を直せば、次の 1 回で索引もそれに追いつく。

索引の置き場は ``.arp/cache/``（自分の ``.gitignore`` を持つ）。**消してよい**
―― 消せば次の検索で作り直す。作り直しは束を 1 度読む時間だけ掛かる。

**索引は片に分ける。** trigram の分かち書きは SQLite の仕事で、1 GB あたり 8〜9 分
掛かる。写しを片（``grep.sqlite`` / ``grep-1.sqlite`` …）に振り分けて別々の
プロセスで写せば、その時間は台数で割れる。引くときは片ごとに引いて並べ直す ――
1 片の答えを足し合わせても、写しは必ずどれか 1 片にしか無いので重ならない。

**表は形を先に読む。** 当たった行が「どの列の値か」を言うには見出しが要るが、
日本の設計書の表は表題行の下に大見出しと小見出しが 2 段で並ぶことがある
（``機密区分`` の下に ``区分`` と ``名称``）。塊の中の表を 1 つずつ
:func:`shape_of` に通し、見出しの段を重ねた列名（``機密区分/区分``）で答える。
"""

from __future__ import annotations

import json
import os
import re
import sqlite3
import time
import unicodedata
import zlib
from concurrent.futures import ProcessPoolExecutor, ThreadPoolExecutor, as_completed
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Callable, Iterable, Iterator, Sequence

from arp4 import fuzzy as fuzzy_module
from arp4 import mdio
from arp4 import organized as organized_module
from arp4 import show
from arp4 import yamlio
from arp4.paths import Paths, Round
from arp4.spec import Spec

#: 索引を使い始める束の大きさ。これより小さければ走査のほうが速い
#: （索引を開いて写しの姿を突き合わせる時間のほうが、読み切る時間より長い）。
INDEX_FROM = 32 * 1024 * 1024

#: 索引の片の数の上限と、1 片の大きさの目安。片は作るときに決めて索引に書き、
#: 後から変えない（片を増やすと写しの振り分けが変わり、全部写し直しになる）。
MAX_SHARDS = 8
SHARD_SIZE = 64 * 1024 * 1024

#: 写し直す本数がこれ以上なら、片ごとに別のプロセスで写す。少なければ 1 つの
#: プロセスで順に写すほうが速い（プロセスを起こす時間のほうが長い）。
PARALLEL_FROM = 200

#: 索引の形の版。変えたら古い索引は捨てて作り直す。
#:
#: 2: trigram は本文だけ・位置を持たない（``detail=none``）。畳んだ本文（loose）は
#:    ``instr`` で流す。実測（525 MB・塊 142 万件）で、両方の列に位置付きの trigram を
#:    張ると索引が 2.3 GB（写しの 4.4 倍）・作るのに 277 秒だった。位置を捨てて
#:    本文だけにすると大きさも時間もほぼ半分で、引く速さは変わらない（候補は
#:    trigram の AND で出し、本文でもう一度確かめるので答えも変わらない）。
#: 3: 畳み方（:func:`normalize`）が桁区切り・日付の区切り・識別子の区切りも落とす
#:    ようになり、``loose`` 列の中身が変わった。索引を片に分けた。
_VERSION = "3"

#: ``.arp/cache/`` の自分用 ``.gitignore``。**プロジェクトの `.arp/.gitignore` に
#: 足さない** ―― 既に `arp4 init` を通した配布先ではそのファイルが書き換わらず、
#: 索引が git に乗る。置き場が自分で自分を無視するほうが確実である。
_IGNORE = "# 検索の索引。消してよい（次の arp4 grep で作り直す）。\n*\n"

#: 写しの名前（相対パス）に当たったときの見出し。塊の見出しと同じ場所に出す。
NAME_HEADING = "写しの名前"


# ── 揺れを畳む ──────────────────────────────────────────────────
#: ``--loose`` で落とす区切り。空白のほかに、桁区切り（``100,000``）・日付の区切り
#: （``2026/02/03`` と ``2026-02-03``）・識別子の区切り（``ORDER_NO`` / ``order-no`` /
#: ``embed/router.py``）を落とす。``.`` は落とさない ―― ``第3.2版`` と ``第32版`` が
#: 同じになる。``・`` も落とさない（``体制・要員`` は語の一部である）。
_DROP = re.compile(r"[\s_\-/,]+")


def normalize(text: str) -> str:
    """全角半角・大小・空白・区切りの揺れを畳む（``--loose``）。

    NFKC で ``ＯＲＤＥＲ`` と ``ORDER``、``ｶﾅ`` と ``カナ`` を揃え、casefold で
    大小を落とし、空白と区切り（:data:`_DROP`）を全部落とす ―― 方眼紙由来の
    ``受注 番号`` と ``受注番号``、``ORDER_NO`` と ``orderNo``、``100,000`` と
    ``100000``、``2026/02/03`` と ``2026-02-03`` は同じ語である。
    **表示には使わない**（当たりの本文は原文のまま出す）。
    """
    return _DROP.sub("", unicodedata.normalize("NFKC", text).casefold())


def normalize_lines(text: str) -> str:
    """行の切れ目を保ったまま畳む。``^`` が行頭に当たる形を崩さない。"""
    return "\n".join(normalize(line) for line in text.splitlines())


def fold_map(text: str) -> tuple[str, list[int]]:
    """畳んだ字と、**その字が原文のどこから来たか**。当たった部分を原文で言うため。

    1 字ずつ畳んで並べるので、まとめて畳んだ結果（:func:`normalize`）と食い違う
    ことが理屈の上ではある（合成文字）。そのときは呼ぶ側が畳んだ字で言う。
    """
    folded: list[str] = []
    origin: list[int] = []
    for index, char in enumerate(text):
        for piece in normalize(char):
            folded.append(piece)
            origin.append(index)
    return "".join(folded), origin


# ── 表の形 ──────────────────────────────────────────────────────
#: 値らしい升 ―― 数・割合・日付・記号。見出しの段は値を持たないので、これが
#: 出てきたら**そこからがデータ行**である（見出しを重ねすぎない番人）。
_DATUM = re.compile(
    r"[+\-−]?[\d,]+(?:\.\d+)?%?"
    r"|\d{4}[/\-.]\d{1,2}(?:[/\-.]\d{1,2})?"
    r"|\d{1,2}[/\-.]\d{1,2}"
    r"|[○◯△×✓✔－ー―]")

#: 見出しとして重ねる段の上限。表題行の下に大見出し・小見出しまでは実在する。
_HEADER_ROWS = 3


@dataclass(frozen=True)
class Shape:
    """塊の中の表 1 つの形 ―― 見出しがどの行で、列がどう名付けられているか。

    ``columns`` は列ごとの名前の段（上の段から）。``機密区分`` の下に ``区分`` と
    ``名称`` が並ぶ表なら ``("機密区分", "区分")`` と ``("機密区分", "名称")``。
    """

    #: 見出しの最初と最後の行（表の中の番号。区切り行は数えない。0 = 見出し無し）。
    first: int = 0
    last: int = 0
    columns: tuple[tuple[str, ...], ...] = ()
    #: 見せる見出し行。段が 1 つなら原文の行、重ねたときは列名を並べ直した行。
    header: str = ""

    @property
    def names(self) -> list[str]:
        """列名。段を ``/`` で重ねる（同じ字が続けば 1 つにする）。"""
        return ["/".join(dict.fromkeys(p for p in parts if p)) for parts in self.columns]

    @property
    def where(self) -> str:
        if not self.first:
            return ""
        return (f"{self.first} 行目" if self.first == self.last
                else f"{self.first}〜{self.last} 行目")


def _filled(cells: Sequence[str]) -> int:
    return sum(1 for value in cells if value)


def _carried(row: Sequence[str], width: int) -> list[str]:
    """大見出しの段の空欄を、**左右を挟まれているときだけ**左の字で埋める。

    結合セルは写しでは左上の 1 升にしか字が無い。``申請部門 | （空） | 機密区分``
    の空欄は左の結合の続きと読んでよいが、右端まで続く空欄は「大見出しの無い列」
    のほうが多い（結合の幅は写しに残らないので、右へは広げない）。
    """
    values = list(row) + [""] * (width - len(row))
    filled = [i for i, value in enumerate(values) if value]
    if not filled:
        return values
    out = list(values)
    current = ""
    for i in range(filled[0], filled[-1] + 1):
        if values[i]:
            current = values[i]
        else:
            out[i] = current
    return out


def _stack(rows: list[list[str]]) -> tuple[tuple[str, ...], ...]:
    width = max(len(row) for row in rows)
    upper = [_carried(row, width) for row in rows[:-1]]
    last = list(rows[-1]) + [""] * (width - len(rows[-1]))
    return tuple(tuple([row[i] for row in upper] + [last[i]]) for i in range(width))


def shape_of(lines: Sequence[str]) -> Shape:
    """表の形を読む。``lines`` は表の行（区切り行は除いてある）。

    見出しは**升が 2 つ以上ある最初の行**から始まる（表題行は升 1 つ）。その行に
    空欄があれば、下の行を見出しの段として重ねる ―― ただし下の行が値らしい升
    （数・日付・記号）を持てば、そこはデータ行なので重ねない。
    """
    rows = [mdio.cells(line) for line in lines]
    start = next((i for i, row in enumerate(rows) if _filled(row) >= 2), None)
    if start is None:
        return Shape()
    stack = [rows[start]]
    last = start
    while len(stack) < _HEADER_ROWS and last + 1 < len(rows):
        if all(any(parts) for parts in _stack(stack)):
            break                                     # 空欄が残っていない
        below = rows[last + 1]
        if _filled(below) < 2 or any(_DATUM.fullmatch(v) for v in below if v):
            break
        stack.append(below)
        last += 1
    columns = _stack(stack)
    if len(stack) == 1:
        header = lines[start]
    else:
        names = ["/".join(dict.fromkeys(p for p in parts if p)) for parts in columns]
        header = "| " + " | ".join(n.replace("|", "\\|").replace("\n", "<br>")
                                   for n in names) + " |"
    return Shape(first=start + 1, last=last + 1, columns=columns, header=header)


def columns_at(shape: Shape, column: str, key: Callable[[str], str]) -> list[int]:
    """``column`` が指す列（複数のことがある）。

    重ねた列名そのもの（``機密区分/区分``）に完全一致するものを先に、無ければ
    **どれかの段**に完全一致するもの全部（``機密区分`` は ``区分`` と ``名称`` の
    2 列）、無ければ含む列が 1 本だけのときそれ。2 本以上に含まれたら決めない
    （決めると違う列を読む）。
    """
    wanted = key(column)
    names = [key(name) for name in shape.names]
    if wanted in names:
        return [i for i, name in enumerate(names) if name == wanted]
    by_part = [i for i, parts in enumerate(shape.columns)
               if any(key(part) == wanted for part in parts if part)]
    if by_part:
        return by_part
    partial = [i for i, name in enumerate(names) if wanted and wanted in name]
    return partial if len(partial) == 1 else []


def _segments(body: str) -> Iterator[tuple[str, Any]]:
    """塊の本文を、表（行の並び）とそれ以外の行に切る。"""
    table: list[tuple[str, str]] = []
    position = 0
    for raw in body.splitlines():
        line = raw.strip()
        if line.startswith("|"):
            position += 1
            # GFM の区切り行は資料に無い。**2 行目に限って**落とす（:func:`arp4.mdio.rows`
            # と同じ）―― 空の行（``|  |  |``）も区切り行の形に見えるので、位置で
            # 見ないと表の 3 行目の空行まで落ち、以降の行番号が 1 つずれる。
            if position == 2 and mdio.separator(line):
                continue
            table.append((raw, line))
            continue
        if table:
            yield "table", table
            table = []
        position = 0
        yield "line", raw
    if table:
        yield "table", table


# ── 当たりの判定 ────────────────────────────────────────────────
@dataclass(frozen=True)
class Found:
    """塊の中で当たった 1 行。"""

    text: str
    #: 表の中なら ``3 行目``。表でなければ空。
    where: str = ""
    #: その行が属する表の見出し行。見出し行そのものが当たったときは空。
    header: str = ""
    #: 見出し行の位置（``2 行目`` / ``4〜5 行目``）。
    header_where: str = ""
    #: 表の列名（見出しの段を重ねたもの）。
    columns: tuple[str, ...] = ()
    #: 当たった字（原文のまま）。
    matched: tuple[str, ...] = ()
    #: 当たった升（列名, 値）。表の中でなければ空。
    cells: tuple[tuple[str, str], ...] = ()
    #: 升の値そのものに当たったか（定義らしい当たり）。
    exact: bool = False


class Matcher:
    """探す語と当たり方。**走査と索引が共有する唯一の判定**である。

    ``terms`` が 2 つ以上なら**同じ塊に全部ある**ことを要求する（論理名と
    物理名が別の列に割れている表を探すため）。見せる行はどれか 1 つに当たった行。
    ``same_row`` なら同じ行（表なら同じ行の升）に全部あることを求める。

    ``aliases`` は語ごとの別名（``--aliases``）。語と別名のどれかに当たれば
    その語に当たったことにする。
    """

    def __init__(self, terms: Iterable[str], *, regex: bool = False,
                 ignore_case: bool = False, loose: bool = False,
                 cell: bool = False, column: str = "", fuzzy: int = 0,
                 same_row: bool = False,
                 aliases: Sequence[Sequence[str]] = ()) -> None:
        self.terms = tuple(t for t in terms if t)
        if not self.terms:
            raise ValueError("探す語がありません")
        if fuzzy and regex:
            raise ValueError("--fuzzy は正規表現（--regex）と併用できません")
        self.regex = regex
        self.ignore_case = ignore_case
        self.loose = loose
        self.cell = cell
        self.column = column
        self.same_row = same_row
        #: 許す編集距離（0 = そのまま）。→ :mod:`arp4.fuzzy`
        self.fuzzy = max(0, fuzzy)
        #: 語ごとの形（語そのものと別名）。
        self.forms: tuple[tuple[str, ...], ...] = tuple(
            (term, *[a for a in (aliases[i] if i < len(aliases) else ())
                     if a and a != term])
            for i, term in enumerate(self.terms))
        # loose は casefold した本文に当てるので、正規表現の側も大小を見ない。
        flags = re.IGNORECASE if (ignore_case or loose) else 0
        #: 索引に渡す語（loose なら畳んだ形）。語ごとに形の組。
        self.keys: tuple[tuple[str, ...], ...] = tuple(
            tuple(normalize(f) if loose else f for f in forms) for forms in self.forms)
        self.patterns: list[re.Pattern[str]] = []
        #: 距離 0..k で当たる形（近さを言うため）。fuzzy でなければ空。
        self._levels: list[list[re.Pattern[str]]] = []
        #: 鳩の巣の片（索引で候補を絞るため）。fuzzy でなければ空。
        self.pieces: list[list[str]] = []
        for term, forms_keys in zip(self.terms, self.keys):
            # 長い形を先に並べる ―― 正規表現の ``|`` は左から最初に当たった形を取る
            # ので、``法務`` を ``法務部`` より先に置くと当たった字が ``法務`` で止まる。
            keys = sorted(forms_keys, key=len, reverse=True)
            try:
                if self.fuzzy:
                    levels = [re.compile(_either(fuzzy_module.pattern(k, level)
                                                 for k in keys), flags)
                              for level in range(self.fuzzy + 1)]
                    self._levels.append(levels)
                    self.patterns.append(levels[-1])
                    pieces = [fuzzy_module.pieces(k, self.fuzzy) for k in keys]
                    # 形のどれかが片に割れなければ、その語では絞れない。
                    self.pieces.append([] if any(not p for p in pieces)
                                       else [x for p in pieces for x in p])
                else:
                    self.patterns.append(re.compile(
                        _either(k if regex else re.escape(k) for k in keys), flags))
            except re.error as exc:
                raise ValueError(f"正規表現として読めません: {term}（{exc}）") from exc
        #: ファイル全体・塊全体に当てる粗い篩。``^``/``$`` を行に効かせる。
        self._coarse = [re.compile(p.pattern, p.flags | re.MULTILINE)
                        for p in self.patterns]

    def key(self, text: str) -> str:
        return normalize(text) if self.loose else text

    def distance(self, text: str) -> int:
        """当たった行の近さ（0 = そのまま当たる）。fuzzy でなければ 0。

        距離 0・1・…の形を順に当てて、最初に当たった段を言う ―― 動的計画法で
        測るより安く、見せる行にしか使わないので十分である。
        """
        if not self.fuzzy:
            return 0
        value = self.key(text)
        best = self.fuzzy
        for levels in self._levels:
            for k, pattern in enumerate(levels[:best]):
                if pattern.search(value):
                    best = k
                    break
        return best

    def maybe(self, text: str) -> bool:
        """写し 1 本（か塊 1 つ）の全体に当てる篩。

        **外すことはあっても落とすことはない** ―― 行ごとの判定で当たるものは
        ここでも必ず当たる（同じ正規表現を行の切れ目を保った全文に当てている）。
        逆は成り立たなくてよく、通ったものは :meth:`matches` がもう一度見る。
        """
        haystack = normalize_lines(text) if self.loose else text
        if self.column and self.key(self.column) not in haystack:
            return False                      # 列名が無い写しにその列は無い
        for parts in self.pieces:
            # 鳩の巣 ―― 距離 k 以内なら、どれか 1 片は無傷で残っている。
            if parts and not any(part in haystack for part in parts):
                return False
        return all(p.search(haystack) for p in self._coarse)

    def spans(self, text: str,
              patterns: Sequence[re.Pattern[str]] | None = None) -> tuple[str, ...]:
        """この行で当たった字を**原文のまま**並べる（同じ字は 1 度）。"""
        origin: list[int] | None = None
        if self.loose:
            folded, origin = fold_map(text)
            haystack = folded
            if folded != normalize(text):
                haystack, origin = normalize(text), None
        else:
            haystack = text
        out: list[str] = []
        for pattern in (patterns if patterns is not None else self.patterns):
            for found in pattern.finditer(haystack):
                if not found.group(0):
                    continue
                if origin is None:
                    piece = found.group(0)
                else:
                    piece = text[origin[found.start()]:origin[found.end() - 1] + 1]
                if piece not in out:
                    out.append(piece)
        return tuple(out)

    def matches(self, anchor: mdio.Anchor) -> list[Found]:
        """塊 1 つを行ごとに見る。**語が全部そろわなければ 0 行。**"""
        found: list[Found] = []
        satisfied = [False] * len(self.patterns)
        for kind, payload in _segments(anchor.body):
            if kind == "line":
                raw = payload
                if self.cell or self.column:
                    continue
                hit = [i for i, p in enumerate(self.patterns) if p.search(self.key(raw))]
                if not self._accept(hit, satisfied):
                    continue
                found.append(Found(text=raw.strip(), matched=self.spans(raw)))
                continue
            table: list[tuple[str, str]] = payload
            shape = shape_of([line for _, line in table])
            chosen: list[int] | None = None
            if self.column:
                chosen = columns_at(shape, self.column, self.key)
                if not chosen:
                    continue                          # この表にその列は無い
            names = shape.names
            for row, (raw, line) in enumerate(table, 1):
                hit, cells, exact = self._in_row(raw, line, row, shape, names, chosen)
                if not self._accept(hit, satisfied):
                    continue
                below = bool(shape.first) and row > shape.last
                found.append(Found(
                    text=line, where=f"{row} 行目",
                    header=shape.header if below else "",
                    header_where=shape.where if below else "",
                    columns=tuple(names) if below else (),
                    matched=self.spans(raw), cells=tuple(cells), exact=exact))
        return found if all(satisfied) else []

    def _accept(self, hit: list[int], satisfied: list[bool]) -> bool:
        if not hit:
            return False
        if self.same_row and len(set(hit)) < len(self.patterns):
            return False
        for index in hit:
            satisfied[index] = True
        return True

    def _in_row(self, raw: str, line: str, row: int, shape: Shape, names: list[str],
                chosen: list[int] | None
                ) -> tuple[list[int], list[tuple[str, str]], bool]:
        """表の 1 行。当たった語の番号・当たった升・升そのものに当たったか。"""
        # 升に割る前に行ごと篩う ―― 升に当たる語は行にも含まれている。升に割る
        # のは Python の仕事で、実測（候補 15 万塊）ではここが検索時間の 7 割だった。
        if not any(p.search(self.key(raw)) for p in self.patterns):
            return [], [], False
        below = bool(shape.first) and row > shape.last
        values = mdio.cells(line)
        if chosen is not None:
            # 見出し行までは当たりにしない（列の名前が探す語に当たるのは普通）。
            if not below:
                return [], [], False
            targets = [(names[c], values[c]) for c in chosen if c < len(values)]
        else:
            targets = [(names[i] if below and i < len(names) else "", v)
                       for i, v in enumerate(values)]
        hit: list[int] = []
        cells: list[tuple[str, str]] = []
        exact = False
        for index, pattern in enumerate(self.patterns):
            for name, value in targets:
                key = self.key(value)
                if not key:
                    continue
                if pattern.fullmatch(key):
                    exact = True
                    matched = True
                elif self.cell:
                    matched = False
                else:
                    matched = pattern.search(key) is not None
                if matched:
                    if index not in hit:
                        hit.append(index)
                    if (name, value) not in cells:
                        cells.append((name, value))
        if chosen is None and not self.cell:
            # 升で見る指定が無いときの当たりは行で決める（升をまたぐ語も当たる）。
            hit = [i for i, p in enumerate(self.patterns) if p.search(self.key(raw))]
        return hit, cells, exact


def _either(patterns: Iterable[str]) -> str:
    return "|".join(f"(?:{p})" for p in patterns)


def hit_of(reference: show.Reference, anchor: mdio.Anchor, matcher: Matcher,
           per_hit: int) -> show.Hit | None:
    """塊 1 つを :class:`arp4.show.Hit` に。当たらなければ ``None``。"""
    found = matcher.matches(anchor)
    if not found:
        return None
    shown = found[:per_hit]
    headed = next((f for f in shown if f.header), None)
    return show.Hit(reference=reference, heading=show.heading_of(anchor),
                    lines=[f.text for f in shown], total=len(found),
                    header=headed.header if headed else "",
                    header_where=headed.header_where if headed else "",
                    columns=list(headed.columns) if headed else [],
                    where=[f.where for f in shown],
                    matched=[list(f.matched) for f in shown],
                    cells=[list(f.cells) for f in shown],
                    exact=sum(1 for f in found if f.exact),
                    distance=min(matcher.distance(f.text) for f in shown))


# ── 別名 ────────────────────────────────────────────────────────
def aliases_of(paths: Paths, terms: Sequence[str], loose: bool = False,
               round_name: str = "") -> list[list[str]]:
    """語ごとの別名（``--aliases``）。**整理結果の側に書いてある同じもの**で探す。

    見るのは各ラウンドの ``_concepts.yml``（``new`` / ``assign`` の ``label`` と
    ``aliases``）と、正本の台帳 ``.arp/spec/concepts.yml``。語が ``label`` /
    ``aliases`` / ``concept`` のどれかに一致する項目の、残りの名前が別名になる。
    concept の識別子（``c-…``）は資料に出てこないので別名には入れない。
    """
    key = normalize if loose else (lambda s: s.strip())
    entries: list[dict[str, Any]] = []
    for round_ in paths.rounds():
        if round_name and round_.name != round_name:
            continue
        path = round_.organized / "_concepts.yml"
        if path.is_file():
            data = yamlio.load(path)
            if isinstance(data, dict):
                for kind in ("new", "assign"):
                    value = data.get(kind)
                    if isinstance(value, list):
                        entries += [v for v in value if isinstance(v, dict)]
    ledger = paths.spec / "concepts.yml"
    if ledger.is_file():
        data = yamlio.load(ledger)
        if isinstance(data, list):
            entries += [v for v in data if isinstance(v, dict)]
    out: list[list[str]] = []
    for term in terms:
        wanted = key(term)
        names: list[str] = []
        for entry in entries:
            label = str(entry.get("label") or "")
            concept = str(entry.get("concept") or "")
            aliases = [str(a) for a in (entry.get("aliases") or []) if a is not None]
            forms = [f for f in (label, *aliases) if f]
            if not any(key(f) == wanted for f in (*forms, concept) if f):
                continue
            for form in forms:
                if key(form) != wanted and form not in names:
                    names.append(form)
        out.append(names)
    return out


# ── 束の姿 ──────────────────────────────────────────────────────
@dataclass(frozen=True)
class Entry:
    """写し 1 本の姿。索引はこれと突き合わせて古さを決める。"""

    round: str
    file: str
    path: Path
    size: int
    mtime_ns: int
    #: **いま読むべき版か**（同じ写しが 2 つのラウンドにあれば新しいほうだけ
    #: → :func:`arp4.show.corpus` と同じ決め方）。
    live: bool = True


def walk(paths: Paths) -> list[Entry]:
    """全ラウンドの写しを、姿ごと並べる。**並びは決定的。**

    :func:`arp4.mdio.scan`（``rglob``）ではなく ``os.scandir`` で歩くのは、
    Windows では scandir がディレクトリ一覧から大きさと更新時刻を一緒に返す
    からである ―― 1 本ずつ ``stat`` すると、数万本で秒が要る。
    """
    entries: dict[tuple[str, str], Entry] = {}
    newest: dict[str, str] = {}
    for round_ in paths.rounds():                 # 古い順
        parsed = round_.parsed
        if not parsed.is_dir():
            continue
        # 相対パスは文字列で切る。``Path(...).relative_to(...).with_suffix("")``
        # は 1 本 60 µs で、**2 万本を歩く時間の 8 割**がそこだった（実測）。
        prefix = len(str(parsed)) + 1
        for path, stat in _scan(str(parsed)):
            file = path[prefix:-len(mdio.EXT)].replace(os.sep, "/")
            entries[(round_.name, file)] = Entry(
                round=round_.name, file=file, path=Path(path),
                size=stat.st_size, mtime_ns=stat.st_mtime_ns)
            newest[file] = round_.name                # 後勝ち＝新しいラウンド
    return [Entry(e.round, e.file, e.path, e.size, e.mtime_ns,
                  live=(newest[e.file] == e.round))
            for e in sorted(entries.values(), key=lambda e: (e.file, e.round))]


def _scan(root: str) -> Iterable[tuple[str, os.stat_result]]:
    stack = [root]
    while stack:
        directory = stack.pop()
        with os.scandir(directory) as it:
            for entry in it:
                if entry.is_dir(follow_symlinks=False):
                    stack.append(entry.path)
                elif entry.name.endswith(mdio.EXT) and entry.is_file():
                    yield entry.path, entry.stat()


def in_scope(entries: list[Entry], round_name: str, only: str = "") -> list[Entry]:
    """探す範囲。ラウンドを指せばその全部、指さなければ**いま読むべき版**。
    ``only`` は写しの相対パスに含まれる語（``--path``）。"""
    chosen = ([e for e in entries if e.round == round_name] if round_name
              else [e for e in entries if e.live])
    return [e for e in chosen if only in e.file] if only else chosen


# ── 写しの名前 ──────────────────────────────────────────────────
def name_hits(entries: list[Entry], matcher: Matcher) -> list[show.Hit]:
    """写しの名前（相対パス。冊子名とシート名を含む）に当たったもの。

    塊の本文にはシート名が無い ―― ``申請一覧`` で探しても、シート
    ``1.申請一覧`` の中に同じ字が無ければ届かなかった。名前の当たりは
    アンカーの無い出典（``r001 資料/…/1.申請一覧``）で返す。升や列で見る
    指定のときは出さない（名前に升は無い）。
    """
    if matcher.cell or matcher.column:
        return []
    # 名前はあいまいにしない ―― 名前は短く、``受注`` の 2 字違いは何にでも当たる。
    patterns = ([levels[0] for levels in matcher._levels] if matcher.fuzzy
                else matcher.patterns)
    hits: list[show.Hit] = []
    for entry in entries:
        key = matcher.key(entry.file)
        if not all(p.search(key) for p in patterns):
            continue
        stem = entry.file.rsplit("/", 1)[-1]
        hits.append(show.Hit(
            reference=show.Reference(file=entry.file, round=entry.round),
            heading=NAME_HEADING, lines=[entry.file], total=1, where=[""],
            matched=[list(matcher.spans(entry.file, patterns))], cells=[[]],
            exact=int(any(p.fullmatch(matcher.key(stem)) for p in patterns)),
            kind="name"))
    return hits


# ── 走査 ────────────────────────────────────────────────────────
def scan(entries: list[Entry], matcher: Matcher, per_hit: int) -> list[show.Hit]:
    """写しを 1 本ずつ読む。**篩を通った写しだけ**塊に割る。"""
    hits: list[show.Hit] = []
    for entry in entries:
        text = entry.path.read_text(encoding="utf-8")
        if not matcher.maybe(text):
            continue
        document = mdio.read_text(text, entry.path)
        for anchor in document.anchors:
            hit = hit_of(show.Reference(file=entry.file, anchor=anchor.id,
                                        round=entry.round), anchor, matcher, per_hit)
            if hit is not None:
                hits.append(hit)
    return hits


# ── 索引 ────────────────────────────────────────────────────────
@dataclass
class Refreshed:
    """突き合わせの結果。**何もしなかったときも 0 と言う。**"""

    added: int = 0
    updated: int = 0
    removed: int = 0
    seconds: float = 0.0

    @property
    def changed(self) -> int:
        return self.added + self.updated + self.removed

    def absorb(self, other: "Refreshed") -> None:
        self.added += other.added
        self.updated += other.updated
        self.removed += other.removed


_SCHEMA = """
CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS files(
    id INTEGER PRIMARY KEY, round TEXT NOT NULL, file TEXT NOT NULL,
    size INTEGER NOT NULL, mtime_ns INTEGER NOT NULL, live INTEGER NOT NULL,
    UNIQUE(round, file));
CREATE TABLE IF NOT EXISTS chunks(
    id INTEGER PRIMARY KEY, file_id INTEGER NOT NULL, anchor TEXT NOT NULL,
    at TEXT NOT NULL, body TEXT NOT NULL, loose TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS chunks_file ON chunks(file_id);
CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts USING fts5(
    body, content='chunks', content_rowid='id', tokenize='trigram', detail=none);
CREATE TRIGGER IF NOT EXISTS chunks_ai AFTER INSERT ON chunks BEGIN
    INSERT INTO chunks_fts(rowid, body) VALUES (new.id, new.body);
END;
CREATE TRIGGER IF NOT EXISTS chunks_ad AFTER DELETE ON chunks BEGIN
    INSERT INTO chunks_fts(chunks_fts, rowid, body) VALUES ('delete', old.id, old.body);
END;
"""


def trigrams(key: str) -> str:
    """trigram 索引への問い ―― 語に含まれる 3 文字の並びを全部 AND する。

    位置を持たない索引（``detail=none``）では語をそのまま句として引けないので、
    3 文字ずつに割って**全部含む塊**を候補にする。並び順までは見ないので候補は
    広めに出るが、本文でもう一度確かめる（:func:`hit_of`）ので答えは狂わない。
    """
    parts = {key[i:i + 3] for i in range(len(key) - 2)}
    return " AND ".join('"' + p.replace('"', '""') + '"' for p in sorted(parts))


def _contains(key: str, matcher: Matcher) -> tuple[str, Any]:
    """「この語を含む塊」の SQL。索引で引けるものは引き、残りは C の速さで流す。"""
    if not matcher.loose and len(key) >= 3:
        # trigram は 3 文字以上の語を索引から引ける。大小を見ない索引なので候補は
        # 広めに出るが、本文でもう一度見るので狂わない。
        return ("c.id IN (SELECT rowid FROM chunks_fts WHERE chunks_fts MATCH ?)",
                trigrams(key))
    if matcher.loose:
        return "instr(c.loose, ?) > 0", key
    if matcher.ignore_case:
        return "instr(lower(c.body), ?) > 0", key.lower()
    # 2 文字以下の語は索引で絞れないので流す（片に分けてあれば片ごとに並行して
    # 流すので、その時間も片の数で割れる）。
    return "instr(c.body, ?) > 0", key


def cache_path(paths: Paths) -> Path:
    return paths.arp / "cache" / "grep.sqlite"


def shard_paths(base: Path, count: int) -> list[Path]:
    """片の置き場。1 片目は ``grep.sqlite``、2 片目から ``grep-1.sqlite`` …。"""
    return [base] + [base.with_name(f"{base.stem}-{i}{base.suffix}")
                     for i in range(1, count)]


def clear_cache(base: Path) -> None:
    """索引を片ごと捨てる（WAL の付属ファイルも）。"""
    for path in base.parent.glob(f"{base.stem}*{base.suffix}*"):
        path.unlink(missing_ok=True)


def shards_for(size: int) -> int:
    """束の大きさから片の数を決める。小さな束は 1 片（開く手間のほうが高い）。"""
    wanted = -(-size // SHARD_SIZE)
    return max(1, min(MAX_SHARDS, os.cpu_count() or 1, wanted))


@dataclass
class Plan:
    """片 1 つの写し直しの計画。"""

    stale: list[Entry] = field(default_factory=list)
    known: dict[tuple[str, str], tuple[int, int, int, bool]] = field(default_factory=dict)
    gone: list[int] = field(default_factory=list)
    relive: list[tuple[int, int]] = field(default_factory=list)


class Shard:
    """索引の片 1 つ。写しは ``file`` の指紋で片に振り分けられる。"""

    def __init__(self, path: Path) -> None:
        self.path = path
        self.conn = sqlite3.connect(path)
        self.conn.execute("PRAGMA journal_mode = WAL")
        self.conn.execute("PRAGMA synchronous = NORMAL")
        self.conn.executescript(_SCHEMA)

    def stamp(self, shards: int) -> None:
        self.conn.executemany("INSERT OR REPLACE INTO meta(key, value) VALUES (?, ?)",
                              [("version", _VERSION), ("shards", str(shards))])
        self.conn.commit()

    def close(self) -> None:
        self.conn.close()

    def plan(self, entries: list[Entry]) -> Plan:
        """写しの姿と突き合わせ、何を写し直すかを決める。

        比べるのは大きさと更新時刻である。中身の指紋を取れば確実だが、それには
        写しを全部読むことになり、索引を持つ意味が無くなる。大きさも時刻も同じ
        まま中身だけ変わる書き方（時刻を保って上書きする道具）は異例なので、
        そのときは ``--reindex`` で作り直す。
        """
        plan = Plan()
        plan.known = {(round_, file): (id_, size, mtime, bool(live))
                      for id_, round_, file, size, mtime, live
                      in self.conn.execute("SELECT id, round, file, size, mtime_ns, live "
                                           "FROM files")}
        plan.stale = [e for e in entries
                      if (e.round, e.file) not in plan.known
                      or plan.known[(e.round, e.file)][1:3] != (e.size, e.mtime_ns)]
        seen = {(e.round, e.file) for e in entries}
        plan.gone = [row[0] for key, row in plan.known.items() if key not in seen]
        stale_keys = {(s.round, s.file) for s in plan.stale}
        for entry in entries:
            row = plan.known.get((entry.round, entry.file))
            if row is not None and row[3] != entry.live \
                    and (entry.round, entry.file) not in stale_keys:
                plan.relive.append((int(entry.live), row[0]))
        return plan

    def apply(self, plan: Plan, progress: Callable[[int, int], None] | None = None,
              batch: int = 500) -> Refreshed:
        result = Refreshed()
        done = 0
        for entry in plan.stale:
            row = plan.known.get((entry.round, entry.file))
            if row is None:
                result.added += 1
            else:
                result.updated += 1
                self._drop(row[0])
            self._put(entry)
            done += 1
            if done % batch == 0:
                self.conn.commit()
                if progress is not None:
                    progress(done, len(plan.stale))
        for file_id in plan.gone:
            self._drop(file_id)
            result.removed += 1
        self.conn.executemany("UPDATE files SET live = ? WHERE id = ?", plan.relive)
        self.conn.commit()
        return result

    def refresh(self, entries: list[Entry],
                progress: Callable[[int, int], None] | None = None) -> Refreshed:
        return self.apply(self.plan(entries), progress)

    def _drop(self, file_id: int) -> None:
        self.conn.execute("DELETE FROM chunks WHERE file_id = ?", (file_id,))
        self.conn.execute("DELETE FROM files WHERE id = ?", (file_id,))

    def _put(self, entry: Entry) -> None:
        document = mdio.read(entry.path)
        cursor = self.conn.execute(
            "INSERT INTO files(round, file, size, mtime_ns, live) VALUES (?, ?, ?, ?, ?)",
            (entry.round, entry.file, entry.size, entry.mtime_ns, int(entry.live)))
        file_id = cursor.lastrowid
        self.conn.executemany(
            "INSERT INTO chunks(file_id, anchor, at, body, loose) VALUES (?, ?, ?, ?, ?)",
            [(file_id, a.id, a.at, a.body, normalize_lines(a.body))
             for a in document.anchors])

    def hits(self, matcher: Matcher, round_name: str, per_hit: int,
             only: str = "") -> list[tuple[str, int, show.Hit]]:
        """候補を索引から引き、**本文を同じ判定にもう一度通して**当たりにする。

        返すのは ``(写し, 塊の番号, 当たり)`` ―― 片をまたいで並べ直すため。
        接続はここで開く（片ごとに別のスレッドから引くので、開いてある接続は
        使わない）。
        """
        conn = sqlite3.connect(self.path)
        try:
            return self._hits(conn, matcher, round_name, per_hit, only)
        finally:
            conn.close()

    def _hits(self, conn: sqlite3.Connection, matcher: Matcher, round_name: str,
              per_hit: int, only: str) -> list[tuple[str, int, show.Hit]]:
        where = ["f.round = ?"] if round_name else ["f.live = 1"]
        params: list[Any] = [round_name] if round_name else []
        if only:
            where.append("instr(f.file, ?) > 0")
            params.append(only)
        column = "loose" if matcher.loose else "body"
        if matcher.regex:
            coarse = matcher._coarse
            conn.create_function(
                "arp4_regexp", 2,
                lambda index, text: coarse[index].search(text) is not None,
                deterministic=True)
        for index, keys in enumerate(matcher.keys):
            if matcher.regex:
                where.append(f"arp4_regexp(?, c.{column})")
                params.append(index)
                continue
            clauses: list[str] = []
            if matcher.fuzzy:
                # 鳩の巣 ―― 片のどれかを含む塊が候補。片に割れない短い語は
                # 絞れないので全部確かめる（黙って落とすより遅いほうを取る）。
                pieces = matcher.pieces[index]
            else:
                pieces = list(keys)
            for piece in pieces:
                clause, value = _contains(piece, matcher)
                clauses.append(clause)
                params.append(value)
            if clauses:
                where.append("(" + " OR ".join(clauses) + ")")
        if matcher.column:
            # 列で見るなら、その列名も塊に書いてあるはずである。``極秘`` のような
            # 2 文字の語は索引で絞れないが、列名（``機密区分``）は絞れる ――
            # 候補が 15 万塊から表のある塊だけに落ちる。
            clause, value = _contains(matcher.key(matcher.column), matcher)
            where.append(clause)
            params.append(value)
        rows = conn.execute(
            "SELECT f.round, f.file, c.id, c.anchor, c.at, c.body FROM chunks c "
            "JOIN files f ON f.id = c.file_id "
            f"WHERE {' AND '.join(where)} ORDER BY f.file, c.id", params)
        hits: list[tuple[str, int, show.Hit]] = []
        for round_, file, chunk_id, anchor_id, at, body in rows:
            hit = hit_of(show.Reference(file=file, anchor=anchor_id, round=round_),
                         mdio.Anchor(id=anchor_id, at=at, body=body), matcher, per_hit)
            if hit is not None:
                hits.append((file, chunk_id, hit))
        return hits

    def counts(self) -> tuple[int, int]:
        """``(写し, 塊)`` の本数。"""
        files = self.conn.execute("SELECT count(*) FROM files").fetchone()[0]
        chunks = self.conn.execute("SELECT count(*) FROM chunks").fetchone()[0]
        return files, chunks


def _refresh_shard(path: Path, entries: list[Entry]) -> Refreshed:
    """片 1 つを別のプロセスで写し直す（:class:`ProcessPoolExecutor` の仕事）。"""
    shard = Shard(path)
    try:
        return shard.refresh(entries)
    finally:
        shard.close()


class Index:
    """塊の索引（片の束）。開いたら :meth:`refresh` を通してから :meth:`hits` を引く。

    片の数は最初に作るときに束の大きさから決めて ``meta`` に書く。形の版が違う
    索引や、片が欠けた索引は直さず捨てる ―― 作り直しは束を 1 度読むだけ。
    """

    def __init__(self, path: Path, size_hint: int = 0) -> None:
        self.path = path
        path.parent.mkdir(parents=True, exist_ok=True)
        ignore = path.parent / ".gitignore"
        if not ignore.exists():
            ignore.write_text(_IGNORE, encoding="utf-8", newline="\n")
        count = self._existing()
        if count is None:
            clear_cache(path)
            count = shards_for(size_hint)
        self.shards = [Shard(p) for p in shard_paths(path, count)]
        for shard in self.shards:
            shard.stamp(count)

    def _existing(self) -> int | None:
        """既にある索引の片の数。版が違う・片が欠けているなら ``None``（捨てる）。"""
        if not self.path.exists():
            return None
        conn = sqlite3.connect(self.path)
        try:
            has_meta = conn.execute(
                "SELECT name FROM sqlite_master WHERE type='table' AND name='meta'"
            ).fetchone()
            if has_meta is None:
                return None
            meta = dict(conn.execute("SELECT key, value FROM meta"))
        finally:
            conn.close()
        if meta.get("version") != _VERSION:
            return None
        count = int(meta.get("shards") or 1)
        if not all(p.exists() for p in shard_paths(self.path, count)):
            return None
        return count

    @property
    def count(self) -> int:
        return len(self.shards)

    def close(self) -> None:
        for shard in self.shards:
            shard.close()

    def _groups(self, entries: list[Entry]) -> list[list[Entry]]:
        groups: list[list[Entry]] = [[] for _ in self.shards]
        for entry in entries:
            groups[zlib.crc32(entry.file.encode("utf-8")) % len(groups)].append(entry)
        return groups

    def refresh(self, entries: list[Entry],
                progress: Callable[[int, int], None] | None = None) -> Refreshed:
        """写しの姿と突き合わせ、ずれた分だけ写し直す。**引く前に必ず通す。**

        写し直す本数が :data:`PARALLEL_FROM` 以上で片が 2 つ以上あれば、片ごとに
        別のプロセスで写す（trigram の分かち書きは SQLite の CPU 仕事なので、
        台数で割れる）。
        """
        started = time.monotonic()
        groups = self._groups(entries)
        plans = [shard.plan(group) for shard, group in zip(self.shards, groups)]
        total = sum(len(plan.stale) for plan in plans)
        result = Refreshed()
        if self.count > 1 and total >= PARALLEL_FROM:
            self.close()
            done = 0
            with ProcessPoolExecutor(max_workers=self.count) as pool:
                futures = {pool.submit(_refresh_shard, shard.path, group): len(plan.stale)
                           for shard, group, plan in zip(self.shards, groups, plans)}
                for future in as_completed(futures):
                    result.absorb(future.result())
                    done += futures[future]
                    if progress is not None:
                        progress(done, total)
            self.shards = [Shard(shard.path) for shard in self.shards]
        else:
            offset = 0
            for shard, plan in zip(self.shards, plans):
                base = offset
                result.absorb(shard.apply(
                    plan, progress=(lambda d, _t, base=base: progress(base + d, total))
                    if progress is not None else None))
                offset += len(plan.stale)
        result.seconds = time.monotonic() - started
        return result

    def hits(self, matcher: Matcher, round_name: str, per_hit: int,
             only: str = "") -> list[show.Hit]:
        """片ごとに引いて、写しの並びに戻す。片が複数ならスレッドで並行して引く
        （SQLite は問い合わせの間 GIL を放すので、``instr`` の走査も台数で割れる）。"""
        if self.count == 1:
            rows = self.shards[0].hits(matcher, round_name, per_hit, only)
        else:
            with ThreadPoolExecutor(max_workers=self.count) as pool:
                rows = [row for part in pool.map(
                    lambda s: s.hits(matcher, round_name, per_hit, only), self.shards)
                    for row in part]
            rows.sort(key=lambda row: (row[0], row[1]))
        return [hit for _, _, hit in rows]

    def counts(self) -> tuple[int, int]:
        files = chunks = 0
        for shard in self.shards:
            f, c = shard.counts()
            files += f
            chunks += c
        return files, chunks


# ── 入口 ────────────────────────────────────────────────────────
@dataclass
class Outcome:
    """探した結果と、**どう探したか**。"""

    hits: list[show.Hit]
    #: 見た写しの本数（0 件のときに「見ていない」と取り違えないため）。
    files: int
    #: ``scan`` か ``index``。
    mode: str
    refreshed: Refreshed | None = None


def run(paths: Paths, matcher: Matcher, round_name: str = "", per_hit: int = 3,
        mode: str = "auto", reindex: bool = False, only: str = "",
        progress: Callable[[int, int], None] | None = None,
        sort: str = "path") -> Outcome:
    """束を探す。``mode`` は ``auto`` / ``scan`` / ``index``。

    ``auto`` は、索引が既にあるか束が :data:`INDEX_FROM` より大きいときに索引を
    使う。小さい束で索引を開くと、突き合わせの時間が読み切る時間を上回る。
    ``sort`` は ``path``（写しの並び）か ``cell``（升そのものに当たったものを先に）。
    """
    entries = walk(paths)
    scope = in_scope(entries, round_name, only)
    cache = cache_path(paths)
    if reindex:
        clear_cache(cache)
    size = sum(e.size for e in entries)
    use_index = mode == "index" or (
        mode == "auto" and (cache.exists() or size >= INDEX_FROM))
    names = name_hits(scope, matcher)
    if not use_index:
        return Outcome(hits=_ranked(names + scan(scope, matcher, per_hit), matcher, sort),
                       files=len(scope), mode="scan")

    index = Index(cache, size_hint=size)
    try:
        refreshed = index.refresh(entries, progress=progress)
        hits = index.hits(matcher, round_name, per_hit, only)
    finally:
        index.close()
    return Outcome(hits=_ranked(names + hits, matcher, sort), files=len(scope),
                   mode="index", refreshed=refreshed)


def _ranked(hits: list[show.Hit], matcher: Matcher, sort: str = "path") -> list[show.Hit]:
    """あいまい検索は**近い順**。``cell`` なら升そのものに当たったものを先に。
    同じ順位の中は写しの並び（安定ソート。名前の当たりが先）。"""
    if sort == "cell":
        hits.sort(key=lambda h: (h.distance, -h.exact))
    elif matcher.fuzzy:
        hits.sort(key=lambda h: h.distance)
    return hits


def build(paths: Paths, reindex: bool = False,
          progress: Callable[[int, int], None] | None = None
          ) -> tuple[Refreshed, tuple[int, int]]:
    """索引だけを作る・追いつかせる（``arp4 index --cache``）。"""
    cache = cache_path(paths)
    if reindex:
        clear_cache(cache)
    entries = walk(paths)
    index = Index(cache, size_hint=sum(e.size for e in entries))
    try:
        refreshed = index.refresh(entries, progress=progress)
        counts = index.counts()
    finally:
        index.close()
    return refreshed, counts


# ── ラウンド間の差 ──────────────────────────────────────────────
@dataclass
class Diff:
    """同じ語の当たりが、撮り直したラウンドでどう動いたか。"""

    old: str
    new: str
    #: 新しいラウンドにだけある当たり。
    added: list[show.Hit] = field(default_factory=list)
    #: 古いラウンドにあって新しいラウンドで消えた当たり（撮り直した写しの範囲）。
    removed: list[show.Hit] = field(default_factory=list)
    #: 同じ塊で当たった行が変わったもの（古, 新）。
    changed: list[tuple[show.Hit, show.Hit]] = field(default_factory=list)
    same: int = 0
    #: 新しいラウンドで撮り直した写しの本数（比べた範囲）。
    files: int = 0


def diff(paths: Paths, matcher: Matcher, old: str, new: str, per_hit: int = 3,
         mode: str = "auto", only: str = "") -> Diff:
    """2 つのラウンドで同じ語を探し、**撮り直した写しの範囲で**当たりを突き合わせる。

    古いラウンドの当たりは、新しいラウンドに同じ写しがあるものだけを比べる ――
    撮り直していない写しは束の姿として新しいラウンドにも残っているので、
    「消えた」と言うと嘘になる。塊の番号（アンカー）で突き合わせるので、
    撮り直しで塊の切れ目が動いた表は「消えた」と「増えた」の両方に出る。
    """
    entries = walk(paths)
    rounds = {e.round for e in entries}
    for name in (old, new):
        if name not in rounds:
            known = "・".join(sorted(rounds)) or "（1 つもありません）"
            raise ValueError(f"ラウンド {name} にパース結果がありません（あるラウンド: {known}）")
    new_files = {e.file for e in entries if e.round == new}
    before = {(h.reference.file, h.reference.anchor): h
              for h in run(paths, matcher, old, per_hit, mode, only=only).hits
              if h.reference.file in new_files}
    after = {(h.reference.file, h.reference.anchor): h
             for h in run(paths, matcher, new, per_hit, mode, only=only).hits}
    result = Diff(old=old, new=new,
                  files=len([f for f in new_files if only in f]) if only else len(new_files))
    for key, hit in after.items():
        was = before.get(key)
        if was is None:
            result.added.append(hit)
        elif (was.lines, was.total) != (hit.lines, hit.total):
            result.changed.append((was, hit))
        else:
            result.same += 1
    result.removed = [hit for key, hit in before.items() if key not in after]
    return result


# ── 整理結果の側を探す ──────────────────────────────────────────
@dataclass
class OrganizedHit:
    """整理結果で当たったレコード（か concept）1 件。"""

    round: str
    file: str                        # organized からの相対（拡張子なし）
    anchor: str
    concept: str
    type: str
    name: str
    #: 当たった欄と、その値。
    fields: list[tuple[str, str]] = field(default_factory=list)

    @property
    def where(self) -> str:
        return f"{self.round} {self.file}" + (f"#{self.anchor}" if self.anchor else "")


def in_organized(paths: Paths, matcher: Matcher, round_name: str = "") -> list[OrganizedHit]:
    """整理結果を探す ―― **同じ語が既にどの concept になっているか。**

    見るのはレコードの ``concept`` / ``name`` / ``statement`` / ``attrs`` / 関係の
    相手と本文、それに ``_concepts.yml`` の ``label`` / ``aliases``。二重登録を
    探すとき、素の grep では ``concept:`` の行しか引けず、別名で書かれた同じもの
    には届かなかった（reconcile.md がやらせていた手順）。
    """
    found: list[OrganizedHit] = []
    for round_ in paths.rounds():
        if round_name and round_.name != round_name:
            continue
        if not round_.organized.is_dir():
            continue
        result, _ = organized_module.load(round_)
        for record in result.records:
            fields = [("concept", record.concept), ("name", record.name),
                      ("statement", record.statement)]
            fields += [(f"attrs.{k}", _flat(v)) for k, v in record.attrs.items()]
            fields += [(f"refs.{r.rel}", " ".join(x for x in (r.to, r.note) if x))
                       for r in record.refs]
            matched = _fields(matcher, fields)
            if matched:
                found.append(OrganizedHit(round=round_.name, file=record.file,
                                          anchor=record.anchor, concept=record.concept,
                                          type=record.type, name=record.name,
                                          fields=matched))
        for entry in _concept_entries(result.concepts):
            fields = [("concept", str(entry.get("concept") or "")),
                      ("label", str(entry.get("label") or "")),
                      ("aliases", _flat(entry.get("aliases") or []))]
            matched = _fields(matcher, fields)
            if matched:
                found.append(OrganizedHit(round=round_.name, file="_concepts.yml",
                                          anchor="", concept=str(entry.get("concept") or ""),
                                          type=str(entry.get("type") or ""),
                                          name=str(entry.get("label") or ""),
                                          fields=matched))
    return found


def _concept_entries(concepts: dict[str, Any]) -> list[dict[str, Any]]:
    entries: list[dict[str, Any]] = []
    for key in ("new", "assign"):
        value = concepts.get(key) if isinstance(concepts, dict) else None
        if isinstance(value, list):
            entries += [v for v in value if isinstance(v, dict)]
    return entries


def _flat(value: Any) -> str:
    if isinstance(value, (list, tuple)):
        return " / ".join(_flat(v) for v in value)
    if isinstance(value, dict):
        return " / ".join(f"{k}: {_flat(v)}" for k, v in value.items())
    return "" if value is None else str(value)


def _fields(matcher: Matcher, fields: list[tuple[str, str]]) -> list[tuple[str, str]]:
    """語が全部そろう欄の組。**そろわなければ空**（塊と同じ規律）。"""
    matched: list[tuple[str, str]] = []
    satisfied = [False] * len(matcher.patterns)
    for name, value in fields:
        if not value:
            continue
        hit = [i for i, p in enumerate(matcher.patterns) if p.search(matcher.key(value))]
        if hit:
            for index in hit:
                satisfied[index] = True
            matched.append((name, value))
    return matched if all(satisfied) else []


# ── この塊は何になったか ────────────────────────────────────────
class Uses:
    """塊 → それを出典にしているもの。**整理結果と正本の両方**を見る。

    :func:`arp4.show.used_by` は正本しか見ない。整理の最中に知りたいのは
    「もう誰かが書いたか」で、それはまだ ``build`` していない ``organized/``
    にしか無い。どちらも遅延で読む ―― 探すたびに整理結果を全部読むと、
    束が大きいとき検索そのものより重くなる（だから ``--used`` は任意）。
    """

    def __init__(self, paths: Paths, spec: Spec | None = None) -> None:
        self._paths = paths
        self._spec = spec
        self._organized: dict[tuple[str, str], list[str]] | None = None
        self._built: dict[tuple[str, str], list[str]] | None = None

    def of(self, reference: show.Reference) -> list[str]:
        key = (reference.file, reference.anchor)
        labels = list(self._from_organized().get(key, []))
        for label in self._from_spec().get(key, []):
            if label not in labels:
                labels.append(label)
        return labels

    def _from_organized(self) -> dict[tuple[str, str], list[str]]:
        if self._organized is None:
            self._organized = {}
            for round_ in self._paths.rounds():
                if not round_.organized.is_dir():
                    continue
                result, _ = organized_module.load(round_)
                for record in result.records:
                    self._organized.setdefault(
                        (record.file, record.anchor), []).append(record.concept)
                for declared in result.out_of_scope:
                    self._organized.setdefault(
                        (declared.file, declared.anchor), []).append("（対象外）")
        return self._organized

    def _from_spec(self) -> dict[tuple[str, str], list[str]]:
        if self._built is None:
            self._built = {}
            if self._spec is not None:
                for record in list(self._spec.items) + list(self._spec.relations):
                    label = show.use_label(self._spec, record)
                    for source in show.sources_of(record):
                        self._built.setdefault((source.file, source.anchor), []).append(label)
        return self._built


def to_json(hit: show.Hit, change: str = "") -> str:
    """1 件 1 行の JSON。エージェントが数えたり突き合わせたりするための形。"""
    data: dict[str, Any] = {
        "reference": str(hit.reference), "round": hit.reference.round,
        "file": hit.reference.file, "anchor": hit.reference.anchor,
        "kind": hit.kind, "heading": hit.heading,
        "header": hit.header, "header_where": hit.header_where, "columns": hit.columns,
        "lines": [{"where": w, "text": t, "matched": m,
                   "cells": [{"column": c, "value": v} for c, v in cells]}
                  for w, t, m, cells in zip(hit.where, hit.lines, hit.matched, hit.cells)],
        "total": hit.total, "exact": hit.exact, "distance": hit.distance,
        "uses": hit.uses}
    if change:
        data = {"change": change, **data}
    return json.dumps(data, ensure_ascii=False)
