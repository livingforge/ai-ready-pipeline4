"""``t_embedding`` と pgvector。

★ **量子化した列（``vec_i8``）を読み書きする。** ADR-003 は float の
``vec`` しか想定していないので、この SQL は ADR と食い違っている
（README の仕込み B1）。``quantized`` が真の行は ``vec`` が NULL である。

距離はコサイン（``<=>``）。Voyage のベクトルは長さ 1 に正規化されている
ので内積でもよいが、量子化すると長さが崩れるためコサインで統一した ——
**この理由はどこにも書かれていない。**
"""

from __future__ import annotations

from kotonoha.embed.models import Vector
from kotonoha.embed.quantize import from_bytes, to_bytes
from kotonoha.search.vector import VectorHit
from kotonoha.store.connection import Connection, placeholders

#: 検索。``<=>`` はコサイン距離（0 が同じ、2 が正反対）。
SEARCH_VECTORS = """
SELECT e.chunk_id, 1 - (e.vec <=> %s::vector) AS similarity
FROM t_embedding e
WHERE e.index_name = %s
ORDER BY e.vec <=> %s::vector
LIMIT %s
"""

#: 候補を絞ってから検索する（メタデータの絞り込みが効いているとき）。
SEARCH_VECTORS_SCOPED = """
SELECT e.chunk_id, 1 - (e.vec <=> %s::vector) AS similarity
FROM t_embedding e
WHERE e.index_name = %s AND e.chunk_id IN ({ids})
ORDER BY e.vec <=> %s::vector
LIMIT %s
"""

#: HNSW の探索幅。**検索の直前にセッションへ設定する。**
SET_EF_SEARCH = "SET LOCAL hnsw.ef_search = %s"

UPSERT_EMBEDDING = """
INSERT INTO t_embedding (chunk_id, index_name, embed_model, vec, vec_i8, quantized)
VALUES (%s, %s, %s, %s, %s, %s)
ON CONFLICT (chunk_id, index_name) DO UPDATE SET
    embed_model = EXCLUDED.embed_model,
    vec         = EXCLUDED.vec,
    vec_i8      = EXCLUDED.vec_i8,
    quantized   = EXCLUDED.quantized
"""

SELECT_EMBEDDING = """
SELECT chunk_id, embed_model, vec, vec_i8, quantized
FROM t_embedding WHERE chunk_id = %s AND index_name = %s
"""

DELETE_BY_CHUNK = "DELETE FROM t_embedding WHERE chunk_id = %s"

DELETE_INDEX = "DELETE FROM t_embedding WHERE index_name = %s"

COUNT_IN_INDEX = "SELECT COUNT(*) AS n FROM t_embedding WHERE index_name = %s"


class SqlVectorStore:
    """pgvector 側。"""

    def __init__(self, conn: Connection, quantize: bool = True) -> None:
        self._conn = conn
        self._quantize = quantize

    def search(self, index_name: str, vector: Vector, limit: int,
               chunk_ids: list[str] | None = None,
               ef_search: int = 100) -> list[VectorHit]:
        self._conn.execute(SET_EF_SEARCH, (ef_search,))
        literal = _to_literal(vector)
        if chunk_ids:
            sql = SEARCH_VECTORS_SCOPED.format(ids=placeholders(len(chunk_ids)))
            params = [literal, index_name, *chunk_ids, literal, limit]
        else:
            sql = SEARCH_VECTORS
            params = [literal, index_name, literal, limit]
        rows = self._conn.fetch_all(sql, params)
        return [VectorHit(chunk_id=r["chunk_id"],
                          similarity=float(r["similarity"])) for r in rows]

    def upsert(self, index_name: str, chunk_id: str, vector: Vector) -> None:
        self._conn.execute(UPSERT_EMBEDDING,
                           _upsert_params(chunk_id, index_name, vector))

    def save(self, chunk_id: str, index_name: str, vector: Vector) -> None:
        self.upsert(index_name, chunk_id, vector)

    def save_many(self, index_name: str, items: list[tuple[str, Vector]]) -> None:
        self._conn.execute_many(UPSERT_EMBEDDING, [
            _upsert_params(chunk_id, index_name, vector)
            for chunk_id, vector in items
        ])

    def find(self, chunk_id: str, index_name: str) -> Vector | None:
        row = self._conn.fetch_one(SELECT_EMBEDDING, (chunk_id, index_name))
        if row is None:
            return None
        if row["quantized"]:
            return from_bytes(row["vec_i8"], 1.0 / 127, row["embed_model"])
        return Vector(values=list(row["vec"]), model=row["embed_model"])

    def delete(self, index_name: str, chunk_id: str) -> None:
        self.delete_by_chunk(chunk_id)

    def delete_by_chunk(self, chunk_id: str) -> int:
        self._conn.execute(DELETE_BY_CHUNK, (chunk_id,))
        return 0

    def delete_index(self, index_name: str) -> int:
        self._conn.execute(DELETE_INDEX, (index_name,))
        return 0

    def count_in_index(self, index_name: str) -> int:
        row = self._conn.fetch_one(COUNT_IN_INDEX, (index_name,))
        return int(row["n"]) if row else 0


def _upsert_params(chunk_id: str, index_name: str, vector: Vector):
    if vector.quantized:
        return (chunk_id, index_name, vector.model, None,
                to_bytes(vector), True)
    return (chunk_id, index_name, vector.model, _to_literal(vector), None, False)


def _to_literal(vector: Vector) -> str:
    """pgvector のリテラル（``[0.1,0.2,...]``）。"""
    return "[" + ",".join(f"{v:.6f}" for v in vector.values) + "]"
