"""Strict Markdown profile, parsed as Markdown before interpreting control markers."""
from __future__ import annotations

import json
import math
import re
from dataclasses import dataclass, field
from pathlib import Path
from urllib.parse import unquote, urlsplit

from markdown_it import MarkdownIt

from .contracts import DocumentError, identifier, json_text, validate, yaml_text

MARKER = re.compile(r'<!-- arp:block id="([a-zA-Z0-9][a-zA-Z0-9_-]*)" -->\s*\Z')


@dataclass
class Page:
    path: Path
    id: str
    blocks: dict[str, int] = field(default_factory=dict)
    values: dict[tuple[str, str], object] = field(default_factory=dict)
    links: list[tuple[str, int]] = field(default_factory=list)


def split_row(line: str) -> list[str]:
    line = line.strip()
    if not line.startswith("|") or not line.endswith("|"):
        raise DocumentError("ARP tables require leading and trailing |")
    result, buffer = [], []
    index = 1
    while index < len(line) - 1:
        char = line[index]
        if char == "\\" and index + 1 < len(line) - 1 and line[index + 1] in ("\\", "|"):
            buffer.append(line[index + 1])
            index += 2
            continue
        if char == "|":
            result.append("".join(buffer).strip())
            buffer = []
        else:
            buffer.append(char)
        index += 1
    return result + ["".join(buffer).strip()]


def value_row(key: str, value) -> str:
    kind = "null" if value is None else "boolean" if type(value) is bool else "number" if type(value) in (int, float) else "string"
    raw = json.dumps(value, ensure_ascii=False, allow_nan=False)
    raw = raw.replace("\\", "\\\\").replace("|", "\\|")
    return f"| {key} | {kind} | {raw} |"


def parse(path: Path, document_id: str) -> Page:
    try:
        text = path.read_text(encoding="utf-8")
    except (OSError, UnicodeError) as exc:
        raise DocumentError(f"{path}:1: {exc}") from exc
    lines = text.splitlines()
    if not lines or lines[0] != "---" or "---" not in lines[1:]:
        raise DocumentError(f"{path}:1: YAML frontmatter required")
    end = lines.index("---", 1)
    meta = yaml_text("\n".join(lines[1:end]))
    validate("frontmatter", meta)
    if meta["document_id"] != document_id:
        raise DocumentError(f"{path}:2: document_id mismatch")
    page = Page(path, meta["page_id"])
    body = lines[end + 1:]
    tokens = MarkdownIt("commonmark").enable("table").parse("\n".join(body))
    current = None
    titles = 0
    typed_table = False
    content_counts: dict[str, int] = {}
    for token in tokens:
        line = (token.map[0] if token.map else 0) + end + 2
        def fail(message):
            raise DocumentError(f"{path}:{line}: {message}")
        if token.type == "html_block":
            marker = MARKER.fullmatch(token.content)
            if not marker:
                fail("only standalone arp:block HTML markers are allowed")
            current = identifier(marker[1])
            if current in page.blocks:
                fail(f"duplicate block ID: {current}")
            page.blocks[current] = line
            content_counts[current] = 0
            continue
        if token.type == "fence":
            closing = body[token.map[1] - 1] if token.map else ""
            if not re.fullmatch(r" {0,3}" + re.escape(token.markup[0]) + "{" + str(len(token.markup)) + r",}\s*", closing):
                fail("unclosed fenced code block")
        if token.type == "heading_open" and token.tag == "h1":
            titles += 1
            if current or titles > 1:
                fail("exactly one H1 title before blocks is required")
        if token.map and token.level == 0:
            title = token.type == "heading_open" and token.tag == "h1"
            if not current and not title:
                fail("body content must belong to an arp:block")
            if current:
                content_counts[current] += 1
        if token.type == "table_open":
            rows = [split_row(row) for row in body[token.map[0]:token.map[1]]]
            width = len(rows[0])
            if any(len(row) != width for row in rows):
                fail("table column count mismatch")
            typed_table = rows[0] == ["id", "type", "value"]
            if typed_table:
                for row_index, row in enumerate(rows[2:], 2):
                    key, kind, raw = row
                    identifier(key)
                    value = json_text(raw)
                    if type(value) is float and not math.isfinite(value):
                        fail("non-finite number in typed table")
                    actual = "null" if value is None else "boolean" if type(value) is bool else "number" if type(value) in (int, float) else "string" if type(value) is str else "invalid"
                    if kind != actual or actual == "invalid":
                        fail(f"typed table row {row_index + 1}: type/value mismatch")
                    address = (current, key)
                    if address in page.values:
                        fail(f"duplicate field ID: {key}")
                    page.values[address] = value
        if token.type == "table_close":
            typed_table = False
        for child in token.children or []:
            if typed_table:
                # These cells contain validated JSON literals, not Markdown links/HTML.
                continue
            if child.type == "html_inline":
                fail("inline HTML is not allowed (use Markdown or a code span)")
            if child.type in ("image", "link_open"):
                link = child.attrGet("src" if child.type == "image" else "href") or ""
                page.links.append((link, line))
    if titles != 1 or not page.blocks:
        raise DocumentError(f"{path}:1: one title and at least one block required")
    empty = [key for key, count in content_counts.items() if not count]
    if empty:
        raise DocumentError(f"{path}:1: empty blocks: {', '.join(empty)}")
    return page


def check_links(pages: list[Page], directory: Path) -> None:
    by_path = {p.path.resolve(): p for p in pages}
    for page in pages:
        for link, line in page.links:
            parts = urlsplit(link)
            if parts.scheme in ("http", "https", "mailto"):
                continue
            if parts.scheme or parts.netloc:
                raise DocumentError(f"{page.path}:{line}: unsupported link: {link}")
            target = (page.path.parent / unquote(parts.path)).resolve() if parts.path else page.path.resolve()
            if not target.is_relative_to(directory.resolve()) or not target.is_file():
                raise DocumentError(f"{page.path}:{line}: missing/outside document link: {link}")
            # ARP fragments are stable block IDs, not mutable heading slugs.
            if parts.fragment and (target not in by_path or unquote(parts.fragment) not in by_path[target].blocks):
                raise DocumentError(f"{page.path}:{line}: missing block fragment: {link}")
