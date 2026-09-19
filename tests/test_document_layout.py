"""Layout transitions preserve authorities, provenance, and specification bytes."""
from pathlib import Path

import pytest

from arp4 import paths
from arp4.documents.contracts import DocumentError, read, write
from arp4.documents.store import Store, fingerprint


def legacy_project(root):
    paths.create(root)
    write(root / ".arp/documents.yml", {"schema_version": "1", "directory": "knowledge"})
    (root / "knowledge/documents").mkdir(parents=True)
    source = root / "design.md"
    source.write_text("# Design\n\n## Order\n\nOriginal text.\n", encoding="utf-8")
    store = Store(root)
    proposal = store.import_source(source, "design")
    return store, source, proposal


def test_managed_original_and_shared_spec_paths(tmp_path):
    old = paths.create(tmp_path)
    (old.items / "preserved.yml").write_bytes(b"[]\n")
    store = Store.init(tmp_path, "project-knowledge")
    assert paths.resolve(tmp_path).spec == tmp_path / "project-knowledge/spec"
    assert (paths.resolve(tmp_path).items / "preserved.yml").read_bytes() == b"[]\n"
    assert not (tmp_path / ".arp/spec").exists()
    source = tmp_path / "input.md"
    source.write_text("# Input\n\nHello.\n", encoding="utf-8")
    proposal = store.import_source(source, "document")
    managed = store.document("document") / "original/input.md"
    assert managed.read_bytes() == source.read_bytes()
    assert read(proposal / "document.yml")["source"]["path"] == "project-knowledge/documents/document/original/input.md"
    assert fingerprint(store.document("document")) is None
    source.write_text("# Changed external copy\n", encoding="utf-8")
    assert store.inspect(proposal)["source_current"]
    with pytest.raises(DocumentError, match="different content"):
        store.import_source(source, "document")
    managed.write_text("# Updated managed original\n", encoding="utf-8")
    assert not store.inspect(proposal)["source_current"]
    incoming = store.import_source(managed, "document")
    assert store.inspect(incoming)["source_current"]
    assert "cache/" in (tmp_path / ".arp/.gitignore").read_text(encoding="utf-8")
    workspace = read(tmp_path / ".arp/documents.code-workspace")
    assert workspace["settings"]["search.exclude"]["**/project-knowledge/documents/*/original/**"]


def test_upgrade_preserves_review_and_carries_edited_content(tmp_path):
    store, source, proposal = legacy_project(tmp_path)
    prompt = tmp_path / "prompt.txt"
    prompt.write_text("Original formation", encoding="utf-8")
    store.record(proposal.name, "human", "author", prompt)
    authority = store.adopt(proposal.name, "reviewer")
    page = authority / "content/page-1.md"
    page.write_text(page.read_text(encoding="utf-8") + "\nUser addition.\n", encoding="utf-8")
    before = {p.relative_to(authority): p.read_bytes() for p in authority.rglob("*") if p.is_file()}
    old_spec = paths.Paths(tmp_path).spec
    spec_bytes = {p.relative_to(old_spec): p.read_bytes() for p in old_spec.rglob("*") if p.is_file()}
    result = Store.upgrade_layout(tmp_path)
    upgraded = Store(tmp_path)
    assert not (tmp_path / ".arp/documents.yml").exists()
    assert (tmp_path / ".arp/layout-legacy-documents.yml").exists()
    assert len(result["proposals"]) == 1
    incoming = upgraded.proposal(result["proposals"][0])
    assert (incoming / "content/page-1.md").read_bytes() == page.read_bytes()
    assert not (incoming / "formation.json").exists()
    for relative, body in before.items():
        assert (authority / relative).read_bytes() == body
    for relative, body in spec_bytes.items():
        assert (paths.Paths(tmp_path).spec / relative).read_bytes() == body
    assert Store.upgrade_layout(tmp_path)["proposals"] == result["proposals"]
    upgraded.record(incoming.name, "human", "migration-reviewer", prompt)
    adopted = upgraded.adopt(incoming.name, "migration-reviewer")
    assert (adopted / "original/design.md").read_bytes() == source.read_bytes()
    assert upgraded.inspect(adopted, require_reviewed=True)["reviewed"]


def test_upgrade_candidate_renamed_pages_preserved(tmp_path):
    store, _, proposal = legacy_project(tmp_path)
    page = proposal / "content/page-1.md"
    page.rename(page.with_name("overview.md"))
    report = Store.upgrade_layout(tmp_path)
    incoming = Store(tmp_path).proposal(report["proposals"][0])
    assert not (incoming / "content/page-1.md").exists()
    assert (incoming / "content/overview.md").is_file()
    Store(tmp_path).inspect(incoming)


def test_upgrade_conflicting_spec_refuses_without_mutation(tmp_path):
    store, _, _ = legacy_project(tmp_path)
    (store.directory / "spec").mkdir()
    with pytest.raises(DocumentError, match="both legacy"):
        Store.upgrade_layout(tmp_path)
    assert not (store.arp / "config.yml").exists()
    assert (store.arp / "spec").is_dir()


def test_upgrade_changed_original_refuses_without_mutation(tmp_path):
    store, source, _ = legacy_project(tmp_path)
    source.write_text("changed", encoding="utf-8")
    with pytest.raises(DocumentError, match="source changed"):
        Store.upgrade_layout(tmp_path)
    assert not (store.arp / "config.yml").exists()
    assert (store.arp / "spec").is_dir()
