"""検体の ``スライド:`` を **実物として開ける .pptx** に組み立てる。

:mod:`dataset` から切ってあるのは :mod:`picture` と同じ理由である ―― あちらが
「絵を手で描く」手続きを持つように、ここは「PowerPoint の作法」を持つ。検体の
YAML に書くのは**箱と線と表と文字だけ**で、スライドの寸法・テーマ・マスター・
レイアウト・EMU の座標はここが埋める。

**開けない検体は検体にならない。** `図.yml` の頭に書いてあるのと同じ失敗が
PowerPoint でも起きる ―― `[Content_Types].xml` の申告と、マスター／レイアウト／
テーマへの関係が 1 つでも欠けると、**arp4 は zip の関係だけを辿るのでパースは
通り、PowerPoint で開いたときにだけ「修復が必要」と言われる。** そのとき人は
「何が描いてあるはずだったか」を確かめられない。

**座標は書かせない。** スライドには番地が無いので、書かせるなら EMU を直に
書くことになる（`dataset.py` が Excel の番地を EMU に直しているのと同じ理由で、
それは資料の写しではなく組み立ての都合である）。箱は書いた順に並べる。
"""

from __future__ import annotations

import zipfile
from pathlib import Path
from typing import Any

#: 名前空間。
_P = "http://schemas.openxmlformats.org/presentationml/2006/main"
_A = "http://schemas.openxmlformats.org/drawingml/2006/main"
_R = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
_PKG = "http://schemas.openxmlformats.org/package/2006/relationships"
_CT = "http://schemas.openxmlformats.org/package/2006/content-types"
_CP = "http://schemas.openxmlformats.org/package/2006/metadata/core-properties"
_DC = "http://purl.org/dc/elements/1.1/"

#: スライド 1 枚の寸法（EMU）。16:9 の既定。
_WIDTH, _HEIGHT = 12192000, 6858000

#: 箱 1 個の寸法と、並べる間隔（EMU）。**書いた順に左から右、右端で折り返す。**
_BOX_W, _BOX_H = 2200000, 900000
_GAP_X, _GAP_Y = 500000, 700000
_LEFT, _TOP = 700000, 1900000
_PER_ROW = 4

#: タイトルの枠。
_TITLE = (700000, 500000, _WIDTH - 1400000, 1000000)

#: 表の枠（箱と重ならない高さから始める）。
_TABLE = (700000, 1900000, _WIDTH - 1400000)
_ROW_H = 370000

#: 線種。検体は日本語で書き、ここで DrawingML の綴りに直す。
_DASH = {"実線": "solid", "破線": "dash", "点線": "sysDot", "一点鎖線": "dashDot"}

#: 矢羽根の向き。``終点`` は線の終わり側（``a:tailEnd``）に付く。
_ARROWS = {"終点": ("none", "triangle"), "始点": ("triangle", "none"),
           "両方": ("triangle", "triangle"), "無し": ("none", "none")}


def build(path: Path, spec: dict[str, Any]) -> Path:
    """検体 1 冊を ``.pptx`` として書く。"""
    slides = spec.get("スライド") or []
    notes = {i for i, one in enumerate(slides, start=1) if one.get("ノート")}
    comments = {i for i, one in enumerate(slides, start=1) if one.get("コメント")}
    authors = _authors(slides)

    parts: dict[str, str] = {
        "_rels/.rels": _rels([
            ("rId1", f"{_R}/officeDocument", "ppt/presentation.xml"),
            ("rId2", f"{_PKG}/metadata/core-properties", "docProps/core.xml")]),
        "docProps/core.xml": _core(spec),
        "ppt/presentation.xml": _presentation(slides, bool(notes)),
        "ppt/_rels/presentation.xml.rels": _presentation_rels(slides, notes, authors),
        "ppt/slideMasters/slideMaster1.xml": _master(),
        "ppt/slideMasters/_rels/slideMaster1.xml.rels": _rels([
            ("rId1", f"{_R}/slideLayout", "../slideLayouts/slideLayout1.xml"),
            ("rId2", f"{_R}/theme", "../theme/theme1.xml")]),
        "ppt/slideLayouts/slideLayout1.xml": _layout(),
        "ppt/slideLayouts/_rels/slideLayout1.xml.rels": _rels([
            ("rId1", f"{_R}/slideMaster", "../slideMasters/slideMaster1.xml")]),
        "ppt/theme/theme1.xml": _theme(),
    }
    if authors:
        parts["ppt/commentAuthors.xml"] = _authors_part(authors)
    if notes:
        parts["ppt/notesMasters/notesMaster1.xml"] = _notes_master()
        parts["ppt/notesMasters/_rels/notesMaster1.xml.rels"] = _rels([
            ("rId1", f"{_R}/theme", "../theme/theme1.xml")])

    for order, slide in enumerate(slides, start=1):
        parts[f"ppt/slides/slide{order}.xml"] = _slide(slide)
        links = [("rId1", f"{_R}/slideLayout", "../slideLayouts/slideLayout1.xml")]
        if order in notes:
            links.append((f"rId{len(links) + 1}", f"{_R}/notesSlide",
                          f"../notesSlides/notesSlide{order}.xml"))
            parts[f"ppt/notesSlides/notesSlide{order}.xml"] = _notes(slide, order)
            parts[f"ppt/notesSlides/_rels/notesSlide{order}.xml.rels"] = _rels([
                ("rId1", f"{_R}/notesMaster", "../notesMasters/notesMaster1.xml"),
                ("rId2", f"{_R}/slide", f"../slides/slide{order}.xml")])
        if order in comments:
            links.append((f"rId{len(links) + 1}", f"{_R}/comments",
                          f"../comments/comment{order}.xml"))
            parts[f"ppt/comments/comment{order}.xml"] = _comments(slide, authors)
        parts[f"ppt/slides/_rels/slide{order}.xml.rels"] = _rels(links)

    parts["[Content_Types].xml"] = _content_types(parts)
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as archive:
        for name in sorted(parts):
            archive.writestr(name, parts[name])
    return path


# ── 包み（これが欠けると PowerPoint が「修復が必要」と言う） ─────
#: パートの種別。**`[Content_Types].xml` に申告の無いパートは存在しない扱い**
#: になる ―― zip には入っているので、arp4 のパースだけが通ってしまう。
_TYPES = (
    ("ppt/presentation.xml", "presentationml.presentation.main+xml"),
    ("ppt/slideMasters/", "presentationml.slideMaster+xml"),
    ("ppt/slideLayouts/", "presentationml.slideLayout+xml"),
    ("ppt/notesMasters/", "presentationml.notesMaster+xml"),
    ("ppt/notesSlides/", "presentationml.notesSlide+xml"),
    ("ppt/slides/", "presentationml.slide+xml"),
    ("ppt/comments/", "presentationml.comments+xml"),
    ("ppt/commentAuthors.xml", "presentationml.commentAuthors+xml"),
    ("ppt/theme/", "theme+xml"),
)

_OFFICE = "application/vnd.openxmlformats-officedocument."


def _content_types(parts: dict[str, str]) -> str:
    listed = ['<Default Extension="rels" ContentType="application/vnd.'
              'openxmlformats-package.relationships+xml"/>',
              '<Default Extension="xml" ContentType="application/xml"/>',
              '<Override PartName="/docProps/core.xml" ContentType="application/'
              'vnd.openxmlformats-package.core-properties+xml"/>']
    for name in sorted(parts):
        if name.endswith(".rels") or name.startswith("_rels"):
            continue
        for prefix, kind in _TYPES:
            if name.startswith(prefix):
                listed.append(f'<Override PartName="/{name}" '
                              f'ContentType="{_OFFICE}{kind}"/>')
                break
    return f'<?xml version="1.0" encoding="UTF-8"?><Types xmlns="{_CT}">' \
           + "".join(listed) + "</Types>"


def _rels(pairs: list[tuple[str, str, str]]) -> str:
    body = "".join(f'<Relationship Id="{i}" Type="{t}" Target="{g}"/>'
                   for i, t, g in pairs)
    return (f'<?xml version="1.0" encoding="UTF-8"?>'
            f'<Relationships xmlns="{_PKG}">{body}</Relationships>')


def _core(spec: dict[str, Any]) -> str:
    props = spec.get("プロパティ") or {}
    listed = "".join(
        f"<{tag}>{_esc(props[label])}</{tag}>"
        for label, tag in (("作成者", "dc:creator"),
                           ("文書の表題", "dc:title"),
                           ("最終更新者", "cp:lastModifiedBy"))
        if props.get(label))
    return (f'<?xml version="1.0" encoding="UTF-8"?>'
            f'<cp:coreProperties xmlns:cp="{_CP}" xmlns:dc="{_DC}">'
            f"{listed}</cp:coreProperties>")


def _presentation(slides: list[dict[str, Any]], notes: bool) -> str:
    listed = "".join(f'<p:sldId id="{255 + i}" r:id="rId{i + 1}"/>'
                     for i in range(1, len(slides) + 1))
    notes_id = f'<p:notesMasterIdLst><p:notesMasterId r:id="rId{len(slides) + 2}"/>' \
               "</p:notesMasterIdLst>" if notes else ""
    return (f'<?xml version="1.0" encoding="UTF-8"?>'
            f'<p:presentation xmlns:a="{_A}" xmlns:r="{_R}" xmlns:p="{_P}">'
            '<p:sldMasterIdLst><p:sldMasterId id="2147483648" r:id="rId1"/>'
            "</p:sldMasterIdLst>"
            f"<p:sldIdLst>{listed}</p:sldIdLst>{notes_id}"
            f'<p:sldSz cx="{_WIDTH}" cy="{_HEIGHT}"/>'
            '<p:notesSz cx="6858000" cy="9144000"/></p:presentation>')


def _presentation_rels(slides: list[dict[str, Any]], notes: set[int],
                       authors: dict[str, str]) -> str:
    pairs = [("rId1", f"{_R}/slideMaster", "slideMasters/slideMaster1.xml")]
    pairs += [(f"rId{i + 1}", f"{_R}/slide", f"slides/slide{i}.xml")
              for i in range(1, len(slides) + 1)]
    if notes:
        pairs.append((f"rId{len(slides) + 2}", f"{_R}/notesMaster",
                      "notesMasters/notesMaster1.xml"))
    pairs.append(("rIdTheme", f"{_R}/theme", "theme/theme1.xml"))
    if authors:
        pairs.append(("rIdAuthors", f"{_R}/commentAuthors", "commentAuthors.xml"))
    return _rels(pairs)


# ── スライド ────────────────────────────────────────────────────
def _slide(spec: dict[str, Any]) -> str:
    shown = "" if spec.get("非表示") is not True else ' show="0"'
    shapes: list[str] = []
    identity = 2
    if spec.get("表題"):
        shapes.append(_box(identity, "タイトル 1", spec["表題"], _TITLE,
                           placeholder="title"))
        identity += 1

    where: dict[str, int] = {}
    for order, box in enumerate(spec.get("図形") or []):
        name = box.get("名前") or f"図形 {order + 1}"
        shapes.append(_box(identity, name, box.get("文字") or "",
                           _grid(order), alt=box.get("代替")))
        where[name] = identity
        identity += 1

    for line in spec.get("接続") or []:
        shapes.append(_line(identity, line, where))
        identity += 1

    for table in spec.get("表") or []:
        shapes.append(_table(identity, table))
        identity += 1

    return (f'<?xml version="1.0" encoding="UTF-8"?>'
            f'<p:sld xmlns:a="{_A}" xmlns:r="{_R}" xmlns:p="{_P}"{shown}>'
            f"<p:cSld><p:spTree>{_tree_head()}{''.join(shapes)}</p:spTree>"
            "</p:cSld></p:sld>")


def _tree_head() -> str:
    """``spTree`` の頭。**中身が無くても要る**（無いと開けない）。"""
    return ('<p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/>'
            "</p:nvGrpSpPr><p:grpSpPr/>")


def _grid(order: int) -> tuple[int, int, int, int]:
    """箱を書いた順に並べる（左から右、右端で折り返す）。"""
    column, row = order % _PER_ROW, order // _PER_ROW
    return (_LEFT + column * (_BOX_W + _GAP_X),
            _TOP + row * (_BOX_H + _GAP_Y), _BOX_W, _BOX_H)


def _box(identity: int, name: str, text: str, at: tuple[int, int, int, int],
         placeholder: str = "", alt: str | None = None) -> str:
    """1 個の箱。``文字`` の改行は**行区切り**（``a:br``）で入れる。"""
    holder = (f'<p:nvPr><p:ph type="{placeholder}"/></p:nvPr>' if placeholder
              else "<p:nvPr/>")
    descr = f' descr="{_esc(alt)}"' if alt else ""
    return (f'<p:sp><p:nvSpPr><p:cNvPr id="{identity}" name="{_esc(name)}"{descr}/>'
            f'<p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr>{holder}</p:nvSpPr>'
            f"<p:spPr>{_frame(at)}"
            '<a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr>'
            f"<p:txBody><a:bodyPr/><a:lstStyle/>{_paragraphs(text)}</p:txBody>"
            "</p:sp>")


def _line(identity: int, spec: dict[str, Any], where: dict[str, int]) -> str:
    """接続子 1 本。**繋がっている線に座標は要らない**（端は箱が持っている）。

    ``元`` / ``先`` に書いた名前が箱に無ければ、``a:stCxn`` を持たない線
    （＝どこにも繋がっていない、目分量で置いた線）になる ―― 検体で
    「取れない線」を書きたいときは、そこに無い名前を書く。
    """
    start, end = where.get(spec.get("元", "")), where.get(spec.get("先", ""))
    head, tail = _ARROWS.get(spec.get("矢") or "終点", ("none", "triangle"))
    joins = ""
    if start is not None and end is not None:
        joins = (f'<a:stCxn id="{start}" idx="3"/><a:endCxn id="{end}" idx="1"/>')
    dash = _DASH.get(spec.get("線種") or "実線", "solid")
    return (f'<p:cxnSp><p:nvCxnSpPr>'
            f'<p:cNvPr id="{identity}" name="{_esc(spec.get("名前") or f"直線コネクタ {identity}")}"/>'
            f"<p:cNvCxnSpPr>{joins}</p:cNvCxnSpPr><p:nvPr/></p:nvCxnSpPr>"
            f"<p:spPr>{_frame((_LEFT, _TOP, _BOX_W, 0))}"
            '<a:prstGeom prst="straightConnector1"><a:avLst/></a:prstGeom>'
            f'<a:ln w="12700"><a:prstDash val="{dash}"/>'
            f'<a:headEnd type="{head}"/><a:tailEnd type="{tail}"/></a:ln>'
            "</p:spPr></p:cxnSp>")


def _table(identity: int, rows: list[list[Any]]) -> str:
    """表 1 枚（``a:tbl``）。``なし`` と書いた升は**縦結合の続き**にする。

    実物の一覧は分類列を縦に結合してあり、続きのセルは空で入っている ――
    Excel の結合セルとまったく同じことが XML でも起きる。
    """
    width = max(len(row) for row in rows)
    grid = "".join(f'<a:gridCol w="{(_TABLE[2]) // width}"/>' for _ in range(width))
    body: list[str] = []
    for row in rows:
        cells: list[str] = []
        for index in range(width):
            value = row[index] if index < len(row) else ""
            merged = ' vMerge="1"' if value is None else ""
            cells.append(f"<a:tc{merged}><a:txBody><a:bodyPr/><a:lstStyle/>"
                         f"{_paragraphs('' if value is None else str(value))}"
                         '</a:txBody><a:tcPr/></a:tc>')
        body.append(f'<a:tr h="{_ROW_H}">{"".join(cells)}</a:tr>')
    at = (_TABLE[0], _TABLE[1], _TABLE[2], _ROW_H * len(rows))
    return (f"<p:graphicFrame><p:nvGraphicFramePr>"
            f'<p:cNvPr id="{identity}" name="表 {identity}"/>'
            "<p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr>"
            f"<p:xfrm>{_offset(at)}</p:xfrm>"
            f'<a:graphic><a:graphicData uri="{_A}/table">'
            f'<a:tbl><a:tblPr/><a:tblGrid>{grid}</a:tblGrid>{"".join(body)}</a:tbl>'
            "</a:graphicData></a:graphic></p:graphicFrame>")


def _notes(spec: dict[str, Any], order: int) -> str:
    """発表者ノート。**スライド番号のプレースホルダも置く**（実物と同じ形）。"""
    body = (f'<p:sp><p:nvSpPr><p:cNvPr id="2" name="スライド番号 プレースホルダー"/>'
            '<p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr>'
            '<p:nvPr><p:ph type="sldNum"/></p:nvPr></p:nvSpPr><p:spPr/>'
            f"<p:txBody><a:bodyPr/><a:lstStyle/>{_paragraphs(str(order))}"
            "</p:txBody></p:sp>"
            '<p:sp><p:nvSpPr><p:cNvPr id="3" name="ノート プレースホルダー"/>'
            '<p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr>'
            '<p:nvPr><p:ph type="body" idx="1"/></p:nvPr></p:nvSpPr><p:spPr/>'
            f"<p:txBody><a:bodyPr/><a:lstStyle/>"
            f"{_paragraphs(str(spec['ノート']).rstrip(), split=True)}"
            "</p:txBody></p:sp>")
    return (f'<?xml version="1.0" encoding="UTF-8"?>'
            f'<p:notes xmlns:a="{_A}" xmlns:r="{_R}" xmlns:p="{_P}">'
            f"<p:cSld><p:spTree>{_tree_head()}{body}</p:spTree></p:cSld></p:notes>")


def _comments(spec: dict[str, Any], authors: dict[str, str]) -> str:
    listed: list[str] = []
    for one in spec.get("コメント") or []:
        who = str(one.get("誰") or "")
        when = f' dt="{_esc(str(one.get("いつ")))}"' if one.get("いつ") else ""
        listed.append(f'<p:cm authorId="{authors.get(who, "0")}"{when} idx="1">'
                      f'<p:pos x="100" y="100"/>'
                      f"<p:text>{_esc(str(one.get('本文') or ''))}</p:text></p:cm>")
    return (f'<?xml version="1.0" encoding="UTF-8"?>'
            f'<p:cmLst xmlns:a="{_A}" xmlns:r="{_R}" xmlns:p="{_P}">'
            f'{"".join(listed)}</p:cmLst>')


def _authors(slides: list[dict[str, Any]]) -> dict[str, str]:
    """``{表示名: authorId}``。**名簿は別のパートにしか無い**（Excel と同じ）。"""
    found: dict[str, str] = {}
    for slide in slides:
        for one in slide.get("コメント") or []:
            who = str(one.get("誰") or "")
            if who and who not in found:
                found[who] = str(len(found) + 1)
    return found


def _authors_part(authors: dict[str, str]) -> str:
    listed = "".join(
        f'<p:cmAuthor id="{identity}" name="{_esc(who)}" initials="" '
        f'lastIdx="1" clrIdx="{index}"/>'
        for index, (who, identity) in enumerate(authors.items()))
    return (f'<?xml version="1.0" encoding="UTF-8"?>'
            f'<p:cmAuthorLst xmlns:a="{_A}" xmlns:r="{_R}" xmlns:p="{_P}">'
            f"{listed}</p:cmAuthorLst>")


# ── マスター・レイアウト・テーマ（中身は空でよいが、無いと開けない） ──
def _master() -> str:
    return (f'<?xml version="1.0" encoding="UTF-8"?>'
            f'<p:sldMaster xmlns:a="{_A}" xmlns:r="{_R}" xmlns:p="{_P}">'
            f"<p:cSld><p:spTree>{_tree_head()}</p:spTree></p:cSld>"
            f"{_color_map()}"
            '<p:sldLayoutIdLst><p:sldLayoutId id="2147483649" r:id="rId1"/>'
            "</p:sldLayoutIdLst></p:sldMaster>")


def _layout() -> str:
    return (f'<?xml version="1.0" encoding="UTF-8"?>'
            f'<p:sldLayout xmlns:a="{_A}" xmlns:r="{_R}" xmlns:p="{_P}" '
            'type="titleOnly" preserve="1">'
            f"<p:cSld name=\"タイトルのみ\"><p:spTree>{_tree_head()}</p:spTree>"
            "</p:cSld></p:sldLayout>")


def _notes_master() -> str:
    return (f'<?xml version="1.0" encoding="UTF-8"?>'
            f'<p:notesMaster xmlns:a="{_A}" xmlns:r="{_R}" xmlns:p="{_P}">'
            f"<p:cSld><p:spTree>{_tree_head()}</p:spTree></p:cSld>"
            f"{_color_map()}</p:notesMaster>")


def _color_map() -> str:
    return ('<p:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" '
            'accent2="accent2" accent3="accent3" accent4="accent4" '
            'accent5="accent5" accent6="accent6" hlink="hlink" '
            'folHlink="folHlink"/>')


#: テーマの色。**12 個ちょうど要る**（順序も決まっている）。
_SCHEME = (("dk1", "000000"), ("lt1", "FFFFFF"), ("dk2", "44546A"),
           ("lt2", "E7E6E6"), ("accent1", "4472C4"), ("accent2", "ED7D31"),
           ("accent3", "A5A5A5"), ("accent4", "FFC000"), ("accent5", "5B9BD5"),
           ("accent6", "70AD47"), ("hlink", "0563C1"), ("folHlink", "954F72"))


def _theme() -> str:
    colors = "".join(f'<a:{tag}><a:srgbClr val="{value}"/></a:{tag}>'
                     for tag, value in _SCHEME)
    font = ('<a:latin typeface="Yu Gothic"/><a:ea typeface="Yu Gothic"/>'
            '<a:cs typeface=""/>')
    fill = '<a:solidFill><a:schemeClr val="phClr"/></a:solidFill>'
    line = (f'<a:ln w="6350" cap="flat" cmpd="sng" algn="ctr">{fill}'
            '<a:prstDash val="solid"/></a:ln>')
    return (f'<?xml version="1.0" encoding="UTF-8"?>'
            f'<a:theme xmlns:a="{_A}" name="arp4 検体">'
            f'<a:themeElements><a:clrScheme name="arp4">{colors}</a:clrScheme>'
            f'<a:fontScheme name="arp4"><a:majorFont>{font}</a:majorFont>'
            f"<a:minorFont>{font}</a:minorFont></a:fontScheme>"
            f'<a:fmtScheme name="arp4">'
            f"<a:fillStyleLst>{fill * 3}</a:fillStyleLst>"
            f"<a:lnStyleLst>{line * 3}</a:lnStyleLst>"
            "<a:effectStyleLst>"
            + "<a:effectStyle><a:effectLst/></a:effectStyle>" * 3
            + "</a:effectStyleLst>"
            f"<a:bgFillStyleLst>{fill * 3}</a:bgFillStyleLst>"
            "</a:fmtScheme></a:themeElements></a:theme>")


# ── 小道具 ──────────────────────────────────────────────────────
def _frame(at: tuple[int, int, int, int]) -> str:
    return f"<a:xfrm>{_offset(at)}</a:xfrm>"


def _offset(at: tuple[int, int, int, int]) -> str:
    left, top, width, height = at
    return f'<a:off x="{left}" y="{top}"/><a:ext cx="{width}" cy="{height}"/>'


def _paragraphs(text: str, split: bool = False) -> str:
    """``文字`` を段落にする。

    既定では**行区切り**（``a:br``）で 1 段落に収める ―― 機能構成図を 1 つの
    テキストボックスで書くのは日本の資料でごく普通で、そこでは親子が行頭の
    全角空白にしか現れない。``split`` は段落（``a:p``）に割る（ノート用）。
    """
    lines = text.split("\n") if text else [""]
    if split:
        return "".join(f"<a:p><a:r><a:rPr lang=\"ja-JP\"/>"
                       f"<a:t>{_esc(line)}</a:t></a:r></a:p>" for line in lines)
    runs: list[str] = []
    for index, line in enumerate(lines):
        if index:
            runs.append("<a:br/>")
        runs.append(f'<a:r><a:rPr lang="ja-JP"/><a:t>{_esc(line)}</a:t></a:r>')
    return f"<a:p>{''.join(runs)}</a:p>"


def _esc(value: Any) -> str:
    return (str(value).replace("&", "&amp;").replace("<", "&lt;")
            .replace(">", "&gt;").replace('"', "&quot;"))
