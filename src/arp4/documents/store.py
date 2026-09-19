"""Project-local document authorities, immutable evidence and reviewed proposals."""
from __future__ import annotations

import difflib
import json
import re
import shutil
import tempfile
import uuid
from dataclasses import asdict
from importlib.metadata import PackageNotFoundError, version
from pathlib import Path

from arp4 import mdio, parse as legacy_parse, paths as legacy_paths
from arp4.paths import Round

from . import excel, markdown, operations
from .instructions import AGENT_GUIDE, FORMATION_PROMPT
from .contracts import (DocumentError, digest, encoded, identifier, read, under,
                        validate, write)


def normalized(path: Path) -> bytes:
    raw = path.read_bytes()
    return raw.replace(b"\r\n", b"\n") if path.suffix in (".md", ".yml", ".json") else raw


def fingerprint(directory: Path) -> str | None:
    if not directory.exists() or not (directory / "document.yml").exists():
        return None
    entries = {}
    for path in sorted(directory.rglob("*")):
        if path.is_symlink():
            raise DocumentError(f"symlinks are not allowed in document data: {path}")
        if path.relative_to(directory).parts[0] == "original":
            continue
        if path.is_file() and path.name not in ("review.json", "formation.json", "proposal.json"):
            entries[path.relative_to(directory).as_posix()] = digest(normalized(path))
    return digest(encoded(entries))


class Store:
    def __init__(self, root: Path):
        self.root = root.resolve()
        self.arp = under(self.root, ".arp")
        self.managed = (self.arp / "config.yml").exists()
        if self.managed:
            project_config = read(self.arp / "config.yml", "project-config")
            self.config = project_config["documents"]
            legacy_paths.Paths(self.root).spec  # Validate the shared destination.
        else:
            self.config = read(self.arp / "documents.yml", "config")
        self.directory = under(self.root, self.config["directory"])
        first = self.directory.relative_to(self.root).parts[0]
        if first in (".git", ".arp", "src", "tests"):
            raise DocumentError("choose a dedicated document directory outside .git/.arp/src/tests")
        self.documents = under(self.directory, "documents")
        self.evidence = under(self.root, ".arp/evidence")
        self.proposals = under(self.root, ".arp/proposals")

    @classmethod
    def init(cls, root: Path, directory: str = "knowledge") -> "Store":
        root = root.resolve()
        destination = under(root, directory)
        if destination.relative_to(root).parts[0] in (".arp", "src", "tests", ".git"):
            raise DocumentError("choose a dedicated document directory")
        config = under(root, ".arp/config.yml")
        if (root / ".arp/documents.yml").exists() and not config.exists():
            raise DocumentError("legacy layout: run arp4 documents upgrade-layout --root <project>")
        if config.exists():
            current = read(config, "project-config")["documents"]
            if current["directory"] != directory:
                raise DocumentError("document directory is already configured; move it explicitly")
            return cls(root)
        if destination.exists() and any(destination.iterdir()):
            raise DocumentError(f"refusing to take over nonempty directory: {destination}")
        old_spec = root / ".arp/spec"
        if old_spec.exists() and (destination / "spec").exists():
            raise DocumentError("both legacy and destination spec exist; reconcile before initialization")
        destination.mkdir(parents=True, exist_ok=True)
        if old_spec.exists():
            old_spec.rename(destination / "spec")
        write(config, {"schema_version": "1", "documents": {"directory": directory},
                       "spec": {"directory": f"{directory}/spec"}}, "project-config")
        store = cls(root)
        store.documents.mkdir(parents=True, exist_ok=True)
        legacy_paths.create(root)
        (store.arp / "cache").mkdir(exist_ok=True)
        (store.directory / ".gitattributes").write_text("documents/*/original/** -text\n", encoding="utf-8")
        (store.directory / "README.md").write_text(
            "# 文書の正本\n\n通常検索: `documents/*/content/**/*.md`。本文は直接編集できます。\n"
            "編集後は `arp4 documents check --root <project>` を実行してください。\n"
            "PR/CI: 同コマンドに `--require-reviewed` を付けます。\n"
            "抽出記録・候補・生成物は `.arp/` にあり、通常検索に含めません。\n"
            "原本は documents/<文書ID>/original/、仕様データは spec/ に配置します。\n"
            "原本の所在と固定した版は各 document.yml に記録されます。\n"
            "新規記述には対応表を追加し、出典なしの場合は理由を記録します。\n",
            encoding="utf-8", newline="\n")
        (store.directory / "AGENTS.md").write_text(AGENT_GUIDE, encoding="utf-8", newline="\n")
        # A separate workspace avoids overwriting a host project's JSONC settings.
        workspace = store.arp / "documents.code-workspace"
        workspace.write_bytes(encoded({
            "folders": [{"path": ".."}],
            "settings": {"search.exclude": {"**/.arp/evidence/**": True,
                "**/.arp/proposals/**": True, "**/.arp/rounds/**": True,
                "**/.arp/out/**": True, "**/.arp/spec/**": True,
                "**/.arp/runtime/**": True, "**/.arp/cache/**": True,
                f"**/{directory}/documents/*/original/**": True}},
            "tasks": {"version": "2.0.0", "tasks": [{"label": "ARP: validate documents",
                "type": "process", "command": "arp4",
                "args": ["documents", "check", "--root", "${workspaceFolder}"],
                "problemMatcher": {"owner": "arp-documents", "fileLocation": "absolute",
                    "pattern": {"regexp": r"^(.*):(\d+): (error|warning): (.*)$",
                                "file": 1, "line": 2, "severity": 3, "message": 4}},
                "group": "test"}, {
                "label": "ARP: validate on save", "type": "process", "command": "arp4",
                "args": ["documents", "watch", "--root", "${workspaceFolder}"],
                "isBackground": True,
                "problemMatcher": {"owner": "arp-documents", "fileLocation": "absolute",
                    "background": {"activeOnStart": True, "beginsPattern": "^ARP validation started$",
                                   "endsPattern": "^ARP validation finished$"},
                    "pattern": {"regexp": r"^(.*):(\d+): (error|warning): (.*)$",
                                "file": 1, "line": 2, "severity": 3, "message": 4}}}]}}))
        ignore = store.arp / ".gitignore"
        old = ignore.read_text(encoding="utf-8") if ignore.exists() else ""
        extra = [name for name in ("out/", "runtime/", "cache/") if name not in old.splitlines()]
        if extra:
            ignore.write_text(old + "\n" + "\n".join(extra) + "\n", encoding="utf-8", newline="\n")
        # Byte-preserving originals and portable hashes even with core.autocrlf.
        attrs = store.evidence / ".gitattributes"
        attrs.parent.mkdir(parents=True, exist_ok=True)
        attrs.write_text("sources/** -text\nassets/** -text\nprompts/** -text\nlegacy/** -text\n*.json text eol=lf\n", encoding="utf-8")
        return store

    def document(self, document_id: str) -> Path:
        return under(self.documents, identifier(document_id))

    @classmethod
    def upgrade_layout(cls, root: Path) -> dict:
        """Move specification data and create reviewable document migration proposals.

        Immutable evidence and existing authorities are never rewritten. Edited content
        is carried into new proposals; formation/review must be recorded anew.
        """
        root = root.resolve()
        store = cls(root)
        legacy = store.arp / "documents.yml"
        config = store.arp / "config.yml"
        destination = store.directory / "spec"
        old_spec = store.arp / "spec"
        archive = store.arp / "layout-legacy-documents.yml"
        if legacy.exists() and archive.exists() and archive.read_bytes() != legacy.read_bytes():
            raise DocumentError("legacy config archive conflict")
        if old_spec.exists() and destination.exists():
            raise DocumentError("both legacy and destination spec exist; reconcile before migration")
        # Preflight all originals before changing the layout.
        candidates = [p for p in store.documents.iterdir() if (p / "document.yml").is_file()]
        adopted = {p.name for p in candidates}
        candidates += [p for p in store.proposals.glob("*")
                       if (p / "proposal.json").is_file()
                       and read(p / "document.yml", "document")["document_id"] not in adopted]
        pending = []
        for directory in candidates:
            store.inspect(directory)
            meta = read(directory / "document.yml", "document")
            source = under(root, meta["source"]["path"])
            managed = under(store.document(meta["document_id"]), f"original/{source.name}")
            if source == managed:
                continue
            if not source.is_file() or digest(source.read_bytes()) != meta["source"]["sha256"]:
                raise DocumentError(f"source changed/missing; reconcile before layout migration: {source}")
            if managed.exists() and managed.read_bytes() != source.read_bytes():
                raise DocumentError(f"managed original conflict: {managed}")
            pending.append((directory, meta, source))
        if old_spec.exists():
            old_spec.rename(destination)
        if not config.exists():
            try:
                write(config, {"schema_version": "1", "documents": {"directory": store.config["directory"]},
                      "spec": {"directory": destination.relative_to(root).as_posix()}}, "project-config")
            except Exception:
                if destination.exists() and not old_spec.exists():
                    destination.rename(old_spec)
                raise
        if legacy.exists():
            archive.write_bytes(legacy.read_bytes())
            legacy.unlink()
        store = cls(root)
        legacy_paths.create(root)
        (store.arp / "cache").mkdir(exist_ok=True)
        attributes = store.directory / ".gitattributes"
        rules = attributes.read_text(encoding="utf-8") if attributes.exists() else ""
        if "documents/*/original/** -text" not in rules.splitlines():
            attributes.write_text(rules + "\ndocuments/*/original/** -text\n", encoding="utf-8")
        ignore = store.arp / ".gitignore"
        text = ignore.read_text(encoding="utf-8") if ignore.exists() else ""
        ignore.write_text(text + "".join(f"\n{name}\n" for name in ("cache/", "runtime/", "out/")
                                        if name not in text.splitlines()), encoding="utf-8")
        workspace = store.arp / "documents.code-workspace"
        if workspace.exists():
            data = read(workspace)
            data.setdefault("settings", {}).setdefault("search.exclude", {})["**/.arp/cache/**"] = True
            data["settings"]["search.exclude"][f"**/{store.config['directory']}/documents/*/original/**"] = True
            workspace.write_bytes(encoded(data))
        journal_path = store.arp / "layout-migration.json"
        journal = read(journal_path) if journal_path.exists() else {"proposals": {}}
        for directory, meta, source in pending:
            identity = directory.relative_to(root).as_posix() + ":" + fingerprint(directory)
            if identity in journal["proposals"]:
                continue
            proposal = store.import_source(source, meta["document_id"])
            # Parsing is unchanged; only the source location differs. Preserve edits.
            for generated in (proposal / "content").rglob("*.md"):
                generated.unlink()
            for name in ("content", "assets"):
                if (directory / name).exists():
                    shutil.copytree(directory / name, proposal / name, dirs_exist_ok=True)
            shutil.copy2(directory / "mappings.yml", proposal / "mappings.yml")
            store.inspect(proposal)
            journal["proposals"][identity] = proposal.name
            write(journal_path, journal)
        guide = store.directory / "README.md"
        text = guide.read_text(encoding="utf-8") if guide.exists() else "# 文書の正本\n"
        if "<!-- arp:layout-v2 -->" not in text:
            guide.write_text(text + "\n<!-- arp:layout-v2 -->\n"
                "原本は `documents/<文書ID>/original/`、仕様データは `spec/`、配置設定は `.arp/config.yml` です。\n"
                "移行候補は `.arp/layout-migration.json` を参照し、成形記録・レビュー後に採用します。\n"
                "`.arp/evidence/` は固定した抽出・成形の証跡、`.arp/proposals/` は未採用候補と旧正本です。\n"
                "通常検索は `documents/*/content/**/*.md`。`cache/`・`out/`・`runtime/` はGit対象外です。\n",
                encoding="utf-8")
        return {"config": str(config), "spec": str(destination),
                "proposals": list(journal["proposals"].values())}

    def proposal(self, name: str) -> Path:
        return under(self.proposals, identifier(name))

    def extraction(self, key: str) -> dict:
        if len(key) != 64 or any(c not in "0123456789abcdef" for c in key):
            raise DocumentError("invalid extraction hash")
        path = under(self.evidence, f"extractions/{key}.json")
        result = read(path, "extraction")
        if digest(encoded(result)) != key:
            raise DocumentError(f"extraction evidence changed: {path}")
        # Cross-field uniqueness cannot be expressed by JSON Schema alone.
        pages, targets = set(), set()
        for page in result["pages"]:
            if page["id"] in pages:
                raise DocumentError("duplicate extraction page ID")
            pages.add(page["id"])
            blocks = [c["id"] for c in page["chunks"]]
            if len(blocks) != len(set(blocks)):
                raise DocumentError("duplicate extraction block ID")
        for sheet in result["sheets"]:
            for cell in sheet["cells"]:
                target = (sheet["name"], cell["address"])
                if target in targets:
                    raise DocumentError("duplicate extraction cell")
                targets.add(target)
        return result

    def import_source(self, source: Path, document_id: str) -> Path:
        document_id = identifier(document_id)
        source = source.resolve()
        if source.suffix.lower() not in legacy_parse.SUPPORTED:
            raise DocumentError(f"unsupported source: {source.suffix}")
        original = under(self.document(document_id), "original")
        if source.is_relative_to(self.arp) or (source.is_relative_to(self.documents) and not source.is_relative_to(original)):
            raise DocumentError("import the external authoring source, not ARP evidence/content")
        if not source.is_relative_to(self.root):
            raise DocumentError("place the source inside the project so Git can track it")
        raw = source.read_bytes()
        source_hash = digest(raw)
        snapshot = under(self.evidence, f"sources/{source_hash}{source.suffix.lower()}")
        snapshot.parent.mkdir(parents=True, exist_ok=True)
        if snapshot.exists() and snapshot.read_bytes() != raw:
            raise DocumentError("source snapshot integrity failure")
        snapshot.write_bytes(raw)
        managed_source = under(original, source.name) if self.managed else source
        if managed_source != source and managed_source.exists() and managed_source.read_bytes() != raw:
            raise DocumentError("managed original already exists with different content; update original/ and import that file")
        source_info = {"path": managed_source.relative_to(self.root).as_posix(),
                       "sha256": source_hash, "snapshot": snapshot.relative_to(self.root).as_posix()}
        pages, assets = [], {}
        # The legacy parser supplies Office diagrams, text and explicit unread reports.
        with tempfile.TemporaryDirectory(prefix="arp-extract-") as temp:
            targets, findings = legacy_parse.plan(Round(Path(temp), "r001"), [source], self.root, use_ocr=False)
        if any(f.level == "error" for f in findings):
            raise DocumentError("\n".join(f.render() for f in findings))
        for i, target in enumerate(targets, 1):
            page = {"id": f"page-{i}", "title": target.doc.title, "source": target.doc.source,
                    "notes": target.doc.notes, "chunks": []}
            for chunk in target.doc.chunks:
                data = asdict(chunk)
                data["id"] = data.pop("anchor")
                data["cells"] = [list(cell) for cell in data["cells"]]
                page["chunks"].append(data)
            pages.append(page)
            for path, body in target.images:
                assets[digest(body) + path.suffix.lower()] = body
        try:
            parser_version = version("ai-ready-pipeline4")
        except PackageNotFoundError:
            parser_version = "source"
        extraction = {"schema_version": "1", "document_id": document_id,
            "source": source_info, "parser": f"arp4/{parser_version};documents/1;ocr=false",
            "pages": pages, "sheets": excel.extract(snapshot) if source.suffix.lower() in (".xlsx", ".xlsm") else [],
            "findings": [{"level": f.level, "code": f.code, "message": f.message} for f in findings],
            "assets": [{"path": name, "sha256": digest(body)} for name, body in sorted(assets.items())]}
        if source.read_bytes() != raw:
            raise DocumentError("source changed while parsing; retry import")
        validate("extraction", extraction)
        if self.managed and managed_source != source:
            managed_source.parent.mkdir(parents=True, exist_ok=True)
            managed_source.write_bytes(raw)
        key = digest(encoded(extraction))
        write(self.evidence / "extractions" / f"{key}.json", extraction, "extraction")
        proposal = self.proposal(f"{document_id}-{uuid.uuid4().hex[:12]}")
        proposal.mkdir(parents=True)
        current = self.document(document_id)
        base = fingerprint(current)
        if base is not None:
            self._snapshot(current, "authorities", base)
        write(proposal / "proposal.json", {"schema_version": "1", "document_id": document_id,
                                           "base": base}, "proposal")
        write(proposal / "document.yml", {"schema_version": "1", "document_id": document_id,
              "extraction": key, "source": source_info}, "document")
        for name, body in assets.items():
            archive = under(self.evidence, f"assets/{name}")
            archive.parent.mkdir(parents=True, exist_ok=True)
            archive.write_bytes(body)
            path = under(proposal, f"assets/{name}")
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(body)
        self._seed(proposal, extraction)
        proposal.with_suffix(".prompt.md").write_text(FORMATION_PROMPT.format(
            proposal=proposal.relative_to(self.root).as_posix(),
            extraction=f".arp/evidence/extractions/{key}.json",
            authority=current.relative_to(self.root).as_posix(),
            guide=(self.directory / "AGENTS.md").relative_to(self.root).as_posix()),
            encoding="utf-8", newline="\n")
        self.inspect(proposal)
        return proposal

    def _snapshot(self, directory: Path, group: str, key: str) -> Path:
        destination = under(self.evidence, f"{group}/{key}")
        if destination.exists():
            if fingerprint(destination) != key:
                raise DocumentError("snapshot integrity failure")
            return destination
        shutil.copytree(directory, destination,
                        ignore=shutil.ignore_patterns("review.json", "formation.json", "proposal.json"))
        return destination

    def _seed(self, directory: Path, extraction: dict) -> None:
        """Mechanical draft, never labelled as LLM output or adopted automatically."""
        entries = []
        def page_start(page_id, title):
            # JSON string quoting is also valid YAML and handles unusual IDs safely.
            return ["---", 'schema_version: "1"',
                    f'document_id: {extraction["document_id"]}', f"page_id: {page_id}", "---", "",
                    "# " + title.replace("\n", " ").replace("<", "&lt;"), ""]
        def entry(page, block, field=None, origins=None, reason="", target=None, state="pending"):
            entries.append({"page": page, "block": block, "field": field,
                            "origins": origins or [], "reason": reason,
                            "writeback": state, "target": target})
        content = directory / "content"
        content.mkdir()
        for i, page in enumerate(extraction["pages"], 1):
            page_id = page["id"]
            lines = page_start(page_id, page["title"])
            if not page["chunks"]:
                lines += ['<!-- arp:block id="empty" -->', "", "読み取り結果が空です。原本を確認してください。", ""]
                entry(page_id, "empty", reason="機械抽出が空。原本確認が必要")
            for chunk in page["chunks"]:
                block = chunk["id"]
                identifier(block)
                lines += [f'<!-- arp:block id="{block}" -->', "", f"## {block}", ""]
                # Fence the faithful source preview; Agent reshapes it into prose/tables.
                preview = json.dumps(chunk, ensure_ascii=False, indent=2)
                fence = "`" * (max([len(m[0]) for m in re.finditer(r'`+', preview)] + [2]) + 1)
                lines += [fence + "json", preview, fence, ""]
                entry(page_id, block, origins=[{"page": page_id, "block": block}])
            if page["notes"]:
                lines += ['<!-- arp:block id="extraction-notes" -->', "", "## 未読取・注意事項", ""]
                lines += ["    " + note.replace("\n", "\n    ") + "\n" for note in page["notes"]]
                entry(page_id, "extraction-notes", reason="パーサーによる未読取・注意事項", state="excluded")
            (content / f"{page_id}.md").write_text("\n".join(lines), encoding="utf-8", newline="\n")
        for i, sheet in enumerate(extraction["sheets"], 1):
            if not sheet["cells"]:
                continue
            page_id = f"sheet-{i}"
            lines = page_start(page_id, sheet["name"] + " — 書き戻し項目")
            lines += ['<!-- arp:block id="cells" -->', "", "## セルの値", "",
                      "| id | type | value |", "| --- | --- | --- |"]
            entry(page_id, "cells", reason="セルの編集表の見出し・構造", state="excluded")
            for cell in sheet["cells"]:
                lines.append(markdown.value_row(cell["id"], cell["value"]))
                writable = cell["type"] in ("string", "number", "boolean", "null")
                entry(page_id, "cells", cell["id"], reason="原本セルの直接転記" if writable else "計算キャッシュ・エラー値の転記。数式変更はformula項目で明示する",
                      target={"sheet": sheet["name"], "cell": cell["address"]},
                      state="cell" if writable else "excluded")
                if cell["type"] == "formula":
                    lines.append(markdown.value_row(cell["id"] + "-formula", "=" + cell["formula"]))
                    entry(page_id, "cells", cell["id"] + "-formula",
                          reason="数式の原文。変更するときはwritebackをformulaにする",
                          target={"sheet": sheet["name"], "cell": cell["address"]}, state="excluded")
            (content / f"{page_id}.md").write_text("\n".join(lines) + "\n", encoding="utf-8", newline="\n")
        if extraction["assets"]:
            lines = page_start("assets", "抽出した画像") + ['<!-- arp:block id="images" -->', "", "## 原本の画像", ""]
            for asset in extraction["assets"]:
                lines += [f'![原本画像](../assets/{asset["path"]})', ""]
            (content / "assets.md").write_text("\n".join(lines), encoding="utf-8", newline="\n")
            entry("assets", "images", reason="原本から抽出した画像。変更はoperationsで明示する", state="excluded")
        write(directory / "mappings.yml", {"schema_version": "2", "entries": entries, "omissions": [], "operations": []}, "mappings")

    def inspect(self, directory: Path, require_reviewed: bool = False) -> dict:
        fingerprint(directory)  # Reject symlinks before following document paths.
        meta = read(directory / "document.yml", "document")
        if directory.parent == self.documents and directory.name != meta["document_id"]:
            raise DocumentError("authority directory name must equal document_id")
        extraction = self.extraction(meta["extraction"])
        if meta["source"] != extraction["source"] or meta["document_id"] != extraction["document_id"]:
            raise DocumentError("document/extraction identity mismatch")
        snapshot = under(self.root, meta["source"]["snapshot"])
        if not snapshot.is_relative_to(self.evidence / "sources") or digest(snapshot.read_bytes()) != meta["source"]["sha256"]:
            raise DocumentError("original snapshot integrity failure")
        pages = [markdown.parse(p, meta["document_id"]) for p in sorted((directory / "content").rglob("*.md"))]
        if not pages:
            raise DocumentError(f"{directory}:1: content is empty")
        known_files = {"document.yml", "mappings.yml", "review.json", "formation.json", "proposal.json"}
        for path in directory.rglob("*"):
            if path.is_file():
                rel = path.relative_to(directory)
                if not (len(rel.parts) == 1 and rel.name in known_files or rel.parts[0] in ("assets", "original") or rel.parts[0] == "content" and path.suffix == ".md"):
                    raise DocumentError(f"{path}:1: unmanaged file in document")
        ids = [p.id for p in pages]
        if len(ids) != len(set(ids)):
            raise DocumentError("duplicate page ID")
        markdown.check_links(pages, directory)
        blocks = {(p.id, block, None) for p in pages for block in p.blocks}
        values = {(p.id, block, field): value for p in pages for (block, field), value in p.values.items()}
        expected = blocks | set(values)
        mapping_file = read(directory / "mappings.yml", "mappings")
        mappings = mapping_file["entries"]
        ops = mapping_file.get("operations", [])
        row_ops = operations.validate_rows(ops, {s["name"] for s in extraction["sheets"]})
        actual, targets = set(), set()
        origins = {(p["id"], c["id"]) for p in extraction["pages"] for c in p["chunks"]}
        cells = {(s["name"], c["address"]): c for s in extraction["sheets"] for c in s["cells"]}
        covered_origins, covered_cells = set(), set()
        deleted_cells = {pair for pair in cells if operations.transform_row(pair[0], operations.coordinate(pair[1])[1], row_ops) is None}
        covered_cells.update(deleted_cells)
        for mapping in mappings:
            key = (mapping["page"], mapping["block"], mapping["field"])
            if key in actual:
                raise DocumentError(f"duplicate mapping: {key}")
            actual.add(key)
            if not mapping["origins"] and not mapping["reason"].strip():
                raise DocumentError(f"new text needs a reason: {key}")
            for origin in mapping["origins"]:
                if (origin["page"], origin["block"]) not in origins:
                    raise DocumentError(f"missing origin: {origin}")
                covered_origins.add((origin["page"], origin["block"]))
            target = mapping["target"]
            if target and "cell" in target and (target["sheet"], target["cell"]) not in cells:
                raise DocumentError(f"missing Excel target: {target}")
            if target and "cell" in target:
                covered_cells.add((target["sheet"], target["cell"]))
            elif target:
                operations.resolve_target(target, row_ops)
            if mapping["writeback"] == "operation" and (key not in values or target is not None):
                raise DocumentError("operation mapping requires a typed field and no cell target")
            if mapping["writeback"] == "excluded" and not mapping["reason"].strip():
                raise DocumentError(f"excluded mapping needs reason: {key}")
            if mapping["writeback"] in ("cell", "formula"):
                if key not in values or not target:
                    raise DocumentError(f"writeback requires a typed field and a target: {key}")
                final_target = operations.resolve_target(target, row_ops)
                pair = (final_target["sheet"], final_target["cell"])
                if pair in targets:
                    raise DocumentError(f"duplicate writeback target: {pair}")
                targets.add(pair)
                source_cell = cells[(target["sheet"], target["cell"])] if "cell" in target else {"type": "null"}
                from openpyxl.utils.cell import range_boundaries, coordinate_to_tuple
                row, col = coordinate_to_tuple(target["cell"]) if "cell" in target else (0, 0)
                for sheet in extraction["sheets"]:
                    if sheet["name"] == target["sheet"]:
                        for merged in sheet["merges"]:
                            c1, r1, c2, r2 = range_boundaries(merged)
                            if r1 <= row <= r2 and c1 <= col <= c2 and (row, col) != (r1, c1):
                                raise DocumentError(f"merged cells can only update their top-left cell: {pair}")
                if mapping["writeback"] == "formula":
                    operations.formula(values[key])
                    continue
                if source_cell["type"] in ("formula", "error"):
                    raise DocumentError(f"formula/error cells cannot be overwritten: {pair}")
                value = values[key]
                kind = "null" if value is None else "boolean" if type(value) is bool else "number" if type(value) in (int, float) else "string"
                if kind not in ("null", source_cell["type"]) and source_cell["type"] != "null":
                    raise DocumentError(f"Excel target type mismatch: {pair}")
        if expected != actual:
            raise DocumentError(f"mapping coverage mismatch; missing={expected - actual}; orphaned={actual - expected}")
        for omission in mapping_file["omissions"]:
            if not omission["origin"] and not omission["target"] or not omission["reason"].strip():
                raise DocumentError("omission must identify source content and give a reason")
            if omission["origin"]:
                origin = (omission["origin"]["page"], omission["origin"]["block"])
                if origin not in origins or origin in covered_origins:
                    raise DocumentError(f"invalid/duplicate omission origin: {origin}")
                covered_origins.add(origin)
            if omission["target"]:
                target = (omission["target"]["sheet"], omission["target"]["cell"])
                if target not in cells or target in covered_cells:
                    raise DocumentError(f"invalid/duplicate omission target: {target}")
                covered_cells.add(target)
        if covered_origins != origins or covered_cells != set(cells):
            raise DocumentError("source coverage incomplete; record deliberately removed content in omissions with a reason")
        resolved_ops = operations.resolve_operations(ops, values, mappings, directory,
            excel.drawings(snapshot) if ops else [])
        for asset in extraction["assets"]:
            if digest(under(self.evidence, f"assets/{asset['path']}").read_bytes()) != asset["sha256"]:
                raise DocumentError("extracted asset integrity failure")
        current_hash = fingerprint(directory)
        review_path = directory / "review.json"
        reviewed = False
        if review_path.exists():
            review = read(review_path, "review")
            reviewed = review["content"] == current_hash
            formation, formation_hash = self._formation(directory)
            if review["formation"] != formation_hash or formation["document_id"] != meta["document_id"] or formation["extraction"] != meta["extraction"]:
                raise DocumentError("review/formation identity mismatch")
        if require_reviewed and not reviewed:
            raise DocumentError(f"{directory}:1: content changed or not reviewed; run documents review")
        source_path = under(self.root, meta["source"]["path"])
        source_current = source_path.is_file() and digest(source_path.read_bytes()) == meta["source"]["sha256"]
        return {"meta": meta, "extraction": extraction, "pages": pages, "mappings": mappings,
                "values": values, "cells": cells, "content": current_hash,
                "omissions": mapping_file["omissions"],
                "operations": resolved_ops, "row_operations": row_ops, "deleted_cells": sorted(deleted_cells),
                "reviewed": reviewed, "source_current": source_current,
                "pending": [m for m in mappings if m["writeback"] == "pending"]}

    def record(self, name: str, model: str, actor: str, prompt: Path) -> dict:
        proposal = self.proposal(name)
        result = self.inspect(proposal)
        prompt_bytes = prompt.read_bytes()
        prompt_hash = digest(prompt_bytes)
        prompt_path = self.evidence / "prompts" / f"{prompt_hash}.txt"
        prompt_path.parent.mkdir(parents=True, exist_ok=True)
        prompt_path.write_bytes(prompt_bytes)
        formation = {"schema_version": "1", "document_id": result["meta"]["document_id"],
                     "extraction": result["meta"]["extraction"], "content": result["content"],
                     "actor": actor, "model": model, "prompt_sha256": prompt_hash}
        validate("formation", formation)
        key = digest(encoded(formation))
        self._snapshot(proposal, "formed", result["content"])
        write(self.evidence / "formations" / f"{key}.json", formation, "formation")
        write(proposal / "formation.json", formation, "formation")
        return formation

    def _formation(self, directory: Path) -> tuple[dict, str]:
        formation = read(directory / "formation.json", "formation")
        key = digest(encoded(formation))
        original = read(self.evidence / "formations" / f"{key}.json", "formation")
        if original != formation:
            raise DocumentError("formation evidence mismatch")
        prompt = self.evidence / "prompts" / f"{formation['prompt_sha256']}.txt"
        if digest(prompt.read_bytes()) != formation["prompt_sha256"]:
            raise DocumentError("formation prompt changed")
        if fingerprint(self.evidence / "formed" / formation["content"]) != formation["content"]:
            raise DocumentError("formation baseline changed")
        return formation, key

    def review(self, document_id: str, reviewer: str) -> dict:
        if not reviewer.strip():
            raise DocumentError("reviewer required")
        directory = self.document(document_id)
        result = self.inspect(directory)
        formation, key = self._formation(directory)
        if formation["document_id"] != document_id or formation["extraction"] != result["meta"]["extraction"]:
            raise DocumentError("formation identity mismatch")
        review = {"schema_version": "1", "content": result["content"], "reviewer": reviewer,
                  "formation": key}
        write(directory / "review.json", review, "review")
        return review

    def adopt(self, name: str, reviewer: str) -> Path:
        proposal = self.proposal(name)
        plan = read(proposal / "proposal.json", "proposal")
        result = self.inspect(proposal)
        if result["meta"]["document_id"] != plan["document_id"]:
            raise DocumentError("proposal identity mismatch")
        formation, key = self._formation(proposal)
        if formation["document_id"] != plan["document_id"] or formation["extraction"] != result["meta"]["extraction"]:
            raise DocumentError("proposal/formation identity mismatch")
        if formation["content"] != result["content"]:
            raise DocumentError("proposal edited after recording; record formation again")
        current = self.document(plan["document_id"])
        if fingerprint(current) != plan["base"]:
            raise DocumentError("authority changed after import; re-import and reconcile before adoption")
        if not reviewer.strip():
            raise DocumentError("reviewer required")
        # Stage and rename; an interrupted copy never exposes half a document.
        staged = self.documents / (".adopt-" + uuid.uuid4().hex)
        def original_bytes():
            return {p.relative_to(current / "original").as_posix(): digest(p.read_bytes())
                    for p in (current / "original").rglob("*") if p.is_file()}
        original_state = original_bytes()
        shutil.copytree(proposal, staged, ignore=shutil.ignore_patterns("proposal.json", "review.json"))
        if (current / "original").exists():
            shutil.copytree(current / "original", staged / "original")
        write(staged / "review.json", {"schema_version": "1", "content": result["content"],
              "reviewer": reviewer, "formation": key}, "review")
        if fingerprint(current) != plan["base"] or fingerprint(proposal) != result["content"] or original_bytes() != original_state:
            raise DocumentError("authority/proposal changed during adoption; staged data kept for recovery")
        backup = self.proposals / ("replaced-" + uuid.uuid4().hex)
        try:
            if current.exists():
                current.rename(backup)
            staged.rename(current)
        except OSError:
            if backup.exists() and not current.exists():
                backup.rename(current)
            raise
        # Keep proposal/backup as evidence, outside normal search; never erase user work.
        return current

    def diff(self, name: str) -> dict:
        proposal = self.proposal(name)
        plan = read(proposal / "proposal.json", "proposal")
        current = self.document(plan["document_id"])
        base = self.evidence / "authorities" / plan["base"] if plan["base"] else None
        def texts(directory):
            return {p.relative_to(directory).as_posix(): p.read_text(encoding="utf-8")
                    for p in directory.rglob("*.md")} if directory and directory.exists() else {}
        def difference(left, right):
            return "".join("".join(difflib.unified_diff(left.get(k, "").splitlines(True),
                right.get(k, "").splitlines(True), fromfile="before/" + k, tofile="after/" + k))
                for k in sorted(left.keys() | right.keys()))
        return {"base_to_current": difference(texts(base), texts(current)),
                "current_to_proposal": difference(texts(current), texts(proposal)),
                "base": str(base) if base else None, "current": str(current), "proposal": str(proposal),
                "authority_changed": fingerprint(current) != plan["base"]}

    def migrate(self, source: Path, document_id: str, legacy_directory: Path) -> Path:
        """Preserve legacy edited Markdown as a proposal; never pretend it is raw extraction."""
        legacy_directory = legacy_directory.resolve()
        if not legacy_directory.is_relative_to(self.root):
            raise DocumentError("legacy directory must be inside the project")
        legacy_files = sorted(legacy_directory.rglob("*.md"))
        if not legacy_files:
            raise DocumentError("legacy directory contains no Markdown")
        proposal = self.import_source(source, document_id)
        result = self.inspect(proposal)
        by_source = {p["source"]: p for p in result["extraction"]["pages"]}
        migrated = 0
        mappings = read(proposal / "mappings.yml", "mappings")
        for path in legacy_files:
            parsed = mdio.read(path)
            page = by_source.get(parsed.source)
            if not page:
                continue
            raw = path.read_bytes()
            archived = self.evidence / "legacy" / (digest(raw) + ".md")
            archived.parent.mkdir(parents=True, exist_ok=True)
            archived.write_bytes(raw)
            # A literal fence preserves edits, diagrams and even formerly permissive syntax.
            # The Agent rewrites this candidate under the new validated profile.
            text = path.read_text(encoding="utf-8")
            fence = "`" * (max([len(m[0]) for m in re.finditer(r'`+', text)] + [2]) + 1)
            destination = proposal / "content" / f"{page['id']}.md"
            destination.write_text('---\nschema_version: "1"\n' + f'document_id: {document_id}\npage_id: {page["id"]}\n---\n\n'
                + '# 旧パース結果からの移行\n\n<!-- arp:block id="legacy-content" -->\n\n'
                + fence + 'markdown\n' + text + '\n' + fence + '\n', encoding="utf-8", newline="\n")
            mappings["entries"] = [e for e in mappings["entries"] if e["page"] != page["id"]]
            mappings["entries"].append({"page": page["id"], "block": "legacy-content", "field": None,
                "origins": [{"page": page["id"], "block": c["id"]} for c in page["chunks"]],
                "reason": f"旧編集結果を保存: {archived.relative_to(self.root).as_posix()}。現在の抽出との差分確認が必要",
                "writeback": "pending", "target": None})
            migrated += 1
        if not migrated:
            raise DocumentError(f"no matching legacy source comments; ordinary import proposal remains at {proposal}")
        write(proposal / "mappings.yml", mappings, "mappings")
        self.inspect(proposal)
        return proposal

    def prepare_spec(self, document_id: str) -> dict:
        """Validate the authority before feeding the compatible specification pipeline."""
        directory = self.document(document_id)
        result = self.inspect(directory, require_reviewed=True)
        paths = legacy_paths.create(self.root)
        round_ = paths.new_round()
        sources = [page.path for page in result["pages"]]
        targets, findings = legacy_parse.plan(round_, sources, self.root, use_ocr=False)
        if any(f.level == "error" for f in findings):
            raise DocumentError("\n".join(f.render() for f in findings))
        round_.open(reason=f"document authority {document_id} {result['content']}")
        written, findings = legacy_parse.write(targets)
        if any(f.level == "error" for f in findings):
            raise DocumentError("\n".join(f.render() for f in findings))
        legacy_parse.record(round_, targets, written)
        write(round_.dir / "authority.json", {"document_id": document_id, "content": result["content"]})
        return {"round": round_.name, "authority": result["content"], "files": len(written)}

    def export(self, document_id: str, output: Path | None = None, *, engine: str = "auto") -> dict:
        directory = self.document(document_id)
        result = self.inspect(directory, require_reviewed=True)
        meta = result["meta"]
        source = under(self.root, meta["source"]["snapshot"])
        if source.suffix not in (".xlsx", ".xlsm"):
            raise DocumentError("Excel writeback requires an Excel source")
        if not result["source_current"]:
            raise DocumentError("source changed/missing; re-import and reconcile before Excel writeback")
        changes, excluded = [], []
        for mapping in result["mappings"]:
            if mapping["writeback"] == "excluded":
                excluded.append(mapping)
            if mapping["writeback"] not in ("cell", "formula"):
                continue
            target = mapping["target"]
            final_target = operations.resolve_target(target, result["row_operations"])
            original = result["cells"].get((target["sheet"], target.get("cell")))
            old = original["value"] if original else None
            if mapping["writeback"] == "formula":
                old = "=" + original["formula"] if original and original["formula"] is not None else None
            new = result["values"][(mapping["page"], mapping["block"], mapping["field"])]
            if type(old) is not type(new) or old != new or mapping["writeback"] == "formula" and result["row_operations"]:
                change = {**final_target, "before": old, "after": new, "field": mapping["field"]}
                if mapping["writeback"] == "formula":
                    change["kind"] = "formula"
                changes.append(change)
        advanced = bool(result["operations"]) or any(c.get("kind") == "formula" for c in changes)
        if engine not in ("auto", "xml", "excel"):
            raise DocumentError("engine must be auto, xml or excel")
        if advanced and engine == "xml":
            raise DocumentError("row/shape/formula changes require the Excel engine")
        selected_engine = "excel" if engine == "excel" or advanced else "xml"
        report = {"schema_version": "1", "document_id": document_id,
                  "content": result["content"], "source_sha256": meta["source"]["sha256"],
                  "changes": changes, "unreflected": result["pending"], "excluded": excluded,
                  "omissions": result["omissions"],
                  "operations": result["operations"], "deleted_cells": result["deleted_cells"],
                  "engine": selected_engine,
                  "complete": not result["pending"], "written": False}
        if output is None:
            return report
        if result["pending"]:
            raise DocumentError("unresolved writeback mappings; inspect export preview and resolve them")
        output = output.resolve()
        allowed = under(self.root, ".arp/out")
        if not output.is_relative_to(allowed) or output.suffix.lower() != source.suffix:
            raise DocumentError(f"output must be a new {source.suffix} file inside {allowed}")
        report_path = output.with_suffix(output.suffix + ".report.json")
        if output.exists() or report_path.exists():
            raise DocumentError("output/report already exists; choose a new output filename")
        output.parent.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(prefix="arp-export-", dir=output.parent) as temp:
            staged = Path(temp) / output.name
            if selected_engine == "excel":
                from . import native_excel
                report.update(native_excel.patch(source, staged, changes, result["operations"], directory))
            else:
                report.update(excel.patch(source, staged, changes))
            if fingerprint(directory) != result["content"] or digest(under(self.root, meta["source"]["path"]).read_bytes()) != meta["source"]["sha256"]:
                raise DocumentError("authority/source changed during export; retry after review")
            report.update(written=True, output_sha256=digest(staged.read_bytes()))
            # Byte comparison of all untouched ZIP entries is inherent to excel.patch.
            staged.rename(output)
        write(report_path, report)
        return report
