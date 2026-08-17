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

#: 欧文の送り幅。**ここを書かないと ASCII が全角送りで出る。**
#:
#: 幅を 1 つも書かないと全部の字が ``/DW`` になる ―― 日本語は全角なので合うが、
#: **`No` も `2026-07-15` も全角 1 文字ぶん送られる**ので、空白で桁を揃えた行
#: （実物の PDF の表はそうなっている）が倍に伸びて**紙の右から出る。**実測で
#: 検収仕様書の「判定」列が `MediaBox` の外に落ちていた ―― パースは位置を持った
#: 文字を取るだけなので通り、**開いた人にだけ列が消えて見える。**
#:
#: ``UniJIS-UCS2-H`` は ASCII を Adobe-Japan1 のプロポーショナル欧文
#: （CID 1〜95）へ送るので、そこを半角（500）に揃える。全角のちょうど半分に
#: なるので、**空白で揃えた桁が日本語の端末と同じ見え方になる。**
_ROMAN_FIRST, _ROMAN_LAST, _HALF = 1, 95, 500


def build(path: Path, spec: dict[str, Any]) -> Path:
    """検体 1 冊を ``.pdf`` として書く。"""
    pages = spec.get("ページ") or []
    toc = [(str(one["しおり"]), int(one.get("深さ") or 0), index + 1)
           for index, one in enumerate(pages) if one.get("しおり")]

    #: オブジェクト番号を先に決める。**相互参照表は番号順に並ぶ**ので、
    #: 後から足すと全部の offset を数え直すことになる。
    catalog, tree, font, descendant, descriptor = 1, 2, 3, 4, 5
    first = 6
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
        f"/Supplement 6 >> /FontDescriptor {descriptor} 0 R "
        f"/DW 1000 /W [{_ROMAN_FIRST} {_ROMAN_LAST} {_HALF}] >>")
    objects[descriptor] = (
        f"<< /Type /FontDescriptor /FontName /{_BASE_FONT} /Flags 6 "
        "/FontBBox [-437 -340 1147 1317] /ItalicAngle 0 /Ascent 1137 "
        "/Descent -349 /CapHeight 742 /StemV 80 >>")

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
        objects.update(_outline(toc, outline_root, outline_ids, page_ids))

    path.write_bytes(_assemble(objects, catalog))
    return path


def _outline(toc: list[tuple[str, int, int]], root: int, ids: dict[int, int],
             page_ids: dict[int, int]) -> dict[int, str]:
    """しおりの**木**。``深さ`` を書けば子しおりになる。

    実物の検収仕様書・他社製品の仕様書は章の下に節のしおりを持っている ――
    平らなしおりしか書けなかったあいだ、`pdf.py` の「深い階層では割らない」
    という規律は**1 度も試されていなかった**（章より細かく割ると 1 本が
    数段落になり、出典が細かすぎて資料の姿が見えなくなる）。

    PDF のしおりは双方向の連結リストである ―― ``/First`` ``/Last`` ``/Count``
    を親に、``/Prev`` ``/Next`` ``/Parent`` を子に置く。1 つでも欠けると
    読み手はしおりの木を辿れない（そして**しおりの無い PDF と同じに見える**）。
    """
    kids: dict[int, list[int]] = {root: []}
    parents: dict[int, int] = {}
    stack: list[int] = []                            # 深さごとの直近のしおり
    for order, (_shown, depth, _page) in enumerate(toc):
        identity = ids[order]
        parent = stack[depth - 1] if depth and len(stack) >= depth else root
        parents[identity] = parent
        kids.setdefault(parent, []).append(identity)
        kids.setdefault(identity, [])
        del stack[depth:]                            # 浅いところへ戻った
        stack.append(identity)

    made: dict[int, str] = {
        root: (f"<< /Type /Outlines /First {kids[root][0]} 0 R "
               f"/Last {kids[root][-1]} 0 R /Count {len(toc)} >>")}
    for order, (title, _depth, page_number) in enumerate(toc):
        identity = ids[order]
        siblings = kids[parents[identity]]
        place = siblings.index(identity)
        links = ""
        if place:
            links += f" /Prev {siblings[place - 1]} 0 R"
        if place + 1 < len(siblings):
            links += f" /Next {siblings[place + 1]} 0 R"
        if kids[identity]:
            # **正の ``/Count`` は「開いた状態」**である（負にすると畳まれる）。
            links += (f" /First {kids[identity][0]} 0 R "
                      f"/Last {kids[identity][-1]} 0 R "
                      f"/Count {len(kids[identity])}")
        made[identity] = (
            f"<< /Title {_title(title)} /Parent {parents[identity]} 0 R{links} "
            f"/Dest [{page_ids[page_number - 1]} 0 R /Fit] >>")
    return made


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
