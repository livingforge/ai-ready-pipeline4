"""出典を開く ―― **索引の逆関数**。

設計書の出典セルには ``r001 資料/A/基本設計書.xlsx/受注テーブル#s1-t1`` と出る
（→ :func:`arp4.publish._source`）。これが指しているのは
``.arp/rounds/r001/parsed/資料/A/基本設計書.xlsx/受注テーブル.md`` の ``s1-t1``
で、**辿る道具は最初からあった** ―― :func:`arp4.mdio.read` の ``by_id`` が
アンカー 1 件を本文ごと返し、:mod:`arp4.trace` は正本の出典全件をそれで照合して
いる。無かったのは**読み手に出す口**だけである。

口が無いあいだ、読み手がやることは 3 手に増える ―― 出典セルの文字列から
パスを組み立て直し、シート 1 枚を丸ごと開き、目でアンカーを探す。**20 行が
欲しいときに 1 シート全部が文脈に入る。** 設計書を索引として使う道は通って
いるのに、索引から引く手段だけが無い状態だった。

3 つを引き受ける。

``r001 資料/…/3.SLO#s5-t1``   出典セルの文字列**そのまま**。塊 1 つを出す
``r001 資料/…/3.SLO``         アンカーを省いた形。**その写しに何があるか**を出す
``NFR-002``                    表示 ID。正本を経由して**出典全件**を出す

表示 ID を受けるのは畳みがあるからである。出典セルは
:data:`arp4.publish._SOURCE_LIMIT`（2 件）で切られて ``ほか 3 件`` になるので、
**セルの字だけを辿ると畳まれた出典には永久に届かない。** 正本を経由すれば
上限は関係が無い。

ここがやるのは照合だけで、意味の判断はしない（読めなかったものを読めたことに
しない・見つからないものを近いもので代えない）。
"""

from __future__ import annotations

import difflib
import re
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from arp4 import mdio
from arp4 import sequence as sequence_module
from arp4.paths import Paths, Round
from arp4.spec import Spec

#: 出典セルの区切り（→ :func:`arp4.publish._source` の ``" / ".join``）。
#: **セルを丸ごと貼れる**ようにしてある ―― 読み手が持っているのは 1 件だけを
#: 切り出した文字列ではなく、表の升目 1 つぶんである。
_JOIN = " / "

#: 畳まれた出典（``ほか 3 件``）。**数しか残っていない**ので開く先が無い。
#: 黙って捨てず、件数と次の一手（表示 ID で引く）を言う。
_FOLDED = re.compile(r"^ほか\s*(\d+)\s*件$")

#: ラウンドの連番（``r001``）。**実在しなくてもラウンド名として読む** ――
#: 読まないと ``r999 資料/…`` の ``r999`` がファイル名の一部になり、
#: 「そのラウンドがありません」ではなく「そのファイルがありません」と言うことに
#: なる。**間違いの在り処を取り違えた案内は、案内しないより悪い。**
_ROUND = re.compile(r"^r\d+$")

#: 近いファイル名の提案。**3 件まで**（並べるほど当たらなくなる）。
_SUGGEST = 3


@dataclass(frozen=True)
class Reference:
    """出典 1 件。``round`` は省かれていることがある（設計書の外で書かれた形）。"""

    file: str
    anchor: str = ""
    round: str = ""

    def __str__(self) -> str:
        """**出典セルに出ている字へ戻す**（:func:`arp4.publish._source` と同じ形）。"""
        return ((f"{self.round} " if self.round else "")
                + self.file + (f"#{self.anchor}" if self.anchor else ""))


@dataclass
class Opened:
    """開けた塊 1 つ。**切り出しても何の資料か分かる形で持つ。**

    ``source`` と ``notes`` はパース結果の**ファイルの頭**にしかない。塊だけを
    渡すとどちらも落ちるので、切り出す側が付け直す ―― 申告（``notes``）が
    落ちると「資料に無い」と「機械が読めていない」が読み手から区別できなくなる。
    """

    reference: Reference
    path: Path
    title: str
    source: str
    notes: list[str]
    anchor: mdio.Anchor
    #: 前後の塊（``--around``）。**出したものは出したと分かる形**で並べる。
    around: list[mdio.Anchor] = field(default_factory=list)


@dataclass
class Listing:
    """アンカーを指していない出典 ―― **その写しに何があるか**。"""

    reference: Reference
    path: Path
    title: str
    source: str
    notes: list[str]
    anchors: list[mdio.Anchor]


@dataclass
class Missing:
    """開けなかった。**次の一手まで出す**（:class:`arp4.finding.Finding` と同じ規律）。"""

    reference: Reference
    reason: str
    hints: list[str] = field(default_factory=list)


#: 開いた結果。
Result = Opened | Listing | Missing


# ── 出典セルの字を読む ──────────────────────────────────────────
def references(text: str, paths: Paths | None = None) -> tuple[list[Reference], int]:
    """出典セルの文字列を :class:`Reference` へ。**畳まれた件数も返す。**

    ``r001 資料/A/基本設計書.xlsx/受注テーブル#s1-t1 / r001 …#s6-t1 / ほか 3 件``

    ラウンドは空白で区切られているだけなので、**ファイル名に空白があると
    どちらとも読める**（``資料/A/基本設計書 v2.xlsx/受注`` は実在する書き方で
    ある）。先頭の語がラウンドかどうかは、``r\\d+`` の形か、**そのラウンドが
    実在するか**で決める ―― 形にも実在にも当たらなければファイル名の一部として
    扱う（勝手にラウンドだと決めて切り落とすと、開ける資料が開けなくなる）。
    """
    found: list[Reference] = []
    folded = 0
    for part in (p.strip() for p in text.split(_JOIN)):
        if not part:
            continue
        fold = _FOLDED.match(part)
        if fold:
            folded += int(fold.group(1))
            continue
        found.append(_one(part, paths))
    return found, folded


def split(piece: str) -> tuple[str, str, str]:
    """出典 1 件の字を ``(先頭の語, 写し, アンカー)`` に割る。

    **``#`` は最後のものを見る。** シート名に ``#`` は使える
    （``safe_name`` が落とすのは記号 9 種で ``#`` はそこに無い）ので、最初で切ると
    ``受注#1`` という写しの名前が ``受注`` になり、**リンクも ``arp4 show`` も
    その 1 本だけ外す。** 組み立て側（:func:`arp4.publish._source`）はアンカーを
    末尾に足しているので、末尾から割るのが逆演算である。

    先頭の語を切り出すだけで、それがラウンドかどうかはここでは決めない
    ―― 決めるのに要る材料（実在するラウンドの一覧）が呼ぶ側で違う。
    """
    body, sharp, anchor = piece.rpartition("#")
    if not sharp:                         # ``#`` が無い（rpartition は左を空にする）
        body, anchor = anchor, ""
    head, space, rest = body.partition(" ")
    if not space:
        return "", body.strip(), anchor.strip()
    return head, rest.strip(), anchor.strip()


def _one(part: str, paths: Paths | None) -> Reference:
    head, file, anchor = split(part)
    if head and not (_ROUND.match(head)
                     or (paths is not None and paths.round(head).exists())):
        # ラウンドの形でも実在するラウンドでもない ―― **写しの名前の一部**である
        # （``資料/A/基本設計書 v2.xlsx/受注`` は実在する書き方）。勝手に切り
        # 落とすと、開ける資料が開けなくなる。
        return Reference(file=f"{head} {file}".strip(), anchor=anchor)
    return Reference(file=file, anchor=anchor, round=head)


# ── 開く ────────────────────────────────────────────────────────
def open_anchor(paths: Paths, reference: Reference, around: int = 0) -> Result:
    """出典 1 件を開く。**見つからなければ、探した場所と次の一手を言う。**"""
    rounds = _rounds_with(paths, reference)
    if isinstance(rounds, Missing):
        return rounds

    round_, others = rounds[0], rounds[1:]
    path = _parsed(round_, reference.file)
    document = mdio.read(path)
    here = Reference(file=reference.file, anchor=reference.anchor, round=round_.name)

    if not reference.anchor:
        return Listing(reference=here, path=path, title=document.title,
                       source=document.source, notes=list(document.notes),
                       anchors=list(document.anchors))

    anchor = document.by_id.get(reference.anchor)
    if anchor is None:
        return Missing(
            reference=here,
            reason=f"その写しに {reference.anchor} というアンカーはありません",
            hints=[_where(path, paths.root),
                   "この写しが持つアンカー: "
                   + "・".join(a.id for a in document.anchors[:12])
                   + ("…" if len(document.anchors) > 12 else "")]
            + _other_rounds(others, reference))

    return Opened(reference=here, path=path, title=document.title,
                  source=document.source, notes=list(document.notes),
                  anchor=anchor,
                  around=_around(document.anchors, anchor, around))


def _rounds_with(paths: Paths, reference: Reference) -> list[Round] | Missing:
    """その写しを持つラウンド。**新しい順**（指定があればそれ 1 つ）。"""
    if reference.round:
        round_ = paths.round(reference.round)
        if not round_.exists():
            known = "・".join(r.name for r in paths.rounds()) or "（1 つもありません）"
            return Missing(reference, f"ラウンド {reference.round} がありません",
                           [f"あるラウンド: {known}"])
        if not _parsed(round_, reference.file).is_file():
            return _no_file(paths, round_, reference)
        return [round_]

    holders = [r for r in reversed(paths.rounds())
               if _parsed(r, reference.file).is_file()]
    if not holders:
        latest = paths.latest_round()
        if latest is None:
            return Missing(reference, "ラウンドが 1 つもありません",
                           ["arp4 parse で資料を取り込んでください"])
        return _no_file(paths, latest, reference)
    return holders


def _no_file(paths: Paths, round_: Round, reference: Reference) -> Missing:
    """写しが無い。**近い名前を出すが、代わりに開きはしない。**"""
    hints = [f"探した場所: {_where(_parsed(round_, reference.file), paths.root)}"]
    near = _near(round_, reference.file)
    if near:
        hints.append("近い写し: " + "・".join(near))
    return Missing(Reference(file=reference.file, anchor=reference.anchor,
                             round=round_.name),
                   "その写しがありません", hints)


def _near(round_: Round, file: str) -> list[str]:
    """名前の近い写し。**開く先は読み手が選ぶ**（機械が代えると出典が偽になる）。"""
    if not round_.parsed.is_dir():
        return []
    known = [p.relative_to(round_.parsed).with_suffix("").as_posix()
             for p in round_.parsed.rglob(f"*{mdio.EXT}")]
    return difflib.get_close_matches(file, known, n=_SUGGEST, cutoff=0.5)


def _other_rounds(others: list[Round], reference: Reference) -> list[str]:
    """同じ写しを持つ**別のラウンド**。アンカーはラウンドごとに違いうる。"""
    if not others:
        return []
    return ["同じ写しを持つ別のラウンド: "
            + "・".join(f"{r.name} {reference.file}" for r in others)]


def _parsed(round_: Round, file: str) -> Path:
    return round_.parsed / f"{file}{mdio.EXT}"


def _where(path: Path, root: Path | None) -> str:
    r"""プロジェクト根からの相対（外に出ていたらそのまま）。**区切りは ``/``。**

    Windows の ``\`` のまま出すと、そのパスを次の道具へ貼った人が
    エスケープの扱いで躓く ―― 出典の字（``資料/A/…``）とも見た目が揃わない。
    """
    if root is not None:
        try:
            return path.resolve().relative_to(root.resolve()).as_posix()
        except ValueError:
            pass
    return path.as_posix()


def _around(anchors: list[mdio.Anchor], anchor: mdio.Anchor,
            width: int) -> list[mdio.Anchor]:
    if width <= 0:
        return []
    index = anchors.index(anchor)
    lower = max(0, index - width)
    return anchors[lower:index] + anchors[index + 1:index + 1 + width]


# ── 表示 ID から引く ────────────────────────────────────────────
@dataclass
class Found:
    """正本のレコード 1 件と、**畳まれていない出典全件**。"""

    id: str
    display: str
    type: str
    name: str
    references: list[Reference]


def by_key(spec: Spec, key: str) -> Found | None:
    """表示 ID（``NFR-002``）か内部 ID（``itm-…``）でアイテムを引く。

    **設計書に出ている字で引ける**ことが要点である ―― 読み手が持っているのは
    表の左端の列で、内部 ID は生成物のどこにも出ていない。
    """
    for item in spec.items:
        display = _display(spec, item)
        if key in (display, str(item.get("id") or "")):
            return Found(id=str(item.get("id") or ""), display=display,
                         type=str(item.get("type") or ""),
                         name=str(item.get("name") or ""),
                         references=_sources(item))
    return None


def _display(spec: Spec, item: dict[str, Any]) -> str:
    definition = spec.metamodel.item_types.get(str(item.get("type"))) or {}
    attribute = sequence_module.display_attribute(definition)
    return str(item.get(attribute) or "") if attribute else ""


def _sources(item: dict[str, Any]) -> list[Reference]:
    source = item.get("source")
    entries = source if isinstance(source, list) else [source]
    return [Reference(file=str(e.get("file") or ""), anchor=str(e.get("anchor") or ""),
                      round=str(e.get("round") or ""))
            for e in entries if isinstance(e, dict) and e.get("file")]


# ── 出す ────────────────────────────────────────────────────────
def render(result: Result, root: Path | None = None) -> list[str]:
    """**切り出した断片が自己記述的である**ように出す。

    頭に付けるのは 3 つ ―― 出典（設計書の升目に出ている字と同じ形）・実ファイル・
    原本の位置。パース結果の中では ``<!-- source: … -->`` がファイルの頭に
    1 度だけ書かれているので、**塊だけを渡すとどの資料の話か分からなくなる。**

    パスは**プロジェクト根からの相対**で出す（``--root`` を渡していれば）――
    指摘の位置と同じ基準である（→ ``docs/lint.md``）。絶対パスにすると端末と
    エディタでは開けるが、**そのまま次の道具へ渡せない**（読み手の手元と
    書き手の手元でパスが違う）。
    """
    if isinstance(result, Missing):
        return [f"{result.reference} ― {result.reason}"] + \
               [f"  {hint}" for hint in result.hints]

    lines = [str(result.reference), f"  {_where(result.path, root)}"]
    if result.source:
        lines.append(f"  {result.source}")
    lines += [f"> {note}" for note in result.notes]

    if isinstance(result, Listing):
        lines += ["", f"塊 {len(result.anchors)} 件"]
        lines += [f"  {a.id:<8} {_heading(a)}" for a in result.anchors]
        lines += ["", "1 つ開くには: arp4 show "
                       f"'{result.reference}#{_first(result.anchors)}'"]
        return lines

    lines += ["", result.anchor.body]
    for neighbour in result.around:
        lines += ["", f"── 隣の塊 {neighbour.id} ──", neighbour.body]
    return lines


def _heading(anchor: mdio.Anchor) -> str:
    """塊の 1 行目（:func:`arp4.mdio.read` が見出しを本文の頭に置く）。"""
    head = anchor.body.splitlines()
    return head[0] if head else (anchor.at or "（空）")


def _first(anchors: list[mdio.Anchor]) -> str:
    return anchors[0].id if anchors else "s1-t1"


# ── 束を横断する ────────────────────────────────────────────────
def corpus(paths: Paths, round_name: str = "") -> list[tuple[Round, str]]:
    """いま読むべき写しの一覧 ``(ラウンド, 写し)``。**並びは決定的**。

    **同じ写しが 2 つのラウンドにあれば新しいほうだけ**を出す。ラウンドは
    「どの資料の版を扱っているか」の単位なので、資料を 3 冊だけ撮り直した
    ``r002`` があるとき、束の姿は「``r002`` の 3 冊 ＋ ``r001`` の残り」である
    ―― :func:`open_anchor` がラウンドを省いた出典に対してやっているのと同じ
    決め方を、横断する側にも揃える。

    いちばん新しいラウンドだけを見る形にしていたら、**部分的な撮り直しの直後に
    束のほとんどが消える**（3 冊しか無いラウンドが「全部」になる）。逆に全部の
    ラウンドを並べると、同じ資料の古い版が検索に混ざる ―― どちらも嘘である。
    """
    found: dict[str, Round] = {}
    for round_ in paths.rounds():
        if round_name and round_.name != round_name:
            continue
        for path in mdio.scan(round_.parsed):
            found[path.relative_to(round_.parsed).with_suffix("").as_posix()] = round_
    return [(round_, file) for file, round_ in sorted(found.items())]


@dataclass
class Entry:
    """索引の 1 行 ―― 塊 1 つ。``reference`` は**そのまま貼れる出典**である。"""

    reference: Reference
    heading: str
    at: str
    lines: int


def catalogue(paths: Paths, round_name: str = "") -> list[Entry]:
    """束が持っている塊を全部並べる ―― **索引そのもの**。

    **ファイルには書かない。** 書けば `parsed/` を人が編集した瞬間に古くなり、
    「写しに書いてあること」と「索引が言っていること」が食い違う ―― パース結果が
    グラフの値を持たない（指し先のシートを読めばよい）のと同じ理屈である。
    要るなら出力を書き出せばよく、そのとき古くなる責任は書き出した人の側にある。
    """
    entries: list[Entry] = []
    for round_, file in corpus(paths, round_name):
        document = mdio.read(_parsed(round_, file))
        for anchor in document.anchors:
            entries.append(Entry(
                reference=Reference(file=file, anchor=anchor.id, round=round_.name),
                heading=_heading(anchor), at=anchor.at,
                lines=len(anchor.body.splitlines())))
    return entries


@dataclass
class Hit:
    """当たった塊 1 つ。**当たりを塊に帰属させる**のが素の grep との違いである。"""

    reference: Reference
    heading: str
    #: 当たった行（表示するぶんだけ）。
    lines: list[str]
    #: 当たった行の総数。**畳んだぶんは数で言う。**
    total: int


def search(paths: Paths, pattern: str, round_name: str = "",
           regex: bool = False, ignore_case: bool = False,
           per_hit: int = 3) -> list[Hit]:
    """束を横断して探す。**当たりはアンカーに帰属させて返す。**

    素の ``grep`` との違いはそこ 1 点である ―― ファイルと行番号で返されると、
    読み手は当たった行が**どの塊のものか**を自分で数え直すことになり、
    そのままでは出典として書けない。ここが返す :class:`Reference` は
    ``arp4 show`` にも整理結果の ``source`` にもそのまま貼れる。

    探す先は塊の本文だけである（申告・OCR の読み・図形の文字も本文なので入る）。
    **既定は部分一致**にしてある ―― 資料の語は ``（第3.2版）`` のように正規表現の
    記号を普通に含んでいて、そこを既定で解釈すると**探した人の意図しない当たり方**
    をする。正規表現が要るなら ``regex`` で明示する。
    """
    flags = re.IGNORECASE if ignore_case else 0
    try:
        found = re.compile(pattern if regex else re.escape(pattern), flags)
    except re.error as exc:
        raise ValueError(f"正規表現として読めません: {pattern}（{exc}）") from exc

    hits: list[Hit] = []
    for round_, file in corpus(paths, round_name):
        document = mdio.read(_parsed(round_, file))
        for anchor in document.anchors:
            lines = [line.strip() for line in anchor.body.splitlines()
                     if found.search(line)]
            if not lines:
                continue
            hits.append(Hit(
                reference=Reference(file=file, anchor=anchor.id, round=round_.name),
                heading=_heading(anchor),
                lines=lines[:per_hit], total=len(lines)))
    return hits


# ── 逆向き ―― この塊は何になったか ──────────────────────────────
@dataclass
class Use:
    """その塊を出典に挙げている正本のレコード 1 件。"""

    id: str
    display: str
    type: str
    name: str
    #: 関係なら ``起点 → 終点``（アイテムなら空）。
    ends: str = ""


def used_by(spec: Spec, reference: Reference) -> list[Use]:
    """**この塊は何になったか。** :func:`by_key` の反対側である。

    索引として往復するとき、いちばん先に知りたいのは「この塊はもう拾われて
    いるか」である ―― 分からないと、同じ塊を何度も読み直すことになる。

    照合は ``(写し, アンカー)`` だけで、**ラウンドは見ない**。同じ資料を撮り
    直したラウンドの写しを開いているとき、それを起こしたレコードが前のラウンドを
    出典にしているのは普通のことで、そこで「使われていない」と言うと嘘になる。

    **「使われていない」は「不要だった」ではない**（:mod:`arp4.origins` と同じ
    規律）―― 表紙・改訂履歴のように、出典にならないのが正しい塊もある。
    """
    found: list[Use] = []
    for record in list(spec.items) + list(spec.relations):
        if not any(r.file == reference.file and r.anchor == reference.anchor
                   for r in _sources(record)):
            continue
        ends = (f"{record.get('from')} → {record.get('to')}"
                if record.get("from") else "")
        found.append(Use(id=str(record.get("id") or ""),
                         display=_display(spec, record),
                         type=str(record.get("type") or ""),
                         name=str(record.get("name") or ""), ends=ends))
    return found


def render_listing(spec: Spec, listing: Listing) -> list[str]:
    """写しの一覧に「その塊が正本に出ているか」を添える。

    塊 1 つのときと同じことを写しの単位で言う ―― **どこがまだ拾われていないか**が
    一覧の段で分かると、開く 1 本を決めるのに塊を 1 つずつ開かずに済む。
    """
    counted = [(a.id, len(used_by(spec, Reference(
        file=listing.reference.file, anchor=a.id, round=listing.reference.round))))
        for a in listing.anchors]
    live = sum(1 for _, count in counted if count)
    lines = ["", f"塊 {len(counted)} 件のうち、正本に出ているのは {live} 件"]
    lines += [f"  {anchor:<8} {f'{count} 件' if count else '―'}"
              for anchor, count in counted]
    return lines


def render_uses(uses: list[Use]) -> list[str]:
    """**0 件でも黙らない。** 「まだ整理されていない」は言うだけの値打ちがある。"""
    if not uses:
        return ["", "この塊を出典にしている正本のレコードはありません"
                    "（まだ整理されていない、か、出典にならない塊です）"]
    lines = ["", f"この塊から起きた正本のレコード {len(uses)} 件"]
    for use in uses:
        label = " ".join(x for x in (use.display, use.type, use.name, use.ends) if x)
        lines.append(f"  {label}  （{use.id}）")
    return lines
