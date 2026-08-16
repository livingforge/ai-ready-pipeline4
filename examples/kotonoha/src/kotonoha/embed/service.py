"""エンベディング生成の入口。``POST /v1/embeddings`` と取り込みの両方が使う。

流れは 1 本。

    区分からモデルと経路を決める（router）
      → キャッシュを引く（cache）
      → 外れた分だけ提供元の上限で分割して投げる（batch → provider）
      → 量子化する（quantize）★ ADR-003 に無い
      → キャッシュへ入れる
      → 監査ログへ残す（本文は残さない）

**課金に数えるのは「提供元へ投げた件数」だけ**である（``billed_count``）。
キャッシュに当たった分は数えない。
"""

from __future__ import annotations

from kotonoha.common import logging as applog
from kotonoha.common.audit import AuditEntry, record
from kotonoha.common.clock import now
from kotonoha.common.settings import SETTINGS
from kotonoha.embed import batch as batching
from kotonoha.embed import quantize as quant
from kotonoha.embed.cache import EmbedCache
from kotonoha.embed.models import EmbedRequest, EmbedResult, Vector
from kotonoha.embed.router import Router

log = applog.get(__name__)


class EmbedService:
    """埋め込みを作る。"""

    def __init__(self, router: Router, cache: EmbedCache | None = None,
                 quantize: bool | None = None) -> None:
        self._router = router
        self._cache = cache
        self._quantize = SETTINGS.quantize if quantize is None else quantize

    def embed(self, request: EmbedRequest) -> EmbedResult:
        """まとめて埋め込む。順序は入力と同じ。

        :raises ClassificationViolation: 極秘に外部モデルを指定した
        :raises InvalidInput: 長すぎるテキストがある
        :raises ProviderError: 提供元が最後まで失敗した
        """
        started = now()
        model = self._router.model_for(request.model, request.classification)
        provider = self._router.provider_for(model, request.classification)

        vectors: list[Vector | None] = [None] * len(request.texts)
        cached = 0
        pending = list(range(len(request.texts)))

        if self._cache is not None:
            lookup = self._cache.lookup(request.texts, model.name,
                                        request.input_type)
            for index, vector in lookup.hits.items():
                vectors[index] = vector
            cached = lookup.hit_count
            pending = lookup.misses

        billed = 0
        if pending:
            missing = [request.texts[i] for i in pending]
            for group in batching.split(missing, model):
                chunk = [missing[i] for i in group]
                made = provider.embed(chunk, model, request.input_type)
                for offset, vector in zip(group, made):
                    vectors[pending[offset]] = vector
                billed += len(chunk)
            if self._cache is not None:
                self._cache.store(missing, [vectors[i] for i in pending],
                                  model.name, request.input_type)

        if self._quantize:
            vectors = [quant.quantize(v) for v in vectors if v is not None]

        result = EmbedResult(
            vectors=[v for v in vectors if v is not None],
            model=model.name,
            route=model.route,
            billed_count=billed,
            cached_count=cached,
            elapsed_ms=int((now() - started).total_seconds() * 1000),
        )

        record(AuditEntry(
            tenant_id=request.tenant_id,
            operation="embed",
            classification=request.classification,
            status_code=200,
            embed_model=model.name,
            route=model.route,
            item_count=len(request.texts),
            elapsed_ms=result.elapsed_ms,
        ))
        log.info("埋め込みました tenant=%s model=%s route=%s 件数=%d 課金=%d 蓄積=%d",
                 request.tenant_id, model.name, model.route,
                 len(request.texts), billed, cached)
        return result

    def embed_query(self, text: str, model: str, classification: str, *,
                    tenant_id: str = "") -> Vector:
        """検索語 1 件。``input_type='query'`` で呼ぶ。

        **検索語はキャッシュしない** —— 同じ語が二度来ることは少なく、
        キャッシュに載せると検索履歴が残ってしまう。
        """
        model_spec = self._router.model_for(model, classification)
        provider = self._router.provider_for(model_spec, classification)
        vectors = provider.embed([text], model_spec, "query")
        vector = vectors[0]
        return quant.quantize(vector) if self._quantize else vector
