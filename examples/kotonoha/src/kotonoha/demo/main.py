"""20 シナリオを流す。``python -m kotonoha.demo.main``

**設計文書に対応する成果物ではない。** 資材が動くことを確かめるための
足場である。仕込んだ食い違い（README の A〜F）が実際にどう出るかを
最後にまとめて表示する。
"""

from __future__ import annotations

import sys

from kotonoha.common import logging as applog
from kotonoha.common.clock import year_month
from kotonoha.common.errors import (ClassificationViolation, KotonohaError,
                                    QuotaExceeded)
from kotonoha.demo import fixtures
from kotonoha.demo.wiring import build
from kotonoha.embed import quantize as quant
from kotonoha.embed.models import EmbedRequest
from kotonoha.ingest.chunker import OVERLAP_TOKENS, TARGET_TOKENS, chunk_text
from kotonoha.search.fusion import RRF_K
from kotonoha.search.models import SearchQuery
from kotonoha.tenant.quota import DEFAULT_QUOTA

log = applog.get(__name__)


def line(no: int, title: str) -> None:
    print(f"\n── {no:2d}. {title} " + "─" * max(0, 56 - len(title)))


def main(argv: list[str] | None = None) -> int:
    applog.setup("WARNING")
    services = build()
    secrets = fixtures.install(services)
    print(f"Kotonoha デモ: テナント {len(secrets)} 件を登録しました")

    line(1, "テナントの一覧")
    for tenant in services.tenants.list_active():
        print(f"  {tenant.tenant_id:16s} {tenant.department:12s} "
              f"区分={tenant.classification} 上限={tenant.monthly_quota:,} "
              f"モデル={tenant.embed_model}")

    line(2, "コレクションを作る（一般・社外秘・極秘）")
    manual = services.collections.create("cs-support", "maintenance-manual")
    defect = services.collections.create("qa-defect", "defect-report")
    contract = services.collections.create("legal-contract", "contract-clause")
    for col in (manual, defect, contract):
        print(f"  {col.collection_name:20s} 区分={col.classification} "
              f"モデル={col.embed_model} 次元={col.embed_dim}")

    line(3, "極秘テナントは外部モデルを指定できない")
    try:
        services.collections.create("legal-contract", "should-fail",
                                    embed_model="voyage-law-2")
    except ClassificationViolation as exc:
        print(f"  弾きました: {exc.message}")

    line(4, "機密区分は下げられない")
    try:
        services.collections.create("legal-contract", "should-fail-2",
                                    classification="10")
    except ClassificationViolation as exc:
        print(f"  弾きました: {exc.message}")

    line(5, "保守マニュアルを取り込む")
    job = services.ingest.submit("cs-support", manual.collection_id,
                                 fixtures.MANUAL_DOCS)
    services.worker.drain()
    job = services.tracker.get(job.job_id)
    print(f"  job={job.job_id[:8]} status={job.status} "
          f"成功={job.done_count} チャンク={job.chunk_count}")

    line(6, "不具合報告（CSV）を取り込む")
    services.ingest.submit("qa-defect", defect.collection_id,
                           [fixtures.DEFECT_CSV])
    services.worker.drain()
    print(f"  チャンク数={services.chunks.count_in_collection(defect.collection_id)}")

    line(7, "契約書（極秘）を取り込む —— 社内 GPU で埋め込む")
    services.ingest.submit("legal-contract", contract.collection_id,
                           fixtures.CONTRACT_DOCS)
    services.worker.drain()
    print(f"  チャンク数={services.chunks.count_in_collection(contract.collection_id)}")

    line(8, "検索する（ハイブリッド）")
    result = services.search.search(SearchQuery(
        text="異音がするときの対処", collection_id=manual.collection_id,
        tenant_id="cs-support", top_k=3))
    for hit in result.hits:
        print(f"  {hit.score:.5f} [{hit.heading_path}] {hit.snippet[:44]}…")
    print(f"  候補={result.total_candidates} rerank={result.reranked} "
          f"経路={result.sources}")

    line(9, "型番で検索する（全文検索が効く）")
    result = services.search.search(SearchQuery(
        text='"A-2210"', collection_id=defect.collection_id,
        tenant_id="qa-defect", top_k=3))
    print(f"  当たり={len(result.hits)} 件")
    for hit in result.hits[:2]:
        print(f"    {hit.snippet[:52]}…")

    line(10, "点数の内訳（explain）")
    result = services.search.search(SearchQuery(
        text="潤滑油の交換", collection_id=manual.collection_id,
        tenant_id="cs-support", top_k=1, explain=True))
    if result.hits and result.hits[0].detail:
        detail = result.hits[0].detail
        print(f"  score={detail['score']} k={detail['k']}")
        print(f"  内訳={detail['contributions']}")

    line(11, "メタデータで絞り込む")
    result = services.search.search(SearchQuery(
        text="異音", collection_id=manual.collection_id,
        tenant_id="cs-support", top_k=5, filters={"年度": "2026"}))
    print(f"  2026 年度に絞ると {len(result.hits)} 件")

    line(12, "他テナントのコレクションは引けない（404 と同じ扱い）")
    try:
        services.search.search(SearchQuery(
            text="契約", collection_id=contract.collection_id,
            tenant_id="cs-support", top_k=3))
    except KotonohaError as exc:
        print(f"  弾きました: {exc.code}")

    line(13, "同じ文書を入れ直すと飛ぶ（dedupe）")
    before = services.chunks.count_in_collection(manual.collection_id)
    services.ingest.submit("cs-support", manual.collection_id,
                           fixtures.MANUAL_DOCS)
    services.worker.drain()
    after = services.chunks.count_in_collection(manual.collection_id)
    print(f"  チャンク数 {before} -> {after}（増えないのが正しい）")

    line(14, "埋め込みキャッシュ")
    request = EmbedRequest(texts=["軸受の交換手順"], model="voyage-4",
                           classification="10", tenant_id="cs-support")
    first = services.embed.embed(request)
    second = services.embed.embed(request)
    print(f"  1 回目: 課金={first.billed_count} 蓄積={first.cached_count}")
    print(f"  2 回目: 課金={second.billed_count} 蓄積={second.cached_count}")

    line(15, "上限を超える取り込みは受け付けない")
    services.tenants.change_quota("sales-proposal", 1)
    proposal = services.collections.create("sales-proposal", "proposal")
    try:
        services.ingest.submit("sales-proposal", proposal.collection_id,
                               fixtures.MANUAL_DOCS)
    except QuotaExceeded as exc:
        print(f"  弾きました: {exc.message}")

    line(16, "利用量と上限")
    for tenant_id in ("cs-support", "qa-defect", "legal-contract"):
        tenant = services.tenants.get(tenant_id)
        status = services.quota.status(tenant)
        print(f"  {tenant_id:16s} {status.used:>6,} / {status.quota:>9,} "
              f"({status.ratio:.1%})")

    line(17, "月次締めと請求内訳")
    closed = services.close.run(year_month())
    for invoice in closed.invoices:
        for label, quantity, unit, amount in invoice.lines:
            print(f"  {invoice.tenant_id:16s} {label:22s} "
                  f"{quantity:>6,} × {unit} = {amount:>6,} 円")
    print(f"  合計 {closed.total_yen:,} 円")
    print("  ※ 円未満は切り捨て。端数は基盤側（AI基盤グループ）が持つ")
    print("     —— この決めごとは docs/runbook/billing.md にしかない")

    line(18, "監査ログ —— 極秘が外部へ出ていないこと")
    entries = services.audit.entries
    secret_calls = [e for e in entries if e.classification == "30"]
    print(f"  記録={len(entries)} 件 うち極秘={len(secret_calls)} 件")
    print(f"  極秘が外部経路へ出た件数={len(services.audit.violations())}（0 が正しい）")
    print(f"  本文を持つ記録={sum(1 for e in entries if getattr(e, 'body', None))} 件")

    line(19, "再インデックスの見積り")
    plan = services.reindex.plan(manual.collection_id, "voyage-4-large")
    print(f"  {plan.from_model} -> {plan.to_model} "
          f"チャンク={plan.total_chunks} 呼び出し={plan.estimated_calls} "
          f"見積={plan.estimated_minutes} 分")
    for warning in plan.warnings:
        print(f"    ※ {warning}")

    line(20, "仕込んだ食い違い（README の期待値と対）")
    pieces = chunk_text(fixtures.MANUAL_DOCS[0].content)
    sample = services.embed.embed(EmbedRequest(
        texts=["量子化の確認"], model="voyage-4", classification="10"))
    vector = sample.vectors[0]
    print(f"  A1 チャンク分割 target={TARGET_TOKENS} overlap={OVERLAP_TOKENS} "
          f"→ {len(pieces)} 個（設計文書に記載なし）")
    print(f"  A2 RRF の定数 k={RRF_K}（設計文書に記載なし）")
    print(f"  B1 量子化 quantized={vector.quantized}"
          f"（ADR-003 は 1024 次元 float のまま）")
    print(f"  B2 リランク 実装済み（ADR-005 は第2次リリースと記載）")
    print(f"  C1 法務部の上限 台帳=100,000 / 実装の既定={DEFAULT_QUOTA:,}")
    print(f"  C2 監査ログ 表に本文の列なし・アプリログの DEBUG に本文あり")

    print("\n完了しました。")
    return 0


if __name__ == "__main__":
    sys.exit(main())
