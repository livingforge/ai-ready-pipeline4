"""凍結 ―― **1 つの操作で 3 つのことをやる。**

1. **網羅の確認** ―― 資料が黙って落ちていないか
2. **ハッシュの固定** ―― 以後、出典が動かないことの保証
3. **機械に渡せる状態であることの検査** ―― 通れば ``build`` は原理的に失敗しない

3 の中核だった再現性ゲート（``R001``「同じファクトから 2 回で同じ正本」）は、抽出者が
エージェントになった時点で作れない。**再現できないなら、動かないことを保証すればよい。**

ゲートの 5 条件::

    G001  未整理のアンカーが 0（レコードにも out_of_scope にもなっていない）
    G002  語彙外の type / rel が 0
    G003  concept が実在する（レコード・refs の相手とも）
    G012  関係の from/to が宣言と合う（**どちらの向きでも成立しないものが 0**）
    G013  参照だけのレコードの種別が決まる（完全なレコードか台帳のどちらかにある）

その手前に ``G014``（YAML として壊れている）がある。**5 条件と並ぶものではなく、
5 条件が意味を持つための前提である** ―― 壊れたファイルは
:func:`arp4.organized.load` が読み飛ばすので、そのファイルのレコードは 0 件になり、
**カバーしていたアンカーが丸ごと G001（未整理）に化ける。** 数字だけ見ると整理の
やり残しに見えるので、``G014`` が 1 件でもあるあいだ他の条件の件数は当てにならない。

補助として ``G004``（アンカー実在）・``G005``（本文に語が無い。warn）・``G016``
（宣言に無い属性名。warn）・``G028``（**宣言に無い enum の値**。warn）・``G018``
（出典の欄が落ちている。warn）・``G019``（**原本が撮ったときと違う**。warn）・
``G020``（語彙が要ると言っている関係が 0 本。warn）を見る。
``G020`` には**降りる口がある** ―― レコードの ``known_gaps`` に理由つきで宣言
すれば、その関係の ``G020`` は出さない（``G031`` が宣言そのものを検査し、件数は
集計に必ず出す。→ :func:`_required_refs`）。
``G005`` を warn に留めるのは、**言い換えは
許される**ので機械が真偽を決められないため。``G018`` は逆に**照合しかしていない**
が、落としたことに理由がある場合（同じことを statement に書いた）を潰せないので
同じく warn である ―― どちらも「build を落とさない」ものは段を上げない。

``G012`` は後から足した条件である。``G002`` は「語彙にある関係か」しか見ないので、
**語彙にある関係を語彙にない組み合わせで使っても凍結が通り**、``build`` が ``B013``
で落ちていた ―― 「通れば build は原理的に失敗しない」という約束が守れていなかった。
判定は :func:`arp4.metamodel.orient` を ``build`` と共有する（規則が 2 つあると
同じ問題が形を変えて戻る）。
"""

from __future__ import annotations

import datetime as _datetime
import hashlib
import re
from dataclasses import dataclass, field
from typing import Any

from arp4 import (mdio, metamodel as mm, organized as organized_module,
                  parse as parse_module, yamlio)
from arp4.concepts import Concept
from arp4.finding import Finding, order
from arp4.metamodel import Metamodel
from arp4.paths import Round

#: 語の包含検査で見る語の長さ（これ未満は助詞・記号なので当てにならない）。
#: 例外は漢字 1 字の**語幹**（:func:`_grounded`）―― 「揃えるか」（本文）と
#: 「揃え方」（statement）に共通する語は「揃」しか無い。
_MIN_WORD = 2

#: 語として拾う並び。日本語は分かち書きしないので、字種の連続で切る。
#: 漢字は 1 字から拾う ―― 2 字未満を捨てていたころは、漢字とひらがなが交互に
#: 来る名前（``test_3つ揃えるか3つとも省くか``）から語が 1 つも取れず**検査
#: そのものが飛び**、そこへ丁寧な言い換え（``type と name と statement の
#: 揃え方``）を足すと**足した語だけが照合されて**鳴った ―― 説明を丁寧にする
#: ほど鳴り、何も足さないほど静かに通る検査になっていた。ひらがなは拾わない
#: （助詞・活用語尾で、語ではない）。
_WORD = re.compile(r"[一-鿿々]+|[ァ-ヶー]{2,}|[A-Za-z_][A-Za-z0-9_]+")


@dataclass
class Report:
    """凍結ゲートの結果。**通らなかった理由は数える。**"""

    findings: list[Finding] = field(default_factory=list)
    unclaimed: list[tuple[str, str]] = field(default_factory=list)
    #: ``known_gaps`` の宣言で ``G020`` を出さなかったぶん（``(concept, 関係型)``）。
    #: **黙って消えるのがいちばん悪い形**なので、件数は集計に出す（→ :attr:`metrics`）。
    gaps_used: list[tuple[str, str]] = field(default_factory=list)
    metrics: dict[str, Any] = field(default_factory=dict)

    @property
    def blocked(self) -> bool:
        return any(f.level == "error" for f in self.findings)


def gate(round_: Round, model: Metamodel, known: dict[str, Concept]) -> Report:
    """凍結できるかを見る。**書き込みはしない。**"""
    report = Report()
    result, findings = organized_module.load(round_)
    report.findings += findings

    parsed = {p.relative_to(round_.parsed).with_suffix("").as_posix(): mdio.read(p)
              for p in mdio.scan(round_.parsed)}

    report.findings += _unclaimed(round_, result, parsed, report)
    report.findings += _orphans(result, parsed)
    report.findings += _unread(result, parsed)
    report.findings += _vocabulary(result, model)
    report.findings += _concepts(round_, result, known)
    report.findings += _defined(result, known)
    report.findings += _pairs(result, model, known)
    report.findings += _anchors(result, parsed)
    report.findings += _wording(result, parsed)
    report.findings += _columns(result, parsed, model)
    report.findings += _conventions(result, parsed, model, known)
    report.findings += _todo_slots(result)
    report.findings += _extraction(result)
    report.findings += _descriptions(result, model)
    report.findings += _gap_names(result, model)
    report.findings += _required_refs(result, model, report)
    report.findings += _proposal(round_, result, model, known)
    # **上流のずれ。** 門の 5 条件は整理結果とパース結果の関係しか見ておらず、
    # その手前（原本 → パース結果）が動いていないことは誰も確かめていなかった。
    report.findings += parse_module.drifted(round_)
    report.findings += _metamodel_add(round_, result, model)

    report.metrics = {
        "parsed_files": len(parsed),
        "anchors": sum(len(p.anchors) for p in parsed.values()),
        "records": len(result.records),
        "references": sum(1 for r in result.records if not r.complete),
        "out_of_scope": len(result.out_of_scope),
        "unreadable": sum(1 for o in result.out_of_scope if o.unreadable),
        "unclaimed": len(report.unclaimed),
        # **宣言したぶんは必ず数に出す。** `known_gaps` は指摘を 1 件消す仕組み
        # なので、件数を言わないと「直したから減った」と「宣言で降ろした」が
        # 画面から区別できない ―― 黙って消えるのがいちばん悪い形である。
        "known_gaps": sum(len(r.known_gaps) for r in result.records),
        "known_gaps_silenced": len(report.gaps_used),
    }
    report.findings = order(report.findings)
    return report


# ── ① 網羅 ──────────────────────────────────────────────────────
def _unclaimed(round_: Round, result: organized_module.Organized,
               parsed: dict[str, mdio.ParsedFile], report: Report) -> list[Finding]:
    """レコードにも ``out_of_scope`` にもなっていないアンカー。

    **未整理を 0 にする方法は 2 つある** ―― レコードを起こすか、理由を付けて
    対象外と宣言するか。どちらも記録に残るので、あとから「なぜこのシートは仕様に
    なっていないのか」に答えられる。
    """
    claimed = result.claimed
    findings: list[Finding] = []
    for file, document in sorted(parsed.items()):
        source = _relative(round_, organized_module.parsed_path(round_, file))
        for anchor in document.anchors:
            if (file, anchor.id) in claimed:
                continue
            report.unclaimed.append((file, anchor.id))
            # **指す先はパース結果である。** 書く先（整理結果）はまだ無いことが
            # あるので、開けないパスを出しても次の一手にならない。
            findings.append(Finding(
                "error", "G001", anchor.id,
                "整理も対象外宣言もされていません"
                + (f"（{anchor.at}）" if anchor.at else ""),
                file=source))
    return findings


def lint(round_: Round, model: Metamodel, known: dict[str, Concept],
         only: Any = None) -> Report:
    """**1 ファイルだけで決まる指摘**を、書いている最中に出す。

    ``freeze`` は 200 ファイルを読んでゲート 5 条件を見るので、1 ファイル書いた
    直後に回すには重い。しかも出てくるのは未整理 296 件の山で、**自分がいま書いた
    1 件がその中に埋もれる。**

    見るもの・見ないものの線引きは「**そのファイルだけで決まるか**」である。

    ====================  ==========================================
    出す                  ``G014`` 構文 / ``G000`` 形 / ``G006`` 必須欄 /
                          ``G008`` 属性の置き場所 / ``G011`` 区分 /
                          ``G016`` 属性名 / ``G028`` enum の値（warn）/
                          ``G002`` 語彙 / ``G012`` 関係の向き /
                          ``G004`` アンカー実在 / ``G005`` 作文の疑い（warn）/
                          ``G018`` 出典の欄の取りこぼし（warn）/
                          ``G031`` known_gaps の名前 /
                          ``G021`` 台帳への提案の相手（``_concepts.yml``）
    出さない              ``G001`` 未整理 / ``G003`` concept 実在 /
                          ``G013`` 参照だけのレコード / ``G007`` 語彙の追加提案 /
                          ``G019`` 原本のずれ / ``G020`` 要る関係が 0 本 /
                          ``G031`` の「宣言したのに関係が書いてある」（warn）
    ====================  ==========================================

    ``G004`` を出せるのは、整理結果とパース結果が**名前で 1:1**だからである
    （相方は 1 つに決まるので、他のファイルを読む必要が無い）。幻覚の最頻形は
    「存在しない出典」なので、ここで潰せるのは大きい。

    **判定は ``gate`` と同じ関数を呼ぶ。** 別実装にすると、``G002`` が ``B013``
    を取りこぼしていたのと同じ事故（規則が 2 つあると同じ問題が形を変えて戻る）が
    lint と freeze のあいだで起きる。

    ここが出せるのは ``gate`` が出すものの**部分集合**である ―― ``G012`` は
    相手の種別が分からなければ黙る（``_pairs`` の規律）ので、他のファイルでしか
    定義されていない concept との関係は lint では判定されず、``freeze`` まで
    持ち越される。**lint が通ったことは凍結できることを意味しない。**
    """
    report = Report()
    result, findings = organized_module.load(round_, only=only)
    report.findings += findings

    # パース結果は**読んだ整理結果の相方だけ**を読む（1:1 対応から決まる）。
    parsed: dict[str, mdio.ParsedFile] = {}
    for file in result.files:
        path = organized_module.parsed_path(round_, file)
        if path.is_file():
            parsed[file] = mdio.read(path)

    report.findings += _orphans(result, parsed)
    report.findings += _vocabulary(result, model)
    report.findings += _pairs(result, model, known)
    report.findings += _anchors(result, parsed)
    report.findings += _wording(result, parsed)
    report.findings += _columns(result, parsed, model)
    # 規約（G022〜G025）も 1 ファイルで決まるものが大半なので lint でも見る。
    # 相手の種別が他のファイルでしか決まらない G022 は `_pairs` と同じ規律で
    # 黙り、freeze まで持ち越される。
    report.findings += _conventions(result, parsed, model, known)
    # 文章化スロット（G026）と抽出的文章（G027）は 1 ファイルで決まる ――
    # 埋めた直後の手元で鳴るのが価値である（鳴る時期の話は G020 と同じ）。
    report.findings += _todo_slots(result)
    report.findings += _extraction(result)
    # `description` への逃がし（G029 / G030）は 1 ファイルで決まる ―― 書いた
    # 直後の手元で鳴ることに価値がある。`G030` の件数は読んだファイルの中だけ
    # なので、ラウンド全体でしか閾値に届かないものは `freeze` まで持ち越される
    # （`G012` と同じ規律 ―― **lint が通ったことは凍結できることを意味しない**）。
    report.findings += _descriptions(result, model)
    # `known_gaps` の**名前**（``G031``）は 1 ファイルで決まる ―― 相手は語彙
    # （関係型・属性名）であって他の整理結果ではない。誤字の宣言は「守っている
    # つもりで守られていない」を作り、しかも `build` がそのまま正本へ運ぶので、
    # 気づけるのは凍結の後（`check` の `E018`）になる ―― そのとき整理結果は
    # もう編集できない。**宣言が効いているかは書いた手元で言う。**
    # 「宣言したのに関係が書いてある」（古い宣言）は `_required_refs` の側で、
    # 全部読んでいる `gate` でしか言えない（関係は別のファイルにありうる）。
    report.findings += _gap_names(result, model)
    # `_concepts.yml` は 1 ファイルで決まる（台帳と語彙は入力であって他の
    # 整理結果ではない）ので lint でも見る ―― 素通りさせると、書いた内容が
    # 効いたか分かるのは build の後になる。
    report.findings += _proposal(round_, result, model, known)

    report.metrics = {
        "files": len(result.files),
        "records": len(result.records),
        "references": sum(1 for r in result.records if not r.complete),
        "out_of_scope": len(result.out_of_scope),
        "unreadable": sum(1 for o in result.out_of_scope if o.unreadable),
        # **書いた宣言が読まれたことを数で言う。** `G020` は `lint` では出ない
        # （関係は別のファイルにありうる）ので、宣言が効いたかどうかを確かめる
        # 手がかりが件数しか無い ―― 0 なら、書いた場所か綴りが違う。
        "known_gaps": sum(len(r.known_gaps) for r in result.records),
        # **読んだ予約名を数の外に出す。** `_concepts.yml` はレコードではない
        # ので `files` にも `records` にも入らず、それだけを渡すと出力が
        # 「0 ファイル / 0 レコード / error 0 / warn 0」になって、検査したのか
        # 素通りしたのかが打った人から区別できなかった。
        "proposals": list(result.special),
    }
    report.findings = order(report.findings)
    return report


def _relative(round_: Round, path: Any) -> str:
    """プロジェクト根からの相対。**指摘に載せる位置はここで 1 つに揃える。**"""
    try:
        return path.relative_to(round_.root).as_posix()
    except ValueError:                      # 根の外（起こらないが黙って壊さない）
        return path.as_posix()


def _orphans(result: organized_module.Organized,
             parsed: dict[str, mdio.ParsedFile]) -> list[Finding]:
    """**逆向きの対応検査** ―― 整理結果にあってパース結果に無いファイル。

    順方向（パース結果にあって整理結果に無い）は ``G001`` が見ている。逆は
    どこも見ていなかったので、次の 2 つが**黙って通っていた**。

    - パース結果のファイル名が変わった（資料が改訂されてシート名が変わった、
      パース結果を手でリネームした）。整理結果は前の名前のまま残り、**中身は
      1 件も正本に入らない**のに凍結が通る
    - 整理結果を先に書いた（相方がまだ無い）。書いた本人には正しく書いたように
      しか見えない

    レコード 1 件ずつではなく**ファイル 1 件で言う。** 50 レコードあるファイルが
    孤児になると 50 件並び、それが 3 ファイルあれば 150 件の山になる ―― 直す
    操作はどのみち「ファイルの名前を直す」1 つしかない。

    対象外宣言だけのファイル（レコード 0 件）も拾う。そこは ``G004`` の
    レコード側では 1 件も出ないので、**いちばん静かに消えるのがこの形**である。
    """
    findings: list[Finding] = []
    for file in result.files:
        if file in parsed:
            continue
        findings.append(Finding(
            "error", "G004", "",
            f"対応するパース結果がありません: {file}"
            "（整理結果とパース結果は名前で 1:1 です。資料が改訂されて"
            "シート名が変わっていないか確かめてください）",
            file=result.locations.get(file)))
    return findings


def _unread(result: organized_module.Organized,
            parsed: dict[str, mdio.ParsedFile]) -> list[Finding]:
    """**絵があるのに `未読取` のままの宣言。**

    画像化できるようになって、`未読取` は「読まずに済ませる楽な出口」になった
    ―― レコードを起こすより宣言するほうが速いからである。**絵が用意されている
    ことは機械が知っている**ので、そこだけは突く。

    warn に留めるのは、**絵を読んでもなお確定できないことがある**ためで、機械には
    真偽が決められない（そのときは理由を「絵を見たが確定できない」に書き換える）。

    絵はアンカー単位ではなく**パース結果 1 ファイル単位**で探す。宣言は表題のセル
    （`s4-x1`）に付き、絵は図形のアンカー（`s4-g1`）に貼られるのが普通で、
    アンカーを揃えて探すと**いちばん多い形を取りこぼす**。
    """
    findings: list[Finding] = []
    for entry in result.out_of_scope:
        if not entry.unreadable:
            continue
        document = parsed.get(entry.file)
        if document is None:
            continue
        pictures = [image for anchor in document.anchors
                    for image in mdio.images(anchor)]
        if not pictures:
            continue
        findings.append(Finding(
            "warn", "G015", entry.anchor,
            f"未読取 と宣言していますが、このシートには絵があります（{pictures[0]}"
            + (f" ほか {len(pictures) - 1} 枚" if len(pictures) > 1 else "")
            + "）。絵を読んだうえでの宣言ですか？"
              "（読んで確定できたならレコードにする、絵でも確定できないなら"
              "その旨を reason に書く）",
            file=entry.path or None, line=entry.line or None))
    return findings


# ── ② 語彙 ──────────────────────────────────────────────────────
def _vocabulary(result: organized_module.Organized, model: Metamodel) -> list[Finding]:
    """受け皿の無いレコードが ``build`` で黙って消えるのを防ぐ。"""
    findings: list[Finding] = []
    relations = set(model.relation_types)
    for record in result.records:
        # 参照だけのレコードは種別を名乗らない（G013 が「どこかに完全なレコードが
        # あるか」を見る）。
        if record.type and model.for_fact(record.type) is None:
            findings.append(Finding(
                "error", "G002", record.subject,
                f"語彙に無い種別です: {record.type}"
                "（_metamodel-add.yml で提案するか、既存種別に寄せてください）",
                file=record.path or None, line=record.line_of("type") or None))
        for ref in record.refs:
            if ref.rel not in relations:
                findings.append(Finding("error", "G002", record.subject,
                                        f"語彙に無い関係です: {ref.rel}",
                                        file=record.path or None,
                                        line=ref.line or record.line or None))
        findings += _attributes(record, model)
    return findings


def _attributes(record: organized_module.Record, model: Metamodel) -> list[Finding]:
    """**宣言に無い属性名を前倒しで言う**（``build`` の ``B021`` / ``B024`` 相当）。

    ``build`` は宣言に無い属性を**捨てる**。捨てたことは ``B021`` で言うが、
    それが出るのは凍結したあと ―― **整理結果はもう編集できない。** 直すには
    正本を手で埋めるか、ラウンドを起こし直すしかない。

    桁数を ``length`` ではなく ``桁数`` と書く、``pk`` を ``primary_key`` と書く、
    といった取り違えは語彙の綴りの問題であって意味の判断ではないので、**書いて
    いる最中に言えば直る**。関係の属性（``B024`` の側）は ``build`` が黙って
    捨てるだけなので、こちらは言われなければ気づく手がかりが無い。

    警告に留めるのは ``B021`` と同じ重みにするためである。ゲートの 5 条件は
    「通れば ``build`` が落ちない」ことの保証で、属性の取り違えは ``build`` を
    落とさない ―― **段の重みを勝手に上げると、いま凍結できているものが
    凍結できなくなる。**
    """
    findings: list[Finding] = []
    mapped = model.for_fact(record.type) if record.type else None
    if mapped is not None:
        declared = (model.item_types.get(mapped[0]) or {}).get("attributes") or {}
        stray = [key for key in record.attrs if key not in declared]
        if stray:
            findings.append(Finding(
                "warn", "G016", record.subject,
                f"{mapped[0]} に無い属性です: {'、'.join(sorted(stray))}"
                "（build が捨てます。arp4 model --attributes で名前を確かめてください）",
                file=record.path or None,
                line=record.line_of("attrs") or None))

        findings += _enum_values(record, declared, record.attrs, mapped[0],
                                 record.line_of("attrs") or record.line)

    for ref in record.refs:
        definition = model.relation_types.get(ref.rel)
        if definition is None:
            continue                          # G002 が報告済み
        declared = definition.get("attributes") or {}
        stray = [key for key in ref.attrs if key not in declared]
        if stray:
            findings.append(Finding(
                "warn", "G016", record.subject,
                f"{ref.rel} に無い属性です: {'、'.join(sorted(stray))}（build が捨てます）",
                file=record.path or None, line=ref.line or record.line or None))
        findings += _enum_values(record, declared, ref.attrs, ref.rel,
                                 ref.line or record.line)
    return findings


def _enum_values(record: organized_module.Record, declared: dict[str, Any],
                 written: dict[str, Any], owner: str,
                 line: Any) -> list[Finding]:
    """**宣言に無い enum の値**（``check`` の ``E011`` 相当）を前倒しで言う。

    値の検査は ``check`` にしか無かった ―― つまり出るのは **``freeze`` の後**で、
    そのときには整理結果はもう編集できない（直すには正本を ``overridden`` で
    上書きするか、ラウンドを起こし直すしかない）。**属性の名前は書いている最中に
    言うのに、値だけが凍結の向こう側にある**のは筋が通らない。

    しかも取り違えは意味の判断ではなく綴りの問題である ―― 実測（sales-corpus・
    30 冊）で、日本のテーブル定義書が書く ``CHAR`` / ``VARCHAR`` / ``DECIMAL`` を
    ``data-item.data_type``（enum: 数値/文字列/日付/…）へそのまま書いた整理結果が
    ``lint`` を error 0 / warn 0 で通っていた。**「lint が通った」を完了条件に
    配ると、値が語彙外のまま 200 ファイルが凍る。**

    ``extensible`` な enum は対象外である ―― 寄せ先を増やしてよい語彙なので、
    宣言に無い値は「資料の語をそのまま採った」正しい整理でありうる。

    段は ``warn`` に留める（``G016`` と同じ理由）。ゲートの 5 条件は「通れば
    ``build`` が落ちない」ことの保証で、値の取り違えは ``build`` を落とさない
    ―― **段の重みを勝手に上げると、いま凍結できているものが凍結できなくなる。**
    """
    findings: list[Finding] = []
    for name, value in (written or {}).items():
        attribute = declared.get(name)
        if not isinstance(attribute, dict):
            continue                          # G016 が報告済み
        if attribute.get("kind") != "enum" or attribute.get("extensible"):
            continue
        values = attribute.get("values") or []
        candidates = value if isinstance(value, list) else [value]
        stray = [c for c in candidates if c not in values]
        if not stray:
            continue
        findings.append(Finding(
            "warn", "G028", record.subject,
            f"{owner}.{name} が enum 外です: {'、'.join(map(str, stray))}"
            f"（{'、'.join(map(str, values))} のどれか）"
            "。資料がその値を言っていないなら空けてください"
            "（enum は行き先の一覧であって、選ぶ根拠ではありません）",
            file=record.path or None, line=line or None))
    return findings


def _metamodel_add(round_: Round, result: organized_module.Organized,
                   model: Metamodel) -> list[Finding]:
    """語彙の追加提案が**人に承認されるまで凍結を通さない。**

    例外は ``status: deferred``（保留）である。コード資産の通しでは提案が
    必ず出る（実測 r001 で 5 件）ので、承認待ちで凍結が止まると**無承認の
    通し実行が構造的にできない** ―― 保留を一級にすれば、提案は決定ログとして
    残ったまま凍結が通り、人は止めたいときだけ介入すればよい。

    保留には ``deferred_reason`` が必須である。理由の無い保留は、提案ファイルを
    ``organized/`` の外へ退避する規約外運用（r001 で実際に起きた）と同じで、
    なぜ寄せなかったかが記録に残らない。
    """
    findings: list[Finding] = []
    for entry in result.metamodel_add.get("add_item_types") or []:
        if not isinstance(entry, dict) or not entry.get("name"):
            continue
        name = str(entry["name"])
        if name in model.item_types or model.for_fact(name) is not None:
            continue
        location = _relative(round_, round_.metamodel_add)
        if str(entry.get("status") or "") == "deferred":
            reason = str(entry.get("deferred_reason") or "")
            if reason:
                findings.append(Finding(
                    "warn", "G007", name,
                    f"語彙の追加提案を保留しています: {name}（{reason}）"
                    "。凍結は通ります。採るなら .arp/spec/metamodel.yml へ反映"
                    "してください", file=location))
            else:
                findings.append(Finding(
                    "error", "G007", name,
                    f"status: deferred には deferred_reason が要ります: {name}"
                    "（理由の無い保留は、提案を記録せずに捨てるのと区別できません）",
                    file=location))
            continue
        findings.append(Finding(
            "error", "G007", name,
            f"語彙の追加提案が未承認です: {name}"
            "（.arp/spec/metamodel.yml に反映して承認するか、"
            "status: deferred と deferred_reason で保留してください）",
            file=location))
    return findings


# ── ③ concept ───────────────────────────────────────────────────
def _concepts(round_: Round, result: organized_module.Organized,
              known: dict[str, Concept]) -> list[Finding]:
    """**機械がマージできない状態での凍結**を防ぐ。

    このラウンドで作られる concept（レコードが名乗ったもの＋整理②の ``new``）と、
    既に台帳にあるものを合わせて「実在する」とみなす。
    """
    declared = set(known)
    declared |= {str(e.get("concept")) for e in (result.concepts.get("new") or [])
                 if isinstance(e, dict) and e.get("concept")}
    declared |= {r.concept for r in result.records}

    findings: list[Finding] = []
    for record in result.records:
        for ref in record.refs:
            if ref.to not in declared:
                findings.append(Finding(
                    "error", "G003", record.subject,
                    f"参照先の concept がありません: {ref.to}",
                    file=record.path or None,
                    line=ref.line or record.line or None))
    for entry in result.concepts.get("assign") or []:
        if isinstance(entry, dict) and str(entry.get("concept") or "") not in declared:
            findings.append(Finding("error", "G003", str(entry.get("concept") or ""),
                                    f"assign の相手がありません: {entry.get('concept')}",
                                    file=_relative(round_, round_.concepts)))
    return findings


# ── ④ 参照だけのレコードの受け皿 ────────────────────────────────
def _defined(result: organized_module.Organized,
             known: dict[str, Concept]) -> list[Finding]:
    """参照だけのレコードの ``concept`` に、種別を決める根拠があるか。

    **参照だけのレコードは種別を名乗らない**ので、``build`` は「同じラウンドの
    完全なレコード」か「台帳（前ラウンドの成果）」から引く。どちらにも無ければ
    アイテムが起こせない ―― 凍結を通してから ``build`` で落ちるのを防ぐ。
    """
    defining = {r.concept for r in result.records if r.complete}
    defining |= {concept for concept, entry in known.items() if entry.type}
    findings: list[Finding] = []

    for concept in sorted({r.concept for r in result.records
                           if not r.complete and r.concept not in defining}):
        where = sorted((r for r in result.records if r.concept == concept),
                       key=lambda r: r.target)
        first = where[0]
        findings.append(Finding(
            "error", "G013", first.subject,
            f"参照だけのレコードしかありません: {concept}"
            "（いちばん詳しい 1 シートで type / name / statement を書いてください）"
            + (f"。ほか {len(where) - 1} 件" if len(where) > 1 else ""),
            file=first.path or None, line=first.line or None))
    return findings


# ── ⑤ 関係の組み合わせ ──────────────────────────────────────────
def _item_types(result: organized_module.Organized, model: Metamodel,
                known: dict[str, Concept]) -> dict[str, str]:
    """concept → 正本のアイテム種別。**このラウンドの宣言が台帳より優先。**

    台帳側は整理結果と同じ「ファクト種別」を持っているので、写像表を通して
    アイテム種別に直してから使う。参照だけのレコードは種別を名乗らないので、
    完全なレコードか台帳から引かれる（引けなければ ``G013``）。
    """
    types: dict[str, str] = {}
    for concept, entry in known.items():
        mapped = model.for_fact(entry.type) if entry.type else None
        if mapped:
            types[concept] = mapped[0]
    for record in result.records:
        mapped = model.for_fact(record.type) if record.type else None
        if mapped:
            types[record.concept] = mapped[0]
    return types


def _pairs(result: organized_module.Organized, model: Metamodel,
           known: dict[str, Concept]) -> list[Finding]:
    """``refs`` の起点・終点の種別が宣言と合うか（**build の B013 を前倒しする**）。

    種別が分からない相手（別ラウンドで整理された concept で、台帳に種別が
    載っていない）は見ない ―― **知らないことを error にしない。**
    """
    types = _item_types(result, model, known)
    findings: list[Finding] = []

    for record in result.records:
        source_type = types.get(record.concept)
        if source_type is None:
            continue                              # G002 / G013 が報告済み
        for ref in record.refs:
            definition = model.relation_types.get(ref.rel)
            target_type = types.get(ref.to)
            if definition is None or target_type is None:
                continue                          # G002 / G003 が報告済み
            if mm.orient(definition, source_type, target_type) is not None:
                continue
            findings.append(Finding(
                "error", "G012", record.subject,
                f"{ref.rel} は {source_type} と {target_type} の間に張れません"
                f"（{ref.rel}: {'、'.join(definition.get('from') or ['*'])}"
                f" → {'、'.join(definition.get('to') or ['*'])}）",
                file=record.path or None, line=ref.line or record.line or None))
    return findings


# ── コード整理の規約 ────────────────────────────────────────────
#: 取り込みの塊のアンカー。コードのパース結果でファイル全体を指す唯一のアンカー。
IMPORT_ANCHOR = "i1"

#: コード由来のアンカーの形（``m1`` / ``v1`` / ``i1`` / ``t1`` / ``p1`` /
#: Java の ``j1``）。規約の検査はこの形にしか掛けない ―― Excel のシートの
#: アンカー（``s4-t2``）には当たらないので、資料の整理で鳴ることは無い。
_CODE_ANCHOR = re.compile(r"^[mvitpj]\d+$")

#: 特殊メソッド（dunder）の名前。
_DUNDER = re.compile(r"__\w+__$")

#: コードの concept の接頭辞（organize.md の命名規約 ―― パスから機械的に導く）。
_MOD_PREFIX = "c-mod-"

#: ファイル単位のモジュールの出典に**できない**塊の見出し（先頭一致）。
#: ファイル自身を指す塊は「モジュール関数」と「取り込み」だけである。
_NOT_FILE_CHUNKS = ("クラス:", "テストクラス:", "定数", "テスト")


def _conventions(result: organized_module.Organized,
                 parsed: dict[str, mdio.ParsedFile], model: Metamodel,
                 known: dict[str, Concept]) -> list[Finding]:
    """コード整理の規約。**lint が許す自由度＝ブレの発生源**なので検査に落とす。

    整理を 4 エージェントで分担した実測（r001）で、規約に無い判断 ―― calls の
    粒度・dunder の起票・package の根拠・関数の塊が無いファイルの出典 ―― が
    人数分に割れ、整理②で 14 ファイルの修正が要った。**割れた箇所は規則が
    未成文だった箇所と正確に一致する**ので、文書（organize.md）に書くだけでなく
    機械の検査に落とす（文書は読まれないことがあるが、lint は必ず読まれる）。

    段はどれも warn である ―― どの形も ``build`` を落とさない（ゲートの約束
    「通れば build は原理的に失敗しない」に関わらない）ので、``G005`` / ``G016``
    と同じ重みにする。
    """
    types = _item_types(result, model, known)
    findings: list[Finding] = []
    for record in result.records:
        if not _CODE_ANCHOR.match(record.anchor):
            continue

        # ① 取り込み（i1）から張る calls の相手はモジュールに畳む。名指しの
        #    取り込み（from x import f）をメソッドへ張るかは r001 で担当ごとに
        #    割れた ―― import が言っているのは「このモジュールに依存する」まで
        #    である（関数単位の依存は呼び出し行にしか無い）。
        if record.anchor == IMPORT_ANCHOR:
            for ref in record.refs:
                if ref.rel == "calls" and types.get(ref.to) == "method":
                    findings.append(Finding(
                        "warn", "G022", record.subject,
                        f"取り込みから張る calls の相手がメソッドです: {ref.to}"
                        "（名指しの取り込みでも相手はモジュールへまとめてください"
                        "。メソッド単位にすると粒度が担当ごとに割れます）",
                        file=record.path or None,
                        line=ref.line or record.line or None))

        mapped = model.for_fact(record.type) if record.type else None
        item_type = mapped[0] if mapped else ""

        # ② dunder（__init__ 等）は method に起こさない。本数は親の description
        #    に申告する ―― 起こすかどうかが担当ごとに 0% と 100% に割れた形。
        if item_type == "method" and _DUNDER.search(record.name or record.concept):
            findings.append(Finding(
                "warn", "G023", record.subject,
                f"特殊メソッド（dunder）を起こしています: {record.name or record.concept}"
                "（起こさず、親のレコードの description に本数で申告してください"
                "。「載せない」と「無い」を混ぜないための規約です）",
                file=record.path or None, line=record.line or None))

        # ③ package はファイルの路（concept のパス）から取る。路に無い値を
        #    書くなら、越境の根拠を description に書く（越境そのものは許す）。
        if item_type == "module" and record.concept.startswith(_MOD_PREFIX):
            package = str(record.attrs.get("package") or "")
            path = record.concept[len(_MOD_PREFIX):]
            if (package and not _in_path(package, path)
                    and not str(record.attrs.get("description") or "")):
                findings.append(Finding(
                    "warn", "G024", record.subject,
                    f"package がファイルの路に現れません: {package}（路: {path}）"
                    "。路から取るのが規約です。越境させるなら根拠を "
                    "description に書いてください",
                    file=record.path or None,
                    line=record.line_of("attrs") or record.line or None))

        # ④ ファイル単位のモジュールの出典は「モジュール関数」の塊か i1。関数の
        #    塊が無いファイルで、クラス・定数・テストの塊を出典にする判断が割れた。
        if (item_type == "module" and record.complete
                and not record.attrs.get("class_name")):
            document = parsed.get(record.file)
            anchor = document.by_id.get(record.anchor) if document else None
            head = (anchor.body.splitlines() or [""])[0] if anchor else ""
            if head.startswith(_NOT_FILE_CHUNKS):
                findings.append(Finding(
                    "warn", "G025", record.subject,
                    f"ファイル単位のモジュールの出典が「{head}」の塊です"
                    "（ファイル自身を指す塊は「モジュール関数」と取り込み（i1）"
                    "だけ。関数の塊が無いファイルの出典は i1 にしてください）",
                    file=record.path or None,
                    line=record.line_of("source") or record.line or None))
    return findings


# ── 文章化スロット（arp4 draft の TODO） ────────────────────────
#: draft が空けた文章化スロット。**残っているあいだは凍結できない**（G001 と
#: 同じく作業キューに載る）―― 骨格だけの正本を黙って作らないため。
_TODO = re.compile(r"<TODO[^>]*>")

#: 抽出的文章の文字数のレンジ（G027）。下限は「〜こと」だけの空文を、上限は
#: docstring の丸写しを弾く ―― 抽出（写し）であって生成（言い換え）ではない、
#: の外形的な検査である。
_STATEMENT_RANGE = (10, 200)

#: 識別子の形の名前（この形なら statement への包含を求める）。日本語のテスト名
#: には求めない ―― 全文の包含を強いると、名前の言い換え（organize.md の
#: 「name は対象を名指しする」）ができなくなる。
_IDENT = re.compile(r"^[A-Za-z_][\w.]*$")


def _todo_slots(result: organized_module.Organized) -> list[Finding]:
    """draft の TODO が残っているレコード。**文章化はゲートの内側**である。

    error にするのは、TODO の残った statement がそのまま正本に入ると、設計書に
    ``<TODO …>`` が印字されるため ―― 「通れば build は原理的に失敗しない」の
    build には「読める設計書が出る」ことまで含める。
    """
    findings: list[Finding] = []
    for record in result.records:
        holes = [key for key, value in
                 (("name", record.name), ("statement", record.statement))
                 if _TODO.search(value)]
        holes += [f"attrs.{key}" for key, value in sorted(record.attrs.items())
                  if isinstance(value, str) and _TODO.search(value)]
        if not holes:
            continue
        findings.append(Finding(
            "error", "G026", record.subject,
            f"文章化が残っています: {'、'.join(holes)}"
            "（draft の <TODO 抽出元 …> を、抽出元を読んで埋めてください）",
            file=record.path or None, line=record.line or None))
    return findings


def _extraction(result: organized_module.Organized) -> list[Finding]:
    """抽出的文章の検査（``arp4 draft`` が書いたファイルだけ）。

    文章は docstring・シグネチャからの**写し（抽出）**であることを外形で見る ――
    ①文字数のレンジ、②識別子の包含（名前が識別子の形のとき）。抽出元アンカーの
    併記は draft が ``source.anchor`` として機械が書くので、ここでは照合しない
    （実在は G004、語の照合は G005 が見ている）。

    **言い換え（生成）はブレの発生源**である ―― 同じ docstring から 4 エージェント
    が 4 通りの文章を書けば、再現性の検査（バイト一致）はそこで終わる。warn に
    留めるのは、抽出かどうかの真偽を機械が決め切れないため（G005 と同じ規律）。
    """
    findings: list[Finding] = []
    low, high = _STATEMENT_RANGE
    for record in result.records:
        if record.file not in result.drafted or not record.complete:
            continue
        if _TODO.search(record.statement):
            continue                      # G026 が報告済み
        trouble: list[str] = []
        if not low <= len(record.statement) <= high:
            trouble.append(f"文字数がレンジ外です（{len(record.statement)} 字 / "
                           f"{low}〜{high} 字）")
        ident = record.name.rsplit(".", 1)[-1]
        if (_IDENT.match(record.name) and ident
                and ident not in record.statement):
            trouble.append(f"識別子 {ident} が本文にありません"
                           "（抽出元の名前をそのまま置いてください）")
        if trouble:
            findings.append(Finding(
                "warn", "G027", record.subject,
                "、".join(trouble) + "。文章は抽出元の写しであること"
                "（言い換えは再現性を壊します）",
                file=record.path or None,
                line=record.line_of("statement") or record.line or None))
    return findings


# ── description への逃がし（G029 / G030） ──────────────────────
#: ``description`` の箇条を切る記号。整理層が書くのは ``A ／ B ／ C`` の形である。
_ESCAPE_SPLIT = re.compile(r"[／/｜|、]")

#: 見出しと値を切る記号。``初期値 システム日付`` / ``初期値: システム日付``。
_ESCAPE_HEAD = re.compile(r"[ 　:：=＝]")

#: 見出しが何件重なったら「受け皿の属性が要る」と言うか（``G030``）。実測（r001）で
#: 外部インターフェースの「異常時の扱い」は 5 件、`process-step` の「参照テーブル」は
#: 35 件そろっていた ―― 3 件までは資料 1 枚の言い回しでありうる。
_ESCAPE_REPEAT = 4


def _headings(text: Any) -> list[str]:
    """``物理名 orderNo ／ 初期値 システム日付`` → ``["物理名", "初期値"]``。

    **値を持たない断片は見出しではない。** 区切りの中に値が無い（空白も記号も
    無い）ものは、ただの散文か箇条の 1 項目である ―― そこまで見出しと数えると
    補足の 1 文がまるごと見出しの列になる。
    """
    found: list[str] = []
    for segment in _ESCAPE_SPLIT.split(str(text or "")):
        segment = segment.strip()
        if not segment:
            continue
        head = _ESCAPE_HEAD.split(segment, maxsplit=1)[0].strip()
        if head and head != segment:
            found.append(head)
    return found


def _receptacles(model: Metamodel, owner: str,
                 relation: bool) -> dict[str, str]:
    """その種別が**宣言している**欄 ―― ``見出しに書かれうる語 → 属性名``。

    見るのは**属性の ``label`` と属性名の完全一致**だけである（部分一致にすると
    「条件」が「出力条件」に当たる）。
    """
    declared = ((model.relation_types if relation else model.item_types)
                .get(owner) or {}).get("attributes") or {}
    found: dict[str, str] = {}
    for name in declared:
        found[str(name)] = str(name)
        found[model.label(str(name))] = str(name)
    return found


def _descriptions(result: organized_module.Organized,
                  model: Metamodel) -> list[Finding]:
    """``description`` に流れた事実を、**書いた直後の手元で**言う。

    ``description`` は :data:`arp4.metamodel.RELATION_RESERVED` の予約キーなので、
    宣言なしにどの関係へも書ける ―― ``G016``（宣言に無い属性名）にも ``G028``
    （enum 外）にも当たらず、**スキーマ検査を素通りする。** 実測（r001）で、
    整理層は同じ誤りを 2 度した::

        1 度目   displays 164 本すべてが note 空（欄を埋めなかった）
        2 度目   displays 154 本の初期値・物理名が description へ流れた

    ``organize.md`` は 1 度目のあとに「初期値・物理名は `displays` の `note` へ
    写す」と名指ししたが、**散文で規則を強めても直らなかった。** ここは同じ規則を
    機械に移したものである。

    ========  ==============================================================
    ``G029``  **宣言済みの欄があるのに** ``description`` へ流した（warn）
    ``G030``  同じ見出しが種別内で重なる ―― **受け皿の属性が無い**（warn）
    ========  ==============================================================

    どちらも warn である。``description`` は補足の受け皿として要る（決定 70 の
    ``merge: append``）ので、**書ける場所を減らす話ではない** ―― 変えるのは
    「書けるのに設計書のどの列にも出ない」ほうである（→ ``P111``）。

    **``description`` が出ないとは言わないこと。** ここは ``Metamodel`` しか
    受け取らず、様式（パックの ``documents/*.yml``）を読んでいない ―― 出るか
    出ないかは様式の側でしか決まらないので、ここから断定できるのは「**書かな
    かった欄の列が空になる**」ことだけである。実測（jp-sier-std の
    ``documents/*.yml`` を機械集計）で ``description`` はほぼ全節で列になる::

        節を持つ種別        アイテム・関係とも、**その全節が** description 列を持つ
        節を持たない種別    アイテム側（data-item・code-value・method・
                            process-step・flow-step・index・batch-step・
                            test-run）は関係の節が `to.description` /
                            `from.description` で出す
        列を持たない節      `kind: matrix` と `kind: trace` だけ

    :func:`arp4.audit._unsurfaced` の docstring にある「``description`` 628 件の
    うち 528 件（84%）がどの生成物の本文にも現れなかった」は**その列が足される
    前の実測**であって、いまの様式の姿ではない（同じ ``.arp`` に ``check
    --code P111`` を掛けても ``*.description`` は 1 件も出ない）。**過去の実測を
    現在形のヒントに書かないこと** ―― 実測（8 分担）で、この一文を読んだ 5 人が
    「``description`` に置くと消える」と解して値を ``statement`` の文中へ畳み、
    **列としては二度と引けなくなった。**

    **中身が正しいかは見ない。** 機械が言えるのは**置き場所**だけで、その
    ``description`` が事実かどうかは人と ``decision`` の仕事である。
    """
    findings: list[Finding] = []
    #: (種別, 先頭見出し) → その見出しを持つレコード（``G030`` が数える）。
    repeated: dict[tuple[str, bool, str], list[tuple[str, int]]] = {}
    #: (ファイル, 種別) → (件数, 写す先の欄, 最初の行)。**ファイル 1 件で言う。**
    escaped: dict[tuple[str, str], tuple[int, dict[str, str], int]] = {}

    def look(owner: str, relation: bool, written: dict[str, Any],
             path: str, line: int) -> None:
        told = written.get("description")
        headings = _headings(told)
        if not headings:
            return
        receptacles = _receptacles(model, owner, relation)
        matched = [(head, receptacles[head]) for head in headings
                   if head in receptacles and receptacles[head] != "description"]
        # **1 つだけの一致では言わない。** 補足の 1 文が偶然に欄の名前で始まる
        # ことはある（「条件 が複雑なため…」）―― 2 つ並んで初めて、欄に割って
        # 書けたものを 1 つの散文に畳んだと言える。埋まっている欄を言い直して
        # いるだけのときも黙る（TC-C3）。
        if len(matched) >= 2 and any(
                not str(written.get(attribute) or "") for _head, attribute in matched):
            count, where, first = escaped.get((path, owner), (0, {}, line))
            escaped[(path, owner)] = (count + 1, {**where, **dict(matched)},
                                      first or line)
        if headings[0] not in receptacles:
            repeated.setdefault((owner, relation, headings[0]), []).append(
                (path, line))

    for record in result.records:
        mapped = model.for_fact(record.type) if record.type else None
        if mapped is not None:
            look(mapped[0], False, record.attrs,
                 record.path, record.line_of("attrs") or record.line)
        for ref in record.refs:
            if ref.rel not in model.relation_types:
                continue                      # G002 が報告済み
            look(ref.rel, True, {**ref.attrs, "description": ref.note},
                 record.path, ref.line or record.line)

    # **レコード 1 件ずつ並べない**（:func:`_orphans` と同じ規律）。実測（r001）で
    # `displays` は 1 ファイルに 154 本あり、直す操作は「その欄へ写す」1 つしか
    # 無い ―― 154 行に割ると、他の指摘がその中に埋もれる。
    for (path, owner), (count, where, line) in sorted(escaped.items()):
        listed = "、".join(f"「{head}」は {attribute}"
                           for head, attribute in sorted(where.items()))
        findings.append(Finding(
            "warn", "G029", owner,
            f"{count} 件の description に {owner} が宣言している欄が"
            f"書かれています（{listed} へ写す欄です）"
            "。欄に書かないと、その欄の列は空のままです"
            "（description 側は「補足」列を持つ章なら本文に残りますが、"
            "欄ごとの列にはならないので、並べ替えにも突き合わせにも使えません）",
            file=path or None, line=line or None))

    for (owner, _relation, head), places in sorted(repeated.items()):
        if len(places) < _ESCAPE_REPEAT:
            continue
        path, line = places[0]
        findings.append(Finding(
            "warn", "G030", owner,
            f"「{head}」が {len(places)} 件の description にありますが、"
            "受け皿の属性がありません",
            file=path or None, line=line or None,
            hint="_metamodel-add.yml に属性を提案してください"
                 "（欄になれば設計書の列になり、並べ替えにも突き合わせにも"
                 "使えます）。通るまでは値を description に置いたままにします "
                 "―― description を「補足」列に出す章なら本文には残り、"
                 "その列を持たない章なら残りません（どちらかは様式"
                 "（パックの documents/*.yml）次第で、ここからは分かりません。"
                 "凍結後に `arp4 check` の `P111` が言います）。"
                 "**statement へ畳まないこと** ―― 畳むと列としては二度と"
                 "引けません"))
    return findings


def _in_path(package: str, path: str) -> bool:
    """``package`` が路のドット区切りの**連続部分列**として現れるか。"""
    parts = path.split(".")
    want = package.split(".")
    if not want:
        return True
    return any(parts[i:i + len(want)] == want
               for i in range(len(parts) - len(want) + 1))


# ── 補助 ────────────────────────────────────────────────────────
def _anchors(result: organized_module.Organized,
             parsed: dict[str, mdio.ParsedFile]) -> list[Finding]:
    """**幻覚の最頻形は「存在しない出典」。** 全件を機械で潰せる。"""
    findings: list[Finding] = []
    for record in result.records:
        document = parsed.get(record.file)
        if document is None:
            continue                      # ファイル単位で _orphans が 1 件だけ言う
        if record.anchor not in document.by_id:
            findings.append(Finding(
                "error", "G004", record.subject,
                f"アンカーがありません: {record.anchor}"
                f"{_moved(record.anchor, document)}",
                file=record.path or None, line=record.line or None))
    for entry in result.out_of_scope:
        document = parsed.get(entry.file)
        if document is not None and entry.anchor not in document.by_id:
            findings.append(Finding("error", "G004", entry.anchor,
                                    "対象外宣言のアンカーがありません"
                                    f"{_moved(entry.anchor, document)}",
                                    file=entry.path or None,
                                    line=entry.line or None))
    return findings


#: アンカーの形（``s4-t2`` ―― シートの並び順とシートの中の通し番号）。
_ANCHOR = re.compile(r"^s(\d+)-(.+)$")


def _moved(anchor: str, document: mdio.ParsedFile) -> str:
    """**同じ塊が別のシート番号で出ていないか。** 出ていれば理由まで言える。

    アンカーの ``s4`` は**ブックの中でシートが何枚目か**である。資料が改訂されて
    シートが 1 枚差し込まれると、それより後ろのシートは番号がまとめてずれる ――
    パース結果のファイル名（シート名）は変わらないので、**同じファイルの同じ表が
    別の番地になる。**

    ここは黙って壊れる側ではない（番号がずれた先は他のシートのファイルにあり、
    このファイルには無いので必ず ``G004`` になる）が、**落ちる理由が
    「アンカーがありません」としか出ていなかった** ―― 200 件まとめて落ちた
    ときに、整理結果を書き直す話なのか資料が変わった話なのかが分からない。
    どちらであるかはパース結果に書いてあるので、これは転記である。
    """
    found = _ANCHOR.match(anchor)
    if not found:
        return ""
    same = [one for one in document.by_id
            if (other := _ANCHOR.match(one)) and other.group(2) == found.group(2)]
    if not same:
        return ""
    return (f"（同じ塊が `{same[0]}` にあります。ブックにシートが差し込まれた"
            "か並び順が変わったようです。アンカーの `s` はシートが何枚目かなので、"
            "資料が改訂されると後ろのシートがまとめてずれます）")


def _wording(result: organized_module.Organized,
             parsed: dict[str, mdio.ParsedFile]) -> list[Finding]:
    """本文に語がまったく無いレコード。**作文の疑い**として warn で出す。

    3 の ``evidence``（原文の写経）を廃したので、代わりにこれが「出典にない主張」を
    拾う。**言い換えは許される**ので error にはできない ―― 機械に真偽は決められない。

    **本文の側に語が 1 つも無いときは黙る。** 取り込みを 1 本も持たないモジュールの
    ``i1`` は見出しだけの表なので、突き合わせる語が**原理的に 0 個**である ――
    そこで「本文に出てこない語だけで書かれています」と言うのは、一致しなかったこと
    ではなく**照合できなかったこと**を報告している。決定 36 は「識別子をそのまま
    ``name`` に置く」で応じる読み方を示したが、置くべき識別子が本文に無い。
    ``_member`` や ``_pairs`` と同じ規律で、**決められないなら黙る**。
    """
    findings: list[Finding] = []
    for record in result.records:
        document = parsed.get(record.file)
        if document is None:
            continue
        anchor = document.by_id.get(record.anchor)
        if anchor is None:
            continue
        if _TODO.search(record.statement):
            continue                      # 文章化前（G026 が報告済み）
        words = _words(f"{record.name} {record.statement}")
        if not words:
            continue
        haystack = anchor.text
        # 突き合わせる語が本文に 1 つも無いなら黙る（照合できなかったことを、
        # 一致しなかったことにしない）。**1 字の語幹は数えない** ―― 見出しは
        # arp4 自身が書く（`取り込み` → 取・込）ので、見出しだけの表まで
        # 「語がある」ことになってしまう。
        if not any(len(w) >= _MIN_WORD for w in _words(haystack)):
            continue
        if _grounded(words, haystack, record.name):
            continue
        findings.append(Finding(
            "warn", "G005", record.subject,
            f"本文に出てこない語だけで書かれています（{record.anchor}）"
            "。出典を取り違えていないか確かめてください",
            file=record.path or None, line=record.line or None))
    return findings


def _columns(result: organized_module.Organized,
             parsed: dict[str, mdio.ParsedFile], model: Metamodel) -> list[Finding]:
    """出典の欄は埋まっているのに、整理結果でその属性が空。**照合だけである。**

    ``G005`` の裏返しにあたる ―― あちらは「出典に無いことを書いた」を疑い、
    こちらは「**出典に書いてあったことを書かなかった**」を数える。意味の判断は
    1 つも要らない（欄が空かどうかしか見ない）ので、整理層ではなく機械の仕事である。

    **見るのは「落ちた」だけで、「違う」は見ない。** 書き換え方には幅がある ――
    ``例外`` が ``YamlError, _broken`` のとき ``raises: YamlError`` と書くのは
    private なヘルパを落としただけで正しい。どちらが正かは機械には言えない。
    空欄だけは言える。**資料に書いてあったものが 1 つも残っていない。**

    逆向き（属性は埋まっているのに出典の欄が空）は出さない。Excel の資料では
    欄の外 ―― 備考欄や下の注記 ―― に書いてあることがあり、**機械には「無い」と
    「別のところにある」の区別が付かない**。コードだけを見て決めると、シートの
    資料で誤検出の山になる。

    実測（``src/arp4`` 26 ファイル / メソッド 135 件）では 1 件出て、誤検出は 0 ――
    ``yamlio.marked`` の ``例外`` が ``_broken`` と読めていたのに落ちていた。
    ``シグネチャ`` と ``戻り値`` は 135 件とも埋まっていた。
    """
    findings: list[Finding] = []
    for record in result.records:
        document = parsed.get(record.file)
        if document is None:
            continue
        anchor = document.by_id.get(record.anchor)
        if anchor is None:
            continue                      # G004 が報告済み
        mapped = model.for_fact(record.type) if record.type else None
        if mapped is None:
            continue                      # G002 / G013 が報告済み
        declared = (model.item_types.get(mapped[0]) or {}).get("attributes") or {}
        bound = {str(attr["column"]): name
                 for name, attr in declared.items() if (attr or {}).get("column")}
        if not bound:
            continue                      # 欄と属性の対応が宣言されていない種別
        table = mdio.rows(anchor)
        if len(table) < 2:
            continue
        row = _member(record.name, table[1:])
        if row is None:
            continue                      # どの行を指しているか決められない
        for column, value in zip(table[0], row):
            attribute = bound.get(column)
            if not attribute or not value or record.attrs.get(attribute):
                continue
            # **どの行の話かは名前で言う。** アンカーはメンバの表 1 枚を指していて
            # （``m3`` に 8 メソッド）、宛先だけでは開いたあと探すことになる。
            findings.append(Finding(
                "warn", "G018", record.subject,
                f"{record.name}: 出典の「{column}」欄は埋まっている（{value}）のに "
                f"{attribute} がありません"
                "。資料に書いてあることを落としていないか確かめてください",
                file=record.path or None,
                line=record.line_of("attrs") or record.line or None))
    return findings


def _gap_names(result: organized_module.Organized,
               model: Metamodel) -> list[Finding]:
    """``known_gaps`` の名前が語彙にあるか（``G031``）。**誤字は宣言を無効にする。**

    判定も材料も正本側の :func:`arp4.gaps.check`（``E018``）と同じで、違うのは
    **鳴る時期だけ**である（``G020`` / ``G028`` と同じ話）。``build`` は宣言を
    そのまま正本へ運ぶので、綴りを間違えた宣言は

    * ``G020`` を降ろさない（宣言したのに warn が残る）
    * 正本に入ってから ``E018`` で error になる（**整理結果はもう編集できない**）

    の二重に効く。どちらも書いている手元でなら 1 文字直せば済む。error にするのは
    ``E018`` と段を揃えるためで、``G016``（宣言に無い属性名。``build`` が黙って
    捨てるだけ）とは行き先が違う ―― こちらは**凍結の向こうで error になる。**

    名前は**関係型と属性名の両方**を受ける（正本の ``known_gaps`` がそうなので
    揃える）。属性名は種別が決まって初めて引けるので、**参照だけのレコード
    （種別を名乗らない）では関係型しか照合できない** ―― そこで決められないものは
    黙る（:func:`_pairs` と同じ規律。知らないことを error にしない）。
    """
    relations = set(model.relation_types)
    findings: list[Finding] = []
    for record in result.records:
        if not record.known_gaps:
            continue
        mapped = model.for_fact(record.type) if record.type else None
        attributes = set(((model.item_types.get(mapped[0]) or {})
                          .get("attributes") or {})) if mapped else set()
        for name in sorted(record.known_gaps):
            if name in relations or name in attributes:
                continue
            if mapped is None:
                continue              # 種別が決まらない ―― 属性名かもしれない
            findings.append(Finding(
                "error", "G031", record.subject,
                f"known_gaps に語彙の無い名前が挙がっています: {name}"
                f"（{mapped[0]} の属性名か、関係型の名前で書いてください。"
                "arp4 model で確かめられます）",
                file=record.path or None,
                line=record.line_of("known_gaps") or record.line or None))
    return findings


def _required_refs(result: organized_module.Organized, model: Metamodel,
                   report: Report | None = None) -> list[Finding]:
    """語彙が「1 本は要る」と宣言している関係が、1 本も書かれていない。

    判定そのものは :func:`arp4.validate._coverage`（``W031``）と同じで、材料も
    同じ ``warn_if_no_upstream`` である。**違うのは鳴る時期だけ**である ――
    ``W031`` は ``arp4 check``、つまり ``build`` が正本を書いたあとに鳴る。
    そのころ整理層はもう次のファイルへ移っていて、``constrains`` を張る先が
    本文のどこに書いてあったかは**開き直さないと分からない**。実測では制約
    32 件が 32 件とも繋がっておらず、それが分かったのは生成された要件定義書を
    人が読んだときだった。**書いている本人の手元で鳴れば 1 件ずつ直せる。**

    ``lint`` には出さない。同じ concept のレコードは複数のファイルに散るので、
    関係が**別のファイルに書いてある**ことがある ―― 1 ファイルだけ読んで
    「1 本も無い」と言うと嘘になる。``_pairs`` の規律（相手が分からなければ
    黙る）と同じで、**全部読んでいる ``gate`` でしか言えない**。

    数えるのは concept 単位である。レコード単位にすると、同じ concept を
    2 ファイルで書いて関係を片方にだけ張った正しい整理が誤検出になる。

    **「調べたうえで相手がいない」は宣言で降ろせる。** レコードの ``known_gaps``
    にその関係型を理由つきで書けば ``G020`` を出さない ―― 縛る先がこのラウンドの
    資料に入っていない制約は実在し、宣言する口が無いと**正しく処理した warn が
    永久に鳴り続けて、処理済みと未処理が一覧の上で区別できなくなる**（正本側で
    ``W031`` → ``W032`` が同じことをしている理由と同じ。→ :mod:`arp4.gaps`）。
    降ろしたぶんは :attr:`Report.gaps_used` に積んで**必ず集計に出す** ――
    黙って消えるのがいちばん悪い形である。

    宣言は concept 単位に集める（``written`` と同じ規律）。同じ concept を
    2 ファイルで書いて、宣言を片方にだけ書いた整理は正しい。

    逆に**関係が書いてあるのに宣言が残っている**なら、その宣言はもう要らない
    （``check`` の ``W033`` と同じ）。正本へ運ぶ前にここで言う ―― 古い言い訳が
    正本に残り続けるほうが、error より始末が悪い。
    """
    needed: dict[str, str] = {}
    for name, definition in model.item_types.items():
        upstream = (definition or {}).get("warn_if_no_upstream")
        if upstream:
            needed[name] = str(upstream)

    # concept ごとに「書かれた関係の名前」「宣言された欠落」「代表のレコード」。
    # 宣言は**書いたレコードごと**に覚える ―― 指摘は宣言が書いてある行を指す
    # （同じ concept が複数のファイルに散るので、代表のレコードとは限らない）。
    written: dict[str, set[str]] = {}
    declared: dict[str, dict[str, organized_module.Record]] = {}
    speaker: dict[str, organized_module.Record] = {}
    for record in result.records:
        written.setdefault(record.concept, set()).update(
            ref.rel for ref in record.refs)
        for name in record.known_gaps:
            declared.setdefault(record.concept, {}).setdefault(name, record)
        if record.complete and record.concept not in speaker:
            speaker[record.concept] = record

    findings: list[Finding] = []
    for concept, names in sorted(declared.items()):
        for name in sorted(set(names) & written.get(concept, set())):
            record = names[name]
            findings.append(Finding(
                "warn", "G031", record.subject,
                f"known_gaps に {name} を宣言していますが、{name} は書かれています"
                "（宣言を消してください。埋まった欠落の宣言が残ると、"
                "承知しているものと放置しているものが区別できなくなります）",
                file=record.path or None,
                line=record.line_of("known_gaps") or record.line or None))
    if not needed:
        return findings

    for concept, record in sorted(speaker.items()):
        mapped = model.for_fact(record.type) if record.type else None
        if mapped is None:
            continue                      # G002 / G013 が報告済み
        want = needed.get(mapped[0])
        if not want or want in written.get(concept, set()):
            continue
        if want in declared.get(concept, {}):
            # **調べたうえで相手がいないと宣言してある。** 数だけは必ず出す。
            if report is not None:
                report.gaps_used.append((concept, want))
            continue
        label = (model.relation_types.get(want) or {}).get("label")
        findings.append(Finding(
            "warn", "G020", record.subject,
            f"{record.name}: {want}"
            + (f"（{label}）" if label else "")
            + " が 1 本もありません"
            "。相手は本文（縛る先・取り込み）に書いてあることが多いので、"
            "いま開いているうちに refs へ足してください"
            "（探したうえで相手がいないなら、レコードの known_gaps に "
            f"{want} を理由つきで宣言してください ―― build が正本へ引き継ぎ、"
            "check では W032 として理由つきで出続けます）",
            file=record.path or None,
            line=record.line_of("refs") or record.line or None))
    return findings


def _proposal(round_: Round, result: organized_module.Organized,
              model: Metamodel, known: dict[str, Concept]) -> list[Finding]:
    """整理②の提案（``_concepts.yml``）の語彙と相手。**形はスキーマが見る。**

    台帳は次のラウンドの判断材料なので、レコードと同じ語彙を守らせる ――
    台帳だけ別の語彙を持てると、次のラウンドで台帳を引いた整理が語彙外の
    ``type`` をレコードへ写す。``assign`` の相手は build の ``B003`` も見るが、
    それが鳴るのは凍結の後で、整理②はもう終わっている（``G020`` と同じ
    「鳴る時期」の話 ―― 書いている手元で言えば、その場で直せる）。
    """
    location = _relative(round_, round_.organized / f"_concepts{yamlio.EXT}")
    findings: list[Finding] = []
    for entry in result.concepts.get("new") or []:
        if not isinstance(entry, dict):
            continue                      # 形はスキーマが報告済み
        fact = str(entry.get("type") or "")
        if fact and model.for_fact(fact) is None:
            findings.append(Finding(
                "error", "G002", str(entry.get("concept") or ""),
                f"台帳への提案（new）の type が語彙にありません: {fact}"
                "（arp4 model の「整理結果に書く type」から選んでください）",
                file=location))
    for entry in result.concepts.get("assign") or []:
        if not isinstance(entry, dict):
            continue
        key = str(entry.get("concept") or "")
        if key and key not in known:
            findings.append(Finding(
                "error", "G021", key,
                "assign の相手が台帳にありません。assign は既存 concept へ"
                "別名を足す宣言です。新しい概念は new に書いてください",
                file=location))
    return findings


def _member(name: str, body: list[list[str]]) -> list[str] | None:
    """レコードが指している行。**当てられなければ黙る。**

    整理結果の名前は修飾されている（``yamlio.marked``）が、表の左端は資料に
    書いてあるままの短い名前（``marked``）である。区切りが何かは資料の出自で
    違う（Python は ``.``、Java の内部クラスは ``$``）ので、**区切りの種類は
    決め打ちにせず、語の切れ目であることだけを見る** ―― ``scan_tree`` の行を
    ``tree`` のレコードが掴まないために要る。

    2 行に当たったら黙る。どちらかを選ぶのは意味の判断である。
    """
    hit = [row for row in body if row and _same_member(name, row[0])]
    return hit[0] if len(hit) == 1 else None


def _same_member(name: str, cell: str) -> bool:
    if not cell or not name.endswith(cell):
        return False
    if name == cell:
        return True
    edge = name[-len(cell) - 1]
    return not (edge.isalnum() or edge == "_")


def _words(text: str) -> list[str]:
    """語の候補。**長いものを優先**する（短い語は偶然一致しやすい）。

    1 字の語は漢字しか無い（片仮名・英字は 2 字から拾う）。**8 語の枠の外**で
    全部残す ―― 和文の言い換えと本文を繋ぐ線はたいてい語幹の漢字 1 字で、
    枠の中に入れると長い語に必ず押し出される。
    """
    found = set(_WORD.findall(text))
    longest = sorted((w for w in found if len(w) >= _MIN_WORD),
                     key=len, reverse=True)[:8]
    stems = sorted(w for w in found if len(w) < _MIN_WORD)
    return longest + stems


def _grounded(words: list[str], haystack: str, name: str = "") -> bool:
    """語が本文から辿れるか。

    2 字以上の語は 1 つ一致すれば足りる。**漢字 1 字の語幹は 2 つ揃って初めて
    一致とみなす** ―― パース結果の見出しは arp4 自身が書く（``取り込み`` ``元``
    ``行``）ので、1 字の偶然の一致を許すと「込」1 字がどのレコードでも見出しに
    当たり、コードの塊では検査がほぼ黙る。独立な語幹が 2 つ重なる偶然は稀で、
    「揃えるか／揃え方」「省くか／省き方」のような活用違いの言い換えだけが通る。

    **``name`` がそのまま本文にあるなら、それだけで辿れている。** 語に切ると
    ひらがなが落ちる（``戻る`` → ``戻`` の 1 字）ので、**2 文字の出典を写した
    レコードは語幹 1 つしか持てず、2 つ揃えようが無い** ―― 画面レイアウトの
    ボタンのように出典が 1 セルしかない項目では、``name`` にも ``statement`` にも
    何を書いても鳴った（実測・sales-corpus）。同じシートの ``与信残`` は漢字 3 字
    なので鳴らない。**同性質の 2 件が語の長さだけで割れる**のは検査の側の欠陥で、
    原文の写しを「作文の疑い」と呼ぶのは筋が悪い。
    """
    stripped = re.sub(r"\s+", "", name)
    if stripped and stripped in re.sub(r"\s+", "", haystack):
        return True
    if any(w in haystack for w in words if len(w) >= _MIN_WORD):
        return True
    stems = [w for w in words if len(w) < _MIN_WORD]
    return sum(1 for w in stems if w in haystack) >= 2


# ── 凍結 ────────────────────────────────────────────────────────
def digest(text: str) -> str:
    return hashlib.sha256(text.encode("utf-8")).hexdigest()[:12]


def hashes(round_: Round) -> dict[str, str]:
    """``organized/`` の内容ハッシュ。**マニフェスト自身は含めない。**"""
    directory = round_.organized
    return {path.relative_to(directory).as_posix():
            digest(path.read_text(encoding="utf-8"))
            for path in yamlio.scan_tree(directory) if path.name != round_.frozen.name}


def apply(round_: Round, report: Report, today: str | None = None) -> dict[str, Any]:
    """ハッシュを固定する。**ゲートを通っている前提。**"""
    manifest = {
        "frozen_at": today or _datetime.date.today().isoformat(),
        "files": hashes(round_),
        "records": report.metrics.get("records", 0),
        "out_of_scope": report.metrics.get("out_of_scope", 0),
        "unreadable": report.metrics.get("unreadable", 0),
    }
    previous = yamlio.load(round_.frozen) if round_.frozen.is_file() else None
    if isinstance(previous, dict) and previous.get("amendments"):
        manifest["amendments"] = previous["amendments"]
    yamlio.dump(round_.frozen, manifest)
    return manifest


def verify(round_: Round) -> list[Finding]:
    """凍結後に整理結果が編集されていないか。

    **凍結しているからこそ、例外的な手当てが例外として見える。** 利用者の指示で
    直したのなら、``.frozen.yml`` の ``amendments`` に理由を残して固定し直す。
    """
    if not round_.frozen.is_file():
        return []
    manifest = yamlio.load(round_.frozen) or {}
    recorded = {str(k): str(v) for k, v in (manifest.get("files") or {}).items()}
    current = hashes(round_)

    amended = {str(a.get("file")) for a in (manifest.get("amendments") or [])
               if isinstance(a, dict)}
    findings: list[Finding] = []
    for name in sorted(set(recorded) | set(current)):
        if recorded.get(name) == current.get(name):
            continue
        if name in amended:
            continue
        # 消えたファイルにも位置は載せる ―― **無いことを確かめに行く先**である。
        where = _relative(round_, round_.organized / name)
        if name not in current:
            findings.append(Finding("error", "G009", round_.name,
                                    "凍結後に整理結果が消えています", file=where))
        elif name not in recorded:
            findings.append(Finding("error", "G009", round_.name,
                                    "凍結後に整理結果が増えています"
                                    "（新しいラウンドを起こしてください）", file=where))
        else:
            findings.append(Finding(
                "error", "G009", round_.name,
                "凍結後に整理結果が編集されています"
                "（正本側で直すか、.frozen.yml の amendments に理由を残してください）",
                file=where))
    return findings
