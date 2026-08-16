"""実行時の設定。環境変数で上書きできる。

**注意（設計文書との食い違い）**

``RATE_LIMIT_RPS`` は 100 だが、``docs/runbook/rate-limit.md`` は 60 rps と
書いている。2026/05 に品質保証部の取り込みが詰まったとき、Ingress 側の
制限と揃えないまま**アプリ側だけ**を 100 へ上げたのが残っている。
どちらが正しいのかは誰も確認していない（README の仕込み A3）。
"""

from __future__ import annotations

import os
from dataclasses import dataclass


def _int(name: str, default: int) -> int:
    raw = os.environ.get(name)
    return int(raw) if raw and raw.isdigit() else default


def _str(name: str, default: str) -> str:
    return os.environ.get(name) or default


def _bool(name: str, default: bool) -> bool:
    raw = os.environ.get(name)
    if raw is None:
        return default
    return raw.lower() in ("1", "true", "yes", "on")


@dataclass(frozen=True)
class Settings:
    """1 プロセスぶんの設定。"""

    # ── API ──────────────────────────────────────────────
    #: 1 テナントあたりの秒間要求数。★ runbook は 60 と書いている
    rate_limit_rps: int = 100
    #: /v1/embeddings が一度に受けるテキスト数
    max_embed_batch: int = 128
    #: 検索が返す最大件数
    max_top_k: int = 100

    # ── 取り込み ──────────────────────────────────────────
    #: 1 ジョブが受ける文書数
    max_documents_per_job: int = 1_000
    #: 取り込みワーカの並列数
    ingest_workers: int = 4

    # ── 埋め込み ──────────────────────────────────────────
    default_model: str = "voyage-4"
    #: 極秘（機密区分 30）で使う社内ホストのモデル
    internal_model: str = "voyage-4-nano"
    #: 埋め込みキャッシュの保持日数
    cache_ttl_days: int = 30
    #: int8 量子化を使うか。★ ADR-003 は量子化に触れていない
    quantize: bool = True

    # ── 検索 ─────────────────────────────────────────────
    #: リランクを掛けるか。★ ADR-005 は「第2次リリース」と書いている
    rerank_enabled: bool = True
    rerank_model: str = "rerank-2.5"
    #: リランクへ渡す候補数
    rerank_candidates: int = 50

    # ── ログ ─────────────────────────────────────────────
    log_level: str = "INFO"
    #: 監査ログの保持年数（機密区分 20 の保持期間に合わせた）
    audit_retention_years: int = 5

    @classmethod
    def from_env(cls) -> "Settings":
        """環境変数から組む。未設定は既定値。"""
        return cls(
            rate_limit_rps=_int("KOTONOHA_RATE_LIMIT_RPS", 100),
            max_embed_batch=_int("KOTONOHA_MAX_EMBED_BATCH", 128),
            max_top_k=_int("KOTONOHA_MAX_TOP_K", 100),
            max_documents_per_job=_int("KOTONOHA_MAX_DOCS_PER_JOB", 1_000),
            ingest_workers=_int("KOTONOHA_INGEST_WORKERS", 4),
            default_model=_str("KOTONOHA_DEFAULT_MODEL", "voyage-4"),
            internal_model=_str("KOTONOHA_INTERNAL_MODEL", "voyage-4-nano"),
            cache_ttl_days=_int("KOTONOHA_CACHE_TTL_DAYS", 30),
            quantize=_bool("KOTONOHA_QUANTIZE", True),
            rerank_enabled=_bool("KOTONOHA_RERANK", True),
            rerank_model=_str("KOTONOHA_RERANK_MODEL", "rerank-2.5"),
            rerank_candidates=_int("KOTONOHA_RERANK_CANDIDATES", 50),
            log_level=_str("KOTONOHA_LOG_LEVEL", "INFO"),
            audit_retention_years=_int("KOTONOHA_AUDIT_RETENTION_YEARS", 5),
        )


#: プロセス既定の設定。テストは ``Settings(...)`` を直に作って差し替える。
SETTINGS = Settings.from_env()
