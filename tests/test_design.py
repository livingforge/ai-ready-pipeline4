"""``arp4 design`` ―― 正本からプログラム設計の骨格を機械生成する（``draft`` の鏡像）。

見るものは 4 つ。**同一入力から同一出力**（バイト一致）、**転記しかしない**
（出典のパスとシグネチャに書いてあることだけ）、**決めないものは決めない**
（実装規約とエンドポイントは作らず、次にどこへ行くかを言う）、そして
**文章化だけで freeze / build / check が通る**。

`draft` と同じ流れを 1 段先で繰り返している ―― コードのラウンドを通して正本を
作り、そこへ `design` を掛けると引数と問い合わせの骨格が出る。
"""

from __future__ import annotations

from arp4 import build, concepts as concepts_module, design, draft, freeze
from arp4 import organized as organized_module, parse, sequence
from arp4 import spec as spec_module, yamlio
from arp4.metamodel import Metamodel
from arp4.paths import Paths, Round
from arp4.validate import validate
from conftest import sources_dir, write

_ORDER = '''\
"""受注の登録。"""
from dataclasses import dataclass


@dataclass
class OrderService:
    """受注を登録する。"""

    def register(self, order: str, note: str = "", *, dry: bool = False) -> int:
        return 0

    def _hidden(self) -> None:
        pass


def build_key(prefix: str, number: int) -> str:
    return f"{prefix}{number}"
'''


def _round_through(project: Paths, round_: Round, model: Metamodel,
                   files: dict[str, str]):
    """コードのラウンドを 1 周通して正本を作る（`design` の入力を用意する）。"""
    root = sources_dir(project)
    for name, body in files.items():
        write(root / name, body)
    targets, findings = parse.plan(round_, [root], root)
    assert not [f for f in findings if f.level == "error"]
    parse.write(targets)

    draft.write(round_, draft.plan(round_))
    for path in organized_module.yaml_files(round_):
        data = yamlio.load(path)
        if not isinstance(data, dict):
            continue
        for record in data.get("records") or []:
            name = str(record.get("name") or "")
            if str(record.get("statement") or "").startswith("<TODO"):
                record["statement"] = f"{name} は仕様の検体として振る舞うこと"
            for key, value in (record.get("attrs") or {}).items():
                if isinstance(value, str) and value.startswith("<TODO"):
                    record["attrs"][key] = "正常に終わること"
        yamlio.dump(path, data)

    report = freeze.gate(round_, model, {})
    assert not report.blocked, [f.render() for f in report.findings
                                if f.level == "error"]
    freeze.apply(round_, report)

    spec, _ = spec_module.load(project)
    result, _ = organized_module.load(round_)
    known, _ = concepts_module.load(project)
    plan = build.plan(spec, result, known, round_.name)
    build.apply(spec, plan)
    concepts_module.save(project, plan.concepts)
    spec_module.save_in_place(spec)
    return spec_module.load(project)[0], concepts_module.load(project)[0]


# ── 再現性 ──────────────────────────────────────────────────────
def test_同一入力から同一出力(project: Paths, round_: Round,
                              model: Metamodel) -> None:
    """**バイト一致。** 時刻も乱数も辞書順の揺れも含まない（`draft` と同じ）。"""
    spec, known = _round_through(project, round_, model, {"order.py": _ORDER})

    first = design.plan(spec, round_, known)
    second = design.plan(spec, round_, known)

    assert [d.file for d in first.designed] == [d.file for d in second.designed]
    for a, b in zip(first.designed, second.designed):
        assert yamlio.dumps(a.data) == yamlio.dumps(b.data)
    assert first.decisions == second.decisions


# ── 転記しかしない ──────────────────────────────────────────────
def _records(result: design.Result) -> list[dict]:
    return [r for d in result.designed for r in d.data["records"]]


def test_言語と出す先は出典のパスの転記(project: Paths, round_: Round,
                                        model: Metamodel) -> None:
    """**当てにいく余地が 1 つも無い。** 拡張子は資料のパスに書いてある。"""
    spec, known = _round_through(project, round_, model, {"order.py": _ORDER})
    found = [r for r in _records(design.plan(spec, round_, known))
             if r.get("concept") == "c-mod-order" and r.get("attrs")]

    assert found, "モジュールの実装の欄が起きていません"
    attrs = found[0]["attrs"]
    assert attrs["language"] == "Python"
    assert attrs["source_path"] == "order.py"


def test_引数はシグネチャの升を割ったもの(project: Paths, round_: Round,
                                          model: Metamodel) -> None:
    """**呼ぶ順が仕様である。** 既定値のある引数は省略可として写す。

    ``self`` は `parse` が既に落としている（シグネチャに現れない）ので、ここは
    落とす処理を持たない ―― 同じ規則を 2 か所に置かない。
    """
    spec, known = _round_through(project, round_, model, {"order.py": _ORDER})
    records = _records(design.plan(spec, round_, known))

    parameters = {r["attrs"]["param_name"]: r for r in records
                  if r.get("type") == "引数"}
    assert set(parameters) >= {"order", "note", "dry", "prefix", "number"}
    assert parameters["order"]["attrs"]["impl_type"] == "str"
    # 既定値は**シグネチャに出ている綴りのまま**（`ast.unparse` は `''` と出す）。
    assert parameters["note"]["attrs"]["default_value"] == "''"
    assert parameters["note"]["attrs"]["optional"] is True
    assert parameters["order"]["attrs"].get("optional") is None
    # 文章は空ける ―― 「その引数が何か」は原本にしか無い。
    assert parameters["order"]["statement"].startswith("<TODO")

    # 並びは has-parameter が持つ（`ordered: true`）。
    method = [r for r in records
              if r.get("concept") == "c-mtd-order.OrderService.register"][0]
    assert [ref["to"] for ref in method["refs"] if ref["rel"] == "has-parameter"] \
        == ["c-prm-order.OrderService.register.order",
            "c-prm-order.OrderService.register.note",
            "c-prm-order.OrderService.register.dry"]


def test_呼ぶ名前と公開範囲はシグネチャの頭から(project: Paths, round_: Round,
                                                model: Metamodel) -> None:
    """`name` は日本語の見出し、`signature` は資料の写しで、**実名が無かった。**"""
    spec, known = _round_through(project, round_, model, {"order.py": _ORDER})
    records = _records(design.plan(spec, round_, known))

    method = [r for r in records
              if r.get("concept") == "c-mtd-order.OrderService.register"][0]
    assert method["attrs"]["method_name"] == "register"
    assert method["attrs"]["visibility"] == "public"


# ── 決めないものは決めない ──────────────────────────────────────
def test_実装規約とエンドポイントは作らない(project: Paths, round_: Round,
                                            model: Metamodel) -> None:
    """**作れば全部が作文になる。** 言語も配置も命名も正本のどこにも無い。

    黙って作らないのではなく、**次にどこへ行くか**を申告する ―― 空の生成物と
    「作らないと決めた」は、端末では同じ顔をする。
    """
    spec, known = _round_through(project, round_, model, {"order.py": _ORDER})
    result = design.plan(spec, round_, known)

    assert not [r for r in _records(result)
                if r.get("type") in ("実装規約", "エンドポイント")]
    said = " ".join(result.notes)
    assert "実装規約" in said and "資料です" in said
    assert "エンドポイント" in said


def test_書いたものは上書きしない(project: Paths, round_: Round,
                                  model: Metamodel) -> None:
    """文章を埋めたあとの再実行で潰さない（`draft` と同じ規律）。"""
    spec, known = _round_through(project, round_, model, {"order.py": _ORDER})
    design.write(round_, design.plan(spec, round_, known))

    again = design.plan(spec, round_, known)

    assert not again.designed
    assert again.skipped


# ── 文章化だけで通しが通る ──────────────────────────────────────
def test_文章化だけでfreezeとbuildとcheckが通る(project: Paths, round_: Round,
                                                model: Metamodel) -> None:
    """**骨格 → 文章 → 正本**が 1 本で繋がる（`draft` と同じ確かめ方）。"""
    spec, known = _round_through(project, round_, model, {"order.py": _ORDER})
    round_.unfreeze() if hasattr(round_, "unfreeze") else None
    design.write(round_, design.plan(spec, round_, known))
    for path in organized_module.yaml_files(round_):
        data = yamlio.load(path)
        if not isinstance(data, dict):
            continue
        changed = False
        for record in data.get("records") or []:
            if str(record.get("statement") or "").startswith("<TODO"):
                record["statement"] = \
                    f"{record.get('name')} は仕様の検体として振る舞うこと"
                changed = True
        if changed:
            yamlio.dump(path, data)

    result, _ = organized_module.load(round_)
    plan = build.plan(spec, result, known, round_.name)
    assert not [f for f in plan.findings if f.level == "error"], \
        [f.render() for f in plan.findings if f.level == "error"]
    build.apply(spec, plan)
    assignments, _ = sequence.assign(spec)
    sequence.apply(spec, assignments)

    said = [f for f in validate(spec) if f.level == "error"]
    assert not said, [f.render() for f in said]

    # 引数が正本に立ち、メソッドから順序つきで引ける。
    parameters = [i for i in spec.items if i.get("type") == "parameter"]
    assert parameters
    assert all(p.get("param_name") for p in parameters)
    assert [r for r in spec.relations if r.get("type") == "has-parameter"]


# ── シグネチャの割り（4 言語）────────────────────────────────────
def test_4言語のシグネチャを割る() -> None:
    """**対象は C / Java / JS・TS / Python。** 割り方は言語ごとに違う。

    ここは正本を通さずに 1 本ずつ突く ―― 通しで見ると、割れなかった 1 本が
    「資料にそう書いてあった」に紛れる。
    """
    def split(signature: str, language: str) -> list[tuple]:
        return [design.split_argument(t, language)
                for t in design.arguments(signature, language)]

    assert split("scan(directory: Path, *, deep: bool = False)", "Python") == [
        ("directory", "Path", "", False), None, ("deep", "bool", "False", True)]
    assert split("public Order register(@Valid Order order, final String note)",
                 "Java") == [("order", "Order", "", False),
                             ("note", "String", "", False)]
    assert split("register(order: Order, note?: string): number",
                 "TypeScript") == [("order", "Order", "", False),
                                   ("note", "string", "", True)]
    # C の `void` は「引数 0 本」であって、`void` という名前の引数ではない。
    assert split("int main(void)", "C") == []
    assert split("int put(const char *name, size_t len)", "C") == [
        ("name", "const char*", "", False), ("len", "size_t", "", False)]
    # 型引数のカンマで割らない（深さを見る）。
    assert split("void put(Map<String, Integer> counts)", "Java") == [
        ("counts", "Map<String, Integer>", "", False)]


def test_型は正本に1つだけ当たるときしか指さない() -> None:
    """**当てにいって半分間違えるより、当てないほうがよい**（`draft` と同じ）。"""
    assert design._bare_type("List<Order>") == "List"
    assert design._bare_type("const char *") == "char"
    assert design._bare_type("Optional[str]") == "Optional"
