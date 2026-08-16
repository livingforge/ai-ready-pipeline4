"""``/v1/collections/{id}/documents`` —— 取り込みと削除。

取り込みは**受付だけ返す**（202）。実際の処理はワーカが非同期に回すので、
進捗は ``/v1/jobs/{job_id}`` で引く。
"""

from __future__ import annotations

from kotonoha.api.errors import guard
from kotonoha.api.schemas import DocumentBody, IngestBody, job_to_dict
from kotonoha.framework.routing import Request, Response, Router
from kotonoha.ingest.models import SourceDocument


def register(ingest_service, document_repo, chunk_repo, embedding_repo,
             collection_service) -> Router:
    """依存を挿して経路を組み立てる。"""
    router = Router(prefix="/v1")

    @router.post("/collections/{collection_id}/documents",
                 summary="文書を取り込む（非同期）")
    @guard
    def ingest_documents(request: Request) -> Response:
        """受け付けてジョブを返す。**まだ処理していない。**"""
        body = IngestBody(**request.body)
        sources = [_to_source(item) for item in body.documents]
        job = ingest_service.submit(
            request.tenant_id, request.path_params["collection_id"], sources)
        return Response(status=202, body=job_to_dict(job),
                        headers={"Location": f"/v1/jobs/{job.job_id}"})

    @router.delete("/collections/{collection_id}/documents/{document_id}",
                   summary="文書を削除する")
    @guard
    def delete_document(request: Request) -> Response:
        """文書とそのチャンク・ベクトルを消す。

        **文書は論理削除、チャンクとベクトルは物理削除。** 文書の行を
        残すのは監査ログから辿れるようにするため、チャンクを消すのは
        検索に出続けると困るため。
        """
        collection = collection_service.get(
            request.path_params["collection_id"], tenant_id=request.tenant_id)
        document_id = request.path_params["document_id"]

        document = document_repo.find(document_id)
        if document is None or document.collection_id != collection.collection_id:
            from kotonoha.common.errors import NotFound
            raise NotFound(f"文書がありません: {document_id}", document_id=document_id)

        for chunk in chunk_repo.list_by_document(document_id):
            embedding_repo.delete_by_chunk(chunk.chunk_id)
        chunk_repo.delete_by_document(document_id)
        document_repo.delete(document_id)

        return Response(status=204, body=None)

    return router


def _to_source(item: dict) -> SourceDocument:
    body = DocumentBody(**item)
    return SourceDocument(
        external_id=body.external_id,
        title=body.title,
        content=body.content,
        content_type=body.content_type,
        metadata=body.metadata or {},
    )
