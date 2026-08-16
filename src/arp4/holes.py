"""``0_この設計書の穴.md`` ―― **読み手が最初に穴を知る 1 枚。**

穴の情報は既に全部ある。ただし **4 か所に散っていて、互いに食い違っていた。**

============================  ==============================================
未解決の矛盾                  課題管理表（``disputes``）
設計要素に繋がらない要件      トレーサビリティ・マトリクス §3
資料に定義が無いと宣言        各設計書の脚注（``known_gaps``）
機械が読めなかった            整理層の ``out_of_scope: kind: 未読取``
``--force`` で通したこと      **どこにも無い**
============================  ==============================================

実測（sales-corpus 30 冊・r001）で何が起きたか::

    トレーサビリティ §2   業務要件 44 件のうち 43 件が「―」（設計要素なし）
    トレーサビリティ §3   その 43 件が**1 件も載っていない**（NFR しか出ない）
    整理層               `未読取` を 1 件宣言している
    生成物               「未読取」の文字列が **0 件**

同じ事実から逆の判断が 2 つ出て、しかも読み手には両方とも見えない。散らばって
いること自体が原因なので、**集めた 1 枚を目次の先頭に置く**。

**ここは新しい判断をしない。** 既に正本と整理結果にあるものを 1 か所へ集めるだけ
である ―― 集約の場所が判断を始めると、課題管理表との食い違いが 5 か所目になる。
"""

from __future__ import annotations

import html
from pathlib import Path
from typing import Any, Iterable

from arp4 import gate as gate_module
from arp4 import organized as organized_module
from arp4 import page as page_module
from arp4.finding import Finding
from arp4.spec import Spec

#: 生成するファイル名。:mod:`arp4.gate` の帯がここへリンクする。
STEM = "0_この設計書の穴"

#: 一覧に出す上限。**超えた分は件数で言う**（全部並べると本文より長くなる）。
_LIMIT = 40


def write(spec: Spec, out_dir: Path, findings: Iterable[Finding],
          gate: gate_module.Gate | None = None,
          folded: Iterable[tuple[str, str, str, str]] = (),
          misdirected: Iterable[tuple[str, str, str, str, str]] = ()) -> list[Path]:
    """``out/0_この設計書の穴.{md,html}`` を書く。

    ``folded`` は畳んだ列 ``(設計書, 章, 列, 定義のパス)``（→ :func:`_folded`）。
    ``misdirected`` は**畳まなかった**列 ``(設計書, 章, 列, 定義のパス, 値がある欄)``
    （→ :func:`_misdirected`）。**2 つを 1 つの表に混ぜない** ―― 次の一手が
    正反対である。
    """
    sections = _sections(spec, list(findings), gate, list(folded),
                         list(misdirected))

    lines = [f"# {STEM[2:]}", "",
             "> この文書は生成物です。**直接編集しないでください**"
             "（`arp4 publish` で再生成されます）。", "",
             "この設計書の束が何を言えていないかを 1 枚に集めたものです。"
             "中身は課題管理表・トレーサビリティ・各設計書の脚注・整理層の宣言に"
             "あるものと同じで、ここが新しく判断したことはありません。", ""]
    for heading, intro, rows, columns in sections:
        lines += [f"## {heading}", ""]
        if intro:
            lines += [intro, ""]
        if not rows:
            lines += ["（該当なし）", ""]
            continue
        lines.append("| " + " | ".join(columns) + " |")
        lines.append("|" + "|".join(["---"] * len(columns)) + "|")
        for row in rows[:_LIMIT]:
            lines.append("| " + " | ".join(_cell(c) for c in row) + " |")
        if len(rows) > _LIMIT:
            lines.append(f"| ほか {len(rows) - _LIMIT} 件 |"
                         + " |" * (len(columns) - 1))
        lines.append("")

    md = out_dir / f"{STEM}.md"
    md.write_text("\n".join(lines).rstrip() + "\n", encoding="utf-8", newline="\n")

    # 体裁は :mod:`arp4.page` から取る。ここが空の ``<link rel="stylesheet">`` を
    # 出していたあいだ、**束の中でいちばん先に読ませたい 1 枚だけ**が罫線も色も
    # 無い素の HTML で出ていた（他の 12 冊には体裁があった）。
    escape = html.escape
    parts = page_module.head(STEM[2:])
    parts.append(page_module.toolbar("アイテム名・コードで絞り込み"))
    parts += ['<div class="wrap">',
              f'<p class="meta"><a href="目次.html"{page_module.NEW_TAB}>'
              "← 生成した設計書（目次）</a></p>",
              f"<h1>{escape(STEM[2:])}</h1>",
              '<p class="meta">この設計書の束が何を言えていないかを 1 枚に'
              "集めたものです。ここが新しく判断したことはありません。</p>"]
    tabs: list[tuple[str, str]] = []
    for index, (heading, intro, rows, columns) in enumerate(sections, start=1):
        parts.append(f'<h2 id="h{index}">{escape(heading)}</h2>')
        tabs.append((f"h{index}", heading))
        if intro:
            parts.append(f'<p class="meta">{escape(_plain(intro))}</p>')
        if not rows:
            parts.append('<p class="empty">（該当なし）</p>')
            continue
        parts.append(page_module.grid(
            columns, [[_plain(str(c)) for c in row] for row in rows[:_LIMIT]]))
        if len(rows) > _LIMIT:
            parts.append(f'<p class="meta">ほか {len(rows) - _LIMIT} 件</p>')
    parts.append("</div>")
    parts += page_module.tail(page_module.tabs(tabs))
    page = out_dir / f"{STEM}.html"
    page.write_text("\n".join(parts) + "\n", encoding="utf-8", newline="\n")
    return [md, page]


def _sections(spec: Spec, findings: list[Finding],
              gate: gate_module.Gate | None,
              folded: list[tuple[str, str, str, str]] | None = None,
              misdirected: list[tuple[str, str, str, str, str]] | None = None
              ) -> list[tuple[str, str, list[list[str]], list[str]]]:
    return [_gate_section(gate),
            _disputes(spec),
            _conflicts(spec),
            _unread(spec),
            _declared_gaps(spec),
            _folded(folded or []),
            _misdirected([r for r in (misdirected or [])
                          if not _escaped_to(r[4])]),
            _escaped_section([r for r in (misdirected or [])
                              if _escaped_to(r[4])]),
            _unlinked(spec, findings)]


def _escaped_to(alt: str) -> bool:
    """指し先が ``description`` か（＝中身を照合していない答えか）。

    判定は :func:`arp4.publish._escaped_to` と同じだが、**インポートしない** ――
    ここが受け取るのは既に文字列へ整形された欄の名前（``displays.description``）で、
    向こうが見るのは正本の生のパスである。同じ 1 行を共有すると、片方の整形を
    変えたときにもう片方が黙って全部「同名の別の欄」側へ倒れる。
    """
    return alt.rsplit(".", 1)[-1] == "description"


def _folded(folded: list[tuple[str, str, str, str]]
            ) -> tuple[str, str, list[list[str]], list[str]]:
    """様式にはあるのに、正本が値を 1 つも持たない列。

    **畳んだことは各設計書の脚注に出ているが、束としては数えられていなかった。**
    脚注は章の末尾に 1 行出るだけで、実測（r001）の画面帳票項目定義書は
    16 の節を 250 行ぶん並べたあとに「全行が空だったので省略した列: 備考」と
    書いた ―― その `備考` が `displays.note` で、整理の手順書が
    「初期値・物理名はそこへ写す」と名指ししている欄だとは辿れない。
    実測でその欄は `displays` 164 本すべてで空だった。

    **「資料に無い」とは言っていない。** 様式が持つ欄に正本が値を持たない、
    としか言えない ―― 資料にその欄が無いのか、整理層が写さなかったのかは、
    ここからは区別が付かない（区別が付くのは資料を見た人だけである）。
    見るべき先を名前で出すのが、ここでできることである。

    **値が別の欄にあると分かっているものは、ここに来ない。** そちらは
    :func:`_misdirected` が別の表で言う ―― 混ぜていたころ、`displays.note` の
    ように「資料にはあるが写す先を間違えた」ものが「資料に無いのかもしれない」
    という文言の下に並び、**読み手は資料を見に行かなかった。**
    """
    rows = [[document, heading, column, f"`{path}`" if path else "―"]
            for document, heading, column, path in sorted(folded)]
    return ("様式にあるのに、正本が値を持たない列",
            "「資料に無い」と決めたわけではありません。様式が持つ欄に正本が"
            "値を 1 つも持たない、というだけです。資料にその欄が無いのか、"
            "整理のときに写されなかったのかは、資料を見た人にしか分かりません。"
            "見に行く先（定義のパス）を出しています（`arp4 check` の W046）。",
            rows, ["設計書", "章", "列", "定義のパス"])


def _misdirected(misdirected: list[tuple[str, str, str, str, str]]
                 ) -> tuple[str, str, list[list[str]], list[str]]:
    """列は空だが、**同じ名前の欄に値が入っている**もの。

    上の表とは**次の一手が正反対**である ―― あちらは資料を見に行く話で、
    こちらは正本の中で置き場所が違うという話である。同じ名前が両側にあり、
    片方だけが埋まっているので、**値の置き場所は 1 つに決まる。**

    **どちらが正かは決めていない。** 様式が指す先を直すのか、整理層が書く先を
    直すのかは、資料を見た人が決める ―― ここは両方の欄を名前で出すだけである。

    **``description`` を指しているものはここに来ない**（→
    :func:`_escaped_section`）。あちらは中身を照合していないので、この表の
    「同じ事実が別の欄に入っています」を言えない。
    """
    rows = [[document, heading, column, f"`{path}`" if path else "―", f"`{alt}`"]
            for document, heading, column, path, alt in sorted(misdirected)]
    return ("列は空だが、同じ名前の別の欄に値がある",
            "「資料に無い」ではありません。同じ名前の欄が両側にあり、"
            "片方だけが埋まっています。様式が指す先を直すか、整理のときの"
            "書き先を直すかは、資料を見た人が決めることです"
            "（`arp4 check` の W043）。",
            rows, ["設計書", "章", "列", "定義のパス", "値がある欄"])


def _escaped_section(escaped: list[tuple[str, str, str, str, str]]
                     ) -> tuple[str, str, list[list[str]], list[str]]:
    """列は空で、その母集合が ``description`` を使っているもの。

    **上の表と混ぜない。** 上は「値はそこにある」と言えているが、こちらが
    言えているのは 2 つだけである ―― この列は空であること、そして母集合の
    **どれか 1 件**が ``description`` を持っていること。空の列の名前と
    ``description`` の中身を突き合わせる処理は無い（→
    :func:`arp4.publish._escaped_to`）。

    したがって読み方は :func:`_folded`（``W046``）と同じ側である ――
    **資料にその列が無いのか、整理層が写さなかったのかは、まだ割れていない。**

    実測（kotonoha r001）で 9 件すべてがこの表の側だった。上の表に混ざって
    いたころ、読み手は「同じ事実が正本の別の欄に入っています」を信じて
    「どこにあるか」を探し、**別のシートの意味の違う列を持ってきた。**
    出発点が「あるはずだ」だと、「無い」という結論には辿り着けない。
    """
    rows = [[document, heading, column, f"`{path}`" if path else "―", f"`{alt}`"]
            for document, heading, column, path, alt in sorted(escaped)]
    return ("列は空で、この母集合が description を使っている",
            "**`description` の中身がこの列の値かどうかは見ていません。**"
            "言えているのは「この列は空」と「この母集合のどれかが "
            "`description` を持っている」の 2 つだけです。`description` を"
            "開いて確かめ、別の値なら出典の資料にその列があるかを 1 件ずつ"
            "当たってください ―― 資料に無ければ空のままで正しい状態です"
            "（`arp4 check` の W047）。",
            rows, ["設計書", "章", "列", "定義のパス", "description を持つ欄"])


def _conflicts(spec: Spec) -> tuple[str, str, list[list[str]], list[str]]:
    """出典どうしで食い違い、``build`` が採らなかった値。

    実測で、``5.権限マトリクス`` の「与信保留の解除は △（部長職のみ可）」が
    ``4.セキュリティ方式`` の「営業部 120 名。…」に上書きされ、``△`` は正本からも
    生成物からも消えた ―― ``B022`` は 6 件鳴っていたが build の出力は端末に流れて
    消える。相補的な補足は足し合わせるようにしたので、ここに残るのは**足し合わせ
    られないもの**（スカラ属性と ``statement``）だけである。
    """
    rows: list[list[str]] = []
    for item in spec.items:
        entries = item.get("conflicts")
        if not isinstance(entries, dict):
            continue
        for name, dropped in sorted(entries.items()):
            for entry in dropped if isinstance(dropped, list) else []:
                if not isinstance(entry, dict):
                    continue
                where = entry.get("source") or {}
                rows.append([_ref(item), str(item.get("name") or ""), str(name),
                             str(item.get(name) or ""), str(entry.get("value") or ""),
                             f"{where.get('file', '')}#{where.get('anchor', '')}"])
    rows.sort()
    return ("出典どうしで食い違っているもの",
            "機械はどちらが正かを決めていません。採った値を出していますが、"
            "採らなかった値も資料に書いてあるものです。",
            rows, ["アイテム", "名称", "属性", "採った値", "採らなかった値",
                   "採らなかった値の出典"])


def _gate_section(gate: gate_module.Gate | None
                  ) -> tuple[str, str, list[list[str]], list[str]]:
    """``publish`` が通った条件。**通ったことは穴が無いことを意味しない。**"""
    if gate is None or gate.clean:
        return ("生成したときの状態", "未解決の指摘はありませんでした。", [], [])
    intro = ("未解決の指摘を残したまま `--force` で生成しています。"
             if gate.forced else
             "error はありませんが、未解決の warn が残っています。")
    rows = [[code, str(count), _CODES.get(code, "")]
            for code, count in sorted(gate.counts.items())]
    return ("生成したときの状態", intro + " 全文は `arp4 check` で出ます。",
            rows, ["コード", "件数", "意味"])


#: 検出コードの一言説明。**`arp4 check` を読みに行かせる**ためのもので、
#: ここで全部を説明しようとしない（説明が 2 か所に増えると片方が古くなる）。
#:
#: **その古くなるほうを、この表自身が 2 度やった。** ``W045`` を足したときに
#: ここへ書き足すのを忘れ、穴の一覧は「意味」列が ``―`` の行を出した。決定 78 で
#: ``P0xx`` を ``P1xx`` へ改番したときも :mod:`arp4.audit` だけ直り、ここは
#: 古い綴りのまま残った ―― **どちらも「片方が古くなる」と書いた注釈の真下で
#: 起きている。** 注釈では防げないので、:func:`tests.test_holes` が
#: 「ゲートへ流れるコードは全部ここにある」を見る。

_CODES = {
    "E010": "必須属性が無い",
    "W010": "メタモデルに無い属性が書かれている",
    "W012": "上書きを宣言したのに値が無い",
    "W020": "同じ関係が重複している",
    "W021": "順序のある関係に order が無い（または重複）",
    "W030": "どの設計要素からも参照されていない（トレース欠落）",
    "W031": "自分から出る関係が 1 本も無い（トレース欠落）",
    "W032": "欠落を known_gaps で承知している",
    "W033": "known_gaps の宣言が古い",
    "W044": "仕様文がほぼ同一（二重登録の疑い）",
    "W045": "出典どうしで値が食い違い、採らなかった値がある",
    "W034": "資料は届いているのに設計書が空になる",
    "W043": "列が全行空だが、同じ名前の別の欄に値がある（列の書き間違い）",
    "W046": "様式が持つ列を全行空で省略した",
    "W047": "列が全行空で、母集合が description を使っている（中身は未照合）",
    "P101": "母集合をそのまま並べた表",
    "P102": "升目に凡例が無い",
    "P103": "節の見出しが enum の生値",
    "P104": "同じ本文の繰り返し",
    "P105": "争点のあるアイテムに印が無い",
    "P106": "同じアイテムが複数の設計書に全文で重複",
    "P107": "出典列の有無が揃っていない",
    "P108": "「未分類」の節が大きすぎる",
    "P109": "禁止を書ける升目に禁止が 1 件も無い",
    "P110": "正本にあるのに、どの設計書にも出ていない関係",
    "P111": "正本に値があるのに、どの設計書の列にも出ない属性",
}


def _disputes(spec: Spec) -> tuple[str, str, list[list[str]], list[str]]:
    """未決の矛盾。**設計書の本文には並列に載っている。**

    実測で、基本設計書には互いに矛盾する 4 組（引当のタイミング・消費税の計算
    単位・請求の締め日・受注取消の期限）が印も無く並んだ。課題管理表を開かない
    読み手は、両方を確定仕様として受け取る。
    """
    by_id = spec.by_id
    rows: list[list[str]] = []
    for relation in spec.relations_of("disputes"):
        if relation.get("status") == "deprecated":
            continue
        issue = by_id.get(str(relation.get("from"))) or {}
        target = by_id.get(str(relation.get("to"))) or {}
        rows.append([_ref(issue), str(issue.get("name") or ""),
                     _ref(target), str(target.get("name") or ""),
                     str(target.get("statement") or "")])
    rows.sort()
    return ("決着していない矛盾",
            "同じことについて資料が違うことを言っており、どちらへも寄せて"
            "いません。設計書の本文には両方が並んで載っています。",
            rows, ["課題", "課題の名称", "対象", "対象の名称", "対象の仕様"])


def _unread(spec: Spec) -> tuple[str, str, list[list[str]], list[str]]:
    """機械が読めなかったもの。**「資料に無い」と混ぜない。**

    実測で、整理層は業務フローの図形の凡例を ``kind: 未読取`` で 1 件宣言して
    いた（形状が取れないので ``step_kind`` を決められない）。結果、要件定義書の
    「ステップ種別」は全行 `―` で出たが、**生成物に「未読取」の文字は 0 件**
    ―― 読めなかったことが「資料に書いていない」と見分けられない。
    """
    rows: list[list[str]] = []
    if spec.paths is not None:
        for round_ in spec.paths.rounds():
            data, _ = organized_module.load(round_)
            for entry in data.out_of_scope:
                if entry.unreadable:
                    rows.append([round_.name, entry.file, entry.anchor,
                                 entry.reason])
    rows.sort()
    return ("機械が読めなかったもの",
            "空欄に見えても「資料に無い」ではありません。次のラウンドで"
            "拾い直す対象です（`arp4 render` で絵にして読みます）。",
            rows, ["ラウンド", "資料", "アンカー", "読めなかった理由"])


def _declared_gaps(spec: Spec) -> tuple[str, str, list[list[str]], list[str]]:
    """資料に定義が無いと宣言したもの（``known_gaps``）。"""
    rows: list[list[str]] = []
    for item in spec.items:
        for name, entry in sorted((item.get("known_gaps") or {}).items()):
            if not isinstance(entry, dict):
                continue
            rows.append([_ref(item), str(item.get("name") or ""), str(name),
                         str(entry.get("reason") or ""), str(entry.get("at") or "")])
    rows.sort()
    return ("資料に定義が無いと確かめたもの",
            "確かめたうえで無いものです（まだ見ていないものではありません）。"
            "資料が届いたら宣言を消します。",
            rows, ["アイテム", "名称", "欠けているもの", "理由", "宣言日"])


def _unlinked(spec: Spec,
              findings: list[Finding]) -> tuple[str, str, list[list[str]], list[str]]:
    """トレースが繋がっていないもの（``W030`` / ``W031``）。

    トレーサビリティ・マトリクス §3 は**非機能要件しか載せていなかった** ――
    §2 で「―」だった業務要件 43 件が「設計漏れ」に上がってこない。判定の基準が
    章によって違い、しかもどちらが正か書かれていない。ここは検出そのものを
    並べるので、章ごとの取りこぼしが起きない。
    """
    rows = [[f.code, f.target, f.message]
            for f in findings if f.code in ("W030", "W031")]
    rows.sort()
    return ("設計要素に繋がっていないもの",
            "資料が足りないだけのこともあります（error ではありません）。"
            "承認の前に必ず見てください。",
            rows, ["コード", "アイテム", "内容"])


def _ref(item: dict[str, Any]) -> str:
    """穴の一覧に出す**引ける** ID。無い種別は空にする（内部 ID を出さない）。

    表示 ID を持たない種別はある ―― `data-item` は採番の宣言（`sequence`）を
    持たず、画面帳票項目定義書もテーブル定義書も項目の表は `No` 列で出る。
    ここで `item["id"]` へ落ちると、**どの設計書からも引けないハッシュ**が
    `FR-002` / `CST-044` と同じ列に並ぶ ―― 実測（sales-corpus・r001）で
    `itm-b10f410bedc4` のような値が 23 種類出ていた。

    **隣の「名称」列が対象を名指ししている**ので、空にしても辿る道は消えない。
    逆に内部 ID を出すと、読み手は引ける ID だと思って探しに行く。
    """
    for key, value in item.items():
        if str(key).endswith("_id") and value:
            return str(value)
    return ""


def _cell(value: Any) -> str:
    text = str(value or "―").replace("|", "\\|")
    return text.replace("\n", "<br>")


def _plain(value: str) -> str:
    return value.replace("**", "").replace("`", "")
