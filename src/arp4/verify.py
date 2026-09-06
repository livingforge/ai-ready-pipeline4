"""``arp4 verify`` ―― 実装と正本を突き合わせる（閉ループ）。

``emit`` が正本からコードを出すのに対し、こちらは**書かれたコードを読み直して
正本と比べる** ―― 生成したものでも、人が書いたものでも、LLM が書いたものでも
同じように効く。**揺れは「測れる」ようになって初めて止まる。**

読み方は `parse` そのものである（構文木で取る）―― ここに別の読み手を置くと、
`draft` が起こしたものと `verify` が見るものがずれる。見るのは 4 つ::

    V001  正本にあるのに実装に無い（未実装）                        warn
    V002  実装にあるのに正本に無い（設計に戻っていない）            warn
    V003  引数が違う（名前・並び・本数）                            error
    V004  正本が出す先と言っているファイルが、渡した範囲に無い      warn

**`V003` だけが error である。** 未実装も設計外も開発の途中では普通に起きるが、
**引数の食い違いは呼ぶ側と噛み合わない** ―― プログラム設計を入れた目的が
そこにあるので、そこだけ止める（`--strict` を付ければ全部止まる）。

**決めていないものは比べない。** `method_name` を持たないメソッドや
`source_path` を持たないモジュールは、正本がまだ何も言っていないので黙って
飛ばし、件数だけを申告する ―― 比べていないことを「一致した」と出さない。
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any

from arp4 import design, draft, mdio
from arp4.finding import Finding, order
from arp4.spec import Spec

#: クラスにあたる塊の見出し（`parse` が書いている）。**テストクラスは見ない**
#: ―― あれは検証の宣言であって実装ではない。
_TYPES = ("クラス: ", "インタフェース: ", "列挙: ", "レコード: ", "注釈型: ")

#: ファイル直下の関数が載る塊。
_FUNCTIONS = "モジュール関数"

#: メンバの表で「呼べるもの」の種類。
_CALLABLE = ("メソッド", "関数")


@dataclass(frozen=True)
class Unit:
    """実装の側の 1 単位（ファイル直下、またはクラス 1 つ）。"""

    source: str                          # 渡したパスからの相対
    class_name: str                      # "" ならファイル直下
    #: 公開メンバ ``名前 → シグネチャ``。
    methods: dict[str, str] = field(default_factory=dict)


@dataclass
class Report:
    findings: list[Finding] = field(default_factory=list)
    #: 突き合わせた組の数（モジュール・メソッド）。
    modules: int = 0
    methods: int = 0
    #: 比べなかったもの ―― **「一致した」と混ぜない。**
    unspecified: int = 0                 # 正本がまだ何も言っていない
    outside: int = 0                     # 渡した範囲の外にあるモジュール

    @property
    def blocked(self) -> bool:
        return any(f.level == "error" for f in self.findings)


# ── 入口 ────────────────────────────────────────────────────────
def verify(spec: Spec, docs: list[mdio.Doc]) -> Report:
    """正本と、読み直した実装を突き合わせる。**書き込みはしない。**"""
    report = Report()
    units = _units(docs)
    sources = {doc.source for doc in docs}
    seen: set[tuple[str, str]] = set()

    for module in sorted(spec.of_type("module"),
                         key=lambda m: str(m.get("module_id") or m.get("id"))):
        path = str(module.get("source_path") or "")
        if not path:
            report.unspecified += 1
            continue
        if path not in sources:
            # **渡していないだけかもしれない。** 範囲の外を「無い」と言うと、
            # 一部だけ突き合わせたときに指摘が本物の欠落を埋める。
            report.outside += 1
            continue
        key = (path, _class_of(module))
        seen.add(key)
        unit = units.get(key)
        if unit is None:
            # **メソッドを 1 本も宣言していないなら、比べるものが無い。** 実測で
            # 「クラスしか無いファイル」がここに落ちた ―― `draft` はファイル
            # そのもののレコードを必ず起こすが、モジュール関数の塊は無い。
            # 宣言が無いのに「実装に見当たりません」と言うと、**正しい状態が
            # 毎回 1 件鳴る。**
            if not _named_methods(spec, module):
                continue
            report.findings.append(Finding(
                "warn", "V004", _named(module),
                f"正本は {path} に "
                f"{_class_of(module) or 'ファイル直下'} を出すと言っていますが、"
                "実装に見当たりません", file=path))
            continue
        report.modules += 1
        _methods(spec, module, unit, report)

    for key, unit in sorted(units.items()):
        if key in seen or not unit.methods:
            continue
        report.findings.append(Finding(
            "warn", "V002", unit.class_name or unit.source,
            f"実装にあるのに正本にありません（公開メンバ {len(unit.methods)} 件: "
            f"{'・'.join(sorted(unit.methods)[:5])}"
            f"{'…' if len(unit.methods) > 5 else ''}）",
            file=unit.source))

    report.findings = order(report.findings)
    return report


def _own_methods(spec: Spec, module: dict[str, Any]) -> list[dict[str, Any]]:
    """モジュールが持つメソッド。**並び順**（`has-method` は `ordered`）。"""
    found = [r for r in spec.relations_of("has-method")
             if str(r.get("from")) == str(module.get("id"))]
    out = []
    for relation in sorted(found, key=lambda r: (int(r.get("order") or 0),
                                                 str(r.get("to")))):
        method = spec.by_id.get(str(relation.get("to")))
        if method is not None:
            out.append(method)
    return out


def _named_methods(spec: Spec, module: dict[str, Any]) -> list[dict[str, Any]]:
    """**呼ぶ名前が決まっているものだけ。** 残りは比べようがない。"""
    return [m for m in _own_methods(spec, module) if m.get("method_name")]


def _methods(spec: Spec, module: dict[str, Any], unit: Unit,
             report: Report) -> None:
    language = str(module.get("language") or "")
    named: dict[str, dict[str, Any]] = {}

    for method in _own_methods(spec, module):
        name = str(method.get("method_name") or "")
        if not name:
            report.unspecified += 1      # 呼ぶ名前をまだ決めていない
            continue
        named[name] = method
        if name not in unit.methods:
            report.findings.append(Finding(
                "warn", "V001", _named(method),
                f"正本にありますが実装にありません（{unit.source} の "
                f"{unit.class_name or 'ファイル直下'}）", file=unit.source))
            continue
        report.methods += 1
        _parameters(spec, method, unit.methods[name], language, unit, report)

    for name in sorted(unit.methods):
        if name in named:
            continue
        report.findings.append(Finding(
            "warn", "V002", f"{unit.class_name or unit.source}.{name}",
            "実装にありますが正本にありません（設計に戻っていない公開メンバ）",
            file=unit.source))


def _parameters(spec: Spec, method: dict[str, Any], signature: str,
                language: str, unit: Unit, report: Report) -> None:
    """**引数の食い違いだけが error である。** 呼ぶ側と噛み合わないため。"""
    declared = [str(p.get("param_name") or "") for p in
                _parameter_items(spec, method)]
    if not declared:
        report.unspecified += 1          # 引数をまだ決めていない
        return
    written = [split[0] for split in
               (design.split_argument(token, language)
                for token in design.arguments(signature, language))
               if split]
    if declared == written:
        return
    report.findings.append(Finding(
        "error", "V003", _named(method),
        f"引数が正本と違います ―― 正本 ({', '.join(declared)}) / "
        f"実装 ({', '.join(written)})", file=unit.source,
        hint="どちらかを直す。実装が正しいなら arp4 design を掛け直すか、"
             "引数のレコードを書き直してから build する"))


def _parameter_items(spec: Spec, method: dict[str, Any]) -> list[dict[str, Any]]:
    found = [r for r in spec.relations_of("has-parameter")
             if str(r.get("from")) == str(method.get("id"))]
    out = []
    for relation in sorted(found, key=lambda r: (int(r.get("order") or 0),
                                                 str(r.get("to")))):
        item = spec.by_id.get(str(relation.get("to")))
        if item is not None:
            out.append(item)
    return out


# ── 実装の側を読む ──────────────────────────────────────────────
def _units(docs: list[mdio.Doc]) -> dict[tuple[str, str], Unit]:
    """パース結果の塊から、実装の単位を起こす。

    **`draft` と同じ見出し・同じ欄で読む**（`draft.MEMBER_COLUMNS`）―― 別々に
    覚えると、起こす側と突き合わせる側で片方だけ古くなる。
    """
    units: dict[tuple[str, str], Unit] = {}
    for doc in docs:
        java = doc.source.endswith(".java")
        for chunk in doc.chunks:
            heading = str(chunk.heading or "")
            if heading.startswith(_TYPES):
                name = heading.split(": ", 1)[1]
            elif heading == _FUNCTIONS:
                name = ""
            else:
                continue                 # 定数・取り込み・テストは実装ではない
            unit = units.setdefault((doc.source, name),
                                    Unit(source=doc.source, class_name=name))
            unit.methods.update(_members(chunk, name, java))
    return units


def _members(chunk: mdio.Chunk, class_name: str,
             java: bool) -> dict[str, str]:
    """塊 1 つの公開メンバ。**公開名だけ**（`draft` の規約と同じ）。"""
    table = draft.Table(chunk.rows)
    found: dict[str, str] = {}
    for row in table.body:
        name = table.get(row, "name")
        if table.get(row, "kind") not in _CALLABLE:
            continue
        if not name or name.startswith("_") or name == class_name:
            continue                     # 内部用・特殊メソッド・コンストラクタ
        declaration = table.get(row, "decl")
        if java and declaration and "public" not in declaration:
            continue                     # Java は宣言に書いてある可視性で見る
        found[name] = table.get(row, "signature") or declaration
    return found


# ── 小道具 ──────────────────────────────────────────────────────
def _class_of(module: dict[str, Any]) -> str:
    """正本の ``class_name`` の末尾。**パッケージは出す先が持っている。**"""
    return str(module.get("class_name") or "").rsplit(".", 1)[-1]


def _named(item: dict[str, Any]) -> str:
    for key in ("module_id", "method_id"):
        if item.get(key):
            return f"{item.get(key)}（{item.get('name')}）"
    return str(item.get("name") or item.get("id") or "")
