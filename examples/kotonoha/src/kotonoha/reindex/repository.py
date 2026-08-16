"""再インデックス側の保存先の約束。"""

from __future__ import annotations

from typing import Protocol

from kotonoha.reindex.models import ReindexJob


class ReindexRepository(Protocol):
    """``t_reindex_job``。"""

    def find(self, job_id: str) -> ReindexJob | None: ...
    def save(self, job: ReindexJob) -> None: ...
    def find_running(self, collection_id: str) -> ReindexJob | None:
        """走っている（``building`` / ``verifying``）ものがあれば返す。"""
        ...
    def latest_switched(self, collection_id: str) -> ReindexJob | None:
        """最後に張り替えたもの。いまの実インデックス名が分かる。"""
        ...
    def next_generation(self, collection_id: str) -> int:
        """次の世代番号。``idx_xxxxxxxx_vNNN`` の NNN。"""
        ...
    def list_by_collection(self, collection_id: str) -> list[ReindexJob]: ...
    def list_droppable(self) -> list[ReindexJob]:
        """保持期間を過ぎて旧を消せるもの。日次のバッチが読む。"""
        ...
