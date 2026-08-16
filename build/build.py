"""表面（``surface/``）から配布物を組み立てる。

**展開先に Python を 1 行も置かない。** スキルは文書だけで、実行環境は
``pip install`` が用意する（arp4 は console script ``arp4`` を持つ）。
展開先に実行体を置くと、更新のたびに 2 か所を直すことになる。

**スキルごとの専用ビルド関数を作らない。** ``surface/manifest.yaml`` を読む
汎用処理 1 本で組む ―― スキルを足すたびにビルド側へ分岐が増えると、
「足したのに出てこない」の原因が build 側に散る。

出力::

    .claude/skills/<name>/SKILL.md + docs/
    .github/skills/<name>/SKILL.md + docs/

使い方::

    python build/build.py                 # リポジトリ直下へ展開する
    python build/build.py --root <パス>   # 消費側プロジェクトへ展開する
    python build/build.py --check         # 差分があるかだけ見る（無変更）
"""

from __future__ import annotations

import argparse
import shutil
import sys
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
SURFACE = ROOT / "surface"
MANIFEST = SURFACE / "manifest.yaml"


def load_manifest() -> dict[str, Any]:
    import yaml                                   # 依存は使うときだけ読む

    return yaml.safe_load(MANIFEST.read_text(encoding="utf-8")) or {}


def _read(path: Path) -> str:
    """**行末を \\n に揃えて読む。** cp932 の Windows で CRLF が混ざると差分が騒ぐ。"""
    return path.read_text(encoding="utf-8").replace("\r\n", "\n").strip()


def _fragment(path: Path) -> str:
    """フロントマターの断片。コメント行（``#``）は展開先へ持ち出さない。"""
    if not path.is_file():
        return ""
    lines = [line for line in _read(path).splitlines()
             if line.strip() and not line.lstrip().startswith("#")]
    return "\n".join(lines)


def skill_text(skill_dir: Path, platform: str) -> str:
    """``SKILL.md`` の中身。共通フロントマター + プラットフォーム固有 + 本文。"""
    parts = [_fragment(skill_dir / "frontmatter.common.yaml"),
             _fragment(skill_dir / f"frontmatter.{platform}.yaml")]
    front = "\n".join(part for part in parts if part)
    return f"---\n{front}\n---\n\n{_read(skill_dir / 'body.md')}\n"


def build(root: Path, manifest: dict[str, Any],
          check: bool = False) -> tuple[list[Path], list[Path]]:
    """展開する。戻り値は ``(書いたファイル, 中身が変わったファイル)``。

    ``check`` なら書かずに差分だけ数える ―― CI で「展開し忘れ」を落とすため。
    """
    written: list[Path] = []
    changed: list[Path] = []

    for platform, layout in (manifest.get("platforms") or {}).items():
        base = root / str(layout.get("root") or f".{platform}")
        for skill in manifest.get("skills") or []:
            name = str(skill.get("name") or "")
            source = SURFACE / "skills" / name
            if not source.is_dir():
                raise FileNotFoundError(f"スキルの元がありません: {source}")

            target = base / "skills" / name
            if not check:
                # 宣言から外した docs を残さない（消したのに出続けるのを防ぐ）。
                shutil.rmtree(target, ignore_errors=True)
                target.mkdir(parents=True, exist_ok=True)

            files = {str(layout.get("skill_file") or "SKILL.md"):
                     skill_text(source, platform)}
            for doc in skill.get("docs") or []:
                path = source / "docs" / f"{doc}.md"
                if not path.is_file():
                    raise FileNotFoundError(
                        f"manifest に宣言された docs がありません: {path}")
                files[f"docs/{doc}.md"] = _read(path) + "\n"

            for relative, text in files.items():
                out = target / relative
                if out.is_file() and _read(out) == text.strip():
                    written.append(out)
                    continue
                changed.append(out)
                if check:
                    continue
                out.parent.mkdir(parents=True, exist_ok=True)
                out.write_text(text, encoding="utf-8", newline="\n")
                written.append(out)
    return written, changed


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="build", description=__doc__)
    parser.add_argument("--root", default=str(ROOT),
                        help="展開先（既定: このリポジトリ）")
    parser.add_argument("--check", action="store_true",
                        help="書かずに差分があるかだけ見る")
    args = parser.parse_args(argv)

    manifest = load_manifest()
    root = Path(args.root).resolve()
    written, changed = build(root, manifest, check=args.check)

    if args.check:
        if changed:
            print(f"展開が古いファイルが {len(changed)} 件あります:")
            for path in changed:
                print(f"  {path.relative_to(root)}")
            print("python build/build.py で展開してください")
            return 1
        print(f"展開は最新です（{len(written)} ファイル）")
        return 0

    print(f"{root} へ展開しました（{len(written)} ファイル / "
          f"更新 {len(changed)}）")
    for path in sorted(written):
        print(f"  {path.relative_to(root)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
