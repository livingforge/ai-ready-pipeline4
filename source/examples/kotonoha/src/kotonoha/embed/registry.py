"""使えるモデルの台帳。

**採用の根拠はここに無い。** どのモデルを既定にするかは PoC の精度評価で
決めたが、その比較表は ``資料/PoC評価報告書.xlsx`` にしかない。ADR にも
docs/ にも書かれていないので、コードだけを読んでも「なぜ voyage-4 なのか」は
分からない（README の仕込み A）。

極秘（機密区分 30）で使えるのは ``voyage-4-nano`` だけである。オープン
ウェイト（Apache-2.0）なので社内 GPU に載せられる、というのが唯一の理由で、
精度は外部 API のモデルに劣る。
"""

from __future__ import annotations

from kotonoha.common.errors import InvalidInput
from kotonoha.embed.models import EmbedModel
from kotonoha.tenant import classification as cls

#: 既定のモデル。設定でも上書きできる。
DEFAULT_MODEL = "voyage-4"

#: 極秘で使う社内ホストのモデル。**これ以外を極秘に使ってはならない。**
INTERNAL_MODEL = "voyage-4-nano"

_MODELS: dict[str, EmbedModel] = {
    "voyage-4": EmbedModel(
        "voyage-4", dim=1024, max_tokens=32_000, route="external",
        note="既定。品質とコストの釣り合いで選定"),
    "voyage-4-large": EmbedModel(
        "voyage-4-large", dim=1024, max_tokens=32_000, route="external",
        note="最高品質。第2次リリースで移行を検討中"),
    "voyage-4-lite": EmbedModel(
        "voyage-4-lite", dim=1024, max_tokens=32_000, route="external",
        note="低遅延・低コスト。PoC で精度が要求に届かず不採用"),
    "voyage-4-nano": EmbedModel(
        "voyage-4-nano", dim=1024, max_tokens=32_000, route="internal",
        note="オープンウェイト（Apache-2.0）。極秘の唯一の選択肢"),
    "voyage-law-2": EmbedModel(
        "voyage-law-2", dim=1024, max_tokens=16_000, route="external",
        note="法務・長文向け。★極秘では使えないので法務部には出せない"),
}


def get(name: str) -> EmbedModel:
    """名前で引く。

    :raises InvalidInput: 台帳に無いモデル
    """
    model = _MODELS.get(name)
    if model is None:
        raise InvalidInput(
            f"知らない埋め込みモデルです: {name}",
            model=name, available=sorted(_MODELS),
        )
    return model


def resolve(name: str | None, classification: str) -> EmbedModel:
    """機密区分に照らして使えるモデルを決める。

    極秘（30）では、名前が何であっても社内ホストのモデルへ倒す。
    **黙って倒す**のではなく、外部モデルを名指しされたときは弾く ——
    利用側が「voyage-law-2 を使っている」と思い込んだまま別のモデルで
    埋め込まれると、精度の議論が噛み合わなくなるため。

    :raises ClassificationViolation: 極秘に外部モデルを指定した
    """
    if not name:
        name = INTERNAL_MODEL if classification == cls.SECRET else DEFAULT_MODEL
    model = get(name)
    cls.ensure_route_allowed(classification, model.route)
    return model


def all_models() -> list[EmbedModel]:
    return [_MODELS[name] for name in sorted(_MODELS)]


def usable_for(classification: str) -> list[EmbedModel]:
    """その区分で使えるモデルの一覧。"""
    return [m for m in all_models() if cls.allows_external(classification) or not m.external]
