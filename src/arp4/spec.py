"""正本ストア（アイテム・関係）の読み書き。

置き場は :mod:`arp4.paths` が決める（``.arp/spec/``）。

**読み込みはディレクトリ内の全 YAML を対象にする**ので、ファイルの割り方は自由である
（種別ごと・工程ごとのどちらでもよい）。

書き戻しには 2 通りある。

``save``           種別ごとに割り直して書く。**ファイル構成が変わる**
``save_in_place``  読んだファイルへそのまま書き戻す。**変更のあったファイルだけ**

機械が正本を触るとき（採番など）は ``save_in_place`` を使う。人が書いた
ファイル構成とコメントを、関係のないファイルまで壊さないためである。
"""

from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Iterator

from arp4 import metamodel as mm
from arp4 import yamlio
from arp4.finding import Finding
from arp4.paths import Paths


@dataclass
class Spec:
    """読み込み済みの正本。"""

    metamodel: mm.Metamodel
    items: list[dict[str, Any]]
    relations: list[dict[str, Any]]
    paths: Paths | None = None
    #: 読み込み元（パス → そのファイルが持つレコード）。``items`` と同じ辞書を指す。
    item_files: list[tuple[Path, list[dict[str, Any]]]] = field(default_factory=list)
    relation_files: list[tuple[Path, list[dict[str, Any]]]] = field(default_factory=list)

    @property
    def by_id(self) -> dict[str, dict[str, Any]]:
        return {str(item.get("id")): item for item in self.items if item.get("id")}

    def of_type(self, type_name: str) -> Iterator[dict[str, Any]]:
        return (item for item in self.items if item.get("type") == type_name)

    def relations_of(self, type_name: str) -> Iterator[dict[str, Any]]:
        return (rel for rel in self.relations if rel.get("type") == type_name)

    def definition_of(self, item: dict[str, Any]) -> dict[str, Any]:
        return self.metamodel.item_types.get(str(item.get("type"))) or {}


def load(paths: Paths, packs_dir: Path | None = None) -> tuple[Spec, list[Finding]]:
    """正本一式を読む。**読めなかったファイルは error にして握り潰さない。**"""
    findings: list[Finding] = []

    model_path = yamlio.find(paths.spec, "metamodel")
    if model_path is None:
        raise FileNotFoundError(
            f"{paths.metamodel} がありません\n"
            f"  arp4 init で作成できます（extends: jp-sier-std を書いた雛形を置きます）")
    model, model_findings = mm.load(model_path, packs_dir=packs_dir)
    findings += model_findings

    item_files, item_findings = _read(paths.items, "アイテム")
    relation_files, relation_findings = _read(paths.relations, "関係")
    findings += item_findings + relation_findings

    return Spec(metamodel=model,
                items=[r for _, records in item_files for r in records],
                relations=[r for _, records in relation_files for r in records],
                paths=paths, item_files=item_files,
                relation_files=relation_files), findings


def _read(directory: Path,
          label: str) -> tuple[list[tuple[Path, list[dict[str, Any]]]], list[Finding]]:
    files: list[tuple[Path, list[dict[str, Any]]]] = []
    findings: list[Finding] = []
    for path in yamlio.scan(directory):
        data = yamlio.load(path)
        if data is None:
            continue
        if not isinstance(data, list):
            findings.append(Finding("error", "E000", path.name,
                                    f"{label}のファイルは配列でなければなりません"))
            continue
        records: list[dict[str, Any]] = []
        for index, record in enumerate(data):
            if not isinstance(record, dict):
                findings.append(Finding("error", "E000", f"{path.name}[{index}]",
                                        f"{label}は連想配列でなければなりません"))
                continue
            records.append(record)
        files.append((path, records))
    return files, findings


def display_config(paths: Paths | None) -> dict[str, Any]:
    """``.arp/display.yml``（表示 ID の接頭辞など）。無ければ空。"""
    if paths is None or not paths.display.is_file():
        return {}
    return yamlio.load(paths.display) or {}


# ── 書き戻し ────────────────────────────────────────────────────
#: 関係を指す鍵。``(関係型, 起点, 終点)``。
RelationKey = tuple[str, str, str]


def relation_key(relation: dict[str, Any]) -> RelationKey:
    return (str(relation.get("type") or ""), str(relation.get("from") or ""),
            str(relation.get("to") or ""))


def save_in_place(spec: Spec, item_ids: set[str] | None = None,
                  relation_keys: set[RelationKey] | None = None) -> list[Path]:
    """読んだファイルへ書き戻す。**変更のあったファイルだけ**。

    書き戻したファイルの**コメントは失われる**（YAML を読み書きで往復するため）。
    対象を絞るのはその被害を最小にするためなので、アイテムと関係は**別の鍵で**
    絞る ―― アイテムだけ触ったのに関係ファイルのコメントまで消してはならない。

    両方 ``None`` のときだけ全ファイルを書き戻す。
    """
    everything = item_ids is None and relation_keys is None
    written: list[Path] = []

    for path, records in spec.item_files:
        if not everything and not any(
                str(r.get("id") or "") in (item_ids or set()) for r in records):
            continue
        yamlio.dump(path, records)
        written.append(path)

    for path, records in spec.relation_files:
        if not everything and not any(
                relation_key(r) in (relation_keys or set()) for r in records):
            continue
        yamlio.dump(path, records)
        written.append(path)
    return written


def save(spec: Spec, spec_dir: Path | None = None) -> list[Path]:
    """種別ごとに割り直して書く。**ファイル構成が変わる**ので新規出力向け。"""
    target = spec_dir or (spec.paths.spec if spec.paths else None)
    if target is None:
        raise ValueError("書き出し先が決まっていません")

    written: list[Path] = []
    for section, records in (("items", spec.items), ("relations", spec.relations)):
        buckets: dict[str, list[dict[str, Any]]] = {}
        for record in records:
            buckets.setdefault(str(record.get("type") or "_unknown"), []).append(record)
        for type_name, bucket in sorted(buckets.items()):
            path = target / section / f"{type_name}{yamlio.EXT}"
            yamlio.dump(path, sorted(bucket, key=_sort_key))
            written.append(path)
    return written


def _sort_key(record: dict[str, Any]) -> tuple[str, str, str]:
    """**同じ内容からは同じ並び**にする（差分をノイズにしないため）。"""
    return (str(record.get("id") or ""), str(record.get("from") or ""),
            str(record.get("to") or ""))
