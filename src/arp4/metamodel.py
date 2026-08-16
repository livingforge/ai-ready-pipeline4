"""メタモデルの読み込み・継承の解決・メタモデル自身の検査。

3 で v2 から増えた宣言は 6 つ。すべて**ここで畳んでから**下流に渡すので、
検証や生成の側は「素の item_types / relation_types」だけを見ればよい。

===================  ==========================================================
``layers``           工程レイヤの一覧。全種別が必ずどれか 1 つに属する
``common_attributes``全アイテム共通の属性。種別側の宣言が優先（厳格化のみ可）
``item_groups``      ``from`` / ``to`` の列挙をまとめる別名
``relation_types[*].attributes``  関係が持つ属性（PK・入出力区分・CRUD）
``cardinality``      多重度。「列 0 本のテーブル」を error にできる
``pattern`` / ``multi``  属性値の正規表現／複数値の enum
===================  ==========================================================

継承（``extends``）で消費側にできるのは**追加と厳格化だけ**である。
緩和は M1xx で拒否する ―― 標準パックを配る意味がなくなるため。
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from arp4 import pack as pack_module
from arp4 import yamlio
from arp4.finding import Finding
from arp4.pack import PACKS_DIR, Pack

#: アイテムが持つ管理用のキー（メタモデルの属性ではない）。
#: ``overridden`` は「出典と異なる値にした理由」を持ち、``known_gaps`` は
#: 「資料に定義が無いことを承知している」を持つ（4 は再生成しないので
#: 保護ではなく**記録**である）。``conflicts`` は **``build`` が採らなかった値**
#: を持つ ―― 人は書かない（毎回の構築で書き直される）。build の warn は端末に
#: 流れて消えるので、残さないと衝突があったことが後から見えない。
ITEM_RESERVED = frozenset({"id", "type", "status", "source", "overridden",
                           "known_gaps", "conflicts"})

#: 関係が持つ管理用のキー。``order`` は ``ordered: true`` の並び順。
RELATION_RESERVED = frozenset({"type", "from", "to", "status", "source",
                               "description", "order", "overridden"})

#: ``status`` に許される値。
STATUSES = ("draft", "review", "approved", "deprecated")

#: 属性の ``kind`` に許される値。
KINDS = ("string", "int", "bool", "enum")

_CARDINALITY = re.compile(r"^(\d+)(?:\.\.(\d+|\*))?$")


@dataclass(frozen=True)
class Metamodel:
    """解決済みのメタモデル。``item_groups`` は展開済みで、参照する必要はない。"""

    version: int
    layers: tuple[str, ...]
    item_types: dict[str, dict[str, Any]]
    relation_types: dict[str, dict[str, Any]]
    groups: dict[str, tuple[str, ...]] = field(default_factory=dict)
    #: 属性名 → 設計書の列見出し（日本語）。生成側は見つからなければ属性名を出す。
    labels: dict[str, str] = field(default_factory=dict)
    #: ファクト種別 → ``(アイテム種別, 種別が決まれば入る属性)``。⑤の写像表。
    fact_types: dict[str, tuple[str, dict[str, Any]]] = field(default_factory=dict)
    extends: str | None = None
    #: 解決済みのパックチェーン（根 → 派生）。文書定義・準拠ルールの出所。
    chain: tuple[Pack, ...] = ()

    def label(self, key: str) -> str:
        return self.labels.get(key, key)

    def for_fact(self, fact_type: str) -> tuple[str, dict[str, Any]] | None:
        """ファクト種別を受ける種別と固定属性。**受け皿が無ければ None。**"""
        return self.fact_types.get(fact_type)

    def layer_of(self, type_name: str) -> str:
        return str((self.item_types.get(type_name) or {}).get("layer") or "")

    def types_in_layer(self, layer: str) -> list[str]:
        return [name for name, d in self.item_types.items() if d.get("layer") == layer]


# ── 読み込み ────────────────────────────────────────────────────
def load_pack(name: str, packs_dir: Path | None = None) -> dict[str, Any]:
    """同梱パックのメタモデルを素の辞書で読む。"""
    return _pack_metamodel(pack_module.find(name, packs_dir=packs_dir))


def _pack_metamodel(pack: Pack) -> dict[str, Any]:
    path = pack.metamodel_path
    if path is None:
        raise FileNotFoundError(f"パックのメタモデルがありません: {pack.dir}")
    return yamlio.load(path) or {}


def load(path: Path, packs_dir: Path | None = None) -> tuple[Metamodel, list[Finding]]:
    """メタモデルを 1 本読み、継承を解決して検査まで済ませる。"""
    raw = yamlio.load(path) or {}
    return resolve(raw, packs_dir=packs_dir)


def resolve(raw: dict[str, Any],
            packs_dir: Path | None = None) -> tuple[Metamodel, list[Finding]]:
    """継承・共通属性・グループを畳んで :class:`Metamodel` にする。

    返す findings には**継承の緩和（M1xx）とメタモデル自身の欠陥（M0xx）**が入る。
    """
    findings: list[Finding] = []
    extends = raw.get("extends")
    chain: list[Pack] = []
    merged: dict[str, Any] | None = None

    if extends:
        chain, chain_findings = pack_module.resolve_chain(str(extends),
                                                          packs_dir=packs_dir)
        findings += chain_findings
        # 根から順に重ねる。多段継承でも「追加と厳格化だけ」が各段で効く。
        for pack in chain:
            layer = _pack_metamodel(pack)
            if merged is None:
                merged = _copy(layer)
                continue
            merged, layer_findings = _merge(merged, layer)
            findings += layer_findings

    if merged is None:
        merged = _copy(raw)
    else:
        merged, overlay_findings = _merge(merged, raw)
        findings += overlay_findings

    model = _fold(merged, extends=str(extends) if extends else None,
                  chain=tuple(chain))
    findings += check(model)
    return model, findings


# ── 継承のマージ ────────────────────────────────────────────────
def _copy(value: Any) -> Any:
    if isinstance(value, dict):
        return {k: _copy(v) for k, v in value.items()}
    if isinstance(value, list):
        return [_copy(v) for v in value]
    return value


def _merge(base: dict[str, Any],
           overlay: dict[str, Any]) -> tuple[dict[str, Any], list[Finding]]:
    """標準パック + 消費側。**追加と厳格化だけ**を通す。"""
    findings: list[Finding] = []
    result = _copy(base)
    result["version"] = overlay.get("version", base.get("version"))

    # layers / item_groups / common_attributes は素直に追記・上書き。
    result["layers"] = list(base.get("layers") or []) + [
        layer for layer in (overlay.get("layers") or [])
        if layer not in (base.get("layers") or [])]

    groups = dict(base.get("item_groups") or {})
    groups.update(overlay.get("item_groups") or {})
    result["item_groups"] = groups

    labels = dict(base.get("labels") or {})
    labels.update(overlay.get("labels") or {})
    result["labels"] = labels

    common = dict(base.get("common_attributes") or {})
    for name, attr in (overlay.get("common_attributes") or {}).items():
        findings += _relaxation(common.get(name), attr, f"common_attributes.{name}")
        common[name] = {**(common.get(name) or {}), **(attr or {})}
    result["common_attributes"] = common

    for section, code in (("item_types", "M101"), ("relation_types", "M102")):
        merged = dict(base.get(section) or {})
        for name, definition in (overlay.get(section) or {}).items():
            if definition is None:
                if name in merged:
                    findings.append(Finding(
                        "error", code, name,
                        f"標準パックの {section} は削除できません（緩和の禁止）"))
                continue
            if name not in merged:
                merged[name] = _copy(definition)
                continue
            merged[name], type_findings = _merge_definition(
                merged[name], definition, name)
            findings += type_findings
        result[section] = merged

    return result, findings


def _merge_definition(base: dict[str, Any], overlay: dict[str, Any],
                      target: str) -> tuple[dict[str, Any], list[Finding]]:
    findings: list[Finding] = []
    result = {**_copy(base), **{k: _copy(v) for k, v in overlay.items()
                                if k != "attributes"}}

    for key, widened in (("from", "起点"), ("to", "終点")):
        if key in overlay and base.get(key):
            removed = [v for v in base[key] if v not in (overlay.get(key) or [])]
            if removed:
                findings.append(Finding(
                    "error", "M103", target,
                    f"{widened}の種別を削除できません: {'、'.join(map(str, removed))}"))
                result[key] = list(base[key]) + [
                    v for v in (overlay.get(key) or []) if v not in base[key]]

    attributes = dict(base.get("attributes") or {})
    for name, attr in (overlay.get("attributes") or {}).items():
        findings += _relaxation(attributes.get(name), attr, f"{target}.{name}")
        attributes[name] = {**(attributes.get(name) or {}), **(attr or {})}
    result["attributes"] = attributes
    return result, findings


def _relaxation(base: dict[str, Any] | None, overlay: dict[str, Any] | None,
                target: str) -> list[Finding]:
    """厳格化なら通し、緩和なら error にする。"""
    if not base or not overlay:
        return []
    findings: list[Finding] = []
    for flag, label in (("required", "必須"), ("unique", "一意")):
        if base.get(flag) and overlay.get(flag) is False:
            findings.append(Finding("error", "M110", target,
                                    f"{label}指定を外せません（緩和の禁止）"))
    if base.get("kind") == "enum" and "values" in overlay:
        removed = [v for v in (base.get("values") or [])
                   if v not in (overlay.get("values") or [])]
        if removed:
            findings.append(Finding(
                "error", "M111", target,
                f"enum の値を削除できません: {'、'.join(map(str, removed))}"))
    if base.get("extensible") is False and overlay.get("extensible"):
        findings.append(Finding("error", "M112", target,
                                "enum を extensible に緩められません"))
    return findings


# ── 畳み込み ────────────────────────────────────────────────────
def _fold(raw: dict[str, Any], extends: str | None,
          chain: tuple[Pack, ...] = ()) -> Metamodel:
    """``common_attributes`` を各種別へ、``item_groups`` を from/to へ展開する。"""
    groups = {str(k): tuple(str(m) for m in (v or []))
              for k, v in (raw.get("item_groups") or {}).items()}
    common = raw.get("common_attributes") or {}

    item_types: dict[str, dict[str, Any]] = {}
    fact_types: dict[str, tuple[str, dict[str, Any]]] = {}
    for name, definition in (raw.get("item_types") or {}).items():
        definition = _copy(definition or {})
        attributes = {k: _copy(v) or {} for k, v in common.items()}
        for attr_name, attr in (definition.get("attributes") or {}).items():
            # 種別側の宣言が勝つ（共通属性の厳格化）。
            attributes[attr_name] = {**(attributes.get(attr_name) or {}),
                                     **(_copy(attr) or {})}
        definition["attributes"] = attributes
        # fact_types は list（固定属性なし）でも dict でも書ける。dict に正規化する。
        declared = definition.get("fact_types")
        if isinstance(declared, list):
            declared = {str(fact): {} for fact in declared}
        declared = {str(k): dict(v or {}) for k, v in (declared or {}).items()}
        definition["fact_types"] = declared
        for fact_type, fixed in declared.items():
            fact_types[fact_type] = (str(name), fixed)
        item_types[str(name)] = definition

    relation_types: dict[str, dict[str, Any]] = {}
    for name, definition in (raw.get("relation_types") or {}).items():
        definition = _copy(definition or {})
        for key in ("from", "to"):
            if definition.get(key) is not None:
                definition[key] = _expand(definition[key], groups)
        relation_types[str(name)] = definition

    return Metamodel(
        version=int(raw.get("version") or 0),
        layers=tuple(str(layer) for layer in (raw.get("layers") or [])),
        item_types=item_types,
        relation_types=relation_types,
        groups=groups,
        labels={str(k): str(v) for k, v in (raw.get("labels") or {}).items()},
        fact_types=fact_types,
        extends=extends,
        chain=chain,
    )


def _expand(names: Any, groups: dict[str, tuple[str, ...]]) -> list[str]:
    """グループ名を種別名に開く。**順序を保ち、重複は落とす。**"""
    result: list[str] = []
    for name in (names if isinstance(names, list) else [names]):
        for resolved in groups.get(str(name), (str(name),)):
            if resolved not in result:
                result.append(resolved)
    return result


# ── メタモデル自身の検査 ────────────────────────────────────────
def check(model: Metamodel) -> list[Finding]:
    """30 種別・29 関係を人が書く以上、**メタモデル自身の誤字を機械で拾う。**"""
    findings: list[Finding] = []
    known = set(model.item_types)
    prefixes: dict[str, str] = {}
    claimed: dict[str, str] = {}

    for name in model.groups:
        if name in known:
            findings.append(Finding("error", "M002", name,
                                    "item_groups の名前が種別名と衝突しています"))
    for name, members in model.groups.items():
        for member in members:
            if member not in known:
                findings.append(Finding("error", "M003", f"item_groups.{name}",
                                        f"未知の種別です: {member}"))

    for name, definition in model.item_types.items():
        layer = definition.get("layer")
        if not layer:
            findings.append(Finding("error", "M001", name, "layer が宣言されていません"))
        elif layer not in model.layers:
            findings.append(Finding("error", "M001", name,
                                    f"layers に無い工程です: {layer}"))

        prefix = str(definition.get("id_prefix") or "")
        if prefix:
            if prefix in prefixes:
                findings.append(Finding("error", "M012", name,
                                        f"id_prefix が {prefixes[prefix]} と重複: {prefix}"))
            prefixes[prefix] = name

        findings += _check_attributes(name, definition.get("attributes") or {},
                                      reserved=ITEM_RESERVED)
        findings += _check_fact_types(name, definition, claimed)

        sequence = definition.get("sequence") or {}
        attributes = definition.get("attributes") or {}
        for key, code in (("attribute", "M004"), ("by", "M005"),
                          ("prefix_from", "M006")):
            value = sequence.get(key)
            if value and value not in attributes:
                findings.append(Finding("error", code, name,
                                        f"sequence.{key} が属性にありません: {value}"))

        upstream = definition.get("warn_if_no_upstream")
        if upstream:
            relation = model.relation_types.get(str(upstream))
            if relation is None:
                findings.append(Finding("error", "M007", name,
                                        f"未知の関係型です: {upstream}"))
            elif relation.get("from") and name not in relation["from"]:
                findings.append(Finding(
                    "error", "M007", name,
                    f"{upstream} の起点になれない種別に warn_if_no_upstream があります"))

    for name, definition in model.relation_types.items():
        for key in ("from", "to"):
            for type_name in definition.get(key) or []:
                if type_name not in known:
                    findings.append(Finding("error", "M003", name,
                                            f"{key} が未知の種別です: {type_name}"))
        cardinality = definition.get("cardinality") or {}
        for side, value in cardinality.items():
            if side not in ("from", "to"):
                findings.append(Finding("error", "M008", name,
                                        f"cardinality の辺が不正です: {side}"))
            elif not _CARDINALITY.match(str(value)):
                findings.append(Finding("error", "M008", name,
                                        f"多重度の書式が不正です: {value}"))
        findings += _check_attributes(name, definition.get("attributes") or {},
                                      reserved=RELATION_RESERVED)

    return findings


def _check_fact_types(target: str, definition: dict[str, Any],
                      claimed: dict[str, str]) -> list[Finding]:
    """⑤の写像表の妥当性。

    **メタモデルに種別を足したのに写像を足し忘れると、その種別は永遠に 0 件**になる。
    宣言をメタモデル 1 箇所に寄せたのはそれを機械で拾うためである。
    """
    findings: list[Finding] = []
    attributes = definition.get("attributes") or {}

    for fact_type, fixed in (definition.get("fact_types") or {}).items():
        owner = claimed.get(fact_type)
        if owner is not None:
            findings.append(Finding("error", "M020", target,
                                    f"ファクト種別が {owner} と重複しています: {fact_type}"))
        claimed[fact_type] = target

        for name, value in fixed.items():
            attribute = attributes.get(name)
            if attribute is None:
                findings.append(Finding(
                    "error", "M021", target,
                    f"fact_types.{fact_type} の固定属性が種別にありません: {name}"))
            elif (attribute.get("kind") == "enum" and not attribute.get("extensible")
                    and value not in (attribute.get("values") or [])):
                findings.append(Finding(
                    "error", "M021", target,
                    f"fact_types.{fact_type} の固定値が enum 外です: {name}={value}"))
    return findings


def _check_attributes(target: str, attributes: dict[str, Any],
                      reserved: frozenset[str]) -> list[Finding]:
    findings: list[Finding] = []
    for name, attr in attributes.items():
        attr = attr or {}
        if name in reserved:
            findings.append(Finding("error", "M009", target,
                                    f"予約キーは属性にできません: {name}"))
        kind = attr.get("kind")
        if kind not in KINDS:
            findings.append(Finding("error", "M010", target,
                                    f"{name} の kind が不正です: {kind}"))
        if kind == "enum" and not attr.get("values"):
            findings.append(Finding("error", "M010", target,
                                    f"{name} は enum なのに values がありません"))
        if attr.get("multi") and kind != "enum":
            findings.append(Finding("error", "M011", target,
                                    f"{name} の multi は enum にのみ付けられます"))
        pattern = attr.get("pattern")
        if pattern:
            if kind != "string":
                findings.append(Finding("error", "M011", target,
                                        f"{name} の pattern は string にのみ付けられます"))
            try:
                re.compile(str(pattern))
            except re.error as exc:
                findings.append(Finding("error", "M013", target,
                                        f"{name} の pattern が壊れています: {exc}"))
    return findings


def orient(definition: dict[str, Any], source_type: str,
           target_type: str) -> bool | None:
    """関係の向きが宣言に合うか。**凍結ゲートと構築で同じ判定を使う。**

    ``True`` は宣言どおり、``False`` は**反転すれば成立**（抽出は「自分は誰に
    属するか」の向きで出がちだが、メタモデルは構造を親 → 子で宣言している）、
    ``None`` は**どちらの向きでも成立しない**。

    ``from`` / ``to`` を宣言していない関係は「何でもよい」とみなす。
    """
    allowed_from = definition.get("from") or []
    allowed_to = definition.get("to") or []

    def fits(origin: str, target: str) -> bool:
        return ((not allowed_from or origin in allowed_from)
                and (not allowed_to or target in allowed_to))

    if fits(source_type, target_type):
        return True
    if fits(target_type, source_type):
        return False
    return None


def parse_cardinality(value: str) -> tuple[int, int | None]:
    """``"1"`` → ``(1, 1)`` / ``"1..*"`` → ``(1, None)`` / ``"0..2"`` → ``(0, 2)``。"""
    match = _CARDINALITY.match(str(value))
    if match is None:
        raise ValueError(f"多重度の書式が不正です: {value}")
    low = int(match.group(1))
    high_raw = match.group(2)
    if high_raw is None:
        return low, low
    return low, None if high_raw == "*" else int(high_raw)
