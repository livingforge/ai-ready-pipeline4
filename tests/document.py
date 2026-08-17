"""検体の ``文書:`` を **実物として開ける .docx** に組み立てる。

:mod:`deck` と同じ切り方である ―― 検体の YAML に書くのは**見出しと段落と表と
指摘だけ**で、Word の作法（`[Content_Types].xml` の申告、スタイル定義、
コメント・脚注・ヘッダのパートと関係）はここが埋める。

**Word は PowerPoint ほど厳しくない**（テーマもマスターも要らない）が、
`word/styles.xml` が無いと**見出しが見出しに見えない** ―― 組み込みスタイルの
`w:name` は日本語版でも `heading 1` なので、そこを書かずに `w:pStyle` だけ置くと
arp4 は節に割れず、Word で開いても本文と同じ字で出る。**割れなかったことに
気づけない検体**になるので、ここで必ず書く。
"""

from __future__ import annotations

import zipfile
from pathlib import Path
from typing import Any

_W = "http://schemas.openxmlformats.org/wordprocessingml/2006/main"
_R = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
_PKG = "http://schemas.openxmlformats.org/package/2006/relationships"
_CT = "http://schemas.openxmlformats.org/package/2006/content-types"
_CP = "http://schemas.openxmlformats.org/package/2006/metadata/core-properties"
_DC = "http://purl.org/dc/elements/1.1/"
_WPS = "http://schemas.microsoft.com/office/word/2010/wordprocessingShape"
_A = "http://schemas.openxmlformats.org/drawingml/2006/main"
_WP = "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"

_OFFICE = "application/vnd.openxmlformats-officedocument.wordprocessingml."

#: 見出しのスタイル。**id は自動生成の綴りにしてある** ―― 実物の日本語 Word は
#: `a3` のような id を振るので、id だけを見る実装はここで落ちる。
_HEADINGS = {"見出し": ("a3", "heading 1"), "見出し2": ("a4", "heading 2")}


def build(path: Path, spec: dict[str, Any]) -> Path:
    """検体 1 冊を ``.docx`` として書く。"""
    blocks = spec.get("文書") or []
    comments: list[tuple[str, dict[str, Any]]] = []
    notes: list[tuple[str, str]] = []
    links: list[tuple[str, str]] = []
    body: list[str] = []

    for block in blocks:
        body.append(_block(block, comments, notes, links))
    body.append("<w:sectPr>" + _margins_refs(spec) + "</w:sectPr>")

    parts: dict[str, str] = {
        "_rels/.rels": _rels([
            ("rId1", f"{_R}/officeDocument", "word/document.xml"),
            ("rId2", f"{_PKG}/metadata/core-properties", "docProps/core.xml")]),
        "docProps/core.xml": _core(spec),
        "word/document.xml": _document(body),
        "word/styles.xml": _styles(),
    }
    pairs = [("rIdS", f"{_R}/styles", "styles.xml")]
    if comments:
        parts["word/comments.xml"] = _comments(comments)
        pairs.append(("rIdC", f"{_R}/comments", "comments.xml"))
    if notes:
        parts["word/footnotes.xml"] = _footnotes(notes)
        pairs.append(("rIdF", f"{_R}/footnotes", "footnotes.xml"))
    if spec.get("ヘッダ"):
        parts["word/header1.xml"] = _margin("hdr", str(spec["ヘッダ"]))
        pairs.append(("rIdH", f"{_R}/header", "header1.xml"))
    if spec.get("フッタ"):
        parts["word/footer1.xml"] = _margin("ftr", str(spec["フッタ"]))
        pairs.append(("rIdG", f"{_R}/footer", "footer1.xml"))
    pairs += [(identity, f"{_R}/hyperlink", where) for identity, where in links]
    parts["word/_rels/document.xml.rels"] = _rels(pairs)
    parts["[Content_Types].xml"] = _content_types(parts)

    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as archive:
        for name in sorted(parts):
            archive.writestr(name, parts[name])
    return path


# ── 本文 ────────────────────────────────────────────────────────
def _block(block: dict[str, Any], comments: list[tuple[str, dict[str, Any]]],
           notes: list[tuple[str, str]], links: list[tuple[str, str]]) -> str:
    """1 かたまり。**見出し・段落・箇条書き・表・図形のどれか。**"""
    if "表" in block:
        return _table(block["表"])
    if "図形" in block:
        return _textbox(block["図形"])

    style = ""
    for key, (identity, _name) in _HEADINGS.items():
        if key in block:
            style = identity
            text = str(block[key])
            break
    else:
        text = str(block.get("段落") or block.get("箇条書き") or "")

    marks: list[str] = []
    if block.get("コメント"):
        identity = str(len(comments))
        comments.append((identity, block["コメント"]))
        marks.append(f'<w:commentRangeStart w:id="{identity}"/>')
    if block.get("脚注"):
        identity = str(len(notes) + 2)             # 0/1 は区切り用に予約
        notes.append((identity, str(block["脚注"])))
        marks.append(f'<w:r><w:footnoteReference w:id="{identity}"/></w:r>')

    runs = _runs(text)
    if block.get("削除"):
        # **変更履歴で消された文字。** 本文には `w:delText` として残っている
        # ―― 落とすと「もう消した」、残すと「まだ生きている」に見える。
        runs += (f'<w:del w:id="90" w:author="鈴木"><w:r><w:delText '
                 f'xml:space="preserve">{_esc(block["削除"])}</w:delText>'
                 "</w:r></w:del>")
    if block.get("挿入"):
        runs += (f'<w:ins w:id="91" w:author="鈴木">{_runs(str(block["挿入"]))}'
                 "</w:ins>")
    if block.get("リンク"):
        identity = f"rIdL{len(links) + 1}"
        links.append((identity, str(block["リンク"])))
        runs += (f'<w:hyperlink r:id="{identity}">'
                 f"{_runs(str(block.get('リンク文字') or '別紙参照'))}</w:hyperlink>")

    properties: list[str] = []
    if style:
        properties.append(f'<w:pStyle w:val="{style}"/>')
    if "箇条書き" in block:
        level = int(block.get("深さ") or 0)
        properties.append(f'<w:numPr><w:ilvl w:val="{level}"/>'
                          '<w:numId w:val="1"/></w:numPr>')
    prefix = f"<w:pPr>{''.join(properties)}</w:pPr>" if properties else ""
    return f"<w:p>{prefix}{''.join(marks)}{runs}</w:p>"


def _table(rows: list[list[Any]]) -> str:
    """表 1 つ。``なし``（YAML の ``~``）と書いた升は**縦結合の続き**にする。

    開始のセルには ``w:vMerge w:val="restart"`` が要る ―― 続きだけを書くと
    Word が結合として扱わず、**検体のほうが実物と違う形**になる。開始かどうかは
    「下の行の同じ位置が続きか」で決まるので、検体に書かせずここで見る
    （`図.yml` に EMU を書かせないのと同じ理屈）。
    """
    out: list[str] = []
    for index, row in enumerate(rows):
        cells: list[str] = []
        for column, value in enumerate(row):
            below = rows[index + 1] if index + 1 < len(rows) else []
            starts = column < len(below) and below[column] is None
            merge = ("<w:vMerge/>" if value is None
                     else '<w:vMerge w:val="restart"/>' if starts else "")
            text = "" if value is None else str(value)
            cells.append(f"<w:tc><w:tcPr>{merge}</w:tcPr>"
                         f"<w:p>{_runs(text)}</w:p></w:tc>")
        out.append(f"<w:tr>{''.join(cells)}</w:tr>")
    return f"<w:tbl>{''.join(out)}</w:tbl>"


def _textbox(text: str) -> str:
    """テキスト枠（`wps:wsp`）。**Word の図形は Excel とも PowerPoint とも違う。**"""
    body = "".join(f"<a:p><a:r><a:t>{_esc(line)}</a:t></a:r></a:p>"
                   for line in str(text).split("\n"))
    return (f'<w:p><w:r><w:drawing><wp:inline xmlns:wp="{_WP}">'
            '<wp:extent cx="2000000" cy="900000"/>'
            '<wp:docPr id="10" name="テキスト ボックス 1" '
            'descr="現行画面のイメージ（別紙 3 と同じもの）"/>'
            f'<a:graphic xmlns:a="{_A}"><a:graphicData uri="{_WPS}">'
            f'<wps:wsp xmlns:wps="{_WPS}"><wps:spPr/>'
            f"<wps:txbx><w:txbxContent/></wps:txbx>"
            f"<wps:bodyPr/><wps:txBody>{body}</wps:txBody></wps:wsp>"
            "</a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>")


def _runs(text: str) -> str:
    """段落の中身。改行は ``w:br`` で入れる。"""
    if not text:
        return ""
    out: list[str] = []
    for index, line in enumerate(str(text).split("\n")):
        if index:
            out.append("<w:r><w:br/></w:r>")
        out.append(f'<w:r><w:t xml:space="preserve">{_esc(line)}</w:t></w:r>')
    return "".join(out)


def _document(body: list[str]) -> str:
    return (f'<?xml version="1.0" encoding="UTF-8"?>'
            f'<w:document xmlns:w="{_W}" xmlns:r="{_R}">'
            f'<w:body>{"".join(body)}</w:body></w:document>')


def _margins_refs(spec: dict[str, Any]) -> str:
    out = ""
    if spec.get("ヘッダ"):
        out += '<w:headerReference w:type="default" r:id="rIdH"/>'
    if spec.get("フッタ"):
        out += '<w:footerReference w:type="default" r:id="rIdG"/>'
    return out


# ── 付属のパート ────────────────────────────────────────────────
def _styles() -> str:
    listed = "".join(
        f'<w:style w:type="paragraph" w:styleId="{identity}">'
        f'<w:name w:val="{name}"/></w:style>'
        for identity, name in _HEADINGS.values())
    return (f'<?xml version="1.0" encoding="UTF-8"?>'
            f'<w:styles xmlns:w="{_W}">{listed}</w:styles>')


def _comments(comments: list[tuple[str, dict[str, Any]]]) -> str:
    listed = "".join(
        f'<w:comment w:id="{identity}" w:author="{_esc(one.get("誰") or "")}" '
        f'w:date="{_esc(one.get("いつ") or "")}">'
        f'<w:p>{_runs(str(one.get("本文") or ""))}</w:p></w:comment>'
        for identity, one in comments)
    return (f'<?xml version="1.0" encoding="UTF-8"?>'
            f'<w:comments xmlns:w="{_W}">{listed}</w:comments>')


def _footnotes(notes: list[tuple[str, str]]) -> str:
    # **区切り用の疑似脚注**（`separator`）は実物にも必ず入っている ――
    # 本文を持たないので、混ぜて出すと空の脚注が 2 件並ぶ。
    listed = ('<w:footnote w:type="separator" w:id="0"><w:p/></w:footnote>'
              '<w:footnote w:type="continuationSeparator" w:id="1"><w:p/>'
              "</w:footnote>")
    listed += "".join(f'<w:footnote w:id="{identity}">'
                      f"<w:p>{_runs(text)}</w:p></w:footnote>"
                      for identity, text in notes)
    return (f'<?xml version="1.0" encoding="UTF-8"?>'
            f'<w:footnotes xmlns:w="{_W}">{listed}</w:footnotes>')


def _margin(tag: str, text: str) -> str:
    return (f'<?xml version="1.0" encoding="UTF-8"?>'
            f'<w:{tag} xmlns:w="{_W}"><w:p>{_runs(text)}</w:p></w:{tag}>')


def _core(spec: dict[str, Any]) -> str:
    props = spec.get("プロパティ") or {}
    listed = "".join(
        f"<{tag}>{_esc(props[label])}</{tag}>"
        for label, tag in (("作成者", "dc:creator"), ("文書の表題", "dc:title"),
                           ("最終更新者", "cp:lastModifiedBy"))
        if props.get(label))
    return (f'<?xml version="1.0" encoding="UTF-8"?>'
            f'<cp:coreProperties xmlns:cp="{_CP}" xmlns:dc="{_DC}">'
            f"{listed}</cp:coreProperties>")


#: パートの種別。**申告の無いパートは存在しない扱い**（`deck.py` と同じ）。
_TYPES = (("word/document.xml", "document.main+xml"),
          ("word/styles.xml", "styles+xml"),
          ("word/comments.xml", "comments+xml"),
          ("word/footnotes.xml", "footnotes+xml"),
          ("word/header", "header+xml"),
          ("word/footer", "footer+xml"))


def _content_types(parts: dict[str, str]) -> str:
    listed = ['<Default Extension="rels" ContentType="application/vnd.'
              'openxmlformats-package.relationships+xml"/>',
              '<Default Extension="xml" ContentType="application/xml"/>',
              '<Override PartName="/docProps/core.xml" ContentType="application/'
              'vnd.openxmlformats-package.core-properties+xml"/>']
    for name in sorted(parts):
        for prefix, kind in _TYPES:
            if name.startswith(prefix):
                listed.append(f'<Override PartName="/{name}" '
                              f'ContentType="{_OFFICE}{kind}"/>')
                break
    return (f'<?xml version="1.0" encoding="UTF-8"?><Types xmlns="{_CT}">'
            + "".join(listed) + "</Types>")


def _rels(pairs: list[tuple[str, str, str]]) -> str:
    body = "".join(
        f'<Relationship Id="{i}" Type="{t}" Target="{_esc(g)}"'
        + (' TargetMode="External"' if t.endswith("/hyperlink") else "")
        + "/>" for i, t, g in pairs)
    return (f'<?xml version="1.0" encoding="UTF-8"?>'
            f'<Relationships xmlns="{_PKG}">{body}</Relationships>')


def _esc(value: Any) -> str:
    return (str(value).replace("&", "&amp;").replace("<", "&lt;")
            .replace(">", "&gt;").replace('"', "&quot;"))
