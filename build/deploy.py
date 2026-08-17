"""公開用のフォルダへ配布物を展開する。

**何を出すかはこのファイルの ``INCLUDE`` だけが決める。**中身の列挙は
``git ls-files`` に任せる ―― `.gitignore` と二重に「出さないもの」を書くと、
片方を直したときにもう片方が古いまま残り、`__pycache__` や `.venv`、
ローカル設定（`.claude/settings.local.json`）が公開側へ漏れる。

**展開先の中身は毎回作り直す。**元から消したファイルが公開側に残り続けると、
「消したのに出続ける」の原因が展開側に散る。ただし ``.git`` は残す ――
2 回目以降は同じリポジトリへの追加コミットにしたいため。

出力::

    <展開先>/.claude .github src tests build surface docs examples
             pyproject.toml README.md .gitignore
    <展開先>/.git             （--no-git でなければ init + commit まで）

使い方::

    python build/deploy.py                    # C:/arp4-publish へ展開してコミット
    python build/deploy.py --dest <パス>      # 展開先を変える
    python build/deploy.py --check            # 書かずに対象の一覧だけ見る
    python build/deploy.py --no-git           # コピーだけして git は触らない
"""

from __future__ import annotations

import argparse
import os
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

#: 子プロセスに UTF-8 で書かせる。**`encoding="utf-8"` で受ける以上、書かせる側も
#: 揃えないと辻褄が合わない** ―― 日本語 Windows の既定では、パイプへ書く Python の
#: 標準出力は cp932 になる。それを utf-8 として ``errors="replace"`` で読むと、
#: 中身は全部 ``�`` になり、**cp932 のコンソールへ出し直すところで落ちる**
#: （``�`` は cp932 に無い）。展開そのものは終わっているのに、報告の 1 行で
#: 異常終了するので、打った人からは失敗にしか見えない。
CHILD_ENV = {**os.environ, "PYTHONIOENCODING": "utf-8"}


def resilient_output() -> None:
    """**コンソールの文字コードで落ちない**（:func:`arp4.cli._resilient_output` と同じ）。

    出すものに日本語のファイル名が並ぶ ―― `資料/` の中身も `examples/` の検体も
    そうである。**出力の都合で公開作業が止まってはいけない**ので、出せない文字は
    ``backslashreplace`` で符号位置を残す（``?`` にすると、出せなかった文字と
    資料に元からある ``?`` が見分けられなくなる）。
    """
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(errors="backslashreplace")   # type: ignore[union-attr]
        except (AttributeError, OSError, ValueError):
            pass

# 公開する最上位の要素。ここに無いものは（追跡されていても）出さない。
INCLUDE = (
    ".claude",          # Claude Code 向けスキル（surface から生成）
    ".github",          # GitHub Copilot 向けスキル（surface から生成）
    # **これを出さないと、公開側だけで見本の PDF が壊れる。** git は中身を見て
    # テキストか binary かを決めるので、ほとんど ASCII の PDF はテキストと
    # 判定され、Windows で clone すると改行が変換されて開けなくなる。
    ".gitattributes",
    ".gitignore",
    "README.md",
    "build",            # 展開スクリプト（build.py・deploy.py）
    "docs",             # 設計判断の記録
    "examples",         # 検体つきのサンプル一式
    "pyproject.toml",
    "src",              # 本体
    "surface",          # スキルの元（.claude / .github はここから作る）
    "tests",            # テストコードとデータセット
)

DEFAULT_DEST = ROOT.parent / "arp4-publish"


def tracked_files() -> list[str]:
    """公開対象の相対パス。``-z`` で受けるのは**日本語のファイル名**があるため。

    既定の ``git ls-files`` は非 ASCII を ``"\\346\\255\\243..."`` と引用して返すので、
    そのまま Path に渡すと存在しないパスになる。
    """
    out = subprocess.run(
        ["git", "ls-files", "-z"],
        cwd=ROOT, check=True, capture_output=True,
    ).stdout.decode("utf-8")
    paths = [line for line in out.split("\0") if line]
    return sorted(p for p in paths if p.split("/", 1)[0] in INCLUDE)


def check_surface() -> None:
    """``.claude`` / ``.github`` が ``surface/`` と一致しているか確かめる。

    生成物をそのまま公開するので、**展開し忘れたまま公開すると古い手順書が配られる。**
    """
    result = subprocess.run(
        [sys.executable, str(ROOT / "build" / "build.py"), "--check"],
        cwd=ROOT, capture_output=True, text=True, encoding="utf-8", errors="replace",
        env=CHILD_ENV,
    )
    if result.returncode != 0:
        sys.stderr.write(result.stdout + result.stderr)
        raise SystemExit(
            "スキルの展開が古いままです。python build/build.py を先に実行してください"
        )
    print(f"表面の確認: {(result.stdout or '').strip()}")


def clear(dest: Path) -> None:
    """``.git`` を残して展開先を空にする。"""
    for entry in dest.iterdir():
        if entry.name == ".git":
            continue
        if entry.is_dir() and not entry.is_symlink():
            shutil.rmtree(entry)
        else:
            entry.unlink()


def copy(paths: list[str], dest: Path) -> int:
    """展開する。``copy2`` なのは xlsx（検体）を壊さず運ぶため。"""
    for relative in paths:
        target = dest / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(ROOT / relative, target)
    return len(paths)


def git(dest: Path, message: str) -> None:
    """展開先を git リポジトリにして 1 コミット積む。

    ``push`` はしない ―― 送り先はこのリポジトリの情報から決められないので、
    remote の指定は人の手に残す。
    """
    def run(*args: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            ["git", *args], cwd=dest,
            capture_output=True, text=True, encoding="utf-8", errors="replace",
        )

    if not (dest / ".git").is_dir():
        run("init", "-b", "main")
        print("git init（main）")

    run("add", "-A")
    if not run("diff", "--cached", "--quiet").returncode:
        print("git: 変更なし（コミットしません）")
        return

    result = run("commit", "-m", message)
    if result.returncode != 0:
        sys.stderr.write(result.stdout + result.stderr)
        raise SystemExit("コミットに失敗しました")
    print(f"git commit: {(result.stdout or '').strip().splitlines()[0]}")


def main(argv: list[str] | None = None) -> int:
    resilient_output()
    parser = argparse.ArgumentParser(prog="deploy", description=__doc__)
    parser.add_argument("--dest", default=str(DEFAULT_DEST),
                        help=f"展開先（既定: {DEFAULT_DEST}）")
    parser.add_argument("--check", action="store_true",
                        help="書かずに対象の一覧だけ見る")
    parser.add_argument("--no-git", action="store_true",
                        help="コピーだけして git は触らない")
    parser.add_argument("-m", "--message", default="Publish arp4",
                        help="コミットメッセージ")
    args = parser.parse_args(argv)

    dest = Path(args.dest).resolve()
    if dest == ROOT or ROOT in dest.parents:
        raise SystemExit(f"展開先がこのリポジトリの中です: {dest}")

    check_surface()
    paths = tracked_files()

    if args.check:
        print(f"展開対象 {len(paths)} ファイル → {dest}")
        for relative in paths:
            print(f"  {relative}")
        return 0

    dest.mkdir(parents=True, exist_ok=True)
    clear(dest)
    count = copy(paths, dest)
    print(f"{dest} へ展開しました（{count} ファイル）")

    if not args.no_git:
        git(dest, args.message)
        print("push するには remote を足してください:")
        print(f"  cd {dest}")
        print("  git remote add origin <URL>")
        print("  git push -u origin main")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
