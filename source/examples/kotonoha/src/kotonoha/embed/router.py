"""経路の振り分け。**極秘データが外部 API へ出ないことをここで担保する。**

情報セキュリティ点検表（2026/03）の指摘に

    「機密区分が極秘の情報を外部サービスへ送出しないこと。
      送出しない仕組みがコードにあること。」

があり、その「仕組み」がこのモジュールである。区分の判定は
``tenant.classification`` が持ち、ここはそれに従って提供元を選ぶだけ ——
**判定と振り分けを分けてある**のは、区分の規程が変わったときに直す場所を
1 つにするためである。

二重に確かめる。

1. ``registry.resolve`` がモデルを決めるときに区分と経路を突き合わせる
2. :meth:`Router.provider_for` が提供元を返すときにもう一度突き合わせる

冗長だが、経路の選択はここを通らないと起きないので、通り道を 1 本に
絞って両端で見る形にしてある。
"""

from __future__ import annotations

from kotonoha.common import logging as applog
from kotonoha.common.errors import ClassificationViolation, ProviderError
from kotonoha.embed import registry
from kotonoha.embed.models import EmbedModel
from kotonoha.embed.provider import EmbedProvider
from kotonoha.tenant import classification as cls

log = applog.get(__name__)


class Router:
    """機密区分からモデルと提供元を決める。"""

    def __init__(self, external: EmbedProvider, internal: EmbedProvider) -> None:
        self._external = external
        self._internal = internal

    def model_for(self, requested: str | None, classification: str) -> EmbedModel:
        """使うモデルを決める。

        :raises ClassificationViolation: 極秘に外部モデルを指定した
        """
        return registry.resolve(requested, classification)

    def provider_for(self, model: EmbedModel, classification: str) -> EmbedProvider:
        """提供元を決める。

        :raises ClassificationViolation: 区分に許されない経路になった
        :raises ProviderError: 提供元がそのモデルを扱えない
        """
        cls.ensure_route_allowed(classification, model.route)
        provider = self._external if model.external else self._internal
        if not provider.supports(model):
            raise ProviderError(
                f"提供元が {model.name} を扱えません", model=model.name,
                route=model.route)
        return provider

    def route_of(self, classification: str, requested: str | None = None) -> str:
        """経路の名前だけを知りたいとき（監査ログの事前記録など）。"""
        return self.model_for(requested, classification).route

    def check(self, classification: str, model_name: str) -> None:
        """組み合わせが許されるかだけを確かめる。副作用は無い。

        取り込みジョブを受け付ける前の点検に使う —— **埋め込みを始めてから
        弾くと、途中まで外部へ出てしまう。**

        :raises ClassificationViolation: 許されない組み合わせ
        """
        model = registry.get(model_name)
        try:
            cls.ensure_route_allowed(classification, model.route)
        except ClassificationViolation:
            log.error("機密区分に反する組み合わせを弾きました class=%s model=%s",
                      classification, model_name)
            raise
