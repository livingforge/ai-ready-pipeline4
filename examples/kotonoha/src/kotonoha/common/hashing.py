"""ハッシュ。SHA-256 で統一する。

用途は 3 つ —— API キーの照合、埋め込みキャッシュの鍵、文書の同一判定。
いずれも**正規化してから**掛ける（正規化は :func:`normalize_for_hash`）。
"""

from __future__ import annotations

import hashlib
import unicodedata


def sha256_text(text: str) -> str:
    """文字列の SHA-256 を 16 進 64 桁で返す。"""
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def sha256_bytes(data: bytes) -> str:
    """バイト列の SHA-256。文書の同一判定に使う。"""
    return hashlib.sha256(data).hexdigest()


def normalize_for_hash(text: str) -> str:
    """キャッシュ鍵にする前の正規化。

    NFKC で正規化し、前後の空白を落とし、連続する空白を 1 つに畳む。
    **全角と半角の違いでキャッシュが外れるのを防ぐため**で、検索の
    正規化（``search.query``）とは規則が違う。
    """
    folded = unicodedata.normalize("NFKC", text)
    return " ".join(folded.split())


def cache_key(text: str, model: str, input_type: str = "document") -> str:
    """埋め込みキャッシュの鍵。

    **``input_type`` を必ず含める。** 検索語と文書では前置きが変わって
    別のベクトルになるので、同じ鍵にすると取り違える —— 同じ語を
    先に文書として埋めていると、検索語の埋め込みが文書側の値で返る。
    """
    return sha256_text(f"{model}\x00{input_type}\x00{normalize_for_hash(text)}")
