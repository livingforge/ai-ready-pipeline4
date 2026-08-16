"""``0_この設計書の穴.md`` ―― **読み手が最初に穴を知る 1 枚。**

実測（sales-corpus 30 冊・r001）で、穴の情報は 4 か所に散り、互いに食い違って
いた ―― トレーサビリティ §2 で「―」だった業務要件 43 件は §3「未実現の要件
（設計漏れ）」に 1 件も載らず、整理層が宣言した ``未読取`` 1 件は生成物に
**0 文字も出ていなかった。**
"""

from __future__ import annotations

from pathlib import Path
from typing import Any

from arp4 import gate as gate_module
from arp4 import holes, metamodel as mm
from arp4.finding import Finding
from arp4.spec import Spec


def _spec(model: mm.Metamodel) -> Spec:
    """争点 1 組と、欠落を宣言したアイテム 1 件。"""
    return Spec(metamodel=model, items=[
        {"id": "rul-1", "type": "business-rule", "status": "review",
         "rule_id": "RUL-136", "name": "在庫引当を実行する時点",
         "statement": "在庫の引当は出荷指示の実行時に行うこと"},
        {"id": "iss-1", "type": "open-issue", "status": "review",
         "issue_id": "ISS-017", "name": "在庫引当を行う時点",
         "statement": "受注確定時か出荷指示時かを決めること"},
        {"id": "ent-1", "type": "entity", "status": "review",
         "physical_name": "M_PRICE", "name": "得意先別単価マスタ",
         "statement": "得意先別単価マスタは契約単価を保持すること",
         "known_gaps": {"has-column": {
             "reason": "テーブル定義書に列定義シートが無い", "at": "2026-08-11"}}},
    ], relations=[{"type": "disputes", "from": "iss-1", "to": "rul-1",
                   "status": "review"}])


def _read(tmp_path: Path) -> str:
    return (tmp_path / f"{holes.STEM}.md").read_text(encoding="utf-8")


def test_決着していない矛盾が1枚に集まる(model: mm.Metamodel,
                                          tmp_path: Path) -> None:
    """**設計書の本文には両方が並列に載っている。**

    課題管理表を開かない読み手は、矛盾する 2 つを確定仕様として受け取る。
    """
    holes.write(_spec(model), tmp_path, [])

    text = _read(tmp_path)
    assert "ISS-017" in text and "RUL-136" in text
    assert "どちらへも寄せていません" in text


def test_トレース欠落は検出そのものを並べる(model: mm.Metamodel,
                                            tmp_path: Path) -> None:
    """トレーサビリティ §3 は非機能要件しか載せず、§2 で「―」だった業務要件
    43 件が「設計漏れ」に上がってこなかった ―― 章ごとに判定の基準が違い、
    しかもどちらが正か書かれていない。ここは検出を並べるので取りこぼさない。"""
    findings = [Finding("warn", "W030", "req-1（受注の取消）",
                        "どの設計要素からも参照されていません"),
                Finding("warn", "W031", "cst-1（文字コード）",
                        "constrains が 1 本もありません"),
                Finding("warn", "W044", "act-1（営業）", "仕様文がほぼ同一です")]
    holes.write(_spec(model), tmp_path, findings)

    text = _read(tmp_path)
    assert "req-1（受注の取消）" in text and "cst-1（文字コード）" in text
    assert "act-1（営業）" not in text          # W044 はこの章の話ではない


def test_資料に定義が無いと確かめたものが出る(model: mm.Metamodel,
                                              tmp_path: Path) -> None:
    """**確かめたうえで無い**ものと、まだ見ていないものを混ぜない。"""
    holes.write(_spec(model), tmp_path, [])

    text = _read(tmp_path)
    assert "M_PRICE" in text or "得意先別単価マスタ" in text
    assert "列定義シートが無い" in text


def test_forceで通したことが最初の章に出る(model: mm.Metamodel,
                                            tmp_path: Path) -> None:
    gate = gate_module.summarize(
        [Finding("error", "E010", "itm-1", "必須属性がありません: data_type")],
        forced=True, today="2026-08-11")
    holes.write(_spec(model), tmp_path, [], gate)

    text = _read(tmp_path)
    assert "--force" in text and "E010" in text
    assert "必須属性が無い" in text              # コードの意味を添える


def test_指摘が無ければ穴も無いと言う(model: mm.Metamodel, tmp_path: Path) -> None:
    """**通ったことは「穴が無い」ことを意味しない**が、本当に無いなら言い切る。"""
    holes.write(_spec(model), tmp_path, [],
                gate_module.summarize([], forced=False))

    assert "未解決の指摘はありませんでした" in _read(tmp_path)


def test_htmlも出る(model: mm.Metamodel, tmp_path: Path) -> None:
    written = holes.write(_spec(model), tmp_path, [])

    assert [p.suffix for p in written] == [".md", ".html"]
    assert "ISS-017" in written[1].read_text(encoding="utf-8")


def test_新しい判断はしないと明記する(model: mm.Metamodel, tmp_path: Path) -> None:
    """**集約の場所が判断を始めると、課題管理表との食い違いが 5 か所目になる。**"""
    holes.write(_spec(model), tmp_path, [])

    assert "新しく判断したことはありません" in _read(tmp_path)


def test_長い一覧は件数で言う(model: mm.Metamodel, tmp_path: Path) -> None:
    """全部並べると本文より長くなる ―― 読む先は `arp4 check`。"""
    findings = [Finding("warn", "W030", f"req-{i}", "参照されていません")
                for i in range(60)]
    holes.write(_spec(model), tmp_path, findings)

    text = _read(tmp_path)
    assert "ほか 20 件" in text


def _emitted() -> set[str]:
    """ゲートへ流れる指摘のコード。

    出すのは publish / check が回すこの 3 つである（:mod:`arp4.cli`）。
    ``parse`` の ``P0xx`` はラウンドの側で完結するので母集合に入れない ――
    綴りは似ているが別の体系である（→ 決定 78）。
    """
    import re
    from pathlib import Path as _Path

    root = _Path(holes.__file__).parent
    codes: set[str] = set()
    for name in ("validate.py", "audit.py", "publish.py"):
        codes |= set(re.findall(r'"((?:E|W|P|I)\d{3})"',
                                (root / name).read_text(encoding="utf-8")))
    return codes


def test_ゲートに残る警告は全部意味を持つ() -> None:
    """**「意味」列に `―` を出さない。**

    穴の帯はコード別の件数を出し、:data:`holes._CODES` から一言説明を引く。
    引けないコードは `―` になり、読み手は件数だけ渡されて意味を渡されない。

    見るのは ``warn``（``W`` / ``P``）だけである ―― ``error`` が帯に出るのは
    ``--force`` を押したときで、そちらは帯そのものが警告として立つ。

    実測で 2 度起きた ―― ``W045`` を足したときに書き足し忘れ、決定 78 で
    ``P0xx`` を ``P1xx`` へ改番したときは :mod:`arp4.audit` だけが直った。
    どちらも「説明が 2 か所に増えると片方が古くなる」と書いた注釈の真下である。
    **注釈では防げないので機械が見る。**
    """
    missing = sorted(c for c in _emitted()
                     if c[0] in "WP" and c not in holes._CODES)

    assert not missing, (
        f"holes._CODES に説明がありません: {'・'.join(missing)} ―― "
        "穴の一覧で「意味」列が `―` になります")


def test_誰も出さないコードを抱え込まない() -> None:
    """**反対側も見る。** 改番で消えたコードが残ると、綴りだけ古い説明が居座る。"""
    stale = sorted(c for c in holes._CODES if c not in _emitted())

    assert not stale, (
        f"どこも出さないコードの説明が残っています: {'・'.join(stale)}")


def test_様式にあるのに値を持たない列が1枚に集まる(model: mm.Metamodel,
                                                  tmp_path: Path) -> None:
    """**畳んだことは脚注に出ていたが、束としては数えられていなかった。**

    実測（r001）の画面帳票項目定義書は、16 の節を 250 行ぶん並べたあとに
    「全行が空だったので省略した列: 備考」と 1 行書いた ―― その `備考` が
    `displays.note`（整理の手順書が「初期値・物理名はそこへ写す」と名指しして
    いる欄）だとは、この 1 行からは辿れない。実測でその欄は `displays`
    164 本すべてで空だった。
    """
    holes.write(_spec(model), tmp_path, [], None,
                [("画面帳票項目定義書", "画面帳票項目", "備考", "displays.note")])
    text = _read(tmp_path)

    assert "様式にあるのに、正本が値を持たない列" in text
    assert "`displays.note`" in text
    # **「資料に無い」と決めない** ―― 区別が付くのは資料を見た人だけである。
    assert "資料にその欄が無いのか" in text


def test_畳んだ列が無ければ該当なしと書く(model: mm.Metamodel, tmp_path: Path) -> None:
    """章そのものは残す ―― 見出しが消えると「見ていない」と区別が付かない。"""
    holes.write(_spec(model), tmp_path, [])

    assert "様式にあるのに、正本が値を持たない列" in _read(tmp_path)


def test_値が別の欄にある列は別の表に出す(model: mm.Metamodel,
                                          tmp_path: Path) -> None:
    """**次の一手が正反対のものを、同じ文言の下に並べない。**

    上の表は「資料を見に行け」と言い、こちらは「正本の中で置き場所が違う」と
    言う。実測（r001）の `displays.note` は資料に無かったのではなく、初期値も
    物理名も `displays.description` に入っていた（154 本すべて）―― それが
    「資料にその欄が無いのかもしれません」の下に並んでいたので、**誰も資料を
    見に行かなかったし、正本も直らなかった。**
    """
    holes.write(_spec(model), tmp_path, [], None, [],
                [("列定義", "列定義", "物理名",
                  "has-column.physical_name", "from.physical_name")])
    text = _read(tmp_path)

    assert "列は空だが、同じ名前の別の欄に値がある" in text
    assert "`has-column.physical_name`" in text and "`from.physical_name`" in text
    # **「資料に無い」の側へ混ぜない。**
    body = text.split("列は空だが、同じ名前の別の欄に値がある")[0]
    assert "has-column.physical_name" not in body


def test_descriptionを指しているものは強い側の表に出さない(
        model: mm.Metamodel, tmp_path: Path) -> None:
    """**「値はそこにある」と「中身を見ていない」を同じ表に並べない。**

    指し先が `description` のとき、機械が言えているのは「この列は空」と
    「母集合のどれかが `description` を持っている」の 2 つだけである ――
    空の列の名前と `description` の中身は突き合わせていない。

    実測（kotonoha r001）で 9 件すべてがこちら側だったのに、上の表の
    「同じ事実が正本の別の欄に入っています」の下に並んでいた。読み手は
    「どこにあるか」を探し、**別のシートの意味の違う列を持ってきた** ――
    出発点が「あるはずだ」だと「無い」という結論には辿り着けない。
    """
    holes.write(_spec(model), tmp_path, [], None, [],
                [("画面帳票項目定義書", "画面帳票項目", "備考",
                  "displays.note", "displays.description")])
    text = _read(tmp_path)

    assert "列は空で、この母集合が description を使っている" in text
    assert "`displays.note`" in text and "`displays.description`" in text
    # 強い側（同名の別の欄）にも、資料に無い側（W046）にも混ぜない。
    body = text.split("列は空で、この母集合が description を使っている")[0]
    assert "displays.note" not in body
    assert "中身がこの列の値かどうかは見ていません" in text
