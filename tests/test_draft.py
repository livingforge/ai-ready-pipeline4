"""``arp4 draft`` ―― コード整理の 9 割は機械の仕事（Phase 2）。

見るものは 3 つ。**同一入力から同一出力**（バイト一致 ―― 再現性の土台）、
**アンカーの網羅**（draft が書いたファイルは未整理 0）、**文章化だけで通しが
通る**（TODO を埋めれば freeze / build / check が error 0 になる）。
"""

from __future__ import annotations

import re

from arp4 import build, concepts as concepts_module, draft, freeze, mdio
from arp4 import organized as organized_module, parse, sequence, spec as spec_module
from arp4 import yamlio
from arp4.metamodel import Metamodel
from arp4.paths import Paths, Round
from arp4.validate import validate
from conftest import codes, sources_dir, write

_LIB = '''\
"""読み書きの層。"""
from dataclasses import dataclass

EXT = ".md"
_PRIVATE = 1


@dataclass
class Doc:
    title: str

    def render(self) -> str:
        return self.title

    def _hidden(self) -> None:
        pass

    def __len__(self) -> int:
        return 0


def read(path: str) -> str:
    return path


def _helper() -> None:
    pass
'''

_APP = '''\
from lib import read


def main(path: str) -> int:
    print(read(path))
    return 0
'''

_TEST = '''\
from app import main
from helper import fake


def test_mainは0を返す() -> None:
    assert main("x") == 0
'''

_HELPER = '''\
def fake() -> str:
    return "x"
'''


def _setup(project: Paths, round_: Round,
           files: dict[str, str]) -> None:
    root = sources_dir(project)
    for name, body in files.items():
        write(root / name, body)
    targets, findings = parse.plan(round_, [root], root)
    assert not [f for f in findings if f.level == "error"]
    parse.write(targets)


def _python_files() -> dict[str, str]:
    return {"lib.py": _LIB, "app.py": _APP,
            "test_app.py": _TEST, "helper.py": _HELPER}


# ── 再現性 ──────────────────────────────────────────────────────
def test_同一入力から同一出力(project: Paths, round_: Round) -> None:
    """**バイト一致。** 時刻も乱数も辞書順の揺れも含まない。"""
    _setup(project, round_, _python_files())

    first = draft.plan(round_)
    second = draft.plan(round_)

    assert [d.file for d in first.drafted] == [d.file for d in second.drafted]
    for a, b in zip(first.drafted, second.drafted):
        assert yamlio.dumps(a.data) == yamlio.dumps(b.data)
    assert first.decisions == second.decisions


# ── アンカーの網羅 ──────────────────────────────────────────────
def test_draftしたファイルに未整理が残らない(project: Paths, round_: Round,
                                             model: Metamodel) -> None:
    """余ったアンカーは参照レコードが回収する ―― **静かに消えない**の実装。"""
    _setup(project, round_, _python_files())
    draft.write(round_, draft.plan(round_))

    report = freeze.gate(round_, model, {})

    assert "G001" not in codes(report.findings)
    # 文章は空いている（G026 が数える）。凍結はまだ通らない ―― 骨格だけの
    # 正本を黙って作らないため。
    assert "G026" in codes(report.findings)
    assert report.blocked


def test_骨格の中身(project: Paths, round_: Round) -> None:
    _setup(project, round_, _python_files())
    result = draft.plan(round_)
    data = {d.file: d.data for d in result.drafted}

    lib = data["lib.py"]["records"]
    file_record = [r for r in lib if r.get("concept") == "c-mod-lib"
                   and r.get("type")][0]
    assert file_record["name"] == "lib"
    assert "EXT" in file_record["attrs"]["constants"]        # 公開定数の受け皿
    assert "_PRIVATE" not in file_record["attrs"]["constants"]

    doc = [r for r in lib if r.get("concept") == "c-mod-lib.Doc"][0]
    assert doc["attrs"]["tier"] == "Common"                  # @dataclass → Common
    assert doc["attrs"]["class_name"] == "lib.Doc"
    assert {"rel": "refines", "to": "c-mod-lib"} in doc["refs"]
    assert {"rel": "has-method", "to": "c-mtd-lib.Doc.render"} in doc["refs"]
    # 内部・dunder は起こさず本数で申告（規約 G023 / organize.md）
    assert "内部用のメソッド 1 本" in doc["attrs"]["description"]
    assert "特殊メソッド 1 本" in doc["attrs"]["description"]
    assert not [r for r in lib if r.get("concept", "").endswith("__len__")]

    render = [r for r in lib if r.get("concept") == "c-mtd-lib.Doc.render"][0]
    assert render["attrs"]["signature"] == "render()"
    assert render["attrs"]["returns"] == "str"

    # 取り込みの解決 ―― calls はモジュールへ畳む（i1 のレコードが張る）
    app = data["app.py"]["records"]
    imports = [r for r in app if r.get("source", {}).get("anchor") == "i1"]
    assert any({"rel": "calls", "to": "c-mod-lib"} in (r.get("refs") or [])
               for r in imports)


def test_テストはtest_caseになりverifiesが張られる(project: Paths,
                                                   round_: Round) -> None:
    _setup(project, round_, _python_files())
    result = draft.plan(round_)
    data = {d.file: d.data for d in result.drafted}

    cases = [r for r in data["test_app.py"]["records"]
             if r.get("type") == "テストケース"]
    assert len(cases) == 1
    case = cases[0]
    assert case["concept"] == "c-tcs-test_app.test_mainは0を返す"
    refs = case["refs"]
    assert {"rel": "verifies", "to": "c-mod-app"} in refs
    # expected（必須属性）も文章化スロットとして空く
    assert case["attrs"]["expected"].startswith("<TODO")


# ── 文章化だけで通しが通る ──────────────────────────────────────
def _fill(round_: Round) -> None:
    """文章化スロットを機械的に埋める（LLM の代役）。識別子を本文に置く。"""
    for path in organized_module.yaml_files(round_):
        data = yamlio.load(path)
        if not isinstance(data, dict):
            continue
        for record in data.get("records") or []:
            name = str(record.get("name") or "")
            if isinstance(record.get("statement"), str) and \
                    record["statement"].startswith("<TODO"):
                record["statement"] = f"{name} は仕様の検体として振る舞うこと"
            attrs = record.get("attrs") or {}
            for key, value in attrs.items():
                if isinstance(value, str) and value.startswith("<TODO"):
                    attrs[key] = "正常に終わること"
        yamlio.dump(path, data)


def test_文章化だけでfreezeとbuildとcheckが通る(project: Paths, round_: Round,
                                                model: Metamodel) -> None:
    _setup(project, round_, _python_files())
    draft.write(round_, draft.plan(round_))
    _fill(round_)

    report = freeze.gate(round_, model, {})
    assert not report.blocked, [f.render() for f in report.findings
                                if f.level == "error"]
    freeze.apply(round_, report)

    spec, findings = spec_module.load(project)
    assert not [f for f in findings if f.level == "error"]
    result, _ = organized_module.load(round_)
    known, _ = concepts_module.load(project)
    plan = build.plan(spec, result, known, round_.name)
    assert not [f for f in plan.findings if f.level == "error"]
    build.apply(spec, plan)

    assignments, _ = sequence.assign(spec)
    sequence.apply(spec, assignments)

    said = [f for f in validate(spec) if f.level == "error"]
    assert not said, [f.render() for f in said]


# ── 文章化の検査 ────────────────────────────────────────────────
def test_TODOが残ればG026で凍結が止まる(project: Paths, round_: Round,
                                        model: Metamodel) -> None:
    _setup(project, round_, {"lib.py": _LIB})
    draft.write(round_, draft.plan(round_))

    report = freeze.gate(round_, model, {})
    todo = [f for f in report.findings if f.code == "G026"]
    assert todo and all(f.level == "error" for f in todo)


def test_抽出でない文章はG027(project: Paths, round_: Round,
                              model: Metamodel) -> None:
    """識別子の無い言い換え・レンジ外の長さは**ブレの発生源**として警告する。"""
    _setup(project, round_, {"lib.py": _LIB})
    draft.write(round_, draft.plan(round_))
    for path in organized_module.yaml_files(round_):
        data = yamlio.load(path)
        for record in data.get("records") or []:
            if isinstance(record.get("statement"), str) and \
                    record["statement"].startswith("<TODO"):
                record["statement"] = "読み書きの層の仕様の入れ物であること"   # 識別子なし
            attrs = record.get("attrs") or {}
            for key, value in attrs.items():
                if isinstance(value, str) and value.startswith("<TODO"):
                    attrs[key] = "正常に終わること"
        yamlio.dump(path, data)

    report = freeze.gate(round_, model, {})
    assert "G027" in codes(report.findings)
    assert all(f.level == "warn" for f in report.findings
               if f.code == "G027")


def test_人が書いた整理結果にはG027を掛けない(round_: Round,
                                              model: Metamodel) -> None:
    """draft の文章契約は draft が書いたファイル（``drafted:`` の印）だけのもの。"""
    from conftest import organized, parsed
    parsed(round_, "src/x.py.md", """\
# src/x.py

<!-- source: src/x.py -->

## 取り込み  <!-- a:i1 at=src/x.py -->

| 取り込み | 元 | 名前 | 行 |
|---|---|---|---|
""")
    organized(round_, "src/x.py.yml", """\
records:
  - concept: c-mod-src.x
    type: モジュール
    name: x
    statement: 短い文
    source: { anchor: i1 }
""")
    assert "G027" not in codes(freeze.gate(round_, model, {}).findings)


# ── 上書きしない ────────────────────────────────────────────────
def test_整理結果が既にあるファイルは飛ばす(project: Paths,
                                            round_: Round) -> None:
    _setup(project, round_, {"lib.py": _LIB, "app.py": _APP})
    write(round_.organized / "lib.py.yml", "records: []\n")

    result = draft.plan(round_)

    assert result.skipped == ["lib.py"]
    assert [d.file for d in result.drafted] == ["app.py"]


# ── Java ────────────────────────────────────────────────────────
_JAVA_MAIN = """\
package com.example.util;

import com.example.core.Helper;

@Service
public class Text {
    public String trim(String s) { return s; }
    private void inner() { }
}
"""

_JAVA_HELPER = """\
package com.example.core;

public class Helper {
    public int size() { return 0; }
}
"""


def test_javaはファイルと同名の型がモジュールになる(project: Paths,
                                                    round_: Round,
                                                    model: Metamodel) -> None:
    _setup(project, round_, {
        "src/main/java/com/example/util/Text.java": _JAVA_MAIN,
        "src/main/java/com/example/core/Helper.java": _JAVA_HELPER})
    result = draft.plan(round_)
    data = {d.file: d.data for d in result.drafted}
    records = data["src/main/java/com/example/util/Text.java"]["records"]

    primary = [r for r in records if r.get("type") == "モジュール"][0]
    # ファイル名と同名の型に `….Text.Text` と二重の名前を付けない
    assert primary["concept"] == "c-mod-src.main.java.com.example.util.Text"
    assert primary["attrs"]["class_name"] == "com.example.util.Text"
    assert primary["attrs"]["package"] == "com.example.util"
    assert primary["attrs"]["tier"] == "Service"             # @Service の転記

    methods = [r for r in records if r.get("type") == "メソッド"]
    assert [m["name"] for m in methods] == ["trim"]          # public だけ

    imports = [r for r in records
               if r.get("source", {}).get("anchor") == "i1"][0]
    assert {"rel": "calls",
            "to": "c-mod-src.main.java.com.example.core.Helper"} \
        in imports["refs"]

    # 網羅 ―― Java でも未整理 0
    draft.write(round_, result)
    assert "G001" not in codes(freeze.gate(round_, model, {}).findings)


# ── 決定ログ ────────────────────────────────────────────────────
def test_draftの判断は決定ログに残る(project: Paths, round_: Round) -> None:
    from arp4 import decisions
    _setup(project, round_, {"lib.py": _LIB})
    draft.write(round_, draft.plan(round_))

    said = decisions.load(round_)
    assert said and all(e.get("by") == "draft" for e in said)
    assert any("tier=Common" in str(e.get("what")) for e in said)
    assert all(e.get("basis") for e in said)                 # 根拠アンカー必須

    # 2 回書いても判断は二重に積まれない（置き換え）
    draft.write(round_, draft.plan(round_))
    assert len(decisions.load(round_)) == len(said)
