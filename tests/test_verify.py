"""``arp4 verify`` ―― 実装と正本を突き合わせる（閉ループ）。

**揺れは「測れる」ようになって初めて止まる。** ここで見るのは 4 つ ――
一致すれば黙る、引数が違えば **error** で止まる、未実装と設計外は warn で言う、
そして**比べていないものを「一致した」と出さない**。

確かめ方は通しである ―― コードを 1 周させて正本を作り（`draft` → `freeze` →
`build` → `design`）、**そのコードそのものを突き合わせると 0 件**になる。
そこから実装をずらすと、ずらしたぶんだけが出る。
"""

from __future__ import annotations

from arp4 import build, concepts as concepts_module, design, draft, freeze
from arp4 import organized as organized_module, parse
from arp4 import spec as spec_module, verify, yamlio
from arp4.metamodel import Metamodel
from arp4.paths import Paths, Round
from conftest import codes, sources_dir, write

_ORDER = '''\
"""受注の登録。"""
from dataclasses import dataclass


@dataclass
class OrderService:
    """受注を登録する。"""

    def register(self, order: str, amount: int) -> int:
        return 0
'''


def _fill(round_: Round) -> None:
    for path in organized_module.yaml_files(round_):
        data = yamlio.load(path)
        if not isinstance(data, dict):
            continue
        for record in data.get("records") or []:
            if str(record.get("statement") or "").startswith("<TODO"):
                record["statement"] = \
                    f"{record.get('name')} は仕様の検体として振る舞うこと"
            for key, value in (record.get("attrs") or {}).items():
                if isinstance(value, str) and value.startswith("<TODO"):
                    record["attrs"][key] = "正常に終わること"
        yamlio.dump(path, data)


def _build(project: Paths, round_: Round):
    spec, _ = spec_module.load(project)
    result, _ = organized_module.load(round_)
    known, _ = concepts_module.load(project)
    plan = build.plan(spec, result, known, round_.name)
    build.apply(spec, plan)
    concepts_module.save(project, plan.concepts)
    spec_module.save_in_place(spec)
    return spec_module.load(project)[0], concepts_module.load(project)[0]


def _through(project: Paths, round_: Round, model: Metamodel):
    """コード → 正本 → プログラム設計 まで 1 周通す（`verify` の入力）。"""
    root = sources_dir(project)
    write(root / "order.py", _ORDER)
    targets, _ = parse.plan(round_, [root], root)
    parse.write(targets)
    draft.write(round_, draft.plan(round_))
    _fill(round_)
    report = freeze.gate(round_, model, {})
    assert not report.blocked, [f.render() for f in report.findings
                                if f.level == "error"]
    freeze.apply(round_, report)
    spec, known = _build(project, round_)
    design.write(round_, design.plan(spec, round_, known))
    _fill(round_)
    return _build(project, round_)[0], root


def _docs(round_: Round, root):
    """**書かずに読む。** 突き合わせに要るのは塊であってラウンドの記録ではない。"""
    targets, _ = parse.plan(round_, [root], root)
    return [t.doc for t in targets if t.doc.source.endswith(".py")]


# ── 一致すれば黙る ──────────────────────────────────────────────
def test_同じコードなら指摘は出ない(project: Paths, round_: Round,
                                    model: Metamodel) -> None:
    """**正本はこのコードから起こしたもの**なので、突き合わせは 0 件になる。

    ここが鳴るなら、`draft` / `design` / `verify` のどれかが同じものを別々に
    読んでいる ―― 読み手を 1 つ（`parse`）に寄せてあることの検査である。
    """
    spec, root = _through(project, round_, model)

    report = verify.verify(spec, _docs(round_, root))

    assert report.findings == [], [f.render() for f in report.findings]
    assert report.modules and report.methods


# ── 引数の食い違いだけが error ──────────────────────────────────
def test_引数がずれるとerrorで止まる(project: Paths, round_: Round,
                                     model: Metamodel) -> None:
    """**呼ぶ側と噛み合わない。** プログラム設計を入れた目的がここにある。"""
    spec, root = _through(project, round_, model)
    write(root / "order.py", _ORDER.replace(
        "def register(self, order: str, amount: int) -> int:",
        "def register(self, order: str, amount: int, note: str) -> int:"))

    report = verify.verify(spec, _docs(round_, root))

    said = [f for f in report.findings if f.code == "V003"]
    assert said and said[0].level == "error"
    assert "note" in said[0].message
    assert report.blocked


def test_並びが違うだけでも言う(project: Paths, round_: Round,
                                model: Metamodel) -> None:
    """**並びが仕様である**（`has-parameter` は `ordered`）。"""
    spec, root = _through(project, round_, model)
    write(root / "order.py", _ORDER.replace(
        "def register(self, order: str, amount: int) -> int:",
        "def register(self, amount: int, order: str) -> int:"))

    report = verify.verify(spec, _docs(round_, root))

    assert [f.code for f in report.findings if f.level == "error"] == ["V003"]


# ── 未実装と設計外は warn ───────────────────────────────────────
def test_正本にあるのに実装に無いものはV001(project: Paths, round_: Round,
                                            model: Metamodel) -> None:
    """開発の途中では普通に起きるので **warn**（`--strict` なら止まる）。"""
    spec, root = _through(project, round_, model)
    write(root / "order.py", _ORDER.replace(
        "    def register(self, order: str, amount: int) -> int:\n"
        "        return 0\n", "    pass\n"))

    report = verify.verify(spec, _docs(round_, root))

    said = [f for f in report.findings if f.code == "V001"]
    assert said and all(f.level == "warn" for f in said)
    assert not report.blocked


def test_実装にあるのに正本に無いものはV002(project: Paths, round_: Round,
                                            model: Metamodel) -> None:
    """設計に戻っていない公開メンバ。**内部用（先頭 `_`）は見ない。**"""
    spec, root = _through(project, round_, model)
    write(root / "order.py", _ORDER + '''

    def cancel(self, order: str) -> int:
        return 0

    def _hidden(self) -> None:
        pass
'''.replace("\n\n    def", "\n    def"))

    report = verify.verify(spec, _docs(round_, root))

    said = [f for f in report.findings if f.code == "V002"]
    assert said and "cancel" in " ".join(f.message + f.target for f in said)
    assert "_hidden" not in " ".join(f.message + f.target for f in said)


# ── 比べていないものを「一致した」と出さない ────────────────────
def test_正本が決めていないものは比べず件数で言う(project: Paths, round_: Round,
                                                  model: Metamodel) -> None:
    """`method_name` も `source_path` も無い正本は、まだ何も言っていない。

    黙って一致にすると、**プログラム設計を 1 行も書いていない案件で
    「突き合わせは通りました」と出る。**
    """
    spec, root = _through(project, round_, model)
    for item in spec.items:
        item.pop("source_path", None)
        item.pop("method_name", None)

    report = verify.verify(spec, _docs(round_, root))

    assert report.modules == 0 and report.methods == 0
    assert report.unspecified
    assert "V001" not in codes(report.findings)


def test_渡した範囲の外は欠落にしない(project: Paths, round_: Round,
                                      model: Metamodel) -> None:
    """**渡していないだけかもしれない。** 一部だけ突き合わせたときに、
    範囲の外が本物の欠落を件数で埋める。"""
    spec, root = _through(project, round_, model)

    report = verify.verify(spec, [])          # 1 本も渡さない

    assert report.outside
    assert not [f for f in report.findings if f.code in ("V001", "V004")]
