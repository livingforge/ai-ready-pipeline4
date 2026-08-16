"""API キーの発行・失効・照合。

**平文は発行時に一度だけ返し、以後どこにも持たない。**
照合の失敗理由は返さない（有効なキーの存在を探れてしまう）。
"""

from __future__ import annotations

import pytest

from kotonoha.common.clock import freeze, now
from kotonoha.common.errors import NotFound
from kotonoha.common.hashing import sha256_text


def test_発行すると平文が返る(services):
    issued = services.apikeys.issue("cs-support", label="試験用")
    assert issued.secret.startswith("kot_")
    assert issued.key.label == "試験用"


def test_保存されるのはハッシュだけ(services):
    issued = services.apikeys.issue("cs-support")
    assert issued.key.key_hash == sha256_text(issued.secret)
    assert issued.secret not in issued.key.key_hash


def test_発行した鍵で照合できる(services):
    issued = services.apikeys.issue("cs-support")
    found = services.apikeys.authenticate(issued.secret)
    assert found is not None
    assert found.tenant_id == "cs-support"


def test_知らない鍵は通らない(services):
    assert services.apikeys.authenticate("kot_dummy") is None


def test_空の鍵は通らない(services):
    assert services.apikeys.authenticate("") is None


def test_失効させると通らなくなる(services):
    issued = services.apikeys.issue("cs-support")
    services.apikeys.revoke(issued.key.key_id)
    assert services.apikeys.authenticate(issued.secret) is None


def test_失効しても行は残る(services):
    """**監査ログから辿れるようにする。**"""
    issued = services.apikeys.issue("cs-support")
    services.apikeys.revoke(issued.key.key_id)
    assert services.apikeys.authenticate(issued.secret) is None
    assert issued.key.revoked_at is not None


def test_知らない鍵の失効は弾かれる(services):
    with pytest.raises(NotFound):
        services.apikeys.revoke("key_nothing")


def test_期限が切れると通らない(services):
    from datetime import timedelta
    issued = services.apikeys.issue("cs-support", valid_days=1)
    assert services.apikeys.authenticate(issued.secret) is not None
    freeze(now() + timedelta(days=2))
    assert services.apikeys.authenticate(issued.secret) is None


def test_期限を付けなければ無期限(services):
    issued = services.apikeys.issue("cs-support")
    assert issued.key.expires_at is None
    assert issued.key.usable()


def test_最終使用日時が記録される(services):
    issued = services.apikeys.issue("cs-support")
    assert issued.key.last_used_at is None
    services.apikeys.authenticate(issued.secret)
    assert issued.key.last_used_at is not None


def test_テナントごとに引ける(services):
    services.apikeys.issue("cs-support", label="2 本目")
    keys = services.apikeys._keys.list_by_tenant("cs-support")
    assert len(keys) == 2
    assert all(k.tenant_id == "cs-support" for k in keys)
