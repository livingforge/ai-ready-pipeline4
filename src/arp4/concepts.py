"""同一性の台帳（``.arp/spec/concepts.yml``）。**ラウンドをまたいで 1 本。**

``concept`` を item の ID と**同一視しない**のが肝である。同一視すると、あとで
「この 1 アイテムは 2 つに分けるべきだった」となったときに、**凍結済みの整理結果が
指す先が壊れる**。写像を正本が持てば、item を分割・統合しても凍結物は無傷のまま
でいられる。

副次的な効果として、機械のマージ判断が「**同じ concept なら同じアイテム**」だけに
なる。類似度計算も裁定台帳も要らない（3 の ``reconcile`` / ``ledger`` が消えた）。
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any

from arp4 import yamlio
from arp4.finding import Finding
from arp4.paths import Paths


@dataclass
class Concept:
    """同じものだと判断した、という記録 1 件。"""

    concept: str
    type: str = ""
    label: str = ""
    aliases: list[str] = field(default_factory=list)
    since: str = ""
    item: str = ""                       # 正本アイテムへの写像
    note: str = ""

    def to_dict(self) -> dict[str, Any]:
        record: dict[str, Any] = {"concept": self.concept}
        for key in ("type", "label"):
            if getattr(self, key):
                record[key] = getattr(self, key)
        if self.aliases:
            record["aliases"] = sorted(set(self.aliases))
        for key in ("since", "item", "note"):
            if getattr(self, key):
                record[key] = getattr(self, key)
        return record


def load(paths: Paths) -> tuple[dict[str, Concept], list[Finding]]:
    """台帳を読む。**無ければ空**（初回のラウンドはここから始まる）。"""
    findings: list[Finding] = []
    known: dict[str, Concept] = {}
    if not paths.concepts.is_file():
        return known, findings

    data = yamlio.load(paths.concepts)
    if data is None:
        return known, findings
    if not isinstance(data, list):
        return known, [Finding("error", "B000", paths.concepts.name,
                               "concepts.yml は配列でなければなりません")]

    for index, record in enumerate(data):
        if not isinstance(record, dict) or not record.get("concept"):
            findings.append(Finding("error", "B000", f"concepts.yml[{index}]",
                                    "concept がありません"))
            continue
        key = str(record["concept"])
        if key in known:
            findings.append(Finding("error", "B001", key, "concept が重複しています"))
            continue
        known[key] = Concept(
            concept=key, type=str(record.get("type") or ""),
            label=str(record.get("label") or ""),
            aliases=[str(a) for a in (record.get("aliases") or [])],
            since=str(record.get("since") or ""), item=str(record.get("item") or ""),
            note=str(record.get("note") or ""))
    return known, findings


def save(paths: Paths, known: dict[str, Concept]) -> None:
    """**並びは concept 順**（差分をノイズにしない）。"""
    yamlio.dump(paths.concepts,
                [known[key].to_dict() for key in sorted(known)])


def apply_proposal(known: dict[str, Concept], proposal: dict[str, Any],
                   round_name: str) -> tuple[list[str], list[Finding]]:
    """整理②の提案（``_concepts.yml``）を台帳へ重ねる。

    戻り値は ``(追加した concept, 検出)``。**既存の判断は上書きしない** ―― 別名の
    追加だけを受け入れる。同一性の取り消しは人が台帳を直す操作である。

    ``new`` は**再適用しても効く**。初回の ``build`` が使われた concept を台帳へ
    自動登録する（:func:`ensure`）ので、build をやり直しただけで ``new`` が全部
    「既にある」になり、**別名が反映されたのかどうかを台帳を grep して確かめる**
    羽目になっていた。既にある concept でも別名だけは重ねる。
    """
    findings: list[Finding] = []
    added: list[str] = []

    for entry in proposal.get("new") or []:
        if not isinstance(entry, dict) or not entry.get("concept"):
            findings.append(Finding("error", "B000", "_concepts.yml",
                                    f"new の書式が不正です: {entry!r}"))
            continue
        key = str(entry["concept"])
        if key in known:
            again = _again(known[key], entry, round_name)
            if again is not None:
                findings.append(again)
            continue
        known[key] = Concept(
            concept=key, type=str(entry.get("type") or ""),
            label=str(entry.get("label") or key),
            aliases=[str(a) for a in (entry.get("aliases") or [])],
            since=round_name, note=str(entry.get("reason") or ""))
        added.append(key)

    for entry in proposal.get("assign") or []:
        if not isinstance(entry, dict) or not entry.get("concept"):
            findings.append(Finding("error", "B000", "_concepts.yml",
                                    f"assign の書式が不正です: {entry!r}"))
            continue
        key = str(entry["concept"])
        if key not in known:
            findings.append(Finding("error", "B003", key,
                                    "assign の相手が台帳にありません"))
            continue
        known[key].aliases = sorted(set(known[key].aliases)
                                    | {str(a) for a in (entry.get("aliases_add") or [])})
    return added, findings


def _again(entry: Concept, proposed: dict[str, Any],
           round_name: str) -> Finding | None:
    """既にある concept が ``new`` に出たとき。**別名は重ね、理由を言い分ける。**

    **同じラウンドで登録した concept には何も言わない。** 凍結後は整理結果を
    編集できないので、``build`` をやり直すたびに「正常です」と書かれた warn が
    ``new`` の件数ぶん永久に出続けていた ―― 本物の warn を見落とす訓練になる。

    警告に値するのは「前ラウンドの台帳を見ずに ``new`` へ出した」ほうだけである
    （打ち手は整理②の直し）。
    """
    aliases = {str(a) for a in (proposed.get("aliases") or [])}
    fresh = sorted(aliases - set(entry.aliases))
    if fresh:
        entry.aliases = sorted(set(entry.aliases) | aliases)
        return Finding("warn", "B002", entry.concept,
                       f"既にある concept が new に出ています。別名だけ台帳に足しました: "
                       f"{'、'.join(fresh)}")
    if entry.since == round_name:
        return None                           # このラウンドで登録済み ―― 正常
    return Finding("warn", "B002", entry.concept,
                   f"{entry.since or '前'} のラウンドで登録済みです"
                   "（整理②が台帳を見ていない合図。足すものが無いので無視しました）")


def ensure(known: dict[str, Concept], concept: str, type_name: str, label: str,
           round_name: str) -> Concept:
    """レコードが使った concept を台帳に用意する（無ければ作る）。

    整理②を回していないラウンドでも ``build`` が通るようにするための保険であって、
    **横断整理の代わりではない**（別名も表記ゆれもここでは分からない）。
    """
    if concept not in known:
        known[concept] = Concept(concept=concept, type=type_name, label=label,
                                 since=round_name)
    return known[concept]


def check(known: dict[str, Concept], item_ids: set[str]) -> list[Finding]:
    """**逆向きの検査** ―― 台帳が指す先が正本に実在するか。

    ``concept`` を item の ID と同一視しないのがこの設計の肝で、そのぶん
    **写像が切れても誰も気づかない**という穴が空いていた。順方向（整理結果 →
    台帳）は ``G003`` が見ているが、逆（台帳 → 正本）はどこも見ていない。

    切れると何が起きるか。次のラウンドで同じ ``concept`` を書いたレコードは、
    台帳の写像が解決できないので**新しいアイテムとして起こされる** ―― 統合
    したはずのものが 2 つに割れ、しかも凍結物は無傷なので差分にも出ない。
    ``E029`` は「静かに割れる」手前で止めるためにある。

    ``W035``（写像が空）は error にしない。整理②が ``new`` で宣言した concept を
    そのラウンドのレコードが 1 件も使わなければこうなるが、**次のラウンドで
    使われる予定かもしれない** ―― 機械には決められない。
    """
    findings: list[Finding] = []
    for concept in sorted(known):
        entry = known[concept]
        if not entry.item:
            findings.append(Finding(
                "warn", "W035", concept,
                "台帳にありますが、正本のアイテムに写像されていません"
                "（このラウンドではどのレコードも使わなかった concept です）"))
        elif entry.item not in item_ids:
            findings.append(Finding(
                "error", "E029", concept,
                f"台帳が指すアイテムが正本にありません: {entry.item}"
                "（このまま次のラウンドを組むと、同じ concept が別のアイテムとして"
                "起こし直され、統合したはずのものが 2 つに割れます）"))
    return findings
