"""テスト共通の道具。**実物と同じ経路**（paths.create → parse → freeze → build）を通す。"""

from __future__ import annotations

from pathlib import Path

import pytest

from arp4 import metamodel as mm
from arp4 import paths as paths_module
from arp4.paths import Paths, Round


@pytest.fixture
def project(tmp_path: Path) -> Paths:
    return paths_module.create(tmp_path)


@pytest.fixture
def round_(project: Paths) -> Round:
    return project.round("2026-08-02")


@pytest.fixture(scope="session")
def model() -> mm.Metamodel:
    resolved, findings = mm.resolve(mm.load_pack("jp-sier-std"))
    assert not [f for f in findings if f.level == "error"]
    return resolved


def sources_dir(project: Paths) -> Path:
    """テストが仮の資料を置くところ。**製品には対応するものが無い。**

    arp4 は元資料を移動させない（配布先の元の場所を parse が直接読む）ので、
    :class:`Paths` に資料置き場のアクセサは無い ―― ここは「配布先が既に持って
    いるフォルダ」の代わりである。根の**中**に置くのは、出典の表示が根からの
    相対で出るためで、外に出すと絶対パスになって期待値が環境依存になる。
    """
    path = project.root / "資料"
    path.mkdir(parents=True, exist_ok=True)
    return path


def write(path: Path, text: str) -> Path:
    """UTF-8 / LF で書く（Windows でも差分を揃える）。"""
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8", newline="\n")
    return path


def parsed(round_: Round, name: str, body: str) -> Path:
    return write(round_.parsed / name, body)


def organized(round_: Round, name: str, body: str) -> Path:
    return write(round_.organized / name, body)


def codes(findings) -> list[str]:
    return [f.code for f in findings]
