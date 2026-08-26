"""出典を開く ―― **索引の逆関数**（`arp4 show`）。

検体が突くのは 2 つである ―― 設計書に出ている字**そのまま**で引けること、
そして開けなかったときに**間違いの在り処**（ラウンド・写し・アンカーのどれか）
を言い分けること。近いもので代えて開くことは決してしない。
"""

from __future__ import annotations

from pathlib import Path

from arp4 import mdio, show
from arp4.paths import Paths, Round
from arp4.spec import Spec
from arp4.metamodel import Metamodel
from conftest import parsed

_SHEET = """\
# 運用引継ぎ資料.xlsx / 3.SLO

<!-- source: 資料/運用引継ぎ資料.xlsx / シート: 3.SLO -->

> このシートには 図形 3 個 があり、2 個からテキストを取り出しました。

## 表 B2:E9  <!-- a:s5-t1 at=B2:E9 -->

| 指標 | 目標 |
|---|---|
| 検索の応答時間 | p95 で 800ms 以内 |

## セル B12  <!-- a:s5-x1 at=B12 -->

- `B12` ※ 対象は平日 8:00-20:00。
"""


def _sheet(round_: Round) -> Path:
    return parsed(round_, "資料/運用引継ぎ資料.xlsx/3.SLO.md", _SHEET)


# ── 出典セルの字を読む ──────────────────────────────────────────
def test_出典セルの字をそのまま読む(project: Paths) -> None:
    found, folded = show.references(
        "r001 資料/運用引継ぎ資料.xlsx/3.SLO#s5-t1", project)

    assert folded == 0
    assert found == [show.Reference(file="資料/運用引継ぎ資料.xlsx/3.SLO",
                                    anchor="s5-t1", round="r001")]
    assert str(found[0]) == "r001 資料/運用引継ぎ資料.xlsx/3.SLO#s5-t1"


def test_升目を丸ごと貼れる(project: Paths) -> None:
    """**読み手が持っているのは 1 件ではなく表の升目 1 つぶん**である。"""
    found, folded = show.references(
        "r001 資料/A.xlsx/受注#s1-t1 / r001 資料/B.xlsx/画面#s2-t1 / ほか 3 件",
        project)

    assert [r.file for r in found] == ["資料/A.xlsx/受注", "資料/B.xlsx/画面"]
    assert folded == 3          # 畳まれた件数は落とさない（開く先が無いだけ）


def test_ファイル名の空白をラウンドと取り違えない(project: Paths) -> None:
    """``資料/A/基本設計書 v2.xlsx`` は実在する書き方である。

    先頭の語をいつでもラウンドとして切り落とすと、**開ける資料が開けなくなる。**
    """
    found, _ = show.references("資料/基本設計書 v2.xlsx/受注#s1-t1", project)

    assert found[0].round == ""
    assert found[0].file == "資料/基本設計書 v2.xlsx/受注"


# ── 開く ────────────────────────────────────────────────────────
def test_塊だけを出しても何の資料か分かる(project: Paths) -> None:
    """``source`` と申告は**ファイルの頭にしか無い**ので、切り出す側が付け直す。"""
    round_ = project.round("r001")
    _sheet(round_)

    result = show.open_anchor(project, show.Reference(
        file="資料/運用引継ぎ資料.xlsx/3.SLO", anchor="s5-t1", round="r001"))

    assert isinstance(result, show.Opened)
    assert result.source == "資料/運用引継ぎ資料.xlsx / シート: 3.SLO"
    assert result.notes == ["このシートには 図形 3 個 があり、2 個からテキストを取り出しました。"]
    assert "p95 で 800ms 以内" in result.anchor.body

    shown = show.render(result, project.root)
    assert shown[1] == "  .arp/rounds/r001/parsed/資料/運用引継ぎ資料.xlsx/3.SLO.md"


def test_申告を落とすと読めていないことが伝わらない(project: Paths) -> None:
    """**「資料に無い」と「機械が読めていない」が塊だけからは区別できない。**"""
    round_ = project.round("r001")
    _sheet(round_)

    result = show.open_anchor(project, show.Reference(
        file="資料/運用引継ぎ資料.xlsx/3.SLO", anchor="s5-x1", round="r001"))

    assert any("2 個からテキスト" in line for line in show.render(result))


def test_アンカーを省くと写しの塊一覧になる(project: Paths) -> None:
    round_ = project.round("r001")
    _sheet(round_)

    result = show.open_anchor(project, show.Reference(
        file="資料/運用引継ぎ資料.xlsx/3.SLO", round="r001"))

    assert isinstance(result, show.Listing)
    assert [a.id for a in result.anchors] == ["s5-t1", "s5-x1"]
    assert any("表 B2:E9" in line for line in show.render(result))


def test_ラウンドを省くと持っているいちばん新しいラウンド(project: Paths) -> None:
    _sheet(project.round("r001"))
    _sheet(project.round("r002"))

    result = show.open_anchor(project, show.Reference(
        file="資料/運用引継ぎ資料.xlsx/3.SLO", anchor="s5-t1"))

    assert isinstance(result, show.Opened)
    assert result.reference.round == "r002"     # どれを開いたかは必ず名乗る


def test_前後の塊も出せる(project: Paths) -> None:
    round_ = project.round("r001")
    _sheet(round_)

    result = show.open_anchor(project, show.Reference(
        file="資料/運用引継ぎ資料.xlsx/3.SLO", anchor="s5-t1", round="r001"),
        around=1)

    assert isinstance(result, show.Opened)
    assert [a.id for a in result.around] == ["s5-x1"]


# ── 開けなかったとき ────────────────────────────────────────────
def test_ラウンドと写しとアンカーを言い分ける(project: Paths) -> None:
    """**間違いの在り処を取り違えた案内は、案内しないより悪い。**"""
    round_ = project.round("r001")
    _sheet(round_)
    here = "資料/運用引継ぎ資料.xlsx/3.SLO"

    no_round = show.open_anchor(project, show.Reference(
        file=here, anchor="s5-t1", round="r009"))
    no_file = show.open_anchor(project, show.Reference(
        file="資料/運用引継資料.xlsx/3.SLO", anchor="s5-t1", round="r001"))
    no_anchor = show.open_anchor(project, show.Reference(
        file=here, anchor="s9-t9", round="r001"))

    assert "ラウンド r009 がありません" in no_round.reason
    assert "その写しがありません" in no_file.reason
    assert "s9-t9" in no_anchor.reason
    # 近い写しは**出すが、代わりに開かない**（開いたら出典が偽になる）
    assert any(here in hint for hint in no_file.hints)
    assert any("s5-t1" in hint for hint in no_anchor.hints)


def test_無いアンカーを近いもので代えない(project: Paths) -> None:
    _sheet(project.round("r001"))

    result = show.open_anchor(project, show.Reference(
        file="資料/運用引継資料.xlsx/3.SLO", anchor="s5-t1", round="r001"))

    assert isinstance(result, show.Missing)     # 近い写しがあっても開かない


# ── 表示 ID から引く ────────────────────────────────────────────
def _spec(model: Metamodel) -> Spec:
    return Spec(metamodel=model, relations=[], items=[{
        "id": "req-1", "type": "requirement", "req_id": "NFR-002",
        "name": "エンベディングの応答時間",
        "source": [{"round": "r001", "file": "資料/運用引継ぎ資料.xlsx/3.SLO",
                    "anchor": "s5-t1"},
                   {"round": "r001", "file": "資料/運用引継ぎ資料.xlsx/3.SLO",
                    "anchor": "s5-x1"}]}])


def test_表示IDで引くと出典が畳まれない(model: Metamodel) -> None:
    """設計書の升目は 2 件で切れる（``_SOURCE_LIMIT``）。**正本には全件ある。**"""
    found = show.by_key(_spec(model), "NFR-002")

    assert found is not None
    assert found.display == "NFR-002" and found.id == "req-1"
    assert [r.anchor for r in found.references] == ["s5-t1", "s5-x1"]


def test_内部IDでも引ける(model: Metamodel) -> None:
    assert show.by_key(_spec(model), "req-1") is not None
    assert show.by_key(_spec(model), "NFR-999") is None


# ── 束を横断する（索引と検索） ──────────────────────────────────
_OTHER = """\
# 利用申請台帳.xlsx / 1.申請一覧

<!-- source: 資料/利用申請台帳.xlsx / シート: 1.申請一覧 -->

## 表 B2:D4  <!-- a:s3-t1 at=B2:D4 -->

| テナント | 区分 | 上限 |
|---|---|---|
| legal-contract | 極秘（第3.2版） | 100,000 |
"""


def _two(project: Paths) -> Round:
    round_ = project.round("r001")
    _sheet(round_)
    parsed(round_, "資料/利用申請台帳.xlsx/1.申請一覧.md", _OTHER)
    return round_


def test_索引は塊を全部並べる(project: Paths) -> None:
    """出す欄は**そのまま貼れる出典**である（索引の値打ちはそこにしかない）。"""
    _two(project)

    entries = show.catalogue(project)

    assert [str(e.reference) for e in entries] == [
        "r001 資料/利用申請台帳.xlsx/1.申請一覧#s3-t1",
        "r001 資料/運用引継ぎ資料.xlsx/3.SLO#s5-t1",
        "r001 資料/運用引継ぎ資料.xlsx/3.SLO#s5-x1"]
    assert entries[0].heading == "表 B2:D4" and entries[0].at == "B2:D4"


def test_撮り直したラウンドがあっても束は欠けない(project: Paths) -> None:
    """**部分的な撮り直しの直後に束のほとんどが消える**のがいちばん怖い。

    いちばん新しいラウンドだけを見ると、3 冊しか無い `r002` が「全部」になる。
    """
    _two(project)
    _sheet(project.round("r002"))            # 1 本だけ撮り直した

    corpus = [(r.name, f) for r, f in show.corpus(project)]

    assert corpus == [("r001", "資料/利用申請台帳.xlsx/1.申請一覧"),
                      ("r002", "資料/運用引継ぎ資料.xlsx/3.SLO")]


def test_当たりは塊に帰属して返る(project: Paths) -> None:
    """**素の grep との違いはそこ 1 点**である ―― そのまま出典として書ける。"""
    _two(project)

    hits = show.search(project, "極秘")

    assert len(hits) == 1
    assert str(hits[0].reference) == "r001 資料/利用申請台帳.xlsx/1.申請一覧#s3-t1"
    assert hits[0].heading == "表 B2:D4"
    assert "legal-contract" in hits[0].lines[0]


def test_探すのは既定で部分一致(project: Paths) -> None:
    """資料の語は ``（第3.2版）`` のように正規表現の記号を普通に含む。"""
    _two(project)

    assert show.search(project, "極秘（第3.2版）")           # 記号のまま当たる
    assert not show.search(project, "極秘（第3x2版）", regex=True)
    assert show.search(project, "極秘（第3.2版）", regex=True)  # . が 1 字に当たる


def test_申告とOCRの読みも探せる(project: Paths) -> None:
    """**画像の中の字も grep に入る**ことが、`o1` を出している理由の 1 つである。"""
    round_ = project.round("r001")
    parsed(round_, "資料/A.xlsx/画面.md",
           "# A.xlsx / 画面\n\n<!-- source: 資料/A.xlsx / シート: 画面 -->\n\n"
           "## 画像の中の文字（読み違えが混ざります）  <!-- a:s1-o1 at=画像 1 枚 -->\n\n"
           "- 受注番号 ORDER-OOI\n")

    hits = show.search(project, "ORDER-OOI")

    assert [str(h.reference) for h in hits] == ["r001 資料/A.xlsx/画面#s1-o1"]


def test_当たった行の数は畳んでも失わない(project: Paths) -> None:
    round_ = project.round("r001")
    parsed(round_, "資料/A.xlsx/多.md",
           "# A.xlsx / 多\n\n## 表  <!-- a:s1-t1 at=B2:B9 -->\n\n"
           + "\n".join(f"| 受注 {i} |" for i in range(8)) + "\n")

    hits = show.search(project, "受注", per_hit=2)

    assert len(hits[0].lines) == 2 and hits[0].total == 8


def test_読めない正規表現は言う(project: Paths) -> None:
    _two(project)

    try:
        show.search(project, "[", regex=True)
    except ValueError as exc:
        assert "正規表現として読めません" in str(exc)
    else:                                     # pragma: no cover
        raise AssertionError("黙って 0 件にしない")


# ── 逆向き ―― この塊は何になったか ──────────────────────────────
def test_塊から正本のレコードを引ける(model: Metamodel) -> None:
    """索引として往復するとき、**先に知りたいのは「もう拾われているか」**である。"""
    spec = _spec(model)
    here = show.Reference(file="資料/運用引継ぎ資料.xlsx/3.SLO", anchor="s5-t1")

    uses = show.used_by(spec, here)

    assert [u.display for u in uses] == ["NFR-002"]
    assert uses[0].type == "requirement" and uses[0].id == "req-1"


def test_ラウンドが違っても使われている(model: Metamodel) -> None:
    """撮り直した写しを開いているとき、**出典が前のラウンドなのは普通**である。

    そこで「使われていない」と言うと嘘になる。
    """
    spec = _spec(model)                       # 出典は r001
    here = show.Reference(file="資料/運用引継ぎ資料.xlsx/3.SLO",
                          anchor="s5-t1", round="r002")

    assert show.used_by(spec, here)


def test_使われていない塊は0件で黙らない(model: Metamodel) -> None:
    """**「使われていない」は「不要だった」ではない**（表紙・改訂履歴）。"""
    spec = _spec(model)
    here = show.Reference(file="資料/運用引継ぎ資料.xlsx/3.SLO", anchor="s9-t9")

    assert show.used_by(spec, here) == []
    assert any("まだ整理されていない" in line
               for line in show.render_uses([]))


def test_シート名のシャープで写しを取り違えない(project: Paths) -> None:
    """``safe_name`` は ``#`` を落とさない ―― **``受注#1`` は実在する写しの名前**。

    最初の ``#`` で切ると写しの名前が ``受注`` になり、リンクも `arp4 show` も
    その 1 本だけ外す。組み立て側はアンカーを末尾に足しているので、末尾から割る。
    """
    assert show.split("r001 資料/A.xlsx/受注#1#s1-t1") == (
        "r001", "資料/A.xlsx/受注#1", "s1-t1")
    assert show.split("資料/A.xlsx/受注") == ("", "資料/A.xlsx/受注", "")

    found, _ = show.references("r001 資料/A.xlsx/受注#1#s1-t1", project)
    assert found[0].file == "資料/A.xlsx/受注#1" and found[0].anchor == "s1-t1"
