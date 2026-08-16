"""設計書の生成。**正本がデータ、設計書はそこから生成されるビューである。**

出力先（``out/``）は生成物なので直接編集しない。内容が違うなら正本を直す。

外部のテンプレートエンジンに依存しない（依存は PyYAML 1 本のまま）。
**文書定義（パックの ``documents/*.yml``）が宣言、ここが描画**という分担で、
様式を増やすときにコードを触らない。

章の種類（``kind``）は 4 つ。

========  ================================================================
items     種別の一覧（要件一覧・画面一覧）。``group_by`` で節に割る
relation  **関係を行にする表**。テーブル定義書の列定義・画面項目定義書
matrix    **行 × 列の対応表**。CRUD 図・権限マトリクス
trace     トレーサビリティ。``gap: true`` で未対応だけに絞る
========  ================================================================

日本の設計書は表紙・改訂履歴・目次が無いと受け取ってもらえないので、
``<spec>/publish.yml`` があれば表紙と改訂履歴を付け、目次は章から自動で作る。

出力は**工程ごとのフォルダに分ける**（``out/2_基本設計/テーブル定義書.md``）。
12 種を 1 階層に並べると、要件定義書とテスト結果報告書が隣に出て、レビューの単位
（＝工程）が読み手に見えなくなる。フォルダ名の数字は :attr:`Metamodel.layers` の
並び（V 字の順）であって、辞書順に潰されないようにするためのものである。
"""

from __future__ import annotations

import html
import os
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Iterable

from arp4 import audience as audience_module
from arp4 import figure as figure_module
from arp4 import gate as gate_module
from arp4 import holes as holes_module
from arp4 import mdio
from arp4 import origins as origins_module
from arp4 import pack as pack_module
from arp4 import page as page_module
from arp4 import sequence as sequence_module
from arp4 import yamlio
from arp4.conform import matches
from arp4.finding import Finding
from arp4.spec import Spec

_STATUS_LABEL = {"draft": "起票", "review": "レビュー中",
                 "approved": "承認済", "deprecated": "廃止"}

#: 体裁は :mod:`arp4.page` が持つ。**穴の 1 枚・元資料の対応表と同じ見た目**に
#: なるのが要点で、ここに置いていたころは :mod:`arp4.holes` だけが素の HTML で
#: 出ていた（空の ``<link rel="stylesheet">`` を出していた）。


@dataclass
class Block:
    """設計書の 1 章（または 1 節）。**描画前に文字列まで落としておく。**

    **章番号は持たない。** 番号は並びの中での位置であって章の属性ではなく、
    :func:`_drop_empty` が空の章を畳むたびに全部ずれる ―― 持たせると「畳む前の
    番号」という**古くなった値が生まれる余地**ができる。実際それで壊れていた：
    脚注の「省略した章」が畳む前の番号を出し、本文の章は振り直されていたので、
    詳細設計書に `3 呼出関係` と `省略した章: 3 バッチ構成` が同時に載っていた。

    番号を場に持たなければ、脚注に積めるのは :attr:`heading` だけになる ――
    **検査で見つけるのではなく、書けなくする。** 番号は描画の直前に並びから
    決める（→ :func:`_numbering`）。
    """

    level: int                      # 2 = 章、3 = 節
    heading: str
    columns: list[str] = field(default_factory=list)
    rows: list[list[str]] = field(default_factory=list)
    source_columns: set[int] = field(default_factory=set)
    #: 列の**文書定義に書かれたままのパス**（`to.value` 等）。見出し（columns）は
    #: 日本語ラベルに置換済みなので、列が正本のどこを指しているかはここにしか
    #: 残らない ―― :func:`_trim` が「全行が空」と「対応づけの誤り」を見分けるのに使う。
    paths: list[str] = field(default_factory=list)
    #: 節を束ねるだけの章（表を持たない）。「該当なし」とも書かない。
    heading_only: bool = False
    #: 表の下に出す脚注。**畳んだ行・列を必ず名前つきで並べる**（→ :func:`_omitted`）。
    notes: list[str] = field(default_factory=list)
    #: この章が要る語彙（種別・関係型）。空の設計書の理由を書くのに使う。
    needs: list[str] = field(default_factory=list)
    #: 表示 ID の列。HTML でその番号へ飛べるようにする（→ :func:`_id_column`）。
    id_column: int | None = None
    #: この塊の行が出てきた ``(ラウンド, 写し)``。**出典の列があるかとは無関係**に
    #: 集める ―― 出典の列を持つのは 12 冊のうち 3 冊だけ（実測 580 セル）なので、
    #: 列から数えると残りの 9 冊は「どの資料からも出ていない」ことになる。
    sources: set[tuple[str, str]] = field(default_factory=set)
    #: 畳んだ章・列の数。**脚注の文面とは別に数で持つ**（→ :func:`_brief`）。
    folded_sections: int = 0
    folded_columns: int = 0
    #: 畳んだ列を ``(節の見出し, 列, 文書定義のパス)`` で。**脚注の散文とは別に
    #: 構造で持つ** ―― 穴の 1 枚が数えるのに、文を正規表現で割らせない
    #: （→ :mod:`arp4.holes`）。
    #:
    #: 章は**節の見出し**である。脚注は節の最後の表に付く（17 回出さないため）
    #: ので、そこの小見出しを使うと `棚卸入力（SCR-016）` のような**たまたま
    #: 最後だった 1 枚**の名前になり、しかも別々の節が同じ小見出しで終わると
    #: 同じ行が 2 つ並ぶ（実測で要件定義書の `請求管理` が重複した）。
    folded: list[tuple[str, str, str]] = field(default_factory=list)
    #: **畳まなかった**列を ``(節の見出し, 列, 定義のパス, 値がある欄)`` で。
    #: 全行が空だが正本の別の欄に値があるもの（→ :func:`_alternative`）。
    #: **穴の 1 枚では :attr:`folded` と別の表に出す** ―― 「資料に無いのかも
    #: しれない」と「別の欄に入っている」は次の一手が正反対である。
    misdirected: list[tuple[str, str, str, str]] = field(default_factory=list)
    #: 行が無いことの理由。``None`` なら「対象データが無い」―― 母集合そのものが
    #: 空だったということである。**母集合はあるのに関係が 1 本も無い**ときは
    #: 同じ言葉で畳めない（→ :func:`_drop_empty`）。
    blank_reason: str | None = None


def _pairs(records: Iterable[dict[str, Any]]) -> set[tuple[str, str]]:
    """レコードの出典を ``(ラウンド, 写し)`` の集合に。

    **関係は出典を持たない**（実測で 1094 本すべてが空）ので、関係の表では
    両端のアイテムから集める ―― 行が出てきた資料はそちらにしか書かれていない。
    """
    found: set[tuple[str, str]] = set()
    for record in records:
        for entry in record.get("source") or []:
            if isinstance(entry, dict) and entry.get("file"):
                found.add((str(entry.get("round") or ""), str(entry["file"])))
    return found


@dataclass
class Brief:
    """設計書 1 冊の**概要**。中身は数えれば出るものだけである。

    **要約を書かない。** 本文を縮めた文を置くと、出典の無い文章が設計書の
    いちばん目立つところに載る ―― それは整理層の判断であって、生成層は
    「資料がこう言っていた」と言える立場に無い（決定 1 の分担そのもの）。
    ここに出るのは章・表・行の数、出典の内訳、畳んだものの数、未解決の指摘で、
    **全部いま組み立てた ``blocks`` と正本から出る。**

    置き場は本文の前である。表だけ読んで閉じる読み方（設計書はそう読まれる）
    から見えるのは冒頭だけで、脚注へ回すと帯（→ :mod:`arp4.gate`）と同じ理由で
    読まれない。
    """

    chapters: int = 0
    tables: int = 0
    rows: int = 0
    #: 出典の内訳。原本（Excel なら 1 冊）と写し（シート）の数を分けて数える。
    books: int = 0
    copies: int = 0
    #: 畳んだ章・列の数（脚注に名前つきで並んでいるものの件数）。
    folded_sections: int = 0
    folded_columns: int = 0
    #: この設計書が持つ表示 ID の数と、他の設計書を名指ししている升の数。
    owns: int = 0
    refers: set[str] = field(default_factory=set)

    def facts(self) -> list[tuple[str, str, str]]:
        """``(見出し, 値, 補足)``。**md と html で同じものを出す**ための 1 本。"""
        found = [("章 / 表 / 行",
                  f"{self.chapters} 章 / {self.tables} 表 / {self.rows} 行", ""),
                 ("出典", f"原本 {self.books} 件",
                  f"写し {self.copies} 枚" if self.copies else "")]
        if self.owns:
            found.append(("この設計書が持つ表示 ID", f"{self.owns} 件", ""))
        if self.refers:
            found.append(("参照している設計書", f"{len(self.refers)} 冊",
                          "、".join(sorted(self.refers))))
        # **0 を書かない。** 「章 2 ・ 列 0」は畳んでいない側まで数えたように読め、
        # 帯のいちばん狭い升で 2 行を使う。畳んだものだけを言う。
        folded = "・".join(f"{label} {count}" for label, count in
                          (("章", self.folded_sections), ("列", self.folded_columns))
                          if count)
        if folded:
            found.append(("省略したもの", folded, "`--full` で全部出ます"))
        return found


def _brief(spec: Spec, blocks: list[Block]) -> Brief:
    """:class:`Brief` を組み立てる。**脚注の文面からは数えない。**

    「省略した列: …」という文を読み直して数える書き方もできるが、それは
    表示のための文字列を機械が読み戻すということで、文面を直した日に静かに
    0 件になる。畳んだ数は畳んだ場所（:func:`_drop_empty` / :func:`_trim`）が
    そのまま持つ。
    """
    sources: set[tuple[str, str]] = set()
    for block in blocks:
        sources |= block.sources
    books = {origins_module.origin_of(file)[0] for _, file in sources}
    owned = {row[b.id_column].strip()
             for b in blocks if b.id_column is not None
             for row in b.rows
             if b.id_column < len(row) and row[b.id_column].strip() not in _BLANK}
    return Brief(
        chapters=sum(1 for b in blocks if b.level <= 2),
        tables=sum(1 for b in blocks if b.rows),
        rows=sum(len(b.rows) for b in blocks),
        books=len(books), copies=len(sources),
        folded_sections=sum(b.folded_sections for b in blocks),
        folded_columns=sum(b.folded_columns for b in blocks),
        owns=len(owned))


def _numbering(blocks: list[Block]) -> list[str]:
    """章番号を**並びから決める**。畳んだあとの列に対して、描画の直前に呼ぶ。

    :class:`Block` の場に持たせず純粋な関数にしてあるのが要点である ―― 番号を
    書き換える経路が無ければ、古い番号も生まれない（→ :class:`Block`）。

    番号を飛ばさずに詰め直すのは、**章番号は文書内の位置であって識別子ではない**
    からである。識別子は表示 ID（`FR-005`）で、HTML ではそこへ直接飛べる
    （→ :func:`_anchor`）ので、番号が動いてもレビューの参照は壊れない。番号を
    飛ばしたまま「7 業務ルール」から始まる設計書のほうが、読み手に「1〜6 は
    どこへ行った」と探させる。
    """
    numbers: list[str] = []
    chapter = section = 0
    for block in blocks:
        if block.level <= 2:
            chapter += 1
            section = 0
            numbers.append(str(chapter))
        else:
            section += 1
            numbers.append(f"{chapter}.{section}")
    return numbers


def publish(spec: Spec, out_dir: Path, names: Iterable[str] | None = None,
            meta: dict[str, Any] | None = None, flat: bool = False,
            full: bool = False,
            gate: gate_module.Gate | None = None,
            findings: Iterable[Finding] | None = None) -> list[Path]:
    """文書定義に従って Markdown と HTML を書き出す。

    ``flat`` なら工程で分けずに ``out/`` 直下へ並べる（旧来の置き方）。
    ``full`` ならマトリクスの空行・空列も畳まずに出す（→ :func:`_matrix_blocks`）。
    ``gate`` は ``publish`` が通った条件（→ :mod:`arp4.gate`）―― ``--force`` で
    通したことを**生成物の冒頭に残す**。渡さなければ何も足さない（既存の呼び出しは
    そのまま動く）。``findings`` は穴の 1 枚（→ :mod:`arp4.holes`）の材料。
    """
    definitions = pack_module.documents(list(spec.metamodel.chain))
    wanted = set(names or [])
    if wanted:
        definitions = [d for d in definitions
                       if d.get("name") in wanted or d.get("title") in wanted]

    if meta is None:
        meta = _read_meta(spec)

    # **書き出す前に全部組み立てる。** 表示 ID をどの設計書が持っているかは
    # 束ぜんぶを見ないと決まらない（→ :func:`_owners`）。1 冊ずつ書きながら
    # 決めると、先に書いた設計書だけが後ろの番号へ飛べないという非対称が出る。
    prepared: list[tuple[dict[str, Any], list[Block], Path, Path]] = []
    out_dir.mkdir(parents=True, exist_ok=True)
    for definition in definitions:
        blocks = _blocks(spec, definition, full)
        stem = str(definition.get("output") or definition.get("title")
                   or definition.get("name"))
        directory = _phase_dir(spec, out_dir, definition, flat)
        directory.mkdir(parents=True, exist_ok=True)
        prepared.append((definition, blocks,
                         directory / f"{stem}.md", directory / f"{stem}.html"))

    owners = _owners(spec, prepared)
    copies = Copies.of(spec)

    written: list[Path] = []
    #: (工程, 題, Markdown のパス, 対象データが無いか)
    placed: list[tuple[str, str, Path, bool]] = []
    #: (ラウンド, 写し) → その写しが出典として出た設計書の題（元資料の対応表が使う）。
    by_document: dict[tuple[str, str], set[str]] = {}
    #: 題 → その設計書の見取り図の材料（→ :func:`_map_html`）。
    charted: list[tuple[str, str, Path, Brief, set[str]]] = []
    # 目次へ戻る導線は**目次を出すときだけ**付ける（1 冊だけ書き出したときに
    # 付けると、存在しないページを指す）。
    index = out_dir / "目次.html" if not wanted else None
    for definition, blocks, md, page in prepared:
        # 工程フォルダの深さ ―― 帯から穴の一覧へ戻る相対リンクに要る。
        depth = len(md.parent.relative_to(out_dir).parts)
        title = str(definition.get("title") or md.stem)
        brief = _brief(spec, blocks)
        brief.refers = _refers(blocks, owners, page, prepared)
        md.write_text(_markdown(spec, definition, blocks, meta, gate, depth, brief),
                      encoding="utf-8", newline="\n")
        page.write_text(_html(spec, definition, blocks, meta, owners, page, index,
                              gate, depth, copies, brief),
                        encoding="utf-8", newline="\n")
        written += [md, page]
        placed.append((str(definition.get("phase") or ""), title, md,
                       barren(blocks)))
        for block in blocks:
            for key in block.sources:
                by_document.setdefault(key, set()).add(title)
        charted.append((str(definition.get("phase") or ""), title, page, brief,
                        brief.refers))

    # 穴の 1 枚と元資料の対応表は**束を出すときだけ**（1 冊だけ書き出したときに
    # 出すと、束ぜんぶを見ていないのに「これが全部だ」と読める）。
    holes: list[Path] = []
    if not wanted and placed:
        # 畳んだ列は**束を通して**集める。1 冊ずつ見ると「この設計書には無い」で
        # 終わり、**同じ欄がどこにも無いこと**が読めない（→ :mod:`arp4.holes`）。
        folded = sorted({(str(definition.get("title") or definition.get("name")),
                          where, name, path)
                         for definition, blocks, _md, _page in prepared
                         for block in blocks
                         for where, name, path in block.folded})
        # **畳まなかった列は別に集める。** 「資料に無いのかもしれない」と
        # 「別の欄に入っている」は次の一手が正反対である（→ :mod:`arp4.holes`）。
        misdirected = sorted(
            {(str(definition.get("title") or definition.get("name")),
              where, name, path, alt)
             for definition, blocks, _md, _page in prepared
             for block in blocks
             for where, name, path, alt in block.misdirected})
        holes = holes_module.write(spec, out_dir, findings or [], gate, folded,
                                   misdirected)
        written += holes
        written += origins_module.write(spec, out_dir, by_document)
        written += _index(spec, out_dir, placed, gate, bool(holes), charted)
    if gate is not None:
        written.append(gate_module.record(out_dir, gate))
    return written


def _refers(blocks: list[Block], owners: dict[str, Path], here: Path,
            prepared: list[tuple[dict[str, Any], list[Block], Path, Path]],
            ) -> set[str]:
    """この設計書の升が名指ししている**他の設計書の題**。

    見取り図の辺はここから出る（→ :mod:`arp4.figure`）。**探し方は
    :func:`_linkify` と同じ**にしてある ―― 図と本文のリンクが別々の規則で
    引かれると、線はあるのに飛べない（あるいはその逆）という食い違いが出る。
    """
    titles = {page: str(definition.get("title") or page.stem)
              for definition, _blocks, _md, page in prepared}
    found: set[str] = set()
    for block in blocks:
        for row in block.rows:
            for cell in row:
                for line in cell.split("\n"):
                    for segment in line.split("、"):
                        target = owners.get(segment.partition(" ")[0].strip())
                        if target is not None and target != here:
                            found.add(titles.get(target, target.stem))
    return found


def _owners(spec: Spec,
            prepared: list[tuple[dict[str, Any], list[Block], Path, Path]],
            ) -> dict[str, Path]:
    """表示 ID を**どの設計書が持っているか**（`MOD-027` → 詳細設計書.html）。

    番号は束の中で 1 度しか定義されない、という状態を作るための表である。
    同じ `MOD-027` は詳細設計書の一覧にもトレーサビリティ・マトリクスにも出る
    ので、**両方が `id` を名乗ると `#MOD-027` の飛び先が読み手から見て 2 つに
    なる。** 持つのは 1 冊、残りはそこへ**飛ぶ**（→ :func:`_linkify`）。

    持ち主は**工程が先の設計書**（V 字の並び ＝ :attr:`Metamodel.layers`）と
    する ―― 番号が生まれるのは定義する工程で、後ろの工程はそれを参照して
    いるだけである。同じ工程なら様式の並び順（安定ソート）。

    ここで作った表が無いと、番号のアンカーは誰からも参照されない ―― 実測で
    テスト仕様書は `TC-0001`〜`TC-0596` の 596 個の `id` を持ちながら、
    トレーサビリティ・マトリクスからは `TC-0086` がただの文字列だった。
    """
    order = phases(spec)

    def phase_key(entry: tuple[dict[str, Any], list[Block], Path, Path]) -> int:
        phase = str(entry[0].get("phase") or "")
        return order.index(phase) if phase in order else len(order)

    claimed: dict[str, Path] = {}
    for _, blocks, _md, page in sorted(prepared, key=phase_key):
        for block in blocks:
            if block.id_column is None:
                continue
            for row in block.rows:
                if block.id_column >= len(row):
                    continue
                value = row[block.id_column].strip()
                if value and value not in _BLANK and value not in claimed:
                    claimed[value] = page
    return claimed


def phases(spec: Spec) -> list[str]:
    """工程の並び。**メタモデルの ``layers`` が正**（V 字の順）。"""
    return list(spec.metamodel.layers)


def _phase_dir(spec: Spec, out_dir: Path, definition: dict[str, Any],
               flat: bool) -> Path:
    phase = str(definition.get("phase") or "")
    if flat or not phase:
        # 工程を宣言していない様式は分類しない。**勝手にどこかへ入れない。**
        return out_dir
    order = phases(spec)
    index = order.index(phase) + 1 if phase in order else len(order) + 1
    return out_dir / f"{index}_{phase}"


def _index(spec: Spec, out_dir: Path,
           placed: list[tuple[str, str, Path, bool]],
           gate: gate_module.Gate | None = None,
           holes: bool = False,
           charted: list[tuple[str, str, Path, "Brief", set[str]]] | None = None,
           ) -> list[Path]:
    """``out/目次.md`` と ``out/目次.html``。**どの工程に何が出たか**を 1 枚で。

    中身の無い設計書には ``（対象データなし）`` を付ける。付けないと、目次だけ見た
    レビュアーには「作ったが空」と「作っていない」の区別がつかない。

    **HTML にも同じ目次を出す。** 設計書は 2 つの形で出しているのに入口が
    Markdown にしか無く、しかも Markdown の目次は `.md` しか指さない ――
    HTML 一式を渡された人には、束のどこから読み始めるかが無かった。

    **付録（決定記録・stakeholder 向け）も並べる**（→ :func:`_appendix`）。
    どちらも ``placed`` に入らない経路で書かれるので、実測（r001）で 7 本が
    目次のどこからも指されていなかった ―― **束を渡された人はそこへ到達できない。**
    """
    order = phases(spec)
    buckets: dict[str, list[tuple[str, Path, bool]]] = {}
    for phase, title, path, empty in placed:
        buckets.setdefault(phase, []).append((title, path, empty))

    #: (章の名前, その章の項) ―― Markdown と HTML で同じ並びを使う。
    grouped: list[tuple[str, list[tuple[str, Path, bool]]]] = []
    for phase in [*order, ""]:
        entries = buckets.pop(phase, [])
        if entries:
            grouped.append((phase or "工程を宣言していない様式", sorted(entries)))
    for phase in sorted(buckets):          # layers に無い工程も落とさない
        grouped.append((phase, sorted(buckets[phase])))

    lines = ["# 生成した設計書", ""]
    lines += gate_module.banner(gate, depth=0)
    lines += ["> この文書は生成物です。**直接編集しないでください**"
              "（`arp4 publish` で再生成されます）。"
              + (" " + gate_module.footnote(gate)
                 if gate_module.footnote(gate) else ""), ""]
    # **穴の 1 枚は工程より前**に置く。工程の中へ入れると、読み手は設計書を
    # 読み終えてから穴を知ることになる（→ :mod:`arp4.holes`）。
    if holes:
        lines += [f"- [{holes_module.STEM[2:]}]({holes_module.STEM}.md)"
                  "（この一式が何を書けていないか）"]
        lines += [f"- [{origins_module.STEM[2:]}]({origins_module.STEM}.md)"
                  "（届いた資料のどれが使われなかったか）", ""]
    for phase, entries in grouped:
        lines += [f"## {phase}", ""]
        for title, path, empty in entries:
            link = f"- [{title}]({path.relative_to(out_dir).as_posix()})"
            lines.append(link + "（対象データなし）" if empty else link)
        lines.append("")

    appendix = _appendix(out_dir)
    if appendix:
        lines += ["## 付録", ""]
        lines += [f"- [{title}]({path.relative_to(out_dir).as_posix()})"
                  + (f"（{note}）" if note else "")
                  for title, path, note in appendix]
        lines.append("")

    md = out_dir / "目次.md"
    md.write_text("\n".join(lines).rstrip() + "\n", encoding="utf-8", newline="\n")

    escape = html.escape
    parts = page_module.head("生成した設計書")
    # 目次は表を持たないので絞り込みは出さないが、**帯そのものは出す** ――
    # 配色の切り替えがここに無いと、束の入口だけ地の色が変わらない。
    parts.append(page_module.toolbar(filters=False))
    parts += ['<div class="wrap">', "<h1>生成した設計書</h1>",
              gate_module.banner_html(gate, depth=0),
              '<p class="meta">この文書は生成物です。直接編集しないでください'
              '（arp4 publish で再生成されます）。'
              f'{escape(_plain_text(gate_module.footnote(gate)))}</p>']
    away = page_module.NEW_TAB
    if holes:
        parts.append(f'<ul><li><a href="{holes_module.STEM}.html"{away}>'
                     f"{escape(holes_module.STEM[2:])}</a>"
                     "（この一式が何を書けていないか）</li>"
                     f'<li><a href="{origins_module.STEM}.html"{away}>'
                     f"{escape(origins_module.STEM[2:])}</a>"
                     "（届いた資料のどれが使われなかったか）</li></ul>")
    # 見取り図は目次の中ほど ―― **工程の一覧より前**に置く。あとに置くと、
    # 12 冊のリストを読み終えた人にしか束の形が見えない。
    parts.append(_map_html(spec, out_dir, charted or []))
    for phase, entries in grouped:
        parts.append(f"<h2>{escape(phase)}</h2><ul>")
        for title, path, empty in entries:
            href = path.with_suffix(".html").relative_to(out_dir).as_posix()
            note = '<span class="empty">（対象データなし）</span>' if empty else ""
            parts.append(f'<li><a href="{escape(href)}"{away}>{escape(title)}</a>'
                         f"{note}</li>")
        parts.append("</ul>")
    if appendix:
        # **付録は HTML を持たない**ので、こちらの目次からも `.md` を指す。
        # 指さないほうが体裁は揃うが、HTML 一式だけを渡された人はそこで詰まる。
        parts.append("<h2>付録</h2><ul>")
        for title, path, note in appendix:
            href = path.relative_to(out_dir).as_posix()
            parts.append(f'<li><a href="{escape(href)}"{away}>{escape(title)}</a>'
                         + (f"（{escape(note)}）" if note else "") + "</li>")
        parts.append("</ul>")
    parts.append("</div>")
    parts += page_module.tail()

    page = out_dir / "目次.html"
    page.write_text("\n".join(parts) + "\n", encoding="utf-8", newline="\n")
    return [md, page]


def _appendix(out_dir: Path) -> list[tuple[str, Path, str]]:
    """目次の「付録」に並べるもの ―― ``(題, パス, 添え書き)``。

    **工程の節へ混ぜない。** 決定記録も stakeholder 向けの一式も工程の成果物では
    なく**束全体の記録**で、穴の 1 枚・元資料の対応表と同じ性格である ―― 工程へ
    入れると「どの工程で承認するのか」という問いが読み手に生まれる。

    材料は ``out/`` の実物だけ見る。どちらも ``placed``（様式の ``documents/*.yml``
    から出た文書）には入らない ―― 決定記録は :func:`arp4.audience.decision_report`、
    stakeholder は ``--audience stakeholder`` の別経路で書かれる。**様式から
    引けないものは、置かれたものを見るしかない。**
    """
    found: list[tuple[str, Path, str]] = []
    report = out_dir / "決定記録.md"
    if report.is_file():
        # 添え書きは Markdown と HTML の両方へそのまま出るので、強調記法は使わない。
        found.append((report.stem, report,
                      "機械が下した判断の全件（止めたい判断の差し戻し口）"))
    for path in sorted((out_dir / audience_module.DIR).glob("*.md")):
        found.append((path.stem, path, "PM・顧客向け"))
    return found


def _map_html(spec: Spec, out_dir: Path,
              charted: list[tuple[str, str, Path, Brief, set[str]]]) -> str:
    """束の見取り図。工程を横に並べ、**設計書どうしの参照**を矢印にする。

    材料は :func:`_owners`（表示 ID の持ち主）と :func:`_refers`（升が名指し
    している番号）だけである ―― 図のためのデータを別に持たないのが要点で、
    持つと本文のリンクと図の線が別々に古くなる。
    """
    if not charted:
        return ""
    order = phases(spec)
    buckets: dict[str, list[figure_module.Node]] = {}
    for phase, title, page, brief, _refs in charted:
        buckets.setdefault(phase, []).append(figure_module.Node(
            key=title, label=title,
            sub=(f"{brief.tables} 表 / {brief.rows} 行" if brief.rows
                 else "対象データなし"),
            href=page.relative_to(out_dir).as_posix()))
    groups = [(phase or "工程の宣言なし", buckets[phase])
              for phase in [*order, ""] if phase in buckets]
    groups += [(phase, nodes) for phase, nodes in sorted(buckets.items())
               if phase and phase not in order]

    edges = {(title, other) for _phase, title, _page, _brief, refs in charted
             for other in refs}
    return figure_module.documents(groups, edges)


def catalog(spec: Spec) -> list[dict[str, Any]]:
    return pack_module.documents(list(spec.metamodel.chain))


def _read_meta(spec: Spec) -> dict[str, Any]:
    """``.arp/publish.yml``（表紙・改訂履歴）。無ければ表紙を付けない。"""
    if spec.paths is None or not spec.paths.publish.is_file():
        return {}
    return yamlio.load(spec.paths.publish) or {}


# ── 章を組み立てる ──────────────────────────────────────────────
def _blocks(spec: Spec, definition: dict[str, Any], full: bool = False) -> list[Block]:
    blocks: list[Block] = []
    for section in definition.get("sections") or []:
        kind = str(section.get("kind") or "items")
        heading = str(section.get("heading") or "")
        builder = {"items": _items_blocks, "relation": _relation_blocks,
                   "matrix": _matrix_blocks, "trace": _trace_blocks}.get(kind)
        if builder is None:
            blocks.append(Block(2, f"{heading}（未知の章種別: {kind}）"))
            continue
        made = builder(spec, section, heading, full)
        for block in made:
            block.needs = _needs(section)
        # 升目は行と列を自分で畳む（→ :func:`_matrix_blocks`）ので二度やらない。
        # **トレースは畳まない** ―― あちらは空欄そのものが結論である
        # （「対応するテストケース」が全行 `―` なら、それがテスト漏れの一覧）。
        #
        # ただし ``gap: true`` は別。あちらは**行の選び方**が結論で、列は
        # ただの属性である ―― 未実施のテストケースの「レベル」が 596 行とも
        # `―` なのは何の結論でもないのに、この免除の巻き添えで残っていた。
        if not full and (kind in ("items", "relation")
                         or (kind == "trace" and section.get("gap"))):
            _trim(made, spec, section)
        # **畳んだあとに足す。** 争点の印は「全行が空だから省略」の対象では
        # ない ―― 争点が 1 件でもある表にしか付けないので、そもそも全行は空に
        # ならない（→ :func:`_dispute_marks`）。
        _dispute_marks(spec, made)
        blocks += made
    return blocks if full else _drop_empty(blocks)


def _dispute_marks(spec: Spec, blocks: list[Block]) -> None:
    """争点（``disputes``）のあるアイテムの行に、課題の表示 ID を出す。

    **足りないのは印を出すことだけだった。** 関係は既に張ってあり、課題管理表と
    穴の 1 枚は拾えている。それでも実測（r001）で基本設計書の業務ルールには
    **互いに矛盾する 4 組**が並列に載った（引当のタイミング・消費税の計算単位・
    請求の締め日・受注取消の期限）―― この 1 冊だけを読んだ人は、**両方を確定
    仕様として受け取る。**

    **新しい判断はしていない。** 出すのは正本にある ``disputes`` の相手の表示 ID
    だけで、どちらが正かは言わない（それは決着していないことそのものである）。

    列は**争点のある行がある表にだけ足す**。全部の表に「課題」列を並べると、
    ほとんどが `―` の列が増えるだけで、**印は多いほど目に入らなくなる。**
    足す位置は末尾 ―― :attr:`Block.source_columns` と :attr:`Block.id_column` は
    位置で覚えているので、途中に挿すと出典の列と ID の列が 1 つずれる。

    表示 ID は HTML で課題管理表へ飛ぶ（→ :func:`_linkify`）ので、印そのものが
    導線になる。表示 ID を持たない課題は出しようが無いので黙って飛ばす
    ―― 番号が無ければ飛び先も無い。
    """
    by_id = spec.by_id
    issues: dict[str, list[str]] = {}
    for relation in spec.relations_of("disputes"):
        if relation.get("status") == "deprecated":
            continue
        target = by_id.get(str(relation.get("to")))
        issue = by_id.get(str(relation.get("from")))
        if not target or not issue:
            continue
        display, mark = _display_of(spec, target), _display_of(spec, issue)
        if display and mark and mark not in issues.setdefault(display, []):
            issues[display].append(mark)
    if not issues:
        return

    for block in blocks:
        if block.id_column is None or not block.rows:
            continue
        marks = ["、".join(sorted(issues.get(str(row[block.id_column]).strip(), [])))
                 if block.id_column < len(row) else ""
                 for row in block.rows]
        if not any(marks):
            continue
        block.columns.append("課題")
        block.paths.append("")
        for row, mark in zip(block.rows, marks):
            row.append(mark or "―")


def _display_of(spec: Spec, item: dict[str, Any]) -> str:
    """そのアイテムの表示 ID（無ければ空）。"""
    definition = spec.metamodel.item_types.get(str(item.get("type"))) or {}
    attribute = sequence_module.display_attribute(definition)
    return str(item.get(attribute) or "") if attribute else ""


def _children(blocks: list[Block], index: int) -> list[Block]:
    """``blocks[index]`` にぶら下がる節。"""
    level = blocks[index].level
    found: list[Block] = []
    for block in blocks[index + 1:]:
        if block.level <= level:
            break
        found.append(block)
    return found


def _drop_empty(blocks: list[Block]) -> list[Block]:
    """**1 行も出なかった章節を畳む。** 落とした章名は必ず脚注に出す。

    様式は固定なので、コードから起こした正本には画面・帳票・バッチ・外部 IF が
    無く「（該当なし）」の章が毎回並ぶ ―― 実測で **144 章節のうち 20**、基本設計書は
    12 のうち 7 が空だった。目次は「この設計書に何があるか」を見るところなので、
    無いものが過半を占めると**あるものを探せない**。

    文書ぜんぶが空のときは畳まない ―― あちらは :func:`_no_data` が理由を書く。
    畳んだ章名を並べるのは列の省略（→ :func:`_trim`）と同じ規律で、**件数だけ・
    黙って落とすのどちらも「資料に無い」と「様式にあるだけ」を混ぜる。**
    """
    if barren(blocks):
        return blocks

    kept: list[Block] = []
    # 理由ごとに分けて数える。**「対象データが無い」で束ねない** ―― 母集合はある
    # のに関係が 1 本も無い章をそう書くと、読み手は資料を探しに行く
    # （→ :attr:`Block.blank_reason`）。
    dropped: dict[str, list[str]] = {}
    skip_level: int | None = None
    for index, block in enumerate(blocks):
        if skip_level is not None and block.level > skip_level:
            continue                       # 畳んだ章の子（章名だけ脚注に出す）
        skip_level = None
        if block.heading_only:
            if any(child.rows for child in _children(blocks, index)):
                kept.append(block)
            else:
                dropped.setdefault(block.blank_reason or "", []).append(block.heading)
                skip_level = block.level
        elif block.rows:
            kept.append(block)
        else:
            dropped.setdefault(block.blank_reason or "", []).append(block.heading)

    if not kept:
        return blocks
    for reason, names in dropped.items():
        # 章名は `（ロール × 機能）` のように括弧で終わることがあるので、
        # 打ち手は括弧で足さず文を分ける（括弧が 2 つ並ぶと読めない）。
        kept[-1].notes.append(
            (reason or "対象データが無い") + "ので省略した章: " + _listed(names)
            + "。全部の章を出すには `arp4 publish --full`。")
        kept[-1].folded_sections += len(names)
    return kept


def _listed(names: list[str]) -> str:
    """名前を並べる。**1 つずつ鉤括弧で括る。**

    区切りに `・` だけを使うと、**名前の中の `・` と見分けが付かない** ――
    要件定義書で畳んだ章は「業務フロー」「利用者・ロール」「用語」の 3 つ
    なのに、`・` で繋ぐと `業務フロー・利用者・ロール・用語` となって 4 つに
    読める。畳んだものを名指しするのは「資料に無い」と「様式にあるだけ」を
    混ぜないためなので、**いくつ畳んだか**が読めなければ半分意味が無い。
    """
    return "".join(f"「{name}」" for name in names)


#: 空欄の表記。値が無いセルはここまでに `―` になっている（→ :func:`_cell`）。
_BLANK = ("", "―")


def _trim(blocks: list[Block], spec: Spec | None = None,
          section: dict[str, Any] | None = None) -> None:
    """**節を通して全行が空の列を落とす。** 落とした列名は必ず脚注に出す。

    要件 44 件すべてで空の「補足」、非機能 12 件すべてで空の「目標値」、呼出関係の
    全行が `―` の「クラス名」―― こういう列は**読み手に何も伝えないのに、伝わる
    ものを狭める**。列が増えるほど 1 列の幅は減り、紙にすると左右に割れる。

    升目には既に「関係の記載が無いため省略した行 …／全部の升を出すには
    ``--full``」という規律があるので、**同じ規律を列にも当てる**だけである。
    名指しするところまで同じにする ―― 件数だけ・黙って落とすのどちらも、
    「資料に無い」と「機械が出していない」を混ぜる。

    判定は**節ごと**（塊ごとではない）。`group_by` で節が 17 の表に割れるとき、
    表ごとに決めると**同じ列がある表には出てある表には無い**という並びになり、
    比べられなくなる ―― しかも脚注が 17 回出る。

    **「全行が空」と「正本には値があるのに写っていない」は混ぜない。** 全行が
    空になった列は、畳む前に**値が正本の別の欄に無いか**を見る（→
    :func:`_alternative`）。あるなら畳まずに残して脚注で言う ―― 実測（r001）で
    コード定義の列 `value`（正しくは `to.value`）が「全行が空だったので省略した列:
    コード値」と畳まれ、**文書定義の誤りが「資料に無い」と同じ顔で出ていた。**
    同じことが `displays.note`（空）と `displays.description`（154 件）でも
    起きている ―― あちらは**同じ側**なので、反対側だけを探していたころは
    黙って畳まれていた。
    """
    tables = [b for b in blocks if b.rows and len(b.columns) > 1]
    if not tables:
        return
    width = len(tables[0].columns)
    if any(len(b.columns) != width for b in tables):
        return                                  # 列の並びが違う節は畳まない
    empty = [i for i in range(width)
             if all(str(row[i]).strip() in _BLANK if i < len(row) else True
                    for b in tables for row in b.rows)]

    paths = tables[0].paths if len(tables[0].paths) == width else [""] * width
    where = str((section or {}).get("heading") or tables[-1].heading)
    # **持ち主を頭に付ける。** 裸の `note` では何の `note` か辿れない ―― 整理の
    # 手順書は「`displays` の `note` へ写す」と綴っているので、そこと同じ字にする。
    # `to.action` のように相手側を既に名乗っているものはそのまま。
    owner = str((section or {}).get("relation") or (section or {}).get("type") or "")

    #: 全行が空だが、正本の別の場所に値がある列。**畳まずに残して脚注で言う。**
    #:
    #: **判定は ``W043`` と同じ関数を呼ぶ**（→ :func:`_alternative`）。同じ列に
    #: ついて「畳んだ」（``W046``）と「別の欄に値がある」（``W043``）の両方が
    #: 鳴らないのは、ここが排他になっているからである。
    hidden: list[tuple[int, str]] = []
    if spec is not None and section is not None and len(tables[0].paths) == width:
        kind = str(section.get("kind") or "items")
        for index in list(empty):
            alt = _alternative(spec, section, kind, str(paths[index] or ""))
            if alt is not None:
                hidden.append((index, alt))
                empty.remove(index)
    # **どちらの欄も名前で出す。** 「列『備考』は全行が空です」だけでは、その
    # `備考` が `displays.note` で、整理の手順書が「初期値・物理名はそこへ写す」と
    # 名指ししている欄だとは辿れない（実測 r001）。**脚注は 1 本にまとめる** ――
    # 章末に同じ文が列の数だけ並ぶと、読み手はどれも読まない。
    named: list[str] = []
    escaped: list[str] = []
    for index, alt in hidden:
        column = _qualified(owner, str(paths[index] or ""))
        target = _qualified(owner, alt)
        label = f"「{tables[0].columns[index]}」（`{column}` → `{target}`）"
        (escaped if _escaped_to(alt) else named).append(label)
        tables[-1].misdirected.append(
            (where, tables[0].columns[index], column, target))
    if named:
        tables[-1].notes.append(
            "全行が空ですが、同じ名前の別の欄には値がある列: " + "・".join(named)
            + "。指す先が 1 つに決まる取り違えです（`arp4 check` の W043）。")
    # **「見つけた」と「見ていない」を同じ文で言わない。** 実測（kotonoha r001）で
    # この脚注 9 本すべてが下の側だったのに、上の文言で出ていた ―― 読み手は
    # 「資料にはある」と受け取り、資料に無い列まで次のラウンドの宿題に積んだ。
    if escaped:
        tables[-1].notes.append(
            "全行が空で、この母集合が `description` を使っている列: "
            + "・".join(escaped)
            + "。**`description` の中身がこの列の値かは見ていません** ―― "
              "開いて確かめ、別の値なら資料にその列があるかを当たってください"
              "（`arp4 check` の W047）。")

    if not empty or len(empty) >= width:
        return                                  # 1 列も残らないなら畳まない

    # **見出しだけでなく、どこを見に行けば埋まるかを出す。** 実測（r001）で
    # 画面帳票項目定義書は「全行が空だったので省略した列: 備考」とだけ書いた ――
    # その `備考` が `displays.note` だとは、この 1 行からは辿れない。
    folded = [(where, tables[0].columns[i],
               _qualified(owner, str(paths[i] or ""))) for i in empty]
    dropped = [name + (f"（`{path}`）" if path else "")
               for _, name, path in folded]
    keep = [i for i in range(width) if i not in empty]
    for block in tables:
        block.columns = [block.columns[i] for i in keep]
        block.rows = [[row[i] if i < len(row) else "" for i in keep]
                      for row in block.rows]
        if len(block.paths) == width:
            block.paths = [block.paths[i] for i in keep]
        # 出典の列は位置で覚えているので、畳んだあとの位置へ付け替える
        block.source_columns = {new for new, old in enumerate(keep)
                                if old in block.source_columns}
    tables[-1].notes.append(
        "全行が空だったので省略した列: " + "・".join(dropped)
        + "。全部の列を出すには `arp4 publish --full`。")
    tables[-1].folded_columns += len(dropped)
    tables[-1].folded += folded


def _qualified(owner: str, path: str) -> str:
    """``note`` → ``displays.note``。既に持ち主を名乗っているものは触らない。"""
    return f"{owner}.{path}" if owner and path and "." not in path else path


def _needs(section: dict[str, Any]) -> list[str]:
    """この章が要る語彙。**空の設計書に「何が足りないか」を書く**ためだけに使う。"""
    return ([str(section["type"])] if section.get("type") else []) \
        + relation_names(section)


def relation_names(section: dict[str, Any]) -> list[str]:
    """章が使う関係型。**``kind: trace`` だけ配列で書ける。**

    被覆（``realizes``）を 1 本の関係でしか見られなかったころ、非機能要件 15 件が
    全件「未実現の要件（設計漏れ）」に並んだ ―― 実測（r001）で 7 件は
    セキュリティ方式・共通方式が ``constrains`` で応えており、要件定義書
    「制約が縛るもの」にはちゃんと出ていた。``realizes.from`` は ``設計要素``
    グループで ``constraint`` を含まないので、**方式が要件に応える経路は
    ``constrains`` にしか無い**。関係を 1 本に決め打つと、様式が語彙の形を
    決めてしまう。

    書き方は 1 本でも配列でもよい（``relation: realizes`` / ``[realizes,
    constrains]``）。**どれか 1 本あれば被覆**とみなす ―― 和集合である。
    """
    value = section.get("relation")
    if isinstance(value, list):
        return [str(v) for v in value if v]
    return [str(value)] if value else []


def barren(blocks: list[Block]) -> bool:
    """1 行も出なかった設計書か（節を束ねるだけの章は数えない）。"""
    tables = [b for b in blocks if not b.heading_only]
    return bool(tables) and not any(b.rows for b in tables)


def _display_key(spec: Spec, item: dict[str, Any]) -> tuple[Any, ...]:
    """**表示 ID 順**の並び替えキー。表示 ID を持たない種別は名前順。

    採番は id（ハッシュ）順に走り、節は別の属性で割るので、名前順に並べると節の
    中の番号が `FR-006, FR-024, FR-004` と散る ―― **読み手は番号順に読む**ので、
    番号を動かさずに並びだけを合わせる。未採番は末尾へ（先頭に来ると「番号順の
    表」に見えなくなる）。
    """
    definition = spec.metamodel.item_types.get(str(item.get("type"))) or {}
    attribute = sequence_module.display_attribute(definition)
    value = str(item.get(attribute) or "") if attribute else ""
    return (not value, sequence_module.sort_key(value) if value else (),
            str(item.get("name") or ""), str(item.get("id") or ""))


def _live(spec: Spec, type_name: str, where: Any = None) -> list[dict[str, Any]]:
    """廃止は載せない。**同じ正本からは同じ並び**にする。"""
    rows = [i for i in spec.of_type(type_name) if i.get("status") != "deprecated"]
    return sorted((r for r in rows if matches(r, where)),
                  key=lambda r: _display_key(spec, r))


def _source_files(item: dict[str, Any]) -> set[str]:
    return {str(s.get("file")) for s in (item.get("source") or [])
            if isinstance(s, dict) and s.get("file")}


def _self_produced(spec: Spec, rows: list[dict[str, Any]],
                   type_name: str) -> tuple[list[dict[str, Any]], list[dict[str, Any]]]:
    """**その出典ファイルから ``type_name`` が起きているものを分ける。**

    「テストの無いモジュール（テスト漏れ）」に `tests.test_paths` が並んでいた
    ―― 実測で 85 件中 31 件がテスト側のモジュールで、**漏れの一覧として使えない**。
    テストファイルにテストが無いのは漏れではなく、そのファイルがテストである。

    判定は**意味ではなく出典の照合**である ―― 名前が `test_` で始まるかを見ると
    それは意味の判断（整理層の仕事）になるうえ、命名規約の違う資産（`*Spec.java`・
    `*_test.go`）で外れる。`tests/test_paths.py` から `test-case` が 11 件起きて
    いるという**正本に書いてある事実**だけを見れば、規約にも言語にも依らない。

    返すのは（残すもの, 外したもの）。外したものは黙って落とさず数で言う。
    """
    producers = {f for item in spec.of_type(type_name) for f in _source_files(item)}
    if not producers:
        return rows, []
    kept = [r for r in rows if not (_source_files(r) & producers)]
    return kept, [r for r in rows if r not in kept]


def _group_order(spec: Spec, type_name: str, attribute: str,
                 names: Iterable[str]) -> list[str]:
    """節の並び。**宣言順が先、宣言に無い値は文字コード順で末尾。**

    `sorted()` だけで並べていたので、節の順が**文字コード順**になっていた ――
    `nf_category` は IPA 非機能要求グレードの 6 大項目を**順序のある enum として
    宣言してある**のに、出てくるのは `システム環境・エコロジー → 性能・拡張性 →
    運用・保守性`（カタカナが先、あとは漢字のコードポイント順）で、宣言した順序を
    捨てていた。読み手には工程順にも重要度順にも見えず、目次から現在地を掴めない。

    「宣言順が先・未宣言は後ろ」は :func:`build._union` と同じ規律である。
    `extensible` な enum を殺さないために、宣言に無い値も落とさず末尾へ並べる。
    """
    definition = spec.metamodel.item_types.get(type_name) or {}
    values = ((definition.get("attributes") or {}).get(attribute) or {}).get("values")
    declared = [str(v) for v in (values or [])]
    rank = {name: index for index, name in enumerate(declared)}
    return sorted(names, key=lambda n: (rank.get(n, len(declared)), n))


def _items_blocks(spec: Spec, section: dict[str, Any],
                  heading: str, full: bool = False) -> list[Block]:
    columns = [str(c) for c in (section.get("columns") or [])]
    rows = _live(spec, str(section.get("type")), section.get("where"))
    group_by = section.get("group_by")

    buckets: dict[str, list[dict[str, Any]]] = {}
    for row in rows:
        buckets.setdefault(str(row.get(group_by) or "未分類"), []).append(row)

    # **束が 1 つなら節に割らない。** 分類が宣言されていない正本では全件が
    # 「未分類」に集まり、`4.1 未分類` という中身の無い節が 1 つだけ生える。
    # 節は「他と比べる単位」なので、比べる相手が無いなら見出しの水増しである。
    if not group_by or len(buckets) < 2:
        block = _block(spec, 2, heading, columns,
                       [_item_cells(spec, r, columns, full) for r in rows],
                       section.get("labels"))
        block.sources = _pairs(rows)
        return [block]

    # 章（束ねるだけ）→ 節（表）。番号は描画時に並びから決まる。
    blocks = [Block(2, heading, heading_only=True)]
    order = _group_order(spec, str(section.get("type")), str(group_by), buckets)
    gloss = value_labels(spec, str(section.get("type")), str(group_by))
    for name in order:
        block = _block(spec, 3, gloss.get(name, name), columns,
                       [_item_cells(spec, r, columns, full)
                        for r in buckets[name]], section.get("labels"))
        block.sources = _pairs(buckets[name])
        blocks.append(block)
    return blocks


def value_labels(spec: Spec, type_name: str, attribute: str) -> dict[str, str]:
    """enum の値 → 見出しに出す語（``business`` → ``業務``）。

    **値そのものは訳さない。** 訳すと整理結果に書いた値と正本の値がずれ、
    ``arp4 check`` の enum 検査（``E011``）が自分の出力を弾く ―― 見出しだけを
    差し替える。値は機械の語彙、見出しは人の語彙である。
    """
    definition = spec.metamodel.item_types.get(type_name) or {}
    attr = ((definition.get("attributes") or {}).get(attribute) or {})
    return {str(k): str(v) for k, v in (attr.get("value_labels") or {}).items()}


def _relation_blocks(spec: Spec, section: dict[str, Any],
                     heading: str, full: bool = False) -> list[Block]:
    """関係 1 本 = 表の 1 行。列は ``from.`` / ``to.`` で両端の属性も引ける。"""
    relation_type = str(section.get("relation") or "")
    columns = [str(c) for c in (section.get("columns") or [])]
    by_id = spec.by_id

    relations = _live_relations(spec, relation_type)
    # 並べるのは内部 ID ではなく**表示 ID**である（→ :func:`_display_key`）。
    # 1 つの表に畳んだとき、内部 ID 順は読み手には順不同にしか見えない。
    # `order`（資料に現れた順）は from の中での順序なので、キーの 2 番目に置く。
    relations.sort(key=lambda r: (_display_key(spec, by_id.get(str(r.get("from"))) or {}),
                                  _order_key(r),
                                  _display_key(spec, by_id.get(str(r.get("to"))) or {})))

    buckets: dict[str, list[dict[str, Any]]] = {}
    for relation in relations:
        buckets.setdefault(str(relation.get("from")), []).append(relation)

    # 束が 1 つなら節に割らない（→ :func:`_items_blocks`）。
    if section.get("group_by") != "from" or len(buckets) < 2:
        made = [_block(spec, 2, heading, columns,
                       [_relation_cells(spec, r, columns, by_id, full)
                        for r in relations], section.get("labels"))]
        made[0].sources = _ends(relations, by_id)
        return _noted_gaps(spec, relation_type, set(buckets), made)

    blocks: list[Block] = [Block(2, heading, heading_only=True)]
    for item_id, group in buckets.items():
        owner = by_id.get(item_id) or {}
        label = str(owner.get("name") or item_id)
        for key in ("physical_name", "screen_id", "report_id", "class_name", "code_id"):
            if owner.get(key):
                # 整理層が付けた名前は `表示 ID の採番（sequence）` のように識別子を
                # 名前へ畳んでいることがある。そこへ同じ値をもう一度足すと
                # `（sequence）（sequence）` になる（→ :func:`_reference` と同じ規律）。
                # 見るのは末尾の一致だけ ―― `画面（一覧）` のように別の理由で括弧が
                # 付いた名前から識別子を落とすと、同名のアイテムを見分けられなくなる。
                #
                # **名前が識別子そのものということもある。** 論理名と物理名が
                # 一致する資産（コード・DDL）ではこれが普通で、足すと
                # `Anchor（Anchor）` になる ―― 実測でテーブル定義書の全 33 節が
                # 同語反復になった。足しても見分けは 1 つも増えないので足さない。
                if label != owner[key] and not label.endswith(f"（{owner[key]}）"):
                    label = f"{label}（{owner[key]}）"
                break
        block = _block(spec, 3, label, columns,
                       [_relation_cells(spec, r, columns, by_id, full)
                        for r in group], section.get("labels"))
        block.sources = _ends(group, by_id)
        blocks.append(block)
    return _noted_gaps(spec, relation_type, set(buckets), blocks)


def _ends(relations: Iterable[dict[str, Any]],
          by_id: dict[str, dict[str, Any]]) -> set[tuple[str, str]]:
    """関係の**両端のアイテム**の出典。関係そのものは出典を持たない。"""
    items = [by_id[str(r.get(side))] for r in relations for side in ("from", "to")
             if str(r.get(side)) in by_id]
    return _pairs(items)


def _noted_gaps(spec: Spec, relation_type: str, present: set[str],
                blocks: list[Block]) -> list[Block]:
    """**known_gaps を宣言して節が無いものを、黙って消さずに脚注で言う。**

    ``group_by: from`` の章は関係が 1 本も無いアイテムの節を作らない ―― 実測
    （r001）で画面帳票項目定義書には SCR-003 ログインの節が**黙って存在しなかった**。
    正本には「共通基盤の画面にはサブシステム別の画面仕様書が無い。先方へ提供を
    依頼する」という known_gaps の宣言があるのに、生成物からは「項目が無い画面」と
    「資料が無い画面」の区別が読めない ―― `parse` が最も強く守っている
    「『資料に無い』と『機械が読めていない』を混ぜない」を、生成の段でも守る。

    見るのは**宣言だけ**である（無い節を全部並べると升目の省略と同じ議論に戻る
    ―― 宣言していないものは W031 が check で言う）。注記は表を持つ最後の塊に
    付ける ―― 章ぜんぶが空のときは注記ごと畳まれるが、そちらは :func:`_no_data`
    が理由を書くので二重には言わない。
    """
    definition = spec.metamodel.relation_types.get(relation_type) or {}
    declared: list[str] = []
    for type_name in (definition.get("from") or []):
        for item in spec.of_type(type_name):
            if item.get("status") == "deprecated" or str(item.get("id")) in present:
                continue
            gap = (item.get("known_gaps") or {}).get(relation_type)
            if isinstance(gap, dict) and gap.get("reason"):
                declared.append(f"「{item.get('name') or item.get('id')}」"
                                f"（{gap['reason']}）")
    if declared:
        tables = [b for b in blocks if b.rows]
        if tables:
            tables[-1].notes.append(
                "資料に定義が無いことを宣言しているため出ていないもの"
                "（known_gaps）: " + " ／ ".join(sorted(declared)))
    return blocks


def _matrix_blocks(spec: Spec, section: dict[str, Any],
                   heading: str, full: bool = False) -> list[Block]:
    """行 × 列の対応表。セルは関係の属性（無ければ ○）。

    **1 本も関係が無い行・列は既定で出さない。** r001 の CRUD 図は 22 × 16 の升の
    うち埋まったのが 7.2%、権限マトリクスは 30.3% で、空欄が支配的な表は読み手に
    「ツールが壊れている」ようにしか見えなかった。空欄の大半は資料に画面 × テーブル
    の CRUD 表が無いことの正しい反映なのだが、**「資料に無い」と「関係を張り忘れた」
    が升からは見分けられない。**

    ただし**黙って落とさない** ―― 畳んだ行・列は脚注に名前つきで全部並べる
    （→ :func:`_omitted`）。全部の升が要るなら ``arp4 publish --full``。
    """
    relation_type = str(section.get("relation") or "")
    row_types = section.get("rows") or []
    col_types = section.get("cols") or []
    cell_attribute = section.get("cell")

    row_items = [i for t in row_types for i in _live(spec, str(t))]
    col_items = [i for t in col_types for i in _live(spec, str(t))]

    cells: dict[tuple[str, str], str] = {}
    # **升に実際に出た値**を、連結する前の粒で持つ。凡例はこれを見て文面を選ぶ
    # ―― 語彙にあるかどうかで選ぶと、**1 升も無い値の説明**を出す（→ :func:`_legend`）。
    seen: set[str] = set()
    for relation in spec.relations_of(relation_type):
        if relation.get("status") == "deprecated":
            continue
        value = relation.get(cell_attribute) if cell_attribute else None
        if value not in (None, "", []):
            seen.update(str(v) for v in
                        (value if isinstance(value, list) else [value]))
        cells[(str(relation.get("from")), str(relation.get("to")))] = (
            _plain(value) if value not in (None, "", []) else "○")

    notes: list[str] = []
    if not full:
        row_items, dropped_rows = _used(row_items, {f for f, _ in cells})
        col_items, dropped_cols = _used(col_items, {t for _, t in cells})
        notes = _omitted(spec, dropped_rows, "行") + _omitted(spec, dropped_cols, "列")
        if notes:
            notes.append("全部の升を出すには `arp4 publish --full`")
        # **母集合そのものを「省略した」と言わない。** その関係が正本に 1 本も
        # 無ければ升は 1 つも残らず、脚注は母集合の名前を全部並べるだけになる
        # ―― 実測で空の CRUD 図は 2.6KB の大半をモジュール 111 件の名前に
        # 使っていた。空である理由は :func:`_no_data` が語彙の名前で書く。
        #
        # 見るのは**関係が 1 本も無いこと**であって升が残らなかったことではない
        # ―― 張られた関係が全部 `deprecated` なら升は同じく 0 だが、あちらは
        # 「廃止したので消えた」であって「語彙がまだ無い」ではない。名前を
        # 出さないと、読み手は届いている資料を探しに行く。
        if not any(True for _ in spec.relations_of(relation_type)):
            notes = []

    columns = [str(section.get("row_header") or "機能")] \
        + [str(i.get("name") or i.get("id")) for i in col_items]
    rows = [[str(row.get("name") or row.get("id"))]
            + [cells.get((str(row.get("id")), str(col.get("id"))), "")
               for col in col_items]
            for row in row_items]
    # **凡例は表の前に置く。** 升の意味を書かない対応表は、読み手が記号を
    # 推測するしかない（→ :func:`_legend`）。
    return [Block(2, heading, columns, rows,
                  notes=_legend(spec, relation_type, cell_attribute,
                                cells, seen) + notes,
                  sources=_pairs([*row_items, *col_items]))]


def _legend(spec: Spec, relation_type: str, cell_attribute: Any,
            cells: dict[tuple[str, str], str],
            seen: set[str] | None = None) -> list[str]:
    """升の凡例。**空欄が何を意味するかを必ず書く。**

    実測（r001 の権限マトリクス）で起きたこと ―― 原典は ``○=可 / △=部長職のみ可
    / ×=不可`` の 3 値だが、整理層は「× は関係を張らない」で写した。生成物の空欄は
    そこで **「不可」と「そもそも記載が無い」の 2 つ**を同じ見た目にしていた。
    しかも凡例そのものが落ちていたので、読み手には区別する手掛かりが 1 つも無い。

    CLAUDE.md が言う「『資料に無い』と『機械が読めていない』を混ぜない」は、
    パースだけの規律ではない ―― **升目は混ぜやすさが最も高い表現**である。
    ここで断言できるのは関係の有無だけなので、**断言できることだけを書く**。

    **語彙にあることと、この表に出ていることは違う。** ここは長いあいだ
    「``negative`` が宣言されているか」だけで文面を選んでいた ―― メタモデルに
    ``不可`` を足した（決定 71）あとの実測で、``operates`` 38 本すべてが
    ``permission`` を持たないのに、凡例は「**不可 は資料が明示的に禁じている
    ことを表し、空欄とは別である**」と **1 升も無い値の説明**を出し、出どころとして
    **1 件も無い属性**（``operates.permission``）を名指しした。読み手はそこに
    ``不可`` が無いことを「禁止が無い」と読む ―― 原典は同じ升を ``×`` と書いている。
    **語彙の穴を読み手に肩代わりさせるのをやめたつもりで、正本の穴を肩代わり
    させていた。** 以来、選ぶ材料は ``seen``（実際に升へ出た値）である。

    升が 1 つも無いときは書かない ―― 読む升が無いのに凡例だけ残ると、
    :func:`_no_data` が言う「語彙がまだ無い」を凡例が上書きしてしまう。
    """
    if not cells:
        return []
    seen = seen or set()
    definition = spec.metamodel.relation_types.get(relation_type) or {}
    label = definition.get("label")
    what = f"{label}（{relation_type}）" if label else relation_type
    used = sorted({v for v in cells.values() if v})
    attribute = ((definition.get("attributes") or {}).get(cell_attribute) or {}
                 if cell_attribute else {})
    negative = [str(v) for v in (attribute.get("negative") or [])]
    shown = [v for v in negative if v in seen]

    lines = [f"凡例: 升は「{what}」の関係があることを表す。"
             + (f"値は {'・'.join(used)}。" if used and used != ["○"] else "")]
    if shown:
        # **否定が升に出ているときだけ言い切れる。** 語彙にあるだけでは足りない
        # ―― 出ていない値を凡例が約束すると、空欄の意味が反転して伝わる。
        listed = "・".join(shown)
        lines[-1] += (f"{listed} は資料が明示的に禁じていることを表し、"
                      f"空欄とは別である"
                      f"（空欄は権限の記載が正本に無いことを意味する）。")
    else:
        lines[-1] += ("空欄は関係が正本に無いことだけを意味し、"
                      "「不可」「対象外」を意味しない"
                      "（禁止であることは、資料にそう書いてあっても升では表せない）。")
        if negative:
            # 語彙はあるのに 1 升も無い ―― **「資料に禁止が無い」とは限らない。**
            # 断言できるのは「正本に無い」ことだけなので、そこで止める。
            listed = "・".join(negative)
            lines[-1] += (f" なお {listed} は書ける語彙だが、この表には"
                          f"1 件もない。原典が禁止を書いているなら、"
                          f"正本に写されていない。")
    if cell_attribute and seen:
        lines[-1] += f" 升の値の出どころは {relation_type}.{cell_attribute}。"
    return lines


def _live_relations(spec: Spec, relation_type: str) -> list[dict[str, Any]]:
    """廃止を除いた、その型の関係。描画と検査（→ :func:`lint`）で同じ母集合を使う。"""
    return [r for r in spec.relations_of(relation_type)
            if r.get("status") != "deprecated"]


def _used(items: list[dict[str, Any]],
          linked: set[str]) -> tuple[list[dict[str, Any]], list[dict[str, Any]]]:
    """関係のあるものと無いものに分ける（並びは変えない）。"""
    keep = [i for i in items if str(i.get("id")) in linked]
    return keep, [i for i in items if str(i.get("id")) not in linked]


def _omitted(spec: Spec, dropped: list[dict[str, Any]], axis: str) -> list[str]:
    """畳んだ行・列を**種別ごとに名前つきで**並べる。

    件数だけだと「何が落ちたのか」を確かめるのに正本を引き直すことになる。
    捨てたものは必ず見せる ―― 省略した名前は途中で切らない。
    """
    if not dropped:
        return []
    buckets: dict[str, list[str]] = {}
    for item in dropped:
        type_name = str(item.get("type") or "")
        label = str((spec.metamodel.item_types.get(type_name) or {}).get("label")
                    or type_name or "種別不明")
        buckets.setdefault(label, []).append(str(item.get("name") or item.get("id")))
    parts = [f"{label} {len(names)} 件（{'・'.join(sorted(names))}）"
             for label, names in sorted(buckets.items())]
    return [f"関係の記載が無いため省略した{axis}: " + " ／ ".join(parts)]


def _qualifiers(spec: Spec) -> dict[str, str]:
    """同名のアイテムに付ける**所有元の修飾子**（`受注ヘッダ.得意先コード`）。

    同じ表示名の行が区別できずに並んでいた ―― 実測（r001）で `data-item` 226 件の
    うち **40 名称・131 件が同名**（商品コード 9・受注番号 7・得意先名 7 …）で、
    要件定義書「制約が縛るもの」のデータ項目 113 行のうち **100 行は表示名だけでは
    どれのことか決まらなかった**::

        | CST-008 得意先コードのドメイン | 得意先コード（データ項目） | ― |
        | CST-008 得意先コードのドメイン | 得意先コード（データ項目） | ― |

    **正本は正しい。** ``data-item.name`` は論理名だけを持ち、テーブル固有の
    物理名・PK・NOT NULL は ``has-column`` の側にある（同じ「受注番号」が複数
    テーブルに現れるため ―― :file:`metamodel.yml` のコメント）。問題は**出すときに
    親を辿っていない**ことだけである。

    **種別ごとの特別扱いにはしない。** 規則は 1 つで、

        同一種別に同名のアイテムが 2 件以上あるとき、**所有関係を 1 ホップ辿って**
        修飾子を付ける。所有関係とは ``ordered: true`` かつ
        ``cardinality.from: "1..*"`` を持つ関係である。

    順序があって「起点は必ず 1 件以上を持つ」と宣言された関係は、**並びの中の
    1 行として存在するもの**を指している（:func:`arp4.validate._near_duplicates`
    が同じ形を「構造的な種別」として除いている）。多重度の宣言まで見るのは、
    ``leads-to`` / ``proceeds-to`` のような**同種別どうしの鎖**を外すためで、
    あれを辿ると「直前のステップ」が所有者になってしまう。

    親が 2 つ以上あるとき（同じデータ項目を複数のテーブル・画面が指すとき）は
    **修飾子を並べない** ―― `名称（親が 3 件）` として、表示 ID から辿らせる。
    ここで 3 つ全部を並べると、区別のための修飾子が行の中でいちばん長くなる。

    章の ``labels:`` とは衝突しない ―― あちらが名指しするのは**列の見出し**で、
    ここが書き換えるのは**セルの中の相手の名前**である。

    結果は :class:`arp4.spec.Spec` に載せて使い回す（1 冊ごとに正本を全走査すると
    束ぜんぶで二乗になる）。``publish`` は正本を書き換えないので、件数が変わって
    いなければ作り直さない。
    """
    stamp = (len(spec.items), len(spec.relations))
    cached = getattr(spec, "_arp4_qualifiers", None)
    if cached is not None and cached[0] == stamp:
        return cached[1]
    built = _build_qualifiers(spec)
    setattr(spec, "_arp4_qualifiers", (stamp, built))
    return built


def _build_qualifiers(spec: Spec) -> dict[str, str]:
    counts: dict[tuple[str, str], int] = {}
    for item in spec.items:
        name = str(item.get("name") or "")
        if name:
            key = (str(item.get("type") or ""), name)
            counts[key] = counts.get(key, 0) + 1
    ambiguous = {key for key, count in counts.items() if count > 1}
    if not ambiguous:
        return {}

    owning = [name for name, definition in spec.metamodel.relation_types.items()
              if definition.get("ordered")
              and str((definition.get("cardinality") or {}).get("from") or "")
              == "1..*"]
    parents: dict[str, set[str]] = {}
    for relation_type in owning:
        for relation in _live_relations(spec, relation_type):
            parents.setdefault(str(relation.get("to")), set()).add(
                str(relation.get("from")))

    by_id = spec.by_id
    qualified: dict[str, str] = {}
    for item in spec.items:
        name = str(item.get("name") or "")
        if (str(item.get("type") or ""), name) not in ambiguous:
            continue
        owners = sorted(parents.get(str(item.get("id"))) or set())
        if len(owners) == 1:
            owner = by_id.get(owners[0]) or {}
            label = str(owner.get("name") or "")
            if label and label != name:
                qualified[str(item.get("id"))] = f"{label}.{name}"
        elif len(owners) > 1:
            qualified[str(item.get("id"))] = f"{name}（親が {len(owners)} 件）"
    return qualified


def _reference(spec: Spec, item: dict[str, Any]) -> str:
    """他の章のアイテムを**セルの中で**指すときの書き方。

    **表示 ID を頭に置き、種別ラベルは付けない。** 接頭辞（`MOD` / `SCR` / `UT`）が
    種別を語るので、ラベルは同じことを 2 回言っている。しかも整理層が付けた名前は
    `同一性の台帳（concepts）` のように括弧で終わることがあり、そこへ
    `（モジュール・クラス）` を足すと**括弧が 2 つ並んで何が何だか読めない**
    （実測 42 セル）。表示 ID なら短くなり、HTML ではその番号へ飛べる。

    表示 ID を持たない種別（用語・エンティティ）だけラベルで補う ―― ただし
    名前が既に `）` で終わっているなら足さない。

    **同名のアイテムには所有元の修飾子を付ける**（→ :func:`_qualifiers`）。
    """
    name = _qualifiers(spec).get(str(item.get("id")), "") \
        or str(item.get("name") or item.get("id") or "")
    definition = spec.metamodel.item_types.get(str(item.get("type"))) or {}
    attribute = sequence_module.display_attribute(definition)
    display = str(item.get(attribute) or "") if attribute else ""
    if display:
        return f"{display} {name}"
    label = str(definition.get("label") or "")
    return f"{name}（{label}）" if label and not name.endswith("）") else name


def _trace_blocks(spec: Spec, section: dict[str, Any],
                  heading: str, full: bool = False) -> list[Block]:
    """``linked`` 列に「この関係で繋がっている相手」を並べる。

    ``gap: true`` なら**繋がっていないものだけ**を残す。
    トレースは埋まっている部分より**空いている部分に価値がある。**

    ただし**関係が 1 本も無いときは、対応表も漏れの一覧も出さない。** 漏れの一覧は
    「張られている中に、張られていないものが混じっている」から意味を持つ。
    1 本も無ければ残るのは母集合そのもので、それは調べた結果ではなく
    **その語彙をまだ取り込んでいない**ということである ―― テスト結果報告書は
    実施記録（`test-run`）が 1 件も無いのに 596 件を「未実施」として並べ、
    116KB かけてテスト仕様書の再掲になっていた。「実施していない」と
    「実施記録を取り込んでいない」は、次の一手が正反対である。

    **これは長いあいだ ``gap: true`` の側にしか効いていなかった。** 前向きの
    対応表は同じ条件で全行 `―` の母集合を出し続け、実測（r001）では
    「要件 → テストケース」67 行・「モジュール → テストケース」20 行・
    「事業目標 → 要件」10 行・「業務フロー → 業務要件」4 行の**計 101 行が
    全行 `―`** で並んだ。しかも対になる「未検証の要件（テスト漏れ）」は
    *対象データが無いので省略*されており、**同じ事実から逆の判断が 2 つ**
    出ていた（``P101`` の docstring が言っているのはこれである）。

    出さずに畳む。理由は :attr:`Block.blank_reason` に持たせて
    :func:`_drop_empty` が脚注に書く ―― 「対象データが無い」では嘘になる
    （母集合はある。無いのは関係のほうである）。全部見たいときは ``--full``。
    """
    relation_types = relation_names(section) or ["realizes"]
    columns = [str(c) for c in (section.get("columns") or [])]
    by_id = spec.by_id

    # **複数の関係型を和集合で見る**（→ :func:`relation_names`）。同じ相手が
    # 2 本の関係で繋がっていても 1 度しか並べない ―― 読み手には同じ 1 件である。
    incoming: dict[str, set[str]] = {}
    for relation_type in relation_types:
        for relation in spec.relations_of(relation_type):
            origin = by_id.get(str(relation.get("from")))
            label = (_reference(spec, origin) if origin
                     else str(relation.get("from")))
            incoming.setdefault(str(relation.get("to")), set()).add(label)

    if not incoming and not full:
        listed = []
        for relation_type in relation_types:
            label = (spec.metamodel.relation_types.get(relation_type)
                     or {}).get("label")
            listed.append(f"`{relation_type}`（{label}）" if label
                          else f"`{relation_type}`")
        block = _block(spec, 2, heading, columns, [])
        block.blank_reason = "・".join(listed) + " が正本に 1 本も無い"
        return [block]

    items = _live(spec, str(section.get("type")), section.get("where"))
    excluded: list[dict[str, Any]] = []
    produced = str(section.get("exclude_sources_of") or "")
    if produced and section.get("gap") and not full:
        items, excluded = _self_produced(spec, items, produced)

    rows: list[list[str]] = []
    for item in items:
        linked = sorted(incoming.get(str(item.get("id")), set()))
        if section.get("gap") and linked:
            continue
        enriched = {**item, "linked": "、".join(linked) or "―"}
        rows.append(_item_cells(spec, enriched, columns, full))
    # linked 列の見出しは「何と対応させた表なのか」で変わるので章側で上書きできる。
    override = {"linked": str(section["link_label"])} if section.get("link_label") else {}
    block = _block(spec, 2, heading, columns, rows, override)
    block.sources = _pairs(items)
    if excluded:
        label = str((spec.metamodel.item_types.get(produced) or {}).get("label")
                    or produced)
        block.notes.append(
            f"出典が{label}そのものなので外した行: {len(excluded)} 件"
            f"（{'・'.join(sorted(str(i.get('name') or i.get('id')) for i in excluded))}）"
            "。全部の行を出すには `arp4 publish --full`。")
    return [block]


# ── セル ────────────────────────────────────────────────────────
def _block(spec: Spec, level: int, heading: str,
           columns: list[str], rows: list[list[str]],
           override: dict[str, str] | None = None) -> Block:
    """列の見出しは**属性名から引く**が、章側で上書きできる。

    上書きの鍵は書いたままの列名（`from.step_id`）でも葉（`linked`）でもよい。
    **両端が同じ種別の関係を 1 つの表に畳むと、見出しが必ずぶつかる** ――
    `leads-to` を 1 表にすると `ステップID` が 2 列並び、どちらが分岐元なのかが
    読めなくなる（節に割って回避していた頃は起きなかった）。
    """
    override = override or {}
    return Block(level, heading,
                 [override.get(c) or override.get(_leaf(c))
                  or spec.metamodel.label(_leaf(c)) for c in columns], rows,
                 {i for i, c in enumerate(columns) if _leaf(c) == "source"},
                 paths=list(columns),
                 id_column=_id_column(spec, columns))


def _id_column(spec: Spec, columns: list[str]) -> int | None:
    """表示 ID の列。**HTML でそこへ飛べるようにする**（`…#FR-005`）。

    番号に業務的な意味を持たせない代わりに、番号で引けるようにする ―― レビューで
    `FR-005` と書けば、その行が開く。列は「どの種別の表示 ID か」ではなく
    **葉の名前が表示 ID のどれかであること**で決める（関係の表では `to.step_id` の
    ように前置きが付くため）。
    """
    known = {sequence_module.display_attribute(d)
             for d in spec.metamodel.item_types.values()}
    known.discard(None)
    for index, column in enumerate(columns):
        if _leaf(column) in known:
            return index
    return None


def _leaf(column: str) -> str:
    return column.split(".", 1)[1] if "." in column else column


def _order_key(relation: dict[str, Any]) -> tuple[int, Any]:
    value = relation.get("order")
    return (0, value) if isinstance(value, int) else (1, str(value or ""))


def _item_cells(spec: Spec, item: dict[str, Any], columns: list[str],
                full: bool = False) -> list[str]:
    return [_value(item, column, full) for column in columns]


def _relation_cells(spec: Spec, relation: dict[str, Any], columns: list[str],
                    by_id: dict[str, dict[str, Any]],
                    full: bool = False) -> list[str]:
    cells: list[str] = []
    for column in columns:
        # **`from` / `to` を属性なしで書くと「指す」列になる**（`MOD-027 arp4.paths`）。
        # 相手の長文を写す代わりにこれを置く ―― N 対 1 の関係では相手の属性が
        # 辺の数だけ複製され、`verifies` では 1 つの仕様文が 392 行に並んでいた
        # （テスト仕様書 657KB のうち 525KB がこの章）。指せば相手の章で読める。
        if column in ("from", "to"):
            end = by_id.get(str(relation.get(column)))
            cells.append(_reference(spec, end) if end else "―")
        elif column.startswith("from.") or column.startswith("to."):
            side, attribute = column.split(".", 1)
            cells.append(_value(by_id.get(str(relation.get(side))) or {},
                                attribute, full))
        else:
            cells.append(_value(relation, column, full))
    return cells


def _value(record: dict[str, Any], column: str, full: bool = False) -> str:
    if column == "source":
        return _source(record.get("source"), full)
    if column == "status":
        return _STATUS_LABEL.get(str(record.get("status")),
                                 str(record.get("status") or ""))
    value = record.get(column)
    if value is True:
        return "○"
    if value is False:
        return ""
    if value in (None, "", []):
        return "―"
    return _plain(value)


def _plain(value: Any) -> str:
    if isinstance(value, list):
        return "".join(str(v) for v in value) if all(
            len(str(v)) == 1 for v in value) else "、".join(str(v) for v in value)
    return str(value)


#: 出典セルに出す件数の上限。**残りは件数で言う**（``--full`` で全部出る）。
_SOURCE_LIMIT = 2


def _source(source: Any, full: bool = False) -> str:
    """出典。**そのまま辿れる形で出す。**

    正本が持つのは ``{round, file, anchor}`` なので、
    ``r001 資料/A/基本設計書.xlsx/受注テーブル#s1-t1`` と出す ―― これは
    ``rounds/r001/parsed/資料/A/基本設計書.xlsx/受注テーブル.md`` の ``s1-t1`` を
    そのまま指しており、**設計書の行から元資料まで人の手で辿れる。**
    出典の列が空のままだと、生成物のレビューで「どこに書いてあったのか」に
    答えられなくなる。

    **ただし全部は出さない。** 同じ要件が 9 か所の資料に出てくると出典セルが
    仕様の本文より長くなり、Markdown の表は幅を出典が決めるので**読ませたい列が
    潰れる**（実測 ``FR-004`` の出典 9 件）。辿るのに要るのは 1 つで足りる ――
    どれも同じ concept を指しているからである。

    畳んだぶんは ``ほか N 件`` と**数で言う**。件数だけ・黙って落とすのどちらも
    「資料に無い」と「機械が出していない」を混ぜる（列の省略と同じ規律）。
    全部要るなら ``arp4 publish --full``。
    """
    if not source:
        return "―"
    entries = [e for e in (source if isinstance(source, list) else [source])
               if isinstance(e, dict)]
    parts: list[str] = []
    for entry in entries:
        if entry.get("file"):
            round_name = str(entry.get("round") or "")
            anchor = str(entry.get("anchor") or "")
            parts.append((f"{round_name} " if round_name else "")
                         + str(entry["file"]) + (f"#{anchor}" if anchor else ""))
            continue
        location = entry.get("location")       # 外部から持ち込んだ出典の形
        if isinstance(location, dict):
            location = "、".join(f"{k}={v}" for k, v in location.items())
        parts.append(str(entry.get("doc") or "")
                     + (f"（{location}）" if location else ""))
    parts = [p for p in parts if p]
    if not full and len(parts) > _SOURCE_LIMIT:
        rest = len(parts) - _SOURCE_LIMIT
        parts = parts[:_SOURCE_LIMIT] + [f"ほか {rest} 件"]
    return " / ".join(parts) or "―"


# ── Markdown ────────────────────────────────────────────────────
#: 升目の中で Markdown が**別のもの**として読む記号。逃がさないと画面から消える。
#:
#: 逃がしていなかったあいだ、実測で **2071 升のうち 12 升**が HTML 版と違う字を
#: 出していた。`|` と改行しか見ていなかったので、消えたのは記号ではなく**名前**
#: である ―― `arp4.__init__` は `__` を強調記法として食われて **`arp4.init`**、
#: `yamlio.marked(where: Any = '<text>')` は `<text>` を生の HTML タグとして
#: 食われて `where: Any = ''` になっていた。**実在しないモジュール名・実物と
#: 違う既定値**を設計書が名乗る。
#:
#: HTML 版（:func:`_html`）は :func:`html.escape` で正しく出ていたので、**同じ
#: 正本から出した 2 つの形が違うものを言っていた** ―― この食い違いこそが機械で
#: 見つけられるもので、`tests/test_publish.py` が md と html の升目を突き合わせる。
#:
#: 全 ASCII 記号を逃がす手もある（CommonMark はどれも逃がせる）が、
#: `sequence\.assign\(spec: Spec, \*, ...\)` となって**原文が読めなくなる**。
#: 逃がすのは描画を変える記号だけにして、それで足りることは実測で確かめる。
_MD_SPECIAL = "\\`*_[]<>&|~"


def _md_cell(value: str) -> str:
    """升目 1 つを Markdown へ。**読み戻して元に戻る形にする。**

    改行を ``<br>`` にするのは、表の升の中では改行がそのままでは行にならない
    ためである（:func:`_html` も同じ形にしてある ―― **2 つの形で同じ字を出す**）。
    """
    text = "".join("\\" + char if char in _MD_SPECIAL else char for char in value)
    return text.replace("\n", "<br>")


def _markdown(spec: Spec, definition: dict[str, Any], blocks: list[Block],
              meta: dict[str, Any], gate: gate_module.Gate | None = None,
              depth: int = 0, brief: Brief | None = None) -> str:
    title = str(definition.get("title") or definition.get("name"))
    lines = [f"# {title}", ""]

    # **帯は題の直後**。目次より前・本文より前に置く ―― 脚注へ回すと、
    # 表だけ読んで閉じる読み方（設計書はそう読まれる）から見えない。
    lines += gate_module.banner(gate, depth)

    if barren(blocks):
        lines += [f"> {_no_data(spec, definition, blocks)}", ""]

    # 概要は**表にしない**。md を表にすると HTML の概要（``<dl>``）と升目の数が
    # 食い違い、「同じ正本から出した 2 つの形は同じ字を出す」の検査が
    # 見た目の違いを誤りとして拾う ―― 出すべきものは同じ**事実**であって、
    # 同じ**升目**ではない（→ :meth:`Brief.facts`）。
    if brief is not None:
        lines += ["> この設計書について: "
                  + "。".join(f"{label} {value}" + (f"（{note}）" if note else "")
                              for label, value, note in brief.facts())
                  + f"。出典の一覧は [{origins_module.STEM[2:]}]"
                    f"({'../' * depth}{origins_module.STEM}.md)。", ""]

    if meta:
        lines += ["| 項目 | 内容 |", "|---|---|"]
        for label, key in _COVER_FIELDS:
            if meta.get(key):
                lines.append(f"| {label} | {meta[key]} |")
        lines.append("")

    revisions = meta.get("revisions") or []
    if revisions:
        lines += ["## 改訂履歴", "", "| 版 | 日付 | 作成者 | 内容 |", "|---|---|---|---|"]
        for revision in revisions:
            lines.append(f"| {revision.get('version', '')} | {revision.get('date', '')} "
                         f"| {revision.get('author', '')} | {revision.get('note', '')} |")
        lines.append("")

    numbers = _numbering(blocks)
    lines += ["## 目次", ""]
    for number, block in zip(numbers, blocks):
        lines.append("  " * (block.level - 2) + f"- {number} {block.heading}")
    footnote = gate_module.footnote(gate)
    lines += ["", f"> この文書は生成物です。**直接編集しないでください**"
                  f"（`arp4 publish` で再生成されます）。"
                  f"アイテム {len(spec.items)} 件 / 関係 {len(spec.relations)} 件。"
                  + (" " + footnote if footnote else ""), ""]

    for number, block in zip(numbers, blocks):
        lines += ["#" * block.level + f" {number} {block.heading}", ""]
        if block.heading_only:
            continue
        if not block.rows:
            lines += ["（該当なし）", ""]
        else:
            lines.append("| " + " | ".join(_md_cell(c) for c in block.columns) + " |")
            lines.append("|" + "|".join(["---"] * len(block.columns)) + "|")
            for row in block.rows:
                lines.append("| " + " | ".join(_md_cell(c) for c in row) + " |")
            lines.append("")
        for note in block.notes:               # 畳んだ行・列は表の下に必ず出す
            lines += [f"> {note}", ""]
    return "\n".join(lines) + "\n"


def unmet(spec: Spec, needs: list[str]) -> list[tuple[str, str, str]]:
    """足りない語彙 1 つずつを ``(語彙, 区分, 説明)`` にする。

    **区分は 2 つある。**

    ``資料``
        その種別のアイテムが 1 件も無い。資料が届いていない（か、整理層が
        その種別に寄せなかった）。次の一手は資料を足すこと。

    ``関係``
        **両端のアイテムは正本にあるのに、関係が 1 本も張られていない。**
        次の一手は資料を足すことではない ―― 整理層が張れなかった理由
        （必須属性が資料から読めない等）を潰すことである。

    この 2 つを言い分けるのが要点である。実測では CRUD 図が空になったのは
    ``accesses`` の ``crud`` が必須で整理層が推測を避けたためだったのに、
    設計書には「取り込んだ資料に『基本設計』工程の資料が含まれていない
    可能性があります」と出ていた ―― **読み手を反対側へ誘導していた。**
    エンティティ 33 件は正本にあり、資料は届いていた。

    arp4 が ``parse`` で最も強く守っている「**『資料に無い』と『機械が
    読めていない』を混ぜない**」を、最後の段でも守るための関数である。
    """
    found: list[tuple[str, str, str]] = []
    for name in needs:
        definition = spec.metamodel.relation_types.get(name)
        if definition is None:
            count = sum(1 for _ in spec.of_type(name))
            if count == 0:
                found.append((name, "資料", "アイテムが 1 件もありません"))
            continue

        if sum(1 for _ in spec.relations_of(name)) > 0:
            continue                          # 張られている（空の原因はここではない）

        # **どちら側が 0 件なのかを名前で言う。** 「相手がありません」だけだと、
        # 次に探すのが test-run なのか test-case なのかが読めない ―― テスト結果
        # 報告書で空になるのは実施記録（test-run）が無いときだけで、テストケース
        # 596 件は届いている。
        sides = {side: [t for t in (definition.get(side) or [])]
                 for side in ("from", "to")}
        ends = [sum(sum(1 for _ in spec.of_type(t)) for t in sides[side])
                for side in ("from", "to")]
        if not all(ends):
            bare = [t for side, label in (("from", "起点"), ("to", "終点"))
                    for t in sides[side]
                    if not any(True for _ in spec.of_type(t))]
            found.append((name, "資料", "関係の相手になるアイテムがありません"
                          + (f"（`{'` / `'.join(bare)}` が 0 件）" if bare else "")))
            continue

        required = sorted(k for k, a in (definition.get("attributes") or {}).items()
                          if a.get("required"))
        found.append((name, "関係",
                      f"両端のアイテム（起点 {ends[0]} 件 / 終点 {ends[1]} 件）は"
                      "正本にありますが、関係が 1 本も張られていません"
                      + (f"（{name} は {'、'.join(required)} が必須です）"
                         if required else "")))
    return found


def pending(spec: Spec) -> list[Finding]:
    """**資料は届いているのに空になる設計書**を、``publish`` の前に言う。

    段の境界のうち ``check`` → ``publish`` がいちばん空いていた ―― 実測では
    CRUD 図が空だと分かるのが**生成して人が目で見たとき**で、しかも設計書には
    「資料が無いのかもしれない」と出ていた（→ :func:`unmet`）。

    資料が無いだけのもの（要件定義書・テスト仕様書）は**ここでは言わない。**
    それは正しい空であり、``publish`` が本文に書けば足りる ―― 実測で 11 種の
    うち 8 種がそれに当たるので、全部並べると本当に困っているものが埋もれる。

    判定は :func:`_blocks` と :func:`unmet` を ``publish`` と共有する。
    **規則が 2 つあると同じ問題が形を変えて戻る。**
    """
    findings: list[Finding] = []
    for definition in catalog(spec):
        blocks = _blocks(spec, definition)
        if not barren(blocks):
            continue
        needs = sorted({n for b in blocks for n in b.needs})
        blocked = [entry for entry in unmet(spec, needs) if entry[1] == "関係"]
        if not blocked:
            continue
        title = str(definition.get("title") or definition.get("name"))
        findings.append(Finding(
            "warn", "W034", title,
            "資料は届いているのに空になります。"
            + "。".join(f"{name} は{why}" for name, _, why in blocked)
            + "（整理層が関係を張れなかった理由を潰してください）"))
    return findings


# ── 文書定義の検査 ──────────────────────────────────────────────
#: アイテムの表に置ける、属性ではない列。:func:`_value` が特別に描画する。
_ITEM_EXTRA = frozenset({"id", "status", "source"})

#: 関係の表に置ける、関係の予約キー由来の列（→ ``metamodel.RELATION_RESERVED``）。
#: ``from`` / ``to`` は「指す列」なので別扱い（→ :func:`_relation_cells`）。
_RELATION_EXTRA = frozenset({"order", "description", "status", "source"})

#: 章種別ごとに文書定義へ書ける鍵。**未知の鍵は黙って無視される**（``colums`` と
#: 打ち間違えると列の無い表が出る）ので、語彙を閉じて検査する。
_SECTION_KEYS: dict[str, frozenset[str]] = {
    "items": frozenset({"heading", "kind", "columns", "labels",
                        "type", "where", "group_by"}),
    "relation": frozenset({"heading", "kind", "columns", "labels",
                           "relation", "group_by"}),
    "matrix": frozenset({"heading", "kind", "labels",
                         "relation", "row_header", "rows", "cols", "cell"}),
    "trace": frozenset({"heading", "kind", "columns", "labels", "type",
                        "relation", "where", "gap", "link_label",
                        "exclude_sources_of"}),
}


def lint(spec: Spec) -> list[Finding]:
    """文書定義（パックの ``documents/*.yml``）を**メタモデルに照らして**検査する。

    ========  ==============================================================
    E040      列・種別・関係・鍵がメタモデルで解決できない（文書定義の誤り）
    W043      列は解決できるが全行が空で、値は**同じ名前の別の欄**にある
    W047      列は解決できるが全行が空で、母集合が ``description`` を使っている
              （**中身は照合していない** → :func:`_escaped_to`）
    ========  ==============================================================

    ``W046``（畳んだ列）は組み上がった章を見ないと言えないので :func:`folded` に
    ある ―― ここは文書定義だけを読む。

    背景（実測 r001・sales-corpus 30 冊）: コード定義の列が ``value``
    （正しくは ``to.value``）と書かれていた。解決できない列は**エラーにならず
    全行空の列**になり、:func:`_trim` が「全行が空だったので省略した列: コード値」
    と畳んだ ―― **文書定義の誤りが「資料に無い」と同じ顔で出ていた。** 値の列を
    失ったコード定義表には No 連番だけが残り、受注ステータスの表が「1=受付」と
    読めた（実際は 10）。読み手にも書き手にも、誤りだと気づく手掛かりが無い。

    ``E040`` を **error** にするのは、列の書き間違いが**パックの欠陥であって
    データの欠落ではない**からである ―― 直す場所は正本ではなく ``documents/*.yml``
    で、放置すると全プロジェクトの同じ表が黙って欠ける。``W043`` を warn に
    留めるのは、両側に同名の属性が宣言されている場合、どちらを指すかは様式の
    選択でありうるからである（確かめてから直す）。
    """
    findings: list[Finding] = []
    for definition in catalog(spec):
        name = str(definition.get("name") or definition.get("title") or "")
        for section in definition.get("sections") or []:
            findings += _lint_section(spec, name, section)
    return findings


def folded(prepared: list[tuple[dict[str, Any], list["Block"]]]) -> list[Finding]:
    """``W046`` ―― **様式が持つ列を全行空で畳んだ。**

    :func:`_trim` は畳んだ列を章末の脚注に 1 行書くが、**そこで終わっていた**
    ―― 束としては数えられず、``_gate.json`` にも穴の 1 枚のコード表にも
    現れない。実測（r001）で 23 列が畳まれ、ゲートの件数は 1 つも動かなかった。

    「資料に無い」とは言っていない。**様式が持つ欄に正本が値を 1 つも
    持たない**、としか言えない ―― 資料にその欄が無いのか、整理層が写さな
    かったのかは、資料を見た人にしか分からない。値が**別の欄にある**ものは
    ここに来ない（:func:`_trim` が畳まずに残し、``W043`` が言う）。

    材料は組み上がった章（:class:`Block`）なので、**:func:`arp4.audit.audit` が
    組み立てたものをそのまま受け取る** ―― ここで組み直すと、束ぜんぶを
    3 度組み立てることになる。

    **1 列 1 件では出さない。** 様式が持つ列に段階的にデータが追いつくのは正常な
    途中経過で、実測（アイテム 5 件の例）でも 18 列が畳まれる ―― 1 行ずつ出すと
    ゲートの件数がそれだけで埋まり、**本物の指摘が件数に埋もれる**（:func:`_repeats`
    が出典列を外したのと同じ判断）。直す単位は章なので、**章 1 件にまとめて列を
    全部名指しする** ―― 1 件ずつの詳細は `0_この設計書の穴.md` の表にある。
    """
    findings: list[Finding] = []
    for definition, blocks in prepared:
        title = str(definition.get("title") or definition.get("name"))
        chapters: dict[str, list[tuple[str, str]]] = {}
        for block in blocks:
            for where, column, path in block.folded:
                columns = chapters.setdefault(where, [])
                if (column, path) not in columns:
                    columns.append((column, path))
        for where, columns in chapters.items():
            listed = "・".join(f"「{column}」" + (f"（`{path}`）" if path else "")
                               for column, path in columns)
            findings.append(Finding(
                "warn", "W046", f"{title}「{where}」",
                f"{len(columns)} 列を全行空で省略しました: {listed}",
                hint="資料がその欄を持たないならこのままでよい。"
                     "整理層が別の欄へ書いていないか、"
                     "`0_この設計書の穴.md` の表で確かめる"))
    return findings


def _lint_section(spec: Spec, doc: str,
                  section: dict[str, Any]) -> list[Finding]:
    kind = str(section.get("kind") or "items")
    target = f"{doc}「{section.get('heading') or ''}」"
    known_keys = _SECTION_KEYS.get(kind)
    if known_keys is None:
        return [Finding("error", "E040", target, f"未知の章種別です: {kind}")]

    findings: list[Finding] = []
    for key in section:
        if str(key) not in known_keys:
            findings.append(Finding(
                "error", "E040", target, f"文書定義に書けない鍵です: {key}",
                hint=f"kind: {kind} の章で使える鍵: "
                     + "、".join(sorted(known_keys))))

    columns = [str(c) for c in (section.get("columns") or [])]
    #: 解決できなかった列。W043（値の側の検査）から除く ―― 同じ列を 2 回言わない。
    broken: set[str] = set()

    if kind in ("items", "trace"):
        findings += _lint_item_columns(spec, target, section, columns, broken,
                                       trace=(kind == "trace"))
    if kind in ("relation", "matrix", "trace"):
        findings += _lint_relation_side(spec, target, section, columns, broken,
                                        kind)
    findings += _lint_labels(target, section, columns)

    # **``kind: relation`` の章にしか走っていなかった。** 一覧・トレースの章で
    # 同じことが起きても誰も言わない ―― 実測（r001）で画面一覧の `route` が
    # 全行空のまま、値は `description` の散文に入っていた。升目は列を持たない
    # （:data:`_SECTION_KEYS`）ので、渡しても素通りする。
    findings += _lint_misdirected(spec, target, section, columns, broken, kind)
    return findings


def _lint_item_columns(spec: Spec, target: str, section: dict[str, Any],
                       columns: list[str], broken: set[str],
                       trace: bool) -> list[Finding]:
    """アイテムの表（items / trace）の列・``where``・``group_by`` の解決。"""
    findings: list[Finding] = []
    type_name = str(section.get("type") or "")
    definition = spec.metamodel.item_types.get(type_name)
    if definition is None:
        broken.update(columns)
        return [Finding("error", "E040", target,
                        f"未知の種別です: {type_name or '（type 未指定）'}")]

    allowed = set(definition.get("attributes") or {}) | _ITEM_EXTRA
    if trace:
        allowed.add("linked")               # trace が組み立てる計算列
    for column in columns:
        if "." in column:
            broken.add(column)
            findings.append(Finding(
                "error", "E040", target,
                f"アイテムの表に `from.` / `to.` の列は書けません: {column}"))
        elif column not in allowed:
            broken.add(column)
            findings.append(Finding(
                "error", "E040", target,
                f"列 `{column}` は {type_name} の属性にありません"))

    group_by = section.get("group_by")
    if group_by and str(group_by) not in allowed:
        findings.append(Finding(
            "error", "E040", target,
            f"group_by が {type_name} の属性にありません: {group_by}"))
    where = section.get("where")
    for key in (where if isinstance(where, dict) else {}):
        if str(key) not in allowed:
            findings.append(Finding(
                "error", "E040", target,
                f"where の鍵が {type_name} の属性にありません: {key}"))

    produced = section.get("exclude_sources_of")
    if produced and str(produced) not in spec.metamodel.item_types:
        findings.append(Finding("error", "E040", target,
                                f"exclude_sources_of が未知の種別です: {produced}"))
    return findings


def _lint_relation_side(spec: Spec, target: str, section: dict[str, Any],
                        columns: list[str], broken: set[str],
                        kind: str) -> list[Finding]:
    """関係を使う章（relation / matrix / trace）の、関係側の解決。

    **``kind: trace`` だけ関係を配列で書ける**（→ :func:`relation_names`）ので、
    そこは名前が引けるかを 1 本ずつ見る ―― 配列をそのまま文字列にすると
    ``['realizes', 'constrains']`` という関係型を探して ``E040`` が誤検出する。
    """
    findings: list[Finding] = []
    if kind == "trace":
        # trace の列はアイテム側で解決済み（→ :func:`_lint_item_columns`）。
        names = relation_names(section)
        if not names:
            return [Finding("error", "E040", target,
                            "未知の関係型です: （relation 未指定）")]
        return [Finding("error", "E040", target, f"未知の関係型です: {name}")
                for name in names
                if name not in spec.metamodel.relation_types]

    relation_type = str(section.get("relation") or "")
    definition = spec.metamodel.relation_types.get(relation_type)
    if definition is None:
        broken.update(columns)
        return [Finding("error", "E040", target,
                        f"未知の関係型です: {relation_type or '（relation 未指定）'}")]

    relation_attrs = set(definition.get("attributes") or {}) | _RELATION_EXTRA

    if kind == "matrix":
        known = set(spec.metamodel.item_types) | set(spec.metamodel.groups)
        for axis in ("rows", "cols"):
            for type_name in (section.get(axis) or []):
                if str(type_name) not in known:
                    findings.append(Finding(
                        "error", "E040", target,
                        f"{axis} に未知の種別があります: {type_name}"))
        cell = section.get("cell")
        if cell and str(cell) not in relation_attrs:
            findings.append(Finding(
                "error", "E040", target,
                f"cell が {relation_type} の属性にありません: {cell}"))
        return findings

    ends = {side: [str(t) for t in (definition.get(side) or [])]
            for side in ("from", "to")}

    def end_attrs(side: str) -> set[str] | None:
        """その辺に来うる種別の属性の合併。**宣言の無い関係は検査できない。**"""
        if not ends[side]:
            return None
        merged = set(_ITEM_EXTRA)
        for type_name in ends[side]:
            merged |= set((spec.metamodel.item_types.get(type_name) or {})
                          .get("attributes") or {})
        return merged

    for column in columns:
        if column in ("from", "to"):
            continue                        # 相手を「指す」列（→ _relation_cells）
        if "." in column:
            side, attribute = column.split(".", 1)
            if side not in ("from", "to"):
                broken.add(column)
                findings.append(Finding("error", "E040", target,
                                        f"列の前置きが不正です: {column}"))
                continue
            allowed = end_attrs(side)
            if allowed is None or attribute in allowed:
                continue
            broken.add(column)
            hint = (f"`{attribute}`（関係の属性）の書き間違いかもしれません"
                    if attribute in relation_attrs else None)
            findings.append(Finding(
                "error", "E040", target,
                f"列 `{column}` が解決できません"
                f"（{'・'.join(ends[side])} の属性にありません）", hint=hint))
        elif column not in relation_attrs:
            broken.add(column)
            hint = None
            for side in ("to", "from"):
                allowed = end_attrs(side)
                if allowed is not None and column in allowed:
                    hint = f"`{side}.{column}` の書き間違いかもしれません"
                    break
            findings.append(Finding(
                "error", "E040", target,
                f"列 `{column}` は {relation_type} の属性にありません", hint=hint))

    group_by = section.get("group_by")
    if group_by and str(group_by) != "from":
        findings.append(Finding(
            "error", "E040", target,
            f"関係の表の group_by に書けるのは from だけです: {group_by}"))
    return findings


def _lint_labels(target: str, section: dict[str, Any],
                 columns: list[str]) -> list[Finding]:
    """``labels`` の鍵はどれかの列（またはその葉）に一致していなければ**死んだ設定**。"""
    findings: list[Finding] = []
    labels = section.get("labels")
    valid = set(columns) | {_leaf(c) for c in columns}
    for key in (labels if isinstance(labels, dict) else {}):
        if str(key) not in valid:
            findings.append(Finding(
                "error", "E040", target,
                f"labels の鍵がどの列にも一致しません: {key}"))
    return findings


def _lint_misdirected(spec: Spec, target: str, section: dict[str, Any],
                      columns: list[str], broken: set[str],
                      kind: str = "relation") -> list[Finding]:
    """解決はできるのに**全行が空**で、値は別の欄にある列（``W043`` / ``W047``）。

    両側に同名の属性が宣言されていると :func:`_lint_relation_side` は通す ――
    どちらを指すかは静的には決められないので、**データを見て**言う。

    見る母集合は章の種別で決まる ―― 関係の表なら関係、一覧とトレースなら
    アイテムである。**升目は列を持たない**ので何も見ない。

    出すのは**章 1 件**である（列 1 件ではない）。直す先は 1 つの節の定義なので、
    列の数だけ行が並んでも打ち手は増えない（→ :func:`folded` と同じ規律）。

    **強さの違う 2 つを同じコードで出さない**（→ :func:`_escaped_to`）。
    指す先が同名の属性なら、値の置き場所は 1 つに決まる（``W043``）。
    指す先が ``description`` なら、言えているのは「この列は空」と「この母集合は
    ``description`` を使っている」の 2 つだけで、**中身は照合していない**
    （``W047``）―― それは ``W046`` と同じ「まだ割れていない」状態である。

    実測（kotonoha r001）で 9 件すべてが後者だった。1 つのコードに混ざって
    いたころ、手順書は代表して前者の意味（「資料にはある。書き先を間違えた」）を
    書いており、**読み手は「資料に無い」という結論へ辿り着けなかった。**
    """
    records = _population(spec, section, kind)
    if not records:
        return []
    by_id = spec.by_id
    found: list[tuple[str, str]] = []
    for column in columns:
        if column in broken:
            continue                        # E040 で言った列を 2 回言わない
        if kind == "relation":
            rendered = (_relation_cells(spec, r, [column], by_id)[0]
                        for r in records)
        else:
            rendered = (_value(r, column) for r in records)
        if any(cell.strip() not in _BLANK for cell in rendered):
            continue
        alt = _alternative(spec, section, kind, column, records)
        if alt is not None:
            found.append((column, alt))
    if not found:
        return []
    named = [(c, a) for c, a in found if not _escaped_to(a)]
    escaped = [(c, a) for c, a in found if _escaped_to(a)]
    findings: list[Finding] = []
    if named:
        listed = "、".join(f"`{column}` → `{alt}`" for column, alt in named)
        findings.append(Finding(
            "warn", "W043", target,
            f"{len(named)} 列が全行空ですが、同じ名前の別の欄に値があります: {listed}",
            hint="指す先が 1 つに決まる取り違えです。documents/*.yml の列を"
                 "直すか、整理層の書き先を名指しされた欄へ移してください"))
    if escaped:
        listed = "、".join(f"`{column}` → `{alt}`" for column, alt in escaped)
        findings.append(Finding(
            "warn", "W047", target,
            f"{len(escaped)} 列が全行空で、この母集合は description を"
            f"使っています: {listed}",
            hint="**description の中身がこの列の値かは見ていません**（照合して"
                 "いない点は W046 と同じです）。description を開いて確かめ、"
                 "別の値なら出典の資料にその列があるかを 1 件ずつ当たって"
                 "ください。資料に無ければ空のままで構いません"))
    return findings


def _population(spec: Spec, section: dict[str, Any],
                kind: str) -> list[dict[str, Any]]:
    """その章が行にするレコード。**升目は列を持たないので空。**"""
    if kind == "relation":
        return _live_relations(spec, str(section.get("relation") or ""))
    if kind in ("items", "trace"):
        return _live(spec, str(section.get("type") or ""), section.get("where"))
    return []


def _alternative(spec: Spec, section: dict[str, Any], kind: str, column: str,
                 records: list[dict[str, Any]] | None = None) -> str | None:
    """全行が空になった列の値が、正本の**別の欄**にあるならそのパスを返す。

    :func:`_lint_misdirected`（``W043``）と :func:`_trim`（畳むかどうか）が
    **同じ関数で決める** ―― 別々に決めていたころ、`W043` が鳴った列を `_trim` が
    畳んだり、畳んだ列を誰も指摘しなかったりした。同じ判定を 1 か所に置けば、
    「畳んだ列」と「別の欄に値がある列」は構造的に排他になる。
    """
    if records is None:
        records = _population(spec, section, kind)
    if not records:
        return None
    if kind == "relation":
        return _elsewhere(spec, records, spec.by_id, column)
    if kind in ("items", "trace"):
        if column == "linked":
            return None                     # トレースが組み立てる計算列
        return _escaped(records, column)
    return None


def _filled(value: Any) -> bool:
    return value not in (None, "", [], False)


def _escaped_to(alt: str) -> bool:
    """その指し先が ``description`` か ―― **中身を照合していない答え**か。

    :func:`_alternative` が返すのは 2 種類ある。同名の属性（``to.value`` /
    ``note``）は「この列の値はそこにある」と言えているが、``description`` は
    :func:`_escaped` が ``any(...)`` で決めたものでしかない ―― **母集合の
    どれか 1 件が ``description`` を持っていれば返る。**空の列の名前と
    ``description`` の中身を突き合わせる処理はどこにも無い。

    強さが違うので**コードを分ける**（``W043`` / ``W047``）。混ぜると、
    ``--code W043`` で開いたときに「移せばよいもの」と「資料に当たるまで
    何も決まらないもの」が同じ顔で並ぶ。

    実測（kotonoha r001）の要件定義書「非機能要件」では、``category`` を鳴らした
    ``description`` は SLO 5 件の「測り方／確認」で、分類とは無関係だった ――
    しかも分類がありそうなセキュリティ 5 件は ``description`` が空で、判定に
    1 件も寄与していない。**引き金を引いたレコードと、列が欲しかった
    レコードが別だった。**
    """
    return _leaf(alt) == "description"


def _escaped(records: list[dict[str, Any]], column: str) -> str | None:
    """同じレコードの ``description`` へ値が逃げていないか。

    ``description`` は :data:`arp4.metamodel.RELATION_RESERVED` の予約キーで、
    宣言なしにどの関係へも書ける ―― **スキーマ検査を素通りする逃がし先**である。
    実測（r001）で `displays` 164 本すべてが `note` 空のまま、初期値と画面側の
    物理名が `description` へ流れた。

    **総当たりはしない**（対象は ``description`` だけ）。「空の列 A の値は
    埋まっている列 B にある」を全部言うと、埋まっている列の数だけ鳴る ――
    受け皿が 1 つに決まっているものだけが、次の一手を名指しできる。

    宛先が ``description`` 自身の列と、両端を指す列（``from.`` / ``to.``）は
    見ない ―― 後者が空なのは**相手のアイテム**が値を持たないということで、
    関係の補足とは主語が違う。
    """
    if "." in column or column == "description":
        return None
    if column in ("source", "status"):
        return None                         # 管理キーはどちら側にもある
    return "description" if any(
        _filled(r.get("description")) for r in records) else None


def _elsewhere(spec: Spec, relations: list[dict[str, Any]],
               by_id: dict[str, dict[str, Any]], column: str) -> str | None:
    """全行が空になった列の値が、関係の**別の欄**にあるならそのパスを返す。

    探す先は 3 つある。同名の属性が**反対側**にあるもの ―― 関係の属性の列
    （``value``）に対する両端のアイテム（``to.value``）、端の列（``to.note``）に
    対する関係自身（``note``）。そして**同じ側の ``description``**（→
    :func:`_escaped`）である。どれも実データの有無だけを見る。**意味は判断
    しない**（値が同じものかは人が確かめる）。

    ``description`` は**最後に見る。** 先に見ると、コード定義の ``value``
    （正しくは ``to.value``）のように**指す先が 1 つに決まる取り違え**まで
    「補足に入っています」と答えることになり、次の一手が鈍る。
    """
    if not relations or column in ("from", "to"):
        return None
    leaf = _leaf(column)
    if leaf in ("source", "status"):
        return None                         # 管理キーはどちら側にもある

    def endpoints(side: str) -> list[dict[str, Any]]:
        found = [by_id.get(str(r.get(side))) for r in relations]
        return [i for i in found if i is not None]

    if "." in column:
        side, attribute = column.split(".", 1)
        if side not in ("from", "to"):
            return None
        if any(_filled(r.get(attribute)) for r in relations):
            return attribute                # 関係自身が同名の値を持っている
        other = "to" if side == "from" else "from"
        if any(_filled(i.get(attribute)) for i in endpoints(other)):
            return f"{other}.{attribute}"
        return None

    for side in ("to", "from"):
        if any(_filled(i.get(column)) for i in endpoints(side)):
            return f"{side}.{column}"
    return _escaped(relations, column)


def _no_data(spec: Spec, definition: dict[str, Any], blocks: list[Block]) -> str:
    """対象データが 0 件の設計書に書く根拠。**空の表だけ置いて済ませない。**

    「作ったが空」なのか「作っていない」のかが目次からも本文からも読めないと、
    レビューで拾えない。足りない語彙の名前と、**空になった理由の区分**まで書く
    （→ :func:`unmet`）。

    **足りている語彙を「必要な語彙」に混ぜない。** 章が要る語彙を全部並べると、
    テスト結果報告書は `executes` / `test-case` と出る ―― テストケースは 596 件
    あるのに、無いものとして名指しされる。名指しするのは :func:`unmet` が
    実際に 0 件だと言ったものだけにする。
    """
    needs = sorted({n for b in blocks for n in b.needs})
    phase = str(definition.get("phase") or "")
    reasons = unmet(spec, needs)
    missing = [name for name, _, _ in reasons]
    named = missing or needs
    head = ("**この設計書に出せるデータが正本にありません**"
            + (f"（必要な語彙: {' / '.join(f'`{n}`' for n in named)}）" if named else "")
            + "。")

    # **関係が張れていないものが 1 つでもあれば、資料の話にしない。**
    # 「資料が足りないのかもしれない」と併記すると、読み手は探しやすいほう
    # （資料）を先に疑う ―― 実際には資料は届いている。
    blocked = [r for r in reasons if r[1] == "関係"]
    if blocked:
        return head + "".join(f"`{name}` は{why}。" for name, _, why in blocked) \
            + "**資料の欠落ではありません。**関係が張られていない理由を取り除いてください。"

    # **要る語彙の一部が正本にあるなら、工程が丸ごと欠けている話にしない。**
    # テスト結果報告書が空なのはテストの資料が届いていないからではなく、
    # 実施記録という 1 語彙だけが無いからである ―― 次に探すものが違う。
    present = [n for n in needs if n not in missing
               and n not in spec.metamodel.relation_types]
    if present and reasons:
        return (head + f"{' / '.join(f'`{n}`' for n in present)} は正本にあります。"
                + "".join(f"`{name}` は{why}。" for name, _, why in reasons)
                + "「"
                + (phase or "対応する工程")
                + "」の資料が丸ごと欠けているわけではありません。")

    return (head + "取り込んだ資料に"
            + (f"「{phase}」工程の資料" if phase else "対応する資料")
            + "が含まれていない可能性があります。"
            "空であること自体が正しい出力なら、そのままで構いません。")


# ── HTML ────────────────────────────────────────────────────────
_COVER_FIELDS = (("システム名", "project"), ("文書番号", "document_no"),
                 ("版", "version"), ("発行日", "issued_on"),
                 ("作成者", "author"), ("承認者", "approver"),
                 ("作成部署", "company"), ("機密区分", "confidentiality"))


def _html(spec: Spec, definition: dict[str, Any], blocks: list[Block],
          meta: dict[str, Any], owners: dict[str, Path] | None = None,
          here: Path | None = None, index: Path | None = None,
          gate: gate_module.Gate | None = None, depth: int = 0,
          copies: Copies | None = None, brief: Brief | None = None) -> str:
    """設計書 1 冊を HTML に。**Excel の表として読める形にする**（→ :mod:`arp4.page`）。

    ``copies`` があれば出典のセルを**写しへのリンク**にする（→ :func:`Copies`）。
    ``brief`` があれば本文の前に概要を置く（→ :class:`Brief`）。どちらも
    渡さなければ何も足さない ―― 1 冊だけ組み立てる呼び出しがそのまま動く。
    """
    escape = html.escape
    owners = owners or {}
    title = str(definition.get("title") or definition.get("name"))
    parts = page_module.head(title)
    parts.append(page_module.toolbar(filters=any(b.rows for b in blocks)))
    parts.append('<div class="wrap">')
    if index is not None and here is not None:
        parts.append(f'<p class="meta"><a href="{escape(_href(index, here))}"'
                     f"{page_module.NEW_TAB}>← 生成した設計書（目次）</a></p>")
    parts.append(f"<h1>{escape(title)}</h1>")
    parts.append(gate_module.banner_html(gate, depth))

    if barren(blocks):
        parts.append(f'<p class="empty">{escape(_plain_text(_no_data(spec, definition, blocks)))}</p>')

    if brief is not None:
        parts.append(_brief_html(brief, gate, depth))

    if meta:
        rows = "".join(f"<dt>{escape(label)}</dt><dd>{escape(str(meta[key]))}</dd>"
                       for label, key in _COVER_FIELDS if meta.get(key))
        if rows:
            parts.append(f'<div class="cover"><dl>{rows}</dl></div>')

    revisions = meta.get("revisions") or []
    if revisions:
        parts.append('<h2>改訂履歴</h2><div class="sheet">'
                     '<table class="grid"><thead><tr>'
                     "<th>版</th><th>日付</th><th>作成者</th><th>内容</th>"
                     "</tr></thead><tbody>")
        for revision in revisions:
            parts.append("<tr>" + "".join(
                f"<td>{escape(str(revision.get(key, '')))}</td>"
                for key in ("version", "date", "author", "note")) + "</tr>")
        parts.append("</tbody></table></div>")

    numbers = _numbering(blocks)
    parts.append("<h2>目次</h2><nav><ol>")
    for number, block in zip(numbers, blocks):
        indent = "&nbsp;&nbsp;&nbsp;&nbsp;" * (block.level - 2)
        parts.append(f'<li style="list-style:none">{indent}'
                     f'<a href="#b{escape(number)}">'
                     f'{escape(number)} {escape(block.heading)}</a></li>')
    parts.append("</ol></nav>")
    #: 1 ページで 1 度だけ付ける表示 ID の飛び先（→ :func:`_anchor`）。
    anchored: set[str] = set()
    parts.append(f'<p class="meta">この文書は生成物です。直接編集しないでください'
                 f'（arp4 publish で再生成されます）。'
                 f'アイテム {len(spec.items)} 件 / 関係 {len(spec.relations)} 件。'
                 f'{escape(_plain_text(gate_module.footnote(gate)))}</p>')

    for number, block in zip(numbers, blocks):
        tag = f"h{block.level}"
        parts.append(f'<{tag} id="b{escape(number)}">'
                     f'{escape(number)} {escape(block.heading)}</{tag}>')
        if block.heading_only:
            continue
        if not block.rows:
            parts.append('<p class="empty">（該当なし）</p>')
        else:
            #: 固定するのは**先頭列**（Excel の「先頭列の固定」→ :mod:`arp4.page`）。
            key = 0
            parts.append(page_module.sheet(len(block.columns))
                         + '<table class="grid"><thead><tr>'
                         + "".join(f"<th>{escape(c)}</th>" for c in block.columns)
                         + "</tr></thead><tbody>")
            for row in block.rows:
                cells = ""
                for i, cell in enumerate(row):
                    mark = _anchor(block, i, cell, anchored, owners, here)
                    classes = [name for name, on in
                               (("k", i == key), ("src", i in block.source_columns))
                               if on]
                    # 改行は Markdown 側と同じ ``<br>`` にする。生の改行の
                    # ままだと HTML が空白へ潰すので、**同じ正本から出した
                    # 2 つの形が違う見え方をする**（→ :func:`_md_cell`）。
                    body = (_source_html(cell, copies, here)
                            if i in block.source_columns and copies is not None
                            else _linkify(cell, owners, here, bool(mark)))
                    cells += ("<td"
                              + (f' class="{" ".join(classes)}"' if classes else "")
                              + mark + ">" + page_module.cell(cell, body) + "</td>")
                parts.append(f"<tr>{cells}</tr>")
            parts.append("</tbody></table></div>")
        for note in block.notes:
            parts.append(f'<p class="meta">{escape(_plain_text(note))}</p>')

    parts.append("</div>")
    # 下端のシート見出し ―― 章の並びが Excel のシートと同じ位置に出る。
    tabs = page_module.tabs([(f"b{number}", block.heading)
                             for number, block in zip(numbers, blocks)
                             if block.level <= 2])
    parts += page_module.tail(tabs)
    return "\n".join(parts) + "\n"


def _brief_html(brief: Brief, gate: gate_module.Gate | None = None,
                depth: int = 0) -> str:
    """概要の帯（HTML）。**md と同じ事実を出す**（→ :meth:`Brief.facts`）。"""
    escape = html.escape
    items = "".join(
        f"<dt>{escape(label)}</dt><dd>{escape(value)}"
        + (f"<span>（{escape(_plain_text(note))}）</span>" if note else "")
        + "</dd>"
        for label, value, note in brief.facts())
    up = "../" * depth
    away = page_module.NEW_TAB
    say = ('<p>数えれば出るものだけを置いています（本文の要約ではありません）。'
           f'出典の一覧: <a href="{up}{origins_module.STEM}.html"{away}>'
           f"{escape(origins_module.STEM[2:])}</a>")
    if gate is not None and not gate.clean:
        say += (f" ／ この設計書が抱えている穴: "
                f'<a href="{up}{holes_module.STEM}.html"{away}>'
                f"{escape(holes_module.STEM[2:])}</a>")
    return f'<div class="brief"><dl>{items}</dl>{say}。</p></div>'


def _anchor(block: Block, index: int, cell: str, anchored: set[str],
            owners: dict[str, Path] | None = None,
            here: Path | None = None) -> str:
    """表示 ID のセルに ``id`` を付ける。**束の中で 1 度だけ。**

    同じ番号は複数の章に出る（`FS-001` は手順一覧にも流れの表にも出る）ので
    1 ページの中では**最初に出たところ**＝一覧の章を正とする。

    **設計書をまたいでも 1 度だけにする。** `MOD-027` は詳細設計書にも
    トレーサビリティ・マトリクスにも出るが、両方が `id` を名乗ると読み手から
    見て `#MOD-027` の飛び先が 2 つになる ―― 持ち主でなければ名乗らず、
    そのぶん**持ち主へのリンクになる**（→ :func:`_owners` と :func:`_linkify`）。
    """
    if index != block.id_column:
        return ""
    value = cell.strip()
    if not value or value in _BLANK or value in anchored:
        return ""
    if owners and here is not None and owners.get(value) not in (None, here):
        return ""
    anchored.add(value)
    return f' id="{html.escape(value)}"'


def _href(target: Path, here: Path) -> str:
    """``here`` のページから ``target`` のページへの相対パス。"""
    if target == here:
        return ""
    up = "../" * len(here.parent.parts[len(_common(target.parent, here.parent)):])
    rest = target.parts[len(_common(target.parent, here.parent)):]
    return up + "/".join(rest)


def _common(left: Path, right: Path) -> tuple[str, ...]:
    shared: list[str] = []
    for a, b in zip(left.parts, right.parts):
        if a != b:
            break
        shared.append(a)
    return tuple(shared)


def _linkify(cell: str, owners: dict[str, Path], here: Path | None,
             is_anchor: bool) -> str:
    """セルの中の表示 ID を、その番号を持つ設計書へのリンクにする。

    **アンカーはあったのに、誰も参照していなかった。** 実測でテスト仕様書は
    `TC-0001`〜`TC-0596` の 596 個の `id` を持ち、CSS にも `td:target` の
    ハイライトを用意しているのに、トレーサビリティ・マトリクスから
    `TC-0086` はただの文字列だった ―― 飛び先を作って誰も飛ばないなら、
    番号で引けるという設計（→ :func:`_numbering`）が半分しか効いていない。

    探すのは**こちらが組み立てた形の先頭**だけである（`、` で区切った各片の
    最初の語）。本文を正規表現で舐めると、仕様の文中の `G025` のような
    見た目が似ているだけの語まで拾い、**リンクが本文の意味を語り出す。**

    **別の設計書へ行くものだけ新しいタブで開く**（→ :data:`arp4.page.NEW_TAB`）。
    同じ番号でも、持ち主がこのページなら飛び先は同じページの `#FR-005` である
    ―― そこに `target` を付けると**同じ文書が 2 つ開く。**
    """
    escape = html.escape
    if not owners or here is None:
        return escape(cell).replace("\n", "<br>")

    lines: list[str] = []
    for line in cell.split("\n"):
        pieces: list[str] = []
        for segment in line.split("、"):
            head, space, rest = segment.partition(" ")
            target = owners.get(head.strip())
            if target is None or (is_anchor and not space):
                pieces.append(escape(segment))
                continue
            away = page_module.NEW_TAB if target != here else ""
            pieces.append(f'<a href="{escape(_href(target, here))}'
                          f'#{escape(head.strip())}"{away}>{escape(head)}</a>'
                          + escape(space + rest))
        lines.append("、".join(pieces))
    return "<br>".join(lines)


def _plain_text(note: str) -> str:
    """Markdown 用に付けた強調・コード記法を落とす（HTML では体裁が持つ）。"""
    return note.replace("**", "").replace("`", "")


@dataclass
class Copies:
    """パース結果の写しが**実在するか**を知っている表。

    出典のセルは最初から
    ``r001 資料/A/基本設計書.xlsx/受注テーブル#s1-t1`` という**そのまま辿れる形**
    で出ていた（→ :func:`_source`）のに、**文字列のままだった** ―― 実測 580 セル。
    読み手は 201 本の写しの中からその 1 本を手で探すことになる。

    **実在するものだけリンクにする。** 写しが消えている（ラウンドを消した・
    ファイル名を変えた）ときにリンクだけ出すと、辿れない飛び先が「辿れる」顔で
    出る ―― 「資料に無い」と「機械が出していない」を混ぜないという規律は、
    リンクにも当てはまる。

    **飛び先に ``#アンカー`` は付けない。** 写しのアンカーは HTML コメント
    （``<!-- a:s1-t1 -->``）なので、ブラウザの断片識別子としては動かない。
    動かない飛び先を付けるくらいなら、升の文字にアンカーを残して人が探すほうが
    嘘が無い（升には既に ``#s1-t1`` と書いてある）。
    """

    parsed: dict[tuple[str, str], Path] = field(default_factory=dict)

    @classmethod
    def of(cls, spec: Spec) -> "Copies":
        return cls({(round_name, file): round_.parsed / f"{file}{mdio.EXT}"
                    for (round_name, file), round_
                    in origins_module.copies(spec).items()})

    def href(self, round_name: str, file: str, here: Path | None) -> str | None:
        target = self.parsed.get((round_name, file))
        if target is None or here is None or not here.is_absolute():
            return None
        try:
            return os.path.relpath(target, here.parent).replace(os.sep, "/")
        except ValueError:                  # 別ドライブ（Windows）
            return None


def _source_html(cell: str, copies: Copies | None, here: Path | None) -> str:
    """出典のセルを、**実在する写しへのリンク**にする。

    組み立てた形（`" / "` 区切り・先頭がラウンド名）を解いて引くだけで、
    正規表現で本文を舐めない ―― :func:`_linkify` と同じ理由である
    （**リンクが本文の意味を語り出す**のを避ける）。
    """
    escape = html.escape
    if copies is None:
        return escape(cell).replace("\n", "<br>")
    pieces: list[str] = []
    for piece in cell.split(" / "):
        round_name, space, rest = piece.partition(" ")
        file = rest.partition("#")[0]
        href = copies.href(round_name, file, here) if space else None
        pieces.append(f'<a href="{escape(href)}"{page_module.NEW_TAB}>'
                      f"{escape(piece)}</a>" if href else escape(piece))
    return " / ".join(pieces).replace("\n", "<br>")
