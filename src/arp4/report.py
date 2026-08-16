"""指摘の機械可読な出力（``--format json`` / ``--format sarif``）。

**読み手が 2 ついる。**

エージェント
    いまは日本語の文章を読んで次の一手を決めている。``[error] G006 …`` の行を
    正規表現で割るのは、こちらが**出力の書式を仕様として約束していない**のに
    向こうがそれに依存する、という関係になる ―― 書式を直した瞬間に黙って壊れる。

CI
    SARIF にすると GitHub が PR の**行**に注釈を出す。``file`` / ``line`` を
    :class:`arp4.finding.Finding` が構造として持つようになったので、変換が
    転記だけで済む（→ :mod:`arp4.finding`）。

**日本語は ``\\uXXXX`` で出す。** 日本語 Windows の既定コンソールは cp932 で、
そこへ生の日本語を流すと :func:`arp4.cli._resilient_output` が
``backslashreplace`` で潰す ―― **JSON として壊れたものが出る。** 出力先が端末か
ファイルかは呼ばれる側から決められないので、**どこへ出しても壊れないほうを既定に
する。** JSON の読み手は必ずエスケープを解くので、機械には損が無い。
"""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any, Iterable

from arp4.finding import Finding, counts, order

#: 出力の版。**書式を変えたら上げる**（読み手が分岐できる）。
SCHEMA = "arp4/findings/1"

#: SARIF のレベル。``warn`` は SARIF の語彙に無い。
_SARIF_LEVEL = {"error": "error", "warn": "warning"}

#: 選べる書式。``text`` は人が読むもの（:meth:`Finding.render`）。
FORMATS = ("text", "json", "sarif")


def as_json(command: str, findings: Iterable[Finding],
            metrics: dict[str, Any] | None = None, *, ascii: bool = True) -> str:
    """指摘 1 本ぶんの JSON。**件数と指摘だけを約束する。**

    ``metrics`` は命令ごとに中身が違う（``freeze`` なら未整理の件数、``check``
    ならアイテム数）ので、**約束しない**。無いときは欄ごと落とす ―― 空の
    ``{}`` を置くと、読み手が「取れなかった」と「もともと無い」を区別できない。

    ``ascii`` は**出力先を知っている側だけが降ろせる**。既定で潰すのは標準出力が
    cp932 のことがあるからで（→ このモジュールの説明）、**符号化を自分で決められる
    ファイルなら降ろす** ―― ``\\uXXXX`` は日本語 1 文字が 6 バイトになり、
    読み返すエージェントの文脈をそのぶん食う（→ :func:`write`）。
    """
    ordered = order(findings)
    body: dict[str, Any] = {
        "schema": SCHEMA,
        "command": command,
        "counts": counts(ordered),
        "findings": [f.as_dict() for f in ordered],
    }
    if metrics:
        body["metrics"] = metrics
    return json.dumps(body, ensure_ascii=ascii, indent=2)


def write(path: Path, command: str, findings: Iterable[Finding],
          metrics: dict[str, Any] | None = None) -> Path | None:
    """全件をファイルへ。**標準出力と違って、読むかどうかを読み手が決められる。**

    人向けの出力は畳んである（→ :mod:`arp4.digest`）。畳んだぶんを取り戻す道が
    ``--format json`` の**打ち直し**しか無いと、読み手は最初から全件を出させる
    ―― 畳んだ意味が消える。**毎回ここへ全件を置き**、標準出力にはその場所だけを
    出す ―― 標準出力は読み手の文脈に必ず載るが、**ファイルは要るときだけ載る。**

    符号化は UTF-8 で、日本語は潰さない（→ :func:`as_json` の ``ascii``）。
    書けなくても**呼び出し側は止めない** ―― 検証の結論は出力先の都合で変わらない。
    """
    try:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(as_json(command, findings, metrics, ascii=False),
                        encoding="utf-8", newline="\n")
    except OSError:
        return None
    return path


def as_sarif(command: str, findings: Iterable[Finding]) -> str:
    """SARIF 2.1.0。**位置の無い指摘も落とさない。**

    位置が取れない検査（メタモデルの ``M1xx``・正本の ``W030``）は SARIF の
    ``locations`` を持てないが、**出さないという選択はしない** ―― CI の結論が
    「位置が取れたものだけ」になると、位置の取れない指摘ほど静かに消える。
    """
    ordered = order(findings)
    results = []
    for finding in ordered:
        result: dict[str, Any] = {
            "ruleId": finding.code,
            "level": _SARIF_LEVEL.get(finding.level, "note"),
            "message": {"text": _text(finding)},
        }
        if finding.file:
            region = {"startLine": finding.line} if finding.line else {}
            result["locations"] = [{"physicalLocation": {
                "artifactLocation": {"uri": finding.file},
                **({"region": region} if region else {})}}]
        results.append(result)

    rules = [{"id": code} for code in sorted({f.code for f in ordered})]
    body = {
        "version": "2.1.0",
        "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
        "runs": [{
            "tool": {"driver": {"name": "arp4", "rules": rules,
                                "informationUri": "https://github.com/", }},
            "invocations": [{"commandLine": f"arp4 {command}",
                             "executionSuccessful":
                                 not any(f.level == "error" for f in ordered)}],
            "results": results,
        }],
    }
    return json.dumps(body, ensure_ascii=True, indent=2)


def _text(finding: Finding) -> str:
    """SARIF の本文。**次の一手（hint）も落とさない。**"""
    if finding.hint:
        return f"{finding.message}（{finding.hint}）"
    return finding.message


def render(command: str, findings: Iterable[Finding], fmt: str,
           metrics: dict[str, Any] | None = None) -> str:
    """書式名で選ぶ。``text`` はここでは扱わない（人向けは各コマンドが組む）。"""
    if fmt == "sarif":
        return as_sarif(command, findings)
    return as_json(command, findings, metrics)
