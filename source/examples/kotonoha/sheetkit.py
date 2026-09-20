"""kotonoha の Excel 生成の下請け。

**表・図形・レイアウトの機構は sales-corpus の ``xlsxkit`` を共有する。**
図形とコネクタの drawing 部を注入するコードは ``src/arp4/parse.py`` の
読み側と対で保守する必要があり（sales-corpus の README を参照）、
二重に持つと片方だけ直る事故が起きるためである。

ここが足すのは**社内文書の表紙**だけ。sales-corpus の ``cover`` は
受託案件の様式（「○○御中」＋受託者名）なので、内製のこの資材には
合わない。社内の稟議・台帳・点検表は「部門名・起案者・決裁欄」の形になる。
"""

from __future__ import annotations

import sys
from dataclasses import dataclass, field
from pathlib import Path

#: **末尾に足す。** 先頭へ入れると sales-corpus の ``spec.py`` が
#: kotonoha の ``spec.py`` を隠してしまう（同じ名前のモジュールが両方にある）。
_SALES = Path(__file__).resolve().parent.parent / "sales-corpus"
if str(_SALES) not in sys.path:
    sys.path.append(str(_SALES))

import xlsxkit as kit  # noqa: E402

# ── 使い回す道具（そのまま通す）─────────────────────────────────
Diagram = kit.Diagram
Edge = kit.Edge
LayoutError = kit.LayoutError
Node = kit.Node
Box = kit.Box
add_sheet = kit.add_sheet
heading = kit.heading
kv_group_table = kit.kv_group_table
kv_table = kit.kv_table
layout = kit.layout
new_book = kit.new_book
note = kit.note
safe_name = kit.safe_name
save = kit.save
set_print = kit.set_print
table = kit.table

# ── この資材の登場人物 ──────────────────────────────────────────
#: 例示用に用意された実在しない会社名（Adatum / Trey Research の日本語表記）。
COMPANY = "アダタム工業株式会社"
COMPANY_SHORT = "アダタム工業"
VENDOR = "トレイリサーチ株式会社"
VENDOR_SHORT = "トレイリサーチ"

DIVISION = "デジタル基盤本部"
GROUP = "AI基盤グループ"
SYSTEM_NAME = "Kotonoha"


@dataclass
class DocInfo:
    """社内文書の表紙に載る情報。

    受託案件の ``xlsxkit.DocInfo`` と違い、**発注者がいない**。
    代わりに起案の部門と決裁の欄を持つ。
    """

    doc_name: str
    division: str = DIVISION
    group: str = GROUP
    version: str = "1.0"
    date: str = "2026/03/10"
    author: str = "田村"
    #: 決裁欄に並べる役職。左から押していく
    approvers: tuple[str, ...] = ("本部長", "部長", "課長", "起案")
    revisions: list[tuple[str, str, str, str]] = field(default_factory=list)


def cover(wb, info: DocInfo):
    """社内文書の表紙。

    ``xlsxkit.cover`` は「○○御中」＋受託者名の様式で、内製の資材には
    合わない。こちらは**部門・起案者・決裁欄**の形にする。
    """
    ws = add_sheet(wb, "表紙")
    for index, width in enumerate((3, 14, 20, 20, 20, 20, 3), start=1):
        kit._set_width(ws, index, width)

    ws.merge_cells("B3:F3")
    ws["B3"] = f"{COMPANY}　{info.division}"
    ws["B3"].font = kit.F_SUBTITLE
    ws["B3"].alignment = kit.AL_C

    ws.merge_cells("B6:F6")
    ws["B6"] = f"社内エンベディング基盤 {SYSTEM_NAME}"
    ws["B6"].font = kit.Font(name=kit.FONT, size=14, bold=True)
    ws["B6"].alignment = kit.AL_C

    ws.merge_cells("B8:F9")
    ws["B8"] = info.doc_name
    ws["B8"].font = kit.F_TITLE
    ws["B8"].alignment = kit.AL_C
    ws.row_dimensions[8].height = 30
    ws.row_dimensions[9].height = 30

    meta = [
        ("版数", f"第 {info.version} 版"),
        ("作成年月日", info.date),
        ("起案部門", f"{info.division} {info.group}"),
        ("起案者", info.author),
    ]
    row = 12
    for label, value in meta:
        cell = ws.cell(row=row, column=3, value=label)
        cell.font = kit.F_BODY
        cell.fill = kit.FILL_LABEL
        cell.border = kit.BORDER
        cell.alignment = kit.AL_CV
        ws.merge_cells(start_row=row, start_column=4, end_row=row, end_column=6)
        value_cell = ws.cell(row=row, column=4, value=value)
        value_cell.font = kit.F_BODY
        value_cell.alignment = kit.AL_CV
        for col in range(4, 7):
            ws.cell(row=row, column=col).border = kit.BORDER
        ws.row_dimensions[row].height = 20
        row += 1

    # 決裁欄（押印欄）。社内文書は左から本部長・部長・課長・起案の順。
    top = row + 2
    ws.cell(row=top, column=3, value="決裁").font = kit.F_BODY
    ws.cell(row=top, column=3).fill = kit.FILL_LABEL
    ws.cell(row=top, column=3).border = kit.BORDER
    ws.cell(row=top, column=3).alignment = kit.AL_C
    for offset, label in enumerate(info.approvers[:3]):
        col = 4 + offset
        head = ws.cell(row=top, column=col, value=label)
        head.font = kit.F_HEAD
        head.fill = kit.FILL_HEAD
        head.alignment = kit.AL_C
        head.border = kit.BORDER
        ws.cell(row=top + 1, column=col).border = kit.BORDER
    ws.cell(row=top + 1, column=3, value="").border = kit.BORDER
    ws.row_dimensions[top + 1].height = 44

    set_print(ws, landscape=False, doc_name=info.doc_name)
    return ws


def revisions(wb, info: DocInfo):
    """改訂履歴。``xlsxkit.revisions`` と同じ様式でよいので薄く包む。"""
    ws = add_sheet(wb, "改訂履歴")
    heading(ws, 2, 2, "改訂履歴")
    rows = info.revisions or [("1.0", info.date, "初版作成", info.author)]
    table(ws, top=4, left=2,
          header=["版数", "改訂日", "改訂内容", "改訂者"],
          rows=[list(r) for r in rows], widths=[8, 12, 70, 10])
    set_print(ws, doc_name=info.doc_name)
    return ws


def three_sheet(wb, info: DocInfo) -> None:
    """表紙 → 改訂履歴。本体は呼ぶ側が足す。"""
    cover(wb, info)
    revisions(wb, info)
