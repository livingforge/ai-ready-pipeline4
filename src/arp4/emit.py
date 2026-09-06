"""``arp4 emit`` ―― 正本から**決まっているものだけ**をコードにする。

``publish`` が正本から設計書を出すのと同じ位置にある ―― 出力は再生成物
（``.arp/emit/``）で、**直接編集しない。**手を入れたくなったら正本を直して
出し直す。

出すもの::

    ddl/<表>.sql      エンティティ・列・主キー・外部キー・索引（SQL）
    <source_path>     モジュールの骨格（クラス・メソッド・型付きの引数）
    codes.<拡張子>    コード定義の値（魔法の文字列を無くす）
    messages.<拡張子> メッセージの表示文

**本体は書かない。** 手続きの中身は疑似コードにしかなく、そこは人か LLM の
仕事である ―― 骨格は「呼び出しの境界が正本で決まっている」ことの写しであって、
実装ではない。埋める場所は言語ごとの「未実装」で示す。

**決まっていないものは、印を付けて数える。** 型が決まらない引数を黙って
``Object`` にすると、**決まっていないことが生成物から見えなくなる** ―― 印を
付けて件数を申告する（`parse` が「読めなかった」を黙らせないのと同じ）。

**型の対応表は正本が先で、無ければ arp4 が選んだものになる。** 案件が
``implementation-standard``（``standard_kind: 型写像``）で綴りを決めていれば
それを使い、決めていなければパックが配る表（``languages/*.yml``）を使う ――
`数値` を `BigDecimal` にするか `double` にするかは方言の話で、**arp4 が選んだ
ことを黙っていると「資料にそう書いてあった」と読まれる。**だから生成物の頭に
**どちらを使ったか**を必ず書く。

**同一入力に対して出力はバイト一致する。**
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from arp4.spec import Spec

#: 生成物の頭に置く印。**直接編集しない**を毎回言う。
BANNER = "生成物 ―― arp4 emit（直接編集しない。正本を直して出し直す）"

#: 型が決まらなかったところに置く印。**言語ごとの「なんでも型」に逃がさない。**
UNKNOWN = "arp4: 型が正本で決まっていません"

#: 本体を書かない印。言語ごとの「まだ実装していない」の綴り。
_UNIMPLEMENTED = {
    "Python": 'raise NotImplementedError("arp4 emit: 本体は未実装")',
    "Java": 'throw new UnsupportedOperationException("arp4 emit: 本体は未実装");',
    "TypeScript": 'throw new Error("arp4 emit: 本体は未実装");',
    "JavaScript": 'throw new Error("arp4 emit: 本体は未実装");',
    "C": "/* arp4 emit: 本体は未実装 */",
}

#: 型が決まらなかった引数に置く型（印つきで出す）。
_ANY = {"Python": "", "Java": "Object", "TypeScript": "unknown",
        "JavaScript": "", "C": "void *"}

#: 定数名に使えない字。**機械的に潰す**（`E-0007` → `E_0007`）―― 綴りを
#: 案件ごとに選ばせない。
_UNSAFE = re.compile(r"[^A-Za-z0-9_]+")


@dataclass
class Emitted:
    """生成 1 ファイルぶん。**まだ書いていない。**"""

    relative: str                        # 出力先（emit ディレクトリからの相対）
    text: str
    kind: str                            # ddl / module / code / message


@dataclass
class Result:
    emitted: list[Emitted] = field(default_factory=list)
    #: 決まらなかったことの申告。**件数ではなく、どこが決まっていないかを言う。**
    notes: list[str] = field(default_factory=list)

    def of_kind(self, kind: str) -> list[Emitted]:
        return [e for e in self.emitted if e.kind == kind]


# ── 入口 ────────────────────────────────────────────────────────
def plan(spec: Spec, profiles: dict[str, dict[str, Any]]) -> Result:
    """正本から生成計画を作る。**書き込みはしない。**"""
    maker = _Maker(spec, profiles)
    maker.run()
    return Result(emitted=maker.emitted, notes=maker.notes)


def write(out: Path, result: Result) -> list[Path]:
    """計画どおりに書く。**出力先は再生成物**なので、そのまま重ねる。"""
    written: list[Path] = []
    for emitted in result.emitted:
        path = out / emitted.relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(emitted.text, encoding="utf-8", newline="\n")
        written.append(path)
    return written


def type_map(spec: Spec, language: str) -> dict[str, dict[str, str]]:
    """正本が決めた型の綴り（``standard_kind: 型写像``）。**言語ごと。**

    形はパックの ``languages/*.yml`` の ``types`` と揃える ―― 生成の側が
    2 通りの表を読み分けずに済むようにするためで、**どちらから来たかは
    生成物の頭に書く**（読み分けないことと、出どころを黙ることは別である）。
    """
    found: dict[str, dict[str, str]] = {}
    for item in sorted(spec.of_type("implementation-standard"),
                       key=lambda i: str(i.get("standard_id") or i.get("id"))):
        if str(item.get("standard_kind") or "") != "型写像":
            continue
        if str(item.get("language") or "") != language:
            continue
        logical = str(item.get("logical_type") or "")
        physical = str(item.get("physical_type") or "")
        if not logical or not physical:
            continue                     # 決めていない行は使わない
        entry = {"form": physical}
        if item.get("default_type"):
            entry["default"] = str(item.get("default_type"))
        found[logical] = entry
    return found


class _Maker:
    def __init__(self, spec: Spec, profiles: dict[str, dict[str, Any]]) -> None:
        self.spec = spec
        self.profiles = profiles
        self.by_id = spec.by_id
        self.emitted: list[Emitted] = []
        self.notes: list[str] = []
        self.unknown = 0                 # 型が決まらなかった引数
        self.of_ddl = False              # DDL を 1 本でも出したか
        #: 言語 → (使う対応表, 出どころの文言)。**正本が先、無ければ同梱の表。**
        self.tables: dict[str, tuple[dict[str, Any], str]] = {}
        for language, profile in profiles.items():
            declared = type_map(spec, language)
            if declared:
                merged = {**(profile.get("types") or {}), **declared}
                where = (f"正本の実装規約（型写像 {len(declared)} 行）"
                         f"＋ jp-sier-std/languages/{language.lower()}.yml")
            else:
                merged = dict(profile.get("types") or {})
                where = f"jp-sier-std/languages/{language.lower()}.yml"
            self.tables[language] = ({**profile, "types": merged}, where)

    def _profile(self, language: str) -> dict[str, Any]:
        """**正本の写像を重ねたプロファイル。** 生の `profiles` は使わない。"""
        return self.tables[language][0]

    # ── 道具 ────────────────────────────────────────────────────
    def _out(self, relative: str, kind: str, lines: list[str]) -> None:
        self.emitted.append(Emitted(relative=relative, kind=kind,
                                    text="\n".join(lines) + "\n"))

    def _banner(self, profile: dict[str, Any], source: str) -> list[str]:
        mark = str(profile.get("comment") or "#")
        language = str(profile.get("language") or "")
        where = self.tables.get(language, ({}, ""))[1]
        return [f"{mark} {BANNER}",
                f"{mark} 出典: {source}",
                f"{mark} 型の対応: {where}", ""]

    def _ordered(self, relation: str, from_id: str) -> list[dict[str, Any]]:
        """``ordered: true`` の関係を並び順で。**並びが仕様である。**"""
        found = [r for r in self.spec.relations_of(relation)
                 if str(r.get("from")) == from_id]
        return sorted(found, key=lambda r: (int(r.get("order") or 0),
                                            str(r.get("to"))))

    def _languages(self) -> list[str]:
        """正本のモジュールが名乗っている実装言語。**推測しない。**"""
        found = {str(m.get("language") or "") for m in self.spec.of_type("module")}
        return sorted(x for x in found if x and x in self.profiles)

    # ── 本体 ────────────────────────────────────────────────────
    def run(self) -> None:
        self._ddl()
        self._modules()
        languages = self._languages()
        for language in languages:
            self._codes(language)
            self._messages(language)
        if not languages:
            self.notes.append(
                "実装言語を名乗っているモジュールが 0 件です。コード定義・"
                "メッセージ・骨格は出しません ―― module.language は "
                "arp4 design が出典のパスの拡張子から付けます")
        # **同梱の表を使ったことは、使ったときに言う。** 生成物の頭には書いて
        # あるが、端末で 1 度も言わないと「案件が決めた綴りで出た」と読まれる。
        # **型を書けない言語は数えない**（`typed: false`）―― JavaScript の写像を
        # 「決めてください」と言うのは、決めようのないことを催促する形になる。
        borrowed = sorted(
            language for language in
            (["SQL"] if self.of_ddl else []) + languages
            if language in self.tables
            and self._profile(language).get("typed") is not False
            and "正本" not in self.tables[language][1])
        if borrowed:
            self.notes.append(
                f"型の綴りを正本が決めていない言語があります（{'・'.join(borrowed)}）。"
                "arp4 同梱の表を使いました ―― 案件で決めるなら実装規約を "
                "standard_kind: 型写像 で書いてください（論理型・型の綴り）")
        if self.unknown:
            self.notes.append(
                f"型が正本で決まっていない引数が {self.unknown} 件あります。"
                f"生成物では「{UNKNOWN}」の印を付けてあります ―― typed-as で"
                "正本の語彙を指すか、parameter.impl_type を埋めてください")

    # ── DDL ─────────────────────────────────────────────────────
    def _ddl(self) -> None:
        if "SQL" not in self.tables:
            self.notes.append("SQL の型の対応表がありません（DDL は出しません）")
            return
        profile = self._profile("SQL")
        for entity in sorted(self.spec.of_type("entity"),
                             key=lambda i: str(i.get("physical_name") or i.get("id"))):
            table = str(entity.get("physical_name") or "")
            if not table:
                self.notes.append(
                    f"物理名の無いエンティティは DDL にしません: {entity.get('name')}")
                continue
            self._out(f"ddl/{table}.sql", "ddl", self._table(entity, profile))
            self.of_ddl = True

    def _table(self, entity: dict[str, Any],
               profile: dict[str, Any]) -> list[str]:
        table = str(entity.get("physical_name"))
        lines = self._banner(profile, _display(entity))
        if entity.get("statement"):
            lines.append(f"-- {entity.get('statement')}")

        columns: list[str] = []
        keys: list[str] = []
        dropped: list[str] = []
        for relation in self._ordered("has-column", str(entity.get("id"))):
            item = self.by_id.get(str(relation.get("to")))
            name = str(relation.get("physical_name") or "")
            if item is None or not name:
                continue
            sql = _sql_type(item, profile)
            if not sql:
                dropped.append(str(item.get("name")))   # 操作（値を持たない部品）
                continue
            parts = [f"    {name:<20} {sql}"]
            if relation.get("not_null") or relation.get("pk"):
                parts.append("NOT NULL")
            if relation.get("default_value"):
                parts.append(f"DEFAULT {relation.get('default_value')}")
            columns.append(" ".join(parts) + f"    -- {item.get('name')}")
            if relation.get("pk"):
                keys.append(name)

        for relation in sorted(self.spec.relations_of("references"),
                               key=lambda r: str(r.get("to"))):
            if str(relation.get("from")) != str(entity.get("id")):
                continue
            target = self.by_id.get(str(relation.get("to")))
            fk = str(relation.get("fk_columns") or "")
            if target is None or not fk or not target.get("physical_name"):
                continue
            columns.append(
                f"    CONSTRAINT fk_{table}_{target.get('physical_name')} "
                f"FOREIGN KEY ({fk}) REFERENCES {target.get('physical_name')}")

        if keys:
            columns.insert(
                len([c for c in columns if not c.startswith("    CONSTRAINT")]),
                f"    CONSTRAINT pk_{table} PRIMARY KEY ({', '.join(keys)})")
        if not columns:
            lines.append(f"-- 列が 1 本もありません（{_display(entity)}）")
            return lines

        lines.append(f"CREATE TABLE {table} (")
        lines.append(",\n".join(columns))
        lines.append(");")

        for relation in self._ordered("has-index", str(entity.get("id"))):
            index = self.by_id.get(str(relation.get("to")))
            if index is None or not index.get("name") or not index.get("columns"):
                continue
            unique = "UNIQUE " if index.get("uniqueness") == "一意" else ""
            lines.append("")
            lines.append(f"CREATE {unique}INDEX {index.get('name')} "
                         f"ON {table} ({index.get('columns')});")
        if dropped:
            lines.append("")
            lines.append(f"-- 列にしなかった項目 {len(dropped)} 件"
                         f"（データ型が 操作 ＝ 値を持たない画面部品）: "
                         f"{'・'.join(dropped)}")
        return lines

    # ── モジュールの骨格 ────────────────────────────────────────
    def _modules(self) -> None:
        """**1 ファイルに複数のモジュールが載る**ので、出す先でまとめる。"""
        files: dict[tuple[str, str], list[dict[str, Any]]] = {}
        for module in self.spec.of_type("module"):
            path = str(module.get("source_path") or "")
            language = str(module.get("language") or "")
            if not path or language not in self.profiles:
                continue
            files.setdefault((language, path), []).append(module)

        for (language, path), modules in sorted(files.items()):
            profile = self._profile(language)
            # **クラスかファイル直下かは `class_name` が決める。** 宣言されて
            # いるものの転記で、名前の形からの推測ではない。
            modules.sort(key=lambda m: (bool(m.get("class_name")),
                                        str(m.get("module_id") or m.get("id"))))
            self._out(path, "module", self._file(language, profile, modules))

    def _file(self, language: str, profile: dict[str, Any],
              modules: list[dict[str, Any]]) -> list[str]:
        lines = self._banner(profile, "・".join(_display(m) for m in modules))
        package = next((str(m.get("package")) for m in modules
                        if m.get("package")), "")
        if language == "Java" and package:
            lines += [f"package {package};", ""]

        for module in modules:
            if module.get("class_name") and language != "C":
                lines += self._class(language, profile, module)
            else:
                lines += self._functions(language, profile, module)
            lines.append("")
        return lines

    def _class(self, language: str, profile: dict[str, Any],
               module: dict[str, Any]) -> list[str]:
        name = str(module.get("class_name") or "").rsplit(".", 1)[-1]
        head = {"Python": f"class {name}:",
                "Java": f"public class {name} {{",
                "TypeScript": f"export class {name} {{",
                "JavaScript": f"export class {name} {{"}[language]
        lines = [head] + _doc(language, module.get("statement"), indent=1)
        body: list[str] = []
        for relation in self._ordered("has-method", str(module.get("id"))):
            method = self.by_id.get(str(relation.get("to")))
            if method is not None:
                body += self._method(language, profile, method, indent=1)
        if not body:
            body = [_indent(language, 1) + _pass(language)]
        lines += body
        if language != "Python":
            lines.append("}")
        return lines

    def _functions(self, language: str, profile: dict[str, Any],
                   module: dict[str, Any]) -> list[str]:
        lines = _doc(language, module.get("statement"), indent=0)
        for relation in self._ordered("has-method", str(module.get("id"))):
            method = self.by_id.get(str(relation.get("to")))
            if method is not None:
                lines += self._method(language, profile, method, indent=0)
        return lines

    def _method(self, language: str, profile: dict[str, Any],
                method: dict[str, Any], indent: int) -> list[str]:
        name = str(method.get("method_name") or "")
        if not name:
            self.notes.append(
                f"呼ぶ名前が決まっていないので骨格に出しません: {_display(method)}"
                "（arp4 design がシグネチャから付けます）")
            return []
        pad = _indent(language, indent)
        params = [self._parameter(language, profile, r)
                  for r in self._ordered("has-parameter", str(method.get("id")))]
        params = [p for p in params if p]
        returns = self._returns(language, profile, method)
        visibility = str(method.get("visibility") or "")

        if language == "Python":
            joined = ", ".join(["self"] + params) if indent else ", ".join(params)
            arrow = f" -> {returns}" if returns else ""
            head = f"{pad}def {name}({joined}){arrow}:"
        elif language == "Java":
            keyword = visibility if visibility in ("public", "protected",
                                                   "private") else ""
            head = (f"{pad}{keyword + ' ' if keyword else ''}"
                    f"{returns or 'void'} {name}({', '.join(params)}) {{")
        elif language == "C":
            head = f"{returns or 'void'} {name}({', '.join(params) or 'void'})\n{{"
        else:
            keyword = "private " if visibility == "private" else ""
            colon = f": {returns}" if returns else ""
            head = f"{pad}{keyword}{name}({', '.join(params)}){colon} {{"

        lines = [""] + [head]
        lines += _doc(language, method.get("statement"), indent + 1,
                      extra=_pseudo(self.spec, method))
        lines.append(_indent(language, indent + 1) + _UNIMPLEMENTED[language])
        if language in ("Java", "TypeScript", "JavaScript"):
            lines.append(pad + "}")
        elif language == "C":
            lines.append("}")
        return lines

    def _parameter(self, language: str, profile: dict[str, Any],
                   relation: dict[str, Any]) -> str:
        item = self.by_id.get(str(relation.get("to")))
        if item is None:
            return ""
        name = str(item.get("param_name") or "")
        if not name:
            return ""
        type_text, known = self._type_of(item, profile)
        mark = "" if known else f" /* {UNKNOWN} */"
        if not known:
            self.unknown += 1
        default = str(item.get("default_value") or "")

        if language == "Python":
            text = f"{name}: {type_text}" if type_text else name
            return text + (f" = {default}" if default else "") + mark
        if language in ("TypeScript", "JavaScript"):
            optional = "?" if item.get("optional") and not default else ""
            text = (f"{name}{optional}: {type_text}" if type_text
                    else f"{name}{optional}")
            return text + (f" = {default}" if default else "") + mark
        # Java / C ―― 型が先に来る。既定値は書けないので doc に出す。
        return (f"{type_text} {name}" if type_text else name) + mark

    def _returns(self, language: str, profile: dict[str, Any],
                 method: dict[str, Any]) -> str:
        """**資料の綴りが先。** 同じ言語で書かれた宣言の写しだからである。"""
        declared = str(method.get("returns") or "").strip()
        if declared:
            return declared
        for relation in self.spec.relations_of("returns-type"):
            if str(relation.get("from")) != str(method.get("id")):
                continue
            item = self.by_id.get(str(relation.get("to")))
            if item is not None:
                return self._type_of(item, profile)[0]
        return ""

    def _type_of(self, item: dict[str, Any],
                 profile: dict[str, Any]) -> tuple[str, bool]:
        """引数の実装型。``(綴り, 決まっているか)``。

        順は **``impl_type`` → ``typed-as`` の相手** である ―― ``impl_type`` は
        同じ言語で書かれた宣言の写しなので、対応表より確かである。

        **型を書けない言語では「決まっていません」と言わない**（``typed: false``）
        ―― 言えることと言えないことを混ぜると、印が意味を持たなくなる。
        """
        if profile.get("typed") is False:
            return "", True
        declared = str(item.get("impl_type") or "").strip()
        if declared:
            return declared, True
        for relation in self.spec.relations_of("typed-as"):
            if str(relation.get("from")) != str(item.get("id")):
                continue
            target = self.by_id.get(str(relation.get("to")))
            if target is None:
                continue
            if str(target.get("type")) == "data-item":
                found = _form(target, profile)
                if found:
                    return found, True
            physical = str(target.get("physical_name") or "")
            if physical:
                return physical, True
        return _ANY.get(str(profile.get("language")), ""), False

    # ── コード定義とメッセージ ──────────────────────────────────
    def _codes(self, language: str) -> None:
        """**魔法の文字列を無くす。** 値と名称を機械的な綴りで定数にする。

        定数名は ``V_<値>``（使えない字は ``_`` に潰す）である ―― 名称から
        英字を作ると綴りを案件ごとに選ぶことになり、そこが揺れる。
        """
        profile = self._profile(language)
        masters = sorted(self.spec.of_type("code-master"),
                         key=lambda i: str(i.get("code_id") or i.get("id")))
        if not masters:
            return
        lines = self._banner(profile, "コード定義")
        for master in masters:
            title = str(master.get("physical_name") or master.get("code_id") or "")
            lines.append(_line(language, f"{master.get('name')}（{title}）"))
            for relation in self._ordered("has-value", str(master.get("id"))):
                value = self.by_id.get(str(relation.get("to")))
                if value is None:
                    continue
                key = f"{title}_{_safe(str(value.get('value')))}"
                lines.append(_constant(language, key, str(value.get("value")),
                                       str(value.get("name") or "")))
            lines.append("")
        self._out(f"codes{profile.get('extension')}", "code", lines)

    def _messages(self, language: str) -> None:
        profile = self._profile(language)
        messages = sorted(self.spec.of_type("message"),
                          key=lambda i: str(i.get("message_id") or i.get("id")))
        if not messages:
            return
        lines = self._banner(profile, "メッセージ")
        for message in messages:
            key = _safe(str(message.get("message_id") or ""))
            if not key:
                continue
            lines.append(_constant(language, key, str(message.get("body") or ""),
                                   str(message.get("name") or "")))
        self._out(f"messages{profile.get('extension')}", "message", lines)


# ── 綴りの小道具 ────────────────────────────────────────────────
def _display(item: dict[str, Any]) -> str:
    for key in ("module_id", "method_id", "code_id", "id"):
        if item.get(key):
            return f"{item.get('name')}（{item.get(key)}）"
    return str(item.get("name") or "")


def _indent(language: str, level: int) -> str:
    width = 4 if language in ("Python", "Java", "C") else 2
    return " " * (width * level)


def _pass(language: str) -> str:
    return "pass" if language == "Python" else ""


def _line(language: str, text: str) -> str:
    mark = "#" if language == "Python" else "//"
    return f"{mark} {text}"


def _doc(language: str, statement: Any, indent: int,
         extra: str = "") -> list[str]:
    """仕様文を doc コメントに。**本文はここにしか無い**ので落とさない。"""
    text = str(statement or "").strip()
    if not text and not extra:
        return []
    pad = _indent(language, indent)
    body = [text] if text else []
    body += [line for line in extra.splitlines() if line.strip()]
    if language == "Python":
        if len(body) == 1:
            return [f'{pad}"""{body[0]}"""']
        return [f'{pad}"""{body[0]}', *[f"{pad}{line}" for line in body[1:]],
                f'{pad}"""']
    if len(body) == 1:
        return [f"{pad}/** {body[0]} */"]
    return [f"{pad}/**", *[f"{pad} * {line}" for line in body], f"{pad} */"]


def _pseudo(spec: Spec, method: dict[str, Any]) -> str:
    """メソッドが持つ処理ステップの疑似コード。**手順の並びで出す。**"""
    steps = [r for r in spec.relations_of("has-process-step")
             if str(r.get("from")) == str(method.get("id"))]
    out: list[str] = []
    for relation in sorted(steps, key=lambda r: (int(r.get("order") or 0),
                                                 str(r.get("to")))):
        step = spec.by_id.get(str(relation.get("to")))
        if step is None:
            continue
        text = str(step.get("pseudo") or step.get("statement") or "")
        if text:
            out.append(f"{step.get('step_id') or ''} {text}".strip())
    return "\n".join(out)


def _safe(text: str) -> str:
    """定数名に使える形へ**機械的に**潰す（``E-0007`` → ``E_0007``）。"""
    cleaned = _UNSAFE.sub("_", str(text or "")).strip("_")
    return f"V_{cleaned}" if cleaned and cleaned[0].isdigit() else cleaned


def _constant(language: str, key: str, value: str, label: str) -> str:
    note = f"  // {label}" if label else ""
    if language == "Python":
        return f'{key} = "{value}"' + (f"  # {label}" if label else "")
    if language == "Java":
        return f'public static final String {key} = "{value}";{note}'
    if language == "C":
        return f'#define {key} "{value}"{note}'
    return f'export const {key} = "{value}";{note}'


def _form(item: dict[str, Any], profile: dict[str, Any]) -> str:
    """データ項目 → 対応表の綴り。**桁が無いときは桁つきの形を使わない。**"""
    kind = str(item.get("data_type") or "")
    decimals = item.get("decimals")
    if kind == "数値" and not decimals:
        kind = "整数"                      # 小数の無い数は現場の綴りに合わせる
    entry = (profile.get("types") or {}).get(kind)
    if not isinstance(entry, dict):
        return ""
    form = str(entry.get("form") or "")
    if "{" not in form:
        return form
    length = item.get("length")
    if not length:
        return str(entry.get("default") or "")
    return form.format(length=length, decimals=decimals or 0)


def _sql_type(item: dict[str, Any], profile: dict[str, Any]) -> str:
    return _form(item, profile)
