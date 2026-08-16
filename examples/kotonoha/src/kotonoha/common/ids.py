"""識別子の採番。

テナント識別子だけは人が決める（利用申請の時に部門と相談して付ける）。
それ以外はここで機械が振る。
"""

from __future__ import annotations

import re
import uuid

#: テナント識別子の形。小文字英数とハイフンで 3〜32 文字。
TENANT_ID = re.compile(r"^[a-z][a-z0-9-]{2,31}$")

#: コレクション名の形。テナント内で一意。
COLLECTION_NAME = re.compile(r"^[a-zA-Z0-9][a-zA-Z0-9_-]{0,127}$")


def new_id() -> str:
    """UUID v4。文書・チャンク・コレクション・ジョブに使う。"""
    return str(uuid.uuid4())


def new_key_id() -> str:
    """API キーの識別子。UUID の先頭 8 桁に接頭辞を付ける。"""
    return "key_" + uuid.uuid4().hex[:8]


def new_api_key() -> str:
    """API キーの平文。**発行時に一度だけ返し、以後は保持しない。**"""
    return "kot_" + uuid.uuid4().hex + uuid.uuid4().hex[:8]


def index_name(collection_id: str, generation: int) -> str:
    """実インデックス名。別名（alias）とは別。

    再インデックスは世代を 1 つ進めた名前で作り、出来上がったら別名を
    張り替える（``reindex.switch``）。
    """
    return f"idx_{collection_id[:8]}_v{generation:03d}"


def index_alias(collection_id: str) -> str:
    """別名。検索は常にこちらを見る。"""
    return f"col_{collection_id[:8]}"


def valid_tenant_id(value: str) -> bool:
    return bool(TENANT_ID.match(value))


def valid_collection_name(value: str) -> bool:
    return bool(COLLECTION_NAME.match(value))
