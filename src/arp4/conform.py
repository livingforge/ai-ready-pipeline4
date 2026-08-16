"""標準パックへの準拠検証（conform）。

3 段構えのうち**残り 2 段**をここが担う。

======  ==============================================================
L1      継承での**緩和の禁止**。``metamodel.resolve`` が M1xx として出す
        （必須の解除・enum 値の削除・標準種別の削除）
L2      パックの ``conformance/rules.yml``。**メタモデル（構造）では表せない
        工程の約束**を検査する。ここが本モジュール
lock    ``pack.lock`` との照合。``--frozen`` で error に上げる
======  ==============================================================

L2 が要るのは、日本の設計工程には「構造としては省略できるが、**この工程を
抜けるには埋まっていなければならない**」欄が多いためである。status に応じて
必須化する、という条件つきの規律はメタモデルの ``required`` では書けない。
"""

from __future__ import annotations

from pathlib import Path
from typing import Any

from arp4 import pack as pack_module
from arp4.finding import Finding, order
from arp4.spec import Spec


def conform(spec: Spec, spec_dir: Path | None = None, *,
            frozen: bool = False, baseline: bool = False) -> list[Finding]:
    """L2 と lock を検査する。**L1（M1xx）は正本の読み込み時に出ている。**"""
    chain = list(spec.metamodel.chain)
    findings: list[Finding] = []

    if chain:
        rules = pack_module.rules(chain)
        findings += _attribute_rules(spec, rules)
        findings += _trace_rules(spec, rules)
        findings += _require_documents(chain, rules)
        findings += _document_rules(spec, chain, rules)
        if baseline:
            findings += _baseline(spec, rules)

        target = spec_dir or (spec.paths.spec if spec.paths else None)
        if target is not None:
            findings += pack_module.verify_lock(target, chain, frozen=frozen)

    return order(findings)


def matches(item: dict[str, Any], where: Any) -> bool:
    """``where: {kind: 非機能}`` / ``{kind: [機能, 非機能]}`` の絞り込み。

    設計書生成（``publish``）と同じ意味で使う ―― **ルールと様式で
    絞り込みの解釈が食い違うと、検査を通った正本から違う設計書が出る。**

    ``{category: {not: [技術]}}`` は**残り全部**である。1 つの種別を複数の設計書へ
    振り分けるとき、白列挙だけだと ``extensible: true`` の enum に値が生えた
    瞬間に**どの設計書にも出ないものができる** ―― 実測（r001）で
    ``constraint.category`` は宣言に無い ``データ項目`` 14 件・``メッセージ``
    4 件・未設定 1 件を持っていた。最後の 1 つの節が「残り」を引き受ければ、
    振り分けは値が増えても全数を保つ。
    """
    for attribute, expected in (where or {}).items():
        value = item.get(attribute)
        if isinstance(expected, dict):
            excluded = expected.get("not")
            if value in (excluded if isinstance(excluded, list) else [excluded]):
                return False
            continue
        allowed = expected if isinstance(expected, list) else [expected]
        if value not in allowed:
            return False
    return True


# ── L2: 条件つきの必須化 ────────────────────────────────────────
def _attribute_rules(spec: Spec, rules: dict[str, Any]) -> list[Finding]:
    """``status`` に応じて属性を必須化する。

    例: 画面は review に上げる時点で説明が要る（承認後に埋めても手遅れ）。
    """
    findings: list[Finding] = []
    for rule in rules.get("attribute_rules") or []:
        type_name = str(rule.get("type") or "")
        attribute = str(rule.get("attribute") or "")
        level = str(rule.get("level") or "warn")
        statuses = [str(s) for s in (rule.get("when_status") or [])]
        for item in spec.of_type(type_name):
            if statuses and str(item.get("status")) not in statuses:
                continue
            if not matches(item, rule.get("where")):
                continue
            value = item.get(attribute)
            if value in (None, "", []):
                findings.append(Finding(
                    level, "C201", str(item.get("id")),
                    f"{attribute} は status={item.get('status')} では必須です"
                    f"（{type_name}）"))
    return findings


def _trace_rules(spec: Spec, rules: dict[str, Any]) -> list[Finding]:
    """トレースの欠落を、パックの判断で warn から error に上げる。

    メタモデルの ``warn_if_no_upstream`` は warn 固定である。
    「承認までに必ず埋める」かどうかは**様式標準の判断**なのでここで決める。
    """
    findings: list[Finding] = []
    for rule in rules.get("trace_rules") or []:
        type_name = str(rule.get("type") or "")
        relation = str(rule.get("relation") or "")
        direction = str(rule.get("direction") or "from")
        level = str(rule.get("level") or "warn")
        statuses = [str(s) for s in (rule.get("when_status") or [])]

        connected = {str(r.get(direction))
                     for r in spec.relations_of(relation) if r.get(direction)}
        for item in spec.of_type(type_name):
            if statuses and str(item.get("status")) not in statuses:
                continue
            if not matches(item, rule.get("where")):
                continue
            if str(item.get("id")) in connected:
                continue
            side = "上流" if direction == "from" else "下流"
            findings.append(Finding(
                level, "C202", str(item.get("id")),
                f"{side}に繋がっていません（{relation} が 1 本もありません）"))
    return findings


def _require_documents(chain: list[pack_module.Pack],
                       rules: dict[str, Any]) -> list[Finding]:
    """様式標準が「これは必ず出せること」と定めた文書があるか。"""
    required = [str(name) for name in (rules.get("require_documents") or [])]
    if not required:
        return []
    available = {str(d.get("name")) for d in pack_module.documents(chain)}
    return [Finding("error", "C204", name, "様式標準が要求する文書定義がありません")
            for name in required if name not in available]


def _document_rules(spec: Spec, chain: list[pack_module.Pack],
                    rules: dict[str, Any]) -> list[Finding]:
    """**``group_by`` で節を割りすぎていないか。**

    割った数が中身の数の半分を超えたら（＝ 1 節あたり 2 件未満）、それは分類では
    なく**見出しの水増し**である。表は比べるための道具なので、1 行の表が並ぶと
    比べる相手の無い表が並ぶだけになり、目次は「この文書に何があるか」ではなく
    分類の一覧に変わる。

    **これは文書を知っている層でしか判定できない。** 分類が細かいこと自体は悪では
    なく（``nf_category`` は 6 値ある）、悪いのは**その分類で割った節が痩せる**こと
    である。凍結（``freeze``）は文書定義を見ないので、節を数えられない。

    実測で 2 回とも人が目視で見つけている ―― 詳細設計書の「呼出関係」16 節・目次
    58 行（決定 18）と、要件定義書の「機能要件」31 件 17 節（決定 19）。**同じ間違い
    を 2 回して 2 回とも目で見つけた**ので、機械が言えるようにする。
    """
    findings: list[Finding] = []
    for rule in rules.get("document_rules") or []:
        floor = float(rule.get("min_rows_per_section") or 2)
        least = int(rule.get("when_rows_at_least") or 6)
        level = str(rule.get("level") or "warn")
        for definition in pack_module.documents(chain):
            title = str(definition.get("title") or definition.get("name") or "")
            for section in definition.get("sections") or []:
                attribute = str(section.get("group_by") or "")
                type_name = str(section.get("type") or "")
                if not attribute or not type_name or str(
                        section.get("kind") or "items") != "items":
                    continue                    # 関係の章の group_by: from は属性でない
                # 絞り込みは publish と同じ ``matches`` を使う ―― ここが食い違うと、
                # 検査が見ている表と生成される表が別のものになる。
                rows = [i for i in spec.of_type(type_name)
                        if i.get("status") != "deprecated"
                        and matches(i, section.get("where"))]
                buckets = {str(r.get(attribute) or "未分類") for r in rows}
                if len(rows) < least or len(buckets) < 2:
                    continue
                if len(rows) / len(buckets) >= floor:
                    continue
                findings.append(Finding(
                    level, "C205", f"{title}「{section.get('heading')}」",
                    f"{attribute} で {len(buckets)} 節に割ると 1 節あたり "
                    f"{len(rows) / len(buckets):.1f} 件です（{len(rows)} 件）"
                    f"。分類ではなく見出しを増やしただけになっています"))
    return findings


def _baseline(spec: Spec, rules: dict[str, Any]) -> list[Finding]:
    """ベースライン（版として締める）前提の検査。**明示したときだけ回す。**"""
    required = str((rules.get("status_rules") or {}).get("baseline_requires") or "")
    if not required:
        return []

    findings: list[Finding] = []
    for item in spec.items:
        if str(item.get("status")) != required:
            findings.append(Finding(
                "error", "C203", str(item.get("id")),
                f"ベースラインには status={required} が必要です"
                f"（現在 {item.get('status')}）"))
    for relation in spec.relations:
        status = str(relation.get("status") or "")
        if status and status != required:
            findings.append(Finding(
                "error", "C203",
                f"{relation.get('type')} {relation.get('from')}→{relation.get('to')}",
                f"ベースラインには status={required} が必要です（現在 {status}）"))
    return findings
