"""表示 ID の採番（``FR-001`` / ``SCR-001`` / ``E-0001``）。

メタモデルが宣言し、ここが実行する::

    sequence:
      attribute: req_id            # どの属性に入れるか
      by: kind                     # この属性の値ごとに独立採番する
      prefix_from: category        # 接頭辞の元（display.yml の abbrev で略号に引く）
      format: { 機能: "FR-{:03d}", 非機能: "NFR-{:03d}", default: "REQ-{:03d}" }

規律が 3 つある。

1. **既存の番号は動かさない。** 採番が動くと設計書・議事録・課題票の参照が
   一斉に壊れる。埋めるのは空いているアイテムだけである
2. **番号に業務的な意味を埋めない。** 人が ``F-CODE-01`` のような ID を手で
   振ると、意味が変わるたびに ID を変えたくなり 1 に反する
3. **手で直した表示 ID（``overridden``）は振り直しでも触らない。** 契約書や既存資料と
   突き合わせるために固定した番号を機械が動かしてはならない

``format`` は**生成にも照合にも使う。** 宣言した書式を採番のときにしか見ないと、
既に入っている値が書式に合っているかを誰も見ない ―― 別系統の ID（元資料から
持ってきた番号、実装が実際に使っているコード）が正本に混ざったまま、機械にも人にも
気づかれない。:func:`nonconforming` が全件を照合する（``E028`` / ``W042``）。

書式外の値は**その体系の番号ではない**ので、番号としての予約もしない。``W030`` が
``W-{:04d}`` の 30 番を食うと、正規の ``W-0030`` が理由なく飛ぶ。

``--renumber``（全体の振り直し）と ``--fix-format``（書式外だけの振り直し）は
どちらも移行のための操作であり、日常には使わない。**黙って直しはしない** ――
元資料の番号は顧客との対応表であることがあり、機械が書き換えたら復元できない。

検出コード::

    W040  採番の書式が宣言されていない
    W041  手編集の保護（overridden）により振り直さなかった
    E028  表示 ID が採番の書式に合わない
    W042  同上だが overridden に理由が書かれている（承知している）
    E012  採番が重複した表示 ID を作った（``validate`` の一意違反と同じコード）
"""

from __future__ import annotations

import re
from dataclasses import dataclass
from typing import Any

from arp4 import override
from arp4.finding import Finding
from arp4.spec import Spec, display_config

_DIGITS = re.compile(r"(\d+)")
_FIELD = re.compile(r"\{[^{}]*\}")
_ZERO_PAD = re.compile(r"\{:0(\d+)d\}")


@dataclass(frozen=True)
class Assignment:
    """採番 1 件。"""

    item_id: str
    type_name: str
    attribute: str
    value: str
    previous: str | None = None
    #: 名称。**内部 ID だけでは正しさを判断できない。**
    name: str = ""

    def render(self) -> str:
        """1 行。**名前を出す。**

        `mtd-033bd43e8bc1: method_id = MTD-0001` を 131 行並べても、人は
        1 行も検算できない ―― 内部 ID は内容ハッシュなので**名前も種別も
        読み取れない**。番号に業務的な意味を持たせないと決めた（決定 17）以上、
        採番が妥当かを見る手がかりは名称しか無い。
        """
        label = f"{self.item_id}（{self.name}）" if self.name else self.item_id
        if self.previous:
            return f"{label}: {self.attribute} {self.previous} → {self.value}"
        return f"{label}: {self.attribute} = {self.value}"


def assign(spec: Spec, *, renumber: bool = False, fix_format: bool = False,
           abbrev: dict[str, str] | None = None
           ) -> tuple[list[Assignment], list[Finding]]:
    """採番案を作る。**正本は書き換えない**（適用は :func:`apply`）。"""
    if abbrev is None:
        abbrev = _abbrev(spec)

    assignments: list[Assignment] = []
    findings: list[Finding] = []

    for type_name, definition in spec.metamodel.item_types.items():
        sequence = definition.get("sequence") or {}
        attribute = sequence.get("attribute")
        if not attribute:
            continue
        items = sorted(spec.of_type(type_name), key=lambda i: str(i.get("id")))
        pending, used = _partition(items, attribute, sequence, abbrev, renumber,
                                   fix_format, findings, type_name)
        assignments += _number(pending, attribute, sequence, abbrev, used, type_name)

    assignments.sort(key=lambda a: (a.type_name, a.item_id))
    return assignments, findings


def apply(spec: Spec, assignments: list[Assignment]) -> set[str]:
    """採番案を正本へ書き込む。戻り値は**変更したアイテム ID**。"""
    by_id = spec.by_id
    changed: set[str] = set()
    for assignment in assignments:
        item = by_id.get(assignment.item_id)
        if item is None:
            continue
        item[assignment.attribute] = assignment.value
        changed.add(assignment.item_id)
    return changed


# ── 内部 ────────────────────────────────────────────────────────
def _abbrev(spec: Spec) -> dict[str, str]:
    return {str(k): str(v)
            for k, v in (display_config(spec.paths).get("abbrev") or {}).items()}



def _bucket(item: dict[str, Any], sequence: dict[str, Any],
            abbrev: dict[str, str]) -> tuple[str, str]:
    """採番の単位。``(接頭辞, グループ)`` ごとに 001 から振る。"""
    group = str(item.get(sequence["by"]) or "") if sequence.get("by") else ""
    prefix = ""
    if sequence.get("prefix_from"):
        prefix = abbrev.get(str(item.get(sequence["prefix_from"]) or ""), "")
    return prefix, group


def _counter_key(prefix: str, template: str | None) -> tuple[str, str]:
    """番号を数える束。**同じ書式に落ちるものは同じ束である。**

    :func:`_bucket` のグループ（``by`` の生値）で数えてはならない ―― ``format``
    は宣言外の値を ``default`` へ畳むので、``点検`` と ``PoC`` は**別々に数えられ
    ながら同じ ``TC-{:04d}`` を使う**。実測（10 件 + 2 件）で ``TC-0001`` /
    ``TC-0002`` が重複し、``--renumber`` は同じ束の切り方を通るので同じ衝突を
    再生産した。接頭辞の側は ``abbrev`` が未知の値を ``""`` へ畳んで 1 つの束に
    しており（→ ``test_略号の無い分類は1つの束に落ちる``）、**グループの側にだけ
    この畳み込みが無かった**。

    束ねるのは**解決したあとの書式**である。``default`` への畳み込みだけでなく、
    2 つのグループに同じ書式を書いたパック（``単体`` も ``結合`` も
    ``TC-{:04d}``）も、同じ理由で 1 つの束になる。
    """
    return prefix, template or ""


def _template(sequence: dict[str, Any], group: str) -> str | None:
    formats = sequence.get("format")
    if isinstance(formats, dict):
        return formats.get(group) or formats.get("default")
    return formats


def display_attribute(definition: dict[str, Any]) -> str | None:
    """この種別の**表示 ID**（``req_id`` / ``message_id``）。無ければ ``None``。

    **採番するかどうかとは別である。** 機械が振らない種別（資料から取る ID）にも
    表示 ID はある ―― `sequence` を外した種別を「表示 ID を持たない」と扱うと、
    設計書がその ID で並べられず、飛べもしなくなる。

    宣言（`sequence.attribute`）を先に見て、無ければ**必須かつ一意の ``*_id``**
    を探す。2 つ以上あるなら決められないので ``None`` を返す ―― どれが表示 ID かは
    意味の判断であって、ここでやることではない。
    """
    sequence = definition.get("sequence") or {}
    if sequence.get("attribute"):
        return str(sequence["attribute"])
    found = sorted(name for name, attr in (definition.get("attributes") or {}).items()
                   if name.endswith("_id") and (attr or {}).get("required")
                   and (attr or {}).get("unique"))
    return found[0] if len(found) == 1 else None


def sort_key(value: str) -> tuple[tuple[int, str], ...]:
    """表示 ID の並び順。**数字は数として比べる**（`FR-9` は `FR-10` の前）。

    桁を揃えていない資料由来の ID（`E17` / `E100`）が混ざっても崩れない。
    """
    return tuple((int(part), "") if part.isdigit() else (-1, part)
                 for part in _DIGITS.split(value) if part != "")


def expected(template: str, prefix: str = "") -> str:
    """人に見せる書式（``CORE-FR-{:03d}``）。エラー文の「何であるべきか」。"""
    return f"{prefix}-{template}" if prefix else template


def conforms(value: str, template: str, prefix: str = "") -> bool:
    """``value`` が採番の書式から出てくる形か。

    ``{:03d}`` を ``\\d{3}`` に固定してはならない ―― ``format(1000, "03d")`` は
    4 桁になるので、**1000 件を超えた瞬間に既存の番号が全部書式外**になる。
    桁は下限としてだけ見る。

    読めない書式（``{:>8}`` のような整数以外）は**通す。** 解釈できないものを
    error にすると、消費側がパックを拡張したときに直しようのない検出が出る。
    """
    parts: list[str] = []
    last = 0
    for match in _FIELD.finditer(template):
        parts.append(re.escape(template[last:match.start()]))
        pad = _ZERO_PAD.fullmatch(match.group(0))
        if pad:
            parts.append(r"\d{%s,}" % pad.group(1))
        elif match.group(0) in ("{}", "{:d}"):
            parts.append(r"\d+")
        else:
            parts.append(r".+")
        last = match.end()
    parts.append(re.escape(template[last:]))

    head = re.escape(f"{prefix}-") if prefix else ""
    return re.fullmatch(head + "".join(parts), value) is not None


def _partition(items: list[dict[str, Any]], attribute: str,
               sequence: dict[str, Any], abbrev: dict[str, str], renumber: bool,
               fix_format: bool, findings: list[Finding], type_name: str
               ) -> tuple[list[dict[str, Any]], dict[tuple[str, str], set[int]]]:
    """振る対象と、すでに使われている番号に分ける。"""
    pending: list[dict[str, Any]] = []
    used: dict[tuple[str, str], set[int]] = {}

    for item in items:
        prefix, group = _bucket(item, sequence, abbrev)
        template = _template(sequence, group)
        key = _counter_key(prefix, template)
        current = str(item.get(attribute) or "")
        locked = attribute in override.names(item)
        stray = bool(current) and bool(template) and not conforms(current, template,
                                                                  prefix)

        if not current:
            pending.append(item)
            continue
        if renumber or (fix_format and stray):
            if not locked:
                pending.append(item)
                continue
            findings.append(Finding(
                "warn", "W041", str(item.get("id")),
                f"手編集の保護により振り直しませんでした: {attribute}={current}"))
        # 書式外の値は**その体系の番号ではない**ので予約もしない。``W030`` が
        # ``W-{:04d}`` の 30 番を食うと、正規の ``W-0030`` が理由なく飛ぶ。
        if stray:
            continue
        digits = _DIGITS.findall(current)
        if digits:
            used.setdefault(key, set()).add(int(digits[-1]))

    # 振り直しは既存番号の順を保つ（無用な入れ替わりを起こさない）。
    # 番号を持たないものは末尾（``_current_number`` が 10**9 を返す）。
    if renumber or fix_format:
        pending.sort(key=lambda i: (_current_number(i, attribute), str(i.get("id"))))
    return pending, used


def _current_number(item: dict[str, Any], attribute: str) -> int:
    digits = _DIGITS.findall(str(item.get(attribute) or ""))
    return int(digits[-1]) if digits else 10 ** 9


def _number(pending: list[dict[str, Any]], attribute: str,
            sequence: dict[str, Any], abbrev: dict[str, str],
            used: dict[tuple[str, str], set[int]],
            type_name: str) -> list[Assignment]:
    assignments: list[Assignment] = []
    for item in pending:
        prefix, group = _bucket(item, sequence, abbrev)
        template = _template(sequence, group)
        if not template:
            continue                     # 書式が無い（W040 は assign 側で出す）
        taken = used.setdefault(_counter_key(prefix, template), set())
        number = 1
        while number in taken:
            number += 1
        taken.add(number)
        value = template.format(number)
        assignments.append(Assignment(
            item_id=str(item.get("id")), type_name=type_name, attribute=attribute,
            value=f"{prefix}-{value}" if prefix else value,
            previous=str(item.get(attribute)) if item.get(attribute) else None,
            name=str(item.get("name") or "")))
    return assignments


def collisions(spec: Spec, assignments: list[Assignment]) -> list[Finding]:
    """**採番が作った重複を、書き込む前に言う。**

    表示 ID は一意（``unique: true``）なので、機械が同じ値を 2 度振ったら正本が
    壊れる ―― 設計書の相互参照が指す先を失い、``--renumber`` でも戻らない（束の
    切り方が同じなので同じ衝突が出る）。それを言うコードは ``validate`` の
    ``E012`` にあるが、**``number`` は自分が生成した値を検査していなかった** ――
    実測で ``TC-0001`` / ``TC-0002`` の重複が採番時に 1 件も鳴らず、正本へ書き
    込まれたあと ``check --strict`` で初めて出た。

    見るのは**この実行が作る衝突だけ**である。元からある重複は ``check`` の
    仕事で、ここで一緒に止めると、その重複を直すために ``number`` を打った人が
    進めなくなる。
    """
    reassigned = {a.item_id for a in assignments}
    occupied: dict[tuple[str, str, str], str] = {}
    for type_name, attribute in {(a.type_name, a.attribute) for a in assignments}:
        for item in spec.of_type(type_name):
            item_id = str(item.get("id"))
            value = str(item.get(attribute) or "")
            if value and item_id not in reassigned:
                occupied.setdefault((type_name, attribute, value), item_id)

    findings: list[Finding] = []
    for assignment in assignments:
        key = (assignment.type_name, assignment.attribute, assignment.value)
        previous = occupied.get(key)
        if previous is None:
            occupied[key] = assignment.item_id
            continue
        findings.append(Finding(
            "error", "E012", assignment.item_id,
            f"{assignment.attribute} が一意ではありません: {assignment.value}"
            f"（{previous} と重複）"))
    return findings


def nonconforming(spec: Spec, abbrev: dict[str, str] | None = None) -> list[Finding]:
    """宣言した書式に合っていない表示 ID を挙げる。

    **書式は生成にしか使われていなかった。** 空欄は必須属性の欠落（``E010``）が
    言うので、ここが見るのは「値はあるが別系統の ID である」ものだけである ――
    元資料から持ってきた番号、実装が実際に使っているコード、手で振った通番。
    どれも黙って書き換えたら復元できないので、機械は**検知だけ**する
    （直すのは ``arp4 number --fix-format`` を打った人）。

    ``overridden`` に理由が書いてあるものは warn へ落とす。「資料と突き合わせる
    ために固定した」という判断は残す価値があり、理由の無い放置とは別物である。
    """
    if abbrev is None:
        abbrev = _abbrev(spec)

    findings: list[Finding] = []
    for type_name, definition in spec.metamodel.item_types.items():
        sequence = definition.get("sequence") or {}
        attribute = sequence.get("attribute")
        if not attribute:
            continue
        for item in sorted(spec.of_type(type_name), key=lambda i: str(i.get("id"))):
            current = str(item.get(attribute) or "")
            if not current:
                continue                       # 空欄は E010 が言う
            prefix, group = _bucket(item, sequence, abbrev)
            template = _template(sequence, group)
            if not template:
                continue                       # 書式が無い（W040）
            if conforms(current, template, prefix):
                continue

            trouble = (f"{attribute} が採番の書式に合いません: {current}"
                       f"（{expected(template, prefix)}）")
            reason = override.reason(item, attribute)
            if reason:
                findings.append(Finding(
                    "warn", "W042", str(item.get("id")),
                    f"{trouble}。overridden で承知している: {reason}"))
            else:
                findings.append(Finding("error", "E028", str(item.get("id")), trouble))
    return findings


def missing_format(spec: Spec, abbrev: dict[str, str] | None = None) -> list[Finding]:
    """採番できないアイテム（書式が無い）を報告する。**黙って飛ばさない。**"""
    if abbrev is None:
        abbrev = _abbrev(spec)

    findings: list[Finding] = []
    for type_name, definition in spec.metamodel.item_types.items():
        sequence = definition.get("sequence") or {}
        attribute = sequence.get("attribute")
        if not attribute:
            continue
        for item in spec.of_type(type_name):
            if item.get(attribute):
                continue
            _, group = _bucket(item, sequence, abbrev)
            if not _template(sequence, group):
                findings.append(Finding(
                    "warn", "W040", str(item.get("id")),
                    f"採番書式がありません（{type_name}.sequence.format に "
                    f"{group or 'default'} がありません）"))
    return findings
