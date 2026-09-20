"""API キーの発行と照合。

平文は**発行時に一度だけ**返し、以後どこにも持たない。照合は SHA-256 の
突き合わせで、比較は定数時間で行う。
"""

from __future__ import annotations

import hmac
from dataclasses import dataclass

from kotonoha.common import ids
from kotonoha.common.clock import expires_in, now
from kotonoha.common.errors import NotFound
from kotonoha.common.hashing import sha256_text
from kotonoha.tenant.models import ApiKey


@dataclass
class IssuedKey:
    """発行結果。``secret`` はこの瞬間にしか手に入らない。"""

    key: ApiKey
    secret: str


class ApiKeyService:
    """キーの発行・失効・照合。"""

    def __init__(self, key_repo) -> None:
        self._keys = key_repo

    def issue(self, tenant_id: str, *, label: str = "",
              valid_days: int | None = None) -> IssuedKey:
        """新しいキーを発行する。平文は戻り値にしか入らない。"""
        secret = ids.new_api_key()
        key = ApiKey(
            key_id=ids.new_key_id(),
            tenant_id=tenant_id,
            key_hash=sha256_text(secret),
            label=label,
            expires_at=expires_in(valid_days) if valid_days else None,
        )
        self._keys.save(key)
        return IssuedKey(key=key, secret=secret)

    def revoke(self, key_id: str) -> ApiKey:
        """キーを失効させる。**行は消さない**（監査ログから辿れるように）。

        :raises NotFound: そのキーが無い
        """
        key = self._keys.find(key_id)
        if key is None:
            raise NotFound(f"API キーがありません: {key_id}", key_id=key_id)
        key.revoked_at = now()
        self._keys.save(key)
        return key

    def authenticate(self, secret: str) -> ApiKey | None:
        """平文から使えるキーを引く。無ければ ``None``。

        **理由を返り値で区別しない** —— 「無い」と「失効している」を
        呼び出し側へ伝えると、有効なキーの存在を探れてしまう。
        """
        if not secret:
            return None
        digest = sha256_text(secret)
        for key in self._keys.all():
            if hmac.compare_digest(key.key_hash, digest) and key.usable():
                key.last_used_at = now()
                self._keys.save(key)
                return key
        return None
