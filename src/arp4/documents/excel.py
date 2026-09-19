"""Read cell facts and patch OOXML cells without re-saving unrelated Excel parts."""
from __future__ import annotations

import math
import posixpath
import re
import xml.etree.ElementTree as ET
from pathlib import Path
from xml.sax.saxutils import escape
from zipfile import ZipFile

from .contracts import DocumentError

NS = "http://schemas.openxmlformats.org/spreadsheetml/2006/main"
REL = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
Q = "{" + NS + "}"
CELL_RE = re.compile(rb'<(?P<prefix>[\w.-]+:)?c\b[^>]*(?:/>|>.*?</(?:[\w.-]+:)?c\s*>)', re.S)


def _xml(data: bytes) -> ET.Element:
    try:
        return ET.fromstring(data)
    except ET.ParseError as exc:
        raise DocumentError(f"invalid Excel XML: {exc}") from exc


def sheet_parts(book: ZipFile) -> list[tuple[str, str, str]]:
    workbook = _xml(book.read("xl/workbook.xml"))
    if workbook.tag != Q + "workbook":
        raise DocumentError("unsupported Excel namespace (transitional OOXML required)")
    rels = {r.attrib["Id"]: r for r in _xml(book.read("xl/_rels/workbook.xml.rels"))}
    found = []
    for sheet in workbook.findall(Q + "sheets/" + Q + "sheet"):
        rel = rels[sheet.attrib["{" + REL + "}id"]]
        if rel.get("TargetMode") == "External":
            raise DocumentError("external worksheet is unsupported")
        target = rel.attrib["Target"]
        part = target.lstrip("/") if target.startswith("/") else posixpath.normpath("xl/" + target)
        if not part.startswith("xl/"):
            raise DocumentError(f"invalid sheet part: {part}")
        found.append((sheet.attrib["name"], part, sheet.get("state", "visible")))
    return found


def _number(value: str):
    number = float(value)
    if not math.isfinite(number):
        raise DocumentError("non-finite Excel number")
    return int(value) if re.fullmatch(r"-?\d+", value) else number


def cell_value(cell: ET.Element, shared: list[str]):
    kind = cell.get("t", "n")
    value = cell.findtext(Q + "v")
    if kind == "inlineStr":
        return "string", "".join(t.text or "" for t in cell.iter(Q + "t"))
    if value is None or value == "":
        return "null", None
    if kind == "s":
        return "string", shared[int(value)]
    if kind == "b":
        return "boolean", value == "1"
    if kind == "n":
        return "number", _number(value)
    if kind == "e":
        return "error", value
    return "string", value


def extract(path: Path) -> list[dict]:
    from openpyxl.styles.numbers import BUILTIN_FORMATS
    sheets = []
    with ZipFile(path) as book:
        if len(book.namelist()) != len(set(book.namelist())):
            raise DocumentError("duplicate Excel ZIP members")
        shared = []
        if "xl/sharedStrings.xml" in book.namelist():
            shared = ["".join(t.text or "" for t in s.iter(Q + "t"))
                      for s in _xml(book.read("xl/sharedStrings.xml"))]
        formats = dict(BUILTIN_FORMATS)
        styles = ["General"]
        if "xl/styles.xml" in book.namelist():
            root = _xml(book.read("xl/styles.xml"))
            formats.update({int(n.attrib["numFmtId"]): n.attrib["formatCode"]
                            for n in root.findall(Q + "numFmts/" + Q + "numFmt")})
            styles = [formats.get(int(n.get("numFmtId", "0")), "General")
                      for n in root.findall(Q + "cellXfs/" + Q + "xf")]
        for index, (name, part, state) in enumerate(sheet_parts(book), 1):
            xml = _xml(book.read(part))
            cells = []
            for cell in xml.findall(".//" + Q + "sheetData/" + Q + "row/" + Q + "c"):
                kind, value = cell_value(cell, shared)
                formula = cell.find(Q + "f")
                if value is None and formula is None:
                    continue
                address = cell.attrib["r"]
                cells.append({"id": f"c-{index}-{address}", "address": address,
                              "type": "formula" if formula is not None else kind,
                              "value": value, "cached": value if formula is not None else None,
                              "formula": (formula.text or "") if formula is not None else None,
                              "number_format": styles[int(cell.get("s", "0"))]})
            sheets.append({"name": name, "part": part, "state": state,
                           "merges": [m.attrib["ref"] for m in xml.findall(Q + "mergeCells/" + Q + "mergeCell")],
                           "cells": cells})
    return sheets


def _replace_cell(match: re.Match, updates: dict, clear_cache: bool, seen: set) -> bytes:
    raw = match.group()
    opening = raw.split(b">", 1)[0]
    address_match = re.search(rb'\br\s*=\s*[\x22\x27]([^\x22\x27]+)[\x22\x27]', opening)
    if not address_match:
        return raw
    address = address_match[1].decode("ascii")
    if address not in updates:
        if clear_cache and re.search(rb'<(?:[\w.-]+:)?f\b', raw):
            return re.sub(rb'<(?:[\w.-]+:)?v\b[^>]*(?:/>|>.*?</(?:[\w.-]+:)?v\s*>)', b'', raw, flags=re.S)
        return raw
    if re.search(rb'<(?:[\w.-]+:)?f\b', raw):
        raise DocumentError(f"refusing to overwrite formula cell: {address}")
    seen.add(address)
    value = updates[address]
    prefix = match.group("prefix") or b""
    opening = re.sub(rb'\s+t\s*=\s*(?:"[^"]*"|\x27[^\x27]*\x27)', b'', opening).rstrip(b"/")
    if isinstance(value, str):
        if len(value) > 32767 or re.search(r"[\x00-\x08\x0b\x0c\x0e-\x1f]", value):
            raise DocumentError(f"unsupported Excel string at {address}")
        body = (b' t="inlineStr"><' + prefix + b'is><' + prefix + b't xml:space="preserve">'
                + escape(value).replace("\r", "&#13;").encode("utf-8") + b'</' + prefix + b't></' + prefix + b'is>')
    elif value is None:
        body = b">"
    elif type(value) is bool:
        body = b' t="b"><' + prefix + b'v>' + (b'1' if value else b'0') + b'</' + prefix + b'v>'
    else:
        if type(value) not in (int, float) or not math.isfinite(float(value)):
            raise DocumentError(f"unsupported Excel number at {address}")
        body = b' t="n"><' + prefix + b'v>' + str(value).encode("ascii") + b'</' + prefix + b'v>'
    return opening + body + b'</' + prefix + b'c>'


def patch(source: Path, destination: Path, changes: list[dict], *, force_recalc: bool = False) -> dict:
    """Only existing scalar cells; preserve all untouched ZIP member payloads."""
    with ZipFile(source) as book:
        names = book.namelist()
        if any(n.startswith("_xmlsignatures/") for n in names):
            raise DocumentError("signed Excel cannot be modified without invalidating its signature")
        parts = {name: part for name, part, _ in sheet_parts(book)}
        updates = {}
        for change in changes:
            updates.setdefault(parts[change["sheet"]], {})[change["cell"]] = change["after"]
        payloads = {info.filename: book.read(info) for info in book.infolist()}
        recalc = force_recalc or bool(changes) and any(re.search(rb'<(?:[\w.-]+:)?f\b', payloads[p]) for p in parts.values())
        changed_parts = []
        for part in parts.values():
            seen: set[str] = set()
            patched = CELL_RE.sub(lambda m: _replace_cell(m, updates.get(part, {}), recalc, seen), payloads[part])
            if seen != set(updates.get(part, {})):
                raise DocumentError(f"writeback cell missing: {part}")
            if patched != payloads[part]:
                changed_parts.append(part)
                payloads[part] = patched
        if recalc:
            raw = payloads["xl/workbook.xml"]
            prefix = re.search(rb'<([\w.-]+:)?workbook\b', raw)[1] or b""
            calc = b'<' + prefix + b'calcPr calcId="0" fullCalcOnLoad="1" forceFullCalc="1" calcMode="auto"/>'
            if re.search(rb'<(?:[\w.-]+:)?calcPr\b', raw):
                raw = re.sub(rb'<(?:[\w.-]+:)?calcPr\b[^>]*(?:/>|>.*?</(?:[\w.-]+:)?calcPr\s*>)', lambda m: calc, raw, flags=re.S)
            else:
                # calcPr precedes later workbook children in the OOXML sequence.
                later = rb'<(?:[\w.-]+:)?(?:oleSize|customWorkbookViews|pivotCaches|smartTagPr|smartTagTypes|webPublishing|fileRecoveryPr|webPublishObjects|extLst)\b|</(?:[\w.-]+:)?workbook\s*>'
                pos = re.search(later, raw).start()
                raw = raw[:pos] + calc + raw[pos:]
            payloads["xl/workbook.xml"] = raw
            changed_parts.append("xl/workbook.xml")
        with ZipFile(destination, "w") as out:
            out.comment = book.comment
            for info in book.infolist():
                out.writestr(info, payloads[info.filename])
    actual = {(s["name"], c["address"]): c for s in extract(destination) for c in s["cells"]}
    for change in changes:
        found = actual.get((change["sheet"], change["cell"]), {"value": None})["value"]
        if type(found) is not type(change["after"]) or found != change["after"]:
            # JSON numbers deliberately treat integer and decimal representations alike.
            if not (type(found) in (int, float) and type(change["after"]) in (int, float) and found == change["after"]):
                raise DocumentError(f"Excel read-back failed: {change['sheet']}!{change['cell']}")
    return {"changed_parts": changed_parts, "requires_excel_recalculation": recalc}


def drawings(path: Path) -> list[dict]:
    """Original top-level DrawingML names for explicit writeback selectors (no Excel needed)."""
    result = []
    xdr = "{http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing}"
    drawing_ns = "{http://schemas.openxmlformats.org/drawingml/2006/main}"
    with ZipFile(path) as book:
        for sheet, part, _ in sheet_parts(book):
            relpart = posixpath.dirname(part) + "/_rels/" + posixpath.basename(part) + ".rels"
            if relpart not in book.namelist():
                continue
            for rel in _xml(book.read(relpart)):
                if not rel.get("Type", "").endswith("/drawing") or rel.get("TargetMode") == "External":
                    continue
                target = rel.attrib["Target"]
                drawing_part = target.lstrip("/") if target.startswith("/") else posixpath.normpath(posixpath.dirname(part) + "/" + target)
                for anchor in _xml(book.read(drawing_part)):
                    for shape in anchor:
                        kind = shape.tag.removeprefix(xdr)
                        if kind not in ("sp", "pic", "cxnSp", "grpSp", "graphicFrame"):
                            continue
                        props = shape.find(".//" + xdr + "cNvPr")
                        if props is not None:
                            result.append({"sheet": sheet, "name": props.get("name", ""),
                                           "id": props.get("id", ""), "kind": kind,
                                           "text": "".join(n.text or "" for n in shape.iter(drawing_ns + "t"))})
    return result
