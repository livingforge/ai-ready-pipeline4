"""``overridden`` ―― **出典と異なる値の記録。**

3 の ``edited`` は「再変換で人の編集を潰さないための保護」だった。4 は整理結果を
凍結して**再生成しない**ので、保護そのものは要らない。必要なのは
**「なぜ出典と違うのか」の記録**である::

    - id: ent-9a3c4f21b8e0
      physical_name: T_ORDERS
      overridden:
        physical_name:
          was: T_ORDER
          reason: 実装確認の結果、資料が旧名のままだった（PR #142）
          at: 2026-08-20

``reason`` は必須である。空を許すと**単なる上書きと区別がつかない**。設計書の生成時に
「資料と異なる」を注記できるので、あとから来た人が「資料を読み違えたのか、資料が
間違っていたのか」を判断できる。

つねに機械が触らないのは 3 つ ―― ``status``（承認は人の判断）、``overridden``
（記録そのもの）、表示 ID（採番が動くと設計書・議事録・課題票の参照が一斉に壊れる）。
"""

from __future__ import annotations

from typing import Any, Iterable

#: 種別によらずつねに機械が触らない管理キー。
ALWAYS_PROTECTED = ("status", "overridden", "known_gaps")


def names(item: dict[str, Any]) -> set[str]:
    """``overridden`` に挙がっている属性名。"""
    declared = item.get("overridden")
    if isinstance(declared, dict):
        return {str(name) for name in declared}
    if isinstance(declared, list):            # 名前だけの略記も受ける
        return {str(name) for name in declared}
    return set()


def protected(item: dict[str, Any],
              definition: dict[str, Any] | None = None) -> set[str]:
    """このアイテムで**再構築が触ってはいけない**属性名。"""
    fields = set(ALWAYS_PROTECTED) | names(item)
    sequence = (definition or {}).get("sequence") or {}
    if sequence.get("attribute"):
        fields.add(str(sequence["attribute"]))
    return fields


def merge_item(previous: dict[str, Any] | None, derived: dict[str, Any],
               definition: dict[str, Any] | None = None
               ) -> tuple[dict[str, Any], list[str]]:
    """再構築の結果を既存アイテムへ重ねる。戻り値は ``(結果, 守った属性名)``。

    **derived に無くなった属性は消さない。** 整理のゆらぎで人の追記が消えるほうが、
    古い値が残るより回復しにくい。守った属性を返すのは、**黙って守ると「なぜ資料の
    記述が反映されないのか」が利用者に見えない**ためである。
    """
    if not previous:
        return dict(derived), []

    keep = protected(previous, definition)
    kept: list[str] = []
    merged = dict(previous)
    for name, value in derived.items():
        if name in keep:
            if name in previous and previous[name] != value:
                kept.append(name)
            continue
        merged[name] = value
    return merged, sorted(kept)


def unknown(item: dict[str, Any], attributes: Iterable[str]) -> list[str]:
    """``overridden`` に挙がっているが、そんな属性は無いもの（誤字の検出）。"""
    known = set(attributes) | set(ALWAYS_PROTECTED)
    return sorted(names(item) - known)


def missing_reason(item: dict[str, Any]) -> list[str]:
    """理由の無い上書き。**空を許すと単なる上書きと区別がつかない。**"""
    declared = item.get("overridden")
    if not isinstance(declared, dict):
        return sorted(names(item))            # 略記は理由を持てない
    return sorted(name for name, entry in declared.items()
                  if not isinstance(entry, dict)
                  or not str(entry.get("reason") or "").strip())


def reason(item: dict[str, Any], name: str) -> str:
    """その属性の上書きが**理由つきで**表明されているか。無ければ空文字。

    :func:`note` は設計書に出す文面（「資料では〜」）を組み立てるが、こちらは
    検査側が「承知しているものか」を判定するために使う ―― 理由の有無で
    error を warn へ落とすかが決まるので、``was`` の無い記録でも拾う。
    """
    declared = item.get("overridden")
    if not isinstance(declared, dict):
        return ""
    entry = declared.get(name)
    if not isinstance(entry, dict):
        return ""
    return str(entry.get("reason") or "").strip()


def note(item: dict[str, Any], name: str) -> str:
    """設計書に出す注記（「資料と異なる」）。無ければ空。"""
    declared = item.get("overridden")
    if not isinstance(declared, dict) or name not in declared:
        return ""
    entry = declared[name]
    if not isinstance(entry, dict):
        return ""
    was = str(entry.get("was") or "")
    reason = str(entry.get("reason") or "")
    return f"資料では {was}（{reason}）" if was else reason
