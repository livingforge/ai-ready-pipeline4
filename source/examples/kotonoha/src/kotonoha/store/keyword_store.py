"""OpenSearch（BM25）側。

**SQL ではなく OpenSearch の問い合わせ本体を組む。** 日本語は kuromoji で
分かち書きするので、こちらから語に割って渡す必要はない —— ``terms`` を
空白で繋いで ``match`` へ渡す。型番の完全一致だけ ``match_phrase`` を使う。

別名（alias）の張り替えは ``_aliases`` の 1 回の呼び出しで**原子的に**行う。
2 回に分けると、その間に検索が空を引く瞬間ができる。
"""

from __future__ import annotations

from typing import Protocol

from kotonoha.search.keyword import BM25_B, BM25_K1, KeywordHit


class OpenSearchClient(Protocol):
    """OpenSearch とのやり取り。差し替え点。"""

    def search(self, index: str, body: dict) -> dict: ...
    def index_document(self, index: str, doc_id: str, body: dict) -> None: ...
    def delete_document(self, index: str, doc_id: str) -> None: ...
    def update_aliases(self, actions: list[dict]) -> None: ...


#: 索引の設定。BM25 の係数は既定値のまま（**調整していない**）。
INDEX_SETTINGS = {
    "settings": {
        "index": {
            "similarity": {
                "default": {"type": "BM25", "k1": BM25_K1, "b": BM25_B}
            },
            "analysis": {
                "analyzer": {
                    "ja": {"type": "custom", "tokenizer": "kuromoji_tokenizer",
                           "filter": ["kuromoji_baseform", "ja_stop",
                                      "kuromoji_number", "lowercase"]}
                }
            },
        }
    },
    "mappings": {
        "properties": {
            "chunk_id": {"type": "keyword"},
            "collection_id": {"type": "keyword"},
            "body": {"type": "text", "analyzer": "ja"},
            "heading_path": {"type": "text", "analyzer": "ja"},
        }
    },
}


class OpenSearchKeywordStore:
    """全文検索。"""

    def __init__(self, client: OpenSearchClient) -> None:
        self._client = client

    def search(self, index_name: str, terms: list[str], limit: int,
               chunk_ids: list[str] | None = None,
               phrase: str | None = None) -> list[KeywordHit]:
        if phrase:
            query: dict = {"match_phrase": {"body": phrase}}
        elif terms:
            query = {"match": {"body": {"query": " ".join(terms),
                                        "operator": "or"}}}
        else:
            return []

        body: dict = {"size": limit, "_source": ["chunk_id"]}
        if chunk_ids:
            body["query"] = {"bool": {
                "must": [query],
                "filter": [{"terms": {"chunk_id": chunk_ids}}],
            }}
        else:
            body["query"] = query

        result = self._client.search(index_name, body)
        return [
            KeywordHit(chunk_id=hit["_source"]["chunk_id"], score=hit["_score"])
            for hit in result.get("hits", {}).get("hits", [])
        ]

    def index(self, index_name: str, chunk_id: str, body: str) -> None:
        self._client.index_document(index_name, chunk_id,
                                    {"chunk_id": chunk_id, "body": body})

    def delete(self, index_name: str, chunk_id: str) -> None:
        self._client.delete_document(index_name, chunk_id)

    def alias(self, alias: str, index_name: str) -> None:
        """別名を張り替える。**1 回の呼び出しで原子的に。**"""
        self._client.update_aliases([
            {"remove": {"index": "*", "alias": alias}},
            {"add": {"index": index_name, "alias": alias}},
        ])
