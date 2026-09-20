"""機密区分。社内の情報区分規程に沿う。

===  ========  ====================  ==========
値   名称      外部 API への送出      保持期間
===  ========  ====================  ==========
10   一般      可                    無期限
20   社外秘    可（契約による）       5 年
30   極秘      **不可**              3 年
===  ========  ====================  ==========

30（極秘）だけが特別で、**外部の API へテキストを渡してはならない**。
法務部の契約書がこれにあたる。埋め込みは社内 GPU の推論サーバへ回す
（``embed.router``）。精度は落ちるが、それは受け入れた上での運用である
（PoC評価報告書.xlsx の比較表を参照）。
"""

from __future__ import annotations

from dataclasses import dataclass

from kotonoha.common.errors import ClassificationViolation, InvalidInput

GENERAL = "10"
CONFIDENTIAL = "20"
SECRET = "30"


@dataclass(frozen=True)
class Classification:
    """機密区分 1 つぶん。"""

    code: str
    name: str
    external_allowed: bool
    retention_years: int | None   # None は無期限

    @property
    def rank(self) -> int:
        """強さ。大きいほど厳しい。"""
        return int(self.code)


_ALL = {
    GENERAL: Classification(GENERAL, "一般", True, None),
    CONFIDENTIAL: Classification(CONFIDENTIAL, "社外秘", True, 5),
    SECRET: Classification(SECRET, "極秘", False, 3),
}


def of(code: str) -> Classification:
    """コードから引く。

    :raises InvalidInput: 規程に無いコード
    """
    if code not in _ALL:
        raise InvalidInput(f"知らない機密区分です: {code}", code=code)
    return _ALL[code]


def all_codes() -> list[str]:
    return sorted(_ALL)


def allows_external(code: str) -> bool:
    """外部 API へ出してよいか。"""
    return of(code).external_allowed


def ensure_not_lowered(inherited: str, requested: str) -> str:
    """継承した区分より緩い区分を要求していないか確かめる。

    コレクションはテナントの区分を継承し、**厳しくはできるが緩くはできない**。
    要求が無い（``None``）ときは継承をそのまま返す。

    :raises ClassificationViolation: 継承より緩い区分を指定した
    """
    if not requested:
        return inherited
    if of(requested).rank < of(inherited).rank:
        raise ClassificationViolation(
            f"機密区分を {inherited} から {requested} へ下げることはできません",
            inherited=inherited, requested=requested,
        )
    return requested


def ensure_route_allowed(code: str, route: str) -> None:
    """使おうとしている経路が区分に許されているか。

    :param route: ``external`` か ``internal``
    :raises ClassificationViolation: 極秘で外部経路を使おうとした
    """
    if route == "external" and not allows_external(code):
        raise ClassificationViolation(
            f"機密区分 {code}（{of(code).name}）は外部 API へ送れません",
            classification=code, route=route,
        )
