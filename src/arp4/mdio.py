"""アンカー付き Markdown ―― パース結果の器。

**編集されることが前提**なので、編集しやすさを機械可読性より優先する。日本語の表を
YAML の配列で持つ形はエージェントも人も読みにくく、diff も行単位でうるさい。

アンカーは HTML コメントで持ち、本文を汚さない::

    ## 表 B8:J20  <!-- a:s1-t1 at=B8:J20 -->

    | 論理名 | 物理名 | 型 |
    |---|---|---|
    | 受注番号 | ORDER_NO | 文字列 |

``a:`` が識別子、``at=`` が元資料の位置である。**行番号ではなく ID** にするのは、
OCR を 1 行直しただけでそれ以降の出典が全部ずれるのを避けるためである。

読み戻しは「アンカー行から次のアンカー行まで」を本文とみなす。見出しの文言や表の
体裁が編集で変わっても、アンカーさえ残っていれば出典は追える ―― **編集に強い形を
選んでいる。**
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

#: 書き出す拡張子。
EXT = ".md"

#: アンカーのコメント。``at=`` は省略可（読み戻し側で空になる）。
#:
#: **``at=`` に空白を許す。** 番地（``B8:J20``）だけを想定して ``[^\\s>]+`` に
#: していたが、図形のアンカーは ``at=図形 19 個`` と書く ―― 空白で切れるので
#: 行全体がマッチせず、**``s4-g1`` は書かれているのに読み手から見えなかった。**
#: その結果、図形は `freeze` の未整理一覧（G001）に上がらず、`未読取` を宣言しても
#: G004「アンカーがありません」で弾かれていた。**読めていないものが静かに消える**
#: という、いちばん避けたい壊れ方である。
_ANCHOR = re.compile(r"<!--\s*a:(?P<id>[^\s>]+)(?:\s+at=(?P<at>[^>]*?))?\s*-->")

#: 見出し行のコメント（``<!-- source: … -->``）。
_SOURCE = re.compile(r"<!--\s*source:\s*(?P<source>.+?)\s*-->")


@dataclass
class Chunk:
    """パース結果 1 か所。**表かテキストかは提示上の都合**であって意味ではない。"""

    anchor: str
    at: str = ""
    heading: str = ""
    #: 表として出すとき。1 行目も含めて**そのまま**（見出し行を決めつけない）。
    rows: list[list[str]] = field(default_factory=list)
    #: テキストとして出すとき。``(セル番地, 値)``。
    cells: list[tuple[str, str]] = field(default_factory=list)
    #: **原文をそのまま**出すとき。Markdown の資料はこれを使う ―― 原本が既に
    #: 読める形なので、表や箇条書きに組み直すところが 1 つも無い。組み直すと
    #: 入れ子の箇条書き・コードブロック・引用が平らになり、**資料に書いて
    #: あったことが消える**（Excel は「セルの面」を紙に寄せ直す必要があるので
    #: :attr:`rows` / :attr:`cells` を使う ―― 出自で必要な作業が違う）。
    text: str = ""


@dataclass
class Doc:
    """パース結果 1 ファイル。"""

    title: str
    source: str
    chunks: list[Chunk] = field(default_factory=list)
    #: **機械が読めなかったもの**の申告（図形・画像）。アンカーは持たない
    #: ―― 出典にはならないが、「資料に無い」と「読めていない」を読み手が
    #: 取り違えないために必ず出す。
    notes: list[str] = field(default_factory=list)


@dataclass
class Anchor:
    """読み戻したアンカー 1 件。"""

    id: str
    at: str
    body: str

    @property
    def text(self) -> str:
        """記号を落とした本文。**語の包含検査**に使う。"""
        return re.sub(r"[|#`*\-\s]+", " ", self.body).strip()


@dataclass
class ParsedFile:
    """読み戻したパース結果。"""

    path: Path
    title: str
    source: str
    anchors: list[Anchor] = field(default_factory=list)

    @property
    def by_id(self) -> dict[str, Anchor]:
        return {a.id: a for a in self.anchors}


# ── 書き出し ────────────────────────────────────────────────────
def dump(doc: Doc) -> str:
    """Markdown 文字列にする。**改行は LF**（Windows でも diff を揃える）。"""
    out: list[str] = [f"# {_safe_comment(doc.title)}", ""]
    if doc.source:
        out += [f"<!-- source: {_safe_comment(doc.source)} -->", ""]
    for note in doc.notes:
        # 申告にも資料由来の文字列が混ざる（リンク先のファイル名・エラー値）ので、
        # 表のセルと同じくアンカーを偽造できない形にしてから出す。
        out += [f"> {_safe_comment(note)}", ""]

    for chunk in doc.chunks:
        # **見出しにも資料由来の文字列が入る。** Excel の見出しは番地から作った
        # 文字列、コードの見出しはクラス名だったので長らく安全だったが、Markdown の
        # 資料は**見出しの行がそのまま資料の文字**である ―― `## 偽造 <!-- a:s9-t9 -->`
        # と書かれた資料を出すと、読み戻し側は先に出てくる偽アンカーを拾い、
        # **本物の塊が丸ごと別の id の中へ消える**。セルだけ守っても穴は塞がらない。
        heading = _safe_comment(chunk.heading) or chunk.at or chunk.anchor
        marker = (f"<!-- a:{chunk.anchor}"
                  + (f" at={_safe_comment(chunk.at)}" if chunk.at else "") + " -->")
        out += [f"## {heading}  {marker}", ""]
        if chunk.rows:
            out += _table(chunk.rows) + [""]
        for at, value in chunk.cells:
            out += [f"- `{at}` {_inline(value)}"]
        if chunk.cells:
            out.append("")
        if chunk.text:
            # **原文をそのまま**。落とすのはアンカーの偽造だけで、記号も字下げも
            # 触らない ―― ここを整形すると、原本と読み比べたときに差が出る。
            out += [_safe_comment(chunk.text).rstrip(), ""]
    return "\n".join(out).rstrip() + "\n"


def write(path: Path, doc: Doc) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(dump(doc), encoding="utf-8", newline="\n")
    return path


def _table(rows: list[list[str]]) -> list[str]:
    """GFM の表。**1 行目を見出しとして出すが、意味を決めているわけではない**
    （GFM に見出しの無い表が書けないだけ。番地は ``at=`` に残っている）。"""
    width = max((len(row) for row in rows), default=0)
    if width == 0:
        return []
    padded = [list(row) + [""] * (width - len(row)) for row in rows]
    lines = ["| " + " | ".join(_inline(c) for c in padded[0]) + " |",
             "|" + "|".join(["---"] * width) + "|"]
    lines += ["| " + " | ".join(_inline(c) for c in row) + " |" for row in padded[1:]]
    return lines


def _inline(value: Any) -> str:
    """表のセルに入れられる形へ。**中身は落とさない**（改行も記号で残す）。

    落とすのは行末の空白だけである。**行頭の字下げは残す** ―― 項目定義書の
    「項目名」列は字下げで親子を表すのが日本の設計書の慣習で、ここで
    ``strip()`` していたぶん階層がまるごと平らになっていた。表として描画すると
    見えなくなるが、読むのは Markdown の原文なので残っていれば伝わる。
    """
    text = "" if value is None else str(value)
    text = text.replace("\\", "\\\\").replace("|", "\\|").replace("\n", "<br>")
    return _safe_comment(text).rstrip()


#: セルの値が**アンカーを偽造できないように**する。アンカーは HTML コメントで
#: 持っているので、資料のセルに ``<!-- a:s9-t9 -->`` と書いてあると、書き出した
#: Markdown を読み戻した側にはそれが**本物のアンカーとして見える** ―― 表の途中に
#: 無い塊が生え、そこから先の本文が別のアンカーの中身になる。HTML の仕様書・
#: テンプレートの設計書には普通に出てくる文字列である。
#:
#: 実体参照に置き換えるのは、**画面に見えている表記を変えない**ためである
#: （Markdown でも HTML でも `<!--` と表示される）。文字を捨てる・記号を足すと
#: 「セルの値をそのまま出す」という約束のほうが破れる。
_COMMENT_OPEN = "<!--"
_COMMENT_CLOSE = "-->"


def _safe_comment(text: str) -> str:
    return text.replace(_COMMENT_OPEN, "&lt;!--").replace(_COMMENT_CLOSE, "--&gt;")


# ── 読み戻し ────────────────────────────────────────────────────
def read(path: Path) -> ParsedFile:
    """アンカー単位で読み戻す。**見出しや表の体裁が変わっていても動く。**"""
    text = path.read_text(encoding="utf-8")
    lines = text.splitlines()

    title = ""
    source = ""
    anchors: list[Anchor] = []
    current: Anchor | None = None
    body: list[str] = []

    for line in lines:
        if not title and line.startswith("# "):
            title = line[2:].strip()
            continue
        if not source:
            found = _SOURCE.search(line)
            if found:
                source = found.group("source")
                continue

        marker = _ANCHOR.search(line)
        if marker:
            if current is not None:
                current.body = "\n".join(body).strip()
                anchors.append(current)
            current = Anchor(id=marker.group("id"), at=marker.group("at") or "", body="")
            head = _ANCHOR.sub("", line).lstrip("# ").strip()
            body = [head] if head else []
            continue
        if current is not None:
            body.append(line)

    if current is not None:
        current.body = "\n".join(body).strip()
        anchors.append(current)

    return ParsedFile(path=path, title=title, source=source, anchors=anchors)


#: GFM が要求する区切り行（``|---|---|``）。**資料には無い行**なので落とす。
_SEPARATOR = re.compile(r"^[\s:|-]+$")


def rows(anchor: Anchor) -> list[list[str]]:
    """アンカーの表を**セルの値に戻す**。1 塊 1 表（:func:`dump` がそう書く）。

    **1 行目を見出しと決めつけない** ―― :class:`Chunk` の約束と同じである。
    見出しの意味を決めるのは読む側であって、ここは体裁を剥がすだけ。

    落とすのは区切り行だけで、**2 行目に限って**見る。素の ``-`` はセルの値として
    普通に出てくる（「担当者 ｜ - ｜ 未定」）ので、どこの行でも落とすことにすると
    **資料に書いてあった行が消える。**

    :func:`_safe_comment` の置き換え（``<!--`` → ``&lt;!--``）は戻さない。戻すと
    読み戻した文字列がまたアンカーを偽造できる形になる ―― 画面に見えている表記は
    どちらも ``<!--`` なので、読み手から見た資料の中身は変わらない。
    """
    out: list[list[str]] = []
    position = 0
    for line in anchor.body.splitlines():
        line = line.strip()
        if not line.startswith("|"):
            position = 0                  # 表が切れた（次に出てくるのは別の表）
            continue
        position += 1
        if position == 2 and _SEPARATOR.fullmatch(line):
            continue
        out.append(_cells(line))
    return out


def _cells(line: str) -> list[str]:
    """1 行をセルへ割る。**逃がした ``|`` で割らない**（:func:`_inline` の逆）。

    正規表現で「直前が ``\\`` でない ``|``」を探す手はあるが、値の末尾が ``\\``
    だったとき（``a\\`` は ``a\\\\`` と書かれる）に**区切りを逃がしと読み違える**。
    1 文字ずつ見るほうが短い。
    """
    cells: list[str] = []
    buffer: list[str] = []
    index = 0
    while index < len(line):
        char = line[index]
        if char == "\\" and index + 1 < len(line):
            buffer.append(line[index + 1])
            index += 2
            continue
        if char == "|":
            cells.append("".join(buffer))
            buffer = []
            index += 1
            continue
        buffer.append(char)
        index += 1
    cells.append("".join(buffer))
    # 先頭と末尾は ``|`` の外側（空文字）なので落とす。
    return [cell.strip().replace("<br>", "\n") for cell in cells[1:-1]]


# ── 絵の貼り付け ────────────────────────────────────────────────
#: 絵へのリンク（素の Markdown で持つ）。アンカーのコメントへ足さないのは、
#: **出典の書式を増やさない**ため ―― `a:` と `at=` の 2 つだけを守る約束にしておく。
_IMAGE = re.compile(r"^!\[(?P<text>[^\]]*)\]\((?P<link>[^)]*)\)\s*$")


def images(anchor: Anchor) -> list[str]:
    """このアンカーに貼られている絵のパス。"""
    return [found.group("link").strip()
            for line in anchor.body.splitlines()
            if (found := _IMAGE.match(line.strip()))]


def attach(path: Path, anchor: str, links: list[tuple[str, str]]) -> bool:
    """アンカーの見出し直下へ絵を貼る。``links`` は ``(説明, 相対パス)``。

    **既にある絵は残す。** 範囲を狭めて撮り直した拡大図は全体図の代わりではなく
    **追加の根拠**であり、次にシート全体を撮り直したときに黙って消えては困る
    （同じパスは二重に貼らない）。

    **ただし実体の無い絵は落とす。** 撮り方を変えて枚数が減ると古いタイルへの
    リンクだけが残り、開けないリンクは「絵がある」と嘘をつく。

    パース結果は**編集される前提**なので、触るのは**見出しの直後にある絵の行だけ**
    に限る。人がどこかへ貼った絵まで巻き込むと、編集を守るという約束が崩れる。
    書き換えたら True。
    """
    if not path.is_file():
        return False
    lines = path.read_text(encoding="utf-8").splitlines()
    for index, line in enumerate(lines):
        marker = _ANCHOR.search(line)
        if marker and marker.group("id") == anchor:
            merged = _merged(path, lines, index + 1, links)
            return _replace(path, lines, index + 1, merged)
    return False


def _merged(path: Path, lines: list[str], start: int,
            links: list[tuple[str, str]]) -> list[tuple[str, str]]:
    """既にある絵（実体があるもの）を**順番を変えずに**残し、新しいものを足す。

    順番を保つのは、撮り直すたびに全体図と拡大図が入れ替わると diff が騒ぎ、
    読む側も「増えたのか並び替わったのか」が分からなくなるためである。
    """
    fresh = {link: text for text, link in links}
    final: list[tuple[str, str]] = []
    seen: set[str] = set()
    index = start
    while index < len(lines) and (not lines[index].strip()
                                  or _IMAGE.match(lines[index].strip())):
        found = _IMAGE.match(lines[index].strip())
        if found:
            link = found.group("link").strip()
            if link in fresh:                       # 撮り直し ―― 同じ位置に残す
                final.append((fresh[link], link))
                seen.add(link)
            elif (path.parent / link).is_file():
                final.append((found.group("text"), link))
        index += 1
    return final + [(text, link) for text, link in links if link not in seen]


def _replace(path: Path, lines: list[str], start: int,
             links: list[tuple[str, str]]) -> bool:
    """見出し直後の「空行＋絵の行」の塊を、新しいものへ置き換える。"""
    end = start
    while end < len(lines) and (not lines[end].strip()
                                or _IMAGE.match(lines[end].strip())):
        end += 1
    # 絵がまだ無いときは、見出し直後の**空行 1 本だけ**を置き換える（貼る塊が
    # 空行を含んでいるので、丸ごと残すと空行が二重になる）。
    if not any(_IMAGE.match(lines[i].strip()) for i in range(start, end)):
        end = start + 1 if end > start else start

    block = [""] + [f"![{text}]({link})" for text, link in links] + [""]
    updated = lines[:start] + block + lines[end:]
    while updated and not updated[-1].strip():
        updated.pop()
    body = "\n".join(updated) + "\n"
    if body == "\n".join(lines).rstrip() + "\n":
        return False
    path.write_text(body, encoding="utf-8", newline="\n")
    return True


def scan(directory: Path) -> list[Path]:
    """``parsed/`` 配下の Markdown。**並びは決定的**（差分をノイズにしない）。"""
    if not directory.is_dir():
        return []
    return sorted(p for p in directory.rglob(f"*{EXT}") if p.is_file())
