"""指摘の機械可読な出力 ―― **書式を約束していないものに依存させない。**

エージェントはいま ``[error] G006 …`` の行を読んで次の一手を決めている。
書式が約束になっていないので、**こちらが出力を直した瞬間に黙って壊れる。**
ここで見るのは「JSON / SARIF が壊れずに出るか」と「終了コードが書式で
変わらないか」である。
"""

from __future__ import annotations

import io
import json
import sys

from arp4 import cli, report
from arp4.finding import Finding
from arp4.paths import Paths, Round
from conftest import organized, parsed

_FINDINGS = [
    Finding("error", "G006", "s1-t1", "必須の欄がありません",
            file=".arp/rounds/r001/organized/a.yml", line=47),
    Finding("warn", "W030", "ent-1", "accesses がありません"),
]


def test_JSONは件数と指摘を約束する() -> None:
    body = json.loads(report.as_json("freeze", _FINDINGS, {"records": 3}))

    assert body["schema"] == report.SCHEMA
    assert body["command"] == "freeze"
    assert body["counts"] == {"error": 1, "warn": 1}
    assert body["metrics"] == {"records": 3}
    assert body["findings"][0]["code"] == "G006"      # error が先
    assert body["findings"][0]["line"] == 47
    assert "line" not in body["findings"][1]          # 位置の無い指摘は欄ごと落とす


def test_metricsが無ければ欄ごと落とす() -> None:
    """空の ``{}`` を置くと、「取れなかった」と「もともと無い」が混ざる。"""
    assert "metrics" not in json.loads(report.as_json("check", _FINDINGS))


def test_JSONは日本語をエスケープして出す() -> None:
    """日本語 Windows の既定コンソールは cp932 で、生の日本語を流すと
    ``backslashreplace`` が潰す ―― **JSON として壊れたものが出る。**"""
    text = report.as_json("check", _FINDINGS)

    assert text.isascii()
    assert text.encode("cp932")                       # cp932 でも出せる
    assert json.loads(text)["findings"][0]["message"] == "必須の欄がありません"


def test_SARIFは位置の無い指摘も落とさない() -> None:
    """位置の取れない検査（``M1xx`` / ``W030``）を落とすと、**位置の取れない
    指摘ほど静かに消える。**"""
    run = json.loads(report.as_sarif("check", _FINDINGS))["runs"][0]

    assert [r["ruleId"] for r in run["results"]] == ["G006", "W030"]
    assert run["results"][0]["locations"][0]["physicalLocation"] == {
        "artifactLocation": {"uri": ".arp/rounds/r001/organized/a.yml"},
        "region": {"startLine": 47}}
    assert "locations" not in run["results"][1]
    assert run["results"][1]["level"] == "warning"    # warn は SARIF の語彙に無い
    assert run["invocations"][0]["executionSuccessful"] is False


def test_SARIFのヒントは本文に畳む() -> None:
    """SARIF に「次の一手」の欄が無いので、落とさずに本文へ入れる。"""
    said = json.loads(report.as_sarif(
        "freeze", [Finding("error", "G014", "", "壊れています", hint="引用符で囲む")]))
    assert said["runs"][0]["results"][0]["message"]["text"] == \
        "壊れています（引用符で囲む）"


# ── CLI ────────────────────────────────────────────────────────
_PARSED = """\
# a.xlsx / 受注テーブル

<!-- source: 資料/a.xlsx / シート: 受注テーブル -->

## 表 B5:H8  <!-- a:s1-t1 at=B5:H8 -->

| 論理名 | 物理名 |
|---|---|
| 受注番号 | ORDER_NO |
"""

_ORGANIZED = """\
records:
  - concept: c-受注番号
    type: データ項目
    name: 受注番号
    statement: 受注番号は文字列型の項目であること
    source: { anchor: s1-t1 }
"""


def _run(argv: list[str], monkeypatch) -> tuple[int, str]:
    out = io.StringIO()
    monkeypatch.setattr(sys, "stdout", out)
    code = cli.main(argv)
    return code, out.getvalue()


def test_freezeのJSONに人向けの行が混ざらない(project: Paths, round_: Round,
                                              monkeypatch) -> None:
    """1 行でも混ざると標準出力が JSON として読めなくなり、呼ぶ側が
    「JSON でない行を捨てる」処理を書くことになる。"""
    parsed(round_, "資料/a.xlsx/受注テーブル.md", _PARSED)
    organized(round_, "資料/a.xlsx/受注テーブル.yml", _ORGANIZED)

    code, text = _run(["freeze", "--dry-run", "--root", str(project.root),
                       "--format", "json"], monkeypatch)
    body = json.loads(text)                            # 前後に何も無い

    assert code == 0
    assert body["command"] == "freeze"
    assert body["metrics"]["unclaimed"] == 0


def test_終了コードは書式で変わらない(project: Paths, round_: Round,
                                      monkeypatch) -> None:
    """CI と手元で結論がずれると、**書式のほうを疑うことになる。**"""
    parsed(round_, "資料/a.xlsx/受注テーブル.md", _PARSED)      # 整理結果なし

    text_code, _ = _run(["freeze", "--dry-run", "--root", str(project.root)],
                        monkeypatch)
    json_code, body = _run(["freeze", "--dry-run", "--root", str(project.root),
                            "--format", "json"], monkeypatch)

    assert text_code == json_code == 1
    assert json.loads(body)["counts"]["error"] == 1


def test_JSONでも凍結はする(project: Paths, round_: Round, monkeypatch) -> None:
    """**書式は出し方だけを変える。** 「JSON で見たときだけ凍っていない」
    という差が生まれると、CI と手元で状態がずれる。"""
    parsed(round_, "資料/a.xlsx/受注テーブル.md", _PARSED)
    organized(round_, "資料/a.xlsx/受注テーブル.yml", _ORGANIZED)

    code, _ = _run(["freeze", "--root", str(project.root), "--format", "json"],
                   monkeypatch)

    assert code == 0
    assert round_.is_frozen()


def test_人向けを畳んでも件数と終了コードは変えない(monkeypatch) -> None:
    """**畳むのは表示だけ。** 「畳んだから減った」が起きると、人と CI の結論が
    ずれる（→ :mod:`arp4.digest`）。"""
    findings = [Finding("warn", "W031", f"cst-{i}", "相手がありません")
                for i in range(30)]
    out = io.StringIO()
    monkeypatch.setattr(sys, "stdout", out)

    code = cli._report(findings, strict=True)
    text = out.getvalue()

    assert code == 2                                   # warn のみ = 2
    assert "error 0 / warn 30" in text                 # 件数は畳まない
    assert len([l for l in text.splitlines() if l.startswith("[warn]")]) == 1


def test_全件はファイルへ置いて標準出力には場所だけ出す(project: Paths,
                                                        monkeypatch) -> None:
    """**標準出力は読み手の文脈に必ず載るが、ファイルは要るときだけ載る。**
    畳んだぶんを取り戻す道が打ち直ししか無いと、読み手は最初から全件を出させる。"""
    findings = [Finding("warn", "W031", f"cst-{i}", "相手がありません")
                for i in range(30)]
    out = io.StringIO()
    monkeypatch.setattr(sys, "stdout", out)

    cli._report(findings, strict=False, command="check", paths=project)
    body = json.loads((project.out / cli._FINDINGS).read_text(encoding="utf-8"))

    assert ".arp/out/findings.json" in out.getvalue()
    assert len(body["findings"]) == 30                 # ファイルは畳まない
    assert body["command"] == "check"


def test_ファイルの日本語は潰さない(project: Paths, monkeypatch) -> None:
    """``\\uXXXX`` は日本語 1 文字が 6 バイトになる ―― 読み返すエージェントの
    文脈をそのぶん食う。**符号化を自分で決められるファイルなら降ろす。**"""
    monkeypatch.setattr(sys, "stdout", io.StringIO())
    cli._report([Finding("warn", "W031", "cst-1", "相手がありません")],
                strict=False, command="check", paths=project)

    text = (project.out / cli._FINDINGS).read_text(encoding="utf-8")

    assert "相手がありません" in text                  # 生の日本語で入っている
    assert report.as_json("check", _FINDINGS).isascii()  # 標準出力向けは潰したまま


def test_書けなくても検証は止めない(project: Paths, monkeypatch) -> None:
    """検証の結論は出力先の都合で変わらない。"""
    monkeypatch.setattr(sys, "stdout", io.StringIO())
    monkeypatch.setattr(report.Path, "write_text",
                        lambda *a, **kw: (_ for _ in ()).throw(OSError()))

    assert cli._report(_FINDINGS, strict=True, command="check",
                       paths=project) == 1


def test_JSONは人向けの畳みに影響されない(monkeypatch) -> None:
    """機械が読むものを間引く理由は無い。"""
    findings = [Finding("warn", "W032", f"ent-{i}", "承知している欠落")
                for i in range(10)]
    assert len(json.loads(report.as_json("check", findings))["findings"]) == 10


def test_作業キューをJSONでは間引かない(project: Paths, round_: Round,
                                        monkeypatch) -> None:
    """人向けの一覧を 20 件で切るのは**画面が有限だから**である。読み手が
    機械なら切る理由が無い ―― 切ると「未整理は 20 件」と読む事故になる。"""
    anchors = "\n".join(
        f"## 表 B{i}  <!-- a:s1-t{i} at=B{i} -->\n\n- `B{i}` x\n"
        for i in range(1, 26))
    parsed(round_, "a.md",
           f"# a\n\n<!-- source: a / シート: a -->\n\n{anchors}")

    _, text = _run(["freeze", "--dry-run", "--root", str(project.root),
                    "--format", "json"], monkeypatch)

    assert len(json.loads(text)["findings"]) == 25
