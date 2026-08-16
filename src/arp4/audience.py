"""読者別の生成 ―― **stakeholder（PM・顧客）向けの設計書**と決定記録（Phase 3 / 4）。

developer 向けの 12 種は開発者以外に読めない ―― r001 の実測で、要件定義書は
制約 16 件のみ、詳細設計書はモジュール 92 件の羅列だった。読み手が PM・顧客の
ときに要るのは網羅ではなく**説明**である: 何をするシステムか（概要）、何が
できるか（機能一覧 ―― 業務語彙）、言葉の対訳（用語集）、どこまで確かめたか
（テスト状況）、全体の絵（構成図・処理フロー図）。

材料は 2 つ。**正本**（機械の集計 ―― 件数・対訳・関係）と **derived 層**
（AI の解釈 ―― グルーピング・要約・推定フロー）。derived 由来の記述には
**確度（confidence）を必ず添える** ―― 読み手が「どこまで信じてよいか」を
判断できない要約は、無いより悪い。

**空の自己申告は developer 向けと同じ規律で持つ**（Phase 3-4）。データの無い
文書は「この文書に出せるデータが正本にありません（必要な語彙: …）」と自分で
言う ―― 目次だけ見たレビュアーに「作ったが空」と「作っていない」を
取り違えさせない（r001 で価値が実証された挙動）。
"""

from __future__ import annotations

from pathlib import Path
from typing import Any

from arp4 import decisions as decisions_module
from arp4 import derived as derived_module
from arp4.derived import Derived
from arp4.spec import Spec

#: ``--audience`` に許される値。
AUDIENCES = ("developer", "stakeholder")

#: stakeholder 向けの置き場（``out/`` の下）。工程では割らない ―― 読み手は
#: V 字の工程を知らない。
DIR = "stakeholder"


def publish_stakeholder(spec: Spec, derived: Derived,
                        out_dir: Path) -> list[Path]:
    """stakeholder 向け一式を書き出す。"""
    target = out_dir / DIR
    target.mkdir(parents=True, exist_ok=True)
    written: list[Path] = []
    for name, lines in (
            ("システム概要", _overview(spec, derived)),
            ("機能一覧", _functions(spec, derived)),
            ("用語集", _glossary(spec)),
            ("テスト状況サマリ", _tests(spec)),
            ("構成図", _structure(spec)),
            ("処理フロー図", _flows(derived))):
        path = target / f"{name}.md"
        body = [f"# {name}", "",
                "> この文書は生成物です。**直接編集しないでください**"
                "（`arp4 publish --audience stakeholder` で再生成されます）。",
                ""] + lines
        path.write_text("\n".join(body).rstrip() + "\n",
                        encoding="utf-8", newline="\n")
        written.append(path)
    return written


def _missing(needs: str) -> list[str]:
    """空の自己申告。developer 向け（:func:`arp4.publish._no_data`）と同じ形。"""
    return [f"**この文書に出せるデータが正本にありません**（必要な語彙: {needs}）。",
            "空であること自体が正しい出力なら、そのままで構いません。", ""]


def _interpreted() -> list[str]:
    return ["> この文書は AI の解釈（derived 層）を含みます。各項の確度は"
            "宣言であり、根拠（basis）の実在は `arp4 check` が機械検証しています。",
            ""]


# ── システム概要 ────────────────────────────────────────────────
def _overview(spec: Spec, derived: Derived) -> list[str]:
    summaries = derived_module.of_type(derived, derived_module.SUMMARY)
    groups = derived_module.of_type(derived, derived_module.GROUP)
    lines: list[str] = []
    if not summaries and not groups:
        return _missing("derived 層の `概要`（AI の解釈は derived 層に書きます"
                        "。正本からは要約を作れません）")

    lines += _interpreted()
    for item in summaries:
        lines += [str(item.get("statement") or ""),
                  f"（確度: {item.get('confidence')}）", ""]

    if groups:
        lines += ["## 主な機能", ""]
        for item in groups:
            lines.append(f"- {item.get('name')}。{item.get('statement')}"
                         f"（確度: {item.get('confidence')}）")
        lines.append("")

    lines += ["## 規模（正本の集計）", "", "| 工程 | 登録件数 |", "|---|---|"]
    for layer in spec.metamodel.layers:
        count = sum(1 for item in spec.items
                    if spec.metamodel.layer_of(str(item.get("type"))) == layer)
        lines.append(f"| {layer} | {count} |")
    lines.append("")
    return lines


# ── 機能一覧 ────────────────────────────────────────────────────
def _functions(spec: Spec, derived: Derived) -> list[str]:
    groups = derived_module.of_type(derived, derived_module.GROUP)
    if not groups:
        return _missing("derived 層の `機能グループ`（モジュール・画面を業務の"
                        "言葉に束ねたもの）")
    by_id = spec.by_id
    lines = _interpreted()
    lines += ["| 機能 | 説明 | 確度 | 対応する設計要素 |", "|---|---|---|---|"]
    for item in groups:
        members = [str(by_id[str(m)].get("name") or m)
                   for m in item.get("members") or [] if str(m) in by_id]
        lines.append(f"| {item.get('name')} | {item.get('statement')} "
                     f"| {item.get('confidence')} | {'、'.join(members) or '―'} |")
    lines.append("")
    return lines


# ── 用語集 ──────────────────────────────────────────────────────
def _glossary(spec: Spec) -> list[str]:
    terms = sorted(spec.of_type("glossary-term"),
                   key=lambda t: (str(t.get("reading") or ""),
                                  str(t.get("name") or "")))
    if not terms:
        return _missing("`glossary-term`（用語）")
    lines = ["技術用語と業務用語の対訳です。", "",
             "| 用語 | 読み | 英語表記 | 説明 |", "|---|---|---|---|"]
    for term in terms:
        lines.append(f"| {term.get('name')} | {term.get('reading') or '―'} "
                     f"| {term.get('english') or '―'} "
                     f"| {term.get('statement') or ''} |")
    lines.append("")
    return lines


# ── テスト状況サマリ ────────────────────────────────────────────
def _tests(spec: Spec) -> list[str]:
    cases = list(spec.of_type("test-case"))
    if not cases:
        return _missing("`test-case`（テストケース。`test_*.py` か"
                        "テスト仕様書を parse に渡すと入ります）")
    lines = [f"テストケースは {len(cases)} 件 登録されています"
             "（件数と観点の集計であり、合否は「実施結果」の節が言います）。", ""]

    levels: dict[str, int] = {}
    for case in cases:
        levels[str(case.get("level") or "（段階の記載なし）")] = \
            levels.get(str(case.get("level") or "（段階の記載なし）"), 0) + 1
    lines += ["| 段階 | 件数 |", "|---|---|"]
    for level, count in sorted(levels.items()):
        lines.append(f"| {level} | {count} |")
    lines.append("")

    verified = {str(r.get("from")) for r in spec.relations_of("verifies")}
    covered = sum(1 for c in cases if str(c.get("id")) in verified)
    lines += [f"検証相手（要件・機能・モジュール）に結び付いているケース: "
              f"{covered} / {len(cases)} 件", ""]

    runs = list(spec.of_type("test-run"))
    lines += ["## 実施結果", ""]
    if not runs:
        lines += ["実施結果はまだ正本にありません（必要な語彙: `test-run`）。", ""]
    else:
        tally: dict[str, int] = {}
        for run in runs:
            tally[str(run.get("result") or "")] = \
                tally.get(str(run.get("result") or ""), 0) + 1
        lines += ["| 結果 | 件数 |", "|---|---|"]
        for result, count in sorted(tally.items()):
            lines.append(f"| {result} | {count} |")
        lines.append("")
    return lines


# ── 構成図 ──────────────────────────────────────────────────────
def _structure(spec: Spec) -> list[str]:
    """モジュールの呼出関係を mermaid で描く。**正本の関係の転記**である。

    描くのは呼出関係の**両端に現れたモジュールだけ**で、残りは絵に描くものが
    無い ―― クラス由来の module は取り込み（i1）から作る ``calls`` の端に一生
    現れず、テストの取り込みは ``verifies`` へ回るので ``calls`` を 1 本も
    持たない。**だから描いた件数を「正本の全件」と言ってはいけない。**
    自己仕様では描いた 33 件に対し正本のモジュールは 111 件で、括弧が両方に
    掛かって読めた ―― 読み手は経営層なので、ここで数を間違えるのが一番効く。

    落としたものは**種別と件数で名指しする**（`publish._omitted` と同じ規律）。
    名前を全部並べないのは、78 件の内部名は stakeholder には読むものが無い
    から ―― 件数と種別があれば「絵より多い」ことは読める。
    """
    items = list(spec.of_type("module"))
    modules = {str(i.get("id")): str(i.get("name") or i.get("id"))
               for i in items}
    calls = [(str(r.get("from")), str(r.get("to")))
             for r in spec.relations_of("calls")
             if str(r.get("from")) in modules and str(r.get("to")) in modules]
    if not calls:
        return _missing("`module` と `calls`（コードを parse に渡すと入ります）")

    used = sorted({end for edge in calls for end in edge})
    label = {item_id: f"n{position}" for position, item_id in enumerate(used)}
    caption = (f"呼出関係 {len(calls)} 本（正本に登録された全件）と、"
               f"その両端に現れるモジュール {len(used)} 件。")
    dropped = [i for i in items if str(i.get("id")) not in set(used)]
    if dropped:
        classes = sum(1 for i in dropped if i.get("class_name"))
        caption += (f"正本のモジュールは全 {len(items)} 件で、"
                    f"呼出関係を 1 本も持たない {len(dropped)} 件"
                    f"（クラス由来 {classes} 件・"
                    f"ファイル由来 {len(dropped) - classes} 件）"
                    "は描いていない。")
    lines = [caption, "", "```mermaid", "flowchart LR"]
    for item_id in used:
        lines.append(f"    {label[item_id]}[\"{modules[item_id]}\"]")
    for source, target in sorted(set(calls)):
        lines.append(f"    {label[source]} --> {label[target]}")
    lines += ["```", ""]
    return lines


# ── 処理フロー図 ────────────────────────────────────────────────
def _flows(derived: Derived) -> list[str]:
    flows = derived_module.of_type(derived, derived_module.FLOW)
    if not flows:
        return _missing("derived 層の `処理フロー`（推定した流れは derived 層に"
                        "書きます。`flow:` に手順を並べると図になります）")
    lines = _interpreted()
    for item in flows:
        lines += [f"## {item.get('name')}", "",
                  f"{item.get('statement')}（確度: {item.get('confidence')}）", ""]
        steps = [str(s) for s in item.get("flow") or []]
        if steps:
            lines += ["```mermaid", "flowchart TD"]
            for position, step in enumerate(steps):
                lines.append(f"    s{position}[\"{step}\"]")
            for position in range(len(steps) - 1):
                lines.append(f"    s{position} --> s{position + 1}")
            lines += ["```", ""]
    return lines


# ── 決定記録（developer 向けの付録） ────────────────────────────
#: 畳んだ 1 行に出す代表の数。**省略したことは必ず言う**（`publish._trim` の
#: 「省略した列」・升目の「省略した行」と同じ規律）。
_SHOWN = 3


def _kind_of(what: str) -> tuple[str, str]:
    """判断の記述を（**判断の型**, その 1 件の主語）に割る。

    ``leads-to（与信確認 → 在庫引当）を書いた向きのまま入れた`` から
    ``leads-to を書いた向きのまま入れた`` と ``与信確認 → 在庫引当`` を取る。
    括弧は入れ子になりうる（``オーダー入力（代行入力） → 与信の枠内？``）ので、
    **最初の `（` と最後の `）` で割る** ―― 非貪欲な正規表現だと入れ子の内側で
    切れる。括弧が無い記述は型がそれ自身で、主語は無い。
    """
    start, end = what.find("（"), what.rfind("）")
    if start < 0 or end <= start:
        return what, ""
    return (what[:start] + what[end + 1:]).strip(), what[start + 1:end]


def _folded(said: list[dict[str, Any]]
            ) -> list[tuple[tuple[str, str, str], int, list[str]]]:
    """同じ ``(主体, 判断の型, 確度)`` を 1 行に束ねる。**並びは初出順**。

    返すのは ``(鍵, 件数, 代表にできる主語)``。**件数は主語の数ではない** ――
    括弧を持たない記述（``矛盾から課題 iss-… を起こした``）には主語が無く、
    主語の数で数えると 0 件と出る。
    """
    order: list[tuple[str, str, str]] = []
    counts: dict[tuple[str, str, str], int] = {}
    subjects: dict[tuple[str, str, str], list[str]] = {}
    for entry in said:
        kind, subject = _kind_of(str(entry.get("what") or ""))
        key = (str(entry.get("by") or ""), kind, str(entry.get("confidence") or ""))
        if key not in counts:
            order.append(key)
            counts[key], subjects[key] = 0, []
        counts[key] += 1
        if subject:
            subjects[key].append(subject)
    return [(key, counts[key], subjects[key]) for key in order]


def decision_report(spec: Spec, out_dir: Path) -> Path | None:
    """全ラウンドの決定ログを 1 枚の付録にする。**事後拒否権の入口**である。

    無承認の通し実行で人が失うのは「事前に止める」機会だけで、判断そのものは
    ``decisions.yml`` に全件残る ―― 任意の 1 件の basis から出典アンカーへ辿り、
    間違っていれば差し戻す（正本を直すか、ラウンドを起こし直す）。

    **ここは全件を並べない。判断の型で畳む。** 実測（r001）の明細 175 行は
    ``build`` 175 / 推定 157・確実 18 で、理由は 7 種の「書いた向きのまま入れた」
    に集中していた ―― **ほぼ同一の文の反復**である。「止めたい判断を差し戻す」
    という目的に対して、同じ文が 42 回並ぶことは 1 度並ぶことに何も足さない。
    畳むと 25 行になり、型ごとの件数と確度が 1 画面で読める。

    全件は ``rounds/<ラウンド>/decisions.yml`` にあるので、**そこを名指しする
    1 行を必ず添える**（畳んで脚注で言う ―― :func:`arp4.publish._trim` と同じ）。
    """
    if spec.paths is None:
        return None
    rounds = [(r, decisions_module.load(r)) for r in spec.paths.rounds()]
    rounds = [(r, said) for r, said in rounds if said]
    if not rounds:
        return None

    # **「AI が下した判断」とは言わない。** :func:`arp4.decisions.entry` の主体は
    # ``draft`` / ``build`` / ``auto`` の 3 つで、整理層（AI）の判断はここへ
    # 1 件も来ない ―― 実測（r001）で 175 件すべてが ``build`` だった。
    lines = ["# 決定記録", "",
             "> この文書は生成物です。機械が下した判断を型ごとにまとめた表です"
             "。承認ゲートの代わりに、止めたい判断をここから差し戻します。", ""]
    for round_, said in rounds:
        folded = _folded(said)
        lines += [f"## ラウンド {round_.name}"
                  f"（{len(said)} 件を {len(folded)} 型にまとめました）", "",
                  f"全件（なぜ・根拠の出典アンカーつき）は "
                  f"`rounds/{round_.name}/{decisions_module.FILE}` にあります。", "",
                  "| 主体 | 何を | 件数 | 確度 | 代表（3 件まで）|",
                  "|---|---|---|---|---|"]
        for (by, kind, confidence), count, members in folded:
            shown = "・".join(members[:_SHOWN]) or "―"
            if len(members) > _SHOWN:
                shown += f" ほか {len(members) - _SHOWN} 件"
            lines.append(f"| {by} | {kind} | {count} | {confidence} | {shown} |")
        lines.append("")

    out_dir.mkdir(parents=True, exist_ok=True)
    path = out_dir / "決定記録.md"
    path.write_text("\n".join(lines).rstrip() + "\n",
                    encoding="utf-8", newline="\n")
    return path
