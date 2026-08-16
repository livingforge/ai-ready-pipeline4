"""表面（スキル）―― **展開し忘れとリンク切れを機械で落とす。**"""

from __future__ import annotations

import importlib.util
import re
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
SURFACE = ROOT / "surface"
SKILL = SURFACE / "skills" / "arp4"


def _build_module():
    spec = importlib.util.spec_from_file_location("surface_build",
                                                  ROOT / "build" / "build.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


@pytest.fixture(scope="module")
def builder():
    return _build_module()


def test_展開すると両プラットフォームに出る(builder, tmp_path: Path) -> None:
    builder.build(tmp_path, builder.load_manifest())

    for platform in (".claude", ".github"):
        skill = tmp_path / platform / "skills" / "arp4" / "SKILL.md"
        text = skill.read_text(encoding="utf-8")
        assert text.startswith("---\nname: arp4\n")
        assert "license: MIT" in text
        assert "あなたが担当するのは整理層だけ" in text
        assert (skill.parent / "docs" / "organize.md").is_file()


def test_展開先にPythonを置かない(builder, tmp_path: Path) -> None:
    """実行環境は pip install が用意する。2 か所を直すことにしない。"""
    builder.build(tmp_path, builder.load_manifest())
    assert not list(tmp_path.rglob("*.py"))


def test_宣言から外したdocsは残さない(builder, tmp_path: Path) -> None:
    builder.build(tmp_path, builder.load_manifest())
    stray = tmp_path / ".claude" / "skills" / "arp4" / "docs" / "消したもの.md"
    stray.write_text("x", encoding="utf-8")

    builder.build(tmp_path, builder.load_manifest())
    assert not stray.is_file()


def test_checkは書かずに差分を数える(builder, tmp_path: Path) -> None:
    _, changed = builder.build(tmp_path, builder.load_manifest(), check=True)
    assert changed and not list(tmp_path.iterdir())

    builder.build(tmp_path, builder.load_manifest())
    _, again = builder.build(tmp_path, builder.load_manifest(), check=True)
    assert not again


def test_リポジトリへの展開が最新(builder) -> None:
    """**展開し忘れ**を落とす（CI 用）。"""
    _, changed = builder.build(ROOT, builder.load_manifest(), check=True)
    assert not changed, "python build/build.py で展開してください"


def _target(page: Path, link: str) -> Path:
    """リンクの行き先を **表面での置き場**へ読み替える。

    docs が書く `../SKILL.md` は展開先（`.claude/skills/arp4/`）では実在するが、
    `surface/` には同じ中身が `body.md` の名前で置いてある ―― 表面はプラット
    フォームごとの `skill_file` を持たないためである（→ `manifest.yaml`）。
    読み替えないと、**正しいリンクが壊れていると報告される。**
    """
    if link == "../SKILL.md":
        return SKILL / "body.md"
    return page.parent / link


def test_docsのリンクが切れていない(builder) -> None:
    declared = {d for skill in builder.load_manifest()["skills"]
                for d in skill["docs"]}
    pages = [SKILL / "body.md"] + [SKILL / "docs" / f"{d}.md" for d in declared]

    broken: list[str] = []
    for page in pages:
        text = page.read_text(encoding="utf-8")
        # **画像（`![…](…)`）は見ない。** docs に出てくる画像リンクは、パース結果に
        # 貼られる絵の**書式の例**であって、docs 自身の行き先ではない（実体は
        # 利用者のプロジェクトにしかできない）。行き先が壊れていないかを見たいのは
        # ページ間のリンクのほうである。
        for link in re.findall(r"(?<!!)\[[^\]]*\]\((?!https?:)([^)#]+)\)", text):
            if not _target(page, link).is_file():
                broken.append(f"{page.name} -> {link}")
    assert not broken


def test_cp932で出力できる(builder) -> None:
    """日本語 Windows の既定コンソールで**処理が停止する**のを防ぐ。"""
    bad: dict[str, list[str]] = {}
    for page in SURFACE.rglob("*.md"):
        ng = sorted({hex(ord(c)) for c in page.read_text(encoding="utf-8")
                     if c.encode("cp932", "ignore") == b""})
        if ng:
            bad[page.name] = ng
    assert not bad
