"""原文の置き場（S3 互換・社内）。

**原文はデータベースに入れない。** 1 文書が数 MB になることがあり、
``t_document`` には位置（``source_uri``）だけを持つ。

★ **機密区分ごとにバケットを分けている。** 極秘の原文は別バケットで、
   別の鍵で暗号化されている。この分離は情報セキュリティ点検表の指摘に
   対する回答だが、**どの設計文書にも書かれていない** —— バケット名の
   規則がここにしか無い。
"""

from __future__ import annotations

from typing import Protocol

from kotonoha.common.hashing import sha256_bytes
from kotonoha.tenant import classification as cls

#: 機密区分ごとのバケット。★ この対応表はここにしか無い。
BUCKETS = {
    cls.GENERAL: "kotonoha-source-general",
    cls.CONFIDENTIAL: "kotonoha-source-confidential",
    cls.SECRET: "kotonoha-source-secret",       # 別の鍵で暗号化
}


class BlobClient(Protocol):
    """オブジェクトストアとのやり取り。差し替え点。"""

    def put(self, bucket: str, key: str, data: bytes,
            content_type: str) -> None: ...
    def get(self, bucket: str, key: str) -> bytes: ...
    def delete(self, bucket: str, key: str) -> None: ...


class ObjectStore:
    """原文を置く・取る。"""

    def __init__(self, client: BlobClient, classification: str) -> None:
        self._client = client
        self._classification = classification
        self._bucket = BUCKETS[classification]

    def put(self, data: bytes, *, content_type: str = "text/plain") -> str:
        """置いて ``s3://`` の位置を返す。

        鍵は中身のハッシュ。**同じ中身は 1 つしか置かない**ので、
        全件同期で毎晩同じものが送られても増えない。
        """
        digest = sha256_bytes(data)
        key = f"{digest[:2]}/{digest[2:4]}/{digest}"
        self._client.put(self._bucket, key, data, content_type)
        return f"s3://{self._bucket}/{key}"

    def get(self, uri: str) -> bytes:
        """取り出す。

        :raises ValueError: 自分の区分のバケットでない位置を指している
        """
        bucket, key = parse_uri(uri)
        if bucket != self._bucket:
            raise ValueError(
                f"機密区分 {self._classification} からは {bucket} を読めません")
        return self._client.get(bucket, key)

    def delete(self, uri: str) -> None:
        bucket, key = parse_uri(uri)
        self._client.delete(bucket, key)


def parse_uri(uri: str) -> tuple[str, str]:
    """``s3://bucket/key`` を割る。"""
    if not uri.startswith("s3://"):
        raise ValueError(f"位置の形が不正です: {uri}")
    rest = uri[len("s3://"):]
    bucket, _, key = rest.partition("/")
    return bucket, key
