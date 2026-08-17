"""テスト共通の道具。**実物と同じ経路**（paths.create → parse → freeze → build）を通す。"""

from __future__ import annotations

from pathlib import Path

import pytest

from arp4 import metamodel as mm
from arp4 import ocr as ocr_module
from arp4 import paths as paths_module
from arp4.paths import Paths, Round


@pytest.fixture(scope="session", autouse=True)
def _no_engine():
    """**テストで Windows OCR を起こさない**（`render` の Excel と同じ理屈）。

    起こすと 3 つとも壊れる ―― CI（Linux）には engine が無いので**環境で
    結果が変わり**、Windows でも入っている言語パックで**読める字が変わり**、
    ブック 1 冊ごとに ``powershell.exe`` が起きるので**遅くなる**。

    「engine は動いたが字は無かった」に寄せるのは、そこが**画像のある検体の
    ほとんどで正しい**からである（`tests/picture.py` の絵柄は、人には読めて
    機械には読めないものばかり）。読んだ字が要るテストは自分で差し替える。

    **セッションごとに掛ける。** 検体を組むフィクスチャ（`parsed`）は
    session スコープで、関数スコープの差し替えより**先に**動く ―― そこだけ
    本物の engine が走ると、正解ファイルがマシンごとに変わる。
    """
    with pytest.MonkeyPatch.context() as patch:
        patch.setattr(ocr_module, "_unavailable", lambda: "")
        patch.setattr(
            ocr_module, "_run",
            lambda bodies: ("", [ocr_module.Reading(language="ja")
                                 for _ in bodies]))
        yield


@pytest.fixture(autouse=True)
def _forget_readings() -> None:
    """読み置きをテストごとに捨てる。**前のテストの読みを持ち越さない。**"""
    ocr_module.forget()
    yield
    ocr_module.forget()


@pytest.fixture
def pdf_reader() -> None:
    """**PDF の期待値は、読む道具が入っている環境でだけ見る。**

    ``pypdfium2`` は ``[parse]`` の追加依存である。入っていない環境で arp4 は
    ``P020`` を出して 1 冊を飛ばす ―― **それが正しい振る舞い**で、資料に中身が
    無いのではないことも申告している。

    同じことをテストが「アンカーがありません」で落ちる形で言うと、依存を
    入れていない環境（中核だけを入れた人・PDF を使わない CI）で**毎回 6 本
    赤くなる** ―― 赤が常態になると、本当に壊れたときの赤が読まれなくなる。
    ここで飛ばすのは**環境の話であって、arp4 の振る舞いの話ではない**。
    """
    pytest.importorskip(
        "pypdfium2",
        reason="PDF を読むには pypdfium2 が要ります"
               '（pip install "ai-ready-pipeline4[parse]"）')


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
