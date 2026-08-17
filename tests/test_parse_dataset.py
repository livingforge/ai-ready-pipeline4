"""データで書いた検体（``tests/dataset/*.yml``）を丸ごとパースして確かめる。

:mod:`test_parse_corpus` と役割は同じで、**検体の書き方だけが違う**
―― あちらは 1 冊ごとに Python の関数、こちらは YAML の宣言である
（なぜ分けたかは :mod:`dataset` の docstring）。

ここも 1 本ずつブックを組み立てない。``arp4 parse sources/`` と同じく**20 冊を
1 度に**通し、各テストが 1 つの観点だけを見る ―― 実運用で起きるのは「4 冊が
開けなくて 16 冊が落ちる」であって、「1 冊だけを完璧にパースする」ではない。

規律はどれも同じところに戻ってくる ―― **画面に見えているものへ寄せ、読めな
かったものは黙らない**。**中身を読む前に壊れるもの**（開けない・大きすぎる・
そもそも資料でない）も同じ規律の下にある ―― `異常.yml` がそこを担ぐ。
"""

from __future__ import annotations

import ast
import os
import re
import time
from pathlib import Path

import pytest

import dataset
from arp4 import mdio, parse

#: **形式ごとに 1 冊だけ**、パース結果を丸ごと置いてある（→ `docs/parsed.md`）。
#: ほかの検体は「1 つの観点だけを見る」テストで押さえるが、現場の設計書 1 冊は
#: **出来上がりそのものを人が読んで**正しさを判定できないと確かめようがない。
GOLDEN = Path(__file__).with_name("dataset") / "正解"

#: その 1 冊（Excel）。
BOOK = "資料/K/受注管理システム基本設計書（第3.2版）.xlsx"

#: Excel 以外の 1 冊ずつ。**丸ごと置くのを Excel だけにしていた**あいだ、
#: 観点の隙間（申告の二重出し・塊の並び・節の名前の付き方）は Excel でしか
#: 見えていなかった ―― 形式が違えば割り方も申告も別の実装なので、**隙間も
#: 形式ごとに別の場所にある。**
PAPER = "資料/P/受注登録機能仕様書（第1.2版）.docx"
DECK = "資料/O/新販売管理システム方式提案（第2.1版）.pptx"
#: PDF の本文は **pypdfium2 が読んだ字**である ―― 行の中の連続した空白が
#: どう畳まれるかは読み手の実装で決まるので、`pypdfium2` を上げたときに
#: ここだけ差分が出ることがある。**そのときは差分を読んで書き直す**
#: （arp4 が壊れたのではない、と分かる形でここに書いておく）。
ACCEPT = "資料/Q/受注管理システム検収仕様書（第1.0版）.pdf"
#: CSV は**1 ファイルが 1 本**である（割る構造を持たない）ので、パース結果の
#: 道も `…csv.md` になる ―― 丸ごと置く相手としては、そこも含めて 1 冊である。
LIST = "資料/N/得意先マスタ移行.csv"

#: 丸ごと置いてある原本。**1 形式に 1 冊**（増やすと維持費だけが倍になる）。
FROZEN = (BOOK, PAPER, DECK, ACCEPT, LIST)

#: その**下流**の 1 冊（機能 1 本ぶんの詳細設計書）。**正解は置かない** ――
#: 丸ごと置くのは「出来上がりを人が読んで判定する」ためで、同じ役目の 2 冊目を
#: 置くと**同じ理由の維持費が倍になる**（`設計書.yml` が担ぐと決めてある）。
#: こちらは横に長い表と時間の値だけを、1 観点 1 本のテストで押さえる。
DETAIL = "資料/L/受注登録詳細設計書（SCR001）第1.4版.xlsx"

#: 正解を書き直すときの合図。**既定では絶対に書かない** ―― 期待値が黙って
#: 出力に合わせて動くと、テストは何も検査しなくなる。
#:
#: ```bash
#: ARP4_GOLDEN=write .venv/Scripts/python -m pytest tests/test_parse_dataset.py -q
#: git diff tests/dataset/正解/       # ← **差分を読んでから**コミットする
#: ```
_WRITE = os.environ.get("ARP4_GOLDEN") == "write"


@pytest.fixture(scope="module")
def dataset_source(tmp_path_factory: pytest.TempPathFactory) -> Path:
    directory = tmp_path_factory.mktemp("dataset") / "sources"
    directory.mkdir()
    dataset.build(directory)
    return directory


@pytest.fixture(scope="module")
def parsed(dataset_source: Path, tmp_path_factory: pytest.TempPathFactory):
    """検体全体のパース結果 ``{パース結果の相対パス: Doc}`` と findings。"""

    class _Round:
        parsed = tmp_path_factory.mktemp("dataset-parsed")
        images = tmp_path_factory.mktemp("dataset-images")

    targets, findings = parse.plan(_Round(), [dataset_source], dataset_source)
    docs = {t.path.relative_to(_Round.parsed).as_posix(): t.doc for t in targets}
    return docs, findings


def _doc(parsed, name: str):
    docs, _ = parsed
    assert name in docs, f"{name} がありません（あるのは {sorted(docs)}）"
    return docs[name]


def _table(doc, anchor: str | None = None) -> list[list[str]]:
    tables = [c for c in doc.chunks if c.rows
              and (anchor is None or c.anchor == anchor)]
    assert tables, f"表がありません（{[c.anchor for c in doc.chunks]}）"
    return tables[0].rows


def _chunk(doc, anchor: str):
    found = [c for c in doc.chunks if c.anchor == anchor]
    assert found, f"{anchor} がありません（{[c.anchor for c in doc.chunks]}）"
    return found[0]


def _notes(doc) -> str:
    return "\n".join(doc.notes)


# ── 検体そのもの ────────────────────────────────────────────────
def test_検体はデータとして読める(dataset_source: Path) -> None:
    """**検体に Python が要らない。** 資料の写しは手続きではなく事実の列である。"""
    assert len(dataset.specs()) >= 12
    assert all("なぜ" in spec for spec in dataset.specs())     # 何を突くかを書く


def test_検体がExcelで開ける形になっている(dataset_source: Path) -> None:
    """**Excel が描けないものは検体ではない。**

    描画パートを zip に置いてシートから関係を張るところまでは合っていたが、
    それだけでは Excel に何も出ない ―― arp4 は関係だけを辿るので**パースは
    通り、実物の Excel で開いたときにだけ図が消えていた**。人が開いて確かめ
    られず、`arp4 render` も空の絵しか撮れない検体だった。

    足りなかったのは 3 つで、どれもここで見る。実物の Excel で開いて確かめる
    テストは置けない（CI に Office は無い）ので、**Excel が見に行く場所に
    書いてあるか**だけを構造で押さえる。
    """
    import xml.etree.ElementTree as ET
    import zipfile

    seen = 0
    for book in sorted(dataset_source.rglob("*.xlsx")):
        try:
            archive = zipfile.ZipFile(book)
        except zipfile.BadZipFile:
            continue                                   # 開けない 1 冊は検体である
        names = archive.namelist()
        if "[Content_Types].xml" not in names:
            continue                                   # zip だがブックでない検体
        kinds = archive.read("[Content_Types].xml").decode("utf-8")
        for part in [n for n in names if n.startswith("xl/drawings/drawing")]:
            shapes = archive.read(part).decode("utf-8")
            assert ET.fromstring(shapes) is not None, part
            assert f'PartName="/{part}"' in kinds, f"{part} の種別が申告されていません"
            seen += 1
            # ① 位置と寸法と形と文字枠 ―― どれか 1 つでも欠けると図が出ない
            #    （何が要るかは図形の種類で違う。枠は ``xdr:xfrm`` を持つ）
            required = ["<xdr:from>", "<xdr:to>"]
            if "<xdr:sp " in shapes:
                required += ["<a:xfrm>", "<a:prstGeom", "<a:bodyPr"]
            if "<xdr:pic>" in shapes:
                required += ["<a:xfrm>", "r:embed="]
            if "<xdr:graphicFrame " in shapes:
                required += ["<xdr:xfrm>"]
            if "/diagram" in shapes:
                # SmartArt は 4 パートが揃って初めて図になる
                required += ["<dgm:relIds", "r:dm=", "r:lo=", "r:qs=", "r:cs="]
            for one in required:
                assert one in shapes, f"{part} に {one} がありません"
            # ② 画像は実体まで要る（無いと Excel が「修復」して落とす）
            for media in re.findall(r'r:embed="([^"]+)"', shapes):
                rels = archive.read(
                    f"xl/drawings/_rels/{Path(part).name}.rels").decode("utf-8")
                assert media in rels, f"{part} の {media} が関係にありません"

        for sheet in [n for n in names if n.startswith("xl/worksheets/sheet")]:
            try:
                body = archive.read(sheet).decode("utf-8")
            except UnicodeDecodeError:
                continue                               # わざと壊した 1 枚である
            if not body.rstrip().endswith(">"):
                # **わざと途中で切った 1 枚**（`壊す`）。ここだけは XML として
                # 壊れているのが検体の中身なので、以下の検査から外す ――
                # 実物の Excel でも開けない（修復モードでも戻らないことを
                # 確かめてある）ことが、`P012` の案内の根拠になっている。
                continue
            # ④ **手で書いた高さ 0 / 幅 0 は要素の順も属性の重なりも崩さない。**
            #    openpyxl が 0 を書けないのでシート XML に直に入れている ――
            #    ``customHeight`` を二重に付ければ XML として壊れ、``<cols>`` を
            #    ``<sheetData>`` の後ろに置けばブックごと開けなくなる
            assert ET.fromstring(body) is not None, sheet
            if "<cols>" in body:
                assert body.index("<cols>") < body.index("<sheetData"), sheet
            if "<drawing " not in body:
                continue
            # ③ 要素の順番。逆に置くと**ブックごと開けなくなる**
            assert "<legacyDrawing" not in body or (
                body.index("<drawing ") < body.index("<legacyDrawing")), sheet

        # ⑤ **ブックのプロパティは差し替えても関係から辿れる形にする。**
        #    ``docProps/`` は慣習であって規約ではないので、パースは
        #    ``_rels/.rels`` から辿る（辿れなければ黙って消える）
        root = ET.fromstring(archive.read("_rels/.rels"))
        for relation in root:
            target = (relation.get("Target") or "").lstrip("/")
            if target.startswith("docProps/"):
                assert target in names, target
                assert ET.fromstring(archive.read(target)) is not None, target
    assert seen >= 4                                   # 描かせている描画パート


def test_Noを偽と読まない(parsed) -> None:
    """YAML 1.1 は ``No`` を偽と読む ―― **課題一覧の 1 列目はたいてい ``No``**。

    パース結果は正しいのに検体のほうが間違っている、というのがいちばん質の
    悪い失敗である（落ちたテストから直す場所が決まらない）。
    """
    assert _table(_doc(parsed, "資料/I/議事録.xlsx/指摘.md"))[0][0] == "No"


def test_壊れた4冊と読めない1本で残りが落ちない(parsed) -> None:
    """開けない 4 冊＋骨格の取れないソース 1 本。**残りは全部読めている。**"""
    docs, findings = parsed
    assert [f.code for f in findings if f.level == "error"] == ["P010"] * 5
    assert len(docs) >= 28


# ── 結合（画面と食い違うところ） ────────────────────────────────
def test_縦に広がる結合は幅を問わず下へ展開する(parsed) -> None:
    """**区分を 2 列ぶんまとめて括るのは普通の書き方である。**

    幅 1 だけを展開していた頃は、面結合（`A10:B11`）の 2 行目が空欄になり、
    整理層には「区分の無い行」に見えた ―― 画面ではその区分が全行に掛かって
    見えているのだから、これは判断ではなく忠実性の回復である。
    """
    table = _table(_doc(parsed, "資料/F/テーブル定義書.xlsx/受注ヘッダ.md"))
    区分 = [row[0] for row in table]
    assert 区分[2:] == ["区分", "ヘッダ", "ヘッダ", "ヘッダ",
                       "ヘッダ", "ヘッダ", "ヘッダ", "明細", "明細"]
    小区分 = [row[1] for row in table]
    assert 小区分[2:] == ["小区分", "キー", "キー", "キー",
                         "金額", "金額", "金額", "", ""]


def test_横結合は広げない(parsed) -> None:
    """**1 行だけの結合は「同上」ではなく表題**である（値は 1 つしか見えていない）。"""
    table = _table(_doc(parsed, "資料/F/テーブル定義書.xlsx/受注ヘッダ.md"))
    assert table[0][:2] == ["受注テーブル（T_ORDER）定義書", ""]
    assert table[1] == ["分類", "", "名前", "", "型", ""]     # 二段見出しの上段


# ── 数の表記（Excel が見せる桁） ────────────────────────────────
def test_Excelが見せない桁を出さない(parsed) -> None:
    """**保存されている数と、画面に出ている数は違う。**

    二進の浮動小数点なので `=1200*0.08` は `96.00000000000001` として保存
    されるが、Excel は有効数字 15 桁までしか持てないので画面には `96` と
    出ている ―― 素の `str()` は**資料に一度も書かれていない桁**を仕様にする。
    """
    rows = {row[0]: row[1] for row
            in _table(_doc(parsed, "資料/F/テーブル定義書.xlsx/桁と丸め.md"))}
    assert rows["消費税額"] == "96"
    assert rows["按分率"] == "0.333333333333333"          # 16 桁目は切る
    assert rows["単価"] == "1200"                         # 通貨書式は触らない
    assert rows["想定件数"] == "1200000"


def test_15桁を超える整数は0埋めで見えている(parsed) -> None:
    """**Excel の画面もそう出ている。** 文字列で持たれた番号のほうは触らない。

    型で扱いを変えると、同じ表の隣り合う 2 行が別の桁数で出る ―― 読み手は
    それを「片方は 19 桁の番号だ」と読む。
    """
    rows = {row[0]: row[1] for row
            in _table(_doc(parsed, "資料/F/コード値一覧.xlsx/桁あふれ.md"))}
    assert rows["会員番号（数値）"] == "1234567890123460000"
    assert rows["会員番号（文字列）"] == "1234567890123456789"   # 数ではない
    assert "e+" not in mdio.dump(_doc(parsed, "資料/F/コード値一覧.xlsx/桁あふれ.md"))


def test_書式のエスケープをパーセントと読まない(parsed) -> None:
    """``0\\%`` の ``\\`` は「次の 1 文字をそのまま出す」という OOXML の
    エスケープである ―― 画面に ``15%`` と出ている欄が ``1500%`` になると、
    **資料に無い数**が仕様になる。
    """
    rows = {row[0]: row[1] for row
            in _table(_doc(parsed, "資料/F/テーブル定義書.xlsx/率の書き方.md"))}
    assert rows["達成率"] == "15.3%"                      # 本物のパーセント書式
    assert rows["消費税率"] == "10%"
    assert rows["増減率"] == "-2.5%"                      # 負の節が付いていても
    assert rows["評価点"] == "15"                         # 0\% は 100 倍しない
    assert rows["係数"] == "15"                           # 引用符の中も同じ


def test_数に見えて数でない値を作り変えない(parsed) -> None:
    """前ゼロ・桁区切り・ハイフンは**資料の側が文字列として持っている**。"""
    rows = {row[1]: row[0] for row
            in _table(_doc(parsed, "資料/F/コード値一覧.xlsx/支店コード.md"))}
    assert rows["本店"] == "007"
    assert rows["札幌"] == "0001"
    assert rows["数として入った 7"] == "7"
    assert rows["桁区切りの入った文字列"] == "1,200"
    assert rows["日付に見える文字列"] == "2026/8/2"       # 日付に直さない
    assert rows["空白だけのセル"] == ""                   # 画面では空欄である


# ── 塊の切り出しとアンカーの採番 ────────────────────────────────
def test_塊が12個でもアンカーがずれない(parsed) -> None:
    """**塊が 1 つ増えるたびにアンカーがずれると、前のラウンドの整理結果が
    指す先が黙って別の表になる。** 採番は左上からの並び順で決まる。
    """
    doc = _doc(parsed, "資料/F/一覧のならび.xlsx/一覧.md")
    assert [c.anchor for c in doc.chunks] == [f"s1-t{i}" for i in range(1, 13)]
    assert [c.at for c in doc.chunks][:4] == ["A1:B2", "E1:F2", "I1:J2", "M1:N2"]
    assert _chunk(doc, "s1-t5").rows[1] == ["SCR002", "受注一覧"]


def test_方眼紙は塊が散るが番地は全部付く(parsed) -> None:
    """**セルを方眼にして画面を描いたシート**は、塊が 20 個に散る。

    区切りは提示上の都合なので散ること自体は誤りではない ―― 落ちている値が
    無いこと（どのセルにも番地が付いていること）だけが約束である。
    """
    doc = _doc(parsed, "資料/I/画面方眼紙.xlsx/受注入力レイアウト.md")
    assert all(c.cells and not c.rows for c in doc.chunks)     # 表にはならない
    出た = {ref for c in doc.chunks for ref, _ in c.cells}
    assert {"A1", "K3", "AC6", "AP14", "BH40"} <= 出た
    assert len(出た) == 27


def test_空欄の多い表は表のまま(parsed) -> None:
    """**任意項目の多い表がぜんぶ箇条書きになっては困る。** 大きさと密度を両方見る。"""
    doc = _doc(parsed, "資料/I/画面方眼紙.xlsx/任意項目の多い表.md")
    assert [c.anchor for c in doc.chunks] == ["s2-t1"]
    assert "表ではなく番地付きの箇条書き" not in _notes(doc)


# ── 図形 ────────────────────────────────────────────────────────
def test_行区切りで語が繋がらない(parsed) -> None:
    """**改行は 2 通りの書かれ方をする。**

    段落（`a:p`）だけを改行として扱っていたぶん、`承認待ち` と `3 営業日以内`
    が `承認待ち3 営業日以内` という 1 語に化けていた ―― 整理層はそれを
    1 つの状態名として読む（元の 2 語には戻せない）。
    """
    doc = _doc(parsed, "資料/G/業務フロー集.xlsx/受注フロー.md")
    labels = [text for _, text in _chunk(doc, "s1-g1").cells]
    assert labels == ["受注登録", "承認待ち\n3 営業日以内", "出荷指示"]


def test_矢羽根の向きを起こし直す(parsed) -> None:
    """**始点側に付いた矢羽根**を素直に出すと、資料と逆の遷移が仕様になる。

    付いていない線は無向のまま出す（決めつけない）。
    """
    doc = _doc(parsed, "資料/G/業務フロー集.xlsx/画面遷移.md")
    assert _chunk(doc, "s2-c1").rows[1:] == [
        # 矢羽根は始点側にあった／`type="none"` は矢印ではない。末尾は名前と線種
        # （検体の生成器が振った自動名も**そのまま**並べる）。
        ["メニュー", "→", "受注一覧\nSCR002", "図形30", "実線"],
        ["受注一覧\nSCR002", "―", "受注入力", "図形31", "実線"]]


def test_双方向と自己ループをそのまま出す(parsed) -> None:
    """どちらも実物の業務フローに普通にある ―― **意味は付けない。**"""
    doc = _doc(parsed, "資料/G/業務フロー集.xlsx/受注フロー.md")
    向き = {(元, 先): 矢 for 元, 矢, 先, *_線 in _chunk(doc, "s1-c1").rows[1:]}
    assert 向き[("承認待ち\n3 営業日以内", "出荷指示")] == "↔"
    assert 向き[("承認待ち\n3 営業日以内", "承認待ち\n3 営業日以内")] == "→"


def test_取れない線は2種類あり案内が違う(parsed) -> None:
    """**囲み枠へ繋がった線と、どこにも繋がっていない線を混ぜない。**

    2 本まとめて「両端が図形に結びついておらず（線を目分量で置いた図）」と
    申告していたが、片方は**繋がっている** ―― 相手（ゾーンの囲み枠）が文字を
    持たないだけで、絵にすればどこへ向かう線かは読める。もう片方は両端の id が
    資料に無いので絵にしても決まらない。**次にやることが正反対**なのに、
    片方には成り立たない案内を両方へ出していた（申告のほうが嘘になる）。
    """
    doc = _doc(parsed, "資料/G/業務フロー集.xlsx/受注フロー.md")
    assert "図形 4 個・接続子 5 本" in _notes(doc)
    assert "接続子 1 本は、繋がってはいますが相手の図形が文字を持ちません" \
        in _notes(doc)
    assert "接続子 1 本はどこにも繋がっていません" in _notes(doc)


def test_SmartArtが2つあっても両方読む(parsed) -> None:
    """**描画パートは 1 本のまま関係が 2 本になる。** 1 本しか辿らないと、
    後から足したほうの図が丸ごと消える（体制図と流れ図を並べるのは普通である）。
    """
    doc = _doc(parsed, "資料/G/体制と役割.xlsx/体制.md")
    assert "SmartArt 2 個（箱 6 個）" in _notes(doc)
    labels = [text for _, text in _chunk(doc, "s1-g1").cells]
    assert labels == ["PM", "品質保証", "基盤チーム\n（インフラ・共通）\n受注・請求",
                      "受付", "与信", "出荷"]


# ── グラフ ──────────────────────────────────────────────────────
def test_散布図の参照範囲を取る(parsed) -> None:
    """**どのタグに入るかはグラフの種類で変わる。**

    棒・折れ線・円は `c:cat` / `c:val` だが、散布図とバブルは `c:xVal` /
    `c:yVal` である ―― 前者しか見ていなかったぶん、散布図だけが分類も値も
    空欄のまま「参照範囲を取り出しました」と申告していた（申告のほうが嘘）。
    """
    doc = _doc(parsed, "資料/G/実績グラフ.xlsx/相関図.md")
    assert _chunk(doc, "s2-k1").rows[1] == [
        "件数と応答時間の相関", "応答時間",
        "'元データ'!$A$2:$A$4", "'元データ'!$B$2:$B$4"]
    assert "参照先は「元データ」シート" in _notes(doc)


def test_タイトルがセル参照でも取る(parsed) -> None:
    """**実物では表題をセルから引くほうが多い。** `a:t` しか見ていなかった
    ぶん、そのグラフだけが「（タイトルなし）」になっていた。
    """
    doc = _doc(parsed, "資料/G/実績グラフ.xlsx/相関図.md")
    assert "（タイトルなし）" not in mdio.dump(doc)


def test_人が打ったタイトルは変わらない(parsed) -> None:
    """セル参照を読めるようにしても、**打った文字のほうを先に採る。**"""
    doc = _doc(parsed, "資料/G/実績グラフ.xlsx/構成比.md")
    assert _chunk(doc, "s3-k1").rows[1] == [
        "区分別の構成比", "系列1",                        # 系列名は書かれていない
        "'元データ'!$C$2:$C$4", "'元データ'!$A$2:$A$4"]


# ── 読めなかったものの申告 ──────────────────────────────────────
def test_再表示できない隠し方を区別する(parsed) -> None:
    """**隠し方は 2 通りある。**

    右クリックの「再表示」で戻せる `hidden` と、そこに出てこない
    `veryHidden` である。同じ案内をしていたぶん、後者は「再表示してください」
    と言われて**メニューに無い**ところで止まっていた ―― 案内どおりにやって
    届かない申告は、読まれなくなるのと同じである。
    """
    _, findings = parsed
    hidden = {f.target: f.message for f in findings if f.code == "P003"}
    assert "隠した旧版" in hidden["移行対象一覧.xlsx"]
    assert "veryHidden" in hidden["移行対象一覧.xlsx"]
    assert "VBE のプロパティ" in hidden["移行対象一覧.xlsx"]
    assert "veryHidden" not in hidden["様式サンプル.xlsx"]      # ただの hidden


def test_エラー値は種類を並べて数える(parsed) -> None:
    """**次にやることは種類で変わらない**（どれも開き直しても直らない）が、
    何が起きているかは種類にしか書いていない。
    """
    doc = _doc(parsed, "資料/H/移行対象一覧.xlsx/突合結果.md")
    kinds = {value for _, value in _chunk(doc, "s1-e1").cells}
    assert {"#REF!", "#N/A", "#DIV/0!", "#VALUE!",
            "#NAME?", "#NUM!", "#NULL!"} <= kinds
    assert "開き直しても直りません" in _notes(doc)


def test_番地を並べきれないときは切ったと言う(parsed) -> None:
    """**黙って切らない。** 20 個で切れているのに黙ると「20 個だけ」と読める。"""
    doc = _doc(parsed, "資料/H/移行対象一覧.xlsx/突合結果.md")
    cells = _chunk(doc, "s1-e1").cells
    assert len(cells) == 21 and cells[-1][0] == "…"
    assert "ほか" in cells[-1][1]

    batch = _doc(parsed, "資料/H/バッチ仕様書.xlsx/処理時間.md")
    assert _chunk(batch, "s1-f1").cells[-1] == (
        "…", "ほか 3 個（番地はここに並べきれません）")
    assert "ほか 3 個" in _notes(batch)


def test_数式しかないシートも1本出る(parsed) -> None:
    """非空セルが 0 になるので、黙っているとシートごと消える。"""
    doc = _doc(parsed, "資料/H/バッチ仕様書.xlsx/派生値のみ.md")
    assert [ref for ref, _ in _chunk(doc, "s2-f1").cells] == ["A1", "A2"]


def test_申告が7本並んでもアンカーがぶつからない(parsed) -> None:
    """**実物の画面仕様書は 1 枚に全部入っている。**

    1 つずつなら通っていた申告が、重なったときに崩れないかを見る ――
    アンカーが 1 つでもぶつかると、整理結果の出典が黙って別の塊を指す。
    """
    doc = _doc(parsed, "資料/H/画面仕様書.xlsx/受注入力.md")
    anchors = [c.anchor for c in doc.chunks]
    assert anchors == ["s1-t1", "s1-f1", "s1-e1", "s1-m1", "s1-l1",
                       "s1-d1", "s1-a1", "s1-i1", "s1-o1", "s1-g1"]
    assert len(anchors) == len(set(anchors))
    assert len(doc.notes) == 7
    # 表の値は 1 つも落ちていない（消してある行も、隠してある行も残す）
    assert ["SCR001-04", "希望納期", "×", "", "", "廃止（受注日から自動計算へ）"] \
        in _table(doc)
    assert ["旧仕様（2025 年度版）", "", "", "", "", ""] in _table(doc)
    # **強調は値を偽らない。** 取り消し線だけが「消してある」と見せている
    assert "C4" not in {ref for ref, _ in _chunk(doc, "s1-d1").cells}


# ── スレッドコメント ────────────────────────────────────────────
def test_返信の返信を落とさない(parsed) -> None:
    """**``parentId`` の連なりは 1 段とは限らない。** 画面では返信が縦に並ぶ
    だけなので、落ちていても誰も気付かない。
    """
    doc = _doc(parsed, "資料/I/議事録.xlsx/指摘.md")
    本文 = [text for ref, text in _chunk(doc, "s1-m1").cells if ref == "C3"]
    assert 本文 == [
        "設計者（2026-07-01）: 採番は 2026 年度から 10 桁へ",
        "返信 レビュア（2026-07-02）: 移行対象の洗い出しは 8/20 まで",
        "返信 設計者（2026-07-03）: 洗い出しの結果は別紙 3 のとおり",
        "レビュア（2026-07-04）: 桁を変えると外部 IF も直る必要がある"]


def test_親を失った返信も出す(parsed) -> None:
    """**いちばん静かに消える形。** 起点のコメントだけ削除された資料・抜粋して
    配られた資料でこうなり、そのセルにコメントが付いていたことすら伝わらない。
    """
    doc = _doc(parsed, "資料/I/議事録.xlsx/指摘.md")
    orphan = [text for ref, text in _chunk(doc, "s1-m1").cells if ref == "C6"]
    assert orphan == ["返信（起点のコメントは資料に残っていません） "
                      "記入者不明: 親を消された返信（起点のコメントがパートに無い）"]
    assert "コメント（メモ）が 6 件" in _notes(doc)


def test_解決済みは頭に付く(parsed) -> None:
    """片付いた指摘と生きている指摘が混ざると、**整理層は全部を積み残しとして読む。**"""
    doc = _doc(parsed, "資料/I/議事録.xlsx/指摘.md")
    assert ("C4", "[解決済み] レビュア（2026-06-20）: 桁あふれの検討結果は"
            "別紙 1 のとおり") in _chunk(doc, "s1-m1").cells


# ── シートの扱い ────────────────────────────────────────────────
def test_全角空白のシート名でも1枚も消さない(parsed) -> None:
    """``strip()`` を通すと隣のシートと同じ名前になる ―― **半角空白だけを
    見ていると気付けない**（全角で囲むのは実物にごく普通にある）。
    """
    docs, findings = parsed
    assert docs["資料/J/様式サンプル.xlsx/受注.md"].chunks[0].cells[0][1] == "受注（新）"
    assert docs["資料/J/様式サンプル.xlsx/受注~2.md"].chunks[0].cells[0][1] == "受注（旧）"
    assert [f.code for f in findings if f.code == "P002"] == ["P002"]


def test_数字だけのシート名も出す(parsed) -> None:
    docs, _ = parsed
    assert "資料/J/様式サンプル.xlsx/2026.md" in docs
    assert "資料/J/様式サンプル.xlsx/白紙.md" not in docs         # 空は出さない


# ── 詳細設計書（横に長い 1 枚と、時間の値） ────────────────────
#
# `設計書.yml`（基本設計書）の**下流**の 1 冊である。あちらは 9 種類のシートを
# 1 冊に綴じるので 1 枚が浅く、こちらは機能 1 本ぶんなのに 1 枚が横に長い ――
# **縦に伸びる表と横に伸びる表は、機械から見て別の資料である**。
def test_横に長い表は見出し列も申告する(parsed) -> None:
    """**繰り返し印刷する見出し「列」を一度も見ていなかった。**

    見出し行（``print_title_rows``）はどの検体にもあるが、見出し列
    （``print_title_cols``）は 34 冊のどれも指定しておらず、:func:`parse._print`
    が読んでいるのに**申告へ 1 度も出たことがなかった**。横に長い表は紙にすると
    左右に割れるので、実物では左の 3 列を毎ページ刷る ―― 取らないと、2 枚目
    以降が「項目 No も名前も物理名も無い、型と桁だけの紙」であることが
    読み手に伝わらない。
    """
    doc = _doc(parsed, f"{DETAIL}/項目編集仕様.md")
    printed = dict(_chunk(doc, "s2-p1").cells)
    assert printed["印刷タイトル列"] == "$A:$C"      # ここで初めて出る
    assert printed["印刷タイトル行"] == "$2:$3"      # 二段見出しは人が決めた


def test_見出し列は申告の文には出ないが番地では読める(parsed) -> None:
    """**提示の偏りであって、忠実性の欠落ではない。**

    ``>`` の申告文が名前を挙げるのは見出し行だけで、見出し列は挙げない
    （:func:`parse._print_note`）。アンカーには番地付きで出ているので読み直せる
    ―― 落ちているなら直すが、**読める場所に出ているものを二重に言うかどうか**
    は提示の決めごとである。**気付かないまま片方だけ出ている**のと、
    決めてそうしているのを分けるために、ここで固定しておく。
    """
    doc = _doc(parsed, f"{DETAIL}/項目編集仕様.md")
    assert "繰り返し印刷する見出し行" in _notes(doc)
    assert "見出し列" not in _notes(doc)             # 文には出ない
    assert "印刷タイトル列" in str(_chunk(doc, "s2-p1").cells)   # 番地では読める


def test_時刻と経過時間と日時を混ぜない(parsed) -> None:
    """**YAML に素直に書くと 60 進数の数になる。**

    YAML 1.1 は ``02:00`` を 120、``27:30`` を 1650 と読む。:mod:`dataset` は
    明示形（``時刻`` / ``経過`` / ``日時``）でそこを塞いでいるが、**その明示形を
    使う検体がこれまで 1 つも無かった**ので、塞げているかを誰も確かめていな
    かった ―― 検体のほうが間違っていると、パース結果は正しいのに落ちる。

    3 つは画面の見え方が似ているのに値の型が違う。詳細設計書のタイムアウト・
    リトライ間隔・測定日時はこの形でしか書かれない。
    """
    rows = _table(_doc(parsed, f"{DETAIL}/エラー処理.md"))
    間隔, タイムアウト, 通知 = rows[6][5], rows[6][6], rows[6][7]
    assert (間隔, タイムアウト, 通知) == ("0:01", "0:03", "07:00")
    assert rows[6][0] == "E2001"                     # 行がずれていないこと


def test_24時間を超える経過時間は時刻に丸めない(parsed) -> None:
    """**``27:30`` は時刻として存在しない。**

    月次締めの累計処理時間は日を跨ぐので ``[h]:mm`` でしか画面に出ない ――
    時刻として読むと ``03:30`` になり、**1 日ぶん短く見える**。
    """
    rows = _table(_doc(parsed, f"{DETAIL}/性能と試験.md"))
    月次 = next(row for row in rows if "月次締め" in row[1])
    assert (月次[2], 月次[3]) == ("30:00", "27:30")
    assert 月次[4] == "2026-07-26 02:00:00"          # 日時は日付と時刻の両方


def test_YとNのフラグを真偽値と読まない(parsed) -> None:
    """**必須・入力可否は実物ではこの 1 文字で書かれる。**

    YAML 1.1 は ``Y`` も ``N`` も真偽値と読むので、素直に書くと検体のほうが
    ``TRUE`` / ``FALSE`` になる ―― :class:`dataset._Loader` が塞いでいるのは
    見出しの ``No``（`設計書.yml`）と同じ罠だが、**値としての ``Y`` / ``N`` は
    どの検体にも無かった**。
    """
    rows = _table(_doc(parsed, f"{DETAIL}/項目編集仕様.md"))
    受注番号 = next(row for row in rows if row[1] == "受注番号")
    assert (受注番号[7], 受注番号[8]) == ("Y", "N")   # TRUE / FALSE ではない


def test_表の真ん中で隠した列も落とさない(parsed) -> None:
    """**非表示列は端にあるとは限らない。**

    旧物理名は移行のために持ち回るが客先へ出す体裁には入れない列で、実物では
    端ではなく**項目名のすぐ隣**に置かれる ―― 落とすと表の格子が 1 列詰まり、
    隣の列の値が旧物理名の位置に並ぶ。値は出したうえで「画面には出ていない」
    と言う（`設計書.yml` の幅 0 とは隠し方が違う）。
    """
    doc = _doc(parsed, f"{DETAIL}/項目編集仕様.md")
    assert "非表示の列 1 列" in _notes(doc)
    rows = _table(doc)
    受注番号 = next(row for row in rows if row[1] == "受注番号")
    assert 受注番号[3] == "JUCHU_NO"                  # 隠れていても落とさない
    assert 受注番号[4] == "CHAR"                      # 格子が詰まっていない


def test_自己ループを同じ箱への線として出す(parsed) -> None:
    """**リトライは元と先が同じ図形を指す。**

    向きだけを見ると「同じ箱から同じ箱へ」という無意味な行に見えるが、資料に
    そう描いてあるのでそのまま出す ―― リトライだと決めるのは整理層である。
    """
    links = _chunk(_doc(parsed, f"{DETAIL}/処理ロジック.md"), "s4-c1").rows
    自己 = [row for row in links if row[0] == row[2]]
    assert len(自己) == 1 and "与信判定" in 自己[0][0]


def test_疑似コードの字下げはセルでも潰さない(parsed) -> None:
    """**表ではないシートでも階層は字下げにしか無い。**

    図形の段落で起きていたのと同じことが、**セルを 1 列だけ使った縦長のシート**
    でも起きる ―― 潰すと入れ子の深さがまるごと平らになる。
    """
    cells = dict(_chunk(_doc(parsed, f"{DETAIL}/処理ロジック.md"), "s4-x1").cells)
    assert cells["A8"] == "2. 与信判定"                    # 親は字下げ無し
    assert cells["A10"].startswith("　2-2.")               # 子は 1 段
    assert cells["A11"].startswith("　　2-2-1.")           # 孫は 2 段


def test_作成アプリが数式のキャッシュ無しの理由になっている(parsed) -> None:
    """**「機械が読めていない」に資料の側の理由があることがある。**

    この 1 冊を書いたのは Excel ではなく LibreOffice で、そのぶん数式の
    キャッシュを持たないまま配られている ―― 申告（``P005`` の作成アプリ）と
    申告（計算結果が保存されていない）が突き合わせられるのは、**1 冊に両方が
    同時に入ったときだけ**である。
    """
    docs, findings = parsed
    プロパティ = [f.render() for f in findings
                  if f.code == "P005" and "SCR001" in f.render()]
    assert プロパティ and "LibreOffice" in プロパティ[0]
    assert "計算結果が保存されていない" in _notes(docs[f"{DETAIL}/性能と試験.md"])


# ── 形式ごとの 1 冊（正解結果との照合） ────────────────────────
def _owner(name: str) -> str:
    """パース結果の道が**どの原本のものか**（どれでもなければ空）。

    1 冊が何本にも割れる形式は原本の名前がディレクトリになり、割れない形式
    （CSV）は `…csv.md` がそのまま 1 本である ―― どちらも「1 冊ぶん」として
    まとめて突き合わせたいので、ここで畳む。
    """
    for source in FROZEN:
        if name.startswith(f"{source}/") or name == f"{source}.md":
            return source
    return ""


def test_形式ごとの1冊は正解のとおり(parsed) -> None:
    """**形式ごとに 1 冊、出来上がりを丸ごと置いてある。**

    ほかの検体は 1 つの観点だけを見る（落ちたテストから直す場所が一意に
    決まる）。それだけだと、**観点の隙間**は誰の目にも触れないまま残る ――
    申告の文が二重になっていても、塊の並びが入れ替わっても、どのテストも
    落ちない。現場の設計書は 1 冊に全部が同時に入っているので、そこだけは
    **人が読める形の答え**を置いて丸ごと突き合わせる。

    **Excel だけに掛けていたあいだ、その番人は Excel にしかいなかった。**
    割り方（シート／節／スライド／しおり）も申告の文も形式ごとに別の実装で、
    隙間もそれぞれの場所にある ―― 1 冊ずつ足したのはそのためである。

    代償は正直に言う ―― 申告の文言を 1 つ直すと期待値がまとめて書き換わる。
    だから**1 形式に 1 冊しか掛けない**（`corpus.py` を移し替えないのと同じ
    理由である）。書き直すときは `ARP4_GOLDEN=write` を渡し、**差分を読んで
    から**コミットすること。

    PDF の 1 冊も同じように掛ける ―― 読み手（`pypdfium2`）は本体依存なので、
    「入っていない環境では外す」という母集合の揺れはもう無い。
    """
    docs, _ = parsed
    frozen = set(FROZEN)

    made = {name: mdio.dump(doc) for name, doc in docs.items()
            if _owner(name) in frozen}

    if _WRITE:                                   # 既定では通らない道である
        for name, body in made.items():
            path = GOLDEN / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(body, encoding="utf-8", newline="\n")

    answers = {name for p in GOLDEN.rglob("*.md")
               if _owner(name := p.relative_to(GOLDEN).as_posix()) in frozen}
    assert answers == set(made), "正解とパース結果でファイルの数が違います"
    for name, body in sorted(made.items()):
        assert (GOLDEN / name).read_text(encoding="utf-8") == body, name


def test_正解に置き忘れた形式が無い() -> None:
    """**丸ごと置くと決めた形式が、置かれないまま通らない。**

    `FROZEN` に足しても正解ファイルを書き出し忘れれば、上の照合は
    「両方とも 0 本」で緑になる ―― **番人が 1 冊も見ていない状態**が
    成功に見える。ここは母集合そのものを見る。
    """
    placed = {_owner(p.relative_to(GOLDEN).as_posix()) for p in GOLDEN.rglob("*.md")}

    assert placed == set(FROZEN), (
        "正解の置いてある原本が `FROZEN` と違います "
        f"（置いてある: {sorted(placed)}）")
    assert len({Path(source).suffix for source in FROZEN}) == len(FROZEN), (
        "同じ形式を 2 冊置いています ―― 維持費だけが倍になります")


def test_設計書は9枚出て非表示の1枚は申告される(parsed) -> None:
    """**綴じられた 1 冊が何枚のファイルになるか**は、それ自体が仕様である。"""
    docs, findings = parsed
    sheets = sorted(Path(name).stem for name in docs if name.startswith(f"{BOOK}/"))
    assert sheets == ["システム構成図", "テーブル定義", "外部IF一覧", "改訂履歴",
                      "機能一覧", "画面レイアウト", "目次", "表紙", "課題管理"]
    hidden = {f.target: f.message for f in findings if f.code == "P003"}
    assert "旧構成図" in hidden[Path(BOOK).name]


def test_群に書かれた代替テキストを落とさない(parsed) -> None:
    """**構成図の 1 文はそこにしか書かれていない。**

    画像と埋め込みオブジェクトからは取るのに、図形（`xdr:sp`）と群
    （`xdr:grpSp`）からは取っていなかった ―― ゾーンごとに 1 文添える書き方は
    構成図でごく普通で、その文は箱の文字にも表にも無い。「現用／待機の 2 系統。
    DR は別紙 5」は**代替テキストにしか存在しない**。
    """
    doc = _doc(parsed, f"{BOOK}/システム構成図.md")
    alts = _chunk(doc, "s4-a1").cells
    assert ("図形の群", "現用／待機の 2 系統で構成する。DR 環境は別紙 5 を参照") in alts
    assert [kind for kind, _ in alts] == ["画像", "図形の群", "図形の群", "図形の群"]


def test_図形の中の字下げを潰さない(parsed) -> None:
    """**機能構成図を 1 つのテキストボックスで書くのは普通の書き方である。**

    親子は段落頭の全角空白にしか無いので、まとめて `strip()` すると 12 個の
    機能が同じ深さで並ぶ ―― セルの値では残すと決めてあった規律
    （`受注ヘッダ` / `　　受注番号`）が、図形の側だけ掛かっていなかった。
    """
    doc = _doc(parsed, f"{BOOK}/機能一覧.md")
    labels = [text for _, text in _chunk(doc, "s5-g1").cells]
    assert labels == ["機能構成\n"
                      "　1. 受注管理\n　　1-1. 受注登録\n　　1-2. 受注修正\n"
                      "　　1-3. 受注取消\n"
                      "　2. 照会\n　　2-1. 受注一覧\n　　2-2. 受注明細照会\n"
                      "　3. 出荷\n　　3-1. 出荷指示作成\n　　3-2. 出荷実績取込\n"
                      "　4. 請求\n　　4-1. 月次締め"]


def test_画像しか無いシートに絵にしろと言わない(parsed) -> None:
    """**案内どおりにやって届かない申告は、申告しないのと同じである。**

    表紙の会社ロゴは貼り付け画像 1 枚で、`arp4 render` で撮り直しても絵が絵の
    まま出るだけ ―― 図形と違い、画像は絵にする以前から絵である。

    **代わりに実体を出す。** 代替テキストが無くても、画像そのものを渡せば
    整理層は開いて読める ―― 「何の画像かも分かりません」で終わらせていたころ、
    そのロゴは**誰の目にも触れないままブックの中に入っていた。**
    """
    doc = _doc(parsed, f"{BOOK}/表紙.md")
    note = _notes(doc)
    assert "`arp4 render` で撮り直しても中身は変わりません" in note
    assert "実体は `s1-i1` に出してあります" in note
    assert "絵にして読んでください" not in note        # 空振りさせる案内
    # **名前は md の中にある。** 出典として指せないと、読んだ内容を整理結果へ
    # 書くときに何を根拠にしたのかが残らない。
    chunk = _chunk(doc, "s1-i1")
    assert [name for name, _ in chunk.cells] == ["表紙-p1.png"]
    assert "![表紙-p1.png](../../../../images/資料/K/" in chunk.text


def test_代替テキストのある画像とない画像を分けて数える(parsed) -> None:
    """**中身が読めないのは同じでも、次にやることが変わる。** 書いてあれば
    何の画像かは分かるが、無ければ画像そのものを開くしかない。
    """
    doc = _doc(parsed, f"{BOOK}/画面レイアウト.md")
    note = _notes(doc)
    assert "貼り付け画像 2 枚は絵のままです" in note
    assert "うち 1 枚には代替テキストがあるので" in note
    # **2 枚とも実体は出ている。** 代替テキストの有無で読めるかどうかは変わらない
    # ―― 変わるのは「開く前に何の画像か分かるか」だけである。
    assert [name for name, _ in _chunk(doc, "s7-i1").cells] == [
        "画面レイアウト-p1.png", "画面レイアウト-p2.png"]


def test_構成図の接続は10本取れて2本は取れないと言う(parsed) -> None:
    """**入れ子の群に描かれた構成図でも、接続は id の転記で取れる。**

    ゾーンの囲み枠・見出し帯・機器の箱が 3 段の群になっていても、線が持って
    いるのは相手の id なので、群を跨いだ接続もそのまま取れる ―― 取れないのは
    2 本だけで、**その 2 本は理由が違う**（片方は絵にすれば読める）。
    """
    doc = _doc(parsed, f"{BOOK}/システム構成図.md")
    assert "図形 18 個・接続子 12 本・画像 1 枚" in _notes(doc)
    向き = {(元, 先): 矢 for 元, 矢, 先, *_線 in _chunk(doc, "s4-c1").rows[1:]}
    assert 向き[("AP サーバ #1\nTomcat 10 / Java 21",
                "AP サーバ #2\nTomcat 10 / Java 21")] == "―"      # 冗長構成の対
    assert 向き[("バッチサーバ\n日次 02:00 起動",
                "DB サーバ（現用）\nPostgreSQL 16")] == "→"
    assert len(向き) == 10
    assert "相手の図形が文字を持ちません" in _notes(doc)          # ゾーンの囲み枠
    assert "どこにも繋がっていません" in _notes(doc)              # 目分量の線


def test_高さ0で潰した行は再表示では戻らないと言う(parsed) -> None:
    """**隠すフラグだけを見ていたぶん、潰れた行を「見えている行」として出していた。**

    3.2 版のあとに入った 3.3 版の改訂は、行を消さずに高さ 0 で潰してある ――
    `hidden` は立たないので非表示行として数えられず、**申告にも上がらなかった**
    （`未読取` を宣言する先が無い）。非表示行の裏返しの壊れ方で、こちらのほうが
    静かである。

    **戻し方が違うので文句を分ける。** 「再表示」で戻る非表示行と違い、潰れた
    行はメニューに出てこない ―― `veryHidden` のときと同じ理由である。
    """
    doc = _doc(parsed, f"{BOOK}/改訂履歴.md")
    note = _notes(doc)
    assert "非表示の行 1 行にある 4 セルも読み込んでいます" in note
    assert "うち 1 行は高さ 0 で潰されています" in note
    assert "「再表示」では戻りません" in note
    # 値は落とさない ―― 表紙も目次もフッタも 3.2 版のままで、この行だけが知っている
    assert ["3.3", "2026-07-03", "7 節", "IF-05 の周期を月次から日次へ", ""] \
        in _table(doc)


def test_幅0で潰した列も数えるが文句は分けない(parsed) -> None:
    """**客先に出さない作業用の列を、列ごと消さずに潰す**のはよくある形である。

    数えていなかったぶん、画面に出ていない値が「見えている列」として表に
    混ざっていた ―― そこは行と同じである。**違うのは戻し方で**、幅 0 の列は
    「再表示」で既定幅に戻る（実測）。戻るものに「戻りません」と言わないよう、
    行に付ける案内はここには付けない。
    """
    doc = _doc(parsed, f"{BOOK}/機能一覧.md")
    note = _notes(doc)
    assert "非表示の列 1 列にある 9 セルも読み込んでいます" in note
    assert "「再表示」では戻りません" not in note
    assert "見積番号（社内）" in _table(doc)[0]        # 落とさずに出してある


def test_紙にしか出ないものを別のアンカーに出す(parsed) -> None:
    """**綴じて配る文書なので、文書番号も版も機密区分もフッタにある。**

    どのページにも刷られているのにセルには 1 つも書かれていない ―― 表だけを
    読むと、その 1 冊が何の文書なのかが落ちる。`&P` は刷るときに決まる値なので
    埋めない（埋めれば資料に無い数を書くことになる）。
    """
    doc = _doc(parsed, f"{BOOK}/テーブル定義.md")
    setup = dict(_chunk(doc, "s6-p1").cells)
    assert setup["フッタ左"] == "TLG-OMS-BD-014"
    assert setup["フッタ右"] == "&P / &N"
    assert setup["ヘッダ右"] == "社外秘（取扱注意）"
    # **どこまでが見出しかは人が決めている。** 機械は当てない（render の
    # --title-rows を人に聞くのと同じ）が、資料にそう書いてあるなら取る
    assert setup["印刷タイトル行"] == "$1:$3"
    assert "資料の作成者が「ここまでが見出し」と決めた範囲です" in _notes(doc)


def test_フッタの版が本文と食い違っていても機械は言わない(parsed) -> None:
    """**1 冊の中で版が食い違うのは実物どおりである。**

    ページ設定はシートごとなので、改訂のたびに全シートを直し損ねる ―― どれが
    正しいかを当てるのは意味の判断なので、機械は並べるだけにする。
    """
    old = dict(_chunk(_doc(parsed, f"{BOOK}/機能一覧.md"), "s5-p1").cells)
    now = dict(_chunk(_doc(parsed, f"{BOOK}/課題管理.md"), "s9-p1").cells)
    assert "第 3.1 版" in old["フッタ中"]           # 直し忘れたシート
    assert "第 3.2 版" in now["フッタ中"]


def test_誰がいつ触ったかはプロパティにしか残らない(parsed) -> None:
    """**改訂履歴は人が書いた申告**で、書き忘れれば何も残らない。

    プロパティは Excel が黙って書くので、そこに無い更新がここにだけ残る。
    最終更新者（川瀬）は表紙の承認欄にも他の改訂行にも出てこず、日時は
    3.2 版の改訂日より後である ―― 突き合わせるのは整理層の仕事なので、
    機械は並べるだけにする。
    """
    _, findings = parsed
    said = {f.target: f.message for f in findings if f.code == "P005"}
    message = said[Path(BOOK).name]
    assert "最終更新者 川瀬 直樹" in message
    assert "最終更新日時 2026-07-03T11:48:00Z" in message
    assert "作成者 郡司 亮太" in message
    assert "機械は判断していません" in message
    assert all(f.level == "warn" for f in findings if f.code == "P005")


def test_入力規則の候補は現実の1冊でも取らない(parsed) -> None:
    """**取らないと決めたものは、実物に入っていても取らない。**

    区分の `保守`・状態の `取り下げ` はどの行にも選ばれておらず、入力規則に
    しか無い ―― 取れば整理層は楽になるが、「クリックすると出る」を認めると
    条件付き書式・数式の分岐まで同じ理屈で入ってきて、**どこまでが資料に
    書いてあることなのかを機械が決める**ことになる。

    検体の側には入れてある。入っていなければ、この線引きが実物で何を落として
    いるかを誰も確かめられない（→ `docs/parsed.md`）。
    """
    for sheet in ("機能一覧", "課題管理"):
        body = mdio.dump(_doc(parsed, f"{BOOK}/{sheet}.md"))
        assert "保守" not in body and "取り下げ" not in body
    # **取らないものは、取れていないとも言わない**（申告の山に埋もれさせない）
    assert "入力規則" not in mdio.dump(_doc(parsed, f"{BOOK}/課題管理.md"))


# ── 書き出しと読み戻し ──────────────────────────────────────────
def test_書き出して読み戻してもアンカーが増えない(tmp_path: Path, parsed) -> None:
    """**書いて読み戻すところまでやらないと壊れているかは分からない。**

    セルの値がアンカーを偽造できないことは :mod:`test_parse_corpus` が見て
    いるが、こちらは**検体の全ファイル**で数が合うことを見る（図形の接続・
    グラフの参照範囲・Markdown の見出しなど、資料由来の文字列が入るところは
    増え続けるので、**個別に塞ぐのではなく全件で数を合わせる**）。
    """
    docs, _ = parsed
    for name, doc in docs.items():
        path = mdio.write(tmp_path / Path(name).name, doc)
        assert [a.id for a in mdio.read(path).anchors] == \
            [c.anchor for c in doc.chunks], name


# ── 性能 ────────────────────────────────────────────────────────
def test_塊が数千に散っても実用的な時間で終わる() -> None:
    """**印が飛び飛びに散る CRUD マトリクス**は 1 枚で数千の塊になる。

    塊の起点を毎回 ``min(remaining)`` で探していたので、残りセル全部を走る
    ―― 塊の数 × セルの数になり、5,000 セルのシート 1 枚に 0.30 秒かかって
    いた。30 冊に数枚あれば、それだけでパース全体より長くなる。
    """
    cells = {}
    for row in range(1, 401):
        cells[(row, 1)] = f"F{row:03d}"
        for column in range(4, 250, 3):
            if (row + column) % 7 == 0:
                cells[(row, column)] = "○"

    start = time.perf_counter()
    regions = parse._regions(cells)
    assert time.perf_counter() - start < 0.15    # 直す前は 0.30 秒だった
    assert len(regions) > 3000                   # 塊はちゃんと散っている
    assert sum(len(r) for r in regions) == len(cells)   # 1 セルも落ちていない


def test_枠だけ大きい塊に格子ぶんの時間を払わない() -> None:
    """**いちばん捨てる塊にいちばん払っていた。**

    工程表を年単位に伸ばすと、斜めに並んだ `■` が空白 1 マスずつで繋がって
    1 つの塊になる ―― 枠は 5,000 行 × 5,000 列、中身は 5,000 セルである。
    `_grid` がいきなり格子を組み、そのあとで「すかすかだ」と捨てていたので、
    **判定に届く前に 2,500 万マスぶんのメモリと時間を使っていた**
    （実測 12.3 秒・211MB）。出来上がりは箇条書き 5,000 行である。

    枠が大きいほど中身が薄いのだから、**枠を先に決めて、表として出すと
    決めたときにだけ格子を組む**。出来上がりは 1 セルも変わらない。
    """
    cells = {(i, i): "■" for i in range(1, 5001)}

    start = time.perf_counter()
    regions = parse._regions(cells)
    frame = parse._frame(cells, regions[0])
    elapsed = time.perf_counter() - start

    assert elapsed < 2.0                         # 直す前は 12.3 秒だった
    assert frame.sparse                          # 箇条書きへ回る側である
    assert (frame.height, frame.width) == (5000, 5000)
    assert len(frame.addresses) == 5000          # 1 セルも落ちていない
    assert frame.addresses[0] == ("A1", "■")


# ── 異常系（中身を読む前に壊れるもの） ──────────────────────────
def test_開けない理由は先頭のバイトに書いてある(parsed) -> None:
    """**「読めません」で終わる申告は、資料が 1 冊落ちたまま拾い直されない。**

    4 冊とも例外は「zip じゃない」か「部品が無い」しか言わないが、**次にやる
    ことは 4 つとも違う** ―― 保護は外して保存し直す、PDF は名前を直して
    渡し直す、.ods は Excel で保存し直す、切れた添付は取り出し直す。
    先頭の数バイトに何であるかは書いてあるので、読むのは転記である。
    """
    _, findings = parsed
    said = {f.target: f.message for f in findings if f.code == "P010"}
    assert len(said) == 5                              # 4 冊＋ソース 1 本

    assert "パスワードで保護" in said["受注実績（第2.0版）.xlsx"]
    # **PDF はもう「よそへ回す」相手ではない。** 名前が違うだけで、直せば読める。
    assert "拡張子を .pdf に直して" in said["画面遷移図.xlsx"]
    assert ".ods" in said["様式集.xlsx"]
    assert "添付" in said["添付_破損.xlsx"]
    # **取り違えないことのほうが大事である。** PDF に「保護を外して」と言うと、
    # 言われたとおりにやって届かないところで止まる。
    assert "パスワード" not in said["画面遷移図.xlsx"]


def test_機械が置いたものは申告の山にしない(parsed) -> None:
    """**客先からもらったフォルダをそのまま置くと `.git` が付いてくる。**

    1 つずつ `P001` と言うと、本当に読めなかった 1 冊がその山に埋もれる ――
    申告が読み飛ばされるのは、申告しないのと同じである。**資料でないものだけを
    切る**のであって、読めないものを黙るのではない。
    """
    docs, findings = parsed
    unreadable = {f.target for f in findings if f.code == "P001"}
    assert unreadable == {"一式.zip"}
    assert not [name for name in docs if ".git" in name or "node_modules" in name]


def test_見出しの無い覚書も塊になる(parsed) -> None:
    """**`.txt` は長らく「申告するだけで読まない」側にいた。**

    見出しも表も無いので塊は 1 個しか出ないが、**アンカーが 0 個だと `freeze` の
    未整理一覧に上がらない** ―― 読めていないものではなく読んでいないものが、
    誰にも数えられないまま消える。1 個でも出れば整理か対象外宣言かを迫れる。
    """
    doc = _doc(parsed, "受領/受注システム/docs/受注一覧.txt.md")
    assert [c.anchor for c in doc.chunks] == ["h1"]
    assert "2026-04 の改訂で 3 つ増えた" in doc.chunks[0].text
    assert doc.chunks[0].at.startswith("受領/受注システム/docs/受注一覧.txt#L1")


def test_キーワード専用引数と既定値がシグネチャに出る(parsed) -> None:
    """**書いてある数と違うものを書いていた。**

    `ast.arguments` は引数を 4 つの入れ物に分けて持つのに `args` しか読んで
    いなかったので、`*` より後ろがまるごと消えていた ―― 読み落としなら申告
    できるが、これは**その表を見て書いた呼び出しが通らない**形である。

    `/` の位置は**残った**位置専用引数で数える。`self` を落としたぶんを
    数え直さないと区切りが 1 つ右へずれ、普通の引数まで位置専用に見える。
    """
    doc = _doc(parsed, "受領/受注システム/src/order/検索条件.py.md")
    rows = {r[0]: r for r in _table(doc)[1:]}
    assert rows["build"][3] == (
        "build(keyword, /, limit: int = 50, *columns: str, "
        "order: str = '受注日', descending: bool = False, **options: object)")
    assert rows["parse_period"][3] == "parse_period(text: str)"
    assert rows["build"][5] == "ValueError"


HEADER = "受領/受注システム/src/order/受注ヘッダ.py.md"


def test_デコレータを出す(parsed) -> None:
    """**`tier` を決める最大の手がかりが出ていなかった。**

    `@dataclass` が付いているかどうかは、そのクラスが値の入れ物なのか処理を持つ
    層なのかを分ける ―― 出していなかったあいだ、57 モジュールの層分けを整理層が
    **全部推測で**付けていた。`@property` も同じで、呼び出しではなく値に見える
    メンバかどうかが変わる。
    """
    rows = {r[0]: r for r in _table(_doc(parsed, HEADER), "m1")[1:]}

    assert rows["OrderHeader"][2] == "@dataclass(frozen=True)"
    assert rows["line_count"][2] == "@property"
    assert rows["parse"][2] == "@staticmethod"
    # 種類は**構文木の位置だけ**で決める（`@property` を呼び替えるのは解釈）
    assert rows["line_count"][1] == "メソッド"


def test_クラス直下の属性を出す(parsed) -> None:
    """**`@dataclass` だと分かっても、何を持つ入れ物なのかが書いていなかった。**

    Java の `private String orderNo` にあたるものが Python 側だけ落ちていた形で、
    データ項目を起こす根拠がここにしか無い。
    """
    rows = _table(_doc(parsed, HEADER), "m1")[1:]

    assert [r[0] for r in rows if r[1] == "フィールド"] == [
        "order_no", "customer_cd", "order_date", "lines", "MAX_LINES"]
    assert rows[4][3] == "lines: list[str] = field(default_factory=list)"


def test_継承は書いてあるとおりに出す(parsed) -> None:
    """層を分けるもう 1 つの手がかりが継承である（注釈が無い現場では唯一）。"""
    doc = _doc(parsed, HEADER)
    assert _table(doc, "m2")[1][3] == "class OrderHeaderValidator(BaseValidator)"
    assert _table(doc, "m1")[1][3] == "class OrderHeader"   # 継承が無ければ括弧も無い


def test_取り込みを出す(parsed) -> None:
    """**`import` は構文木からゼロ曖昧に取れる。**

    出していなかったあいだ、呼出関係は整理層が原本を読んで手で起こしていた ――
    「構文木から取れるものに LLM を使っても精度は上がらず、コストと見落としだけが
    増える」と決めた当のものである（決定 2）。

    **関数の中の取り込みも拾う**（重い依存を遅らせる書き方は普通にある）。
    どれが自分たちのモジュールかは決めない ―― 点の数も書いてあるとおりに出す。

    **1 行 = 1 名前。** 名前を「元」に畳んでいると、`from a import x, y` が
    1 升になって**依存の本数が数えられない**（→ :func:`test_取り込みは名前ごとに割る`）。
    """
    doc = _doc(parsed, "受領/受注システム/src/order/受注登録.py.md")
    rows = _table(doc, "i1")
    assert rows[0] == ["取り込み", "元", "名前", "行"]
    assert ["from . import 採番", ".", "採番", "4"] in rows
    assert ["from ..common.log import 監査ログ", "..common.log", "監査ログ", "5"] in rows
    # メソッドの中でしか読まない依存 ―― ここが落ちると設計書から丸ごと消える
    assert ["from arp4.order.在庫 import 引当", "arp4.order.在庫", "引当", "13"] in rows


def test_取り込みは名前ごとに割る() -> None:
    """**1 文に名前が 2 つ書いてあるなら、升も 2 つである。**

    `from arp4 import mdio, yamlio` を 1 行で出していたあいだ、「元」に書けるのは
    `arp4` だけで、**2 本の依存が 1 升に畳まれていた**。整理層はそこから `calls`
    を 2 本起こすことになり、実測で `arp4.parse → arp4.yamlio` が落ちて 92 本の
    うち 91 本しか出ていなかった ―― しかも升と関係の本数が対応していないので、
    **機械には落ちたことすら言えなかった**。

    畳まれたものを展開するだけで、**どこを指すかは解かない** ―― `import a.b` は
    モジュールそのものを束ねるので「名前」は空のままにする。
    """
    tree = ast.parse("import a.b as c\nfrom arp4 import mdio, yamlio\n"
                     "from . import x\n")
    rows = parse._imports(tree)

    assert rows[0] == ["取り込み", "元", "名前", "行"]
    assert rows[1:] == [
        ["import a.b as c", "a.b", "", "1"],
        ["from arp4 import mdio, yamlio", "arp4", "mdio", "2"],
        ["from arp4 import mdio, yamlio", "arp4", "yamlio", "2"],
        ["from . import x", ".", "x", "3"],
    ]


def test_テストは骨格ではなくテストとして出る(parsed) -> None:
    """**テストは 1 本も設計書に出ていなかった。**

    骨格は取れていたが「モジュール」として詳細設計書へ流れるだけで、
    トレーサビリティは「要件はすべてテスト漏れ」と出す ―― **事実に反する表**を
    出すほうが、空の表より悪い。

    集める規則は pytest のものをそのまま写す。**fixture と補助関数は混ぜない**
    ―― 混ぜると整理層がテストケースを数えられない。
    """
    doc = _doc(parsed, "受領/受注システム/tests/test_受注サービス.py.md")
    cases = _table(doc, "t1")
    assert all(r[1] == "テスト" for r in cases[1:])
    assert [r[0] for r in cases[1:]] == [
        "test_受注番号は採番テーブルから取る", "test_与信NGは差戻しになる"]

    # fixture と補助関数は**テストではない**ので、別の塊に残る
    helpers = _table(doc, "m2")
    assert [r[0] for r in helpers[1:]] == ["service", "_注文"]
    assert helpers[1][2] == "@pytest.fixture"      # fixture はここで見分けが付く
    assert _chunk(doc, "m1").heading == "テストクラス: TestCancel"


def test_1本ずつの行は列で持ち塊は分けない(parsed) -> None:
    """**出典の精度と、整理に迫られる件数はぶつかる。**

    1 関数 1 塊にすると出典は正確になるが、`freeze` は塊 1 個ずつに整理か対象外
    宣言かを迫るので、補助関数 30 本のモジュールが未整理 30 件になる ―― 通すために
    宣言を量産させると、**宣言そのものが読まれなくなる**。塊は 1 個のまま、
    行を列で持たせて原本を 1 行ずつ読み直せるようにする。
    """
    doc = _doc(parsed, "受領/受注システム/src/order/検索条件.py.md")
    rows = _table(doc, "m1")
    assert rows[0][-1] == "行"
    assert [r[-1] for r in rows[1:]] == ["4", "7", "14"]
    # 塊の `at` は**その塊が覆う範囲**（ファイル名だけで終わらせない）
    assert _chunk(doc, "m1").at.endswith("#L4-L16")


# ── Java ────────────────────────────────────────────────────────
JAVA = "受領/受注システム/src/main/java/jp/co/example/order/"


def test_注釈は宣言と別の列に出す(parsed) -> None:
    """**Java は種別を決める手がかりが本体ではなく注釈に載っている。**

    `@Entity` はエンティティ、`@Column` はデータ項目、`@RestController` は外部
    インターフェース ―― 本体と混ぜて 1 つの文字列にすると、整理層が正規表現を
    書くことになる。引数の中身まで残す（物理名の根拠がそこにしか無い）。
    """
    doc = _doc(parsed, JAVA + "OrderEntity.java.md")
    rows = {r[0]: r for r in _table(doc, "j1")[1:]}

    assert rows["OrderEntity"][2] == '@Entity @Table(name = "T_ORDER")'
    assert rows["orderNo"][2] == (
        '@Id @Column(name = "ORDER_NO", length = 10, nullable = false)')
    # javadoc は宣言の欄に入れない（意図の層は出さない ―― 決定 2）
    assert rows["getOrderNo"][3] == "public String getOrderNo()"


def test_メソッドの中の文はメンバにしない(parsed) -> None:
    """**`{` を数えるだけだと、本体の中の文がメンバに見える。**

    `int matched = 0;` がフィールドとして、`for (...)` がメソッドとして出て
    いた。深さでは決まらないので、波括弧 1 つずつに**それが型の本体か**を
    覚えさせ、型の本体だけを通ってきた宣言をメンバとする。
    """
    doc = _doc(parsed, JAVA + "OrderService.java.md")
    names = [r[0] for r in _table(doc, "j1")[1:]]

    assert names == ["OrderService", "repository", "register",
                     "Limit", "MAX_LINES", "max"]      # 入れ子の型も落とさない
    assert "matched" not in names and "for" not in names


def test_文字列とコメントの中の閉じ括弧で型がずれない(parsed) -> None:
    """**1 つ混ざると、そこから先の型が全部ずれる。**

    `"}"` という文字列も `// }` というコメントも実物には普通にある ――
    ずれるのは読み落としではなく作り替えなので、申告のしようがない。
    """
    doc = _doc(parsed, JAVA + "OrderService.java.md")
    rows = {r[0]: r for r in _table(doc, "j1")[1:]}

    assert rows["register"][3] == \
        "public String register(Order order, String staffCd)"
    assert rows["Limit"][1] == "クラス"                 # ずれると出てこない
    assert rows["MAX_LINES"][3] == "public static final int MAX_LINES = 30"


def test_注釈型は注釈ではなく型として出す(parsed) -> None:
    """**一緒に落とすと、自前で宣言した注釈が型として 1 つも出ない。**

    Spring を持ち込めない現場（外部依存を持てない配布物）では、注釈を自前で
    宣言するのは普通である ―― `examples/sales-corpus` の Java もそうしている。
    """
    from arp4 import parse as parse_module

    assert parse_module._java_kind("public @interface Service") == "注釈型"
    assert parse_module._java_kind("public interface OrderRepository") \
        == "インタフェース"
    assert parse_module._java_split("@Target(ElementType.TYPE) public @interface S")[0] \
        == "@Target(ElementType.TYPE)"


# ── DDL ─────────────────────────────────────────────────────────
DDL = "受領/受注システム/db/受注管理.sql.md"


def test_区切りに見える記号で表が割れない(parsed) -> None:
    """**素朴に `;` で切ると 1 つのテーブルが 2 つになって出る。**

    この検体は `;` を、切ってはいけないところに 3 か所置いてある ―― 行コメント・
    ブロックコメント・`COMMENT ON` の文字列リテラル。どれも実物では普通の書き方で、
    割れるのは読み落としではなく**作り替え**なので、申告のしようがない。
    """
    doc = _doc(parsed, DDL)
    assert [c.heading for c in doc.chunks if c.anchor.startswith("t")] == [
        "テーブル: T_ORDER", "テーブル: M_CUSTOMER"]
    comments = {r[1]: r[2] for r in _table(doc, "c1")[1:]}
    assert comments["T_ORDER.ORDER_NO"] == "受注番号; 採番のみ、手入力は不可"


def test_型は書いてあるものをそのまま出す(parsed) -> None:
    """**`NUMBER(11, 2)` を「数値」へ寄せない。**

    メタモデルの enum に当てはめるのは意味の判断で、方言ごとの型
    （`NUMBER` / `NUMERIC` / `DECIMAL`）が同じものかは、資料と実装を見た人にしか
    決められない。桁と小数を分けるのも同じ理由で整理層に渡す。
    """
    rows = {r[0]: r for r in _table(_doc(parsed, DDL), "t1")[1:]}
    assert rows["TOTAL"][1] == "NUMBER(11, 2)"
    assert rows["ORDER_DATE"][2] == "SYSDATE"        # 既定値は分けて出す
    assert rows["CREDIT_KBN"][2] == "'0'"
    assert rows["TOTAL"][3] == "NOT NULL"


def test_表に掛かる制約は列と別のアンカーにする(parsed) -> None:
    """**主キーと外部キーはテーブル間の関係の根拠**である。

    列 1 本の話と混ぜると、「どの事実を出典にしたいか」が選べなくなる。
    """
    rows = [r[0] for r in _table(_doc(parsed, DDL), "k1")[1:]]
    assert rows[0] == "CONSTRAINT PK_T_ORDER PRIMARY KEY (ORDER_NO)"
    assert "REFERENCES M_CUSTOMER (CUST_CD)" in rows[1]
    # 囲いは区切りであって名前ではない（`"M_CUSTOMER"` の引用符は外す）
    assert _chunk(_doc(parsed, DDL), "t2").heading == "テーブル: M_CUSTOMER"


def test_読まなかったDDLの文を数えて申告する(parsed) -> None:
    """**ビューとストアドの中に仕様が入っていることがある。**

    黙って落とすと「資料に無い」と「機械が読めていない」が混ざる ―― 方言を
    全部読みにいくのは決定 1 が捨てた側なので、読まないことは決めてよい。
    決めてはいけないのは、読まなかったことを言わないほうである。
    """
    said = _notes(_doc(parsed, DDL))
    assert "読まなかった文が 1 本あります" in said
    assert "V_ORDER" in said


def test_索引は一意かどうかまで取る(parsed) -> None:
    """索引が無いと、性能設計のレビューがテーブル定義書を離れて別ファイルへ散る。"""
    rows = {r[0]: r for r in _table(_doc(parsed, DDL), "x1")[1:]}
    assert rows["IX_T_ORDER_01"][2:4] == ["CUST_CD, ORDER_DATE", "UNIQUE"]
    assert rows["IX_T_ORDER_02"][3] == ""


# ── Markdown の資料 ─────────────────────────────────────────────
MEMO = "受領/受注システム/docs/受注管理の設計メモ.md.md"


def test_コードブロックの中の記号は見出しにならない(parsed) -> None:
    """**手順書はシェルの例を必ず載せる。**

    行頭の `# コメント` を見出しに取ると、塊が本文の途中で切れる ―― `at` の
    行範囲は本文とずれ、**「読めた」と言いながら中身が入れ替わる**。読み落とし
    より悪い（申告のほうが嘘になる）。
    """
    doc = _doc(parsed, MEMO)
    assert [c.heading for c in doc.chunks] == [
        "（前書き）", "受注登録", "取り込みの手順 <!-- a:s9-t9 -->", "取消"]
    fence = _chunk(doc, "h3")
    assert "この行は見出しではない" in fence.text
    assert fence.at.endswith("#L14-L20")          # フェンスごと 1 塊に入っている


def test_見出しの前の本文を落とさない(parsed) -> None:
    """**落とすと `freeze` の未整理一覧に上がらないまま消える。**

    改訂日・適用範囲は見出しの前に書かれるのが普通で、そこがいちばん効く。
    """
    assert "改訂は 2026-04" in _chunk(_doc(parsed, MEMO), "h1").text


def test_入れ子の箇条書きと表がそのまま残る(parsed) -> None:
    """**表に組み直すと階層がそこで消える。** Markdown の資料は原本が既に
    読める形なので、寄せ直すところが 1 つも無い ―― 畳むのは損しかしない。"""
    body = _chunk(_doc(parsed, MEMO), "h2").text
    assert "  - 採番の単位は年度" in body            # 字下げが残っている
    assert "| 受注番号 | ○ | 10 |" in body
    assert "（`固定: 130010` ではない）" in body


def test_見出しはアンカーを偽造できない(tmp_path: Path, parsed) -> None:
    """**セルだけ守っても穴は塞がらない。**

    Markdown の資料は**見出しの行がそのまま資料の文字**である。`## 取り込みの
    手順 <!-- a:s9-t9 -->` をそのまま出すと、読み戻し側は先に出てくる偽アンカーを
    拾い、**本物の塊（`h3`）が丸ごと `s9-t9` の中へ消える。**
    """
    doc = _doc(parsed, MEMO)
    back = mdio.read(mdio.write(tmp_path / "memo.md", doc))
    assert [a.id for a in back.anchors] == ["h1", "h2", "h3", "h4"]
    assert "s9-t9" not in {a.id for a in back.anchors}
    # **画面に見えている表記は変えない**（実体参照にするので `<!--` と表示される）
    assert "&lt;!-- a:s9-t9 --&gt;" in back.by_id["h3"].body


def test_資料である読めない形式には次の一手を書く(parsed) -> None:
    """**`.zip` は資料そのものである。**

    展開し忘れると**中の 30 冊がまるごと 1 行の申告になる** ―― 「読めません」で
    終わる申告は、その 30 冊が誰にも拾い直されないまま終わる。
    """
    _, findings = parsed
    said = {f.target: f.message for f in findings if f.code == "P001"}

    assert "展開してから" in said["一式.zip"]
    # **`.csv` はもうこの山にいない。** 読むと決めた以上、`P001` に出てきては
    # ならない（出ていたら、読めるものを「読めない」と言っている）。
    assert not [name for name in said if name.endswith((".csv", ".tsv"))]


def test_csvは当てた区切りと文字コードを必ず書く(parsed) -> None:
    """**当てるのをやめたのではなく、当てた結果を黙らないことにした。**

    区切りと文字コードは資料ごとに違い、当てにいけば値が変わる ―― だから
    決めた結果をパース結果の頭に書く。整理層はそれを見て、値を信じるか原本を
    開くかを決められる。**書いていない当て推量は、当て推量だと分からない。**
    """
    doc = _doc(parsed, "資料/N/得意先マスタ移行.csv.md")
    said = " ".join(doc.notes)

    assert "cp932" in said and "カンマ" in said
    assert "100% が 4 列" in said
    assert "機械が決めたもの" in said


def test_csvの引用符の中の区切りと改行を守る(parsed) -> None:
    """**素朴に `,` で割ると、住所の 1 行が丸ごとずれる。**

    `東京都千代田区1-2-3, 丸の内ビル` にはカンマが、備考には改行が入っている
    ―― どちらも実物の移行データにごく普通にある。`csv` 標準ライブラリに渡すのは
    そのためで、自前で `split(",")` すると**列が 1 本増えた表が「読めた」顔で出る。**
    """
    table = _table(_doc(parsed, "資料/N/得意先マスタ移行.csv.md"))

    assert [len(row) for row in table] == [4, 4, 4, 4]
    assert table[1][2] == "東京都千代田区1-2-3, 丸の内ビル"
    assert "取引停止" in table[2][3] and "与信枠は 0" in table[2][3]


def test_tsvは拡張子が名乗る区切りを当てにいかない(parsed) -> None:
    """**資料が名乗っているものを機械が疑う理由は 1 つも無い。**

    この 1 本は説明文にカンマが入っているので、当てにいくと `,` を選んで列が
    増える ―― 区分値は正本の `enum` の元になるので、1 文字ずれると仕様が変わる。
    """
    doc = _doc(parsed, "資料/N/区分値一覧.tsv.md")
    table = _table(doc)

    assert "タブ" in " ".join(doc.notes)
    assert [row[0] for row in table] == ["区分コード", "01", "02", "03"]
    assert table[1][2].startswith("受注を受け付けた状態。")


def test_区切りは出現数ではなく列の揃い方で決める(parsed) -> None:
    """**いちばん多く出てくる文字が区切りとは限らない。**

    帳票ツールが欧州ロケールで吐いた 1 本で、区切りは `;` なのに**本文には
    カンマのほうが多い**（`3,200 件`・`01:00, 02:00`）。出現数で決めると `,` を
    選び、列数がばらばらの表が出る ―― 決め手は行ごとの列数が揃うかである。
    """
    doc = _doc(parsed, "資料/N/バッチ実行結果.csv.md")
    table = _table(doc)

    assert "セミコロン" in " ".join(doc.notes)
    assert [len(row) for row in table] == [4, 4, 4, 4]
    assert table[1][3] == "受注データを 3,200 件 取り込みました"


def test_どの区切りでも揃わないなら表にしない(parsed) -> None:
    """**1 列の CSV は、どの区切りでも「揃って」見える。**

    運用手順を Excel の 1 列に書いて CSV に落とすとこうなる。そこで先頭の候補を
    選ぶのは、揃ったから選んだのではなく**偶然に賭けている**だけである ――
    表にせず原文のまま出す。値は 1 つも落ちていない。
    """
    doc = _doc(parsed, "資料/N/夜間停止手順.csv.md")

    assert [c.anchor for c in doc.chunks] == ["x1"]
    assert "バッチスケジューラを停止する" in doc.chunks[0].text
    assert "区切りは決められません" in " ".join(doc.notes)


def test_列が揃っていない行があることを言う(parsed) -> None:
    """**表は幅を揃えて出すので、黙ると「資料が空欄」に見える。**

    末尾に注記の 1 行（`※ 桁あふれは切り捨て`）が 1 列だけで残るのは実物では
    普通のことで、そこで表にしないと**揃っている残りの行まで道連れになる**。
    表にはするが、揃っていなかったことは必ず言う。
    """
    doc = _doc(parsed, "資料/N/項目対応表.csv.md")

    assert "80% が 4 列" in " ".join(doc.notes)
    assert "足りない列は空欄に見えます" in " ".join(doc.notes)
    # **値としては 1 列しか無い**（機械は列を作らない）。
    assert _table(doc)[-1] == ["※ 桁あふれは切り捨て"]
    # **書き出すと 4 列に見える**（`mdio` が幅を揃える）。申告が要るのはここ。
    assert "| ※ 桁あふれは切り捨て |  |  |  |" in mdio.dump(doc)


def test_例外の出ない文字化けを申告する(parsed) -> None:
    """**EUC-JP のかなは、すべて cp932 の半角カタカナの範囲に収まる。**

    つまり `UnicodeDecodeError` が出ない ―― 「cp932 で読めました」と言いながら
    `うけつけ` が `､ｦ､ｱ､ﾄ､ｱ` として表に入り、整理層はそれを資料の字として読む。
    `�` すら出ないので、読めた中身のほうを見るしか気づく手が無い。
    """
    doc = _doc(parsed, "資料/N/かな索引.csv.md")
    said = " ".join(doc.notes)

    assert "半角カタカナばかり" in said
    assert "資料の字ではありません" in said
    # **数えた事実だけを言う。**「EUC-JP である」と決めつけない（決めるのは人）。
    assert "EUC-JP で保存された資料を cp932 として読むとこの形になります" in said


MINUTES = "資料/Q/受注管理システム移行判定会議_議事録.pdf"
SCANNED = "資料/Q/受注管理システム受入確認書（押印済）.pdf"


def test_PDFはしおりで節に割れる(parsed) -> None:
    """**しおりは資料が持っている構造である。**

    1 冊 1 本のまま出すと、200 ページの検収仕様書が 1 つのアンカーになり、
    整理層はどの章の話かを言えない。最初のしおりより前（表紙・改訂履歴）を
    捨てないのは、捨てるとアンカーの無いページができるからである。
    """
    docs, _ = parsed
    made = sorted(name[len(ACCEPT) + 1:] for name in docs if name.startswith(ACCEPT))

    assert made == ["01_（前書き）.md", "02_1 適用範囲.md",
                    "03_2 確認項目.md", "04_3 検収の合否.md"]
    # 表紙は前書きに入っている（落ちていない）
    assert "株式会社あかつき商事" in _chunk(
        docs[f"{ACCEPT}/01_（前書き）.md"], "p1-x1").text


def test_PDFのアンカーはページ番号で振る(parsed) -> None:
    """**PDF には番地が無い。** 唯一そこにあるのはページ番号なので、それで振る。

    節の中に何ページ入っていても、出典は 1 ページを名指しできる。
    """
    doc = _doc(parsed, f"{ACCEPT}/01_（前書き）.md")

    assert [c.anchor for c in doc.chunks] == ["p1-x1", "p2-x1"]
    assert [c.at for c in doc.chunks] == ["p.1", "p.2"]


def test_PDFの表は組み直さない(parsed) -> None:
    """**PDF が持っているのは位置を持った文字だけ**で、列の切れ目は無い。

    文字の隙間から当てにいくと、閾値の外れたところで**列がずれた表が
    「読めた」顔で出る** ―― CSV の区切りを機械に当てさせないと決めたのと
    同じ理由である。行のまま出し、組み直していないことを言う。
    """
    doc = _doc(parsed, f"{ACCEPT}/03_2 確認項目.md")

    assert not [c for c in doc.chunks if c.rows]    # 表にはしていない
    assert "得意先名が表示される" in _chunk(doc, "p4-x1").text
    assert "表は組み直していません" in " ".join(doc.notes)


def test_しおりが無ければ1本のまま出す(parsed) -> None:
    """**割れないことを失敗にしない。**

    議事録・メモを印刷した PDF にアウトラインは無く、それが普通である ――
    ページ数で割ると、資料に無い切れ目を機械が作ることになる。
    """
    docs, _ = parsed
    made = [name for name in docs if name.startswith(MINUTES)]

    assert made == [f"{MINUTES}/01_全ページ.md"]
    assert "しおり（アウトライン）がありません" in " ".join(docs[made[0]].notes)


def test_テキスト層が無いページは絵にして字を読む(parsed) -> None:
    """**「テキスト層が無い」は「字が無い」ではない。**

    押印の要る書類は必ずスキャンで回ってくる ―― 黙って空のページを出すと
    整理層は「資料に何も書いていない」と読む。絵は `i1`、機械の読みは `o1` に
    分けるのは、**読み違えを資料の字と混ぜない**ためである（Excel と同じ）。
    """
    doc = _doc(parsed, f"{SCANNED}/01_全ページ.md")
    anchors = [c.anchor for c in doc.chunks]

    assert anchors == ["p1-x1", "p2-i1", "p2-o1", "p3-i1", "p3-o1"]
    assert "p002.png" in _chunk(doc, "p2-i1").text

    _, findings = parsed
    said = {f.target: f.message for f in findings if f.code == "P017"}
    assert "テキスト層の無いページが 2 ページあります" in said[SCANNED.rsplit("/", 1)[-1]]


def test_字の出なかったページでも絵は出す(parsed) -> None:
    """**「読んで字が無かった」と「絵が無い」は別である。**

    OCR が 1 文字も返さなくても、絵は `images/` に出ている ―― 整理層は開いて
    読めるので、そこで止めると**読める資料を読めなくする**。空の `o1` を
    出さないのも同じ規律で、「文字は見つかりませんでした」と必ず書く。
    """
    doc = _doc(parsed, f"{SCANNED}/01_全ページ.md")

    assert "![p003.png](" in _chunk(doc, "p3-i1").text
    assert "文字は見つかりませんでした" in _chunk(doc, "p3-o1").text
    assert _chunk(doc, "p3-o1").text.strip()       # 空の `o1` は出さない


MEMO_DOC = "資料/P/受注管理システム操作手順（暫定）.docx"


def test_Wordは見出し1で節に割れる(parsed) -> None:
    """**Word は 1 冊が 1 本の流れである。**

    そのまま出すと 200 ページが 1 本の md になり、出典として指せる先が「その
    1 本」しか無くなる ―― 整理層はどの節の話かを言えず、`未読取` の宣言先も
    1 つしか持てない。割るのは**資料自身が持っている構造**（見出し 1）だけで、
    段落数や字数では割らない（それは資料に無い切れ目を機械が作ることになる）。
    """
    docs, _ = parsed
    made = sorted(name[len(PAPER) + 1:] for name in docs if name.startswith(PAPER))

    assert made == ["00_ヘッダとフッタ.md", "01_（前書き）.md", "02_1 機能概要.md",
                    "03_2 画面項目.md", "04_3 業務ルール.md"]


def test_見出しはスタイルidではなく名前で決める(parsed) -> None:
    """**日本語版の Word は組み込みスタイルに `a3` のような id を自動生成する。**

    `w:name` のほうには英語の綴り（`heading 1`）が入っており、画面に出る
    「見出し 1」は表示名で XML には無い ―― id だけを見る実装はこの検体で
    1 つも当たらず、**1 冊が丸ごと 1 本のまま出る**（割れなかったことに
    気づけないので、いちばん静かな壊れ方になる）。
    """
    doc = _doc(parsed, f"{PAPER}/02_1 機能概要.md")

    assert doc.title.endswith("2 1 機能概要")
    assert "受注ヘッダと受注明細を登録する機能" in _chunk(doc, "w2-h1").text


def test_見出しが無ければ1本のまま出す(parsed) -> None:
    """**割れないことを失敗にしない。**

    字を大きくしただけで見出しを表す手順書・議事録には、機械が読める構造が
    1 つも無い ―― そのときは 1 本のまま出すのが正しく、「割る構造が資料に
    無かった」という事実がそのまま形に出る。
    """
    docs, _ = parsed
    made = [name for name in docs if name.startswith(MEMO_DOC)]

    assert made == [f"{MEMO_DOC}/01_（本文）.md"]
    assert "メニューから「受注登録」を選ぶ" in _chunk(docs[made[0]], "w1-h1").text


def test_Wordの縦結合も下へ広げる(parsed) -> None:
    """**`w:vMerge` の続きのセルは空で保存される**（Excel の結合セルと同じ）。

    分類列の 2 行目以降だけが空欄になるが、画面では全行に掛かって見えている
    ―― 広げるのは忠実性の回復であって、判断ではない。
    """
    table = _table(_doc(parsed, f"{PAPER}/03_2 画面項目.md"))

    assert table[0] == ["区分", "項目名", "型", "必須", "備考"]
    assert [row[0] for row in table[1:]] == ["ヘッダ", "ヘッダ", "明細", "明細"]


def test_未確定の変更履歴を機械が確定させない(parsed) -> None:
    """**レビュー中の版がそのまま棚卸しに回ってくる。**

    `w:delText` を黙って残せば「まだ生きている」に見え、黙って落とせば
    「もう消した」に見える ―― どちらも資料はまだ決めていない。本文は反映後の
    姿にし、消された文字は `d1` に出して、決めるのは人に残す。
    """
    doc = _doc(parsed, f"{PAPER}/04_3 業務ルール.md")

    assert "承認期限は 3 営業日以内とする。" in _chunk(doc, "w4-h1").text
    assert "5 営業日" not in _chunk(doc, "w4-h1").text
    assert _chunk(doc, "w4-d1").cells == [("1", "承認期限は 5 営業日以内とする。")]

    _, findings = parsed
    said = {f.target: f.message for f in findings if f.code == "P019"}
    assert "変更履歴が確定していません" in said[PAPER.rsplit("/", 1)[-1]]


def test_ヘッダとフッタは文書全体に掛かるので別の1本にする(parsed) -> None:
    """**文書番号・版・機密区分はここにしか無い。**

    ただし Word のヘッダ・フッタは**文書全体**に掛かるので、節ごとに写すと
    同じ 3 行が節の数だけ並ぶ ―― Excel の印刷設定（シートごとに掛かる）とは
    掛かり方が違う。
    """
    doc = _doc(parsed, f"{PAPER}/00_ヘッダとフッタ.md")
    cells = dict(_chunk(doc, "p1").cells)

    assert cells["フッタ"] == "文書番号 DS-2026-014 ／ 第 1.2 版 ／ 社外秘"
    assert "文書全体に掛かります" in " ".join(doc.notes)
    # ほかの節には出てこない（出ていたら、同じ 3 行が 5 本に並ぶ）
    assert not [c for c in _doc(parsed, f"{PAPER}/02_1 機能概要.md").chunks
                if c.anchor == "p1"]


def test_コメントと脚注とリンク先は本文に出てこない(parsed) -> None:
    """**どれも本文を読んだだけでは出てこない。**

    コメントはレビュー指摘の置き場で本文より新しいことがあり、脚注は但し書き
    （例外条件）の置き場で、リンクは**まだ手元に無い資料**の在り処である。
    """
    rules = _doc(parsed, f"{PAPER}/04_3 業務ルール.md")
    outline = _doc(parsed, f"{PAPER}/02_1 機能概要.md")

    assert "与信管理課長に変わりました" in _chunk(rules, "w4-m1").cells[0][1]
    assert "100 万円未満は与信照会そのものを省略" in _chunk(outline, "w2-f1").cells[0][1]
    assert _chunk(rules, "w4-l1").cells[0][1].endswith("承認フロー.xlsx")


def test_申告は実在するアンカーを名指しする(parsed) -> None:
    """**案内どおりに開いて無いのは、申告していないのと同じである。**

    図形の申告は「アンカー `…-g1` を見てください」と書くが、接頭辞は形式ごとに
    違う（Excel と PowerPoint は `s`、Word は `w`）―― 決め打つと、Word の写しが
    **存在しない `s1-g1`** を案内する。
    """
    doc = _doc(parsed, f"{PAPER}/01_（前書き）.md")
    said = " ".join(doc.notes)

    assert "`w1-g1`" in said
    assert "`s1-g1`" not in said
    assert "w1-g1" in {c.anchor for c in doc.chunks}


def test_スライドは1枚がファイル1本になる(parsed) -> None:
    """**Excel のシートとまったく同じ形にする。**

    白紙のスライド（章の切れ目に置く 1 枚）と非表示のスライドは出さない ――
    出すと「作業用の白紙がぜんぶパース結果になる」（Excel と同じ規律）。
    名前に並び順を付けるのは、**スライドは順序が意味を持つ**からである
    （シート名と違い、表題だけでは並べ直せない）。
    """
    docs, _ = parsed
    made = sorted(name[len(DECK) + 1:] for name in docs if name.startswith(DECK))

    assert made == ["01_全体構成（A 案）.md", "02_移行対象と件数.md",
                    "03_体制と役割分担.md"]


def test_スライドの図形と接続はExcelと同じ道で取れる(parsed) -> None:
    """**`p:sp` / `p:cxnSp` は `xdr:sp` / `xdr:cxnSp` と名前空間しか違わない。**

    箱の中の文字も、接続の端点（`a:stCxn`）も、矢羽根も、線種も同じものである
    ―― だから `_shapes` は 1 本しか無い。線種を必ず出すのも Excel と同じ理由で、
    体制図の凡例は「実線＝同期 / 破線＝夜間バッチ」と線で意味を描き分ける。
    """
    doc = _doc(parsed, f"{DECK}/01_全体構成（A 案）.md")
    links = _chunk(doc, "s1-c1").rows

    assert links[0] == ["元", "向き", "先", "名前", "線種"]
    assert ["与信の枠内？", "→", "在庫引当", "夜間バッチ", "破線"] in links
    # **箱の中の改行は残る**（`a:br`）―― 潰すと別々の語が 1 語に化ける。
    assert "オーダー入力\n（Web／代行）" in [text for _, text in _chunk(doc, "s1-g1").cells]


def test_繋がっていない線は本数だけ言う(parsed) -> None:
    """**両端の id が資料に無い線は、どこからどこへの線か決まらない。**

    目分量で置いた矢印は実物の構成図に普通に混ざる ―― 座標から当てるのは
    意味の判断なので、取らずに本数を申告する（Excel とまったく同じ扱い）。
    """
    doc = _doc(parsed, f"{DECK}/01_全体構成（A 案）.md")
    said = " ".join(doc.notes)

    assert "接続子 1 本はどこにも繋がっていません" in said
    assert len(_chunk(doc, "s1-c1").rows) == 4         # 見出し ＋ 取れた 3 本


def test_スライドの表はa_tblから取る(parsed) -> None:
    """**PowerPoint の一覧はすべて `a:tbl` に入っている。**

    埋め込みオブジェクトとして数えて中身を捨てると、スライドの表は 1 枚も
    仕様にならない。縦結合（`vMerge`）は Excel の結合セルと同じく下へ広げる
    ―― 画面では上の「受注」が 2 行に掛かって見えている。
    """
    table = _table(_doc(parsed, f"{DECK}/02_移行対象と件数.md"))

    assert table[0] == ["区分", "表名", "論理名", "件数"]
    assert [row[0] for row in table[1:]] == ["受注", "受注", "マスタ"]


def test_発表者ノートは別のアンカーに出す(parsed) -> None:
    """**なぜそう決めたかがノートにしか書かれていないことがある。**

    スライドの箱は結論だけを載せる書き方をするので、B 案を採らなかった理由は
    スライドの上に 1 文字も出ていない。`g1` と混ぜないのは、**客先に見せた結論と
    書いた人の手控え**を同じ出典にしないためである。
    """
    doc = _doc(parsed, f"{DECK}/01_全体構成（A 案）.md")
    note = _chunk(doc, "s1-n1").text

    assert "B 社の保守期限が 2027-03 で切れるため" in note
    assert "採らなかった" in note
    # スライドの側には出ていない（出ていたら、この検体の主張が崩れる）
    assert "保守期限" not in " ".join(t for _, t in _chunk(doc, "s1-g1").cells)


def test_スライドのコメントは本文より新しいことがある(parsed) -> None:
    """**レビュー指摘の置き場**（Excel のセルのコメントと同じ役目）。

    体制のスライドは「常駐 3 名」のままで、コメントに「2 名に減った」が付く
    ―― 表にもスライドにも出てこないので、読まないと古い体制が正本になる。
    """
    cells = _chunk(_doc(parsed, f"{DECK}/03_体制と役割分担.md"), "s3-m1").cells

    assert cells[0][0].startswith("鈴木（PMO）")
    assert "常駐は 2 名に減りました" in cells[0][1]


def test_非表示のスライドは読まないが言う(parsed) -> None:
    """**旧版を隠して配るのは実案件でごく普通**（非表示シートと同じ事情）。

    読まないので写しには 1 文字も出てこない ―― 中身が要るなら再表示して
    取り込み直すという判断は、機械ではなく人がする。
    """
    _, findings = parsed
    said = {f.target: f.message for f in findings if f.code == "P003"}
    name = DECK.rsplit("/", 1)[-1]

    assert "非表示のスライドが 1 枚あります" in said[name]
    assert "4 全体構成（B 案・旧版）" in said[name]


def test_レイアウトとマスターを読んでいないことを言う(parsed) -> None:
    """**取ると全スライドに同じ文字が並び、資料が増えたように見える。**

    が、黙ると「資料に無い」と読まれる ―― 文書番号・版・機密区分がフッタに
    しか書かれていないことがあり、それは Excel の `p1` とまったく同じ事情である。
    """
    doc = _doc(parsed, f"{DECK}/02_移行対象と件数.md")

    assert "レイアウトとマスターに書かれた文字" in " ".join(doc.notes)
    assert "資料に無いのではありません" in " ".join(doc.notes)


def test_読めないバイトは置き換えたことを言う(parsed) -> None:
    """**EUC-JP の漢字は cp932 でも読み切れない**（`受` の 0xF5 が先導バイト）。

    最後の砦（`errors="replace"`）まで落ちて `�` が並ぶ ―― その欄は資料の値
    ではないので、表に入れる以上そう書く。
    """
    doc = _doc(parsed, "資料/N/旧コード表.csv.md")
    said = " ".join(doc.notes)

    assert "読めないバイトがあります" in said
    assert "資料の値ではありません" in said


def test_宣言の無いcp932のソースでも骨格が取れる(parsed) -> None:
    """**既存資産の日本語は cp932 である。** UTF-8 で決め打っていたぶん、
    コメントに日本語が 1 文字あるだけで**骨格ごと落ちていた**（`P010`）
    ―― シグネチャは ASCII なので、読めていれば出せたものである。"""
    doc = _doc(parsed, "受領/受注システム/src/order/受注サービス.py.md")
    table = _table(doc)

    assert doc.chunks[0].heading == "クラス: OrderService"
    assert [row[0] for row in table[1:]] == ["OrderService", "register", "cancel"]
    assert table[3][5] == "AlreadyShipped"


def test_構文の通らないソースにzipの話をしない(parsed) -> None:
    """**開けなかった理由の続きは、形式ごとに別のことを言う。**

    `_unopenable` を形式に関わらず足していたので、`print "…"`（Python 2 の
    コード ―― 棚卸しの対象には普通に混ざる）に「**zip として開けません ――
    添付が途中で切れた**」と言っていた。メールを探しに行かせて、そこに資料は
    無い ―― `veryHidden` に「再表示してください」と言ったのと同じ形である。

    しかもソースの側は壊れていない。**原本はテキストなので整理層が直接読める**
    ので、資料を取り直す話にしてはいけない。
    """
    _, findings = parsed
    said = {f.target: f.message for f in findings if f.code == "P010"}

    assert "Python として構文が通りません" in said["帳票出力.py"]
    assert "原本はテキストなので整理層が直接読めます" in said["帳票出力.py"]
    # **案内どおりにやって届かないことは言わない。**
    assert "zip" not in said["帳票出力.py"]
    assert "添付" not in said["帳票出力.py"]
    # Excel の側の案内は変わっていない（形式で分けただけである）
    assert "添付" in said["添付_破損.xlsx"]


def test_大きい塊は大きいと言う(parsed) -> None:
    """**抽出ツールが吐いた一覧は 1 枚で数千行ある。**

    見出し行と最初の 20 行を見たかぎりでは小さい表と見分けが付かないので、
    読み手（整理層）は**先頭だけ読んで「読んだ」と思う**。値は落ちていないので
    「読めなかったもの」の申告ではなく、`_sparse_note` と同じ**提示上の申告**
    である ―― 黙ると読み手が事実と違うものを読む、というところが同じである。
    """
    doc = _doc(parsed, "資料/M/受注明細抽出.xlsx/抽出結果.md")
    table = _table(doc, "s1-t1")

    assert len(table) == 1201                    # 表のまま出す（切り詰めない）
    assert table[-1][0] == "ORD-001200"
    assert "1201 行" in _notes(doc) and "先頭だけ読むと" in _notes(doc)


def test_大きさの申告は数えるほどしか出ない(parsed) -> None:
    """**何でも申告すると、本当に読めていないものが山に埋もれる。**

    しきい値（1,000）を超えるのは 60 枚を超える検体のうち 4 枚だけである。
    ただし**その内訳は変わった** ―― `限界.yml` を足すまでは抽出ツールの
    一覧 1 枚だけで、そこから「現場の設計書では 1 度も出ない」と書いていた。

    出るようになったのは、**人が書いた設計書のまま大きい 2 枚**である ――
    83 列 × 200 行の CRUD 図（4,707 セル）と、紙のために見出しを 6 回
    繰り返す一覧（1,836 セル）。しきい値は変えない（何でも申告する側へ
    戻る）が、**申告文が「抽出ツールが吐いた一覧はこの形になります」と
    出どころを決めつけている**ぶんは当てにできない（→ `docs/parsed.md`）。

    現場の設計書 2 冊（`設計書.yml` / `詳細設計書.yml`）は**どのシートも
    出てこない** ―― そちらは 1 枚が浅いか、横に長くても行が少ない。
    """
    docs, _ = parsed
    said = sorted(name for name, doc in docs.items()
                  if "先頭だけ読むと" in _notes(doc))
    assert said == [
        "資料/M/受注明細抽出.xlsx/抽出結果.md",          # 1,201 行 × 8 列
        "資料/P/CRUD図.xlsx/CRUD図.md",                  # 203 行 × 83 列
        "資料/P/移行対象一覧（全件）.xlsx/移行対象.md",         # 6,001 行 × 30 列
        "資料/P/移行対象一覧（全件）.xlsx/繰り返す見出し.md",   # 306 行 × 6 列
    ]
    assert not [name for name in said if name.startswith(("資料/K/", "資料/L/"))]


def test_枠だけ大きい塊は箇条書きへ回る(parsed) -> None:
    """工程表を年単位に伸ばした 1 枚。**枠は 600 列、中身は 600 セル**である。"""
    doc = _doc(parsed, "資料/M/受注明細抽出.xlsx/年間工程.md")
    chunk = _chunk(doc, "s2-x1")

    assert not chunk.rows                        # 格子は組まない
    assert len(chunk.cells) == 601
    assert chunk.at == "A1:WB601"
    assert "601 セルしか無い" in _notes(doc)


def test_壊れたシート1枚で1冊を落とさない(parsed) -> None:
    """**「1 冊が壊れていて 29 冊が落ちる」は、ブックの中でも起きていた。**

    シート 1 枚の XML が壊れていると `load_workbook` は 1 冊まるごと投げる
    ―― 3 枚のうち 2 枚は読めるのに `P010` で 1 冊が消えていた。救出モード
    （`read_only`）は 1 枚ずつ遅延で読むので、**壊れた 1 枚だけが落ちる**。
    """
    docs, findings = parsed
    said = [f for f in findings if f.code == "P012"]
    assert [f.target for f in said] == ["移行データ仕様書.xlsx"]
    assert "変換規則" in said[0].message and "残り 2 枚" in said[0].message

    read = sorted(n.split("/")[-1] for n in docs if "移行データ仕様書" in n)
    assert read == ["移行スケジュール.md", "移行対象.md"]
    assert "変換規則" not in " ".join(docs)         # 壊れた 1 枚は出さない
    # **開けない 1 冊（P010）とは別に数える。** 壊れ方が違えばやることも違う。
    assert "移行データ仕様書.xlsx" not in [f.target for f in findings
                                          if f.code == "P010"]


def test_救出モードで取れなかったものは値の隣で言う(parsed) -> None:
    """**申告はシートの中に置く。**

    ブック単位の findings はコンソールに 1 度出るだけで、パース結果を読むときに
    は手元に無い ―― 救出モードでは**縦結合が広がらない**ので、区分の列は
    2 行目以降が空欄になる（画面では全行に掛かって見えている）。黙ると
    整理層には「区分の無い行」に見え、**資料に無いのか読めていないのかが混ざる。**
    """
    doc = _doc(parsed, "資料/M/移行データ仕様書.xlsx/移行対象.md")
    table = _table(doc)

    assert [row[0] for row in table] == ["区分", "受注", "", "マスタ"]
    assert "救出モード" in _notes(doc)
    assert "縦結合" in _notes(doc)
    # **案内どおりにやって届かないことは言わない。** 実物の Excel（16.0）では
    # 修復モードでも開けなかったので、「開いて保存し直せば直る」とは書かない。
    assert "保存し直せば" not in _notes(doc)


def test_日付として範囲外のシリアルはopenpyxlの言い分ごと出す(parsed) -> None:
    """**資料は壊れていないのに、壊れていると言っていた。**

    納期の欄に年数や引き算の結果が入っていると、openpyxl はそのセルを
    `#VALUE!` にして**警告する** ―― その警告は stderr へ 1 行流れて消えるので、
    こちらはそれを「エラー値（計算できていない）」としてだけ申告していた。
    読み落としより悪い（申告のほうが嘘になる）。
    """
    docs, findings = parsed
    said = [f for f in findings if f.code == "P013"]
    assert [f.target for f in said] == ["納期管理表.xlsx"]
    assert "outside the limits for dates" in said[0].message

    doc = _doc(parsed, "資料/M/納期管理表.xlsx/納期.md")
    assert _chunk(doc, "s1-e1").cells[0][0] == "C3"   # 突き合わせる先は残す


def test_openpyxlが黙っているブックでP013を出さない(parsed) -> None:
    """**何でも申告すると、本当に読めていないものが山に埋もれる。**"""
    _, findings = parsed
    assert len([f for f in findings if f.code == "P013"]) == 1


def test_開けなかったブックがファイルを掴んだままにしない(parsed) -> None:
    """**身に覚えのない申告が、読めているブックに付く。**

    `load_workbook` は自分でファイルを開くので、シート 1 枚の XML で投げると
    handle が閉じられずに残る ―― 回収されるのは GC のときなので、そこで出る
    `unclosed file` は**次に読んだブックの言い分**（`P013`）として出た。

    しかも付く先は読む順で変わる（**資料を 1 冊足すだけで動く**）ので、
    受け取った側は追いようがない。申告が嘘になる形はどれも、読み落としより悪い。
    """
    _, findings = parsed
    assert not [f for f in findings if "unclosed file" in f.message]


def test_セルの一部だけの取り消し線も取る(parsed) -> None:
    """**セルの書式は 1 つしか持てない。**

    項目名を残して `（廃止）` だけを消すのは実物でごく普通の書き方だが、
    `cell.font.strike` は**セル 1 つに 1 つ**なので 1 件も引っ掛からなかった
    ―― 取り消し線を書式から取ると決めた理由（その書式だけが値を偽る）が、
    **いちばん値を偽る書き方でだけ成り立っていなかった。**
    """
    doc = _doc(parsed, "資料/M/項目定義書.xlsx/項目.md")
    struck = dict(_chunk(doc, "s1-d1").cells)

    assert "消してあるのは「（廃止）」だけです" in struck["B3"]
    assert struck["B3"].startswith("希望納期（廃止）")     # 値は削らない
    # **まるごと消した欄と混ぜない。** 一部なら残りは生きている。
    assert struck["D3"] == "受注日から自動計算へ"
    assert "うち 1 個はセルの一部だけ" in _notes(doc)
    # 表のほうは画面どおり（値を落とさない・記号を足さない）
    assert [row[1] for row in _table(doc)][1:] == ["受注番号", "希望納期（廃止）",
                                                   "与信区分"]


def test_Markdownの記法はアンカーを増やさない(tmp_path: Path, parsed) -> None:
    """**セルの値が器の構造を壊せてはいけない。**

    `やっかいな値.xlsx` が突いたのは `<!--` だけだった。HTML の画面仕様書には
    `#` も ` ``` ` も `|` も普通に出てくるので、**書き出して読み戻す**ところ
    まででそれを確かめる（書き出したものだけを見ていると、値が正しく出て
    いるようにしか見えない）。
    """
    doc = _doc(parsed, "資料/M/画面項目仕様書.xlsx/項目.md")
    path = mdio.write(tmp_path / "項目.md", doc)
    back = mdio.read(path)

    assert [a.id for a in back.anchors] == [c.anchor for c in doc.chunks]
    # 画面に見えている表記は保つ（記号を落とさない・足さない）
    テンプレート = [row[2] for row in _table(doc)]
    assert "## 見出しに見える行" in [row[3] for row in _table(doc)]
    assert "```" in テンプレート and "---" in テンプレート
    assert "| 行 | 商品 |" in テンプレート            # パイプは逃がして出す
    assert r"\|" in path.read_text(encoding="utf-8")
    assert "承認待ち\n3 営業日以内" in テンプレート    # 改行は繋げない
