"""``資料/`` の Excel 8 冊を生成する。

    python examples/kotonoha/build.py [--out <出力先>] [--clean]

**生成するのは Excel だけである。** ``src/`` ``ddl/`` ``docs/``
``openapi.yaml`` は git 管理のテキストとしてそのまま置いてあり、
生成物ではない。

これは sales-corpus と**意図的に逆**にしてある。sales-corpus は
「設計書が先にあって実装が後」の検体なので全文書を ``spec.py`` から
生成したが、kotonoha は「**実装が先にあって文書が後追い**」の検体で
ある。コードを生成物にすると、いちばん試したい「コードにしか正本が
無い」状態が資材の側で崩れてしまう。

図形・コネクタ・表の機構は sales-corpus の ``xlsxkit`` を共有する
（``sheetkit.py`` の冒頭を参照）。
"""

from __future__ import annotations

import argparse
import shutil
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import docs_daicho  # noqa: E402
import docs_shinsei  # noqa: E402
import docs_tenken  # noqa: E402

MODULES = (docs_shinsei, docs_daicho, docs_tenken)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Kotonoha のテスト資材（Excel）を生成する")
    parser.add_argument("--out", type=Path, default=HERE / "資料",
                        help="Excel の出力先")
    parser.add_argument("--clean", action="store_true",
                        help="出力先を空にしてから生成する")
    args = parser.parse_args(argv)

    out: Path = args.out
    if args.clean and out.exists():
        shutil.rmtree(out)

    paths: list[Path] = []
    for module in MODULES:
        paths.extend(module.build(out))

    for path in sorted(paths):
        print(f"  {path.relative_to(out)}")
    print(f"\n{len(paths)} ファイルを {out} へ生成しました。")
    print("\nコード・DDL・設計メモは生成物ではありません（git 管理）:")
    for name, pattern in (("実装", "src/**/*.py"), ("試験", "tests/*.py"),
                          ("DDL", "ddl/*.sql"), ("設計メモ", "docs/**/*.md")):
        count = len(list(HERE.glob(pattern)))
        print(f"  {name}: {count} ファイル")
    print("  API 仕様: openapi.yaml 1 ファイル"
          "（★ arp4 は .yaml を読まない）")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
