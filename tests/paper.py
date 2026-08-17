"""検体の ``ページ:`` を **実物として開ける .pdf** に組み立てる。

:mod:`deck` / :mod:`document` と同じ切り方である ―― 検体の YAML に書くのは
**ページごとの行としおりだけ**で、PDF の作法（オブジェクト・相互参照表・
フォント辞書・アウトライン）はここが埋める。

**日本語は予め定義された CMap で書く。** フォントを埋め込まずに日本語を出す
道は 1 つしかない ―― ``/Encoding /UniJIS-UCS2-H`` を使い、本文の文字列を
**UTF-16BE のまま**置く。読む側（pdfium も Acrobat も）はこの CMap を知って
いるので字に戻せるし、表示のほうは閲覧環境のフォントで代替される。

**テキスト層の無いページも書ける**（``スキャン: true``）。中身の無いページを
1 枚置くだけで、「紙をスキャンしただけの PDF」と機械には同じに見える ――
実物のスキャン PDF には絵が乗っているが、**arp4 が見ているのは「字が取れない」
という事実のほうである。**
"""

from __future__ import annotations

from pathlib import Path
from typing import Any

#: 用紙（A4 相当、単位はポイント）。
_WIDTH, _HEIGHT = 595, 842

#: 本文の始まりと行送り。
_LEFT, _TOP, _LEADING, _SIZE = 60, 780, 20, 12

#: 日本語を**埋め込まずに**書くための予め定義されたフォントと CMap。
_BASE_FONT = "KozMinPr6N-Regular"
_CMAP = "UniJIS-UCS2-H"


def build(path: Path, spec: dict[str, Any]) -> Path:
    """検体 1 冊を ``.pdf`` として書く。"""
    pages = spec.get("ページ") or []
    toc = [(str(one["しおり"]), index + 1)
           for index, one in enumerate(pages) if one.get("しおり")]

    #: オブジェクト番号を先に決める。**相互参照表は番号順に並ぶ**ので、
    #: 後から足すと全部の offset を数え直すことになる。
    catalog, tree, font, descendant = 1, 2, 3, 4
    first = 5
    page_ids = {i: first + i * 2 for i in range(len(pages))}
    stream_ids = {i: first + i * 2 + 1 for i in range(len(pages))}
    outline_root = first + len(pages) * 2
    outline_ids = {i: outline_root + 1 + i for i in range(len(toc))}

    objects: dict[int, str] = {}
    root = f"<< /Type /Catalog /Pages {tree} 0 R"
    if toc:
        root += f" /Outlines {outline_root} 0 R /PageMode /UseOutlines"
    objects[catalog] = root + " >>"
    kids = " ".join(f"{page_ids[i]} 0 R" for i in range(len(pages)))
    objects[tree] = f"<< /Type /Pages /Kids [{kids}] /Count {len(pages)} >>"
    objects[font] = (f"<< /Type /Font /Subtype /Type0 /BaseFont /{_BASE_FONT} "
                     f"/Encoding /{_CMAP} /DescendantFonts [{descendant} 0 R] >>")
    objects[descendant] = (
        f"<< /Type /Font /Subtype /CIDFontType0 /BaseFont /{_BASE_FONT} "
        "/CIDSystemInfo << /Registry (Adobe) /Ordering (Japan1) "
        "/Supplement 6 >> /DW 1000 >>")

    for index, page in enumerate(pages):
        body = _content([] if page.get("スキャン") else (page.get("行") or []))
        objects[page_ids[index]] = (
            f"<< /Type /Page /Parent {tree} 0 R "
            f"/MediaBox [0 0 {_WIDTH} {_HEIGHT}] "
            f"/Resources << /Font << /F1 {font} 0 R >> >> "
            f"/Contents {stream_ids[index]} 0 R >>")
        objects[stream_ids[index]] = (f"<< /Length {len(body)} >>\nstream\n"
                                      + body.decode("utf-8") + "\nendstream")

    if toc:
        objects[outline_root] = (
            f"<< /Type /Outlines /First {outline_ids[0]} 0 R "
            f"/Last {outline_ids[len(toc) - 1]} 0 R /Count {len(toc)} >>")
        for order, (title, page_number) in enumerate(toc):
            links = ""
            if order:
                links += f" /Prev {outline_ids[order - 1]} 0 R"
            if order + 1 < len(toc):
                links += f" /Next {outline_ids[order + 1]} 0 R"
            objects[outline_ids[order]] = (
                f"<< /Title {_title(title)} /Parent {outline_root} 0 R{links} "
                f"/Dest [{page_ids[page_number - 1]} 0 R /Fit] >>")

    path.write_bytes(_assemble(objects, catalog))
    return path


def _content(lines: list[Any]) -> bytes:
    """本文の描画命令。**行はそのまま 1 行ずつ置く。**"""
    if not lines:
        return b""                                   # スキャンしたページ（字が無い）
    out = ["BT", f"/F1 {_SIZE} Tf", f"1 0 0 1 {_LEFT} {_TOP} Tm",
           f"{_LEADING} TL"]
    for line in lines:
        out.append(f"{_text(str(line))} Tj")
        out.append("T*")
    out.append("ET")
    return "\n".join(out).encode("utf-8")


def _text(value: str) -> str:
    """**本文**の文字列（``/UniJIS-UCS2-H`` が読み戻す）。**BOM は付けない。**

    素の ``(…)`` 文字列にすると 1 バイト＝ 1 文字と読まれ、日本語は化ける。
    """
    return "<" + value.encode("utf-16-be").hex().upper() + ">"


def _title(value: str) -> str:
    """**しおりの表題**。本文と違い、ここは **BOM（``FEFF``）が要る。**

    PDF の「文書文字列」は既定で PDFDocEncoding（1 バイト）と読まれる ――
    BOM を落とすと、`1 適用範囲` が `1  ’iu({ÄVò` になってそのまま
    **ファイル名になる**（節に割った写しの名前が化ける）。本文のほうは
    フォントの CMap が符号化を決めるので、同じことをすると逆に化ける。
    """
    return "<FEFF" + value.encode("utf-16-be").hex().upper() + ">"


def _assemble(objects: dict[int, str], catalog: int) -> bytes:
    """オブジェクトを並べ、**相互参照表**を付ける。

    ここを外すと「壊れた PDF」になる ―― pdfium は寛容なので読めてしまうことが
    あるが、**Acrobat で開けない検体は検体にならない**（人が中身を確かめられない）。
    """
    out = bytearray(b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n")
    offsets: dict[int, int] = {}
    for number in sorted(objects):
        offsets[number] = len(out)
        out += f"{number} 0 obj\n{objects[number]}\nendobj\n".encode("utf-8")

    start = len(out)
    top = max(objects) + 1
    out += f"xref\n0 {top}\n".encode("ascii")
    out += b"0000000000 65535 f \n"
    for number in range(1, top):
        if number in offsets:
            out += f"{offsets[number]:010d} 00000 n \n".encode("ascii")
        else:
            out += b"0000000000 65535 f \n"
    out += (f"trailer\n<< /Size {top} /Root {catalog} 0 R >>\n"
            f"startxref\n{start}\n%%EOF\n").encode("ascii")
    return bytes(out)
