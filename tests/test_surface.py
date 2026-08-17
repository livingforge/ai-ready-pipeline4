"""表面（スキル）―― **展開し忘れとリンク切れを機械で落とす。**"""

from __future__ import annotations

import importlib.util
import re
from pathlib import Path, PurePosixPath

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


def test_環境はarp4_setupへ委任する(builder, tmp_path: Path) -> None:
    """**arp4 が打てないときに読むものを、工程の地図の中に持たない。**

    起動経路の切り分け（PATH に無い・仮想環境の中にしかない・導入）は
    arp4-setup が 1 枚で持つ。両方に書くと、片方だけ古くなる。
    """
    builder.build(tmp_path, builder.load_manifest())

    for platform in (".claude", ".github"):
        setup = tmp_path / platform / "skills" / "arp4-setup" / "SKILL.md"
        assert setup.is_file(), setup
        assert not (setup.parent / "docs").exists()   # 1 枚で終わる

    body = (SKILL / "body.md").read_text(encoding="utf-8")
    assert "../arp4-setup/SKILL.md" in body           # 委任先が書いてある
    assert "pip install" not in body                  # 導入の手順は 1 か所だけ


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

    docs が書く `../SKILL.md` と、スキル間の `../<スキル>/SKILL.md` は展開先
    （`.claude/skills/<スキル>/`）では実在するが、`surface/` には同じ中身が
    `body.md` の名前で置いてある ―― 表面はプラットフォームごとの `skill_file` を
    持たないためである（→ `manifest.yaml`）。読み替えないと、**正しいリンクが
    壊れていると報告される。**
    """
    parts = PurePosixPath(link).parts
    if parts[-1] == "SKILL.md" and parts[:1] == ("..",):
        if len(parts) == 2:                       # docs から自分のスキルへ
            return page.parents[1] / "body.md"
        if len(parts) == 3:                       # 別のスキルへ（arp4 ⇔ arp4-setup）
            return SURFACE / "skills" / parts[1] / "body.md"
    return page.parent / link


def _pages(manifest: dict) -> list[Path]:
    """検査するページ。**宣言したスキルを全部見る** ―― スキルを足したときに
    リンク検査だけが arp4 に据え置かれると、新しいスキルは無検査で配られる。"""
    pages: list[Path] = []
    for skill in manifest["skills"]:
        source = SURFACE / "skills" / skill["name"]
        pages.append(source / "body.md")
        pages += [source / "docs" / f"{d}.md" for d in skill.get("docs") or []]
    return pages


def test_docsのリンクが切れていない(builder) -> None:
    broken: list[str] = []
    for page in _pages(builder.load_manifest()):
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
