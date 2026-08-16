"""凍結ゲート ―― **通れば build は原理的に失敗しない**、を守れているか。"""

from __future__ import annotations

from arp4 import freeze
from arp4.concepts import Concept
from arp4.metamodel import Metamodel
from arp4.paths import Paths, Round
from conftest import codes, organized, parsed, sources_dir, write

_PARSED = """\
# a.xlsx / 受注テーブル

<!-- source: 資料/a.xlsx / シート: 受注テーブル -->

## 表 B5:H8  <!-- a:s1-t1 at=B5:H8 -->

| 論理名 | 物理名 |
|---|---|
| 受注番号 | ORDER_NO |

## セル B2  <!-- a:s1-x1 at=B2 -->

- `B2` 受注テーブル定義書
"""

_ORGANIZED = """\
records:
  - concept: c-受注番号
    type: データ項目
    name: 受注番号
    statement: 受注番号は文字列型の項目であること
    attrs: { data_type: 文字列 }
    source: { anchor: s1-t1 }
out_of_scope:
  - { anchor: s1-x1, reason: 表題 }
"""


def _setup(round_: Round, organized_body: str = _ORGANIZED) -> None:
    parsed(round_, "資料/a.xlsx/受注テーブル.md", _PARSED)
    organized(round_, "資料/a.xlsx/受注テーブル.yml", organized_body)


def test_全部整理されていれば通る(round_: Round, model: Metamodel) -> None:
    _setup(round_)
    report = freeze.gate(round_, model, {})

    assert not report.blocked
    assert report.metrics == {"parsed_files": 1, "anchors": 2, "records": 1,
                              "references": 0, "out_of_scope": 1,
                              "unreadable": 0, "unclaimed": 0,
                              "known_gaps": 0, "known_gaps_silenced": 0}


def test_未読取は対象外と別に数える(round_: Round, model: Metamodel) -> None:
    """**「資料に無い」と「機械が読めていない」は別物**（後者は拾い直す対象）。"""
    _setup(round_, _ORGANIZED.replace(
        "  - { anchor: s1-x1, reason: 表題 }",
        "  - { anchor: s1-x1, kind: 未読取, reason: 本体は図形でパース結果に出ていない }"))
    report = freeze.gate(round_, model, {})

    assert not report.blocked
    assert report.metrics["unreadable"] == 1


def test_知らない区分はG011(round_: Round, model: Metamodel) -> None:
    _setup(round_, _ORGANIZED.replace(
        "  - { anchor: s1-x1, reason: 表題 }",
        "  - { anchor: s1-x1, kind: 保留, reason: 表題 }"))
    report = freeze.gate(round_, model, {})

    assert "G011" in codes(report.findings)


def test_語彙にある関係でも組み合わせが合わなければG012(
        round_: Round, model: Metamodel) -> None:
    """**通れば build は原理的に失敗しない**を守る（以前は B013 で落ちていた）。"""
    _setup(round_, _ORGANIZED.replace(
        "    source: { anchor: s1-t1 }",
        "    source: { anchor: s1-t1 }\n"
        "    refs: [{ rel: constrains, to: c-受注番号 }]"))
    report = freeze.gate(round_, model, {})

    assert "G012" in codes(report.findings)


def test_向きが逆でもG012にしない(round_: Round, model: Metamodel) -> None:
    """向きの補正は build がやるので、逆向きの宣言は凍結を止めない。"""
    _setup(round_, _ORGANIZED.replace(
        "    source: { anchor: s1-t1 }",
        "    source: { anchor: s1-t1 }\n"
        "    refs: [{ rel: has-column, to: c-T_ORDER }]"))
    known = {"c-T_ORDER": Concept(concept="c-T_ORDER", type="エンティティ")}
    report = freeze.gate(round_, model, known)

    assert "G012" not in codes(report.findings)


def test_未整理のアンカーはG001(round_: Round, model: Metamodel) -> None:
    _setup(round_, _ORGANIZED.split("out_of_scope")[0])
    report = freeze.gate(round_, model, {})

    assert "G001" in codes(report.findings)
    assert report.unclaimed == [("資料/a.xlsx/受注テーブル", "s1-x1")]


def test_壊れたYAMLの覆っていたアンカーは未整理に化ける(
        round_: Round, model: Metamodel) -> None:
    """`G014` は 5 条件と並ぶものではなく、**5 条件が意味を持つための前提**である。

    壊れたファイルは読み飛ばされるのでレコードが 0 件になり、覆っていたアンカーが
    誰にも名乗られない ―― 画面には「未整理 N 件」としか出ないので、**整理の
    やり残しにしか見えない。** 実際はコロン 1 つで数十件が化けることがある。

    だから `G014` が残っているあいだ、ほかの条件の件数は当てにならない。
    """
    # 資料に頻出する「（固定: 130010）」―― 引用符が無いと mapping value になる。
    _setup(round_, _ORGANIZED.replace(
        "    statement: 受注番号は文字列型の項目であること",
        "    statement: 受注番号は採番規則（固定: 130010）に従うこと"))
    report = freeze.gate(round_, model, {})

    assert "G014" in codes(report.findings)
    assert report.metrics["records"] == 0                    # 丸ごと読み飛ばされる
    assert report.metrics["unclaimed"] == 2                  # 整理済みだったほうも
    assert "G001" in codes(report.findings)


def test_語彙に無い種別はG002(round_: Round, model: Metamodel) -> None:
    _setup(round_, _ORGANIZED.replace("type: データ項目", "type: 帳票レイアウト"))
    report = freeze.gate(round_, model, {})

    assert "G002" in codes(report.findings)


def test_参照先のconceptが無ければG003(round_: Round, model: Metamodel) -> None:
    _setup(round_, _ORGANIZED.replace(
        "    source: { anchor: s1-t1 }",
        "    source: { anchor: s1-t1 }\n"
        "    refs: [{ rel: has-column, to: c-知らない }]"))
    report = freeze.gate(round_, model, {})

    assert "G003" in codes(report.findings)


def test_台帳にあるconceptは実在とみなす(round_: Round, model: Metamodel) -> None:
    _setup(round_, _ORGANIZED.replace(
        "    source: { anchor: s1-t1 }",
        "    source: { anchor: s1-t1 }\n"
        "    refs: [{ rel: has-column, to: c-T_ORDER }]"))
    known = {"c-T_ORDER": Concept(concept="c-T_ORDER", type="エンティティ")}
    report = freeze.gate(round_, model, known)

    assert "G003" not in codes(report.findings)


_REFERENCE = """\
records:
  - concept: c-受注番号
    source: { anchor: s1-t1 }
out_of_scope:
  - { anchor: s1-x1, reason: 表題 }
"""


def test_完全なレコードがどこにも無ければG013(round_: Round, model: Metamodel) -> None:
    """参照だけのレコードは種別を名乗らない。**build で落ちる前に止める。**"""
    _setup(round_, _REFERENCE)
    report = freeze.gate(round_, model, {})

    assert "G013" in codes(report.findings)


def test_台帳に種別があれば参照だけでも通る(round_: Round, model: Metamodel) -> None:
    """前ラウンドで定義済みの concept は、今回は参照するだけでよい。"""
    _setup(round_, _REFERENCE)
    known = {"c-受注番号": Concept(concept="c-受注番号", type="データ項目")}
    report = freeze.gate(round_, model, known)

    assert not report.blocked
    assert report.metrics["references"] == 1


def _plus(record: str) -> str:
    """``_ORGANIZED`` の ``records`` にもう 1 件足す（``out_of_scope`` の手前へ）。"""
    head, _, tail = _ORGANIZED.partition("out_of_scope:")
    return f"{head}{record}out_of_scope:{tail}"


def test_同じラウンドに完全なレコードがあれば参照だけでも通る(
        round_: Round, model: Metamodel) -> None:
    _setup(round_, _plus("  - { concept: c-受注番号, source: { anchor: s1-x1 } }\n"))
    report = freeze.gate(round_, model, {})

    assert "G013" not in codes(report.findings)
    assert report.metrics["references"] == 1


def test_参照だけのレコードの関係もG012で見る(round_: Round, model: Metamodel) -> None:
    """種別は完全なレコードから引く ―― 引けたなら組み合わせも検査できる。"""
    _setup(round_, _plus("""\
  - concept: c-受注番号
    source: { anchor: s1-x1 }
    refs: [{ rel: has-value, to: c-受注番号 }]
"""))
    report = freeze.gate(round_, model, {})

    assert "G012" in codes(report.findings)


def test_存在しないアンカーはG004(round_: Round, model: Metamodel) -> None:
    _setup(round_, _ORGANIZED.replace("anchor: s1-t1", "anchor: s9-t9"))
    report = freeze.gate(round_, model, {})

    assert "G004" in codes(report.findings)


def test_シートが差し込まれてずれたアンカーは理由まで言う(
        round_: Round, model: Metamodel) -> None:
    """**資料が改訂されてシートが 1 枚増えると、後ろのアンカーがまとめてずれる。**

    アンカーの `s4` は「ブックの中で何枚目のシートか」なので、差し込みが 1 枚
    あれば以降は全部 1 つずれる ―― パース結果のファイル名（シート名）は
    変わらないので、**同じファイルの同じ表が別の番地になる。**

    黙って壊れる側ではない（ずれた先は他のシートのファイルにあるので必ず
    `G004` になる）。困るのは**落ちる理由が「アンカーがありません」しか
    出ていない**ことで、200 件まとめて落ちたときに、整理結果を書き直す話なのか
    資料が変わった話なのかが分からない。
    """
    parsed(round_, "資料/a.xlsx/受注テーブル.md",
           _PARSED.replace("s1-t1", "s2-t1").replace("s1-x1", "s2-x1"))
    organized(round_, "資料/a.xlsx/受注テーブル.yml", _ORGANIZED)
    report = freeze.gate(round_, model, {})

    said = [f for f in report.findings if f.code == "G004"]
    assert said and all("シートが差し込まれた" in f.message for f in said)
    # レコードも対象外宣言も、**ずれた先の番地まで**言う
    told = "\n".join(f.message for f in said)
    assert "`s2-t1` にあります" in told and "`s2-x1` にあります" in told


def test_中身が消えたアンカーには差し込みの話をしない(
        round_: Round, model: Metamodel) -> None:
    """**当てずっぽうは言わない。** 同じ塊が見当たらないなら、ずれではない。"""
    _setup(round_, _ORGANIZED.replace("anchor: s1-t1", "anchor: s1-k9"))
    report = freeze.gate(round_, model, {})

    said = [f for f in report.findings if f.code == "G004"]
    assert said and "シートが差し込まれた" not in said[0].message


def test_本文に無い語だけならG005で警告(round_: Round, model: Metamodel) -> None:
    """**言い換えは許される**ので error にはできない。"""
    _setup(round_, _ORGANIZED.replace(
        "statement: 受注番号は文字列型の項目であること",
        "statement: 請求締日は月末とすること").replace("name: 受注番号", "name: 請求締日"))
    report = freeze.gate(round_, model, {})

    warned = [f for f in report.findings if f.code == "G005"]
    assert warned and warned[0].level == "warn"


def test_nameがそのまま本文にあればG005を出さない(round_: Round,
                                                   model: Metamodel) -> None:
    """**2 文字の出典は語幹 1 つしか持てない。**

    語に切るとひらがなが落ちる（``戻る`` → ``戻``）ので、漢字 1 字の語幹を
    2 つ揃える規則に原理的に届かない ―― 画面レイアウトのボタンのように出典が
    1 セルしかない項目では、``name`` にも ``statement`` にも何を書いても鳴った。
    同じシートの ``与信残``（漢字 3 字）は鳴らないので、**同性質の 2 件が語の
    長さだけで割れていた**（実測・sales-corpus）。
    """
    parsed(round_, "資料/a.xlsx/受注テーブル.md", _PARSED + """
## セル B31  <!-- a:s1-x2 at=B31 -->

- `B31` 戻る
""")
    organized(round_, "資料/a.xlsx/受注テーブル.yml", _ORGANIZED.replace(
        "out_of_scope:", """\
  - concept: c-btn-戻る
    type: データ項目
    name: 戻る
    statement: 押下すると呼び出し元へ帰る操作であること
    source: { anchor: s1-x2 }
out_of_scope:"""))
    report = freeze.gate(round_, model, {})

    assert "G005" not in codes(report.findings)


def test_語彙の追加提案が未承認ならG007(round_: Round, model: Metamodel) -> None:
    _setup(round_)
    organized(round_, "_metamodel-add.yml",
              "add_item_types: [{ name: 帳票レイアウト, layer: 基本設計 }]\n")
    report = freeze.gate(round_, model, {})

    assert "G007" in codes(report.findings)


def test_保留の提案は理由があれば凍結を通す(round_: Round,
                                            model: Metamodel) -> None:
    """``status: deferred`` は**承認待ちで通しを止めない**ための一級の状態である。

    コード資産では提案が必ず出る（r001 実測 5 件）ので、G007 error のままだと
    無承認の通し実行が構造的にできない ―― 保留は warn として記録に残る。
    """
    _setup(round_)
    organized(round_, "_metamodel-add.yml", """\
add_item_types:
  - name: 帳票レイアウト
    layer: 基本設計
    status: deferred
    deferred_reason: 既存 report で持てるか次のラウンドで判断する
""")
    report = freeze.gate(round_, model, {})

    deferred = [f for f in report.findings if f.code == "G007"]
    assert deferred and deferred[0].level == "warn"
    assert not report.blocked


def test_理由の無い保留はG007のerror(round_: Round, model: Metamodel) -> None:
    """理由の無い保留は、提案を黙って捨てるのと区別できない。"""
    _setup(round_)
    organized(round_, "_metamodel-add.yml", """\
add_item_types:
  - name: 帳票レイアウト
    layer: 基本設計
    status: deferred
""")
    report = freeze.gate(round_, model, {})

    deferred = [f for f in report.findings if f.code == "G007"]
    assert deferred and deferred[0].level == "error"


def test_凍結すると内容が固定される(round_: Round, model: Metamodel) -> None:
    _setup(round_)
    manifest = freeze.apply(round_, freeze.gate(round_, model, {}), today="2026-08-02")

    assert manifest["frozen_at"] == "2026-08-02"
    assert set(manifest["files"]) == {"資料/a.xlsx/受注テーブル.yml"}
    assert round_.is_frozen()
    assert freeze.verify(round_) == []


def test_凍結後の編集はG009(round_: Round, model: Metamodel) -> None:
    _setup(round_)
    freeze.apply(round_, freeze.gate(round_, model, {}))
    path = round_.organized / "資料/a.xlsx/受注テーブル.yml"
    write(path, path.read_text(encoding="utf-8").replace("文字列", "数値"))

    findings = freeze.verify(round_)
    assert codes(findings) == ["G009"]


def test_理由を残せば凍結後の修正も通る(round_: Round, model: Metamodel) -> None:
    """**例外の経路は用意する。** ただし理由が残り、例外として見える。"""
    _setup(round_)
    freeze.apply(round_, freeze.gate(round_, model, {}))
    path = round_.organized / "資料/a.xlsx/受注テーブル.yml"
    write(path, path.read_text(encoding="utf-8").replace("文字列", "数値"))

    from arp4 import yamlio
    manifest = yamlio.load(round_.frozen)
    manifest["amendments"] = [{"file": "資料/a.xlsx/受注テーブル.yml",
                               "reason": "利用者の指示により型を修正"}]
    yamlio.dump(round_.frozen, manifest)

    assert freeze.verify(round_) == []


def test_絵があるのに未読取のままなら注意する(project: Paths, round_: Round,
                                              model) -> None:
    """画像化できるようになると、`未読取` は**読まずに済ませる楽な出口**になる。

    絵が用意されていることは機械が知っているので、そこだけは突く。error にしないのは
    **絵を読んでもなお確定できないことがある**ため（機械に真偽は決められない）。
    """
    parsed(round_, "flow.md",
           "# flow\n\n"
           "## セル B2  <!-- a:s4-x1 at=B2 -->\n\n- `B2` 2. 業務フロー\n\n"
           "## 図形（テキストのみ）  <!-- a:s4-g1 at=図形 19 個 -->\n\n"
           "![業務フロー（A1:CC25）](../images/a.xlsx/業務フロー.png)\n\n"
           "- `図形1` 受注登録\n")
    organized(round_, "flow.yml",
              "out_of_scope:\n"
              "  - { anchor: s4-x1, kind: 未読取, reason: 図で描かれており本体が無い }\n"
              "  - { anchor: s4-g1, kind: 未読取, reason: 線の接続が取れていない }\n")

    report = freeze.gate(round_, model, {})
    unread = [f for f in report.findings if f.code == "G015"]
    assert len(unread) == 2                      # 宣言 2 件ぶん出る
    assert all(f.level == "warn" for f in unread)
    # **宣言は表題のセルに付き、絵は図形のアンカーに貼られる**（揃わないのが普通）
    assert any(f.target.endswith("s4-x1") for f in unread)
    assert "業務フロー.png" in unread[0].message


def test_絵が無ければ未読取は静かに通る(project: Paths, round_: Round, model) -> None:
    parsed(round_, "flow.md", "# flow\n\n## セル B2  <!-- a:s4-x1 at=B2 -->\n\n- `B2` x\n")
    organized(round_, "flow.yml",
              "out_of_scope:\n"
              "  - { anchor: s4-x1, kind: 未読取, reason: 図で描かれており本体が無い }\n")

    assert not [f for f in freeze.gate(round_, model, {}).findings if f.code == "G015"]


def test_対象外には注意しない(project: Paths, round_: Round, model) -> None:
    """`対象外` は「資料に仕様が無い」。絵があっても読み直す理由にならない。"""
    parsed(round_, "flow.md",
           "# flow\n\n## 図形  <!-- a:s4-g1 at=図形 19 個 -->\n\n"
           "![図](../images/a.png)\n\n- `図形1` x\n")
    organized(round_, "flow.yml",
              "out_of_scope:\n  - { anchor: s4-g1, reason: 表紙（仕様ではない） }\n")

    assert not [f for f in freeze.gate(round_, model, {}).findings if f.code == "G015"]


# ── 指摘の位置 ──────────────────────────────────────────────────
_WITH_REFS = """\
records:
  - concept: c-受注番号
    type: データ項目
    name: 受注番号
    statement: 受注番号は文字列型の項目であること
    attrs: { data_type: 文字列 }
    source: { anchor: s1-t1 }
    refs:
      - { rel: has-column, to: c-いない }
out_of_scope:
  - { anchor: s1-x1, reason: 表題 }
"""


def test_参照先が無い指摘はその関係の行を指す(round_: Round, model: Metamodel) -> None:
    """関係は 1 レコードに何本でもぶら下がる（実測 20 本超）。**レコードの先頭を
    指すと、どの 1 本の話かを目で探し直すことになる。**"""
    _setup(round_, _WITH_REFS)
    said = [f for f in freeze.gate(round_, model, {}).findings if f.code == "G003"]

    assert len(said) == 1
    assert said[0].line == 9
    assert said[0].file == ".arp/rounds/2026-08-02/organized/資料/a.xlsx/受注テーブル.yml"
    assert said[0].target == "s1-t1"          # 何の話か（アンカー）


def test_未整理はパース結果を指す(round_: Round, model: Metamodel) -> None:
    """**書く先（整理結果）はまだ無い。** 開けないパスを出しても次の一手にならない。"""
    _setup(round_, _WITH_REFS.replace("  - { anchor: s1-x1, reason: 表題 }\n", ""))
    said = [f for f in freeze.gate(round_, model, {}).findings if f.code == "G001"]

    assert [f.target for f in said] == ["s1-x1"]
    assert said[0].file == ".arp/rounds/2026-08-02/parsed/資料/a.xlsx/受注テーブル.md"


#: コードのパース結果（`parse._MEMBER` の並び）と、その整理結果。
_CODE_PARSED = """\
# yamlio.py

<!-- source: yamlio.py -->

## モジュール関数  <!-- a:m1 at=yamlio.py#L51-L160 -->

| メンバ | 種類 | 注釈 | シグネチャ | 戻り値 | 例外 | 行 |
|---|---|---|---|---|---|---|
| load | 関数 |  | load(path: Path) | Any | YamlError, _broken | 51 |
| marked | 関数 |  | marked(text: str) | tuple[Any, Marks] | _broken | 141 |
| scan_tree | 関数 |  | scan_tree(directory: Path) | list[Path] |  | 166 |
"""

_CODE_ORGANIZED = """\
records:
  - concept: c-mtd-yamlio-load
    type: メソッド
    name: yamlio.load
    statement: load は YAML のファイルを読み込むこと
    attrs:
      signature: "yamlio.load(path: Path)"
      returns: Any
      raises: YamlError
    source: { anchor: m1 }
  - concept: c-mtd-yamlio-marked
    type: メソッド
    name: yamlio.marked
    statement: marked は文字列の YAML を読み込むこと
    attrs:
      signature: "yamlio.marked(text: str)"
      returns: "tuple[Any, Marks]"
    source: { anchor: m1 }
  - concept: c-mtd-yamlio-tree
    type: メソッド
    name: yamlio.scan_tree
    statement: scan_tree は階層ごとたどって集めること
    attrs:
      signature: "yamlio.scan_tree(directory: Path)"
      returns: "list[Path]"
    source: { anchor: m1 }
"""


def _code(round_: Round, organized_body: str = _CODE_ORGANIZED) -> None:
    parsed(round_, "yamlio.py.md", _CODE_PARSED)
    organized(round_, "yamlio.py.yml", organized_body)


def test_出典の欄を落としたらG018(round_: Round, model: Metamodel) -> None:
    """**読めていたものが静かに消える**のを、凍結の前に数える。

    パース結果は `marked` の例外を `_broken` と読めていたのに、整理結果は
    `raises` を書いていない。空欄に見えるので、あとから正本を見ても「資料に
    無かった」のか「書き忘れた」のかが区別できない。
    """
    _code(round_)
    said = [f for f in freeze.gate(round_, model, {}).findings if f.code == "G018"]

    assert len(said) == 1
    assert said[0].message.startswith("yamlio.marked: ")   # どの行の話か
    assert "例外" in said[0].message and "_broken" in said[0].message
    assert said[0].file == ".arp/rounds/2026-08-02/organized/yamlio.py.yml"


def test_値の書き換えはG018にしない(round_: Round, model: Metamodel) -> None:
    """**「落とした」は言えるが「違う」は言えない。**

    `load` の出典は `YamlError, _broken` で、整理結果は `YamlError` ―― private な
    ヘルパを落としただけで正しい。どちらが正かは機械には決められないので、
    比べるのは**空か空でないか**だけにする。
    """
    _code(round_)
    said = [f for f in freeze.gate(round_, model, {}).findings if f.code == "G018"]

    assert not [f for f in said if "yamlio.load:" in f.message]


def test_出典の欄が空ならG018にしない(round_: Round, model: Metamodel) -> None:
    """`scan_tree` は例外を投げない。**資料が空なら書かないのが正しい。**"""
    _code(round_)
    said = [f for f in freeze.gate(round_, model, {}).findings if f.code == "G018"]

    assert not [f for f in said if "yamlio.scan_tree:" in f.message]


def test_行を当てられなければG018は黙る(round_: Round, model: Metamodel) -> None:
    """**どの行の話か決められないなら言わない。**

    整理結果の名前は修飾されている（`yamlio.marked`）が表の左端は短い名前である。
    区切りが語の切れ目でないもの ―― `scan_tree` の行を `tree` が掴む ―― を
    拾うと、無関係な行の欄を「落とした」と言い出す。
    """
    _code(round_, _CODE_ORGANIZED.replace("name: yamlio.marked", "name: tree"))
    said = [f for f in freeze.gate(round_, model, {}).findings if f.code == "G018"]

    assert said == []


def test_見出しを自分で書いていない表は照合しない(round_: Round,
                                                  model: Metamodel) -> None:
    """**Excel の見出しは現場が書いた文字列**で、「戻り値」「返却値」と揺れる。

    `column` を宣言していない種別は黙る ―― `data-item` の表（`_PARSED`）は
    列名も並びも資料ごとに違うので、機械には欄と属性の対応が付けられない。
    """
    _setup(round_)

    assert "G018" not in codes(freeze.gate(round_, model, {}).findings)


# ── 要る関係が 0 本（G020） ────────────────────────────────────
_CST_PARSED = """\
# render.py

<!-- source: render.py -->

## 定数  <!-- a:v1 at=render.py#L30-L31 -->

| メンバ | 種類 | シグネチャ | 行 |
|---|---|---|---|
| TARGET_PX | 定数 | TARGET_PX = 1400 | 30 |

## 取り込み  <!-- a:i1 at=render.py -->

| 取り込み | 元 | 行 |
|---|---|---|
| from arp4 import mdio | arp4 | 22 |
"""

_CST_ORGANIZED = """\
records:
  - concept: c-mod-render
    type: モジュール
    name: arp4.render
    statement: render はシートを絵にすること
    source: { anchor: i1 }
  - concept: c-cst-target-px
    type: 制約・前提
    name: 書き出す画像の大きさ
    statement: 書き出す画像の長辺は 1400 画素を目安とすること
    attrs: { category: 技術 }
    source: { anchor: v1 }
"""


def test_語彙が要ると言っている関係が0本ならG020(
        round_: Round, model: Metamodel) -> None:
    """**書いている本人の手元で鳴らす。**

    判定も材料（`warn_if_no_upstream`）も `W031` と同じで、違うのは時期だけである
    ―― `W031` は `build` が正本を書いたあとに鳴るので、そのころ整理層はもう次の
    ファイルへ移っていて、`constrains` を張る先が本文のどこに書いてあったかは
    開き直さないと分からない。実測では制約 32 件が 32 件とも繋がっていなかった。
    """
    parsed(round_, "render.py.md", _CST_PARSED)
    organized(round_, "render.py.yml", _CST_ORGANIZED)
    said = [f for f in freeze.gate(round_, model, {}).findings if f.code == "G020"]

    assert len(said) == 1
    assert said[0].level == "warn"                    # build は落ちない
    assert said[0].message.startswith("書き出す画像の大きさ: constrains")
    assert said[0].file == ".arp/rounds/2026-08-02/organized/render.py.yml"


def test_関係が書いてあればG020を出さない(round_: Round, model: Metamodel) -> None:
    parsed(round_, "render.py.md", _CST_PARSED)
    organized(round_, "render.py.yml", _CST_ORGANIZED.replace(
        "    source: { anchor: v1 }",
        "    refs: [{ rel: constrains, to: c-mod-render }]\n"
        "    source: { anchor: v1 }"))

    assert "G020" not in codes(freeze.gate(round_, model, {}).findings)


def test_関係が別のファイルにあればG020を出さない(
        round_: Round, model: Metamodel) -> None:
    """**数えるのは concept 単位。**

    同じ concept のレコードは複数のファイルに散るので、関係が別のファイルに
    書いてあることがある。レコード単位で数えると、正しい整理が誤検出になる。
    """
    parsed(round_, "render.py.md", _CST_PARSED)
    organized(round_, "render.py.yml", _CST_ORGANIZED)
    parsed(round_, "publish.py.md", _CST_PARSED.replace("render.py", "publish.py"))
    organized(round_, "publish.py.yml", """\
records:
  - concept: c-cst-target-px
    refs: [{ rel: constrains, to: c-mod-render }]
    source: { anchor: v1 }
  - concept: c-mod-publish
    type: モジュール
    name: arp4.publish
    statement: publish は設計書を組み立てること
    source: { anchor: i1 }
""")

    assert "G020" not in codes(freeze.gate(round_, model, {}).findings)


# ── 調べたうえで相手がいない（known_gaps） ────────────────────
_DECLARED = """\
    known_gaps:
      constrains:
        reason: 規模の想定で、縛る先の列がそもそも無い（このラウンドの資料に無い）
"""


def test_known_gapsを宣言すればG020を出さない(round_: Round,
                                              model: Metamodel) -> None:
    """**整理層が「調べたうえで相手がいない」と言える場所。**

    ここが無いあいだ、正本の `known_gaps` は `build` を打った人の欄で、分担して
    いるとき配る側は `build` を禁じる ―― 担当は warn を残したまま報告文に書く
    しかなかった。実測（8 冊 / 8 分担 × 2 周）で 80 件超が 2 周とも warn のまま
    親へ渡り、8 人中 4 人が「宣言する場所が無い」と独立に報告した。
    """
    parsed(round_, "render.py.md", _CST_PARSED)
    organized(round_, "render.py.yml",
              _CST_ORGANIZED.replace("    source: { anchor: v1 }\n",
                                     "    source: { anchor: v1 }\n" + _DECLARED))
    report = freeze.gate(round_, model, {})

    assert "G020" not in codes(report.findings)
    assert not [f for f in report.findings if f.level == "error"]
    # **黙って消えない。** 件数は必ず集計に出る（消えたのか無かったのかを分ける）。
    assert report.metrics["known_gaps"] == 1
    assert report.metrics["known_gaps_silenced"] == 1


def test_理由の無い宣言はG020を降ろせない(round_: Round, model: Metamodel) -> None:
    """理由が無ければスキーマ（`G006`）が宣言そのものを落とすので、`G020` は残る。"""
    parsed(round_, "render.py.md", _CST_PARSED)
    organized(round_, "render.py.yml", _CST_ORGANIZED.replace(
        "    source: { anchor: v1 }\n",
        "    source: { anchor: v1 }\n    known_gaps:\n      constrains: {}\n"))
    report = freeze.gate(round_, model, {})

    assert "G006" in codes(report.findings)
    assert "G020" in codes(report.findings)
    assert report.metrics["known_gaps_silenced"] == 0


def test_known_gapsの名前が語彙に無ければG031(round_: Round,
                                              model: Metamodel) -> None:
    """**誤字の宣言は二重に効く。** `G020` を降ろさないうえ、`build` が正本へ
    運んだ先で `E018` の error になる ―― そのとき整理結果はもう編集できない。
    """
    parsed(round_, "render.py.md", _CST_PARSED)
    organized(round_, "render.py.yml", _CST_ORGANIZED.replace(
        "    source: { anchor: v1 }\n",
        "    source: { anchor: v1 }\n"
        "    known_gaps:\n      constrain:\n        reason: 縛る先が無い\n"))
    said = [f for f in freeze.gate(round_, model, {}).findings if f.code == "G031"]

    assert len(said) == 1
    assert said[0].level == "error"
    assert "constrain" in said[0].message
    assert said[0].file == ".arp/rounds/2026-08-02/organized/render.py.yml"


def test_属性名のknown_gapsも受ける(round_: Round, model: Metamodel) -> None:
    """正本の `known_gaps` が**関係型と属性名の両方**を受けるので揃える
    （書く側から見れば「この欄が資料に無い」の一言で足りる）。"""
    parsed(round_, "render.py.md", _CST_PARSED)
    organized(round_, "render.py.yml", _CST_ORGANIZED.replace(
        "    source: { anchor: v1 }\n",
        "    source: { anchor: v1 }\n"
        "    known_gaps:\n      category:\n        reason: 資料に区分の欄が無い\n"))
    findings = freeze.gate(round_, model, {}).findings

    assert "G031" not in codes(findings)


def test_関係が書いてあるのに宣言が残っていればG031(round_: Round,
                                                  model: Metamodel) -> None:
    """`check` の `W033` と同じ ―― **古い言い訳が正本に残るほうが error より
    始末が悪い。** 正本へ運ぶ前にここで言う。
    """
    parsed(round_, "render.py.md", _CST_PARSED)
    organized(round_, "render.py.yml", _CST_ORGANIZED.replace(
        "    source: { anchor: v1 }\n",
        "    refs: [{ rel: constrains, to: c-mod-render }]\n"
        "    source: { anchor: v1 }\n" + _DECLARED))
    said = [f for f in freeze.gate(round_, model, {}).findings if f.code == "G031"]

    assert len(said) == 1
    assert said[0].level == "warn"
    assert "宣言を消してください" in said[0].message


def test_known_gapsの名前はlintでも見る(round_: Round, model: Metamodel) -> None:
    """名前の照合は**1 ファイルで決まる**（相手は語彙であって他の整理結果ではない）。

    「宣言したのに関係が書いてある」ほうは `gate` でしか言えない ―― 関係は
    別のファイルに書いてありうる（`G020` と同じ規律）。
    """
    parsed(round_, "render.py.md", _CST_PARSED)
    organized(round_, "render.py.yml", _CST_ORGANIZED.replace(
        "    source: { anchor: v1 }\n",
        "    source: { anchor: v1 }\n"
        "    known_gaps:\n      constrain:\n        reason: 縛る先が無い\n"))
    report = freeze.lint(round_, model, {})

    assert "G031" in codes(report.findings)
    assert report.metrics["known_gaps"] == 1


_TST_PARSED = """\
# test_build.py

<!-- source: tests/test_build.py -->

## テスト  <!-- a:t1 at=tests/test_build.py#L10-L40 -->

| テスト | 行 |
|---|---|
| test_採番が安定している | 12 |

## 取り込み  <!-- a:i1 at=tests/test_build.py -->

| 取り込み | 元 | 名前 | 行 |
|---|---|---|---|
| from arp4 import build | arp4 | build | 3 |
"""

_TST_ORGANIZED = """\
records:
  - concept: c-mod-tests.test_build
    type: モジュール
    name: tests.test_build
    statement: test_build は build の検証を束ねること
    source: { anchor: i1 }
  - concept: c-tcs-tests.test_build.test_採番が安定している
    type: テストケース
    name: 採番の安定性
    statement: 同じ入力からは同じ採番が出ること
    attrs: { expected: 同じ採番が出る }
    source: { anchor: t1 }
"""


def test_検証相手の無いテストケースはG020(round_: Round, model: Metamodel) -> None:
    """`constraint`（3.6.0）と同じ付け忘れを、テストケースでも書いている手元で言う。

    実測では 527 件中 517 件が `verifies` 0 本のまま全検査を通り、気づけたのは
    トレーサビリティ・マトリクスが全章「（該当なし）」で出たあとだった。
    """
    parsed(round_, "tests/test_build.py.md", _TST_PARSED)
    organized(round_, "tests/test_build.py.yml", _TST_ORGANIZED)
    said = [f for f in freeze.gate(round_, model, {}).findings if f.code == "G020"]

    assert len(said) == 1
    assert said[0].level == "warn"
    assert said[0].message.startswith("採番の安定性: verifies")


def test_moduleへ張ったverifiesは語彙に合う(round_: Round, model: Metamodel) -> None:
    """テストケース → モジュールは正しい組み合わせである（G012 にも G020 にもならない）。

    テストファイルが相手にするのは仕組み＝ファイル 1 本で、`method` へ無理に
    届かせる必要はない。
    """
    parsed(round_, "tests/test_build.py.md", _TST_PARSED)
    organized(round_, "tests/test_build.py.yml", _TST_ORGANIZED.replace(
        "    source: { anchor: t1 }",
        "    refs: [{ rel: verifies, to: c-mod-tests.test_build }]\n"
        "    source: { anchor: t1 }"))
    findings = freeze.gate(round_, model, {}).findings

    assert "G020" not in codes(findings)
    assert "G012" not in codes(findings)


# ── 台帳への提案（_concepts.yml） ──────────────────────────────
def test_台帳への提案の型が語彙に無ければG002(round_: Round,
                                              model: Metamodel) -> None:
    """台帳は次のラウンドの判断材料 ―― **レコードと同じ語彙を守らせる。**

    台帳だけ別の語彙を持てると、次のラウンドで台帳を引いた整理が語彙外の
    type をレコードへ写す。
    """
    _setup(round_)
    organized(round_, "_concepts.yml",
              "new: [{ concept: c-帳票A, type: 帳票レイアウト }]\n")
    said = [f for f in freeze.gate(round_, model, {}).findings
            if f.code == "G002"]

    assert said and "帳票レイアウト" in said[0].message
    assert said[0].file and said[0].file.endswith("_concepts.yml")


def test_assignの相手が台帳に無ければG021(round_: Round,
                                           model: Metamodel) -> None:
    """build の B003 と同じ判定を、**書いている手元**（凍結前）で言う。"""
    _setup(round_)
    organized(round_, "_concepts.yml",
              "assign: [{ concept: c-未知, aliases_add: [別名] }]\n")

    assert "G021" in codes(freeze.gate(round_, model, {}).findings)


def test_台帳にある相手へのassignは通る(round_: Round, model: Metamodel) -> None:
    _setup(round_)
    organized(round_, "_concepts.yml",
              "assign: [{ concept: c-顧客, aliases_add: [得意先コード] }]\n")
    known = {"c-顧客": Concept(concept="c-顧客", type="データ項目", label="顧客")}
    findings = freeze.gate(round_, model, known).findings

    assert "G021" not in codes(findings)
    assert "G002" not in codes(findings)


def test_lintでも台帳の提案を見る(round_: Round, model: Metamodel) -> None:
    """`_concepts.yml` は 1 ファイルで決まる（台帳と語彙は入力であって他の
    整理結果ではない）ので、lint の線引きの内側である。"""
    path = organized(round_, "_concepts.yml",
                     "new: [{ concept: c-x, type: 存在しない型 }]\n")
    report = freeze.lint(round_, model, {}, only=[path])

    assert "G002" in codes(report.findings)


def test_門は原本のずれも見る(project: Paths, round_: Round,
                              model: Metamodel) -> None:
    """**上流と下流の両方を照合する。**

    門の 5 条件は整理結果とパース結果の関係しか見ておらず、その手前
    （原本 → パース結果）が動いていないことは誰も確かめていなかった。
    """
    from arp4 import parse as parse_module
    from conftest import write as _write

    source = _write(sources_dir(project) / "a.py", "X = 1\n")
    targets, _ = parse_module.plan(round_, [source], sources_dir(project))
    written, _ = parse_module.write(targets)
    parse_module.record(round_, targets, written)
    organized(round_, "a.py.yml", """\
records:
  - concept: c-cst-x
    type: 制約・前提
    name: X の値
    statement: X は 1 であること
    attrs: { category: 技術 }
    refs: [{ rel: constrains, to: c-cst-x2 }]
    source: { anchor: v1 }
out_of_scope:
  - { anchor: i1, reason: 取り込みだけ }
""")

    assert "G019" not in codes(freeze.gate(round_, model, {}).findings)
    _write(source, "X = 2\n")
    assert "G019" in codes(freeze.gate(round_, model, {}).findings)


def test_本文に語が無いアンカーではG005を出さない(
        round_: Round, model: Metamodel) -> None:
    """**照合できなかったことを、一致しなかったことにしない。**

    取り込みを 1 本も持たないモジュールの `i1` は見出しだけの表なので、
    突き合わせる語が原理的に 0 個である。決定 36 は「識別子をそのまま `name` に
    置く」で応じる読み方を示したが、置くべき識別子が本文に無い。
    """
    parsed(round_, "__init__.py.md", """\
# __init__.py

<!-- source: __init__.py -->

## 取り込み  <!-- a:i1 at=__init__.py -->

| 取り込み | 元 | 行 |
|---|---|---|
""")
    organized(round_, "__init__.py.yml", """\
records:
  - concept: c-mod-init
    type: モジュール
    name: arp4.__init__
    statement: __init__ はパッケージの入口として、意味の判断を整理層だけに置くことを宣言すること
    source: { anchor: i1 }
""")

    assert "G005" not in codes(freeze.gate(round_, model, {}).findings)


def test_本文に語があれば従来どおりG005を出す(
        round_: Round, model: Metamodel) -> None:
    """黙るのは**本文が空のとき**だけである（出典の取り違えはコードでも起きる）。"""
    parsed(round_, "a.py.md", """\
# a.py

<!-- source: a.py -->

## 取り込み  <!-- a:i1 at=a.py -->

| 取り込み | 元 | 行 |
|---|---|---|
| import hashlib | hashlib | 3 |
""")
    organized(round_, "a.py.yml", """\
records:
  - concept: c-mod-zzz
    type: モジュール
    name: arp4.受注登録
    statement: 受注登録は受注を台帳へ書き込む処理であること
    source: { anchor: i1 }
""")

    assert "G005" in codes(freeze.gate(round_, model, {}).findings)


_JP_TEST_PARSED = """\
# test_organized.py

<!-- source: tests/test_organized.py -->

## テスト  <!-- a:t1 at=tests/test_organized.py#L10-L40 -->

| テスト | 行 |
|---|---|
| test_3つ揃えるか3つとも省くか | 12 |
"""


def test_言い換えを足してもG005で罰しない(round_: Round,
                                          model: Metamodel) -> None:
    """**説明を丁寧にするほど鳴り、何も足さないほど静かに通る**、の逆転を直す。

    日本語のテスト名は漢字とひらがなが交互に来る ―― 漢字 2 字未満を捨てる
    切り出しでは語が 1 つも取れず検査そのものが飛び、そこへ価値のある言い換え
    （`type と name と statement の揃え方`）を足すと**足した語だけが照合されて**
    鳴った。実際の「直し方」は言い換えを削ることになり、設計書から情報が消えた。
    語幹の漢字（揃・省）は活用が変わっても残るので、そこで繋ぐ。
    """
    parsed(round_, "tests/test_organized.py.md", _JP_TEST_PARSED)
    organized(round_, "tests/test_organized.py.yml", """\
records:
  - concept: c-tcs-tests.test_organized.test_3つ揃えるか3つとも省くか
    type: テストケース
    name: type と name と statement の揃え方
    statement: type と name と statement は 3 つ揃えるか 3 つとも省くかであること
    attrs: { expected: 中途半端なレコードが G006 で止まる }
    source: { anchor: t1 }
""")
    findings = freeze.gate(round_, model, {}).findings

    assert "G005" not in codes(findings)


def test_語幹1字だけの偶然一致ではG005は黙らない(round_: Round,
                                                 model: Metamodel) -> None:
    """見出しは arp4 自身が書く（`取り込み` → 取・込）ので、1 字の一致を許すと
    コードの塊で検査がほぼ黙る。**独立な語幹が 2 つ**揃って初めて一致とみなす。"""
    parsed(round_, "tests/test_organized.py.md", _JP_TEST_PARSED)
    organized(round_, "tests/test_organized.py.yml", """\
records:
  - concept: c-tcs-tests.test_organized.test_3つ揃えるか3つとも省くか
    type: テストケース
    name: 請求書の締め日を揃える規則
    statement: 請求書の締め日は月末に揃えること
    attrs: { expected: 締め日が月末になる }
    source: { anchor: t1 }
""")

    assert "G005" in codes(freeze.gate(round_, model, {}).findings)
