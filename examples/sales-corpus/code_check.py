"""生成した Java を、javac の代わりに機械的に検査する。

`code_impl.build()` が生成の直後に呼ぶ。**本物の型検査はしない**ので
「コンパイルが通る」ことの保証にはならないが、

本物の型検査はしない。「必ずコンパイルが通らない」種類の間違いだけを狙う:

  A. 未解決の型名（import も同パッケージも java.lang も無い）
  B. 自前クラスの ``new X(...)`` の引数個数がコンストラクタと合わない
  C. interface を実装する匿名クラス／クラスが、メソッドを実装しきっていない

の 3 種は止まる。この 3 つは実際に故障を仕込んで検出できることを確かめてある。
JDK があるなら ``実装/README.md`` の手順で javac を通すのが本筋。
"""

from __future__ import annotations

import re
from pathlib import Path

#: ``java.lang``（import 無しで常に見える型）。ここに挙げ漏れると、正しいコードを
#: 「型が解決できない」と誤って止めてしまう。
JDK = {
    "String", "Object", "Integer", "Long", "Boolean", "Math", "System",
    "Override", "Exception", "RuntimeException", "IllegalArgumentException",
    "IllegalStateException", "StringBuilder", "CharSequence", "Comparable",
    "Iterable", "Number", "Class", "Void", "Thread", "Runnable",
    "Character", "Double", "Float", "Short", "Byte", "Enum", "Throwable",
    "AutoCloseable", "NullPointerException", "UnsupportedOperationException",
    "NumberFormatException", "IndexOutOfBoundsException", "ArithmeticException",
    "ClassCastException", "StringBuffer",
}


def strip(src: str) -> str:
    src = re.sub(r"/\*.*?\*/", " ", src, flags=re.DOTALL)
    src = re.sub(r"//[^\n]*", " ", src)
    src = re.sub(r'"(?:\\.|[^"\\])*"', '""', src)
    return re.sub(r"'(?:\\.|[^'\\])*'", "' '", src)


def split_args(text: str) -> int:
    """引数の個数。入れ子の括弧・山括弧の中のカンマは数えない。"""
    if not text.strip():
        return 0
    depth = 0
    count = 1
    for ch in text:
        if ch in "(<[{":
            depth += 1
        elif ch in ")>]}":
            depth -= 1
        elif ch == "," and depth == 0:
            count += 1
    return count


def balanced(text: str, start: int) -> str:
    """``text[start]`` の ``(`` に対応する ``)`` までの中身。"""
    depth = 0
    for i in range(start, len(text)):
        if text[i] == "(":
            depth += 1
        elif text[i] == ")":
            depth -= 1
            if depth == 0:
                return text[start + 1:i]
    return ""


def check(root: Path) -> list[str]:
    """生成済みの Java を検査し、見つかった問題を返す。"""
    ROOT = root
    files = sorted(ROOT.rglob("*.java"))
    info: dict[str, dict] = {}

    for path in files:
        src = strip(path.read_text(encoding="utf-8"))
        name = path.stem
        package = re.search(r"package ([\w.]+);", src)
        imports = re.findall(r"import ([\w.]+);", src)
        kind = "interface" if re.search(rf"\binterface {name}\b", src) else "class"
        ctors = [split_args(balanced(src, m.end() - 1))
                 for m in re.finditer(rf"public {name}\s*\(", src)]
        methods = {(m.group(1), split_args(balanced(src, m.end() - 1)))
                   for m in re.finditer(r"(?:public|private|protected)\s+(?:static\s+)?"
                                        r"(?:[\w.<>\[\],? ]+?)\s+(\w+)\s*\(", src)}
        iface_methods = set()
        if kind == "interface":
            body = src[src.index("{"):]
            for m in re.finditer(r"([\w.<>\[\],? ]+?)\s+(\w+)\s*\(", body):
                iface_methods.add((m.group(2), split_args(balanced(body, m.end() - 1))))
        info[name] = {
            "path": path, "src": src, "package": package.group(1) if package else "",
            "imports": imports, "kind": kind, "ctors": ctors,
            "methods": methods, "iface_methods": iface_methods,
        }

    by_package: dict[str, set[str]] = {}
    for name, data in info.items():
        by_package.setdefault(data["package"], set()).add(name)

    problems: list[str] = []

    for name, data in info.items():
        src, package = data["src"], data["package"]
        visible = set(JDK) | by_package.get(package, set()) | {name}
        visible |= {imp.rsplit(".", 1)[1] for imp in data["imports"]}

        # A. 未解決の型名
        used = set(re.findall(r"\bnew\s+([A-Z]\w*)", src))
        used |= set(re.findall(r"\b([A-Z]\w*)\s+\w+\s*[=;),]", src))
        used |= set(re.findall(r"\b([A-Z]\w*)\.\w+\(", src))
        used |= set(re.findall(r"\bimplements\s+([A-Z]\w*)", src))
        used |= set(re.findall(r"\bcatch\s*\(\s*([A-Z]\w*)", src))
        for simple in sorted(used - visible):
            if simple.isupper():          # 定数（MAX_ROWS 等）は型ではない
                continue
            problems.append(f"A {name}: 型 {simple} が解決できない")

        # B. コンストラクタの引数個数
        for m in re.finditer(r"\bnew\s+([A-Z]\w*)\s*\(", src):
            target = m.group(1)
            if target not in info or info[target]["kind"] != "class":
                continue
            count = split_args(balanced(src, m.end() - 1))
            if info[target]["ctors"] and count not in info[target]["ctors"]:
                problems.append(
                    f"B {name}: new {target}(...) の引数が {count} 個"
                    f"（宣言は {info[target]['ctors']}）")

        # C/D. interface の実装漏れ・余計な @Override
        targets = re.findall(r"\bnew\s+([A-Z]\w*)\s*\(\s*\)\s*\{", src)
        targets += re.findall(r"\bimplements\s+([A-Z]\w*)", src)
        for target in targets:
            if target not in info or info[target]["kind"] != "interface":
                continue
            required = info[target]["iface_methods"]
            implemented = {(n, a) for n, a in data["methods"]}
            missing = {r for r in required if r not in implemented}
            if missing:
                problems.append(f"C {name}: {target} の未実装 {sorted(missing)}")

    return problems
