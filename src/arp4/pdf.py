"""① PDF → パース結果。**しおりの節 = ファイル 1 本**（無ければ 1 冊 1 本）。

PDF は「もらったまま読むしかない資料」の代表である ―― 検収仕様書・議事録・
他社製品の仕様書・官公庁の様式は PDF でしか配られないことが普通で、原本の
Excel や Word はこちらに来ない。**「2 側（docextract）にあります」と言っていた
あいだ、その 1 冊は誰にも読まれずに終わっていた。**

**表は組み直さない。** PDF の中に「表」という構造は無く、あるのは**位置を持った
文字**だけである。列の切れ目は文字の隙間から当てにいくしかなく、閾値を外すと
**列がずれた表が「読めた」顔で出る** ―― CSV の区切りを機械に当てさせないと
決めたのと同じ理由である。行のテキストとして出し、組み直していないことを言う。

**テキスト層の無いページは、字が無いのではない。** 紙をスキャンしただけの PDF
（検収の押印付き・客先から回ってきた FAX）は、機械にとっては 1 枚の絵である。
そこは絵として `images/` へ出し（`i1`）、中の字は :mod:`arp4.ocr` が読む
（`o1`）―― Excel の貼り付け画像とまったく同じ扱いで、**機械の読みは資料の字と
混ぜない。**
"""

from __future__ import annotations

import io
from pathlib import Path
from typing import Any

from arp4 import mdio, ocr, parse
from arp4.finding import Finding

#: 焼くときの倍率（1 = 72dpi）。**OCR に足りるところまで**で止める ―― 上げても
#: 読める字は増えず、1 冊ぶんの時間だけが伸びる（:mod:`arp4.render` と同じ判断）。
_RASTER = 2

#: 「テキスト層が無い」とみなす字数。**0 で切らない** ―― スキャンした PDF にも
#: ページ番号だけがテキストで乗っていることがある（帳票ツールが付ける）。
_EMPTY_TEXT = 8

#: パース結果の ``<!-- source: … -->`` で文書と節を繋ぐ語。
_SECTION_MARK = " / しおり: "

#: しおりが 1 つも無いときの節の名前。
_WHOLE = "全ページ"


def section_source(relative: Path, title: str) -> str:
    """``資料/A/検収仕様書.pdf / しおり: 2 確認項目``。"""
    return f"{relative.as_posix()}{_SECTION_MARK}{title}"


def read(path: Path, relative: Path, use_ocr: bool = True
         ) -> tuple[list[tuple[Path, mdio.Doc]], list[Finding],
                    dict[Path, list[parse.Media]]]:
    """1 冊を読む。戻すのは :func:`arp4.parse._parse_one` と同じ 3 つ組。"""
    try:
        import pypdfium2 as pdfium                  # 依存は使うときだけ読む
    except ImportError:
        return [], _unavailable(path), {}

    document = pdfium.PdfDocument(str(path))
    try:
        pages = [_page(document, index) for index in range(len(document))]
        sections = _sections(_toc(document), len(pages))
        scanned = [one for one in pages if one.scanned]
        if scanned:
            # **`--no-ocr` でも絵は出す。** 落とすのは機械の読みだけで、
            # 実体を渡さないことではない ―― 整理層は絵を開いて読める
            # （Excel の貼り付け画像を `--no-ocr` でも出すのと同じ）。
            _look(document, scanned)
        metadata = _metadata(document)
    finally:
        document.close()

    readings = ocr.read({one.part: one.shot for one in scanned if one.shot}) \
        if use_ocr else None

    made: list[tuple[Path, mdio.Doc]] = []
    media: dict[Path, list[parse.Media]] = {}
    for section in sections:
        doc, shots = _document(section, pages, relative, readings, use_ocr)
        if not doc.chunks:
            continue
        out = Path(*relative.parts) / f"{section.stem}{mdio.EXT}"
        made.append((out, doc))
        if shots:
            media[out] = shots

    return made, (_scanned_note(path, pages, use_ocr)
                  + _properties_note(path, metadata)
                  + _nothing_note(path, pages, made)), media


# ── ページ ──────────────────────────────────────────────────────
class Page:
    """1 ページ。**字が取れたか、絵だったか**のどちらかである。"""

    __slots__ = ("number", "text", "shot", "trouble")

    def __init__(self, number: int, text: str) -> None:
        self.number = number
        self.text = text
        #: 焼いた絵（テキスト層が無かったページだけ）。
        self.shot: bytes | None = None
        #: 焼けなかった理由。
        self.trouble = ""

    @property
    def scanned(self) -> bool:
        """**字が無いのではなく、テキスト層が無い。**"""
        return len(self.text.strip()) < _EMPTY_TEXT

    @property
    def part(self) -> str:
        return f"p{self.number}"


def _page(document: Any, index: int) -> Page:
    """1 ページぶんの字。**改行は LF に揃える**（pdfium は CRLF を返す）。"""
    page = document[index]
    try:
        textpage = page.get_textpage()
        try:
            text = textpage.get_text_range() or ""
        finally:
            textpage.close()
    except Exception:                                # noqa: BLE001
        # **1 ページが読めなくても残りを読む**（ブックの 1 シートと同じ規律）。
        text = ""
    finally:
        page.close()
    return Page(index + 1, text.replace("\r\n", "\n").replace("\r", "\n"))


def _look(document: Any, scanned: list[Page]) -> None:
    """テキスト層の無いページを焼く。**焼けなくても止まらない。**

    焼くのは OCR に掛けるためと、**整理層が開いて読めるようにする**ためである
    ―― 機械が読めないことと、この資料が誰にも読まれないことは別である
    （貼り付け画像を ``images/`` へ出したのと同じ理由）。
    """
    try:
        from PIL import Image                       # noqa: F401
    except ImportError:
        for one in scanned:
            one.trouble = ("焼くには Pillow が要ります"
                           '（pip install "ai-ready-pipeline4[parse]"）')
        return
    for one in scanned:
        try:
            page = document[one.number - 1]
            try:
                image = page.render(scale=_RASTER).to_pil().convert("RGB")
            finally:
                page.close()
            buffer = io.BytesIO()
            image.save(buffer, "PNG")
            one.shot = buffer.getvalue()
        except Exception as exc:                     # noqa: BLE001
            one.trouble = str(exc) or exc.__class__.__name__


# ── しおりで節に割る ────────────────────────────────────────────
class Section:
    """しおり 1 つぶん（``開始ページ`` から次のしおりの手前まで）。"""

    __slots__ = ("order", "title", "start", "stop")

    def __init__(self, order: int, title: str, start: int, stop: int) -> None:
        self.order = order
        self.title = title
        self.start = start                           # 1 始まり
        self.stop = stop                             # この番号を含む

    @property
    def stem(self) -> str:
        return f"{self.order:02d}_{parse.safe_name(self.title)}"

    @property
    def label(self) -> str:
        return f"{self.order} {self.title}"


def _toc(document: Any) -> list[tuple[str, int]]:
    """第 1 階層のしおり ``[(表題, 開始ページ)]``。**深い階層は割り先にしない。**

    章より細かく割ると 1 本が数段落になり、**出典が細かすぎて資料の姿が
    見えなくなる**（Excel のシートより細かく割らないのと同じ）。深い階層の
    しおりは落とすのではなく、割り先にしないだけである ―― 本文はどの節かに
    必ず入る。
    """
    found: list[tuple[str, int]] = []
    try:
        for bookmark in document.get_toc(max_depth=1):
            if getattr(bookmark, "level", 0) != 0:
                continue
            destination = bookmark.get_dest()
            if destination is None:
                continue
            title = (bookmark.get_title() or "").strip()
            if title:
                found.append((title, destination.get_index() + 1))
    except Exception:                                # noqa: BLE001
        return []
    return found


def _sections(toc: list[tuple[str, int]], pages: int) -> list[Section]:
    """しおりを節にする。**しおりが無ければ 1 節**（＝ 1 冊が 1 本）。

    最初のしおりが 1 ページ目より後ろなら、その手前を「（前書き）」にする
    ―― 表紙・改訂履歴はそこにあり、捨てると**アンカーが 0 個のページ**が
    できる（:func:`arp4.parse._markdown` と同じ理由）。
    """
    if not pages:
        return []
    if not toc:
        return [Section(1, _WHOLE, 1, pages)]

    found: list[Section] = []
    if toc[0][1] > 1:
        found.append(Section(1, "（前書き）", 1, toc[0][1] - 1))
    for index, (title, start) in enumerate(toc):
        stop = toc[index + 1][1] - 1 if index + 1 < len(toc) else pages
        if stop < start:
            stop = start                             # 同じページに 2 つのしおり
        found.append(Section(len(found) + 1, title, start, min(stop, pages)))
    return found


# ── 1 節ぶんの組み立て ──────────────────────────────────────────
def _document(section: Section, pages: list[Page], relative: Path,
              readings: dict[str, ocr.Reading] | None, attempted: bool
              ) -> tuple[mdio.Doc, list[parse.Media]]:
    """節 1 つ = ファイル 1 本。**アンカーはページ番号で振る。**"""
    doc = mdio.Doc(title=f"{relative.name} / {section.label}",
                   source=section_source(relative, section.label))
    doc.notes.append(_TABLE_NOTE)
    if section.title == _WHOLE:
        doc.notes.append(_NO_TOC_NOTE)

    shots: list[parse.Media] = []
    mine = [one for one in pages if section.start <= one.number <= section.stop]
    for page in mine:
        if not page.scanned:
            doc.chunks.append(mdio.Chunk(
                anchor=f"p{page.number}-x1", at=f"p.{page.number}",
                heading=f"{page.number} ページ", text=page.text.strip()))
            continue
        shots += _scanned(doc, page, relative, readings, attempted)
    return doc, shots


def _scanned(doc: mdio.Doc, page: Page, relative: Path,
             readings: dict[str, ocr.Reading] | None,
             attempted: bool) -> list[parse.Media]:
    """テキスト層の無いページ。**絵として出し、字は機械が読む。**"""
    if page.shot is None:
        doc.chunks.append(mdio.Chunk(
            anchor=f"p{page.number}-x1", at=f"p.{page.number}",
            heading=f"{page.number} ページ（テキスト層がありません）",
            text=("このページにはテキスト層がありません（紙をスキャンした"
                  "ものです）。絵にもできませんでした"
                  + (f"（{page.trouble}）" if page.trouble else "")
                  + "。開いて読むか、out_of_scope に kind: 未読取 で"
                  "宣言してください。")))
        return []

    shot = parse.Media(name=f"p{page.number:03d}.png", body=page.shot,
                       reading=(readings or {}).get(page.part))
    up = "../" * (len(relative.parts) + 1)
    where = f"{up}images/{relative.as_posix()}"
    doc.chunks.append(mdio.Chunk(
        anchor=f"p{page.number}-i1", at=f"p.{page.number}",
        heading=f"{page.number} ページ（テキスト層が無いので絵にしました）",
        cells=[(shot.name, "紙をスキャンしたページです")],
        text=f"![{shot.name}]({where}/{shot.name})"))
    said = parse._read_chunk(page.number, [shot], attempted, "p")
    if said is not None:
        doc.chunks.append(said)
    return [shot]


#: **表は組み直していない。** PDF の中に「表」という構造は無い。
_TABLE_NOTE = (
    "**表は組み直していません。** PDF が持っているのは位置を持った文字だけで、"
    "「ここからここまでが 1 列」という区切りはどこにも書かれていません ―― "
    "文字の隙間から当てにいくと、閾値の外れたところで**列がずれた表が"
    "「読めた」顔で出ます**。行のテキストとして出しているので、表として"
    "読むなら原本を開いて確かめてください。")

#: しおりが無い 1 冊。
_NO_TOC_NOTE = (
    "この PDF には**しおり（アウトライン）がありません**。割る構造が資料に"
    "無いので、1 冊を 1 本のまま出しています ―― 段落数や字数で割ると、"
    "資料に無い切れ目を機械が作ることになります。")


# ── 申告 ────────────────────────────────────────────────────────
def _scanned_note(path: Path, pages: list[Page], attempted: bool) -> list[Finding]:
    """**テキスト層が無いのは「字が無い」ではない。**

    紙をスキャンしただけの PDF（押印付きの検収書・FAX で回ってきた仕様変更）は
    機械にとって 1 枚の絵である ―― 黙って空のページを出すと、整理層は
    「資料に何も書いていない」と読む。
    """
    scanned = [one for one in pages if one.scanned]
    if not scanned:
        return []
    listed = "・".join(f"p.{one.number}" for one in scanned[:10])
    if len(scanned) > 10:
        listed += f" ほか {len(scanned) - 10} ページ"
    if not attempted:
        return [Finding("warn", "P017", path.name,
                        f"テキスト層の無いページが {len(scanned)} ページ"
                        f"あります（{listed}）。絵にして `images/` へ出して"
                        "ありますが、`--no-ocr` なので**中の字は読みに"
                        "いっていません** ―― 開いて読むのは整理層の仕事です。")]
    failed = [one for one in scanned if one.shot is None]
    note = (f"テキスト層の無いページが {len(scanned)} ページあります"
            f"（{listed}）。紙をスキャンした PDF がこの形になります ―― "
            "字が無いのではありません。絵にして `images/` へ出し、"
            "中の文字は Windows OCR が読んで `o1` に出してあります"
            "（読み違えが混ざるので、値として使う前に絵を開いて"
            "確かめてください）。")
    if failed:
        note += (f"うち {len(failed)} ページは絵にできませんでした"
                 f"（{failed[0].trouble or '理由は不明です'}）。")
    return [Finding("warn", "P017", path.name, note)]


def _metadata(document: Any) -> list[tuple[str, str]]:
    """PDF のプロパティ。**本文にも表にも出てこない**（Excel と同じ事情）。"""
    try:
        found = document.get_metadata_dict()
    except Exception:                                # noqa: BLE001
        return []
    return [(label, str(found[key]).strip())
            for label, key in (("文書の表題", "Title"), ("作成者", "Author"),
                               ("作成アプリ", "Producer"),
                               ("作成日時", "CreationDate"),
                               ("最終更新日時", "ModDate"))
            if found.get(key) and str(found[key]).strip()]


def _properties_note(path: Path, core: list[tuple[str, str]]) -> list[Finding]:
    if not core:
        return []
    listed = "／".join(f"{label} {value}" for label, value in core)
    return [Finding("warn", "P005", path.name,
                    f"PDF のプロパティ（{listed}）。本文にも表にも出てきません。"
                    "**何から書き出された PDF か**がここに残っていることが"
                    "あり（作成アプリ）、原本を当たれるかどうかが変わります。")]


def _unavailable(path: Path) -> list[Finding]:
    """**読める道具が入っていない。** 環境の話なので `P016` と同じ形で言う。"""
    return [Finding("warn", "P020", path.name,
                    "PDF を読むには pypdfium2 が要ります"
                    '（pip install "ai-ready-pipeline4[parse]"）。'
                    "この 1 冊は 1 行もパースできていません ―― 資料に中身が"
                    "無いのではありません。")]


def _nothing_note(path: Path, pages: list[Page],
                  made: list[tuple[Path, mdio.Doc]]) -> list[Finding]:
    if made:
        return []
    return [Finding("warn", "P009", path.name,
                    f"パース結果が 1 本も出ませんでした（{len(pages)} ページ）。"
                    "中身があるはずなら、原本を開いて確かめてください。")]
