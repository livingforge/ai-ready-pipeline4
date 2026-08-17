"""束としての通し ―― **1 案件ぶんの資料 6 冊が、正本と設計書まで 1 本で通るか。**

:mod:`test_example` の通しは**配る見本**（Excel 2 冊／文書 4 冊）を起点にする。
あちらは「形式ごとのアンカーが凍結・正本・出典の照合・設計書の出典欄まで
そのままの形で流れるか」を見るもので、資料の量は最小である。

ここが見るのはその先で、**1 案件ぶんの束でしか出ないもの**である。

* **語彙が閉じない。** 資料が 1 冊なら、書いてあるのはたいてい 1 つの工程の
  ことである。実案件の束は企画から管理までを跨ぐので、**メタモデルの
  32 種別と 36 関係が同時に立つ** ―― `build` も `publish` も、そこを通った
  ことが無かった（見本の通しで立つのは 16 種別である）
* **同じことが 3 冊に書いてある。** しかも字が違う（取引先／得意先／顧客）
* **食い違いが冊をまたぐ。** 方式設計の「2 秒以内」と機能仕様書の「3 秒」は、
  どちらの冊でも矛盾していない ―― `_concepts.yml` の `contradictions` が
  課題（`open-issue`）になる経路は、**文書だけのラウンドでは 1 度も
  通っていなかった**

検体は :mod:`dataset`（`tests/dataset/難読.yml`）、整理結果は
`tests/organized/難読/` にファイルとして置いてある ―― 整理層はエージェントの
仕事なので、ここでは**人が書いたつもりの整理結果**を置く。**この試験のもう
1 つの目的は、その形が実際に書けるものかどうかを確かめることである。**

整理結果をテストの中の辞書ではなくファイルにしてあるのは量のためである
（42 本・1,000 行を超える）―― 実物の `organized/` と同じ形で置いてあるので、
そのまま開いて読める。
"""

from __future__ import annotations

import json
import shutil
import sys
from pathlib import Path

import pytest

from arp4 import cli, paths as paths_module
from conftest import sources_dir

sys.path.insert(0, str(Path(__file__).parent))
import dataset                                        # noqa: E402

#: 検体の置き場の接頭辞。**1 案件ぶんがここに揃っている。**
CORPUS = "資料/R/"

#: 人が書いたつもりの整理結果（`organized/` と同じ形）。
ORGANIZED = Path(__file__).with_name("organized") / "難読"

#: 立つはずのアイテム種別。**メタモデルの全部**である ―― 1 つでも落ちたら、
#: その種別は `build` も `publish` も通っていない。
ITEMS = (
    # 企画
    "project-overview", "business-goal", "current-issue", "risk", "milestone",
    "cost-item",
    # 要件定義
    "business-flow", "flow-step", "requirement", "actor", "constraint",
    "glossary-term",
    # 基本設計
    "screen", "report", "batch", "external-interface", "command", "entity",
    "data-item", "index", "code-master", "code-value", "business-rule",
    "message",
    # 詳細設計
    "module", "method", "batch-step", "process-step",
    # テスト・管理
    "test-case", "test-run", "open-issue", "decision",
)

#: 立つはずの関係。**36 種すべて**である。
RELATIONS = (
    "realizes", "refines", "contributes-to", "addresses", "affects",
    "threatens", "supports", "verifies", "executes", "finds", "uses-specimen",
    "has-column", "has-value", "references", "has-index", "has-method",
    "has-step", "has-flow-step", "leads-to", "has-process-step", "proceeds-to",
    "reports-to", "outsources-to", "displays", "accesses", "uses-code",
    "raises", "operates", "triggers", "calls", "transitions", "interfaces",
    "constrains", "disputes", "resolves", "establishes",
)

#: 形式ごとの出典。**設計書の出典欄にこの形で出る。**接頭辞が形式ごとに違う
#: （`s` スライド／`w` 節／`p` ページ／接頭辞なしの CSV）ので、1 つでも欠ければ
#: どの形式の経路が切れているかがそのまま分かる。
SOURCES = (
    "資料/R/請求・入金管理システム方式設計（第4.2版）.pptx/02_事業目標と効果指標#s2-t1",
    "資料/R/請求・入金管理システム方式設計（第4.2版）.pptx/08_コード体系と採番規則#s8-t1",
    # **1 枚のスライドに表が 2 枚**あるとき、出典はどちらの表かまで指す。
    "資料/R/請求・入金管理システム方式設計（第4.2版）.pptx/08_コード体系と採番規則#s8-t2",
    "資料/R/請求・入金機能仕様書（第2.3版）.docx/03_2 業務フロー#w3-t1",
    # **見出し 2 が 4 つ並ぶ節**でも、表ごとに別の出典になる。
    "資料/R/請求・入金機能仕様書（第2.3版）.docx/06_5 画面#w6-t1",
    "資料/R/請求・入金機能仕様書（第2.3版）.docx/06_5 画面#w6-t4",
    "資料/R/請求・入金機能仕様書（第2.3版）.docx/13_12 モジュール構成#w13-t1",
    # **本文の段落**（表ではない）。
    "資料/R/請求・入金機能仕様書（第2.3版）.docx/04_3 機能要件#w4-h1",
    # PDF は**子しおりでは割らない**ので、2.1 の中身は 3 節目のページを指す。
    "資料/R/請求・入金管理システム総合試験報告書（第1.1版）.pdf/03_2 試験実施結果#p5-x1",
    "資料/R/請求・入金管理システム総合試験報告書（第1.1版）.pdf/06_5 受入判定#p14-x1",
    # CSV は 1 ファイルが 1 本なので接頭辞が無い。
    "資料/R/入金消込対象_移行.csv#t1",
    "資料/R/請求区分コード.tsv#t1",
    # **関係の表にしか出てこない種別**（メソッド・バッチステップ・処理ステップ・
    # テスト結果）と、出典列を落としていた課題一覧。この束で通すまで、**正本には
    # 出典があるのに生成物のどの行からも元資料へ辿れなかった** ―― 同じ種別が
    # 2 冊に出るわけではないので `P107` も鳴らず、`P110` / `P111` は関係と属性の
    # 話なので、この形の欠落を言う指摘が 1 つも無かった（→ 決定 107）。
    "資料/R/請求・入金機能仕様書（第2.3版）.docx/13_12 モジュール構成#w13-t2",
    "資料/R/請求・入金機能仕様書（第2.3版）.docx/13_12 モジュール構成#w13-t3",
    "資料/R/請求・入金機能仕様書（第2.3版）.docx/11_10 バッチ処理#w11-t3",
    "資料/R/請求・入金機能仕様書（第2.3版）.docx/15_14 課題と決定事項#w15-t1",
    "資料/R/請求・入金管理システム総合試験報告書（第1.1版）.pdf/04_3 不具合と是正#p11-x1",
    "資料/R/請求・入金管理システム総合試験報告書（第1.1版）.pdf/04_3 不具合と是正#p12-x1",
)


@pytest.fixture(scope="module")
def 通し_束(tmp_path_factory: pytest.TempPathFactory):
    """検体 6 冊を組み、整理結果を置いて、設計書まで通した 1 ラウンド。

    **module スコープにしてある。** parse から publish までで 40 秒近く掛かる
    ので、観点ごとに組み直すと試験の時間が観点の数だけ伸びる ―― 見ているのは
    どれも「1 度通したものの中身」なので、組むのは 1 回でよい。
    """
    root = tmp_path_factory.mktemp("難読")
    paths = paths_module.create(root)
    資料 = sources_dir(paths)
    dataset.build(資料.parent, only=[spec["置き場"] for spec in dataset.specs()
                                     if spec["置き場"].startswith(CORPUS)])

    assert cli.main(["parse", "--root", str(root), "--round", "r001",
                     str(資料)]) == 0
    shutil.copytree(ORGANIZED, paths.round("r001").organized, dirs_exist_ok=True)
    return paths, str(root)


def test_資料6冊が設計書まで通る(通し_束) -> None:
    """**凍結 → 正本 → 採番 → 検証 → 設計書が 1 本で通るか。**

    `check` が error 0 で返るところまでを 1 本にしてある ―― 途中で止まる
    ラウンドでは、その先の観点（種別の網羅・出典・矛盾）が**全部まとめて
    空振りする**ので、どれが原因かを分けても読み手の役に立たない。
    """
    _paths, root = 通し_束
    for step in ("freeze", "build", "number", "check", "publish"):
        assert cli.main([step, "--root", root]) == 0, f"{step} で止まりました"


def test_メタモデルの全種別が正本に立つ(通し_束) -> None:
    """**32 種別ぜんぶ。** 1 つでも欠けたら、そこは通しで 1 度も通っていない。

    見本の通し（:mod:`test_example`）で立つのは 16 種別で、企画の段
    （事業目標・現状課題・リスク・マイルストーン・費用）と詳細設計の段
    （モジュール・メソッド・処理ステップ）と索引・帳票・コマンドは、
    **資料の側にその表が無いので立てようがなかった。**
    """
    paths, _root = 通し_束
    欠け = [kind for kind in ITEMS if not (paths.items / f"{kind}.yml").is_file()]
    assert not 欠け, f"この種別が正本に立っていません: {欠け}"


def test_メタモデルの全関係が正本に立つ(通し_束) -> None:
    """**36 関係ぜんぶ。**

    関係は種別より落ちやすい ―― 種別は 1 レコード書けば立つが、関係は
    **両端の種別が揃って初めて**立つ。`uses-specimen`（テストケース →
    モジュール）は、テスト資産と実装資産の両方が同じラウンドに無ければ
    立てようがない。
    """
    paths, _root = 通し_束
    欠け = [rel for rel in RELATIONS
            if not (paths.relations / f"{rel}.yml").is_file()]
    assert not 欠け, f"この関係が正本に立っていません: {欠け}"


def test_6工程ぜんぶに設計書が出る(通し_束) -> None:
    """**束の帯も工程ごとのフォルダ分けも、1 つの段で閉じているうちは試されない。**"""
    paths, _root = 通し_束
    layers = {path.parent.name for path in paths.out.rglob("*.md")
              if path.parent != paths.out}
    assert len(layers) == 6, f"工程が {sorted(layers)} しかありません"
    索引 = (paths.out / "目次.md").read_text(encoding="utf-8")
    for 名前 in ("プロジェクト計画書", "要件定義書", "テーブル定義書",
                 "詳細設計書", "テスト結果報告書", "トレーサビリティ"):
        assert 名前 in 索引, f"{名前} が目次にありません"


def test_形式ごとのアンカーが出典欄まで届く(通し_束) -> None:
    """**接頭辞は形式ごとに違う**（`s` / `w` / `p` と、CSV の接頭辞なし）。

    見るのは HTML である ―― Markdown の原文では `03_2 業務フロー` が
    `03\\_2 業務フロー` と逃がされている（`__init__` が `init` に化けるのを
    防ぐため）ので、**読み手に届く字**のほうで確かめる。
    """
    paths, _root = 通し_束
    published = "\n".join(path.read_text(encoding="utf-8")
                          for path in paths.out.rglob("*.html"))
    for source in SOURCES:
        assert f"r001 {source}" in published, f"{source} が設計書の出典に出ていません"


def test_出典を持つ写しは1枚残らず設計書に届く(通し_束) -> None:
    """**母集合のほうを見る。** 挙げたものが届くかではなく、**取りこぼしが無いか。**

    :data:`SOURCES` は代表を並べた一覧なので、**そこに挙げ忘れた形式や種別は
    最初から検査されない** ―― 実際、関係の表にしか出てこない種別（メソッド・
    バッチステップ・処理ステップ・テスト結果）は、様式に出典列が無いせいで
    1 度も届いていなかったのに、期待値の側にも挙がっていなかったので誰も
    気づかなかった。

    ここは整理結果が出典に挙げたアンカーを**全部**集めて突き合わせる。
    `out_of_scope` は対象外の宣言なので数えない ―― 届かないのが正しい。
    """
    paths, _root = 通し_束
    round_ = paths.round("r001")
    published = "\n".join(path.read_text(encoding="utf-8")
                          for path in paths.out.rglob("*.html"))

    使った: set[str] = set()
    for path in round_.organized.rglob("*.yml"):
        if path.name.startswith("_") or path.name.startswith("."):
            continue
        写し = path.relative_to(round_.organized).with_suffix("").as_posix()
        import yaml                                    # noqa: PLC0415

        body = yaml.safe_load(path.read_text(encoding="utf-8")) or {}
        for record in body.get("records") or []:
            anchor = (record.get("source") or {}).get("anchor")
            if anchor:
                使った.add(f"{写し}#{anchor}")

    届かない = sorted(one for one in 使った if f"r001 {one}" not in published)
    assert not 届かない, (
        "整理結果が出典に挙げたのに、どの設計書からも辿れない写しがあります "
        f"（{len(届かない)} 件）: {届かない[:5]}")


def test_冊をまたぐ食い違いが課題になる(通し_束) -> None:
    """**どちらの冊でも矛盾していない。** 1 冊ずつ読んでいるうちは見つからない。

    `_concepts.yml` の `contradictions` を課題（`open-issue`）に起こす経路は、
    Excel のラウンド（:mod:`test_example`）でしか通っていなかった ―― 文書
    だけの束でも同じように通ることを、両論の字まで見て確かめる。
    """
    paths, _root = 通し_束
    課題 = (paths.out / "6_管理" / "課題管理表.md").read_text(encoding="utf-8")
    for 争点 in ("請求データの保持期間が資料間で食い違う",
                 "請求書発行の応答時間の目標値が資料間で食い違う"):
        assert 争点 in 課題, f"{争点} が課題管理表に出ていません"
    # **両論をそのまま残す。** どちらかを消すと、消したという事実ごと残らない。
    assert "請求データは 7 年保持する" in 課題
    assert "保持期間は 5 年とする" in 課題


def test_調べたうえで相手がいないものは理由つきで残る(通し_束) -> None:
    """`known_gaps` → `W032`。**「調べた」と「まだ調べていない」を分ける。**

    帳票の出力項目の表が資料に 1 枚も無いことと、まだ整理していないことは、
    生成物の上ではどちらも空欄にしか見えない ―― 理由が付いて出続けることが、
    その 2 つを分ける唯一の手立てである。
    """
    paths, _root = 通し_束
    report = json.loads((paths.out / "findings.json").read_text(encoding="utf-8"))
    gaps = [one for one in report["findings"] if one["code"] == "W032"]
    assert gaps, "known_gaps が 1 件も W032 として出ていません"
    穴 = (paths.out / "0_この設計書の穴.md").read_text(encoding="utf-8")
    assert "W032" in 穴


def test_使われなかった資料が名指しで出る(通し_束) -> None:
    """**反対側の 1 枚。** 届いた資料のどれが設計書に 1 度も出ていないか。

    この束では方式設計の構成図・業務フロー・移行方式の比較を対象外にして
    あるので、そこが名指しで出る ―― 資料を渡した側が最初に知りたいのは
    こちらである。
    """
    paths, _root = 通し_束
    対応 = (paths.out / "0_元資料と設計書の対応.md").read_text(encoding="utf-8")
    assert "バッチ運用一覧.csv" in 対応
