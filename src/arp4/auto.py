"""``arp4 auto`` の機械判断 ―― known_gaps の自動宣言と status の自動昇格（Phase 4）。

**承認ゲートの代わりは「決定ログ＋事後拒否権」である。** ここにある判断はすべて
:mod:`arp4.decisions` に全件記録され、publish の「決定記録」から出典アンカーへ
辿れる ―― 人は流れを止める代わりに、記録を読んで止めたいものだけ差し戻す。
"""

from __future__ import annotations

from typing import Any

from arp4 import decisions, gaps, yamlio
from arp4.spec import Spec

#: 昇格の対象になる status。
_PROMOTABLE = "review"


def policy(spec: Spec) -> dict[str, Any]:
    """``.arp/policy.yml``。無ければ空（＝すべて既定の保守側）。"""
    if spec.paths is None or not spec.paths.policy.is_file():
        return {}
    data = yamlio.load(spec.paths.policy)
    return data if isinstance(data, dict) else {}


def declare_gaps(spec: Spec, round_name: str
                 ) -> tuple[set[str], list[dict[str, Any]]]:
    """「相手が資料に無い」と**機械が判定できる** ``W031`` を自動で宣言する（4-4）。

    機械が言えるのは 1 つの形だけである ―― **相手になれる種別のアイテムが正本に
    1 件も無い**とき。1 件でもあれば「その中に相手がいるか」は意味の判断なので
    宣言せず、W031 は人（か整理層）に残る。r001 では 7 件を人手で残置したが、
    その手当てと同じ文面を機械が書けるのはこの形だけで、**書けない残りを
    書けるふりをしない**。

    宣言は :mod:`arp4.gaps` の正規の形で書くので、相手が現れて関係を張れば
    ``W033`` が「宣言はもう要らない」と言う ―― 自動宣言が正本に居座らない。
    """
    changed: set[str] = set()
    logged: list[dict[str, Any]] = []
    for item in spec.items:
        definition = spec.metamodel.item_types.get(str(item.get("type"))) or {}
        upstream = definition.get("warn_if_no_upstream")
        if not upstream:
            continue
        upstream = str(upstream)
        item_id = str(item.get("id"))
        if any(str(r.get("from")) == item_id
               for r in spec.relations_of(upstream)):
            continue
        if gaps.reason(item, upstream):
            continue                      # 人が既に宣言している（上書きしない）
        relation = spec.metamodel.relation_types.get(upstream) or {}
        allowed = [str(t) for t in (relation.get("to") or [])]
        if not allowed or any(any(True for _ in spec.of_type(t))
                              for t in allowed):
            continue                      # 相手候補が居る ―― 判断は機械の外
        reason = (f"相手になれる種別（{'、'.join(allowed)}）のアイテムが正本に"
                  f" 1 件もありません（{round_name} 時点の機械判定。資料が"
                  "届いて関係を張れば W033 が宣言の削除を促します）")
        entry = item.setdefault(gaps.KEY, {})
        if isinstance(entry, dict):
            entry[upstream] = {"reason": reason}
            changed.add(item_id)
            logged.append(decisions.entry(
                "auto", f"{item_id} の known_gaps に {upstream} を自動宣言した",
                reason, decisions.SURE))
    return changed, logged


def promote(spec: Spec, round_name: str
            ) -> tuple[set[str], list[dict[str, Any]]]:
    """``check`` を通ったアイテムを ``approved`` へ昇格する（4-3）。

    **呼び出し側が「check error 0」を確かめてから呼ぶ。** 既定では呼ばれない
    （``policy.yml`` の ``auto_approve: true`` を書いたプロジェクトだけ）――
    自動昇格が誤りを正本に固定するリスクは、既定を保守側に倒すことでしか
    塞げない（計画書のリスク欄）。

    **課題（open-issue）は昇格しない。** 矛盾から起こした課題は両論併記のまま
    人が裁くもの ―― ここだけは人の判断として常に残す（矛盾の自動解決はしない）。
    """
    changed: set[str] = set()
    for item in spec.items:
        if str(item.get("status")) != _PROMOTABLE:
            continue
        if str(item.get("type")) == "open-issue":
            continue                      # 矛盾の裁定は常に人に残す
        item["status"] = "approved"
        changed.add(str(item.get("id")))
    if not changed:
        return changed, []
    logged = [decisions.entry(
        "auto", f"アイテム {len(changed)} 件を approved へ昇格した",
        f"arp4 check が error 0（{round_name}）で、policy.yml の "
        "auto_approve が有効", decisions.SURE)]
    return changed, logged
