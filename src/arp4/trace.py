"""出典の追跡 ―― **凍結しているからこそ、リンクは時間が経っても切れない。**

::

    正本レコード → 整理結果（ラウンド・ファイル・レコード）
                 → パース結果（アンカー）
                 → 元資料（シート・セル範囲 / ファイル・行範囲）

各ホップを機械で検証する。3 は出典を「文書名＋位置」の文字列で持っていたので、
**指している先が実在するかを確かめる手段が無かった**。4 はパスとアンカーで持つので
全件を照合できる。
"""

from __future__ import annotations

from typing import Any

from arp4 import freeze as freeze_module
from arp4 import mdio
from arp4 import parse as parse_module
from arp4.finding import Finding
from arp4.paths import Paths
from arp4.spec import Spec


def check(spec: Spec, paths: Paths) -> list[Finding]:
    """正本の ``source`` が実在するか。**凍結の照合も併せてやる。**

    照合は**上下 2 方向**である。:func:`arp4.freeze.verify` は下流（凍結後に
    整理結果が動いていないか）、:func:`arp4.parse.drifted` は上流（撮ったあとで
    原本が動いていないか）を見る。片方だけだと、リンクは全部生きているのに
    **指している先が古い版**という状態が黙って通る。
    """
    findings: list[Finding] = []
    for round_ in paths.rounds():
        findings += freeze_module.verify(round_)
        findings += parse_module.drifted(round_)

    cache: dict[tuple[str, str], mdio.ParsedFile | None] = {}
    for item in spec.items:
        target = str(item.get("id") or "(id なし)")
        for entry in _sources(item):
            findings += _one(paths, entry, target, cache)
    return findings


def _sources(item: dict[str, Any]) -> list[dict[str, Any]]:
    source = item.get("source")
    if isinstance(source, dict):
        return [source]
    if isinstance(source, list):
        return [s for s in source if isinstance(s, dict)]
    return []


def _one(paths: Paths, entry: dict[str, Any], target: str,
         cache: dict[tuple[str, str], mdio.ParsedFile | None]) -> list[Finding]:
    round_name = str(entry.get("round") or "")
    file = str(entry.get("file") or "")
    anchor = str(entry.get("anchor") or "")
    if not (round_name and file and anchor):
        # 人が手で足したアイテムには出典が無いこともある（それ自体は不備ではない）。
        return []

    round_ = paths.round(round_name)
    if not round_.dir.is_dir():
        return [Finding("error", "G010", target,
                        f"出典のラウンドがありません: {round_name}")]

    key = (round_name, file)
    if key not in cache:
        parsed = round_.parsed / f"{file}{mdio.EXT}"
        cache[key] = mdio.read(parsed) if parsed.is_file() else None
    document = cache[key]
    if document is None:
        return [Finding("error", "G004", target,
                        f"出典のパース結果がありません: {round_name}/{file}")]
    if anchor not in document.by_id:
        return [Finding("error", "G004", target,
                        f"出典のアンカーがありません: {round_name}/{file}:{anchor}")]
    return []
