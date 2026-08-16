"""``arp4 draft`` ―― コードのパース結果から整理結果の骨格を**決定的に**生成する。

**「意味を判断しない層は機械」という哲学のまま、境界線を引き直したもの**である
（Phase 2）。r001 の実測で、コード整理の較正エージェントが作った規約 ――
concept 命名のパス導出・公開名だけ起こす・``@dataclass`` → Common・例外欄 →
``raises``・参照レコードでアンカー回収・calls はモジュールへ畳む ―― は事実上
アルゴリズムだった。割れた箇所は規則が未成文だった箇所と正確に一致する。
規則になるものは機械が実行し、**LLM の仕事は statement / description の
日本語文章化だけ**に絞る。

生成するもの（すべて organize.md の規約の実装である）::

    ファイル       完全な module レコード（出典: モジュール関数の塊、無ければ i1）
    クラス         module レコード（class_name / @dataclass → tier: Common /
                   refines → ファイル / has-method → 公開メソッド）
    公開メソッド   method レコード（signature / returns / raises / decorators）
    テスト関数     test-case レコード（verifies は取り込みから解決し module へ畳む）
    コマンド       command レコード（argparse の add_parser。名前が定数のものだけ）
    残りのアンカー ファイル concept の参照レコードで回収（v1 / p1 / 余り）

**判断できないものは決めずに申告する**（パースの規律と同じ）。文章は
``<TODO 抽出元 <位置>>`` の形で空け、取り込みの解決が曖昧なもの（同じ名前に
複数の候補）は関係を張らずに決定ログへ残す。

**同一入力に対して出力はバイト一致する。** 時刻・乱数・辞書順の揺れを含まない
（テストが 2 回生成して比較する）。Excel のシートには手を出さない ―― シートの
整理は従来どおり LLM の仕事である（資料の読みは規則にならない）。

**書いたものは上書きしない。** 相方の整理結果が既にあるファイルは飛ばす ――
draft のあとに LLM / 人が文章を埋めるので、再実行で埋めた文章を潰してはならない。
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from arp4 import decisions, mdio, organized as organized_module, yamlio
from arp4.paths import Round

#: 文章化スロット。freeze は残っていると G026（error）で数える。
TODO = "<TODO 抽出元 {at}>"

#: draft が書いたファイルの印（YAML の根に置く）。抽出的文章の lint（G027）は
#: この印のあるファイルにだけ掛かる ―― 人が最初から書いた整理結果に、draft の
#: 文章契約を後から強いない。
MARK = "drafted"

#: コードのパース結果と判断する拡張子。
_CODE_SUFFIXES = (".py", ".java")

#: Java の注釈 → ``tier``。**注釈に書いてあるものの転記**であり、メンバの名前
#: からの推測ではない（organize.md「tier は注釈と継承で決める」）。
_JAVA_TIER = {"@Service": "Service", "@Controller": "Controller",
              "@RestController": "Controller", "@Repository": "Repository",
              "@Component": "Common"}

#: 引用符付きの文字列（argparse の表で「名前が定数」であった印）。
_QUOTED = re.compile(r"^'(?P<name>[^']*)'$|^\"(?P<dq>[^\"]*)\"$")


@dataclass
class Drafted:
    """生成 1 ファイルぶん。**まだ書いていない。**"""

    path: Path                           # 書き先（organized/**.yml）
    file: str                            # parsed からの相対（拡張子なし）
    data: dict[str, Any]
    todo: int = 0                        # 文章化スロットの数

    @property
    def records(self) -> int:
        return len(self.data.get("records") or [])


@dataclass
class Result:
    """draft 1 ラウンドぶんの結果。"""

    drafted: list[Drafted] = field(default_factory=list)
    skipped: list[str] = field(default_factory=list)     # 整理結果が既にある
    sheets: int = 0                                      # コードでないので対象外
    decisions: list[dict[str, Any]] = field(default_factory=list)

    @property
    def todo(self) -> int:
        return sum(d.todo for d in self.drafted)


# ── 入口 ────────────────────────────────────────────────────────
def plan(round_: Round) -> Result:
    """ラウンドのコード由来のパース結果から生成計画を作る。**書き込みはしない。**"""
    result = Result()
    files = _parsed_files(round_)
    index = _module_index(files)

    for file, document in sorted(files.items()):
        if not _is_code(document):
            result.sheets += 1
            continue
        target = round_.organized / f"{file}{yamlio.EXT}"
        if target.is_file():
            result.skipped.append(file)
            continue
        data, todo, said = _draft_one(file, document, index)
        result.decisions += said
        result.drafted.append(Drafted(path=target, file=file, data=data,
                                      todo=todo))
    return result


def write(round_: Round, result: Result) -> list[Path]:
    """計画どおりに書き、決定ログを残す。"""
    written: list[Path] = []
    for drafted in result.drafted:
        yamlio.dump(drafted.path, drafted.data)
        written.append(drafted.path)
    # 置き換え（追記ではない）―― draft は何度でも回すので、回すたびに同じ判断が
    # 二重に積まれてはいけない。**今回生成したファイルの判断だけ**を置き換える
    # （飛ばしたファイルの判断は前回のまま残る ―― 消すと、整理結果は残って
    # いるのに判断の記録だけが消える）。
    drafted = {f"{d.file}{mdio.EXT}" for d in result.drafted}
    decisions.replace(
        round_, "draft", result.decisions,
        replaced=lambda e: not e.get("basis") or any(
            str(b).split("#")[0] in drafted for b in e.get("basis") or []))
    return written


def _parsed_files(round_: Round) -> dict[str, mdio.ParsedFile]:
    return {p.relative_to(round_.parsed).with_suffix("").as_posix(): mdio.read(p)
            for p in mdio.scan(round_.parsed)}


def _is_code(document: mdio.ParsedFile) -> bool:
    return document.source.endswith(_CODE_SUFFIXES)


# ── 名前の導出（organize.md の規約の実装） ──────────────────────
def _dotted(file: str) -> str:
    """元ファイルの相対パス → ドット結び（拡張子なし）。concept の土台。"""
    return ".".join(Path(file).with_suffix("").parts)


def _display(dotted: str) -> str:
    """読みやすい名前。**先頭の ``src`` だけ落とす**（置き場の都合であって
    名前ではない ―― ``src/arp4/mdio.py`` を import する名前は ``arp4.mdio``）。
    それ以外は落とさない（``tests.dataset`` の ``tests`` は名前の一部である）。"""
    parts = dotted.split(".")
    if parts[0] == "src" and len(parts) > 1:
        return ".".join(parts[1:])
    return dotted


def _file_concept(file: str) -> str:
    return f"c-mod-{_dotted(file)}"


# ── 1 ファイルの生成 ────────────────────────────────────────────
#: メンバの表の欄（見出し行から引く）。
_COLUMNS = {"name": "メンバ", "kind": "種類", "marks": "注釈",
            "signature": "シグネチャ", "returns": "戻り値", "raises": "例外",
            "line": "行", "decl": "宣言"}


class _Table:
    """メンバの表 1 枚。見出しで欄を引く（並びを決め打ちにしない）。"""

    def __init__(self, rows: list[list[str]]) -> None:
        self.header = rows[0] if rows else []
        self.body = rows[1:] if rows else []

    def get(self, row: list[str], column: str) -> str:
        name = _COLUMNS.get(column, column)
        if name not in self.header:
            return ""
        position = self.header.index(name)
        return row[position] if position < len(row) else ""


def _draft_one(file: str, document: mdio.ParsedFile,
               index: dict[str, list[str]]
               ) -> tuple[dict[str, Any], int, list[dict[str, Any]]]:
    maker = _Maker(file, document, index)
    maker.run()
    data: dict[str, Any] = {MARK: True, "records": maker.records}
    return data, maker.todo, maker.decisions


class _Maker:
    """1 ファイルぶんの生成。状態は**このファイルの中**にしか無い。"""

    def __init__(self, file: str, document: mdio.ParsedFile,
                 index: dict[str, list[str]]) -> None:
        self.file = file
        self.document = document
        self.index = index
        self.dotted = _dotted(file)
        self.display = _display(self.dotted)
        self.concept = _file_concept(file)
        self.testing = Path(file).stem.startswith("test_") or \
            Path(file).with_suffix("").stem.endswith("_test")
        self.records: list[dict[str, Any]] = []
        self.claimed: set[str] = set()
        self.todo = 0
        self.decisions: list[dict[str, Any]] = []

    # ── 道具 ────────────────────────────────────────────────────
    def _slot(self, at: str) -> str:
        self.todo += 1
        return TODO.format(at=at)

    def _basis(self, anchor: str) -> str:
        return f"{self.file}{mdio.EXT}#{anchor}"

    def _decide(self, what: str, why: str, confidence: str, anchor: str) -> None:
        self.decisions.append(decisions.entry(
            "draft", what, why, confidence, [self._basis(anchor)]))

    def _at(self, anchor: mdio.Anchor, line: str = "") -> str:
        if line:
            return f"{self.document.source}#L{line}"
        return anchor.at or self.document.source

    def _claim(self, anchor_id: str) -> None:
        self.claimed.add(anchor_id)

    # ── 本体 ────────────────────────────────────────────────────
    def run(self) -> None:
        chunks = {a.id: a for a in self.document.anchors}
        headings = {a.id: (a.body.splitlines() or [""])[0]
                    for a in self.document.anchors}

        functions = next((i for i, h in headings.items()
                          if h == "モジュール関数"), None)
        imports = "i1" if "i1" in chunks else None
        java = self.file.endswith(".java")

        if not java:
            self._file_record(chunks, headings, functions, imports)
        for anchor_id in sorted(chunks, key=_anchor_key):
            head = headings[anchor_id]
            if head.startswith(("クラス: ", "テストクラス: ", "インタフェース: ",
                                "列挙: ", "レコード: ", "注釈型: ")):
                self._class_record(chunks[anchor_id], head,
                                   package=self._java_package(chunks, imports)
                                   if java else "")
            elif head == "テスト":
                self._test_cases(chunks[anchor_id])
            elif head == "コマンド（argparse）":
                self._commands(chunks[anchor_id])

        if java:
            self._java_imports(chunks, imports)

        # 余ったアンカー（v1・空のテストクラス・確定名の無い p1 …）は参照レコードで
        # 回収する ―― アンカーが無いと freeze の未整理一覧に上がらず、**読めて
        # いないものほど静かに消える**（organize.md「1 件も起こさなかったときは
        # 参照だけのレコードでアンカーを回収する」の実装）。
        owner = any(r.get("concept") == self.concept and r.get("type")
                    for r in self.records)
        spoken = any(r.get("concept") == self.concept for r in self.records)
        if chunks and not owner and (spoken or set(chunks) - self.claimed):
            # ファイルの concept を名乗る完全なレコードがまだ無い（型の無い
            # Java 等）。参照レコードだけでは G013 で止まるので、ここで起こす。
            main = imports or sorted(chunks, key=_anchor_key)[0]
            self.records.append({
                "concept": self.concept, "type": "モジュール",
                "name": self.display,
                "statement": self._slot(self._at(chunks[main])),
                "source": {"anchor": main}})
            self._claim(main)
        for anchor_id in sorted(set(chunks) - self.claimed, key=_anchor_key):
            self.records.append({"concept": self.concept,
                                 "source": {"anchor": anchor_id}})

    def _java_package(self, chunks: dict[str, mdio.Anchor],
                      imports: str | None) -> str:
        """Java の ``package`` 宣言。取り込みの塊の転記である。"""
        if imports is None:
            return ""
        table = _Table(mdio.rows(chunks[imports]))
        for row in table.body:
            text = table.get(row, "取り込み")
            if text.startswith("package "):
                return text[len("package "):].strip()
        return ""

    def _java_imports(self, chunks: dict[str, mdio.Anchor],
                      imports: str | None) -> None:
        """Java の取り込み ―― calls を張る参照レコードで ``i1`` を回収する。"""
        if imports is None:
            return
        entry: dict[str, Any] = {"concept": self.concept,
                                 "source": {"anchor": imports}}
        calls = self._calls(chunks[imports])
        if calls:
            entry["refs"] = calls
        self.records.append(entry)
        self._claim(imports)

    # ── ファイルのレコード ──────────────────────────────────────
    def _file_record(self, chunks: dict[str, mdio.Anchor],
                     headings: dict[str, str], functions: str | None,
                     imports: str | None) -> None:
        main = functions or imports
        if main is None:                   # 塊が 1 つも無い（起こらないが守る）
            return
        anchor = chunks[main]
        record: dict[str, Any] = {
            "concept": self.concept,
            "type": "モジュール",
            "name": self.display,
            "statement": self._slot(self._at(anchor)),
        }
        attrs: dict[str, Any] = {}
        package = ".".join(self.display.split(".")[:-1])
        if package:
            attrs["package"] = package
        constants = self._constants(chunks, headings)
        if constants:
            attrs["constants"] = constants

        told = self._hidden_note(chunks, headings)
        if told:
            attrs["description"] = told
        if attrs:
            record["attrs"] = attrs
        record["source"] = {"anchor": main}
        self._claim(main)
        self._decide(
            f"ファイル {self.file} の module の出典を {main} にした",
            "関数の塊があればそこ、無ければ取り込み（i1）。規約 G025",
            decisions.SURE, main)

        refs: list[dict[str, Any]] = []
        if functions:
            refs += [{"rel": "has-method", "to": m}
                     for m in self._public_functions(chunks[functions])]
        record.update({"refs": refs} if refs else {})
        self.records.append(record)

        # calls は**取り込みの塊のレコード**が張る（出典と主張を揃える ――
        # B026 の自動確定も G022 も、アンカーが i1 であることを見ている）。
        # テストファイルの取り込みは verifies / uses-specimen の根拠なので、
        # calls にはしない（テストは呼び出しの構造ではなく検証の宣言である）。
        if imports and imports != main:
            calls = [] if self.testing else self._calls(chunks[imports])
            entry: dict[str, Any] = {"concept": self.concept,
                                     "source": {"anchor": imports}}
            if calls:
                entry["refs"] = calls
            self.records.append(entry)
            self._claim(imports)
        elif imports == main and not self.testing:
            calls = self._calls(chunks[imports])
            if calls:
                record.setdefault("refs", []).extend(calls)

    def _constants(self, chunks: dict[str, mdio.Anchor],
                   headings: dict[str, str]) -> str:
        found = next((i for i, h in headings.items() if h == "定数"), None)
        if found is None:
            return ""
        table = _Table(mdio.rows(chunks[found]))
        names = [table.get(row, "name") for row in table.body
                 if table.get(row, "kind") == "定数"
                 and not table.get(row, "name").startswith("_")]
        return ", ".join(names)

    def _hidden_note(self, chunks: dict[str, mdio.Anchor],
                     headings: dict[str, str]) -> str:
        """起こさなかったものの**本数の申告**。「載せない」と「無い」を混ぜない。

        数えるのは**モジュール関数の塊だけ**である ―― クラスの中の内部メソッドは
        クラスのレコードが自分で申告する（:func:`_class_hidden`）。両方で数えると
        同じ 1 本が 2 回申告され、本数が信用できなくなる。
        """
        found = next((i for i, h in headings.items() if h == "モジュール関数"),
                     None)
        if found is None:
            return ""
        table = _Table(mdio.rows(chunks[found]))
        private = dunder = helpers = 0
        for row in table.body:
            name = table.get(row, "name")
            if table.get(row, "kind") != "関数":
                continue
            if self.testing:
                helpers += 1
            elif _DUNDER.match(name):
                dunder += 1
            elif name.startswith("_"):
                private += 1
        parts = []
        if private:
            parts.append(f"内部用の関数 {private} 本")
        if dunder:
            parts.append(f"特殊メソッド {dunder} 本")
        if parts:
            return ("・".join(parts)
                    + "は公開名だけ載せる規約により起こしていない")
        if helpers:
            return (f"テストの補助（fixture・ヘルパ）{helpers} 本は "
                    "test-case にしない規約により起こしていない")
        return ""

    def _public_functions(self, anchor: mdio.Anchor) -> list[str]:
        """モジュール関数の塊の公開関数を method レコードに起こし、concept を返す。"""
        table = _Table(mdio.rows(anchor))
        out: list[str] = []
        if self.testing:
            # テストファイルの補助関数（fixture・ヘルパ）は起こさない ――
            # 本数は description が申告する。塊はファイルのレコードが出典に
            # しているので、アンカーは消えない。
            return out
        for row in table.body:
            name = table.get(row, "name")
            if table.get(row, "kind") != "関数" or name.startswith("_"):
                continue
            out.append(self._method(anchor, row, table,
                                    f"c-mtd-{self.dotted}.{name}"))
        return out

    # ── クラス ──────────────────────────────────────────────────
    def _class_record(self, anchor: mdio.Anchor, head: str,
                      package: str = "") -> None:
        table = _Table(mdio.rows(anchor))
        name = head.split(": ", 1)[1] if ": " in head else head
        if self.testing and head.startswith("テストクラス: "):
            self._test_cases(anchor, prefix=f"{name}.")
            return

        primary = self._java_primary(name)
        marks = ""
        for row in table.body:
            if table.get(row, "name") == name and table.get(row, "kind") in (
                    "クラス", "インタフェース", "列挙", "レコード", "注釈型"):
                marks = table.get(row, "marks")
                break

        record: dict[str, Any] = {
            "concept": self.concept if primary else f"c-mod-{self.dotted}.{name}",
            "type": "モジュール",
            "name": (self.display if primary
                     else f"{self.display.rsplit('.', 1)[-1]}.{name}"),
            "statement": self._slot(self._at(anchor)),
        }
        attrs: dict[str, Any] = {
            "class_name": (f"{package}.{name}" if package
                           else (self.display if primary
                                 else f"{self.display}.{name}"))}
        if package:
            attrs["package"] = package
        tier = self._tier(marks)
        if tier:
            attrs["tier"] = tier
            self._decide(f"{name} に tier={tier} を付けた",
                         f"注釈 {marks} の転記（メンバの名前からの推測ではない）",
                         decisions.SURE, anchor.id)
        told = self._class_hidden(table, name)
        if told:
            attrs["description"] = told
        record["attrs"] = attrs

        refs: list[dict[str, Any]] = []
        if not self._java_primary(name):
            refs.append({"rel": "refines", "to": self.concept})
        for row in table.body:
            member = table.get(row, "name")
            if (table.get(row, "kind") == "メソッド"
                    and not member.startswith("_")
                    and _is_public_java(table.get(row, "decl"))):
                base = record["concept"].replace("c-mod-", "c-mtd-", 1)
                refs.append({"rel": "has-method",
                             "to": self._method(anchor, row, table,
                                                f"{base}.{member}")})
        if refs:
            record["refs"] = refs
        record["source"] = {"anchor": anchor.id}
        self._claim(anchor.id)
        self.records.append(record)

    def _java_primary(self, name: str) -> bool:
        """Java の**ファイル名と同名の型**はファイルそのものである。
        ``Foo.java`` のクラス ``Foo`` に ``c-mod-….Foo.Foo`` と二重の名前を
        付けない ―― concept はラウンドをまたぐ鍵なので、余計な段を作らない。"""
        return (self.file.endswith(".java")
                and Path(self.file).with_suffix("").stem == name)

    def _tier(self, marks: str) -> str:
        if "@dataclass" in marks:
            return "Common"
        for mark, tier in _JAVA_TIER.items():
            if re.search(rf"{re.escape(mark)}\b", marks):
                return tier
        return ""

    def _class_hidden(self, table: _Table, name: str) -> str:
        private = dunder = 0
        for row in table.body:
            member = table.get(row, "name")
            if table.get(row, "kind") != "メソッド" or member == name:
                continue
            if _DUNDER.match(member):
                dunder += 1
            elif member.startswith("_") or not _is_public_java(
                    table.get(row, "decl")) and table.get(row, "decl"):
                private += 1
        parts = []
        if private:
            parts.append(f"内部用のメソッド {private} 本")
        if dunder:
            parts.append(f"特殊メソッド {dunder} 本")
        if not parts:
            return ""
        return "・".join(parts) + "は公開名だけ載せる規約により起こしていない"

    # ── メソッド ────────────────────────────────────────────────
    def _method(self, anchor: mdio.Anchor, row: list[str], table: _Table,
                concept: str) -> str:
        name = table.get(row, "name")
        record: dict[str, Any] = {
            "concept": concept,
            "type": "メソッド",
            "name": name,
            "statement": self._slot(self._at(anchor, table.get(row, "line"))),
        }
        attrs: dict[str, Any] = {}
        for key in ("signature", "returns", "raises"):
            value = table.get(row, key)
            if value:
                attrs[key] = value
        if not attrs.get("signature") and table.get(row, "decl"):
            attrs["signature"] = table.get(row, "decl")     # Java は「宣言」欄
        if table.get(row, "marks"):
            attrs["decorators"] = table.get(row, "marks")
        if attrs:
            record["attrs"] = attrs
        record["source"] = {"anchor": anchor.id}
        self._claim(anchor.id)
        self.records.append(record)
        return concept

    # ── テスト ──────────────────────────────────────────────────
    def _test_cases(self, anchor: mdio.Anchor, prefix: str = "") -> None:
        table = _Table(mdio.rows(anchor))
        refs = self._verifies(anchor)
        for row in table.body:
            name = table.get(row, "name")
            kind = table.get(row, "kind")
            if not name.startswith("test"):
                continue
            if kind not in ("テスト", "メソッド", "関数"):
                continue
            at = self._at(anchor, table.get(row, "line"))
            record: dict[str, Any] = {
                "concept": f"c-tcs-{self.dotted}.{prefix}{name}",
                "type": "テストケース",
                "name": name,
                "statement": self._slot(at),
                "attrs": {"expected": self._slot(at)},
                "source": {"anchor": anchor.id},
            }
            if refs:
                record["refs"] = [dict(r) for r in refs]
            self._claim(anchor.id)
            self.records.append(record)

    def _verifies(self, anchor: mdio.Anchor) -> list[dict[str, Any]]:
        """テストの相手。**取り込み（i1）から解決し、モジュールへ畳む。**

        同じ木（``tests/``）の中のモジュールは検体・道具なので ``uses-specimen``、
        それ以外は ``verifies``（organize.md「テストが実際に相手にしている粒度が、
        正しい粒度である」―― ファイル単位の取り込みはファイルを相手にしている）。
        """
        imports = self.document.by_id.get("i1")
        if imports is None:
            return []
        refs: list[dict[str, Any]] = []
        seen: set[str] = set()
        top = self.dotted.split(".")[0]
        for target in self._resolved(imports):
            if target == self.file or target in seen:
                continue
            seen.add(target)
            rel = ("uses-specimen"
                   if _dotted(target).split(".")[0] == top else "verifies")
            refs.append({"rel": rel, "to": _file_concept(target)})
        return refs

    # ── 取り込みの解決 ──────────────────────────────────────────
    def _calls(self, anchor: mdio.Anchor) -> list[dict[str, Any]]:
        refs: list[dict[str, Any]] = []
        seen: set[str] = set()
        for target in self._resolved(anchor):
            if target == self.file or target in seen:
                continue
            seen.add(target)
            refs.append({"rel": "calls", "to": _file_concept(target)})
        return refs

    def _resolved(self, anchor: mdio.Anchor) -> list[str]:
        """取り込みの各行を**ラウンドの中のファイル**へ解決する。

        解決は「ドット結びの末尾一致がちょうど 1 件」だけを通す ―― 2 件以上に
        当たる名前をどれかに決めるのは意味の判断なので、**張らずに決定ログへ
        残す**（黙って落とすと、依存が 1 本消えたことが誰にも見えない）。
        """
        table = _Table(mdio.rows(anchor))
        out: list[str] = []
        for row in table.body:
            source = table.get(row, "元")
            # Java の package 宣言は取り込みではない（元の欄は同じ形で出る）。
            if not source or table.get(row, "取り込み").startswith("package "):
                continue
            name = table.get(row, "名前")
            candidates = [c for c in ([f"{source}.{name}"] if name else [])
                          + [source] if c]
            resolved = self._lookup(candidates, anchor)
            if resolved:
                out.append(resolved)
        return out

    def _lookup(self, candidates: list[str], anchor: mdio.Anchor) -> str | None:
        for candidate in candidates:
            if candidate.startswith("."):
                # 相対取り込み。点 1 つが「このパッケージ」、増えるごとに 1 つ上がる。
                rest = candidate.lstrip(".")
                level = len(candidate) - len(rest)
                parts = self.dotted.split(".")[:-1]
                if level > 1:
                    parts = parts[:-(level - 1)] if level - 1 <= len(parts) else []
                candidate = ".".join(parts + ([rest] if rest else []))
            if not candidate:
                continue
            found = sorted(set(self.index.get(candidate) or []))
            if len(found) == 1:
                return found[0]
            if len(found) > 1:
                self._decide(
                    f"取り込み {candidate} から関係を張らなかった",
                    f"候補が {len(found)} 件あり 1 つに決められない: "
                    + "、".join(found),
                    decisions.GUESS, anchor.id)
                return None
        return None                        # 外部ライブラリ（関係にしない）

    # ── コマンド ────────────────────────────────────────────────
    def _commands(self, anchor: mdio.Anchor) -> None:
        table = _Table(mdio.rows(anchor))
        for row in table.body:
            if table.get(row, "kind") != "コマンド":
                continue
            quoted = _QUOTED.match(table.get(row, "名前"))
            if not quoted:
                # 名前が変数（補助関数で包んだ CLI）。何を指すかを解くのは
                # 整理層 ―― アンカーは参照レコードが回収する。
                self._decide(
                    f"add_parser の名前 {table.get(row, '名前')} から command を"
                    "起こさなかった",
                    "名前が定数でない（何を指すかは整理層が原本を読んで決める）",
                    decisions.GUESS, anchor.id)
                continue
            name = quoted.group("name") or quoted.group("dq") or ""
            at = self._at(anchor, table.get(row, "line"))
            record: dict[str, Any] = {
                "concept": f"c-cmd-{self.dotted}.{name}",
                "type": "コマンド",
                "name": name,
                "statement": self._slot(at),
                "source": {"anchor": anchor.id},
            }
            help_text = table.get(row, "help")
            if help_text:
                record["attrs"] = {"description": help_text}
            self._claim(anchor.id)
            self.records.append(record)


#: 特殊メソッド。
_DUNDER = re.compile(r"^__\w+__$")


def _is_public_java(decl: str) -> bool:
    """Java の宣言の公開判定。**宣言欄が無い（Python）なら常に真**（Python の
    公開判定は名前の ``_`` が持つ）。"""
    if not decl:
        return True
    return bool(re.search(r"\bpublic\b", decl))


def _anchor_key(anchor_id: str) -> tuple[str, int]:
    found = re.match(r"^([a-z]+)(\d+)$", anchor_id)
    if not found:
        return (anchor_id, 0)
    return (found.group(1), int(found.group(2)))


def _module_index(files: dict[str, mdio.ParsedFile]) -> dict[str, list[str]]:
    """ドット結びの名前 → ファイル。**末尾一致の候補もここから引く。**"""
    index: dict[str, list[str]] = {}
    for file, document in files.items():
        if not _is_code(document):
            continue
        index.setdefault(_dotted(file), []).append(file)
        # `src.arp4.mdio` は `arp4.mdio` でも引ける（import は置き場の
        # `src` を知らない）。
        parts = _dotted(file).split(".")
        for start in range(1, len(parts)):
            index.setdefault(".".join(parts[start:]), []).append(file)
    return index
