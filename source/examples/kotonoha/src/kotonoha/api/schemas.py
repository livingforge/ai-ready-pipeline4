"""API の入出力の型。``openapi.yaml`` の ``components/schemas`` と対。

★ **同期する仕組みが無い。** 片方を直しても片方は直らない
（README の仕込み F1）。
"""

from __future__ import annotations

from kotonoha.framework.schema import Field, Schema


class EmbedRequestBody(Schema):
    """``POST /v1/embeddings``。"""

    #: 埋め込むテキスト。1〜128 件
    input: list[str] = Field(min_length=1, max_length=128)
    #: モデル名。省略するとテナントの既定
    model: str = Field(default=None, max_length=64)
    #: ``query`` か ``document``。**retrieval では必ず指定する**
    input_type: str = Field(default="document", choices=("query", "document"))


class CreateCollectionBody(Schema):
    """``POST /v1/collections``。"""

    name: str = Field(min_length=1, max_length=128)
    #: 継承より**厳しい**区分にしたいときだけ指定する
    classification: str = Field(default=None, choices=("10", "20", "30"))
    model: str = Field(default=None, max_length=64)


class DocumentBody(Schema):
    """取り込む 1 件。"""

    content: str = Field(min_length=1)
    external_id: str = Field(default=None, max_length=256)
    title: str = Field(default="", max_length=512)
    content_type: str = Field(default="text/plain", max_length=64)
    metadata: dict = Field(default=None)


class IngestBody(Schema):
    """``POST /v1/collections/{id}/documents``。"""

    documents: list[dict] = Field(min_length=1, max_length=1000)


class SearchBody(Schema):
    """``POST /v1/collections/{id}/search``。"""

    query: str = Field(min_length=1, max_length=2000)
    top_k: int = Field(default=10, ge=1, le=100)
    filters: dict = Field(default=None)
    #: リランクを掛けるか。省略すると設定に従う
    rerank: bool = Field(default=None)
    #: 点数の内訳を返すか。**調査用**
    explain: bool = Field(default=False)


def hit_to_dict(hit) -> dict:
    """検索結果 1 件を応答の形へ。"""
    body = {
        "chunk_id": hit.chunk_id,
        "document_id": hit.document_id,
        "score": hit.score,
        "title": hit.title,
        "heading_path": hit.heading_path,
        "seq_no": hit.seq_no,
        "snippet": hit.snippet,
        "metadata": hit.metadata,
    }
    if hit.detail is not None:
        body["explain"] = hit.detail
    return body


def job_to_dict(job) -> dict:
    """ジョブを応答の形へ。"""
    return {
        "job_id": job.job_id,
        "status": job.status,
        "total": job.total_count,
        "done": job.done_count,
        "failed": job.failed_count,
        "chunks": job.chunk_count,
        "cached_chunks": job.cached_count,
        "progress": round(job.progress, 4),
        "queued_at": job.queued_at,
        "finished_at": job.finished_at,
        "error": job.error_message,
    }


def collection_to_dict(collection) -> dict:
    return {
        "collection_id": collection.collection_id,
        "name": collection.collection_name,
        "classification": collection.classification,
        "model": collection.embed_model,
        "dimension": collection.embed_dim,
        "chunk_count": collection.chunk_count,
        "status": collection.status,
        "created_at": collection.created_at,
    }
