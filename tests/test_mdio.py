"""アンカー付き Markdown ―― **編集に強いこと**が要件である。"""

from __future__ import annotations

from pathlib import Path

from arp4 import mdio
from conftest import write


def _doc() -> mdio.Doc:
    return mdio.Doc(title="基本設計書.xlsx / 受注テーブル",
                    source="資料/基本設計書.xlsx / シート: 受注テーブル",
                    chunks=[
                        mdio.Chunk(anchor="s1-t1", at="B5:H8", heading="表 B5:H8",
                                   rows=[["論理名", "物理名"], ["受注番号", "ORDER_NO"]]),
                        mdio.Chunk(anchor="s1-x1", at="B2", heading="セル B2",
                                   cells=[("B2", "受注テーブル定義書")]),
                    ])


def test_書き出して読み戻せる(tmp_path: Path) -> None:
    path = mdio.write(tmp_path / "受注テーブル.md", _doc())
    parsed = mdio.read(path)

    assert parsed.title == "基本設計書.xlsx / 受注テーブル"
    assert parsed.source.startswith("資料/基本設計書.xlsx")
    assert [a.id for a in parsed.anchors] == ["s1-t1", "s1-x1"]
    assert parsed.by_id["s1-t1"].at == "B5:H8"
    assert "ORDER_NO" in parsed.by_id["s1-t1"].body


def test_見出しを書き換えてもアンカーで追える(tmp_path: Path) -> None:
    """**編集されることが前提**なので、体裁の変更で出典を失ってはならない。"""
    path = mdio.write(tmp_path / "a.md", _doc())
    text = path.read_text(encoding="utf-8").replace("## 表 B5:H8", "## 受注テーブルの列")
    write(path, text)

    parsed = mdio.read(path)
    assert "s1-t1" in parsed.by_id
    assert parsed.by_id["s1-t1"].at == "B5:H8"


def test_本文を直してもアンカーは残る(tmp_path: Path) -> None:
    path = mdio.write(tmp_path / "a.md", _doc())
    text = path.read_text(encoding="utf-8").replace("ORDER_NO", "ORDER_NUMBER")
    write(path, text)

    parsed = mdio.read(path)
    assert "ORDER_NUMBER" in parsed.by_id["s1-t1"].body


def test_縦棒と改行はセルを壊さない(tmp_path: Path) -> None:
    doc = mdio.Doc(title="t", source="s", chunks=[mdio.Chunk(
        anchor="s1-t1", rows=[["a|b", "c\nd"], ["e", "f"]])])
    path = mdio.write(tmp_path / "a.md", doc)
    text = path.read_text(encoding="utf-8")

    assert r"a\|b" in text and "c<br>d" in text
    assert len(mdio.read(path).anchors) == 1


def test_アンカーの無いファイルでも落ちない(tmp_path: Path) -> None:
    path = write(tmp_path / "a.md", "# ただの文書\n\n本文だけ。\n")
    parsed = mdio.read(path)

    assert parsed.title == "ただの文書"
    assert parsed.anchors == []


def test_scan_は決定的な並び(tmp_path: Path) -> None:
    for name in ("b/2.md", "a/1.md", "a/2.md"):
        write(tmp_path / name, "# x\n")
    found = [p.relative_to(tmp_path).as_posix() for p in mdio.scan(tmp_path)]

    assert found == sorted(found)


def test_atに空白があっても読める(tmp_path: Path) -> None:
    """図形のアンカーは ``at=図形 19 個``。**空白で切ると行ごと見えなくなる。**

    見えないと `freeze` の未整理一覧に上がらず、`未読取` の宣言も G004 で弾かれる
    ―― 読めていないものが静かに消える、いちばん避けたい壊れ方だった。
    """
    path = tmp_path / "a.md"
    path.write_text("# a\n\n"
                    "## 表 B8:J20  <!-- a:s1-t1 at=B8:J20 -->\n\n本文1\n\n"
                    "## 図形（テキストのみ）  <!-- a:s1-g1 at=図形 19 個 -->\n\n"
                    "- `図形1` 受注登録\n\n"
                    "## 注記  <!-- a:s1-x1 -->\n\n本文2\n", encoding="utf-8")

    anchors = {a.id: a.at for a in mdio.read(path).anchors}
    assert anchors == {"s1-t1": "B8:J20", "s1-g1": "図形 19 個", "s1-x1": ""}


def test_絵を貼っても本文は残る(tmp_path: Path) -> None:
    path = tmp_path / "a.md"
    path.write_text("# a\n\n## 図形  <!-- a:s1-g1 at=図形 3 個 -->\n\n"
                    "- `図形1` 受注登録\n", encoding="utf-8")

    assert mdio.attach(path, "s1-g1", [("業務フロー", "../images/a.png")])
    body = path.read_text(encoding="utf-8")
    assert "![業務フロー](../images/a.png)" in body
    assert "- `図形1` 受注登録" in body
    assert mdio.images(mdio.read(path).anchors[0]) == ["../images/a.png"]

    # 貼り直しは差し替え（積み上がらない）
    assert mdio.attach(path, "s1-g1", [("業務フロー", "../images/b.png")])
    assert path.read_text(encoding="utf-8").count("![") == 1
    assert not mdio.attach(path, "無いアンカー", [("x", "y.png")])


def test_実体の無い絵は貼り直しで落とす(tmp_path: Path) -> None:
    """開けないリンクは「絵がある」と嘘をつく。撮り方を変えて枚数が減ったとき。"""
    (tmp_path / "img").mkdir()
    (tmp_path / "img" / "残る.png").write_bytes(b"x")
    path = tmp_path / "a.md"
    path.write_text("# a\n\n## 図形  <!-- a:s1-g1 -->\n\n"
                    "![古い](img/消えた.png)\n![残る](img/残る.png)\n\n本文\n",
                    encoding="utf-8")

    assert mdio.attach(path, "s1-g1", [("新しい", "img/残る.png")])
    body = path.read_text(encoding="utf-8")
    assert "img/消えた.png" not in body                  # 実体が無いので落ちた
    assert body.count("img/残る.png") == 1               # 二重に貼らない
    assert "本文" in body


def test_絵の並びは撮り直しても変わらない(tmp_path: Path) -> None:
    """入れ替わると diff が騒ぎ、「増えたのか並び替わったのか」が読めなくなる。"""
    (tmp_path / "img").mkdir()
    for name in ("全体.png", "拡大.png"):
        (tmp_path / "img" / name).write_bytes(b"x")
    path = tmp_path / "a.md"
    path.write_text("# a\n\n## 図形  <!-- a:s1-g1 -->\n\n"
                    "![全体](img/全体.png)\n![拡大](img/拡大.png)\n\n本文\n",
                    encoding="utf-8")

    mdio.attach(path, "s1-g1", [("全体（撮り直し）", "img/全体.png")])
    links = [line for line in path.read_text(encoding="utf-8").splitlines()
             if line.startswith("![")]
    assert links == ["![全体（撮り直し）](img/全体.png)", "![拡大](img/拡大.png)"]


def test_表をセルの値に戻す(tmp_path: Path) -> None:
    """**書いたものが読み戻せること。** 逃がした記号を逃がしたまま返すと、
    照合（`G018`）は資料の値と別のものを見ることになる。"""
    doc = mdio.Doc(title="a.py", source="a.py", chunks=[mdio.Chunk(
        anchor="m1", at="a.py#L1-L9",
        rows=[["メンバ", "シグネチャ", "戻り値"],
              ["load", "load(path: Path)", "dict[str, str] | None"],
              ["dump", "dump(data)", "末尾が逆斜線\\"],
              ["note", "1 行目\n2 行目", "-"]])])
    path = mdio.write(tmp_path / "a.md", doc)

    assert mdio.rows(mdio.read(path).anchors[0]) == [
        ["メンバ", "シグネチャ", "戻り値"],
        ["load", "load(path: Path)", "dict[str, str] | None"],
        ["dump", "dump(data)", "末尾が逆斜線\\"],
        # `-` 1 つのセルは**値である**（区切り行と取り違えると資料の行が消える）。
        ["note", "1 行目\n2 行目", "-"]]


def test_表の外は拾わない(tmp_path: Path) -> None:
    """貼り付けた絵・本文が行として混ざると、照合の相手を取り違える。"""
    path = tmp_path / "a.md"
    path.write_text("# a\n\n## 表  <!-- a:s1-t1 -->\n\n"
                    "| 論理名 | 物理名 |\n|---|---|\n| 受注番号 | ORDER_NO |\n\n"
                    "![図](img/a.png)\n\n本文です\n", encoding="utf-8")

    assert mdio.rows(mdio.read(path).anchors[0]) == [
        ["論理名", "物理名"], ["受注番号", "ORDER_NO"]]
