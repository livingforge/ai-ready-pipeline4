"""決定ログ ―― **機械が下した判断を全件、事後拒否権の入口として残す。**

主体（``by``）は ``draft`` / ``build`` / ``auto`` の 3 つで、**どれも機械である**
―― 整理層（AI）の判断はここへ来ない。生成物の文言もそう書く
（→ :func:`arp4.audience.decision_report`）。


無承認の通し実行（Phase 4）で消えるのは**承認ゲート**であって追跡性ではない。
承認を消せるのは、下した判断が 1 件残らず記録され、任意の 1 件から出典アンカーへ
辿れるからである ―― 人は流れを止める代わりに、記録を読んで**止めたいものだけ**
差し戻す（決定ログ＋事後拒否権）。

置き場は ``.arp/rounds/<ラウンド>/decisions.yml``。ラウンドの持ち物である
（判断はそのラウンドの資料に対して下されたもの）が、**凍結ハッシュの対象外**
（``organized/`` の外）に置く ―― 凍結後も build / auto が判断を追記するため。

1 件の形::

    - by: draft                 # 判断した主体（draft / build / auto）
      what: tier=Common を付けた
      why: "@dataclass が付いている（規約: organize.md）"
      confidence: 確実          # 確実（規則の適用）/ 推定（曖昧さが残る）
      basis: [src/arp4/mdio.py.md#m1]   # 根拠にした出典アンカー

時刻は書かない ―― 同じ入力から同じログが出ること（再現性）のほうが、
いつ書かれたかより価値がある。いつのラウンドかはパスが言っている。
"""

from __future__ import annotations

from typing import Any

from arp4 import yamlio
from arp4.paths import Round

#: ログのファイル名。
FILE = "decisions.yml"

#: 確度の値。**規則の適用**（同じ入力なら必ず同じ結果）と、**曖昧さの残る推定**
#: （複数候補から機械が選んだ・選べず飛ばした）を分ける。読む人が拒否権を
#: 行使する優先度はこの順である。
SURE = "確実"
GUESS = "推定"


def path_of(round_: Round):
    return round_.dir / FILE


def entry(by: str, what: str, why: str, confidence: str = SURE,
          basis: list[str] | None = None) -> dict[str, Any]:
    """判断 1 件。``basis`` は出典アンカー（``<パース結果>#<アンカー>``）の列。"""
    record: dict[str, Any] = {"by": by, "what": what, "why": why,
                              "confidence": confidence}
    if basis:
        record["basis"] = list(basis)
    return record


def replace(round_: Round, by: str, entries: list[dict[str, Any]],
            replaced=None) -> None:
    """主体 ``by`` の判断を**置き換える**。draft のように何度でも回すものが使う
    ―― 追記にすると、回すたびに同じ判断が二重に積まれ、件数が判断の数を
    言わなくなる。

    ``replaced``（判断 1 件 → bool）を渡すと、**真を返す既存の判断だけ**を
    捨てる ―― draft が一部のファイルだけ生成し直したとき、飛ばしたファイルの
    判断まで消さないため。"""
    kept = [e for e in load(round_)
            if str(e.get("by")) != by
            or (replaced is not None and not replaced(e))]
    _write(round_, kept + entries)


def append(round_: Round, entries: list[dict[str, Any]]) -> None:
    """判断を追記する（build / auto の 1 回きりの判断）。"""
    if entries:
        _write(round_, load(round_) + entries)


def load(round_: Round) -> list[dict[str, Any]]:
    path = path_of(round_)
    if not path.is_file():
        return []
    data = yamlio.load(path)
    return [e for e in (data or []) if isinstance(e, dict)] \
        if isinstance(data, list) else []


def _write(round_: Round, entries: list[dict[str, Any]]) -> None:
    yamlio.dump(path_of(round_), entries)
