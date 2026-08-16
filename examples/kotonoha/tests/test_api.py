"""HTTP の受け口。

★ **正本は ``openapi.yaml``** だが、arp4 は ``.yaml`` を読まない
   （README の仕込み F1）。この検証と ``api/schemas.py`` が、機械が
   読める唯一の API 仕様になっている。
"""

from __future__ import annotations

import pytest

from kotonoha.api.app import build_app
from kotonoha.framework.routing import Request


@pytest.fixture
def api(services):
    app = build_app(services)

    def call(method: str, path: str, body: dict | None = None,
             *, tenant: str | None = "cs-support", query: dict | None = None):
        headers = {}
        if tenant:
            headers["authorization"] = f"Bearer {services.secrets[tenant]}"
        return app.handle(Request(method=method, path=path, headers=headers,
                                  query=query or {}, body=body or {}))

    call.app = app
    call.services = services
    return call


# ── 死活監視 ─────────────────────────────────────────────────────
def test_healthzは認証なしで通る(api):
    response = api("GET", "/healthz", tenant=None)
    assert response.status == 200
    assert response.body["status"] == "ok"


def test_readyzも認証なしで通る(api):
    assert api("GET", "/readyz", tenant=None).status == 200


# ── 認証 ─────────────────────────────────────────────────────────
def test_鍵が無いと401(api):
    assert api("GET", "/v1/collections", tenant=None).status == 401


def test_知らない経路は404(api):
    assert api("GET", "/v1/nothing").status == 404


def test_使えないメソッドは405(api):
    assert api("GET", "/v1/embeddings").status == 405


def test_停止中のテナントは401(api):
    """**403 にしない** —— テナントの実在が分かってしまう。"""
    api.services.tenants.suspend("cs-support", "試験")
    assert api("GET", "/v1/collections").status == 401


# ── コレクション ─────────────────────────────────────────────────
def test_作れる(api):
    response = api("POST", "/v1/collections", {"name": "manual"})
    assert response.status == 201
    assert response.body["classification"] == "10"


def test_名前が無いと400(api):
    response = api("POST", "/v1/collections", {})
    assert response.status == 400
    assert response.body["error"]["code"] == "invalid_request"


def test_知らない項目は400(api):
    assert api("POST", "/v1/collections",
               {"name": "a", "nope": 1}).status == 400


def test_同じ名前は409(api):
    api("POST", "/v1/collections", {"name": "manual"})
    assert api("POST", "/v1/collections", {"name": "manual"}).status == 409


def test_区分を下げると403(api):
    response = api("POST", "/v1/collections",
                   {"name": "c", "classification": "10"}, tenant="legal-contract")
    assert response.status == 403
    assert response.body["error"]["code"] == "classification_violation"


def test_一覧が引ける(api):
    api("POST", "/v1/collections", {"name": "a"})
    response = api("GET", "/v1/collections")
    assert response.status == 200
    assert len(response.body["data"]) == 1


def test_他テナントのものは404(api):
    created = api("POST", "/v1/collections", {"name": "a"}).body
    response = api("GET", f"/v1/collections/{created['collection_id']}",
                   tenant="qa-defect")
    assert response.status == 404


# ── エンベディング ───────────────────────────────────────────────
def test_ベクトルが返る(api):
    response = api("POST", "/v1/embeddings", {"input": ["点検", "手順"]})
    assert response.status == 200
    assert len(response.body["data"]) == 2
    assert response.body["usage"]["billed"] == 2


def test_2回目は蓄積になる(api):
    api("POST", "/v1/embeddings", {"input": ["同じ本文"]})
    response = api("POST", "/v1/embeddings", {"input": ["同じ本文"]})
    assert response.body["usage"]["cached"] == 1


def test_128件を超えると400(api):
    response = api("POST", "/v1/embeddings",
                   {"input": [f"文書{i}" for i in range(129)]})
    assert response.status == 400


def test_input_typeが不正だと400(api):
    assert api("POST", "/v1/embeddings",
               {"input": ["a"], "input_type": "both"}).status == 400


def test_極秘に外部モデルを指定すると403(api):
    response = api("POST", "/v1/embeddings",
                   {"input": ["条項"], "model": "voyage-law-2"},
                   tenant="legal-contract")
    assert response.status == 403


# ── 取り込み ─────────────────────────────────────────────────────
def test_取り込みは202で受け付ける(api):
    created = api("POST", "/v1/collections", {"name": "manual"}).body
    response = api("POST",
                   f"/v1/collections/{created['collection_id']}/documents",
                   {"documents": [{"content": "点検の手順。", "external_id": "D1"}]})
    assert response.status == 202
    assert response.body["status"] == "queued"
    assert response.headers["Location"].startswith("/v1/jobs/")


def test_文書が空だと400(api):
    created = api("POST", "/v1/collections", {"name": "manual"}).body
    assert api("POST", f"/v1/collections/{created['collection_id']}/documents",
               {"documents": []}).status == 400


def test_進捗が引ける(api):
    created = api("POST", "/v1/collections", {"name": "manual"}).body
    job = api("POST", f"/v1/collections/{created['collection_id']}/documents",
              {"documents": [{"content": "点検の手順。"}]}).body
    api.services.worker.drain()
    response = api("GET", f"/v1/jobs/{job['job_id']}")
    assert response.status == 200
    assert response.body["status"] == "succeeded"
    assert response.body["failures"] == []


def test_他テナントの進捗は404(api):
    created = api("POST", "/v1/collections", {"name": "manual"}).body
    job = api("POST", f"/v1/collections/{created['collection_id']}/documents",
              {"documents": [{"content": "点検。"}]}).body
    assert api("GET", f"/v1/jobs/{job['job_id']}", tenant="qa-defect").status == 404


def test_失敗の明細が返る(api):
    created = api("POST", "/v1/collections", {"name": "manual"}).body
    job = api("POST", f"/v1/collections/{created['collection_id']}/documents",
              {"documents": [{"content": "x", "content_type": "application/zip",
                              "external_id": "NG"}]}).body
    api.services.worker.drain()
    body = api("GET", f"/v1/jobs/{job['job_id']}").body
    assert body["failures"][0]["external_id"] == "NG"


# ── 検索 ─────────────────────────────────────────────────────────
def _ingested(api) -> str:
    created = api("POST", "/v1/collections", {"name": "manual"}).body
    api("POST", f"/v1/collections/{created['collection_id']}/documents",
        {"documents": [{"content": "軸受から異音がする。回転数を落とす。",
                        "title": "保守手順", "external_id": "D1",
                        "metadata": {"年度": "2026"}}]})
    api.services.worker.drain()
    return created["collection_id"]


def test_検索できる(api):
    cid = _ingested(api)
    response = api("POST", f"/v1/collections/{cid}/search", {"query": "異音"})
    assert response.status == 200
    assert response.body["data"]
    assert "candidates" in response.body["meta"]


def test_検索語が空だと400(api):
    cid = _ingested(api)
    assert api("POST", f"/v1/collections/{cid}/search",
               {"query": ""}).status == 400


def test_top_kの上限を超えると400(api):
    cid = _ingested(api)
    assert api("POST", f"/v1/collections/{cid}/search",
               {"query": "異音", "top_k": 101}).status == 400


def test_絞り込みが効く(api):
    cid = _ingested(api)
    hit = api("POST", f"/v1/collections/{cid}/search",
              {"query": "異音", "filters": {"年度": "2026"}})
    miss = api("POST", f"/v1/collections/{cid}/search",
               {"query": "異音", "filters": {"年度": "1999"}})
    assert hit.body["data"]
    assert miss.body["data"] == []


def test_内訳を出せる(api):
    cid = _ingested(api)
    response = api("POST", f"/v1/collections/{cid}/search",
                   {"query": "異音", "explain": True})
    assert "explain" in response.body["data"][0]


# ── 文書の削除 ───────────────────────────────────────────────────
def test_文書を消せる(api):
    cid = _ingested(api)
    document_id = api.services.documents.list_by_collection(cid)[0].document_id
    response = api("DELETE", f"/v1/collections/{cid}/documents/{document_id}")
    assert response.status == 204
    assert api.services.documents.list_by_collection(cid) == []


def test_知らない文書は404(api):
    cid = _ingested(api)
    assert api("DELETE",
               f"/v1/collections/{cid}/documents/nothing").status == 404


# ── 利用量 ───────────────────────────────────────────────────────
def test_利用量が引ける(api):
    api("POST", "/v1/embeddings", {"input": ["点検"]})
    response = api("GET", "/v1/usage")
    assert response.status == 200
    assert response.body["quota"]["monthly_chunks"] == 500_000
    assert response.body["usage"]["embed_chunks"] == 1


def test_金額は返さない(api):
    """★ 締める前の暫定値が独り歩きするため。経緯は runbook にしかない。"""
    body = api("GET", "/v1/usage").body
    assert "amount" not in str(body)
    assert "yen" not in str(body)
