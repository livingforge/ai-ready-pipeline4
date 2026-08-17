"""``0_元資料と設計書の対応`` ―― **届いた資料のどれが使われなかったか。**

穴の 1 枚（:mod:`arp4.holes`）は**正本に何が無いか**を言う。言っていないのは
その反対側 ―― **原本の写しのうち、設計書のどこにも出なかったもの**である。

実測（r001）: ``parsed/`` に **201 本**の写しがあり、設計書 12 冊の出典セルは
**580 個**。どの写しが 1 度も出ていないかは、どの生成物にも書かれていない。
資料を渡した側から見ると、これがいちばん先に知りたいことである ――
「20 冊渡したのに設計書に出てくるのは 6 冊ぶんだった」は、設計の穴ではなく
**受け渡しの穴**で、次の一手が正本を直すことではない。

**ここは新しい判断をしない**（穴の 1 枚と同じ規律）。出るのは 2 つの事実の
突き合わせだけである ―― ``parsed/`` に写しがあるか、正本のレコードがその写しを
出典に挙げているか。**「使われていない」は「不要だった」ではない**（表紙・
改訂履歴のように出典にならないのが正しい写しもある）ので、理由は書かず、
**整理結果があるかどうか**という次の一手が分かる事実だけを添える。
"""

from __future__ import annotations

import html as html_module
from pathlib import Path
from typing import Any, Iterable

from arp4 import mdio
from arp4 import organized as organized_module
from arp4 import page as page_module
from arp4.paths import Round
from arp4.spec import Spec

#: 生成するファイル名。目次と各設計書の概要からここへリンクする。
STEM = "0_元資料と設計書の対応"

#: 一覧に出す上限。**超えた分は件数で言う**（→ :mod:`arp4.holes` と同じ規律）。
_LIMIT = 60

#: 1 冊の中に複数の写しが入る形式。**写しの親が原本**になる。
#: :data:`arp4.parse.SUPPORTED` のうち**中で割れるもの**と揃えること ――
#: ここから漏れると、1 冊が写しの枚数ぶん別々の原本として数えられ、
#: 「20 冊渡したのに 6 冊ぶんしか出ていない」という表が**冊数から嘘になる。**
_BOOKS = (".xlsx", ".xlsm", ".pptx", ".pptm", ".docx", ".docm", ".pdf")


def origin_of(file: str) -> tuple[str, str]:
    """出典の ``file`` を ``(原本, その中の位置)`` に割る。

    ``資料/A/基本設計書.xlsx/受注テーブル`` → ``(資料/A/基本設計書.xlsx, 受注テーブル)``
    ``src/arp4/build.py``                   → ``(src/arp4, build.py)``

    Excel・PowerPoint・Word・PDF を特別扱いするのは、**1 冊の中にシート／
    スライド／節が何枚も入る**という原本側の事実だからである。それ以外は
    ファイルが原本なので、束ねる単位はディレクトリにする ―― Java 114 本を
    114 行並べると、表が「どの資料が届いたか」ではなく ``ls`` になる
    （→ 決定 19 で要件定義書に起きたのと同じ壊れ方）。
    """
    parts = file.replace("\\", "/").split("/")
    for index, part in enumerate(parts):
        if part.lower().endswith(_BOOKS):
            return "/".join(parts[:index + 1]), "/".join(parts[index + 1:])
    return ("/".join(parts[:-1]) or ".", parts[-1])


def used_sources(spec: Spec) -> dict[tuple[str, str], int]:
    """正本が出典に挙げている ``(ラウンド, 写し)`` → 挙げているレコード数。"""
    counted: dict[tuple[str, str], int] = {}
    for record in [*spec.items, *spec.relations]:
        for entry in record.get("source") or []:
            if not isinstance(entry, dict) or not entry.get("file"):
                continue
            key = (str(entry.get("round") or ""), str(entry["file"]))
            counted[key] = counted.get(key, 0) + 1
    return counted


def copies(spec: Spec) -> dict[tuple[str, str], Round]:
    """``parsed/`` に実在する写し ``(ラウンド, 写し)`` → そのラウンド。"""
    found: dict[tuple[str, str], Round] = {}
    if spec.paths is None:
        return found
    for round_ in spec.paths.rounds():
        if not round_.parsed.is_dir():
            continue
        for path in sorted(round_.parsed.rglob(f"*{mdio.EXT}")):
            relative = path.relative_to(round_.parsed)
            found[(round_.name, relative.with_suffix("").as_posix())] = round_
    return found


def write(spec: Spec, out_dir: Path,
          by_document: dict[tuple[str, str], set[str]] | None = None,
          ) -> list[Path]:
    """``out/0_元資料と設計書の対応.{md,html}`` を書く。

    ``by_document`` は ``(ラウンド, 写し)`` → **その写しから起こしたレコードが
    行として出た**設計書の題（:func:`arp4.publish.publish` が組み立てる）。
    渡さなければ列を出さない ―― **無いものを空欄で出すと「どの設計書にも
    出ていない」と読める。**

    ``None``（渡していない）と ``{}``（渡したが 1 枚も出ていない）は**別物**で
    ある。後者は本当に 0 枚なので、列を落とすとその事実まで消える。
    """
    sections = _sections(spec, by_document)

    lines = [f"# {STEM[2:]}", "",
             "> この文書は生成物です。**直接編集しないでください**"
             "（`arp4 publish` で再生成されます）。", "",
             "届いた資料の写し（`parsed/`）と、正本が出典に挙げているものを"
             "突き合わせた表です。「使われていない」は「不要だった」では"
             "ありません。表紙や改訂履歴のように、出典にならないのが正しい"
             "写しもあります。ここは新しい判断をしていません。", ""]
    for heading, intro, rows, columns in sections:
        lines += [f"## {heading}", ""]
        if intro:
            lines += [intro, ""]
        if not rows:
            lines += ["（該当なし）", ""]
            continue
        lines.append("| " + " | ".join(columns) + " |")
        lines.append("|" + "|".join(["---"] * len(columns)) + "|")
        for row in rows[:_LIMIT]:
            lines.append("| " + " | ".join(_cell(c) for c in row) + " |")
        if len(rows) > _LIMIT:
            lines.append(f"| ほか {len(rows) - _LIMIT} 件 |"
                         + " |" * (len(columns) - 1))
        lines.append("")

    md = out_dir / f"{STEM}.md"
    md.write_text("\n".join(lines).rstrip() + "\n", encoding="utf-8", newline="\n")

    escape = html_module.escape
    parts = page_module.head(STEM[2:])
    parts.append(page_module.toolbar("写しの名前で絞り込み"))
    parts += ['<div class="wrap">',
              f'<p class="meta"><a href="目次.html"{page_module.NEW_TAB}>'
              "← 生成した設計書（目次）</a></p>",
              f"<h1>{escape(STEM[2:])}</h1>",
              '<p class="meta">届いた資料の写しと、正本が出典に挙げているものを'
              "突き合わせた表です。「使われていない」は「不要だった」では"
              "ありません。</p>"]
    for heading, intro, rows, columns in sections:
        parts.append(f"<h2>{escape(heading)}</h2>")
        if intro:
            parts.append(f'<p class="meta">{escape(_plain(intro))}</p>')
        if not rows:
            parts.append('<p class="empty">（該当なし）</p>')
            continue
        parts.append(page_module.grid(
            columns, [[_plain(str(c)) for c in row] for row in rows[:_LIMIT]]))
        if len(rows) > _LIMIT:
            parts.append(f'<p class="meta">ほか {len(rows) - _LIMIT} 件</p>')
    parts.append("</div>")
    parts += page_module.tail()
    page = out_dir / f"{STEM}.html"
    page.write_text("\n".join(parts) + "\n", encoding="utf-8", newline="\n")
    return [md, page]


def _sections(spec: Spec, by_document: dict[tuple[str, str], set[str]] | None
              ) -> list[tuple[str, str, list[list[str]], list[str]]]:
    return [_by_origin(spec, by_document), _unused(spec)]


def _by_origin(spec: Spec, by_document: dict[tuple[str, str], set[str]] | None
               ) -> tuple[str, str, list[list[str]], list[str]]:
    """原本ごとに「何枚の写しが設計書に届いたか」。

    **「届いた」は 2 通りある。** 長いあいだ 1 つの列（「設計書に出た写し」）で
    しか言っておらず、それは**出典として 1 度でも引かれたか**でしかなかった ――
    実測（r001）で `処理仕様書_請求締め.xlsx/7.締め期間の例` は「出た」側に
    数えられていたが、その表の中身（締め実行日・締め期間・支払期日の例）は
    **全生成物に 1 文字も無かった。** 引かれたことと、読めることは別である。

    そこで 2 列に割る。

    ====================  ====================================================
    出典に引かれた写し    正本のレコードがその写しを ``source`` に挙げている
    本文に出た写し        **その写しから起こしたレコードが、どこかの表の行**
                          として出ている（:attr:`arp4.publish.Block.sources`）
    ====================  ====================================================

    後者でも「その写しの**どの欄まで**出たか」は言えない ―― そこまでは属性の
    側で見る（``P111``）。ここが言えるのは行が立ったかどうかまでである。
    """
    present = copies(spec)
    cited = used_sources(spec)
    buckets: dict[str, dict[str, Any]] = {}
    for (round_name, file), _ in sorted(present.items()):
        origin, _inside = origin_of(file)
        bucket = buckets.setdefault(origin, {"rounds": set(), "copies": 0,
                                             "cited": 0, "shown": 0,
                                             "records": 0, "docs": set()})
        bucket["rounds"].add(round_name)
        bucket["copies"] += 1
        records = cited.get((round_name, file), 0)
        if records:
            bucket["cited"] += 1
            bucket["records"] += records
        shown = (by_document or {}).get((round_name, file), set())
        if shown:
            bucket["shown"] += 1
        bucket["docs"] |= shown

    rows = [[origin, "・".join(sorted(b["rounds"])), str(b["copies"]),
             str(b["cited"]), str(b["shown"]), str(b["records"]),
             "、".join(sorted(b["docs"])) or "―"]
            for origin, b in sorted(buckets.items())]
    columns = ["原本", "ラウンド", "写し", "出典に引かれた写し", "本文に出た写し",
               "出典に挙げた件数", "出た設計書"]
    if by_document is None:
        # **本文の側は束を出すときにしか分からない。** 空欄で出すと「本文に
        # 出ていない」と読めるので、列ごと落とす（→ :func:`write` の断り）。
        rows = [row[:4] + row[5:6] for row in rows]
        columns = columns[:4] + columns[5:6]

    total = sum(int(row[2]) for row in rows)
    cited = sum(int(row[3]) for row in rows)
    shown = sum(int(b["shown"]) for b in buckets.values())
    tail = ("です" if by_document is None
            else f"、本文に行が出たのは {shown} 枚です")
    return ("原本ごとの使われ方",
            f"原本 {len(rows)} 件 / 写し {total} 枚。うち出典に引かれたのは "
            f"{cited} 枚{tail}。2 つは別のことを言っています。出典に"
            "引かれていても、その写しから起こしたレコードがどの表にも行として"
            "出ていないことがあります（列が無い・章が省略された・関係が張られて"
            "いない）。写しは `.arp/rounds/<ラウンド>/parsed/` にあり、"
            "設計書の出典セルからそのまま開けます。",
            rows, columns)


#: 出ていない理由の 3 通り。**次の一手が全部違う**ので言い分ける。
_DECLARED = "仕様にならないと宣言済み"
_UNREACHED = "整理結果が正本まで届いていません"
_UNORGANIZED = "まだ整理していません"


def _unused(spec: Spec) -> tuple[str, str, list[list[str]], list[str]]:
    """**1 度も出典に挙げられていない写し。** 次の一手が読める形で並べる。

    3 つを言い分けるのがこの表の全部である ―― **同じ「出ていない」でも次の一手が
    正反対**になる。

    ==================================  ==========================================
    整理結果が無い                      整理する（``freeze`` の未整理一覧にも上がる）
    全アンカーが対象外の宣言            **何もしなくてよい**（表紙・改訂履歴）
    レコードはあるが正本に出ていない    語彙外・concept 未割当・関係が張れていない
    ==================================  ==========================================

    言い分けずに「整理結果あり」だけを出していたときは、実測（r001）で
    **出ていない 67 枚が 67 枚とも対象外の宣言**だったのに、表は全部に
    「正本まで届いていません」と書いていた ―― **手を打つところが 1 つも無い
    ものを、67 件の宿題として見せていた。**
    """
    cited = used_sources(spec)
    #: (ラウンド, 写し) → (レコード数, 対象外の数, 対象外の理由の代表)
    organized: dict[tuple[str, str], tuple[int, int, str]] = {}
    seen: set[str] = set()
    for (round_name, _file), round_ in sorted(copies(spec).items()):
        if round_name in seen:
            continue
        seen.add(round_name)
        data, _ = organized_module.load(round_)
        for record in data.records:
            key = (round_name, record.file)
            count, out, why = organized.get(key, (0, 0, ""))
            organized[key] = (count + 1, out, why)
        for entry in data.out_of_scope:
            key = (round_name, entry.file)
            count, out, why = organized.get(key, (0, 0, ""))
            organized[key] = (count, out + 1, why or entry.reason)

    rows: list[list[str]] = []
    for (round_name, file), round_ in sorted(copies(spec).items()):
        if (round_name, file) in cited:
            continue
        records, out, why = organized.get((round_name, file), (0, 0, ""))
        if records:
            state, reading = f"レコード {records} 件", _UNREACHED
        elif out:
            state, reading = f"対象外 {out} 件", f"{_DECLARED}（{why}）" if why \
                else _DECLARED
        else:
            state, reading = "なし", _UNORGANIZED
        origin, inside = origin_of(file)
        rows.append([round_name, origin, inside or "―", state, reading])

    kinds = {row[4].split("（")[0] for row in rows}
    return ("設計書に 1 度も出ていない写し",
            f"漏れとは限りません。{len(rows)} 枚のうち、"
            + "・".join(f"{kind} {sum(1 for r in rows if r[4].startswith(kind))} 枚"
                        for kind in sorted(kinds))
            + "。手を打つのは"
            f"「{_UNORGANIZED}」と「{_UNREACHED}」だけです。",
            rows, ["ラウンド", "原本", "写し", "整理結果", "読み方"])


def _cell(value: Any) -> str:
    return str(value or "―").replace("|", "\\|").replace("\n", "<br>")


def _plain(value: str) -> str:
    return value.replace("**", "").replace("`", "")
