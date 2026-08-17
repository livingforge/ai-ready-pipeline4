"""通し ―― **資料 → パース → 整理 → 凍結 → 正本 → 設計書**が 1 本で通るか。

整理層はエージェントの仕事なので、ここでは**人が書いたつもりの整理結果**を置く。
その形が実際に書けるものかどうかを確かめるのが、この試験のもう 1 つの目的である。
"""

from __future__ import annotations

import importlib.util
from pathlib import Path

import pytest

from arp4 import cli, metamodel as mm, paths as paths_module
from conftest import sources_dir, write

_EXAMPLES = Path(__file__).resolve().parents[1] / "examples"

#: **冊をまたいで比べる**指摘。1 冊だけの母集合では成り立ちようがない
#: （比べる相手がいない）。``P110`` / ``P111`` はこちらではない ―― あれは
#: 正本と様式を比べるので、母集合を絞ると増える。
_CROSS_DOCUMENT = ("P106", "P107")


def _sample_module():
    spec = importlib.util.spec_from_file_location(
        "make_sample", _EXAMPLES / "make_sample.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _make_sample(directory: Path) -> Path:
    """見本の資料を組んで、**その置き場を返す**（parse に渡すため）。"""
    _sample_module().build(directory)
    return directory


def _make_documents(directory: Path) -> Path:
    """Excel 以外の見本（Word・PowerPoint・PDF・CSV）を組む。"""
    _sample_module().build_documents(directory)
    return directory


_ORGANIZED = {
    "資料/基本設計書.xlsx/受注テーブル.yml": """\
records:
  - concept: c-T_ORDER
    type: エンティティ
    name: 受注
    statement: 受注テーブル T_ORDER は受注 1 件を 1 レコードで保持すること
    attrs: { physical_name: T_ORDER, entity_kind: トランザクション }
    source: { anchor: s1-t1 }
    refs:
      - { rel: has-column, to: c-受注番号,
          attrs: { physical_name: ORDER_NO, pk: true, not_null: true } }
      - { rel: has-column, to: c-受注日,
          attrs: { physical_name: ORDER_DATE, not_null: true } }
      - { rel: has-column, to: c-顧客コード,
          attrs: { physical_name: CUSTOMER_CD, not_null: true } }
  - concept: c-受注番号
    type: データ項目
    name: 受注番号
    statement: 受注番号は文字列型（10 桁）の項目であること
    attrs: { data_type: 文字列, length: 10 }
    source: { anchor: s1-t1 }
  - concept: c-受注日
    type: データ項目
    name: 受注日
    statement: 受注日は日付型の項目であること
    attrs: { data_type: 日付 }
    source: { anchor: s1-t1 }
  - concept: c-顧客コード
    type: データ項目
    name: 顧客コード
    statement: 顧客コードは文字列型（8 桁）の項目であること
    attrs: { data_type: 文字列, length: 8 }
    source: { anchor: s1-t1 }
out_of_scope:
  - { anchor: s1-x1, reason: 表題と改訂番号（仕様ではない） }
""",
    "資料/基本設計書.xlsx/表紙.yml": "out_of_scope:\n  - { anchor: s2-x1, reason: 表紙 }\n",
    # `priority` は**わざと入れてある。**`test_stakeholderの記録が…` は
    # 「stakeholder は `P1xx` を回さない」を見るので、**developer 側が `P1xx` を
    # 1 件も出さないと、その否定が空振りになる。**
    #
    # 長いあいだその 1 件を担っていたのは `P107`（entity の出典列が基本設計書と
    # テーブル定義書で揃っていない）だったが、あれはパック側の不具合で、直した
    # 時点で番人が消えた ―― **不具合に寄りかかっていた。**代わりに置いたのが
    # これである。`priority` は要件の 3 節では列に出るが業務ルールの節には無いので
    # `P111 business-rule.priority` が出る。種別×属性の粒度（同じ属性でも節が
    # 違えば出たり出なかったりする）をそのまま突く形で、寄りかかる先としては
    # 不具合より確かである。
    "資料/運用設計.xlsx/運用方針.yml": """\
records:
  - concept: c-受注データの保持期間
    type: 業務ルール
    name: 受注データの保持期間
    statement: 受注データは 13 か月でアーカイブすること
    attrs: { rule_kind: processing, action: 13 か月でアーカイブする,
             priority: 必須 }
    source: { anchor: s1-x1 }
    refs: [{ rel: constrains, to: c-T_ORDER }]
""",
}


#: Excel 以外の 4 形式（Word・PowerPoint・PDF・CSV）から起こした整理結果。
#:
#: **1 形式から 1 つ以上のレコードを起こす。**通しで見たいのは「読めた」では
#: なく、**形式ごとに違うアンカー**（`w3-` 節・`p4-` ページ・`s2-` スライド・
#: 接頭辞の無い CSV）が凍結・正本・出典の照合・設計書の出典欄まで、そのままの
#: 形で流れるかである ―― パースだけを見ていると、ここは 1 度も通らない。
#:
#: 同じ concept が別々の形式に出てくるのも**わざとである**（`c-T_ORDER` は
#: PowerPoint の移行対象一覧と Word の画面項目表の両方に、`c-M_CUSTOMER` は
#: PowerPoint と CSV の両方に出る）。現場の 1 案件はそうなっており、出典が
#: 冊をまたいで揃うかどうかはそこでしか見えない。
_DOCUMENTS_ORGANIZED = {
    # PowerPoint ―― 移行対象の表（スライド 2 枚目）。
    "資料/方式提案.pptx/02_移行対象と件数.yml": """\
records:
  - concept: c-T_ORDER
    type: エンティティ
    name: 受注ヘッダ
    statement: 受注ヘッダ T_ORDER は移行対象であり、移行件数は 120,000 件であること
    attrs: { physical_name: T_ORDER, entity_kind: トランザクション, volume: "120000" }
    source: { anchor: s2-t1 }
  - concept: c-M_CUSTOMER
    type: エンティティ
    name: 得意先
    statement: 得意先マスタ M_CUSTOMER は移行対象であり、移行件数は 3,200 件であること
    attrs: { physical_name: M_CUSTOMER, entity_kind: マスタ, volume: "3200" }
    source: { anchor: s2-t1 }
""",
    # Word ―― 見出し 1 で割れた 3 節目（画面項目の表）。
    "資料/受注登録機能仕様書.docx/03_2 画面項目.yml": """\
records:
  - concept: c-受注登録画面
    type: 画面
    name: 受注登録画面
    statement: 受注登録画面は受注ヘッダと受注明細を 1 画面で登録すること
    attrs: { screen_type: 入力 }
    source: { anchor: w3-t1 }
    refs:
      - { rel: displays, to: c-得意先コード,
          attrs: { io: 入力, required_flag: 必須 } }
      - { rel: accesses, to: c-T_ORDER, attrs: { crud: [C] } }
  - concept: c-得意先コード
    type: データ項目
    name: 得意先コード
    statement: 得意先コードは文字列型（5 桁）の必須項目であること
    attrs: { data_type: 文字列, length: 5 }
    source: { anchor: w3-t1 }
  # **参照だけのレコード。** 受注ヘッダを定義しているのは PowerPoint 側で、
  # ここが言っているのは「その表に列が 1 本ある」だけである ―― 定義を 2 度
  # 書くと、同じ事実が出典どうしで食い違ったことになる（`W045`）。
  - concept: c-T_ORDER
    source: { anchor: w3-t1 }
    refs:
      - { rel: has-column, to: c-得意先コード,
          attrs: { physical_name: CUSTOMER_CD, not_null: true } }
""",
    # Word ―― 4 節目（本文の段落。表ではない）。
    "資料/受注登録機能仕様書.docx/04_3 業務ルール.yml": """\
records:
  - concept: c-与信枠超過の承認
    type: 業務ルール
    name: 与信枠超過時の承認
    statement: 与信枠を超える受注は、営業部長の承認を得るまで出荷指示を行わないこと
    attrs: { rule_kind: business, condition: 与信枠を超える受注,
             action: 営業部長の承認を得るまで出荷指示を行わない }
    source: { anchor: w4-h1 }
    refs: [{ rel: constrains, to: c-T_ORDER }]
""",
    # PDF ―― しおりで割れた 3 節目（原本では罫線の表。行のまま出ている）。
    "資料/検収仕様書.pdf/03_2 確認項目.yml": """\
records:
  - concept: c-数量0の確認
    type: テストケース
    name: 数量に 0 を入力
    statement: 受注登録画面で数量に 0 を入力するとエラーとなること
    attrs: { expected: エラーとなる, level: 受入,
             steps: 受注登録画面で数量に 0 を入力する }
    source: { anchor: p4-x1 }
    refs: [{ rel: verifies, to: c-受注登録画面 }]
""",
    # CSV ―― **1 ファイルが 1 本**なので、アンカーに接頭辞が無い（`t1`）。
    "資料/得意先マスタ移行.csv.yml": """\
records:
  # ここも参照だけ（得意先マスタを定義しているのは PowerPoint 側）。
  # CSV が言っているのは**見出し行にどの列があるか**である。
  - concept: c-M_CUSTOMER
    source: { anchor: t1 }
    refs:
      - { rel: has-column, to: c-得意先コード,
          attrs: { physical_name: CUSTOMER_CD, pk: true, not_null: true } }
      - { rel: has-column, to: c-得意先名,
          attrs: { physical_name: CUSTOMER_NAME, not_null: true } }
  - concept: c-得意先名
    type: データ項目
    name: 得意先名
    statement: 得意先名は文字列型の項目であること
    attrs: { data_type: 文字列 }
    source: { anchor: t1 }
""",
}

#: 形式ごとの出典。**設計書の出典欄にこの形で出る**（ラウンド名＋パース結果の
#: 道＋アンカー）。接頭辞が形式ごとに違うので、1 つでも欠ければどの形式の
#: 経路が切れているかがそのまま分かる。
_DOCUMENT_SOURCES = (
    "資料/方式提案.pptx/02_移行対象と件数#s2-t1",
    "資料/受注登録機能仕様書.docx/03_2 画面項目#w3-t1",
    "資料/受注登録機能仕様書.docx/04_3 業務ルール#w4-h1",
    "資料/検収仕様書.pdf/03_2 確認項目#p4-x1",
    "資料/得意先マスタ移行.csv#t1",
)


@pytest.fixture
def 通し(tmp_path: Path):
    paths = paths_module.create(tmp_path)
    資料 = str(_make_sample(sources_dir(paths)))
    root = str(tmp_path)

    assert cli.main(["parse", "--root", root, "--round", "2026-08-02",
                     資料]) == 0
    round_ = paths.round("2026-08-02")
    for name, body in _ORGANIZED.items():
        write(round_.organized / name, body)
    return paths, root


@pytest.fixture
def 通し_文書(tmp_path: Path, pdf_reader):
    """Excel 以外の 4 形式で、整理結果まで用意した 1 ラウンド。

    残りのアンカーは `declare` でまとめて対象外にする ―― **実際の使われ方が
    そうだから**である（表紙・体制図・発表者ノートを 1 枚ずつ書く人はいない）。
    ここを手書きの YAML で埋めると、通しの経路から `declare` が抜ける。
    """
    paths = paths_module.create(tmp_path)
    資料 = str(_make_documents(sources_dir(paths)))
    root = str(tmp_path)

    assert cli.main(["parse", "--root", root, "--round", "2026-08-02",
                     資料]) == 0
    round_ = paths.round("2026-08-02")
    for name, body in _DOCUMENTS_ORGANIZED.items():
        write(round_.organized / name, body)
    assert cli.main(["declare", "--root", root, "--round", "2026-08-02", "*",
                     "--reason", "表紙・体制図・発表者ノート（この通しでは起こさない）"]) == 0
    return paths, root


def test_Excel以外の4形式が設計書まで通る(通し_文書) -> None:
    """**Word・PowerPoint・PDF・CSV が、正本と設計書まで 1 本で通るか。**

    パースのテストは「読めたか」までしか見ない。そこから先 ―― 凍結が
    アンカーを数え、`build` が出典を正本へ写し、`check` が出典の実在を
    確かめ、`publish` が出典欄に刷る ―― の経路は、長いあいだ **Excel の
    パース結果でしか通っていなかった。**

    ここが Excel 専用に書かれていても、パースのテストは 1 本も落ちない
    （落ちるのは、Excel 以外を実際に流した人のところである）。アンカーの
    接頭辞は形式ごとに違う（`w3-` `p4-` `s2-` と、CSV の接頭辞なし）ので、
    **どこかが `s` を決め打っていればここで止まる。**
    """
    paths, root = 通し_文書

    for step in ("freeze", "build", "number", "check", "publish"):
        assert cli.main([step, "--root", root]) == 0, f"{step} で止まりました"

    # 4 形式ぶんの正本が組み上がっている（種別は形式ではなく資料の中身で決まる）。
    for kind in ("entity", "screen", "data-item", "business-rule", "test-case"):
        assert (paths.items / f"{kind}.yml").is_file(), kind

    # **出典は形式ごとのアンカーのまま設計書に出る。**
    # 見るのは HTML である ―― Markdown の原文では `03_2 画面項目` が
    # `03\_2 画面項目` と逃がされている（`__init__` が `init` に化けるのを
    # 防ぐため）ので、**読み手に届く字**のほうで確かめる。
    published = "\n".join(
        path.read_text(encoding="utf-8") for path in paths.out.rglob("*.html"))
    for source in _DOCUMENT_SOURCES:
        assert f"2026-08-02 {source}" in published, (
            f"{source} が設計書の出典に出ていません")


def test_凍結から設計書まで通る(通し, capsys: pytest.CaptureFixture) -> None:
    paths, root = 通し

    assert cli.main(["freeze", "--root", root]) == 0
    assert paths.round("2026-08-02").is_frozen()

    assert cli.main(["build", "--root", root]) == 0
    assert (paths.items / "entity.yml").is_file()
    assert paths.concepts.read_text(encoding="utf-8").count("concept:") == 5

    assert cli.main(["number", "--root", root]) == 0
    assert cli.main(["check", "--root", root]) == 0
    assert cli.main(["publish", "--root", root]) == 0

    # 設計書は**工程ごとのフォルダ**に出る（11 種を 1 階層に並べない）。
    # 中身は HTML で見る ―― Markdown の原文では `T_ORDER` が `T\_ORDER` と
    # 逃がされている（`__init__` が `init` に化けるのを防ぐため。
    # → :data:`arp4.publish._MD_SPECIAL`）ので、**読み手に届く字**のほうを見る。
    # 番号は `layers` の並びで決まる ―― **パックに工程を足すと繰り下がる**
    # （3.11.0 で `企画` が入り `2_基本設計` → `3_基本設計`）。ここで見たいのは
    # 工程ごとに分かれて出ることなので、番号を期待値に焼き付けない。
    phase = f"{mm.load_pack('jp-sier-std')['layers'].index('基本設計') + 1}_基本設計"
    table = (paths.out / phase / "テーブル定義書.html").read_text(encoding="utf-8")
    assert "T_ORDER" in table and "ORDER_NO" in table and "受注番号" in table
    index = (paths.out / "目次.md").read_text(encoding="utf-8")
    assert f"{phase}/テーブル定義書.md" in index

    # 業務ルールの ID 列に内部 ID（rul-…）を出さない。**実装の規律にあたる
    # ルール（`rule_kind: processing`）は詳細設計書に出る** ―― 基本設計書と
    # 両方へ並べると同じ表が 2 つの工程で別々に承認される（→ basic-design.yml）。
    detail = f"{mm.load_pack('jp-sier-std')['layers'].index('詳細設計') + 1}_詳細設計"
    rules = (paths.out / detail / "詳細設計書.md").read_text(encoding="utf-8")
    assert "RUL-001" in rules and "rul-" not in rules
    basic = (paths.out / phase / "基本設計書.md").read_text(encoding="utf-8")
    assert "RUL-001" not in basic

    # 出典は**そのまま辿れる形**で出る（設計書の行 → パース結果 → 元資料）。
    assert "2026-08-02 資料/運用設計.xlsx/運用方針#s1-x1" in rules


def test_整理を欠くと凍結で止まる(通し) -> None:
    """**黙って落ちない。** 資料が 1 枚整理されていなければ凍結が通らない。"""
    paths, root = 通し
    (paths.round("2026-08-02").organized / "資料/基本設計書.xlsx/表紙.yml").unlink()

    assert cli.main(["freeze", "--root", root]) == 1
    assert not paths.round("2026-08-02").is_frozen()


def test_凍結していなければbuildしない(通し) -> None:
    paths, root = 通し
    assert cli.main(["build", "--root", root]) == 1
    assert cli.main(["build", "--root", root, "--force", "--dry-run"]) == 0


def test_出典が消えるとcheckが落ちる(通し) -> None:
    """凍結しているからこそ、**出典のリンクは機械で確かめられる。**"""
    paths, root = 通し
    assert cli.main(["freeze", "--root", root]) == 0
    assert cli.main(["build", "--root", root]) == 0

    (paths.round("2026-08-02").parsed / "資料/運用設計.xlsx/運用方針.md").unlink()
    assert cli.main(["check", "--root", root]) == 1


def test_Excelだけのラウンドでも決定記録が出る(通し) -> None:
    """**事後拒否権の入口は資産の種類で変わらない。**

    ``decisions.yml`` を書くのが ``draft`` と ``auto`` だけだったころ、Excel だけの
    資産はどちらも通らないので**決定ログが 1 件も無く、``決定記録.md`` が生成
    されなかった**（実測・sales-corpus r001）。そのラウンドでも機械は矛盾から
    課題を 28 件起こしている ―― 判断は下っているのに入口だけが無かった。
    """
    from arp4 import decisions

    paths, root = 通し
    write(paths.round("2026-08-02").organized / "_concepts.yml", """\
contradictions:
  - subject: c-受注データの保持期間
    name: 受注データの保持期間が資料間で食い違う
    positions:
      - { statement: 13 か月でアーカイブする }
      - { statement: 5 年間保存する }
""")
    assert cli.main(["freeze", "--root", root]) == 0
    assert cli.main(["build", "--root", root]) == 0

    said = decisions.load(paths.round("2026-08-02"))
    assert [e for e in said if e.get("by") == "build"]

    # **打ち直しても件数は増えない**（追記ではなく置き換え）。
    assert cli.main(["build", "--root", root]) == 0
    assert decisions.load(paths.round("2026-08-02")) == said

    assert cli.main(["number", "--root", root]) == 0
    assert cli.main(["publish", "--root", root]) == 0
    assert (paths.out / "決定記録.md").is_file()


def test_parseは作業中のラウンドを続ける(tmp_path: Path) -> None:
    """**日付でラウンドを切らない。** 同じ資料を別の日に処理し直しても続きになる。"""
    paths = paths_module.create(tmp_path)
    資料 = str(_make_sample(sources_dir(paths)))
    root = str(tmp_path)

    assert cli.main(["parse", "--root", root, 資料]) == 0
    assert cli.main(["parse", "--root", root, "--yes", 資料]) == 0
    assert [r.name for r in paths.rounds()] == ["r001"]

    # 資料が更新されたときだけ新しいラウンドを起こす。
    assert cli.main(["parse", "--root", root, "--new-round", 資料]) == 0
    assert [r.name for r in paths.rounds()] == ["r001", "r002"]


def test_一括の対象外宣言で未整理が減る(通し) -> None:
    paths, root = 通し
    round_ = paths.round("2026-08-02")
    (round_.organized / "資料/基本設計書.xlsx/表紙.yml").unlink()
    assert cli.main(["freeze", "--root", root]) == 1

    assert cli.main(["declare", "--root", root, "--round", "2026-08-02",
                     "表紙", "--reason", "表紙（仕様ではない）"]) == 0
    assert cli.main(["freeze", "--root", root]) == 0


def test_編集済みのパース結果は上書きしない(通し, monkeypatch) -> None:
    """**未編集のものは黙って上書きしてよい**が、編集済みは守って報告する。"""
    paths, root = 通し
    from arp4 import parse

    target = (paths.round("2026-08-02").parsed
              / "資料/運用設計.xlsx/運用方針.md")
    write(target, target.read_text(encoding="utf-8").replace("13 か月", "18 か月"))
    monkeypatch.setattr(parse, "_edited", lambda path, dirty: True)

    資料 = str(sources_dir(paths))
    assert cli.main(["parse", "--root", root, "--round", "2026-08-02", 資料]) == 0
    assert "18 か月" in target.read_text(encoding="utf-8")

    assert cli.main(["parse", "--root", root, "--round", "2026-08-02",
                     "--yes", 資料]) == 0
    assert "13 か月" in target.read_text(encoding="utf-8")



def test_生成物の帯はcheckと同じ件数を名乗る(通し) -> None:
    """**publish が知らない件数を刻まない。**

    表の形の指摘（``P1xx``）は長いあいだ ``check`` にしか無く、帯・``_gate.json``・
    ``0_この設計書の穴.md`` は publish が見ていない件数を名乗っていた ―― 実測
    （sales-corpus・r001）で ``check`` は warn 143 件、生成物は 124 件と名乗り、
    差の 19 件がまるごと表の形の指摘だった。**手順書は「穴の 1 枚に出る」と
    書いていた**ので、読み手は出ていないものを探しに行く。
    """
    import json

    from arp4 import audit as audit_module, spec as spec_module

    paths, root = 通し
    for step in ("freeze", "build", "number", "publish"):
        assert cli.main([step, "--root", root]) == 0

    spec, _ = spec_module.load(paths)
    record = json.loads((paths.out / "_gate.json").read_text(encoding="utf-8"))
    from collections import Counter
    expected = Counter(f.code for f in audit_module.audit(spec))

    # `audit` が言ったことは、綴りも件数もそのまま記録に載る。
    assert expected, "この検体では audit が何も言わない ―― 番人にならない"
    for code, count in expected.items():
        assert record["counts"].get(code) == count, (
            f"{code} が生成物の記録に届いていません: "
            f"audit {count} 件 / 記録 {record['counts'].get(code)}")

    # 帯の件数も同じものを名乗る。
    band = (paths.out / "目次.md").read_text(encoding="utf-8")
    assert f"warn {record['warns']} 件" in band


def test_帯が出すコードは穴の一覧で意味を持つ(通し) -> None:
    """件数だけ渡して意味を渡さない ―― `―` の行を出さない（→ `holes._CODES`）。"""
    import json
    import re as _re

    paths, root = 通し
    for step in ("freeze", "build", "number", "publish"):
        assert cli.main([step, "--root", root]) == 0

    record = json.loads((paths.out / "_gate.json").read_text(encoding="utf-8"))
    holes_text = (paths.out / "0_この設計書の穴.md").read_text(encoding="utf-8")
    for code in record["counts"]:
        row = _re.search(rf"^\| {code} \| \d+ \| (.+?) \|$", holes_text, _re.M)
        assert row and row.group(1).strip() not in ("―", ""), (
            f"{code} の「意味」が空です")


def test_1冊だけ出したら1冊ぶんの件数を刻む(通し) -> None:
    """**ゲートは「この生成物が通った条件」である。**

    束ぜんぶを検査すると、1 冊だけ出したときにその冒頭へ**出していない設計書の
    件数**が刻まれる ―― 母集合がずれると帯が読めなくなる。

    **「1 冊ぶん ≦ 束ぜんぶ」は `P1xx` 全部には成り立たない。** 以前はそう書いて
    いたが、成り立つのは**冊をまたいで比べる指摘**（``P106`` / ``P107``）だけで
    ある ―― ``P110`` / ``P111``（正本にあるのに、どの設計書の列にも出ない）は
    母集合を絞ると**増える。**出さなかった冊が拾っていた属性が、絞った瞬間に
    「どこにも出ない」に変わるためで、これは正しい振る舞いである。

    そのうえこの検体では、長いあいだ ``P107``（entity の出典列が基本設計書と
    テーブル定義書で揃っていない）が唯一の ``P1xx`` だった ―― **番人がパック側の
    不具合に寄りかかっていた**ので、直した時点でこの試験が落ちた。見る先を
    「その母集合でしか成り立たないこと」に置き直す。
    """
    import json

    paths, root = 通し
    for step in ("freeze", "build", "number"):
        assert cli.main([step, "--root", root]) == 0

    assert cli.main(["publish", "--root", root]) == 0
    everything = json.loads((paths.out / "_gate.json").read_text(encoding="utf-8"))

    assert cli.main(["publish", "--root", root, "table-spec"]) == 0
    one = json.loads((paths.out / "_gate.json").read_text(encoding="utf-8"))

    assert one["counts"] != everything["counts"], (
        "1 冊だけ出したのに束ぜんぶと同じ件数が刻まれています（母集合がずれている）")

    # 冊をまたいで比べる指摘は、1 冊だけの母集合では成り立ちようがない。
    assert not {c for c in one["counts"] if c in _CROSS_DOCUMENT}, (
        f"1 冊だけの記録に冊をまたぐ指摘が残っています: {one['counts']}")

    # 出稿の指摘（全行空で畳んだ列）は、出した冊のぶんしか刻まない。
    for code in ("W046", "W047"):
        assert one["counts"].get(code, 0) <= everything["counts"].get(code, 0), (
            f"{code} が 1 冊ぶんより多く刻まれています")


def test_stakeholderの記録がdeveloperの記録を上書きしない(通し) -> None:
    """**書き先は `out/stakeholder/` である（`out/` 直下ではない）。**

    stakeholder 向けの生成は表の形の指摘（``P1xx``）を回さない ―― 監査の母集合が
    developer の文書定義に紐づくためで、そこは正しい。問題は記録の**置き場**が
    直下だったことで、`publish` の直後に `publish --audience stakeholder` を打つと
    developer の `_gate.json` が **P1xx 抜きの件数で上書き**された。

    実測（sales-corpus・r001）で developer の帯と `0_この設計書の穴.md` が
    warn 132（P106 4・P107 4 を含む）と刻んだ直後に、記録だけが 124 になっていた
    ―― 手順書が「帯・`_gate.json`・穴の 1 枚は同じ件数になった」と書いている
    状態が、**成果物の上では嘘**になる。
    """
    import json

    paths, root = 通し
    for step in ("freeze", "build", "number"):
        assert cli.main([step, "--root", root]) == 0

    assert cli.main(["publish", "--root", root]) == 0
    before = json.loads((paths.out / "_gate.json").read_text(encoding="utf-8"))
    presentation = {c: n for c, n in before["counts"].items() if c.startswith("P")}
    assert presentation, "この検体では表の形の指摘が出ない ―― 番人にならない"

    assert cli.main(["publish", "--root", root, "--audience", "stakeholder"]) == 0

    after = json.loads((paths.out / "_gate.json").read_text(encoding="utf-8"))
    assert after == before, "developer の記録が stakeholder に上書きされました"

    # 読者別の記録は残る ―― **消すのではなく、別の置き場へ書く。**
    theirs = paths.out / "stakeholder" / "_gate.json"
    assert theirs.is_file(), "stakeholder 側の記録が残っていません"
    assert not {c for c in json.loads(theirs.read_text(encoding="utf-8"))["counts"]
                if c.startswith("P")}, "stakeholder は P1xx を回さない"


def test_配る見本が古くなっていない(tmp_path: Path) -> None:
    """**コミットしてある見本と、いま生成したものが同じか。**

    見本（`examples/*/資料/`）は git に入っている ―― Python を動かさずに開いて
    確かめられることに価値があるからで、そのぶん**中身が古いまま置き去りに
    なる**危険を負う。生成器を直したのに見本を作り直し忘れると、配っているのは
    もう存在しない形式である。

    バイト列で比べられるのは、生成を決定的にしてあるからである
    （`build/reproducible.py` ―― 時刻で毎回差分が出る状態では、この検査は
    最初から鳴りっぱなしで意味を持たない）。
    """
    spec = importlib.util.spec_from_file_location(
        "make_sample", _EXAMPLES / "make_sample.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)

    fresh = (module.build(tmp_path / "from-excel" / "資料")
             + module.build_documents(tmp_path / "from-documents" / "資料"))
    assert len(fresh) == 6

    stale: list[str] = []
    for made in fresh:
        committed = _EXAMPLES / made.relative_to(tmp_path)
        if not committed.is_file() or committed.read_bytes() != made.read_bytes():
            stale.append(committed.relative_to(_EXAMPLES).as_posix())
    assert not stale, ("見本が古くなっています ―― `python examples/make_sample.py` "
                       f"で作り直してコミットしてください: {stale}")
