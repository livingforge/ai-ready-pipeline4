"""検証用の資料を作る。**バイナリを置かず、中身が読める形で残す。**"""

from pathlib import Path

from openpyxl import Workbook


def build(directory: Path) -> list[Path]:
    directory.mkdir(parents=True, exist_ok=True)
    made = [_design(directory / "基本設計書.xlsx"),
            _operation(directory / "運用設計.xlsx")]
    return made


def _design(path: Path) -> Path:
    book = Workbook()
    sheet = book.active
    sheet.title = "受注テーブル"
    sheet["B2"] = "受注テーブル（T_ORDER）定義書  改訂 2.1"
    rows = [["論理名", "物理名", "型", "桁", "PK", "必須", "備考"],
            ["受注番号", "ORDER_NO", "文字列", 10, "○", "○", "採番ルールは別紙"],
            ["受注日", "ORDER_DATE", "日付", None, None, "○", None],
            ["顧客コード", "CUSTOMER_CD", "文字列", 8, None, "○", None]]
    for r, row in enumerate(rows, start=5):
        for c, value in enumerate(row, start=2):
            if value is not None:
                sheet.cell(row=r, column=c, value=value)

    cover = book.create_sheet("表紙")
    cover["A1"] = "基本設計書"
    cover["A2"] = "2026-08-01 版"

    book.save(path)
    return path


def _operation(path: Path) -> Path:
    book = Workbook()
    sheet = book.active
    sheet.title = "運用方針"
    sheet["A1"] = "受注データの保持期間"
    sheet["A2"] = "受注データは 13 か月でアーカイブする"
    book.save(path)
    return path


if __name__ == "__main__":
    for made in build(Path(__file__).parent / "from-excel" / "資料"):
        print(made)
