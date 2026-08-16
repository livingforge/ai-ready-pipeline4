"""Java ソースを表から組むための下請け。

`code_impl.py` が値クラス・インターフェース・注釈を組むのに使っていたものを、
`code_master` / `code_inventory` / `code_order2` / `code_billing` からも
使えるように切り出した。**生成する文字列は切り出す前と 1 バイトも変えていない**
（`build.py` を回して差分が出ないことを確かめてある）。

依存は一方向に保つ。ここは `code_*` のどれも import しない。
"""

from __future__ import annotations

PACKAGE_ROOT = "jp.co.contoso.sps"


def package_of(rel: str) -> str:
    """``order/service/Foo.java`` → ``jp.co.contoso.sps.order.service``。"""
    parts = rel.split("/")[:-1]
    return ".".join([PACKAGE_ROOT, *parts]) if parts else PACKAGE_ROOT


def head(rel: str, imports: list[str]) -> str:
    lines = [f"package {package_of(rel)};", ""]
    if imports:
        lines += [f"import {i};" for i in sorted(imports)] + [""]
    return "\n".join(lines)


def fields(spec_text: str) -> list[tuple[str, str, bool, bool]]:
    """``*BigDecimal qty; !String note`` を ``(型, 名, 可変, ctor に載せる)`` へ。

    先頭の ``*`` は「可変かつコンストラクタ引数にする」、``!`` は「可変だが
    コンストラクタ引数にしない」（あとから設定する項目）。
    """
    out = []
    for item in (s.strip() for s in spec_text.split(";")):
        if not item:
            continue
        mutable = item[0] in "*!"
        in_ctor = not item.startswith("!")
        if mutable:
            item = item[1:]
        type_name, name = item.rsplit(" ", 1)
        out.append((type_name.strip(), name.strip(), mutable, in_ctor))
    return out


def getter(type_name: str, name: str) -> str:
    prefix = "is" if type_name == "boolean" else "get"
    return prefix + name[0].upper() + name[1:]


def bean(rel: str, javadoc: str, spec_text: str, imports: list[str] | None = None,
         extra: str = "", extends: str = "") -> str:
    """値クラス（フィールド + コンストラクタ + getter/setter）を組む。"""
    name = rel.split("/")[-1].removesuffix(".java")
    declared = fields(spec_text)
    ctor_fields = [f for f in declared if f[3]]

    body = []
    for type_name, fname, mutable, _in_ctor in declared:
        final = "" if mutable else " final"
        body.append(f"    private{final} {type_name} {fname};")
    body.append("")
    args = ", ".join(f"{t} {n}" for t, n, _m, _c in ctor_fields)
    body.append(f"    public {name}({args}) {{")
    for _t, fname, _m, _c in ctor_fields:
        body.append(f"        this.{fname} = {fname};")
    body.append("    }")
    for type_name, fname, mutable, _c in declared:
        body.append("")
        body.append(f"    public {type_name} {getter(type_name, fname)}() {{")
        body.append(f"        return {fname};")
        body.append("    }")
        if mutable:
            body.append("")
            setter = "set" + fname[0].upper() + fname[1:]
            body.append(f"    public void {setter}({type_name} {fname}) {{")
            body.append(f"        this.{fname} = {fname};")
            body.append("    }")
    if extra:
        body.append("")
        body.append(extra.strip("\n"))

    inherit = f" extends {extends}" if extends else ""
    return (head(rel, imports or [])
            + f"/**\n * {javadoc}\n */\npublic class {name}{inherit} {{\n\n"
            + "\n".join(body) + "\n}\n")


def iface(rel: str, javadoc: str, methods: list[str],
          imports: list[str] | None = None) -> str:
    name = rel.split("/")[-1].removesuffix(".java")
    body = "\n\n".join(f"    {m};" for m in methods)
    return (head(rel, imports or [])
            + f"/**\n * {javadoc}\n */\npublic interface {name} {{\n\n" + body + "\n}\n")


def annotation(rel: str, javadoc: str, target: str) -> str:
    name = rel.split("/")[-1].removesuffix(".java")
    return (head(rel, ["java.lang.annotation.ElementType",
                       "java.lang.annotation.Retention",
                       "java.lang.annotation.RetentionPolicy",
                       "java.lang.annotation.Target"])
            + f"/**\n * {javadoc}\n */\n"
            + f"@Target(ElementType.{target})\n"
            + "@Retention(RetentionPolicy.RUNTIME)\n"
            + f"public @interface {name} {{\n}}\n")


def code_enum(rel: str, javadoc: str, constants: list[tuple[str, str, str]],
              code_type: str = "String", imports: list[str] | None = None,
              extra: str = "") -> str:
    """コード値を持つ列挙を組む。``constants`` は ``(列挙子, コード値, 説明)``。

    コード定義書のコード値（受注経路・在庫移動区分など）をそのまま持たせる。
    """
    name = rel.split("/")[-1].removesuffix(".java")
    literal = (lambda v: f'"{v}"') if code_type == "String" else (lambda v: v)

    body = []
    for i, (const, value, note) in enumerate(constants):
        tail = ";" if i == len(constants) - 1 else ","
        body.append(f"    /** {note} */")
        body.append(f"    {const}({literal(value)}, \"{note}\"){tail}")
        body.append("")
    body += [
        f"    private final {code_type} code;",
        "    private final String label;",
        "",
        f"    {name}({code_type} code, String label) {{",
        "        this.code = code;",
        "        this.label = label;",
        "    }",
        "",
        f"    public {code_type} getCode() {{",
        "        return code;",
        "    }",
        "",
        "    public String getLabel() {",
        "        return label;",
        "    }",
        "",
        "    /** コード値から列挙子を引く。該当が無ければ例外を送出する。 */",
        f"    public static {name} of({code_type} code) {{",
        f"        for ({name} value : values()) {{",
        ("            if (value.code.equals(code)) {" if code_type == "String"
         else "            if (value.code == code) {"),
        "                return value;",
        "            }",
        "        }",
        f'        throw new IllegalArgumentException("未知のコード値: " + code);',
        "    }",
    ]
    if extra:
        body.append("")
        body.append(extra.strip("\n"))
    return (head(rel, imports or [])
            + f"/**\n * {javadoc}\n */\npublic enum {name} {{\n\n"
            + "\n".join(body) + "\n}\n")
