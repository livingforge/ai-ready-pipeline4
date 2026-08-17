"""① Word → パース結果。**見出し 1 の節 = ファイル 1 本。**

Excel はシート、PowerPoint はスライドという「資料の側が決めた 1 枚」を持って
いるが、**Word は 1 冊が 1 本の流れ**である。そのまま出すと 200 ページの設計書が
1 本の md になり、出典として指せる先が「その 1 本」しか無くなる ―― 整理層は
どの節の話かを言えず、`未読取` を宣言する先も 1 つしか持てない。

だから**資料自身が持っている構造**で割る。見出し 1 は Word の段落スタイルとして
資料に書いてあるので、これは転記であって判断ではない（どこで切るかを機械が
決めているわけではない）。**見出しが 1 つも無ければ 1 本のまま出す** ――
そのときは「割る構造が資料に無かった」という事実がそのまま形に出る。

**取るのは 3 つ、申告するのは 4 つ。**

取るもの ―― 段落（原文のまま）、表（`w:tbl`。縦結合は Excel と同じく下へ広げる）、
図形と画像（中の文字は :mod:`arp4.ocr` が読む）。

申告するもの ―― コメント（`m1`）、**未確定の変更履歴**（`d1`）、
ヘッダ・フッタ（別の 1 本）、脚注（`f1`）。どれも**本文を読んだだけでは
出てこない**もので、とくに 2 つ目は危ない ―― `w:del` を黙って落とすと
「もう消した」ように見え、黙って残すと「まだ生きている」ように見える。
どちらも資料はまだ決めていない。
"""

from __future__ import annotations

import re
import xml.etree.ElementTree as ET
import zipfile
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from arp4 import mdio, ocr, parse
from arp4.finding import Finding

_W = parse._NS["w"]
_A = parse._NS["a"]
_R = parse._NS["rel"]

#: 本文と、そこからぶら下がるもの。
_DOCUMENT = "word/document.xml"
_STYLES = "word/styles.xml"
_VBA_PART = "word/vbaProject.bin"

#: 関係の種別。
_REL_HEADER = "/header"
_REL_FOOTER = "/footer"
_REL_COMMENTS = "/comments"
_REL_FOOTNOTES = "/footnotes"
_REL_HYPERLINK = "/hyperlink"

#: 見出しのスタイル名。Word は組み込みスタイルの ``w:name`` に**英語の綴りを
#: 書く**（日本語版でも ``heading 1``）が、スタイル id のほうは資料ごとに
#: ``a3`` のような自動生成になる ―― **id だけを見ると 1 つも当たらない。**
_HEADING_NAME = re.compile(r"^heading\s*(\d)$", re.IGNORECASE)

#: 人が付け直した見出しスタイル。組み込みを使わずに作った資料が実際にある。
_HEADING_JA = re.compile(r"^見出し\s*(\d)$")

#: ヘッダ・フッタを出す 1 本の名前。**文書全体に掛かる事実**なので、節ごとに
#: 写すと同じ文字が 12 本に並ぶ（それは資料が増えたのではない）。
_MARGIN_STEM = "00_ヘッダとフッタ"

#: パース結果の ``<!-- source: … -->`` で文書と節を繋ぐ語。
_SECTION_MARK = " / 節: "


def section_source(relative: Path, title: str) -> str:
    """``資料/A/受注登録仕様書.docx / 節: 3 画面仕様``。"""
    return f"{relative.as_posix()}{_SECTION_MARK}{title}"


# ── 読み取ったもの ──────────────────────────────────────────────
@dataclass
class Block:
    """本文の 1 かたまり。**段落か表のどちらか。**"""

    kind: str                                          # ``p`` / ``t``
    text: str = ""
    rows: list[list[str]] = field(default_factory=list)
    #: 見出しの深さ（``0`` は見出しではない）。
    level: int = 0
    #: この塊に付いていたもの。
    comments: list[str] = field(default_factory=list)
    footnotes: list[str] = field(default_factory=list)
    links: list[str] = field(default_factory=list)
    deleted: list[str] = field(default_factory=list)
    inserted: int = 0


@dataclass
class Section:
    """見出し 1 で割った 1 節。**割れなければ 1 冊が 1 節になる。**"""

    order: int
    title: str
    blocks: list[Block] = field(default_factory=list)

    @property
    def stem(self) -> str:
        return f"{self.order:02d}_{parse.safe_name(self.title)}"

    @property
    def label(self) -> str:
        return f"{self.order} {self.title}"


def read(path: Path, relative: Path, use_ocr: bool = True
         ) -> tuple[list[tuple[Path, mdio.Doc]], list[Finding],
                    dict[Path, list[parse.Media]]]:
    """1 冊を読む。戻すのは :func:`arp4.parse._parse_one` と同じ 3 つ組。"""
    trouble: list[str] = []
    package = _package(path, trouble)
    if package is None:
        return [], _gap_note(path, trouble), {}

    levels = _levels(package.styles)
    blocks = _blocks(package.body, levels, package.links)
    sections = _sections(blocks)
    drawing = _drawing(package.body, package.images)
    readings = parse._readings({"": drawing}, package.bodies) if use_ocr else None

    made: list[tuple[Path, mdio.Doc]] = []
    media: dict[Path, list[parse.Media]] = {}
    for section in sections:
        doc = _document(section, relative, package)
        if section.order == 1 and (drawing.total or drawing.unreadable):
            # **図形は文書のどこにあるかを持たない。** ぶら下がっている段落は
            # 分かるが、節ごとに割ると**どの節の図か**を機械が決めることになる
            # ―― それは判断なので、1 本目にまとめて出して所在を申告する。
            _attach(doc, section, drawing, readings)
            shots, pictures, said = parse._pictures(
                section.order, section.stem, relative, drawing,
                package.bodies, readings, parse._WORD.prefix)
            if pictures is not None:
                doc.chunks.append(pictures)
            if said is not None:
                doc.chunks.append(said)
            if shots:
                media[Path(*relative.parts) / f"{section.stem}{mdio.EXT}"] = shots
        if not doc.chunks:
            continue
        made.append((Path(*relative.parts) / f"{section.stem}{mdio.EXT}", doc))

    margin = _margins(relative, package)
    if margin is not None:
        made.append(margin)

    return made, (_tracked_note(path, blocks) + _macro_note(path)
                  + _properties_note(path) + _gap_note(path, trouble)
                  + _nothing_note(path, made)), media


# ── zip から要るものを 1 度で取る ───────────────────────────────
@dataclass
class Package:
    """1 冊から取り出したもの。**zip を開くのは 1 回**（Excel と同じ理屈）。"""

    body: ET.Element
    styles: dict[str, str] = field(default_factory=dict)
    #: ``{コメント id: (誰がいつ, 本文)}``。
    comments: dict[str, tuple[str, str]] = field(default_factory=dict)
    #: ``{脚注 id: 本文}``。
    footnotes: dict[str, str] = field(default_factory=dict)
    #: ``{rId: リンクの行き先}``。
    links: dict[str, str] = field(default_factory=dict)
    #: ``{rId: 画像のパート名}``。
    images: dict[str, str] = field(default_factory=dict)
    #: ``{パート名: バイト列}``。
    bodies: dict[str, bytes] = field(default_factory=dict)
    #: ヘッダ・フッタ ``(どちら, 本文)``。
    margins: list[tuple[str, str]] = field(default_factory=list)
    #: 文書のプロパティ。
    core: list[tuple[str, str]] = field(default_factory=list)
    macros: bool = False


def _package(path: Path, trouble: list[str]) -> Package | None:
    try:
        with zipfile.ZipFile(path) as archive:
            names = set(archive.namelist())
            if _DOCUMENT not in names:
                trouble.append(f"{_DOCUMENT} がありません")
                return None
            found = Package(body=ET.fromstring(archive.read(_DOCUMENT)))
            found.core = parse._properties(archive, names)
            found.macros = _VBA_PART in names
            if _STYLES in names:
                found.styles = _styles(archive.read(_STYLES))

            rels = f"word/_rels/{Path(_DOCUMENT).name}.rels"
            targets = parse._rel_targets(archive, rels) if rels in names else {}
            types = parse._rel_types(archive, rels) if rels in names else {}
            for identity, target in targets.items():
                kind = types.get(identity, "")
                if kind.endswith(_REL_HYPERLINK):
                    # **行き先は本文に無い。** 表示文字列だけを読むと、
                    # まだ手元に無い資料があること自体が分からない。
                    # 符号化は解く（実物の Word は日本語を必ず符号化して持つ）。
                    found.links[identity] = parse.readable_link(target)
                    continue
                part = parse._resolve("word", target)
                if part not in names:
                    continue
                if kind.endswith(parse._REL_IMAGE):
                    found.images[identity] = part
                    found.bodies.setdefault(part, archive.read(part))
                elif kind.endswith(_REL_COMMENTS):
                    found.comments.update(_comments(archive.read(part)))
                elif kind.endswith(_REL_FOOTNOTES):
                    found.footnotes.update(_footnotes(archive.read(part)))
                elif kind.endswith((_REL_HEADER, _REL_FOOTER)):
                    where = "ヘッダ" if kind.endswith(_REL_HEADER) else "フッタ"
                    text = _plain(ET.fromstring(archive.read(part)))
                    if text.strip():
                        found.margins.append((where, text))
            return found
    except (OSError, zipfile.BadZipFile, KeyError, ET.ParseError) as exc:
        trouble.append(str(exc) or exc.__class__.__name__)
        return None


def _styles(body: bytes) -> dict[str, str]:
    """``{スタイル id: 名前}``。**id ではなく名前で見出しを決めるため**である。"""
    found: dict[str, str] = {}
    try:
        root = ET.fromstring(body)
    except ET.ParseError:
        return found
    for style in root.iter(f"{{{_W}}}style"):
        identity = style.get(f"{{{_W}}}styleId") or ""
        name = style.find(f"{{{_W}}}name")
        if identity and name is not None:
            found[identity] = name.get(f"{{{_W}}}val") or ""
    return found


def _levels(styles: dict[str, str]) -> dict[str, int]:
    """``{スタイル id: 見出しの深さ}``。**組み込みは英語の綴りで書いてある。**

    日本語版の Word でも ``w:name`` は ``heading 1`` である ―― 画面に
    「見出し 1」と出るのは表示名で、XML には入っていない。人が作り直した
    スタイルは日本語で書かれるので、両方を見る。
    """
    found: dict[str, int] = {}
    for identity, name in styles.items():
        matched = _HEADING_NAME.match(name.strip()) or _HEADING_JA.match(name.strip())
        if matched:
            found[identity] = int(matched.group(1))
    return found


# ── 本文を塊に割る ──────────────────────────────────────────────
def _blocks(body: ET.Element, levels: dict[str, int],
            links: dict[str, str]) -> list[Block]:
    """``w:body`` を上から順に。**段落と表を混ぜない**（出す形が違う）。"""
    found: list[Block] = []
    root = body.find(f"{{{_W}}}body")
    for element in list(root) if root is not None else []:
        if element.tag == f"{{{_W}}}p":
            found.append(_paragraph(element, levels, links))
        elif element.tag == f"{{{_W}}}tbl":
            found.append(_table(element, links))
    return found


def _paragraph(element: ET.Element, levels: dict[str, int],
               links: dict[str, str]) -> Block:
    """段落 1 つ。**変更履歴は本文から抜いて別に持つ。**"""
    block = Block(kind="p", text=_text(element), level=_level(element, levels))
    _gather(element, block, links)
    return block


def _table(element: ET.Element, links: dict[str, str]) -> Block:
    """表 1 つ。**縦結合はセルの面と同じ規律で下へ広げる。**

    ``w:vMerge`` は開始のセルに ``val="restart"``、続きのセルには値の無い
    ``w:vMerge`` が入る ―― 続きは空で保存されるので、Excel の結合セルと
    まったく同じことが起きる（分類列の 2 行目以降だけが空欄になる）。
    """
    block = Block(kind="t")
    above: list[str] = []
    for row in element.findall(f"{{{_W}}}tr"):
        line: list[str] = []
        for index, cell in enumerate(row.findall(f"{{{_W}}}tc")):
            text = "\n".join(_text(p) for p in cell.findall(f"{{{_W}}}p")).strip()
            merge = cell.find(f"{{{_W}}}tcPr/{{{_W}}}vMerge")
            if (merge is not None and (merge.get(f"{{{_W}}}val") or "continue")
                    == "continue" and not text and index < len(above)):
                text = above[index]
            line.append(text)
            _gather(cell, block, links)
        if line:
            block.rows.append(line)
            above = line
    return block


def _text(element: ET.Element) -> str:
    """段落の文字。**消された文字（``w:delText``）は入れない。**

    入れると「まだ生きている」ように見え、落として黙ると「もう消した」ように
    見える ―― どちらも資料はまだ決めていない。本文は**変更を反映した姿**にし、
    消された文字は :attr:`Block.deleted` に持って `d1` へ出す。

    箇条書きの深さは行頭の全角空白で表す（図形の段落と同じ ―― 親子が字下げに
    しか現れない書き方は日本の資料でごく普通である）。

    **テキスト枠の中（``w:txbxContent``）へは降りない。** 実物の Word は箱の
    文字を本文の段落と同じ ``w:p`` で、しかも**その箱を置いた段落の中に**
    書く ―― まとめて読むと、箱 1 つが本文の 1 行として節に紛れ込む。図形の
    文字は :func:`_drawing` が図形の一覧へ出すと決めてあり（どの節にあったかは
    機械が決めない）、両方から出すと**同じ 1 行が 2 か所に増える。**
    """
    pieces: list[str] = []
    for node in _walk(element):
        if node.tag == f"{{{_W}}}delText":
            continue
        if node.tag == f"{{{_W}}}t":
            pieces.append(node.text or "")
        elif node.tag == f"{{{_W}}}tab":
            pieces.append("\t")
        elif node.tag in (f"{{{_W}}}br", f"{{{_W}}}cr"):
            pieces.append("\n")
    body = "".join(pieces).rstrip()
    return "　" * _indent(element) + body if body.strip() else body


def _walk(element: ET.Element):
    """``element`` 以下を上から。**テキスト枠の中身は飛ばす**（→ :func:`_text`）。"""
    yield element
    for child in element:
        if child.tag == f"{{{_W}}}txbxContent":
            continue
        yield from _walk(child)


def _indent(element: ET.Element) -> int:
    """箇条書きの深さ（``w:numPr/w:ilvl``）。**無ければ 0。**"""
    level = element.find(f"{{{_W}}}pPr/{{{_W}}}numPr/{{{_W}}}ilvl")
    if level is None:
        return 0
    try:
        return int(level.get(f"{{{_W}}}val") or "0")
    except ValueError:
        return 0


def _level(element: ET.Element, levels: dict[str, int]) -> int:
    """見出しの深さ。``w:outlineLvl`` が書いてあればそちらを優先する。"""
    outline = element.find(f"{{{_W}}}pPr/{{{_W}}}outlineLvl")
    if outline is not None:
        try:
            return int(outline.get(f"{{{_W}}}val") or "0") + 1
        except ValueError:
            pass
    style = element.find(f"{{{_W}}}pPr/{{{_W}}}pStyle")
    if style is None:
        return 0
    return levels.get(style.get(f"{{{_W}}}val") or "", 0)


def _gather(element: ET.Element, block: Block, links: dict[str, str]) -> None:
    """本文には出てこないものを拾う（コメント・脚注・リンク先・変更履歴）。"""
    for node in element.iter():
        if node.tag == f"{{{_W}}}commentRangeStart":
            block.comments.append(node.get(f"{{{_W}}}id") or "")
        elif node.tag == f"{{{_W}}}footnoteReference":
            block.footnotes.append(node.get(f"{{{_W}}}id") or "")
        elif node.tag == f"{{{_W}}}hyperlink":
            where = links.get(node.get(f"{{{_R}}}id") or "")
            if where:
                block.links.append(where)
        elif node.tag == f"{{{_W}}}delText":
            if (node.text or "").strip():
                block.deleted.append(node.text or "")
        elif node.tag == f"{{{_W}}}ins":
            block.inserted += 1


def _sections(blocks: list[Block]) -> list[Section]:
    """見出し 1 で割る。**割る構造が無ければ 1 節のまま出す。**

    見出しの前にある本文（改訂履歴・適用範囲はそこに書かれる）は「（前書き）」
    という節にする ―― 捨てると**アンカーが 0 個の資料**ができ、`freeze` の
    未整理一覧に上がらないまま消える（:func:`arp4.parse._markdown` と同じ）。
    """
    found: list[Section] = []
    for block in blocks:
        if block.kind == "p" and block.level == 1 and block.text.strip():
            found.append(Section(order=len(found) + 1, title=block.text.strip()))
            continue
        if not found:
            found.append(Section(order=1, title="（前書き）"))
        found[-1].blocks.append(block)
    if len(found) == 1 and found[0].title == "（前書き）":
        # 見出し 1 が 1 つも無かった ―― 割ったのではなく、割れなかった。
        found[0].title = "（本文）"
    return [one for one in found if one.blocks]


# ── 1 節ぶんの組み立て ──────────────────────────────────────────
def _document(section: Section, relative: Path, package: Package) -> mdio.Doc:
    """節 1 つ = ファイル 1 本。"""
    doc = mdio.Doc(title=f"{relative.name} / {section.label}",
                   source=section_source(relative, section.label))
    index = section.order
    texts = tables = 0
    buffer: list[str] = []
    heading = ""

    def flush() -> None:
        nonlocal texts, buffer
        body = "\n\n".join(one for one in buffer if one.strip())
        buffer = []
        if not body.strip():
            return
        texts += 1
        doc.chunks.append(mdio.Chunk(
            anchor=f"w{index}-h{texts}",
            at=f"見出し「{heading}」" if heading else "見出しなし（節の先頭）",
            heading=heading or "（本文）", text=body))

    for block in section.blocks:
        if block.kind == "t":
            flush()
            tables += 1
            shape = (f"{len(block.rows)} 行 × "
                     f"{max((len(row) for row in block.rows), default=0)} 列")
            # **見出し 2 の直後が表なのは、実物でいちばん多い形である。**
            # 見出しを本文の塊にしか渡していなかったあいだ、`2.1 入力項目` の
            # ように**次が表の見出しは 1 文字も出てこなかった** ―― 節の中に
            # 5 列の表が 2 つ並び、どちらが入力項目でどちらが表示項目かを
            # 整理層が言えない（表の形は同じなので、中身からも決まらない）。
            doc.chunks.append(mdio.Chunk(
                anchor=f"w{index}-t{tables}",
                at=f"見出し「{heading}」の表 {shape}" if heading else f"表 {shape}",
                heading=f"{heading}｜表 {shape}" if heading else f"表 {shape}",
                rows=block.rows))
            continue
        if block.level and block.text.strip():
            flush()
            heading = block.text.strip()
            continue
        buffer.append(block.text)
    flush()

    _annotations(doc, index, section, package)
    return doc


def _annotations(doc: mdio.Doc, index: int, section: Section,
                 package: Package) -> None:
    """本文を読んだだけでは出てこないもの。**どれも別のアンカーにする。**"""
    comments = [package.comments[i] for block in section.blocks
                for i in block.comments if i in package.comments]
    if comments:
        doc.chunks.append(mdio.Chunk(
            anchor=f"w{index}-m1", at=f"コメント {len(comments)} 件",
            heading="コメント（本文には出てこない）", cells=comments))

    deleted = [text for block in section.blocks for text in block.deleted]
    inserted = sum(block.inserted for block in section.blocks)
    if deleted or inserted:
        doc.notes.append(_tracked(len(deleted), inserted, index))
    if deleted:
        # **消してあるが、まだ確定していない文字。** Excel の取り消し線
        # （`d1`）とまったく同じ扱いで、廃止かどうかは機械が決めない。
        doc.chunks.append(mdio.Chunk(
            anchor=f"w{index}-d1", at=f"変更履歴 {len(deleted)} 箇所",
            heading="変更履歴で消された文字（まだ確定していない）",
            cells=[(f"{order}", text)
                   for order, text in enumerate(deleted, start=1)]))

    notes = [(i, package.footnotes[i]) for block in section.blocks
             for i in block.footnotes if i in package.footnotes]
    if notes:
        doc.chunks.append(mdio.Chunk(
            anchor=f"w{index}-f1", at=f"脚注 {len(notes)} 件",
            heading="脚注（本文の流れには出てこない）",
            cells=[(f"注 {i}", text) for i, text in notes]))

    links = [one for block in section.blocks for one in block.links]
    if links:
        doc.chunks.append(mdio.Chunk(
            anchor=f"w{index}-l1", at=f"リンク {len(links)} 件",
            heading="リンク（本文には表示文字列しか出ていない）",
            cells=[(f"{order}", where)
                   for order, where in enumerate(links, start=1)]))


def _tracked(deleted: int, inserted: int, index: int) -> str:
    return (f"この節には**未確定の変更履歴**があります（消された箇所 {deleted}・"
            f"挿入 {inserted}）。本文は**変更を反映した姿**で出しており、"
            f"消された文字は `w{index}-d1` に出してあります ―― どちらが現行の"
            "仕様かは資料がまだ決めていないので、機械は判断していません。"
            "Word で「変更履歴」を確定してからもらい直すのが確実です。")


def _attach(doc: mdio.Doc, section: Section, drawing: parse.Drawing,
            readings: dict[str, ocr.Reading] | None) -> None:
    """図形と画像を 1 本目へ。**所在は文書全体であることを言う。**"""
    doc.notes.append(parse._shape_note(section.order, drawing, readings,
                                       parse._WORD))
    doc.notes.append(_WHOLE_NOTE)
    if drawing.alts:
        doc.chunks.append(mdio.Chunk(
            anchor=f"w{section.order}-a1",
            at=f"代替テキスト {len(drawing.alts)} 件",
            heading="代替テキスト（人が書いた説明）", cells=drawing.alts))
    labels = drawing.labels
    empty = (f"テキストの入った図形はありません（{drawing.summary}）。"
             + (f"画像の実体は `w{section.order}-i1` に出してあります"
                if drawing.media else "Word で開いて読んでください"))
    doc.chunks.append(mdio.Chunk(
        anchor=f"w{section.order}-g1", at=drawing.summary,
        heading=("図形（テキスト）" if labels else "図形（テキストなし）"),
        cells=([(f"図形{i}", text) for i, text in enumerate(labels, start=1)]
               or [("図形", empty)])))


#: 図形が**どの節にあるか**は機械が決めない。
_WHOLE_NOTE = (
    "図形と画像は**文書 1 冊ぶんをまとめてここに出しています** ―― どの節に"
    "あったかは機械が決めていません（段落への紐付けは持っていますが、節へ"
    "割り当てるのは判断になります）。位置が要るなら Word で開いて確かめてください。")


# ── 図形と画像（Word は Excel とも PowerPoint とも形が違う） ────
#: Word の図形。**接続子が無い**ので、線で描いた流れは取れない。
_WPS = "http://schemas.microsoft.com/office/word/2010/wordprocessingShape"
_PIC = "http://schemas.openxmlformats.org/drawingml/2006/picture"
_WP = "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"
#: 旧 Word のテキスト枠（VML）。**いまの資料にも普通に残っている。**
_VML = "urn:schemas-microsoft-com:vml"


def _alt_of(node: ET.Element) -> str:
    """``title`` と ``descr`` を 1 本にする（両方あることがある）。"""
    return "／".join(one for one in ((node.get("title") or "").strip(),
                                     (node.get("descr") or "").strip()) if one)


def _picture_alts(body: ET.Element) -> dict[int, str]:
    """``{画像の要素: 代替テキスト}``。**枠から降りて画像に配る。**

    Word は同じ説明を 2 か所に書く ―― 図面の枠（``wp:docPr``）と画像
    （``pic:cNvPr``）である。画像のほうが空の資料もあるので、**枠に書いて
    あるものを、その枠の中の画像へ配る。**枠に絵が 1 枚しか入っていない
    のが実物のほとんどなので、これで取り違えない。
    """
    found: dict[int, str] = {}
    for drawn in body.iter(f"{{{_W}}}drawing"):
        outer = ""
        for node in drawn.iter():
            if node.tag in (f"{{{_WP}}}docPr", f"{{{_WP}}}cNvPr"):
                outer = outer or _alt_of(node)
        for picture in drawn.iter(f"{{{_PIC}}}pic"):
            inner = ""
            for node in picture.iter(f"{{{_PIC}}}cNvPr"):
                inner = inner or _alt_of(node)
            if alt := (inner or outer):
                found[id(picture)] = alt
    return found


def _drawing(body: ET.Element, images: dict[str, str]) -> parse.Drawing:
    """文書 1 冊ぶんの図形と画像。**器は Excel と同じ**（申告も画像も使い回す）。

    取り出し方だけが違う ―― Word の図形は ``wps:wsp``、画像は ``pic:pic`` で、
    **接続子が無い**（``a:stCxn`` に当たるものを Word は持たない）。線で流れを
    描いた図は、箱の文字は取れても**繋がりは取れない**ので、そう申告する。
    """
    drawing = parse.Drawing()
    for shape in body.iter(f"{{{_WPS}}}wsp"):
        drawing.shapes += 1
        # **箱の中の文字は Word の段落で入る**（``w:txbxContent``）。DrawingML の
        # ``a:t`` だけを見ていたぶん、**実物の Word が書いたテキスト枠は空に
        # 見えていた** ―― 機能構成図を 1 つの箱で書いた資料はまるごと落ちる。
        # VML と同じ取り出し方をし、``a:t`` は残す（図形の中に貼られた図など、
        # DrawingML で文字が入る書かれ方もあるため）。
        text = "\n".join(_text(p) for p in shape.iter(f"{{{_W}}}p")).strip()
        text = text or parse._shape_text(shape)
        if text:
            drawing.labels.append(text)
    for shape in body.iter(f"{{{_VML}}}shape"):
        # **VML のテキスト枠。** 中身は ``w:txbxContent`` に Word の段落として
        # 入っているので、DrawingML の ``a:t`` では取れない。
        text = "\n".join(_text(p) for p in shape.iter(f"{{{_W}}}p")).strip()
        drawing.shapes += 1
        if text:
            drawing.labels.append(text)
    labelled = _picture_alts(body)
    for picture in body.iter(f"{{{_PIC}}}pic"):
        drawing.pictures += 1
        embedded = parse._embedded(picture)
        if embedded and (where := images.get(embedded)):
            # **代替テキストは画像 1 枚に紐付ける**（Excel と同じ形）。
            # 節の頭にまとめて並べるだけだと、`w1-a1` に 3 件あっても
            # **どの絵の説明かが決まらない** ―― 画像の一覧のほうは
            # 「代替テキストはありません」と言い続けることになる。
            drawing.media.append((where, labelled.get(id(picture), "")))
    for anchor in body.iter():
        # **代替テキストは ``wp:docPr`` にある**（Excel の ``xdr:cNvPr`` に当たる）。
        if anchor.tag not in (f"{{{_WP}}}docPr", f"{{{_WP}}}cNvPr"):
            continue
        alt = "／".join(p for p in ((anchor.get("title") or "").strip(),
                                    (anchor.get("descr") or "").strip()) if p)
        if alt:
            drawing.alts.append(("図形・画像", alt))
            drawing.picture_alts += 1
    return drawing


# ── ヘッダ・フッタ ──────────────────────────────────────────────
def _margins(relative: Path, package: Package
             ) -> tuple[Path, mdio.Doc] | None:
    """ヘッダ・フッタを**別の 1 本**に。文書番号・版・機密区分はここにしか無い。

    節ごとに写さないのは、**文書全体に掛かる事実**だからである（12 節ある
    資料で同じ 3 行が 12 回並ぶと、資料が増えたように見える）。Excel の
    印刷設定（`p1`）がシートごとなのは、あちらが本当にシートごとに掛かる
    からで、ここは掛かり方が違う。
    """
    if not package.margins:
        return None
    doc = mdio.Doc(title=f"{relative.name} / ヘッダとフッタ",
                   source=section_source(relative, "ヘッダとフッタ"))
    doc.notes.append(
        "**文書全体に掛かります**（節ごとには割っていません）。文書番号・版・"
        "機密区分はここにしか書かれていないことがあり、本文だけを読むと"
        "**その 1 冊が何の文書か**が落ちます。")
    doc.chunks.append(mdio.Chunk(
        anchor="p1", at=f"ヘッダ・フッタ {len(package.margins)} 件",
        heading="紙にしたときだけ見えるもの（ヘッダ・フッタ）",
        cells=package.margins))
    return Path(*relative.parts) / f"{_MARGIN_STEM}{mdio.EXT}", doc


# ── 付属のパート ────────────────────────────────────────────────
def _comments(body: bytes) -> dict[str, tuple[str, str]]:
    """``{id: (誰がいつ, 本文)}``。**レビュー指摘の置き場**（Excel と同じ役目）。"""
    found: dict[str, tuple[str, str]] = {}
    try:
        root = ET.fromstring(body)
    except ET.ParseError:
        return found
    for comment in root.iter(f"{{{_W}}}comment"):
        identity = comment.get(f"{{{_W}}}id") or ""
        who = (comment.get(f"{{{_W}}}author") or "（記入者不明）").strip()
        when = (comment.get(f"{{{_W}}}date") or "").strip()
        text = "\n".join(_text(p) for p in comment.iter(f"{{{_W}}}p")).strip()
        if identity and text:
            found[identity] = (f"{who}{f' {when}' if when else ''}", text)
    return found


def _footnotes(body: bytes) -> dict[str, str]:
    """``{id: 本文}``。**区切り用の疑似脚注**（``separator``）は落とす。"""
    found: dict[str, str] = {}
    try:
        root = ET.fromstring(body)
    except ET.ParseError:
        return found
    for note in root.iter(f"{{{_W}}}footnote"):
        if (note.get(f"{{{_W}}}type") or "normal") != "normal":
            continue
        identity = note.get(f"{{{_W}}}id") or ""
        text = "\n".join(_text(p) for p in note.iter(f"{{{_W}}}p")).strip()
        if identity and text:
            found[identity] = text
    return found


def _plain(root: ET.Element) -> str:
    """ヘッダ・フッタの文字。**段落を改行で繋ぐだけ。**"""
    return "\n".join(text for p in root.iter(f"{{{_W}}}p")
                     if (text := _text(p).strip()))


# ── 申告 ────────────────────────────────────────────────────────
def _tracked_note(path: Path, blocks: list[Block]) -> list[Finding]:
    """**未確定の変更履歴が入ったまま配られた資料。**

    レビュー中の版がそのまま棚卸しに回ってくるのは実案件でごく普通で、
    そこには**まだ決まっていない仕様**が「決まった仕様」と同じ見た目で
    入っている ―― 本文を読んだだけでは、どれがどちらか分からない。
    """
    deleted = sum(len(block.deleted) for block in blocks)
    inserted = sum(block.inserted for block in blocks)
    if not (deleted or inserted):
        return []
    return [Finding("warn", "P019", path.name,
                    f"変更履歴が確定していません（消された箇所 {deleted}・"
                    f"挿入 {inserted}）。本文は変更を反映した姿で出してあり、"
                    "消された文字は `d1` に出してあります ―― どちらが現行の"
                    "仕様かは資料がまだ決めていないので、機械は判断して"
                    "いません。Word で確定してからもらい直すのが確実です。")]


def _macro_note(path: Path) -> list[Finding]:
    try:
        with zipfile.ZipFile(path) as archive:
            if _VBA_PART not in set(archive.namelist()):
                return []
    except (OSError, zipfile.BadZipFile):
        return []
    return [Finding("warn", "P006", path.name,
                    "マクロ（VBA）が入っています。中身は取っていません"
                    f"（`{_VBA_PART}` は zip の中にありますが、zip としては"
                    "開けません）。仕様が要るなら Word の VBE（Alt+F11）で"
                    "開いて読むか、作成者に確認してください。")]


def _properties_note(path: Path) -> list[Finding]:
    try:
        with zipfile.ZipFile(path) as archive:
            core = parse._properties(archive, set(archive.namelist()))
    except (OSError, zipfile.BadZipFile):
        return []
    if not core:
        return []
    listed = "／".join(f"{label} {value}" for label, value in core)
    return [Finding("warn", "P005", path.name,
                    f"文書のプロパティ（{listed}）。本文にも表にも出てきません"
                    "（日時は UTC）。改訂履歴は人が書いた申告なので、そこに"
                    "無い更新がここにだけ残っていることがあります。")]


def _gap_note(path: Path, trouble: list[str]) -> list[Finding]:
    if not trouble:
        return []
    return [Finding("warn", "P008", path.name,
                    "この文書の本文を読めませんでした"
                    f"（{trouble[0]}）。パース結果は空の資料として出てきますが、"
                    "資料に無いのではありません。Word で開いて確かめ、"
                    "`.docx` として保存し直してください。")]


def _nothing_note(path: Path, made: list[tuple[Path, mdio.Doc]]) -> list[Finding]:
    if made:
        return []
    return [Finding("warn", "P009", path.name,
                    "パース結果が 1 本も出ませんでした。段落も表も図形も無い"
                    "文書ならこれで正しいのですが、中身があるはずなら"
                    "原本を開いて確かめてください。")]
