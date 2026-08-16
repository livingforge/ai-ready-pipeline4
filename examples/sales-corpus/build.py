"""テスト資材（設計書の xlsx 一式と実装コードの Java 一式）を生成する。

    python examples/sales-corpus/build.py [--out <出力先>] [--code-out <出力先>]
                                          [--change-out <出力先>]

既定の出力先は ``examples/sales-corpus/資料/``（第1次リリースの設計書）、
``examples/sales-corpus/追加資料/``（第2次リリースの追加・是正）、
``examples/sales-corpus/実装/``（Java）。実行のたびに全ファイルを作り直すので、
生成物は追跡せず、必要なときに組み直せばよい。

``資料/`` と ``実装/`` を分けているのは、README が数えている期待値
（30 文書・201 シート・サブシステム別の件数）を実装コードで崩さないため。
``追加資料/`` を分けているのも同じ理由で、**2 度目の束として別に渡せる**ように
してある（``資料/`` を渡した次のラウンドで ``追加資料/`` を渡す）。
``arp4 parse`` に渡すのは ``資料/`` と ``追加資料/`` で、Java は読まない。
"""

from __future__ import annotations

import argparse
import shutil
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import code_impl  # noqa: E402
import docs_basic  # noqa: E402
import docs_change  # noqa: E402
import docs_detail  # noqa: E402
import docs_plan  # noqa: E402
import docs_req  # noqa: E402
import docs_screen  # noqa: E402

MODULES = (docs_plan, docs_req, docs_basic, docs_screen, docs_detail)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="販売管理システムのテスト資材を生成する")
    parser.add_argument("--out", type=Path, default=HERE / "資料", help="設計書の出力先")
    parser.add_argument("--change-out", type=Path, default=HERE / "追加資料",
                        help="追加資料（第2次リリース）の出力先")
    parser.add_argument("--code-out", type=Path, default=HERE / "実装",
                        help="実装コード（Java）の出力先")
    parser.add_argument("--clean", action="store_true", help="出力先を空にしてから生成する")
    args = parser.parse_args(argv)

    out: Path = args.out
    change_out: Path = args.change_out
    code_out: Path = args.code_out
    if args.clean:
        for target in (out, change_out, code_out):
            if target.exists():
                shutil.rmtree(target)

    paths: list[Path] = []
    for module in MODULES:
        paths.extend(module.build(out))
    for path in sorted(paths):
        print(f"  {path.relative_to(out)}")
    print(f"\n{len(paths)} ファイルを {out} へ生成しました。")

    change_paths = docs_change.build(change_out)
    for path in sorted(change_paths):
        print(f"  {path.relative_to(change_out)}")
    print(f"\n{len(change_paths)} ファイルを {change_out} へ生成しました。")

    code_paths = code_impl.build(code_out)
    for path in sorted(code_paths):
        print(f"  {path.relative_to(code_out)}")
    print(f"\n{len(code_paths)} ファイルを {code_out} へ生成しました。")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
