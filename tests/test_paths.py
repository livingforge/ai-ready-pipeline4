"""置き場 ―― **利用者にどこにいるかを気にさせない。**"""

from __future__ import annotations

from pathlib import Path

import pytest

from arp4 import paths as paths_module
from arp4.paths import Paths


def test_骨組みを作る(tmp_path: Path) -> None:
    paths = paths_module.create(tmp_path)

    assert paths.rounds_dir.is_dir()
    assert paths.metamodel.is_file() and paths.concepts.is_file()
    assert "extends: jp-sier-std" in paths.metamodel.read_text(encoding="utf-8")


def test_配布先の直下には何も作らない(tmp_path: Path) -> None:
    """**arp4 は配布先の名前空間を汚さない。** 作ってよいのは `.arp/` だけである。

    `rounds/` も `sources/` も一般名詞なので、直下に置くと相手の持ち物と衝突する。
    """
    paths_module.create(tmp_path)

    assert [p.name for p in tmp_path.iterdir()] == [".arp"]


def test_資料の置き場は作らない(tmp_path: Path) -> None:
    """元資料は動かさない ―― 集める先を作ると、原本と写しの 2 か所に増える。"""
    paths = paths_module.create(tmp_path)

    assert not (paths.root / "sources").exists()
    assert not (paths.arp / "sources").exists()


def test_ラウンドはarpの下にある(tmp_path: Path) -> None:
    paths = paths_module.create(tmp_path)
    round_ = paths.round("r001")

    assert round_.dir == paths.arp / "rounds" / "r001"
    assert paths.rounds_dir == paths.arp / "rounds"


def test_無視するのはoutだけ(tmp_path: Path) -> None:
    """**`.arp/` を丸ごと無視させない。** parsed/ と organized/ は git 管理である
    （決定 3）―― 無視すると「機械が出したもの」と「人が直したもの」の区別が消える。
    """
    paths = paths_module.create(tmp_path)
    text = (paths.arp / ".gitignore").read_text(encoding="utf-8")

    assert [line for line in text.splitlines()
            if line and not line.startswith("#")] == ["out/"]


def test_作り直しても壊さない(tmp_path: Path) -> None:
    paths = paths_module.create(tmp_path)
    paths.metamodel.write_text("extends: jp-sier-std\nversion: 9\n", encoding="utf-8")
    paths_module.create(tmp_path)

    assert "version: 9" in paths.metamodel.read_text(encoding="utf-8")


@pytest.mark.parametrize(
    "where", ["", ".arp", ".arp/rounds/2026-08-02/parsed/資料"])
def test_上方探索でどこからでも解決する(tmp_path: Path, where: str) -> None:
    paths_module.create(tmp_path)
    start = tmp_path / where
    start.mkdir(parents=True, exist_ok=True)

    assert paths_module.resolve(start).root == tmp_path


def test_見つからなければ次の一手まで言う(tmp_path: Path) -> None:
    with pytest.raises(paths_module.ArpNotFound) as error:
        paths_module.resolve(tmp_path)
    assert "arp4 init" in str(error.value)


# ── 直下に rounds/ を置いていたころのプロジェクト ────────────────
def test_古い置き場が残っていたら止める(tmp_path: Path) -> None:
    """**片方だけ読まない。** parse の書き込み先と freeze の読み込み先が別々に
    なると、どちらもエラーにならないまま件数だけが合わなくなる。
    """
    paths_module.create(tmp_path)
    (tmp_path / "rounds" / "r001" / "parsed").mkdir(parents=True)

    with pytest.raises(paths_module.LegacyLayout) as error:
        paths_module.resolve(tmp_path)
    assert "git mv" in str(error.value)              # 移し方まで言う


def test_配布先が持っているだけのroundsは塞がない(tmp_path: Path) -> None:
    """**名前で撥ねない。** `rounds` は一般名詞である ―― それを塞ぐために `.arp/`
    へ畳んだのに、名前だけで止めたら同じ衝突を別の形で起こす。
    """
    paths_module.create(tmp_path)
    (tmp_path / "rounds" / "第1回" / "議事録.md").parent.mkdir(parents=True)
    (tmp_path / "rounds" / "第1回" / "議事録.md").write_text("x", encoding="utf-8")

    assert paths_module.resolve(tmp_path).root == tmp_path


def test_古い置き場のうえにinitしない(tmp_path: Path) -> None:
    (tmp_path / "rounds" / "r001").mkdir(parents=True)
    (tmp_path / "rounds" / "r001" / "round.yml").write_text(
        "round: r001\n", encoding="utf-8")

    with pytest.raises(paths_module.LegacyLayout):
        paths_module.create(tmp_path)


def test_ラウンドは名前順で最新が最後(project: Paths) -> None:
    for name in ("2026-11-14", "2026-08-02"):
        project.round(name).parsed.mkdir(parents=True)

    assert [r.name for r in project.rounds()] == ["2026-08-02", "2026-11-14"]
    assert project.latest_round().name == "2026-11-14"


def test_新しいラウンドは連番(project: Paths) -> None:
    """**日付で切らない。** 同じ資料を別の日に処理し直しても別ラウンドにしない。"""
    assert project.new_round().name == "r001"
    project.round("r001").parsed.mkdir(parents=True)
    assert project.new_round().name == "r002"


def test_連番は日付名のラウンドより後ろに並ぶ(project: Paths) -> None:
    """途中で切り替えても「いちばん新しいラウンド」がずれない。"""
    project.round("2026-08-02").parsed.mkdir(parents=True)
    project.round(project.new_round().name).parsed.mkdir(parents=True)

    assert [r.name for r in project.rounds()] == ["2026-08-02", "r001"]
    assert project.latest_round().name == "r001"


def test_作業中のラウンドは凍結すると閉じる(project: Paths) -> None:
    round_ = project.round("r001")
    round_.organized.mkdir(parents=True)
    assert project.open_round().name == "r001"

    round_.frozen.write_text("frozen_at: '2026-08-02'\n", encoding="utf-8")
    assert project.open_round() is None


def test_ラウンドを開いた日は残る(project: Paths) -> None:
    """名前から日付を外した代わりに、いつ開いたかは round.yml に残す。"""
    round_ = project.round("r001")
    round_.open(reason="最初のラウンド", today="2026-08-02")
    round_.open(reason="別の理由", today="2026-11-14")      # 冪等（開始日は動かない）

    text = round_.meta.read_text(encoding="utf-8")
    assert "started_at: '2026-08-02'" in text and "最初のラウンド" in text


def test_ラウンドの中の置き場(project: Paths) -> None:
    round_ = project.round("2026-08-02")

    assert round_.parsed.parent == round_.dir
    assert round_.frozen.parent == round_.organized
    assert not round_.is_frozen()
