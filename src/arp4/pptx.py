"""① PowerPoint → パース結果。**スライド 1 枚 = ファイル 1 本**（Excel と同じ）。

長いあいだ `.pptx` は「2 側（docextract）にあります」と言って終わっていた。
言い分は「実測（Excel 30 冊）では議事録も処理詳細も Excel の表だった」である ――
**その 30 冊に PowerPoint が入っていなかっただけ**で、方式提案・移行方針・
体制と役割分担・フェーズ計画は PowerPoint で配られるのが普通である。回した先
（2 側）は文書の側なので、そこへ回しても誰も読まない。

**書き足したのは入口だけである。** スライドの図形は ``p:sp`` / ``p:cxnSp`` で、
Excel の ``xdr:sp`` / ``xdr:cxnSp`` と**外側の名前空間しか違わない** ―― 箱の中の
文字（``a:t``）も、接続の端点（``a:stCxn``）も、矢羽根も、線の見た目も、まったく
同じものである。だから :func:`arp4.parse._shapes` をそのまま使い
（:data:`arp4.parse._SLIDE` を渡すだけ）、画像も OCR も Excel と同じ道を通る。

**アンカーの語彙も増やさない。** ``s<並び順>-t1`` ``-g1`` ``-c1`` ``-i1``
``-o1`` ``-m1`` ``-a1`` ``-k1`` は Excel と同じ意味で、整理層が覚え直すものは
1 つも無い。増やしたのは **``n1``（発表者ノート）** だけで、これは
PowerPoint にしか無い置き場である ―― そして**決めた理由がそこにしか書かれて
いない**ことが実際にある（スライドの箱は結論だけを載せる書き方をするので）。

**取れないもの。** レイアウトとマスターに書かれた文字（ページ番号・フッタ・
定型の見出し）は取らない ―― スライド 1 枚ずつに写すと、全スライドに同じ文字が
並んで**資料が増えたように見える**。取っていないことは申告する。
"""

from __future__ import annotations

import xml.etree.ElementTree as ET
import zipfile
from pathlib import Path
from typing import NamedTuple

from arp4 import mdio, ocr, parse
from arp4.finding import Finding

#: スライドの並び。``ppt/presentation.xml`` の ``p:sldIdLst`` が**表示順**である
#: （パート名の ``slide12.xml`` は作られた順で、並べ替えても付け替わらない）。
_PRESENTATION = "ppt/presentation.xml"

#: マクロの入れ物。``.xlsm`` と同じで**中身は取らない**（OLE 複合ドキュメント）。
_VBA_PART = "ppt/vbaProject.bin"

#: 関係の種別。
_REL_NOTES = "/notesSlide"
_REL_COMMENTS = "/comments"
_REL_AUTHORS = "/commentAuthors"
_REL_MODERN_AUTHORS = "/authors"

#: :func:`arp4.parse._related_many` に渡す「何を取りに行くか」。``""`` は
#: **スライドの XML そのもの**（図形はそこに入っている → :func:`follow`）。
_WANTED = {"slide": ("", ""),
           "image": ("", parse._REL_IMAGE),
           "chart": ("", parse._REL_CHART),
           "diagram": ("", parse._REL_DIAGRAM),
           "notes": (_REL_NOTES, ""),
           "comments": (_REL_COMMENTS, "")}

#: パース結果の ``<!-- source: … -->`` でファイルとスライドを繋ぐ語
#: （:data:`arp4.parse._SHEET_MARK` と対）。**シートと綴りを分ける**のは、
#: ``arp4 render --pending`` が「シート」だけを撮り直しに行くからである ――
#: 同じ綴りにすると、撮れないものを撮りに行って空振りする。
_SLIDE_MARK = " / スライド: "


def slide_source(relative: Path, title: str) -> str:
    """``資料/A/方式提案.pptx / スライド: 3 全体構成``。"""
    return f"{relative.as_posix()}{_SLIDE_MARK}{title}"


def read(path: Path, relative: Path, use_ocr: bool = True
         ) -> tuple[list[tuple[Path, mdio.Doc]], list[Finding],
                    dict[Path, list[parse.Media]]]:
    """1 冊を読む。戻すのは :func:`arp4.parse._parse_one` と同じ 3 つ組。"""
    trouble: list[str] = []
    order = _order(path, trouble)
    shown = [one for one in order if one.shown]
    related = parse._related_many(
        path, _WANTED, trouble,
        parts_of=lambda _archive: {one.key: one.part for one in shown})
    bodies = _bodies(related)
    drawings = {one.key: _drawing(related, one.key) for one in shown}
    readings = parse._readings(drawings, bodies) if use_ocr else None
    authors = _authors(path)

    made: list[tuple[Path, mdio.Doc]] = []
    media: dict[Path, list[parse.Media]] = {}
    for one in shown:
        drawing = drawings[one.key]
        notes = _notes(related["notes"].get(one.key, []))
        comments = _comments(related["comments"].get(one.key, []), authors)
        if not (drawing.total or drawing.unreadable or notes or comments):
            continue                                   # 白紙のスライド
        doc = _document(one, relative, drawing, notes, comments, readings)
        shots, pictures, said = parse._pictures(
            one.order, one.stem, relative, drawing, bodies, readings)
        if pictures is not None:
            doc.chunks.append(pictures)
        if said is not None:
            doc.chunks.append(said)
        out = Path(*relative.parts) / f"{one.stem}{mdio.EXT}"
        made.append((out, doc))
        if shots:
            media[out] = shots

    hidden = [one for one in order if not one.shown]
    return made, (_hidden_note(path, hidden) + _macro_note(path)
                  + _properties_note(path) + _gap_note(path, trouble)
                  + _nothing_note(path, order, made, hidden)), media


# ── スライドの並びと中身 ────────────────────────────────────────
class Slide(NamedTuple):
    """スライド 1 枚の見出し。**表題と鍵を分けて持つ。**

    表題（``全体構成``）は資料に書いてあるものだが、**一意ではない** ――
    ``まとめ`` という表題のスライドが 1 冊に 3 枚あるのは普通のことである。
    束ねる側（:func:`arp4.parse._related_many` の戻り値の鍵）は一意でなければ
    ならないので、そこには並び順を付けた ``key`` を使い、読み手に見せる文字は
    ``title`` のまま出す。混ぜると**スライドが 2 枚 1 本に潰れる。**
    """

    order: int                                         # 表示順（1 始まり）
    title: str                                         # タイトルの文字
    part: str                                          # zip の中のパート名
    shown: bool                                        # 非表示（``show="0"``）でない

    @property
    def key(self) -> str:
        return f"{self.order:02d} {self.title}"

    @property
    def stem(self) -> str:
        """書き出す名前。**並び順を頭に付ける** ―― スライドは順序が意味を持ち、
        表題だけでは並べ直せない（シート名と違うのはここである）。"""
        return f"{self.order:02d}_{parse.safe_name(self.title)}"

    @property
    def label(self) -> str:
        """読み手に見せる文字（``3 全体構成``）。**PowerPoint で開いて探す先**
        なので、番号を落とさない。"""
        return f"{self.order} {self.title}"


def _order(path: Path, trouble: list[str]) -> list[Slide]:
    """スライドを**表示順**で。並びも表題もここだけが決める。

    ``ppt/presentation.xml`` の ``p:sldIdLst`` が表示順である ―― パート名の
    ``slide12.xml`` は**作られた順**で、並べ替えても付け替わらない。

    表題はタイトルのプレースホルダから取る。**これは意味の判断ではない** ――
    ``p:ph type="title"`` と資料自身が書いてあるものを写しているだけで、
    Excel のシート名を使うのと同じ立場である。無ければ ``スライド3`` にする
    （章扉・図だけの 1 枚は実際にタイトルを持たない）。
    """
    found: list[Slide] = []
    try:
        with zipfile.ZipFile(path) as archive:
            names = set(archive.namelist())
            if _PRESENTATION not in names:
                return []
            root = ET.fromstring(archive.read(_PRESENTATION))
            targets = parse._rel_targets(archive, "ppt/_rels/presentation.xml.rels")
            # **``or []`` にしない。** 子の無い要素は偽なので、``sldIdLst`` が
            # あって中身が空のときと無いときが同じになる（しかも ElementTree は
            # その書き方を将来やめると言っている）。
            listed = root.find(f"{{{parse._NS['p']}}}sldIdLst")
            for slide in (list(listed) if listed is not None else []):
                relation = slide.get(f"{{{parse._NS['rel']}}}id") or ""
                part = parse._resolve("ppt", targets.get(relation, ""))
                if part not in names:
                    continue
                body = archive.read(part)
                order = len(found) + 1
                found.append(Slide(order=order, title=_title(body, order),
                                   part=part, shown=_shown(body)))
    except (OSError, zipfile.BadZipFile, KeyError, ET.ParseError) as exc:
        trouble.append(str(exc) or exc.__class__.__name__)
        return found
    return found


def _title(body: bytes, order: int) -> str:
    """タイトルのプレースホルダの文字。**無ければ ``スライド3``。**"""
    fallback = f"スライド{order}"
    try:
        root = ET.fromstring(body)
    except ET.ParseError:
        return fallback
    for shape in root.iter(f"{{{parse._NS['p']}}}sp"):
        holder = shape.find(f".//{{{parse._NS['p']}}}ph")
        if holder is None or holder.get("type") not in ("title", "ctrTitle"):
            continue
        text = parse._shape_text(shape).replace("\n", " ").strip()
        if text:
            return text
    return fallback


def _shown(body: bytes) -> bool:
    """``<p:sld show="0">`` は非表示（既定は表示）。"""
    try:
        return (ET.fromstring(body).get("show") or "1") != "0"
    except ET.ParseError:
        return True


def _drawing(related: dict[str, dict[str, list[parse.Part]]],
             key: str) -> parse.Drawing:
    """スライド 1 枚ぶんの図形。**入れ物だけ差し替えて Excel と同じ道を通る。**"""
    drawing = parse.Drawing()
    for part in related["slide"].get(key, []):
        parse._shapes(part, drawing, parse._SLIDE)
    for part in related["diagram"].get(key, []):
        parse._diagram(part.body, drawing)
    for part in related["chart"].get(key, []):
        parse._chart(part.body, drawing)
    return drawing


def _bodies(related: dict[str, dict[str, list[parse.Part]]]) -> dict[str, bytes]:
    """画像の実体 ``{パート名: バイト列}``。**同じ実体は 1 度しか読まない。**"""
    found: dict[str, bytes] = {}
    for parts in related["image"].values():
        for part in parts:
            found.setdefault(part.name, part.body)
    return found


# ── 発表者ノートとコメント ──────────────────────────────────────
def _notes(parts: list[parse.Part]) -> list[str]:
    """発表者ノートの段落。**スライド番号のプレースホルダは落とす。**

    ノートには「なぜそう決めたか」が書かれる ―― スライドの箱は結論だけを
    載せる書き方をするので、**根拠がノートにしか無い**ことが実際にある
    （「A 案にしたのは B 社の保守期限が 2027-03 で切れるため」）。
    """
    found: list[str] = []
    for part in parts:
        try:
            root = ET.fromstring(part.body)
        except ET.ParseError:
            continue
        for shape in root.iter(f"{{{parse._NS['p']}}}sp"):
            holder = shape.find(f".//{{{parse._NS['p']}}}ph")
            if holder is not None and holder.get("type") in ("sldNum", "dt", "ftr"):
                continue
            text = parse._shape_text(shape)
            if text.strip():
                found.append(text)
    return found


def _comments(parts: list[parse.Part], authors: dict[str, str]
              ) -> list[tuple[str, str]]:
    """``(誰がいつ, 本文)``。**新旧 2 つの書かれ方がある**（Excel と同じ事情）。

    2018 年より前の PowerPoint は ``p:cmLst/p:cm``、いまのものは
    ``p188:cm``（modernComment）で書く。**どちらもレビュー指摘の置き場**で、
    決まったことが本文より新しいことも珍しくない。
    """
    found: list[tuple[str, str]] = []
    for part in parts:
        try:
            root = ET.fromstring(part.body)
        except ET.ParseError:
            continue
        for comment in root.iter():
            if not comment.tag.endswith("}cm"):
                continue
            who = authors.get(comment.get("authorId") or "") or "（記入者不明）"
            when = (comment.get("dt") or comment.get("created") or "").strip()
            text = "\n".join(
                node.text or "" for node in comment.iter()
                if node.tag.endswith("}t") or node.tag.endswith("}text")).strip()
            if text:
                found.append((f"{who}{f' {when}' if when else ''}", text))
    return found


def _authors(path: Path) -> dict[str, str]:
    """``{authorId: 表示名}``。**名簿は別のパートにしか無い**（Excel と同じ）。"""
    found: dict[str, str] = {}
    try:
        with zipfile.ZipFile(path) as archive:
            names = set(archive.namelist())
            targets = parse._rel_targets(archive, "ppt/_rels/presentation.xml.rels")
            types = parse._rel_types(archive, "ppt/_rels/presentation.xml.rels")
            for identity, target in targets.items():
                kind = types.get(identity, "")
                if not kind.endswith((_REL_AUTHORS, _REL_MODERN_AUTHORS)):
                    continue
                part = parse._resolve("ppt", target)
                if part not in names:
                    continue
                for element in ET.fromstring(archive.read(part)).iter():
                    identity_ = element.get("id")
                    name = element.get("name") or element.get("initials")
                    if identity_ and name:
                        found[identity_] = name
    except (OSError, zipfile.BadZipFile, KeyError, ET.ParseError):
        return found
    return found


# ── 1 枚ぶんの組み立て ──────────────────────────────────────────
def _document(slide: Slide, relative: Path, drawing: parse.Drawing,
              notes: list[str], comments: list[tuple[str, str]],
              readings: dict[str, ocr.Reading] | None) -> mdio.Doc:
    """スライド 1 枚 = ファイル 1 本。**アンカーの並びは Excel と揃える。**"""
    index = slide.order
    doc = mdio.Doc(title=f"{relative.name} / {slide.label}",
                   source=slide_source(relative, slide.label))
    if drawing.total or drawing.unreadable:
        doc.notes.append(parse._shape_note(index, drawing, readings, parse._SLIDE))
    doc.notes.append(_LAYOUT_NOTE)
    if drawing.series:
        doc.notes.append(parse._chart_note(index, drawing))

    for order, rows in enumerate(drawing.tables, start=1):
        shape = f"{len(rows)} 行 × {max(len(row) for row in rows)} 列"
        doc.chunks.append(mdio.Chunk(
            anchor=f"s{index}-t{order}", at=f"表 {shape}",
            heading=f"表 {shape}", rows=rows))

    if comments:
        doc.chunks.append(mdio.Chunk(
            anchor=f"s{index}-m1", at=f"コメント {len(comments)} 件",
            heading="コメント（スライドには出てこない）", cells=comments))

    if notes:
        # **ノートは別のアンカー**にする。スライドの箱の文字（`g1`）と混ぜると、
        # 「客先に見せた結論」と「そう決めた理由」が同じ出典になる ―― 前者は
        # 合意されたもので、後者は書いた人の手控えである。
        doc.chunks.append(mdio.Chunk(
            anchor=f"s{index}-n1", at=f"ノート {len(notes)} 段落",
            heading="発表者ノート（スライドには出てこない）",
            text="\n\n".join(notes)))

    if drawing.alts:
        doc.chunks.append(mdio.Chunk(
            anchor=f"s{index}-a1", at=f"代替テキスト {len(drawing.alts)} 件",
            heading="代替テキスト（人が書いた説明）", cells=drawing.alts))

    if drawing.series:
        doc.chunks.append(mdio.Chunk(
            anchor=f"s{index}-k1", at=f"グラフ {drawing.charts} 個",
            heading="グラフ（タイトル・系列・参照範囲）",
            rows=[["グラフ", "系列", "分類", "値"],
                  *[list(one) for one in drawing.series]]))

    if drawing.total or drawing.unreadable:
        # **テキストが 1 つも取れなくてもアンカーは出す**（Excel と同じ理由）
        # ―― 出さないと、図だけのスライドがアンカー 0 になり、`freeze` の
        # 未整理一覧にも上がらず `未読取` の宣言先も無くなる。
        empty = (f"テキストの入った図形はありません（{drawing.summary}）。"
                 + (f"画像の実体は `s{index}-i1` に出してあります"
                    if drawing.media else "PowerPoint で開いて読んでください"))
        doc.chunks.append(mdio.Chunk(
            anchor=f"s{index}-g1", at=drawing.summary,
            heading=("図形（テキスト）" if drawing.labels else "図形（テキストなし）"),
            cells=([(f"図形{i}", text)
                    for i, text in enumerate(drawing.labels, start=1)]
                   or [("図形", empty)])))

    if drawing.links:
        doc.chunks.append(mdio.Chunk(
            anchor=f"s{index}-c1", at=f"接続 {len(drawing.links)} 本",
            heading="図形の接続", rows=parse._link_rows(drawing.links)))
    return doc


#: **レイアウトとマスターは読んでいない。** 取ると全スライドに同じ文字が並び、
#: 資料が増えたように見える ―― が、黙ると「資料に無い」と読まれる。
_LAYOUT_NOTE = (
    "スライドのレイアウトとマスターに書かれた文字（ページ番号・フッタ・"
    "定型の見出し）は取っていません。文書番号・版・機密区分がフッタにしか"
    "書かれていないことがあり、そのときこの写しには 1 文字も出てきません "
    "―― 資料に無いのではありません。要るなら PowerPoint で"
    "「表示 → スライドマスター」を開いて確かめてください。")


# ── 申告 ────────────────────────────────────────────────────────
def _hidden_note(path: Path, hidden: list[Slide]) -> list[Finding]:
    """**隠したスライドは読まない（が、あったことは言う）。**

    旧版・予備・客先ごとに出し分ける 1 枚を隠して配るのは実案件でごく普通で、
    Excel の非表示シート（`P003`）とまったく同じ事情である ―― 中身が要るなら
    再表示して取り込み直すという判断は、機械ではなく人がする。
    """
    if not hidden:
        return []
    listed = "／".join(one.label for one in hidden)
    return [Finding("warn", "P003", path.name,
                    f"非表示のスライドが {len(hidden)} 枚あります（{listed}）。"
                    "読んでいないので、この写しには 1 文字も出てきません。"
                    "「旧版」を隠しただけの資料は普通にあるので、中身が要るなら"
                    "PowerPoint で再表示してから取り込み直してください。")]


def _macro_note(path: Path) -> list[Finding]:
    """`.pptm` のマクロ。**中身は取らない**（`.xlsm` と同じ理由）。"""
    try:
        with zipfile.ZipFile(path) as archive:
            if _VBA_PART not in set(archive.namelist()):
                return []
    except (OSError, zipfile.BadZipFile):
        return []
    return [Finding("warn", "P006", path.name,
                    "マクロ（VBA）が入っています。中身は取っていません"
                    f"（`{_VBA_PART}` は zip の中にありますが、zip としては"
                    "開けません）。仕様が要るなら PowerPoint の VBE（Alt+F11）で"
                    "開いて読むか、作成者に確認してください。")]


def _properties_note(path: Path) -> list[Finding]:
    """ファイルのプロパティ。**スライドにも表にも出てこない**（Excel と同じ）。"""
    try:
        with zipfile.ZipFile(path) as archive:
            core = parse._properties(archive, set(archive.namelist()))
    except (OSError, zipfile.BadZipFile):
        return []
    if not core:
        return []
    listed = "／".join(f"{label} {value}" for label, value in core)
    return [Finding("warn", "P005", path.name,
                    f"ファイルのプロパティ（{listed}）。スライドにも表にも"
                    "出てきません（日時は UTC）。改訂履歴は人が書いた申告な"
                    "ので、そこに無い更新がここにだけ残っていることがあります。")]


def _gap_note(path: Path, trouble: list[str]) -> list[Finding]:
    """**辿れなかった関係は「図の無い資料」と見分けが付かない**（`P008` と同じ）。"""
    if not trouble:
        return []
    return [Finding("warn", "P008", path.name,
                    "このファイルのスライドは 1 枚も読めていません。zip の中の"
                    f"関係を辿れませんでした（{trouble[0]}）。パース結果は空の"
                    "資料として出てきますが、資料に無いのではありません。"
                    "PowerPoint で開いて確かめてください。")]


def _nothing_note(path: Path, order: list[Slide],
                  made: list[tuple[Path, mdio.Doc]],
                  hidden: list[Slide]) -> list[Finding]:
    """**1 本も出なかった資料は、置いていない資料と区別が付かない**（`P009`）。"""
    if made:
        return []
    detail = [f"スライド {len(order)} 枚"]
    if hidden:
        detail.append(f"うち非表示 {len(hidden)} 枚")
    return [Finding("warn", "P009", path.name,
                    f"パース結果が 1 本も出ませんでした（{'／'.join(detail)}）。"
                    "図形もノートもコメントも無いスライドは出さない決まりなので、"
                    "白紙だけの資料ならこれで正しいのですが、中身があるはずなら"
                    "原本を開いて確かめてください。")]
