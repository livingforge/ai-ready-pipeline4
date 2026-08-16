"""③ 整理結果 → 正本。**意味の判断を含まない。**

やることは 4 つだけである。

* **マージ** ―― 同じ ``concept`` のレコードを 1 アイテムに畳む（本文は完全な
  レコードからだけ。``multi: true`` の属性は**和集合**にする）
* **採番** ―― アイテム ID を concept から決める（表示 ID は :mod:`arp4.sequence`）
* **向きの補正** ―― メタモデルは構造を親 → 子で宣言している。逆向きで成立するなら
  そちらを採る（言い回しの違いでトレースを落とさない）
* **出典の引き継ぎ** ―― ラウンド・ファイル・アンカーをそのまま運ぶ。整理層が
  書いた ``known_gaps``（「調べたうえで相手がいない」）も同じ欄へ運ぶ
  （→ :func:`_carry_gaps`。**正本側の宣言は上書きしない**）

やらないことも 3 つ。**推測しない**（関係は ``refs`` からだけ）、**必須属性を
埋めない**（``E010`` は「人が埋めよ」の正しい信号）、**承認しない**（すべて
``status: review``）。

3 の ``adopt`` との最大の違いは、**同一性を自分で判断しないこと**である。名寄せは
整理②が済ませて ``concept`` として書いてあるので、ここは「同じ concept なら同じ
アイテム」を実行するだけでよい ―― 類似度計算も裁定台帳も無い。
"""

from __future__ import annotations

import hashlib
from dataclasses import dataclass, field
from typing import Any

from arp4 import decisions
from arp4 import gaps
from arp4 import metamodel as mm
from arp4 import mdio
from arp4 import override
from arp4 import yamlio
from arp4 import spec as spec_module
from arp4.concepts import Concept, ensure
from arp4.finding import Finding, order
from arp4.freeze import IMPORT_ANCHOR, digest   # 課題 ID の元（凍結ハッシュと同じ関数）
from arp4.organized import Organized, Record
from arp4.spec import Spec


@dataclass
class Plan:
    """構築案。**正本は書き換えない**（適用は :func:`apply`）。"""

    created: list[dict[str, Any]] = field(default_factory=list)
    updated: list[tuple[dict[str, Any], dict[str, Any]]] = field(default_factory=list)
    relations: list[dict[str, Any]] = field(default_factory=list)
    relation_updates: list[tuple[dict[str, Any], dict[str, Any]]] = \
        field(default_factory=list)
    protected: dict[str, list[str]] = field(default_factory=dict)
    concepts: dict[str, Concept] = field(default_factory=dict)
    findings: list[Finding] = field(default_factory=list)
    #: 機械が下した判断（``decisions.yml`` へ追記する）。**端末に流れて消える
    #: warn と違い、決定記録から出典アンカーへ辿れる**（→ :mod:`arp4.decisions`）。
    logged: list[dict[str, Any]] = field(default_factory=list)
    metrics: dict[str, int] = field(default_factory=dict)
    #: concept → アイテム ID。**まだ正本に書いていないもの**を含む。
    item_of: dict[str, str] = field(default_factory=dict)
    #: アイテム ID → このラウンドで組み立てたアイテム（種別を引くのに使う）。
    derived: dict[str, dict[str, Any]] = field(default_factory=dict)

    @property
    def empty(self) -> bool:
        return not (self.created or self.updated or self.relations
                    or self.relation_updates)

    def type_of(self, item_id: str) -> str:
        return str((self.derived.get(item_id) or {}).get("type") or "")


def plan(spec: Spec, result: Organized, known: dict[str, Concept],
         round_name: str) -> Plan:
    """整理結果から構築案を作る。**冪等**（2 回作っても同じ案になる）。"""
    out = Plan(concepts=known)
    model = spec.metamodel

    groups: dict[str, list[Record]] = {}
    for record in result.records:
        groups.setdefault(record.concept, []).append(record)

    derived_items = out.derived               # アイテム ID → 組み立てたアイテム
    item_of = out.item_of                     # concept → アイテム ID

    for concept in sorted(groups):
        group = groups[concept]
        # **concept を定義するのは完全なレコードだけ。** 参照だけのレコードは
        # アンカーを解決し関係を張るためにいる（→ :class:`arp4.organized.Record`）。
        complete = [r for r in group if r.complete]
        existing = known.get(concept)
        # 参照だけのレコードは種別を名乗らないので、台帳（＝前ラウンドの成果）から引く。
        fact_type = complete[0].type if complete else (
            existing.type if existing else "")
        if not fact_type:
            # 凍結ゲート（G013）が通っていれば起きない。
            out.findings.append(Finding(
                "error", "B014", group[0].target,
                f"参照だけのレコードしかなく、種別が決まりません: {concept}"))
            continue

        mapped = model.for_fact(fact_type)
        if mapped is None:
            # 凍結ゲート（G002）が通っていれば起きない。通さずに build した場合の保険。
            out.findings.append(Finding("error", "B010", group[0].target,
                                        f"受け皿の無い種別です: {fact_type}"))
            continue
        type_name, fixed = mapped
        definition = model.item_types.get(type_name) or {}

        label = complete[0].name if complete else ""
        item_id = (existing.item if existing and existing.item
                   else _item_id(definition, concept))
        previous = spec.by_id.get(item_id)
        if not complete and previous is None:
            # 台帳は種別を知っているのにアイテムが無い（台帳と正本のずれ）。
            # 名前も本文も無いアイテムを起こすと E010 が出るだけなので、ここで止める。
            out.findings.append(Finding(
                "error", "B014", group[0].target,
                f"参照だけのレコードしかなく、正本にもアイテムがありません: {concept}"))
            continue

        entry = ensure(known, concept, fact_type, label, round_name)
        entry.item = item_id
        entry.type = entry.type or fact_type
        entry.label = entry.label or label
        item_of[concept] = item_id

        item = _item(group, complete, item_id, type_name, fixed, definition,
                     round_name, out.findings)
        derived_items[item_id] = item

        merged, kept = override.merge_item(previous, item, definition)
        if kept:
            out.protected[item_id] = kept
        out.findings += _carry_gaps(merged, group, item_id)
        if previous is None:
            out.created.append(merged)
        elif merged != previous:
            out.updated.append((previous, merged))

    out.findings += _relations(spec, groups, item_of, derived_items, out)

    out.metrics = {
        "concepts": len(groups),
        "created": len(out.created),
        "updated": len(out.updated),
        "relations": len(out.relations),
        "relation_updates": len(out.relation_updates),
        "protected": sum(len(v) for v in out.protected.values()),
    }
    out.findings = order(out.findings)
    return out


def apply(spec: Spec, result: Plan) -> tuple[set[str], set[tuple[str, str, str]]]:
    """構築案を正本へ書き込む。戻り値は**触れたアイテム ID と関係の鍵**。"""
    items: set[str] = set()
    relations: set[tuple[str, str, str]] = set()

    for previous, merged in result.updated:
        previous.clear()
        previous.update(merged)               # 同じ辞書を書き換える（ファイル位置を保つ）
        items.add(str(merged.get("id")))
    for item in result.created:
        _place(spec, item, item=True)
        items.add(str(item.get("id")))

    for previous, merged in result.relation_updates:
        previous.clear()
        previous.update(merged)
        relations.add(spec_module.relation_key(merged))
    for relation in result.relations:
        _place(spec, relation, item=False)
        relations.add(spec_module.relation_key(relation))
    return items, relations


def apply_issues(spec: Spec, found: Issues
                 ) -> tuple[set[str], set[tuple[str, str, str]]]:
    """矛盾から起こした課題と ``disputes`` を正本へ書き込む。"""
    items: set[str] = set()
    relations: set[tuple[str, str, str]] = set()
    existing = {spec_module.relation_key(r) for r in spec.relations}

    for issue in found.items:
        _place(spec, issue, item=True)
        items.add(str(issue["id"]))
    for relation in found.relations:
        key = spec_module.relation_key(relation)
        if key in existing:
            continue
        _place(spec, relation, item=False)
        existing.add(key)
        relations.add(key)
    return items, relations


# ── アイテム ────────────────────────────────────────────────────
def _item_id(definition: dict[str, Any], concept: str) -> str:
    """**concept 由来**。ラウンドが増えても、実行順が変わっても動かない。"""
    prefix = str(definition.get("id_prefix") or "itm")
    digest = hashlib.sha256(concept.encode("utf-8")).hexdigest()[:12]
    return f"{prefix}-{digest}"


def _item(group: list[Record], complete: list[Record], item_id: str, type_name: str,
          fixed: dict[str, Any], definition: dict[str, Any], round_name: str,
          findings: list[Finding]) -> dict[str, Any]:
    """同じ concept のレコード群から 1 アイテムを組み立てる。

    ``name`` / ``statement`` は**完全なレコードからだけ**採る。``source`` は
    参照だけのレコードも含めた全件を運ぶ ―― アンカーはどれも本物の出典である。
    """
    attributes = definition.get("attributes") or {}

    derived: dict[str, Any] = {
        "id": item_id,
        "type": type_name,
        "status": "review",                   # approved に上げるのは人だけ
        "source": [{"round": round_name, "file": r.file, "anchor": r.anchor}
                   for r in group],
    }

    if complete:
        # 本文は**最も情報量の多いもの**を採る（要約は作らない）。文字数は情報量の
        # 代理指標として弱いので、**採らなかった本文は B023 で必ず見せる** ―― 黙って
        # 落とすと「なぜ画面一覧の説明が機能一覧の文に置き換わったのか」が読めない。
        ordered = sorted(complete, key=lambda r: (-len(r.statement), r.file, r.index))
        statement = ordered[0].statement
        dropped = [r for r in ordered[1:] if r.statement != statement]
        if dropped:
            findings.append(Finding(
                "warn", "B023", item_id,
                f"同じ concept で statement が食い違います。長いほうを採りました: "
                f"{statement!r} ← "
                f"{'、'.join(f'{r.statement!r}（{r.target}）' for r in dropped[:3])}"
                + ("…" if len(dropped) > 3 else "")
                + "。1 シートだけ完全なレコードにして、残りを参照だけの"
                  "レコードにすれば起きません"))
            # **採らなかった本文も正本に残す。** statement の食い違いは補足と違って
            # 矛盾なので足し合わせられない ―― どちらが正かは意味の判断であって、
            # 正しい行き先は課題（disputes）である。捨てた値を残しておけば、
            # `W045` が言い続けるので次のラウンドで拾い直せる。
            for dropped_record in dropped:
                _record_conflict(derived, "statement", dropped_record.statement,
                                 dropped_record)
        name = complete[0].name
        derived["name"] = name
        derived["statement"] = statement
        for record in complete:
            if record.name != name:
                findings.append(Finding(
                    "warn", "B020", item_id,
                    f"同じ concept で名前が食い違います: {name!r} / {record.name!r}"
                    f"（{record.target}）"))

    # 属性は**参照だけのレコードからも拾う**（そのシートにしか無い桁数がある）。
    for record in group:
        for key, value in record.attrs.items():
            attribute = attributes.get(key)
            if attribute is None:
                findings.append(Finding(
                    "warn", "B021", record.target,
                    f"{type_name} に無い属性です: {key}（捨てました）"))
                continue
            current = derived.get(key)
            if attribute.get("multi"):
                derived[key] = _union(attribute, current, value)
            elif current in (None, "", []):
                derived[key] = value
            elif current == value:
                pass
            elif attribute.get("merge") == "append":
                # **両方残す。** 相補的な補足は、片方を捨てると事実そのものが消える
                # ―― 実測で権限マトリクスの △（部長職のみ可）がこれで正本から
                # 消えていた。失っていないので B022 は鳴らさない。
                derived[key] = _append(current, value)
            else:
                # **捨てた値を正本に残す。** build の warn は端末に流れて消えるので、
                # 次に `check` を回した人には衝突があったことが見えない ―― 残せば
                # `W045` が言い続け、穴の一覧にも出る（→ 決定 70）。
                findings.append(Finding(
                    "warn", "B022", item_id,
                    f"統合したレコードで {key} が食い違います: "
                    f"{current!r} / {value!r}（{record.target}）"))
                _record_conflict(derived, key, value, record)

    derived.update({k: v for k, v in fixed.items() if k in attributes})
    # **必須属性は埋めない。** 埋めると「人が直しても次の構築で戻る」に戻る。
    return derived


def _carry_gaps(merged: dict[str, Any], group: list[Record],
                item_id: str) -> list[Finding]:
    """整理層が書いた ``known_gaps`` を正本へ引き継ぐ。**正本側を上書きしない。**

    宣言の置き場は正本（:mod:`arp4.gaps`）だが、そこは ``build`` を打った人の欄で
    ある ―― **分担しているとき配る側は ``build`` を禁じる**（凍結済みのラウンドを
    勝手に組み立てさせない）ので、整理層には「調べたうえで相手がいない」と言う
    場所が無かった。ここはレコード側の宣言を**同じ欄へ運ぶだけ**で、意味の判断も
    書式の翻訳もしない（形は ``schemas/organized.yml`` と :mod:`arp4.gaps` で
    同じ ``{reason, at}``）。

    ``known_gaps`` は :data:`arp4.override.ALWAYS_PROTECTED` なので
    :func:`arp4.override.merge_item` は**正本のものをそのまま残す** ―― つまり
    人が凍結後に書いた宣言は消えない。ここが足すのは**正本にまだ無い名前だけ**
    である（:func:`arp4.auto.declare_gaps` の「人が既に宣言している（上書き
    しない）」と同じ規律 ―― 重複させず、後から来た機械が人の理由を書き換えない）。

    同じ concept の 2 つのレコードが同じ名前を別の理由で宣言したら、**先に読んだ
    ほうを採って ``B027`` で言う**（``B022`` / ``B024`` と同じ扱い ―― どちらが正か
    は意味の判断なので機械が決めない）。
    """
    findings: list[Finding] = []
    declared: dict[str, dict[str, Any]] = {}
    for record in group:
        for name, entry in record.known_gaps.items():
            found = declared.get(name)
            if found is None:
                declared[name] = dict(entry)
                continue
            if str(found.get("reason") or "") != str(entry.get("reason") or ""):
                findings.append(Finding(
                    "warn", "B027", item_id,
                    f"同じ concept で known_gaps.{name} の理由が食い違います: "
                    f"{found.get('reason')!r} / {entry.get('reason')!r}"
                    f"（{record.target}）。先に読んだほうを採りました"))
    if not declared:
        return findings

    current = merged.get(gaps.KEY)
    current = dict(current) if isinstance(current, dict) else {}
    # **正本にある宣言はそのまま。** 凍結後に人が書いた理由を、整理結果の側から
    # 上書きすると「なぜ理由が変わったのか」がどこにも残らない。
    merged[gaps.KEY] = {**declared, **current}
    return findings


#: 追記する属性の区切り。**改行 2 つ**にする ―― 1 つだと Markdown の表のセルで
#: 前の文と繋がって読め、区切ったことが読み手に伝わらない。
APPEND_SEPARATOR = "\n\n"

#: 捨てた値の記録（アイテムが持つキー。メタモデルの属性ではない）。
#: ``overridden`` が「出典と違う値にした理由」を持つのと同じ位置づけで、
#: こちらは**機械が捨てたこと**を持つ ―― 人は書かない（build が毎回書き直す）。
CONFLICTS = "conflicts"


def _append(current: Any, value: Any) -> str:
    """追記する。**同じ文を 2 度足さない**（参照だけのレコードが同文を持つ）。"""
    left, right = str(current), str(value)
    if right in left:
        return left
    return left + APPEND_SEPARATOR + right


def _record_conflict(derived: dict[str, Any], key: str, value: Any,
                     record: Any) -> None:
    """採らなかった値を ``conflicts`` に積む。**並びは決定的**（出典順）。"""
    bucket = derived.setdefault(CONFLICTS, {})
    entries = bucket.setdefault(key, [])
    where = {"file": record.file, "anchor": record.anchor}
    for entry in entries:
        if entry.get("value") == value and entry.get("source") == where:
            return
    entries.append({"value": value, "source": where})
    entries.sort(key=lambda e: (str(e["source"]["file"]),
                                str(e["source"]["anchor"]), str(e["value"])))


def _listed(value: Any) -> list[Any]:
    if value in (None, "", []):
        return []
    return list(value) if isinstance(value, list) else [value]


def _union(attribute: dict[str, Any], current: Any, value: Any) -> list[Any]:
    """``multi: true`` の値集合を**和集合**にする。

    先勝ちにしていた頃は、モジュール一覧の ``crud: [C]`` が処理仕様書の
    ``crud: [C, R]`` を押しのけ、**CRUD 図から R が消えた**。値集合が食い違うのは
    「片方が詳しい」だけで矛盾ではないので、報告する必要も無い（落ちるものが無い）。

    並びは enum の宣言順に揃える ―― 資料の読み順で ``[R, C]`` と ``[C, R]`` が
    混ざると、意味が同じ差分が延々と出る。
    """
    merged: list[Any] = []
    for item in _listed(current) + _listed(value):
        if item not in merged:
            merged.append(item)
    declared = list(attribute.get("values") or [])
    return sorted(merged, key=lambda v: (declared.index(v) if v in declared
                                         else len(declared), merged.index(v)))


# ── 関係 ────────────────────────────────────────────────────────
def _target_of(concept: str, item_of: dict[str, str], out: Plan,
               spec: Spec) -> str | None:
    """関係の相手のアイテム ID。**このラウンド → 台帳**の順に引く。

    ``item_of`` はこのラウンドのレコードから作った辞書なので、これだけを見ると
    **前のラウンドで確立した concept を指す関係が落ちる**。更新の無い資料の
    レコードが前のラウンドの整理結果を指し続けるのは正常な状態（→ freeze の
    ラウンドの節）なので、ラウンドが増えるほど落ちる関係が増えていた ――
    しかも ``B012`` の warn 1 行なので、トレースの穴として静かに残る。

    台帳が指すアイテムが**正本に実在するときだけ**通す。台帳と正本がずれている
    （アイテムを消した）なら、それは相手が無いのと同じで ``B012`` が正しい。
    """
    found = item_of.get(concept)
    if found is not None:
        return found
    entry = out.concepts.get(concept)
    if entry and entry.item and entry.item in spec.by_id:
        return entry.item
    return None


def _relations(spec: Spec, groups: dict[str, list[Record]], item_of: dict[str, str],
               derived_items: dict[str, dict[str, Any]], out: Plan) -> list[Finding]:
    """``refs`` を関係にする。**相手は concept で名指しされている**ので探さない。

    同じ関係が複数の資料から張られたら、**属性はマージする**。以前は 2 本目以降を
    黙って捨てていたので、テーブル定義書の ``has-column`` が持つ ``pk: true`` が、
    項目一覧から張った同じ関係の処理順によっては消えていた ―― **整理層からは
    予測も検知もできない**取りこぼしだった。食い違いは ``B024`` で報告する。
    """
    findings: list[Finding] = []
    model = spec.metamodel
    counters: dict[tuple[str, str], int] = {}
    existing = {spec_module.relation_key(r): r for r in spec.relations}
    built: dict[tuple[str, str, str], tuple[dict[str, Any], dict[str, Any]]] = {}
    #: 向きを機械が決められなかった組み合わせ → **関係 1 本ごと**の記録
    #: （警告は B026 でまとめて出すが、決定は 1 本ずつ積む → :func:`_unsure`）。
    #: 内側を ``(from_id, to_id)`` で畳むのは、同じ関係を 2 つのレコードが書いても
    #: 正本に入るのは 1 本だからである ―― ref ごとに積むと決定の件数が
    #: **正本の関係の数を超える。**
    unsure: dict[tuple[str, str, str], dict[tuple[str, str], _Unsure]] = {}

    for concept in sorted(groups):
        source_id = item_of.get(concept)
        if source_id is None:
            continue
        for record in groups[concept]:
            for ref in record.refs:
                definition = model.relation_types.get(ref.rel)
                if definition is None:
                    findings.append(Finding("error", "B011", record.target,
                                            f"語彙に無い関係です: {ref.rel}"))
                    continue
                target_id = _target_of(ref.to, item_of, out, spec)
                if target_id is None:
                    findings.append(Finding(
                        "warn", "B012", record.target,
                        f"関係の相手が正本にありません: {ref.to}"
                        "（相手のレコードがこのラウンドに無い可能性があります）"))
                    continue

                resolved = _direction(ref.rel, definition, source_id, target_id,
                                      derived_items, spec)
                if resolved is None:
                    findings.append(Finding(
                        "error", "B013", record.target,
                        f"{ref.rel} の向きが宣言と合いません"
                        f"（{source_id} → {target_id}）"))
                    continue
                from_id, to_id, ambiguous = resolved
                if ambiguous and not _oriented_by_source(ref, record):
                    unsure.setdefault(ambiguous, {}).setdefault(
                        (from_id, to_id),
                        _Unsure(record.target, _basis(record),
                                _name_of(from_id, derived_items, spec),
                                _name_of(to_id, derived_items, spec)))
                key = (ref.rel, from_id, to_id)

                found = built.get(key)
                if found is None:
                    built[key] = (definition,
                                  _relation(ref, definition, from_id, to_id, counters))
                    continue
                findings += _merge_attrs(found[1], ref, definition, record, out)

    findings += _unsure(unsure, out)

    for key, (definition, relation) in built.items():
        previous = existing.get(key)
        if previous is None:
            out.relations.append(relation)
            continue
        merged, _ = override.merge_item(previous, relation, definition)
        if merged != previous:
            out.relation_updates.append((previous, merged))
    return findings


def _basis(record: Record) -> str:
    """決定記録の根拠にする出典アンカー（``<パース結果>#<アンカー>``）。

    :mod:`arp4.draft` の ``_basis`` と同じ形にしてある ―― 決定記録は 1 枚の表で、
    主体ごとに根拠の書式が違うと読み手が辿り方を切り替えることになる。
    """
    return f"{record.file}{mdio.EXT}#{record.anchor}"


def _merge_attrs(relation: dict[str, Any], ref, definition: dict[str, Any],
                 record: Record, out: Plan) -> list[Finding]:
    """2 本目以降の ``refs`` が持つ属性を重ねる。**空欄は埋め、食い違いは報告する。**

    ``multi: true``（CRUD・権限）は**和集合**にする。ここが先勝ちだったせいで、
    モジュール一覧の ``crud: [C]`` が処理仕様書の ``crud: [C, R]`` を押しのけ、
    CRUD 図から ``R`` が消えていた（→ :func:`_union`）。

    食い違って**採らなかったほうがあるとき**は、``B024`` を出すだけでなく
    ``decisions`` にも積む ―― warn は端末に流れて消えるので、次に ``check`` を
    回した人には「先に読んだほうを採った」ことが見えない（実測・sales-corpus r001 で
    11 件がそうなった）。
    """
    findings: list[Finding] = []
    target = record.target
    # 本文は**足し合わせる**（アイテムの description と同じ理屈 ―― 別の資料が別の
    # ことを書いているなら、片方を捨てると事実が消える）。
    if ref.note:
        current = str(relation.get("description") or "")
        relation["description"] = _append(current, ref.note) if current else ref.note
    declared = definition.get("attributes") or {}
    for name, value in ref.attrs.items():
        attribute = declared.get(name)
        if attribute is None:
            continue                          # 宣言に無い属性は _relation と同じく捨てる
        current = relation.get(name)
        if attribute.get("multi"):
            relation[name] = _union(attribute, current, value)
        elif current in (None, "", []):
            relation[name] = value
        elif current != value:
            findings.append(Finding(
                "warn", "B024", f"{ref.rel} {relation['from']}→{relation['to']}",
                f"別の資料が張った同じ関係で {name} が食い違います: "
                f"{current!r} / {value!r}（{target}）。先に読んだほうを採りました"))
            out.logged.append(decisions.entry(
                "build",
                f"{ref.rel} {relation['from']}→{relation['to']} の {name} は "
                f"{current!r} を採った（採らなかった値: {value!r}）",
                "同じ関係を複数の資料が張っており、先に読んだほうを採った"
                "（どちらが正しいかは決めていない）",
                decisions.GUESS, [_basis(record)]))
    return findings


def _direction(rel: str, definition: dict[str, Any], source_id: str, target_id: str,
               derived_items: dict[str, dict[str, Any]],
               spec: Spec) -> tuple[str, str, tuple[str, str, str] | None] | None:
    """**向きは自動で補正する。** 戻り値は ``(from, to, 決められなかった組み合わせ)``。

    抽出は「自分は誰に属するか」の向き（メソッド → 所属モジュール）で出がちだが、
    メタモデルは構造を親 → 子で宣言している（``has-method`` はモジュール → メソッド）。
    逆向きで成立するならそちらを採る。

    **補正できるのは、from と to の種別が非対称な関係だけ**である。``refines``
    （``same_type_only``）のように両向きとも宣言に合う組み合わせは、書いた向きが
    そのまま残る ―― 黙って残すと「機械が直す」と読んで逆向きに書いたトレースが
    そっくり落ちるので、決められなかったことを ``B026`` で言う。

    判定そのものは :func:`arp4.metamodel.orient` が持つ ―― **凍結ゲート（G012）と
    同じ規則でなければ「凍結は通ったのに build が落ちる」が起きる。**
    """
    def type_of(item_id: str) -> str:
        item = derived_items.get(item_id) or spec.by_id.get(item_id) or {}
        return str(item.get("type") or "")

    source_type, target_type = type_of(source_id), type_of(target_id)
    forward = mm.orient(definition, source_type, target_type)
    if forward is None:
        return None
    reverse = mm.orient(definition, target_type, source_type)
    ambiguous = ((rel, source_type, target_type)
                 if forward and reverse is True else None)
    if forward:
        return source_id, target_id, ambiguous
    return target_id, source_id, None


def _oriented_by_source(ref, record: Record) -> bool:
    """向きの確認が**出典から**済んでいる関係か（``B026`` から除く）。

    ``calls`` の出典が取り込みの塊（``i1``）なら、向きは資料に書いてある ――
    取り込み行は「import する側 → される側」であり、レコードは import する側の
    ファイルに書かれる。**向きの事実がパース結果にあるので、意味の判断ではない**
    （0-3。転記の確認に警告を出す必要は無い）。

    r001 の実測では 232 本が ``B026`` で一括警告され、実質読み飛ばされる量に
    なっていた ―― 本当に確かめてほしい ``refines`` の向きが、確認済みの
    ``calls`` の山に埋もれる。
    """
    return ref.rel == "calls" and record.anchor == IMPORT_ANCHOR


@dataclass(frozen=True)
class _Unsure:
    """向きを決められなかった関係 1 本。**決定はこの粒度で積む。**"""

    #: 書いた整理結果の場所（``file:line``）。警告の例示に使う。
    target: str
    #: 出典アンカー（``<パース結果>#<アンカー>``）。決定記録の根拠になる。
    basis: str
    #: 両端の名前。**内部 ID は出さない** ―― 表示 ID は ``number`` が build の
    #: あとに振るので、この時点で読み手が引ける手掛かりは名前しかない。
    from_name: str
    to_name: str


def _name_of(item_id: str, derived_items: dict[str, Any], spec: Spec) -> str:
    """決定記録に出す名前。引けなければ内部 ID のまま（黙って空にしない）。"""
    item = derived_items.get(item_id) or spec.by_id.get(item_id) or {}
    return str(item.get("name") or item_id)


def _unsure(unsure: dict[tuple[str, str, str], dict[tuple[str, str], _Unsure]],
            out: Plan) -> list[Finding]:
    """向きを決められなかった組み合わせを**組み合わせごとに 1 件**で出す。

    1 本ずつ出すと業務要件 31 本で 31 行になり、本物の warn が埋もれる。

    **場所は重複を畳んでから 3 つ出す。** 1 レコードが同じ組み合わせの ``refs`` を
    複数持つのは普通（モジュール 1 本が 5 本呼ぶ）で、畳まずに出すと
    ``build.py[8]、build.py[8]`` と同じ場所が並ぶ ―― 89 本のうちどれを開けばよいかを
    言うための例示なのに、**1 か所しか教えていないのに 3 つ出したように見える。**

    警告は畳んでも、**決定は畳まずに関係 1 本ずつ ``decisions`` へ積む** ――
    端末の 1 行にまとめた瞬間に「どの 100 本か」が消えるが、事後拒否権はその
    1 本ごとに要る。

    **ここは実際に畳んでいた**（→ 決定 78）。出典アンカーの集合で積んでいたため、
    実測（sales-corpus r001）で同型の関係 111 本に対し決定が **11 件**しか
    残らなかった ―― 1 シートから起こした 42 本の ``leads-to`` が 1 件に化け、
    ``out/決定記録.md`` からはどの矢印を差し戻せばよいかが引けなかった。
    この docstring が「畳まずに積む」と書いているのに実装が逆をしていた状態である。
    """
    findings: list[Finding] = []
    for (rel, source_type, target_type), edges in sorted(unsure.items()):
        places = sorted({edge.target for edge in edges.values()})
        findings.append(Finding(
            "warn", "B026", f"{rel} {source_type}→{target_type}",
            f"{source_type} → {target_type} も {target_type} → {source_type} も宣言に"
            f"合うので、向きは機械が直せません"
            f"（{len(edges)} 本 / {len(places)} レコード）。"
            f"書いた向きのまま入れました。詳細 → 概要になっているか確かめてください: "
            f"{'、'.join(places[:3])}"
            + ("…" if len(places) > 3 else "")))
        for edge in edges.values():
            out.logged.append(decisions.entry(
                "build",
                f"{rel}（{edge.from_name} → {edge.to_name}）を書いた向きのまま入れた",
                f"{source_type} → {target_type} も逆も宣言に合うので、機械には"
                f"決められない（{edge.target}）",
                decisions.GUESS, [edge.basis]))
    return findings


def _relation(ref, definition: dict[str, Any], from_id: str, to_id: str,
              counters: dict[tuple[str, str], int]) -> dict[str, Any]:
    record: dict[str, Any] = {"type": ref.rel, "from": from_id, "to": to_id,
                              "status": "review"}
    # 関係そのものについて資料が書いていること（「呼び出す目的」等）。属性ではなく
    # 本文なので、語彙の宣言を要らない ―― 相手ごとに属性を足すと、資料が 1 列で
    # 書いているものが語彙側で散る（→ 決定 72）。
    if ref.note:
        record["description"] = ref.note
    if definition.get("ordered"):
        key = (ref.rel, from_id)
        counters[key] = counters.get(key, 0) + 1
        record["order"] = counters[key]       # 資料に現れた順を保つ
    declared = definition.get("attributes") or {}
    for name, value in ref.attrs.items():
        if name in declared:
            record[name] = value
    return record


# ── 矛盾から起こす課題 ──────────────────────────────────────────
#: 課題が争点を指す関係。
DISPUTES = "disputes"


@dataclass
class Issues:
    """整理②の ``contradictions`` から起こしたもの。**どちらが正しいかは決めない。**"""

    items: list[dict[str, Any]] = field(default_factory=list)
    relations: list[dict[str, Any]] = field(default_factory=list)
    findings: list[Finding] = field(default_factory=list)
    #: 機械が下した判断（``decisions.yml`` へ追記する）。
    logged: list[dict[str, Any]] = field(default_factory=list)

    def __len__(self) -> int:
        return len(self.items)


def _positions_text(positions: list[dict[str, Any]]) -> str:
    return " ／ ".join(str(p.get("statement") or "") for p in positions
                       if str(p.get("statement") or ""))


def issues(spec: Spec, result: Organized, known: dict[str, Concept],
           built: Plan) -> Issues:
    """矛盾を課題にし、**争点へ ``disputes`` を張る。**

    関係を張らなかった以前の実装では、メタモデルが宣言する多重度
    （``disputes`` の ``from: 1..*``）を課題が必ず破り、``contradictions`` を
    書いた数だけ ``E027`` が積み上がった ―― **整理層からは手当てのしようがない
    error** だったので、起こす側が張る。

    ``subject`` は**争点そのものの concept** であって課題ではない。課題を指すと
    ``iss-<digest(subject)>`` が既存の課題と衝突して ``E002`` / ``E012`` になるため、
    ここで止めて ``B015`` で知らせる。

    **同じ ``subject`` の 2 件目は畳む（捨てない）。** 課題 ID は ``subject`` から
    決まるので 1 subject : 1 課題だが、以前は 2 件目以降を error も warn も出さずに
    ``continue`` していた ―― 実測（sales-corpus・13 ロット / 争点 59 件）で 6 組が
    同一 subject に集まっており、**気づかずに合流させれば 6 件以上が黙って消える**
    状態だった。争点が 1 つのアイテムに集まるのは分担の自然な結果であって書き手の
    誤りではないので、``positions`` を先の 1 件へ足し、``B017`` で畳んだことを言う。
    """
    out = Issues()
    definition = spec.metamodel.relation_types.get(DISPUTES) or {}
    allowed = set(definition.get("to") or [])
    #: 課題 ID → このラウンドで起こした課題（同一 subject を畳む先）。
    filed: dict[str, dict[str, Any]] = {}

    def type_of(item_id: str) -> str:
        return built.type_of(item_id) or str(
            (spec.by_id.get(item_id) or {}).get("type") or "")

    def item_of(concept: str) -> str:
        return built.item_of.get(concept) or (
            known[concept].item if concept in known else "")

    def basis_of(positions: list[dict[str, Any]]) -> list[str]:
        """両論が名指ししている出典。決定記録からここへ辿れる。

        ``positions[].source`` は ``{file, anchor}``（整理②が書く形）である ――
        そのまま文字列にすると ``{'file': …}`` が決定記録の根拠の欄に出る。
        :func:`_basis` と同じ ``<パース結果>#<アンカー>`` に揃える。

        **``file`` に拡張子が付いていても二重にしない。** ``_basis`` が使う
        ``record.file`` は拡張子を持たない内部の値だが、こちらは**人が
        ``_concepts.yml`` に手で書く値**である ―― 手順書の例が organize.md
        （``.md`` あり）と reconcile.md（なし）で割れていたため、実測
        （sales-corpus r001）で矛盾由来の決定 13 行すべてが
        ``…/3.機能要件一覧.md.md#s7-t1`` になり、**根拠パスが 1 本も辿れなかった。**
        規約は「拡張子を書かない」に揃えた（organize.md）が、手で書く値である
        以上は再発するので、ここでも受ける。
        """
        found: list[str] = []
        for position in positions:
            source = position.get("source")
            if isinstance(source, dict):
                file, anchor = source.get("file"), source.get("anchor")
                stem = str(file)[:-len(mdio.EXT)] \
                    if str(file).endswith(mdio.EXT) else str(file)
                one = f"{stem}{mdio.EXT}#{anchor}" if file and anchor else ""
            else:
                one = str(source or "")
            if one and one not in found:
                found.append(one)
        return found

    def dispute(issue_id: str, subject: str, subject_id: str,
                positions: list[dict[str, Any]]) -> None:
        """**両論のそれぞれが別のアイテムを指しているなら全部に張る。**
        指していなければ争点そのものが 1 つの相手になる。"""
        targets: list[str] = []
        for concept in [str(p.get("concept") or "") for p in positions]:
            if concept and item_of(concept) and item_of(concept) not in targets:
                targets.append(item_of(concept))
        if not targets and subject_id:
            targets = [subject_id]

        placed = False
        for target in targets:
            if allowed and type_of(target) not in allowed:
                out.findings.append(Finding(
                    "warn", "B016", issue_id,
                    f"{type_of(target) or '種別不明'} は disputes の相手になれません"
                    f"（{'、'.join(sorted(allowed))}）。争点を指せませんでした"))
                continue
            out.relations.append({"type": DISPUTES, "from": issue_id, "to": target,
                                  "status": "review"})
            placed = True
        if not placed and not targets:
            out.findings.append(Finding(
                "warn", "B016", issue_id,
                f"争点のアイテムが見つかりません: {subject}"
                "（このラウンドに subject のレコードがありますか）"))

    for entry in result.concepts.get("contradictions") or []:
        if not isinstance(entry, dict):
            continue
        subject = str(entry.get("subject") or "")
        positions = [p for p in (entry.get("positions") or []) if isinstance(p, dict)]
        statements = [str(p.get("statement") or "") for p in positions]
        if not subject or len([s for s in statements if s]) < 2:
            continue

        subject_id = item_of(subject)
        if subject_id and type_of(subject_id) == "open-issue":
            out.findings.append(Finding(
                "warn", "B015", subject,
                "contradictions.subject が課題を指しています（争点そのものの concept "
                "を書いてください）。課題は起こしませんでした"))
            continue

        issue_id = f"iss-{digest(subject)}"
        # **名前は書いた人が付けられる。** 既定は争点の label から組むが、
        # 争点が複数のアイテムにまたがるとき（`subject` は 1 つしか取れない）は
        # 代表 1 件を選ぶしかなく、**課題の名前が争点を表さなくなる** ―― 実測で
        # 「夜間バッチの実行順序」の争点が `EDI受注取込 の記述が資料間で食い違う`
        # として起票され、課題一覧から中身が読めなかった。
        label = known[subject].label if subject in known else subject
        name = str(entry.get("name") or "") or f"{label} の記述が資料間で食い違う"

        previous = filed.get(issue_id)
        if previous is not None:            # 同じ subject の 2 件目 ―― 畳む
            # 区切りは `_append` の空行ではなく `／` である ―― `positions` は
            # 課題管理表の 1 升に出るので、改行を入れると表が割れる。
            current = str(previous.get("positions") or "")
            added = _positions_text(positions)
            if added and added not in current:
                previous["positions"] = f"{current} ／ {added}" if current else added
            out.findings.append(Finding(
                "warn", "B017", subject,
                f"同じ subject の contradictions が既にあります（{name}）。"
                "課題 ID は subject から決まるので 1 subject : 1 課題 です。"
                f"両論を先の 1 件（{previous['name']}）へ足しました。"
                "別の争点として立てたいなら subject を分けてください"))
            out.logged.append(decisions.entry(
                "build", f"{issue_id} へ両論を足した（同一 subject の 2 件目）",
                f"課題 ID は subject（{subject}）から決まるので 1 subject : 1 課題",
                decisions.SURE, basis_of(positions)))
            dispute(issue_id, subject, subject_id, positions)
            continue

        if issue_id in spec.by_id:
            # 既に正本にある課題（再実行・前ラウンド起票）。**同じ内容なら黙る**
            # ―― build を打ち直すたびに鳴ると、再実行が warn を増やす。
            #
            # **決定は再実行でも同じものを積む。** build の判断は主体ごと置き換える
            # ので（:func:`arp4.decisions.replace`）、ここで黙ると 2 回目の build で
            # 起票の記録が決定記録から消える ―― ログは正本の状態ではなく
            # **整理結果の内容から決まる**ようにしておく。
            out.logged.append(decisions.entry(
                "build", f"矛盾から課題 {issue_id}（{name}）を起こした",
                f"整理②の contradictions が争点 {subject} に両論を書いている"
                "（どちらが正しいかは決めていない）",
                decisions.SURE, basis_of(positions)))
            recorded = str(spec.by_id[issue_id].get("positions") or "")
            if _positions_text(positions) not in recorded:
                out.findings.append(Finding(
                    "warn", "B017", subject,
                    f"この争点の課題は既に正本にあります（{issue_id}）が、"
                    "両論が一致しません。新しい両論は登録していません。"
                    "正本の positions へ足すか、subject を分けてください"))
            continue

        # **`statement` と `positions` に同じ文を入れない。** 以前はどちらも
        # 両論の連結で、課題管理表の「仕様」列と「両論」列に**同じ長文が二度**
        # 出ていた。`statement` は何が争点かを言い、両論は `positions` が持つ。
        #
        # **仕様文は `name` から作る。** 以前は `<label> について、資料が n 通りの
        # ことを言っている` という定型で、争点が違っても label しか変わらなかった
        # ―― 実測（sales-corpus・r001）で課題 28 件のうち 9 件が `W044`（仕様文が
        # ほぼ同一 ＝ 二重登録の疑い）で鳴り、**本物の二重登録が定型文の山に
        # 埋もれた**。課題管理表の「仕様」列が全件ほぼ同じ文になるのは、warn の
        # 問題である前に設計書の質の問題である。
        issue = {
            "id": issue_id, "type": "open-issue", "status": "review",
            "name": name,
            "statement": f"{name}（資料が {len([s for s in statements if s])} 通りの"
                         "ことを言っている。両論は positions を見ること）",
            "positions": _positions_text(positions),
        }
        out.items.append(issue)
        filed[issue_id] = issue
        out.logged.append(decisions.entry(
            "build", f"矛盾から課題 {issue_id}（{name}）を起こした",
            f"整理②の contradictions が争点 {subject} に両論を書いている"
            "（どちらが正しいかは決めていない）",
            decisions.SURE, basis_of(positions)))
        dispute(issue_id, subject, subject_id, positions)
    return out


# ── 置き場 ──────────────────────────────────────────────────────
def _place(spec: Spec, record: dict[str, Any], item: bool) -> None:
    """新規レコードを**既にその種別が入っているファイル**へ足す。

    人が決めたファイルの割り方（種別ごと／工程ごと）を機械が壊さないため。
    どこにも無ければ ``<種別>.yml`` を新しく作る（初回の構築はこの経路を通る）。
    """
    files = spec.item_files if item else spec.relation_files
    collection = spec.items if item else spec.relations
    type_name = str(record.get("type") or "unknown")

    collection.append(record)
    for _path, records in files:
        if any(str(r.get("type")) == type_name for r in records):
            records.append(record)
            return

    if spec.paths is None:                    # 書き出し先が決まらない（テスト用）
        return
    directory = spec.paths.items if item else spec.paths.relations
    path = (directory / type_name).with_suffix(yamlio.EXT)
    for existing, records in files:
        if existing == path:
            records.append(record)
            return
    files.append((path, [record]))
