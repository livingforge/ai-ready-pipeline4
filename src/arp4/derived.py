"""derived 層 ―― **AI の解釈・要約**の置き場と、その機械検証（Phase 3）。

::

    parsed（機械の事実）→ organized（資料に忠実な記録）→ derived（AI の解釈）★

要約・抽象化・機能のグルーピングは「パース結果に無いことを書かない」という
organized の規律では書けない。**規律を緩めるのではなく、層を足す** ―― 事実の層と
解釈の層を混ぜなければ、どちらの規律も無傷で残る。

置き場は ``.arp/spec/derived/*.yml``。1 ファイル = アイテムの配列で、全アイテムに
2 つの欄が**必須**である。

``basis``
    根拠にした出典の列。organized 側のアンカー
    （``{round, file, anchor}`` ―― 正本の ``source`` と同じ形）か、
    正本のアイテム ID。**実在は機械が照合する**（D003 は error）。

``confidence``
    ``高`` / ``中`` / ``低``。**「資料に無い」と「AI が推定した」を混ぜない** ――
    未読取の申告と同じ構造を解釈層に適用したもので、読み手はこの欄で
    「どこまで信じてよいか」を判断する。

検出コード::

    D001  形が違う（配列でない・必須欄が無い）
    D002  basis が無い・空
    D003  basis / members の指す先が実在しない
    D004  confidence が無い・値が不正
    D005  id が重複している

乖離への向き合い方は organized と同じである ―― basis が指せない解釈は
「confidence を下げて残す」のではなく**error にして課題化**する（計画書の
リスク欄）。根拠の無い要約が正本の隣に置かれ続けるほうが、無いより悪い。
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any

from arp4 import mdio, yamlio
from arp4.finding import Finding, order
from arp4.paths import Paths
from arp4.spec import Spec

#: ``confidence`` に許される値。
CONFIDENCES = ("高", "中", "低")

#: derived の種別。語彙は固定しない（extensible ―― 解釈の形は資産ごとに違う）が、
#: stakeholder 向けの生成が**節として拾う**のはこの 3 つである。
GROUP = "機能グループ"
SUMMARY = "概要"
FLOW = "処理フロー"


@dataclass
class Derived:
    """読み込んだ derived 層。"""

    items: list[dict[str, Any]] = field(default_factory=list)
    #: アイテム → 置き場（指摘に載せる位置）。
    locations: dict[int, str] = field(default_factory=dict)


def directory(paths: Paths):
    return paths.spec / "derived"


def load(paths: Paths) -> tuple[Derived, list[Finding]]:
    """``derived/`` を読む。**形の違反はここで全部数える。**"""
    result = Derived()
    findings: list[Finding] = []
    for path in yamlio.scan(directory(paths)):
        location = path.relative_to(paths.root).as_posix()
        try:
            data = yamlio.load(path)
        except yamlio.YamlError as exc:
            findings.append(Finding("error", "D001", path.name,
                                    f"YAML として壊れています。{exc.detail}",
                                    file=location, line=exc.line))
            continue
        if data is None:
            continue
        if not isinstance(data, list):
            findings.append(Finding("error", "D001", path.name,
                                    "derived のファイルは配列でなければなりません",
                                    file=location))
            continue
        for index, item in enumerate(data):
            if not isinstance(item, dict):
                findings.append(Finding("error", "D001",
                                        f"{path.name}[{index}]",
                                        "derived のアイテムは連想配列で"
                                        "なければなりません", file=location))
                continue
            result.locations[len(result.items)] = location
            result.items.append(item)
    return result, findings


def check(spec: Spec, derived: Derived | None = None) -> list[Finding]:
    """derived 層の機械検証。**basis の実在とconfidence の宣言**を見る。

    解釈の中身（要約が妥当か）は機械には判定できない ―― 見るのは「根拠を
    指しているか」「指した先が実在するか」「確度を宣言しているか」だけである。
    organized の ``G004``（存在しない出典）と同じで、**幻覚の最頻形は
    存在しない根拠**なので、ここを機械で全件潰せることに価値がある。
    """
    if spec.paths is None:
        return []
    if derived is None:
        derived, findings = load(spec.paths)
    else:
        findings = []

    by_id = spec.by_id
    seen: dict[str, str] = {}
    anchors = _AnchorIndex(spec.paths)

    for index, item in enumerate(derived.items):
        location = derived.locations.get(index)
        item_id = str(item.get("id") or "")
        target = item_id or f"derived[{index}]"

        missing = [key for key in ("id", "type", "name", "statement")
                   if not item.get(key)]
        if missing:
            findings.append(Finding("error", "D001", target,
                                    f"必須の欄がありません: {'、'.join(missing)}",
                                    file=location))
        if item_id:
            if item_id in seen:
                findings.append(Finding("error", "D005", target,
                                        "id が重複しています", file=location))
            seen[item_id] = target

        confidence = str(item.get("confidence") or "")
        if confidence not in CONFIDENCES:
            findings.append(Finding(
                "error", "D004", target,
                f"confidence が不正です: {confidence or '(なし)'}"
                f"（{'、'.join(CONFIDENCES)} のどれか。"
                "「資料に無い」と「AI が推定した」を混ぜないための必須欄です）",
                file=location))

        basis = item.get("basis")
        if not isinstance(basis, list) or not basis:
            findings.append(Finding(
                "error", "D002", target,
                "basis がありません（根拠にした organized 側のアンカーか"
                "正本のアイテム ID を、1 件以上必ず書いてください）",
                file=location))
        else:
            for one in basis:
                trouble = _resolve(one, by_id, anchors)
                if trouble:
                    findings.append(Finding("error", "D003", target, trouble,
                                            file=location))

        for member in item.get("members") or []:
            if str(member) not in by_id:
                findings.append(Finding(
                    "error", "D003", target,
                    f"members の指す先が正本にありません: {member}",
                    file=location))
    return order(findings)


def _resolve(one: Any, by_id: dict[str, Any], anchors: "_AnchorIndex") -> str:
    """basis 1 件の実在。**指せない根拠は error**（confidence では逃がさない）。"""
    if isinstance(one, str):
        if one in by_id:
            return ""
        return f"basis の指すアイテムが正本にありません: {one}"
    if isinstance(one, dict):
        round_name = str(one.get("round") or "")
        file = str(one.get("file") or "")
        anchor = str(one.get("anchor") or "")
        if not (round_name and file and anchor):
            return ("basis は アイテム ID か {round, file, anchor} で"
                    f"書いてください: {one!r}")
        found = anchors.lookup(round_name, file)
        if found is None:
            return (f"basis の指すパース結果がありません: "
                    f"{round_name}/{file}")
        if anchor not in found:
            return (f"basis のアンカーがありません: "
                    f"{round_name}/{file}#{anchor}")
        return ""
    return f"basis の書式が不正です: {one!r}"


class _AnchorIndex:
    """ラウンドのパース結果のアンカー一覧。**1 ファイル 1 回しか読まない。**"""

    def __init__(self, paths: Paths) -> None:
        self._paths = paths
        self._cache: dict[tuple[str, str], set[str] | None] = {}

    def lookup(self, round_name: str, file: str) -> set[str] | None:
        key = (round_name, file)
        if key not in self._cache:
            path = self._paths.round(round_name).parsed / f"{file}{mdio.EXT}"
            self._cache[key] = ({a.id for a in mdio.read(path).anchors}
                                if path.is_file() else None)
        return self._cache[key]


def of_type(derived: Derived, type_name: str) -> list[dict[str, Any]]:
    return [item for item in derived.items
            if str(item.get("type") or "") == type_name]
