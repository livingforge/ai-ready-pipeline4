"""埋め込みの値。"""

from __future__ import annotations

from dataclasses import dataclass, field


@dataclass(frozen=True)
class EmbedModel:
    """使えるモデル 1 つぶん。``registry`` が持つ。"""

    name: str
    dim: int
    max_tokens: int
    route: str                    # external / internal
    #: 検索語と文書で前置きを変えるモデルか（``input_type`` を持つか）
    typed_input: bool = True
    note: str = ""

    @property
    def external(self) -> bool:
        return self.route == "external"


@dataclass
class Vector:
    """1 本のベクトル。

    ``values`` は float の列。量子化済みなら ``quantized`` が真になり、
    ``codes`` に int8 の列が入る（``values`` は復元値）。
    """

    values: list[float]
    model: str
    quantized: bool = False
    codes: list[int] = field(default_factory=list)
    scale: float = 1.0

    @property
    def dim(self) -> int:
        return len(self.values)


@dataclass
class EmbedResult:
    """1 回の呼び出しの結果。"""

    vectors: list[Vector]
    model: str
    route: str
    #: 実際に提供元へ投げた件数（キャッシュに当たった分は含まない）
    billed_count: int = 0
    cached_count: int = 0
    elapsed_ms: int = 0

    def __len__(self) -> int:
        return len(self.vectors)


@dataclass
class EmbedRequest:
    """呼び出しの入力。"""

    texts: list[str]
    model: str
    classification: str
    #: ``query`` か ``document``。retrieval では必ず指定する
    input_type: str = "document"
    tenant_id: str = ""
