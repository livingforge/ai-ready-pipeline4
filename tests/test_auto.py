"""``arp4 auto`` と再現性 ―― Phase 4。

**止まるのは「機械が判断できない矛盾・error」と「整理層の手番」だけ**であること、
同一入力の 2 回の通しが**バイト一致**すること（再現性 CI）を見る。
"""

from __future__ import annotations

import re
from pathlib import Path

from arp4 import auto, cli, decisions, organized as organized_module, yamlio
from arp4 import metamodel as mm
from arp4 import paths as paths_module
from arp4.metamodel import Metamodel
from arp4.paths import Paths, Round
from arp4.spec import Spec
from conftest import write

_LIB = '''\
"""読み書きの層。"""
EXT = ".md"


def read(path: str) -> str:
    return path
'''

_TEST = '''\
from lib import read


def test_readは引数を返す() -> None:
    assert read("x") == "x"
'''


def _project(tmp_path: Path, name: str) -> Paths:
    paths = paths_module.create(tmp_path / name)
    root = paths.root / "資料"
    write(root / "lib.py", _LIB)
    write(root / "test_lib.py", _TEST)
    return paths


def _fill(paths: Paths) -> None:
    """文章化スロットを機械的に埋める（LLM の代役 ―― 決定的な埋め方にする）。"""
    round_ = paths.latest_round()
    for path in organized_module.yaml_files(round_):
        data = yamlio.load(path)
        if not isinstance(data, dict):
            continue
        for record in data.get("records") or []:
            name = str(record.get("name") or "")
            if str(record.get("statement") or "").startswith("<TODO"):
                record["statement"] = f"{name} は検体の仕様として振る舞うこと"
            for key, value in (record.get("attrs") or {}).items():
                if isinstance(value, str) and value.startswith("<TODO"):
                    record["attrs"][key] = "正常に終わること"
        yamlio.dump(path, data)


def _auto(paths: Paths) -> int:
    return cli.main(["auto", "--root", str(paths.root),
                     str(paths.root / "資料")])


# ── 停止条件は 2 つだけ ─────────────────────────────────────────
def test_文章化が残っていれば手番の交代で止まる(tmp_path: Path,
                                                capsys) -> None:
    paths = _project(tmp_path, "a")

    code = _auto(paths)

    assert code == 3                             # 完了(0)とも error(1)とも違う
    said = capsys.readouterr().out
    assert "整理層の作業" in said
    assert not paths.latest_round().is_frozen()  # 骨格のまま凍結しない


def test_文章化を済ませれば介入なしで設計書一式まで到達する(
        tmp_path: Path, capsys) -> None:
    paths = _project(tmp_path, "a")
    assert _auto(paths) == 3
    _fill(paths)

    code = _auto(paths)

    assert code == 0, capsys.readouterr().out
    assert paths.latest_round().is_frozen()
    assert (paths.out / "目次.md").is_file()                      # developer
    assert (paths.out / "stakeholder" / "システム概要.md").is_file()
    assert (paths.out / "決定記録.md").is_file()                  # 事後拒否権の入口

    # 決定ログに凍結までの判断が載り、任意の 1 件から出典アンカーへ辿れる
    said = decisions.load(paths.latest_round())
    assert said
    assert any(e.get("basis") for e in said)


# ── 自動昇格（既定は無効） ──────────────────────────────────────
def test_既定ではreview止まり(tmp_path: Path) -> None:
    paths = _project(tmp_path, "a")
    _auto(paths)
    _fill(paths)
    assert _auto(paths) == 0

    from arp4 import spec as spec_module
    spec, _ = spec_module.load(paths)
    assert all(i.get("status") == "review" for i in spec.items)


def test_policyを書けばcheck通過後にapprovedへ昇格する(tmp_path: Path) -> None:
    paths = _project(tmp_path, "a")
    write(paths.policy, "auto_approve: true\n")
    _auto(paths)
    _fill(paths)
    assert _auto(paths) == 0

    from arp4 import spec as spec_module
    spec, _ = spec_module.load(paths)
    assert spec.items and all(i.get("status") == "approved" for i in spec.items)


def test_課題は昇格しない(model: Metamodel) -> None:
    """矛盾から起こした課題の裁定は**常に人に残す**（自動解決はしない）。"""
    spec = Spec(metamodel=model, items=[
        {"id": "iss-1", "type": "open-issue", "status": "review"},
        {"id": "mod-1", "type": "module", "status": "review"},
    ], relations=[])

    changed, _ = auto.promote(spec, "r001")

    assert changed == {"mod-1"}
    assert spec.items[0]["status"] == "review"


# ── known_gaps の自動宣言 ───────────────────────────────────────
def test_相手の種別が正本に無いW031は自動宣言される(model: Metamodel) -> None:
    """機械が言えるのは「相手になれる種別が 1 件も無い」ときだけ（4-4）。"""
    spec = Spec(metamodel=model, items=[
        {"id": "cst-1", "type": "constraint", "status": "review"},
    ], relations=[])

    changed, logged = auto.declare_gaps(spec, "r001")

    assert changed == {"cst-1"}
    assert "constrains" in spec.items[0]["known_gaps"]
    assert "機械判定" in spec.items[0]["known_gaps"]["constrains"]["reason"]
    assert logged and logged[0]["by"] == "auto"


def test_相手候補が居れば自動宣言しない(model: Metamodel) -> None:
    """1 件でも候補が居るなら「その中に相手がいるか」は意味の判断である。"""
    spec = Spec(metamodel=model, items=[
        {"id": "cst-1", "type": "constraint", "status": "review"},
        {"id": "mod-1", "type": "module", "status": "review"},
    ], relations=[])

    changed, _ = auto.declare_gaps(spec, "r001")

    assert changed == set()


def test_人の宣言は上書きしない(model: Metamodel) -> None:
    spec = Spec(metamodel=model, items=[
        {"id": "cst-1", "type": "constraint", "status": "review",
         "known_gaps": {"constrains": {"reason": "先方へ依頼済み"}}},
    ], relations=[])

    changed, _ = auto.declare_gaps(spec, "r001")

    assert changed == set()
    assert spec.items[0]["known_gaps"]["constrains"]["reason"] == "先方へ依頼済み"


# ── 再現性 CI（4-5） ────────────────────────────────────────────
def _tree(root: Path, pattern: str) -> dict[str, bytes]:
    return {p.relative_to(root).as_posix(): p.read_bytes()
            for p in sorted(root.rglob(pattern)) if p.is_file()}


def test_同一入力の2回の通しはバイト一致する(tmp_path: Path) -> None:
    """parse / draft / build / number の成果物は**バイト一致**が再現性の定義。

    文章化部分は G027（抽出元一致・必須語・文字数）が再現性の定義である ――
    ここでは決定的な埋め方をしているので、正本まで一致する。
    """
    outputs: list[dict[str, dict[str, bytes]]] = []
    for name in ("a", "b"):
        paths = _project(tmp_path, name)
        _auto(paths)
        _fill(paths)
        assert _auto(paths) == 0
        round_ = paths.latest_round()
        outputs.append({
            "parsed": _tree(round_.parsed, "*.md"),
            "organized": _tree(round_.organized, "*.yml"),
            "spec": _tree(paths.items, "*.yml") | _tree(paths.relations, "*.yml"),
            "out": _tree(paths.out, "*.md"),
        })

    first, second = outputs
    for section in ("parsed", "organized", "spec", "out"):
        assert first[section].keys() == second[section].keys(), section
        for key in first[section]:
            body = first[section][key]
            # 凍結日は日付なので除いて比べる（唯一の非決定要素）
            if key.endswith(".frozen.yml"):
                continue
            assert body == second[section][key], f"{section}/{key}"
