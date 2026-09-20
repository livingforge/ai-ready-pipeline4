"""検索の値。"""

from __future__ import annotations

from dataclasses import dataclass, field


@dataclass
class SearchQuery:
    """検索の入力。"""

    text: str
    collection_id: str
    tenant_id: str
    top_k: int = 10
    #: メタデータの絞り込み（``{"部署": "品質保証部", "年度": "2026"}``）
    filters: dict = field(default_factory=dict)
    #: リランクを掛けるか。省略すると設定に従う
    rerank: bool | None = None
    #: 点数の内訳を返すか。調査用
    explain: bool = False


@dataclass
class Hit:
    """当たった 1 件。"""

    chunk_id: str
    document_id: str
    score: float
    body: str
    title: str = ""
    heading_path: str = ""
    seq_no: int = 0
    #: 当たった箇所を切り出したもの
    snippet: str = ""
    metadata: dict = field(default_factory=dict)
    #: 点数の内訳（``explain=True`` のときだけ）
    detail: dict | None = None


@dataclass
class SearchResult:
    """検索の結果。"""

    hits: list[Hit]
    total_candidates: int = 0
    elapsed_ms: int = 0
    reranked: bool = False
    #: 実際に使った検索の種類（``vector`` / ``keyword``）
    sources: list[str] = field(default_factory=list)

    def __len__(self) -> int:
        return len(self.hits)
