"""同じ文書の再取り込みを飛ばす。

判定は本文の SHA-256（``content_hash``）。**表題やメタデータが変わっても
本文が同じなら飛ばす** —— 埋め込みは本文からしか作らないので、作り直す
意味が無いためである。ただしメタデータだけは更新する（検索の絞り込みに
使うので古いままだと当たらなくなる）。

保守マニュアルの取り込みでは、毎晩の全件同期のうち 9 割以上がここで
飛ぶ。これが無いと課金が 10 倍になる。
"""

from __future__ import annotations

from dataclasses import dataclass

from kotonoha.common import logging as applog
from kotonoha.common.hashing import sha256_bytes
from kotonoha.ingest.models import Document, SourceDocument

log = applog.get(__name__)


@dataclass
class Decision:
    """飛ばすか、取り込むか、メタデータだけ直すか。"""

    action: str                    # ingest / skip / metadata_only
    reason: str = ""
    existing: Document | None = None

    @property
    def skipped(self) -> bool:
        return self.action in ("skip", "metadata_only")


def content_hash(source: SourceDocument) -> str:
    """本文のハッシュ。正規化**後**の本文から取る。"""
    return sha256_bytes(source.content.encode("utf-8"))


def decide(source: SourceDocument, existing: Document | None) -> Decision:
    """取り込むべきか決める。

    - 同じ ``external_id`` が無い → 取り込む
    - あって本文のハッシュが違う → 取り込む（古いほうは消す）
    - あって本文が同じ・メタデータも同じ → 飛ばす
    - あって本文が同じ・メタデータが違う → メタデータだけ直す
    """
    if existing is None or not existing.alive:
        return Decision(action="ingest", reason="new")
    if existing.content_hash != content_hash(source):
        return Decision(action="ingest", reason="content_changed", existing=existing)
    if existing.metadata != source.metadata:
        return Decision(action="metadata_only", reason="metadata_changed",
                        existing=existing)
    return Decision(action="skip", reason="same_hash", existing=existing)


def summarize(decisions: list[Decision]) -> dict[str, int]:
    """内訳を数える。ジョブの結果に載せる。"""
    counts: dict[str, int] = {}
    for decision in decisions:
        counts[decision.action] = counts.get(decision.action, 0) + 1
    return counts
