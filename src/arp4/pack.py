"""標準パックの解決と ``pack.lock``。

パックは**メタモデル＋文書様式＋運用ルール**をひとまとまりで配る単位である。
消費側は ``metamodel.yml`` に ``extends: jp-sier-std`` と書くだけでよい。

``pack.lock`` は解決結果（パック名・版・内容ハッシュ）を固定する。
CI で ``arp4 conform --frozen`` を回せば、**パックを直したのに lock が古いまま**
という状態を機械的に止められる（既定は warn 止まりなので素通りする）。

照合に使うのは **pack / version / content_hash だけ**である。パックの所在
（絶対パス）は環境で変わるので lock に残さない ―― 残すと同じパックでも
開発リポと消費側で frozen が誤検知する。
"""

from __future__ import annotations

import hashlib
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from arp4 import yamlio
from arp4.finding import Finding

#: 同梱パックの置き場。
PACKS_DIR = Path(__file__).parent / "packs"

#: 内容ハッシュから除くもの（生成物・キャッシュ）。
_SKIP_PARTS = ("__pycache__",)
_SKIP_SUFFIXES = (".pyc",)


@dataclass(frozen=True)
class Pack:
    """解決済みのパック 1 つ。"""

    name: str
    version: str
    dir: Path
    meta: dict[str, Any]

    @property
    def metamodel_path(self) -> Path | None:
        relative = str(self.meta.get("metamodel") or f"metamodel{yamlio.EXT}")
        path = self.dir / relative
        return path if path.is_file() else yamlio.find(self.dir, Path(relative).stem)

    @property
    def documents_dir(self) -> Path:
        return self.dir / str(self.meta.get("documents") or "documents")

    @property
    def conformance_path(self) -> Path | None:
        relative = self.meta.get("conformance")
        if not relative:
            return yamlio.find(self.dir / "conformance", "rules")
        path = self.dir / str(relative)
        return path if path.is_file() else None


def find(name: str, packs_dir: Path | None = None) -> Pack:
    """パック 1 つを読む。見つからなければ :class:`FileNotFoundError`。"""
    base = (packs_dir or PACKS_DIR) / name
    manifest = yamlio.find(base, "pack")
    if manifest is None:
        raise FileNotFoundError(f"パックがありません: {base}")
    meta = yamlio.load(manifest) or {}
    return Pack(name=str(meta.get("pack") or name),
                version=str(meta.get("version") or "0.0.0"),
                dir=base, meta=meta)


def resolve_chain(extends: str | None,
                  packs_dir: Path | None = None) -> tuple[list[Pack], list[Finding]]:
    """``extends`` を辿ってチェーンを解決する。**根（基底）から先頭**で返す。

    パック自身が ``extends`` を持てば多段になる。循環は C011 で止める。
    """
    findings: list[Finding] = []
    chain: list[Pack] = []
    seen: set[str] = set()
    name = extends

    while name:
        if name in seen:
            findings.append(Finding("error", "C011", name,
                                    "パックの継承が循環しています"))
            break
        seen.add(str(name))
        try:
            pack = find(str(name), packs_dir=packs_dir)
        except FileNotFoundError as exc:
            findings.append(Finding("error", "C010", str(name), str(exc)))
            break
        chain.append(pack)
        name = pack.meta.get("extends")

    chain.reverse()                      # 根 → 派生の順にする
    return chain, findings


def documents(chain: list[Pack]) -> list[dict[str, Any]]:
    """チェーン全体の文書定義。**同名は派生側が勝つ。**"""
    merged: dict[str, dict[str, Any]] = {}
    for pack in chain:
        for path in yamlio.scan(pack.documents_dir):
            definition = yamlio.load(path) or {}
            name = str(definition.get("name") or path.stem)
            merged[name] = {**definition, "name": name}
    return [merged[name] for name in sorted(merged)]


def rules(chain: list[Pack]) -> dict[str, Any]:
    """チェーン全体の準拠ルール。**リストは連結、辞書は派生側が勝つ。**"""
    merged: dict[str, Any] = {}
    for pack in chain:
        path = pack.conformance_path
        if path is None:
            continue
        for key, value in (yamlio.load(path) or {}).items():
            if isinstance(value, list):
                merged[key] = list(merged.get(key) or []) + list(value)
            elif isinstance(value, dict):
                merged[key] = {**(merged.get(key) or {}), **value}
            else:
                merged[key] = value
    return merged


# ── 内容ハッシュと pack.lock ────────────────────────────────────
def content_hash(directory: Path) -> str:
    """ディレクトリの内容を 1 本のハッシュにする。**パス順に依存しない。**"""
    digest = hashlib.sha256()
    for path in sorted(p for p in directory.rglob("*") if p.is_file()):
        if any(part in _SKIP_PARTS for part in path.parts):
            continue
        if path.suffix in _SKIP_SUFFIXES:
            continue
        digest.update(path.relative_to(directory).as_posix().encode("utf-8") + b"\0")
        digest.update(path.read_bytes() + b"\0")
    return "sha256:" + digest.hexdigest()


def lock_body(chain: list[Pack]) -> dict[str, Any]:
    return {"chain": [{"pack": pack.name, "version": pack.version,
                       "content_hash": content_hash(pack.dir)} for pack in chain]}


def lock_path(root: Path) -> Path:
    return root / "pack.lock"


def write_lock(root: Path, chain: list[Pack]) -> Path:
    path = lock_path(root)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("# 機械生成。直接編集しない（arp4 lock で更新する）。\n"
                    + yamlio.dumps(lock_body(chain)),
                    encoding="utf-8", newline="\n")
    return path


def verify_lock(root: Path, chain: list[Pack],
                frozen: bool = False) -> list[Finding]:
    """lock と解決結果を照合する。

    lock が無いのは通常運用では正常（明示運用）。``--frozen`` では
    「固定したはずのものが固定されていない」ことを意味するので error にする。
    """
    path = lock_path(root)
    level = "error" if frozen else "warn"

    if not path.is_file():
        if not frozen:
            return []
        return [Finding("error", "C001", "pack.lock",
                        "pack.lock がありません（arp4 lock で作成してください）")]

    locked = yamlio.load(path) or {}
    expected = lock_body(chain)["chain"]
    actual = [{"pack": e.get("pack"), "version": e.get("version"),
               "content_hash": e.get("content_hash")}
              for e in (locked.get("chain") or [])]

    if actual == expected:
        return []

    findings: list[Finding] = []
    by_name = {str(e["pack"]): e for e in actual}
    for entry in expected:
        previous = by_name.pop(str(entry["pack"]), None)
        if previous is None:
            findings.append(Finding(level, "C002", str(entry["pack"]),
                                    "lock に無いパックを解決しました"))
        elif previous["version"] != entry["version"]:
            findings.append(Finding(
                level, "C002", str(entry["pack"]),
                f"版が lock と違います: lock {previous['version']} / 実際 {entry['version']}"))
        elif previous["content_hash"] != entry["content_hash"]:
            findings.append(Finding(
                level, "C002", str(entry["pack"]),
                "パックの内容が lock と違います（arp4 lock で更新してください）"))
    for name in by_name:
        findings.append(Finding(level, "C002", name,
                                "lock にあるパックを解決できませんでした"))
    return findings
