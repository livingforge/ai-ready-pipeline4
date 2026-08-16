"""正本の機械検証 ―― **error はデータの不備**であって、緩めて通すものではない。

ここで見るのは ``known_gaps``（「資料に定義が無い」ことの表明）の扱いである。
資料が揃うまで ``publish --force`` を打ち続けるのが常態化すると、``--force`` が
本物の error まで押し流す。
"""

from __future__ import annotations

from typing import Any

from arp4 import metamodel as mm
from arp4.spec import Spec
from arp4.validate import validate
from conftest import codes


def _spec(model: mm.Metamodel, known_gaps: Any = None) -> Spec:
    """列定義シートが無いテーブル 1 本だけの正本（``has-column`` が 0 本）。"""
    entity: dict[str, Any] = {
        "id": "ent-1", "type": "entity", "status": "review",
        "name": "得意先別単価", "statement": "得意先別単価マスタを保持すること",
        "physical_name": "M_PRICE", "entity_kind": "マスタ",
    }
    if known_gaps is not None:
        entity["known_gaps"] = known_gaps
    return Spec(metamodel=model, items=[entity], relations=[])


def test_定義が無ければE027(model: mm.Metamodel) -> None:
    """検出そのものは正しい。**黙って通してはいけない。**"""
    findings = validate(_spec(model))

    assert "E027" in codes(findings)


def test_known_gapsに載っていればwarnへ落ちる(model: mm.Metamodel) -> None:
    """``publish`` が通るようになるが、**理由つきで出続ける**（消さない）。"""
    findings = validate(_spec(model, {
        "has-column": {"reason": "テーブル定義書に列定義シートが無い。先方へ依頼済み",
                       "at": "2026-08-03"}}))

    assert "E027" not in codes(findings)
    gap = [f for f in findings if f.code == "W032"]
    assert len(gap) == 1 and "先方へ依頼済み" in gap[0].message
    assert gap[0].level == "warn"


def test_理由の無いknown_gapsはE019(model: mm.Metamodel) -> None:
    """``overridden`` と同じ ―― 空を許すと E027 を黙らせるだけのキーになる。"""
    findings = validate(_spec(model, {"has-column": {"at": "2026-08-03"}}))

    assert "E019" in codes(findings)
    assert "E027" in codes(findings)          # 効いていないので多重度は error のまま
    assert "W033" not in codes(findings)      # 打ち手は「理由を書く」の 1 つだけ


def test_誤字のknown_gapsはE018(model: mm.Metamodel) -> None:
    """守っているつもりで守られていない状態を作らない。"""
    findings = validate(_spec(model, {"has-colum": {"reason": "誤字"}}))

    assert "E018" in codes(findings)
    assert "E027" in codes(findings)          # 誤字なので効いていない


def test_違反が無くなったknown_gapsはW033(model: mm.Metamodel) -> None:
    """古い言い訳が正本に残り続けるほうが、error より始末が悪い。"""
    spec = _spec(model, {"has-column": {"reason": "列定義シートが無い"}})
    spec.items.append({"id": "itm-1", "type": "data-item", "status": "review",
                       "name": "単価", "statement": "単価は数値であること",
                       "data_type": "数値"})
    spec.relations.append({"type": "has-column", "from": "ent-1", "to": "itm-1",
                           "status": "review", "physical_name": "PRICE"})
    findings = validate(spec)

    assert "W032" not in codes(findings)
    assert "W033" in codes(findings)


def test_crudが無くてもaccessesを張れる(model: mm.Metamodel) -> None:
    """**真実である依存を捨てさせない。**

    `crud` を必須にしていたあいだ、資料が「触る」ことしか言っていないとき
    （コードの `import`）に整理層が選べたのは**推測して埋めるか、関係そのものを
    捨てるか**しかなく、実測では後者になった ―― 自身のソース 23 本で
    `accesses` 0 本、`entity` 33 件が全部 `W030`（どこからも参照されない）。

    捨てた依存は誰にも見えないが、空欄は空欄として見える（`operates.permission`
    に 3.2.0 で当てたのと同じ判断）。
    """
    spec = Spec(metamodel=model, items=[
        {"id": "mod-1", "type": "module", "status": "review",
         "name": "整理結果", "statement": "organized は整理結果を読むこと",
         "module_id": "MOD-001"},
        {"id": "ent-1", "type": "entity", "status": "review",
         "name": "レコード", "statement": "Record は整理結果 1 件を保持すること",
         "physical_name": "RECORD", "entity_kind": "マスタ"},
    ], relations=[
        {"type": "accesses", "from": "mod-1", "to": "ent-1", "status": "review"},
    ])
    findings = validate(spec)

    assert "E010" not in codes(findings)
    assert "W030" not in codes(findings)      # 参照されているので出ない


def test_カバレッジ欠落は名前で言う(model: mm.Metamodel) -> None:
    """`W030` が 33 件並んだとき、**どれを直せばよいかを決められること。**

    宛先が `ent-037a3625a979` だけだと正本を grep するしかない ―― 同じ状況で
    `publish` は畳んだ行・列を名前つきで並べているので、検証側だけが不親切だった。
    """
    findings = validate(_spec(model))
    said = [f for f in findings if f.code == "W030"]

    assert len(said) == 1
    assert said[0].target == "ent-1（得意先別単価）"


def test_同じシグネチャのメソッドが並んでもE012にしない(model: mm.Metamodel) -> None:
    """**シグネチャはモジュールの中でしか一意でない。**

    `render()` も `as_dict()` も別クラスに普通に並ぶ（実測では 26 ファイルで
    6 種・13 件が重複した）。ここを一意にすると、整理層はパース結果の値を
    `mdio.render()` と修飾して回避するしかなくなり、「シグネチャはそのまま
    呼び出しに写せる形で出す・書き換えない」を破る。同一性は `method_id` が持つ。
    """
    spec = Spec(metamodel=model, items=[
        {"id": "mtd-1", "type": "method", "status": "review",
         "name": "render", "statement": "Tile を絵にすること",
         "method_id": "MTD-0001", "signature": "render()"},
        {"id": "mtd-2", "type": "method", "status": "review",
         "name": "render", "statement": "Block を Markdown にすること",
         "method_id": "MTD-0002", "signature": "render()"},
    ], relations=[])

    assert "E012" not in codes(validate(spec))


def _constrained(model: mm.Metamodel, tied: bool) -> Spec:
    """制約 1 件とモジュール 1 本。``tied`` なら ``constrains`` で繋ぐ。"""
    return Spec(metamodel=model, items=[
        {"id": "cst-1", "type": "constraint", "status": "review",
         "name": "画像化に要る環境",
         "statement": "シートの画像化には Windows の Excel が要ること",
         "constraint_id": "CST-001", "category": "技術"},
        {"id": "mod-1", "type": "module", "status": "review",
         "name": "arp4.render", "statement": "シートを絵にすること",
         "module_id": "MOD-001"},
    ], relations=[{"type": "constrains", "from": "cst-1", "to": "mod-1",
                   "status": "review"}] if tied else [])


def test_何も制約していない制約はW031(model: mm.Metamodel) -> None:
    """**守られているかを誰も確かめられない制約**を、要件定義書に出す前に言う。

    `requirement` / `business-flow` / `flow-step` には旗が付いていたのに制約だけ
    付け忘れられていて、arp4 自身を通した 1 ラウンドでは**制約 32 件が全件
    `constrains` 0 本**のまま error 0 / warn 0 で通っていた。制約は要件定義書に
    行として出るので、**繋がっていないことが読み手からは見えない**（空欄なら
    気づくが、行はちゃんと埋まっている）。

    見るのは `warn_if_unreferenced`（誰の `to` にもなっていない）ではない ――
    制約を指す関係は `disputes`（課題 → 制約）しか無いので、それだと**課題の
    無いプロジェクトで全件出る。**
    """
    said = [f for f in validate(_constrained(model, tied=False)) if f.code == "W031"]

    assert [f.target for f in said] == ["cst-1（画像化に要る環境）"]
    assert "W031" not in codes(validate(_constrained(model, tied=True)))


def test_W031は上流と言わない(model: mm.Metamodel) -> None:
    """旗の名前は最初の使い道（画面 → 要件）を写しただけで、**機構ではない。**

    `constrains` は制約 → モジュールで**下流を縛る**。「上流に繋がっていません」
    と出すと、読んだ人は要件の側を探しに行く ―― 直す先と逆の方向を指してしまう。
    """
    said = [f for f in validate(_constrained(model, tied=False)) if f.code == "W031"]

    assert "上流" not in said[0].message
    assert "constrains（制約する）" in said[0].message


def test_相手が無いと確かめたW031はknown_gapsでW032へ落ちる(
        model: mm.Metamodel) -> None:
    """**「確かめたうえで相手が無い」を宣言する口。**

    縛る先がラウンドの資料に入っていない制約は実在する（実測: 手順書の正本の
    置き場を縛る `build/build.py` が資料の外だった）。宣言の口が無いと、手順書
    どおり statement に書いて残した**正しく処理済みの warn** が毎回鳴り続け、
    警告一覧の中で未処理と区別できない ―― E027 → W032 と同じ落とし方にする。
    """
    spec = _constrained(model, tied=False)
    spec.items[0]["known_gaps"] = {
        "constrains": {"reason": "縛る先（build/build.py）がこのラウンドの資料に"
                                 "入っていない", "at": "2026-08-10"}}
    findings = validate(spec)

    assert "W031" not in codes(findings)
    gap = [f for f in findings if f.code == "W032"]
    assert len(gap) == 1 and "資料に入っていない" in gap[0].message
    assert gap[0].level == "warn"                 # 消しはしない ―― 理由つきで出続ける


def test_相手が現れたW031の宣言はW033で古いと言う(model: mm.Metamodel) -> None:
    """次のラウンドで相手が入って関係を張れたら、宣言のほうを消させる。"""
    spec = _constrained(model, tied=True)
    spec.items[0]["known_gaps"] = {
        "constrains": {"reason": "縛る先が資料に入っていない"}}
    findings = validate(spec)

    assert "W032" not in codes(findings)
    assert "W033" in codes(findings)


def _tested(model: mm.Metamodel, tied: bool) -> Spec:
    """テストケース 1 件とモジュール 1 本。``tied`` なら ``verifies`` で繋ぐ。"""
    return Spec(metamodel=model, items=[
        {"id": "tcs-1", "type": "test-case", "status": "review",
         "name": "採番の安定性", "statement": "同じ入力からは同じ採番が出ること",
         "test_id": "TC-0001", "expected": "同じ採番が出る"},
        {"id": "mod-1", "type": "module", "status": "review",
         "name": "tests.test_build", "statement": "test_build は build を検証すること",
         "module_id": "MOD-001"},
    ], relations=[{"type": "verifies", "from": "tcs-1", "to": "mod-1",
                   "status": "review"}] if tied else [])


def test_verifiesはmoduleへ張れる(model: mm.Metamodel) -> None:
    """**テストが実際に相手にしている粒度で張れること。**

    現実のテストファイルはたいてい**ファイル 1 本（仕組み）**を相手にする ――
    一番細かい `method` しか無かったとき、関数を名指しで取り込んだ 10 件しか
    張れず、527 件中 517 件が関係 0 本のまま通ってトレーサビリティ・マトリクスが
    全章「（該当なし）」で出た。
    """
    findings = validate(_tested(model, tied=True))

    assert "E024" not in codes(findings)          # module は許された終点である
    assert "W031" not in codes(findings)


def test_levelとstepsが無くてもE010にしない(model: mm.Metamodel) -> None:
    """**資料がコードのとき、テストの段階も手順書きもどこにも書いていない。**

    「pytest だから全部単体」は作文である（同じ tests/ にコマンドラインを丸ごと
    動かすテストが混ざる）。必須のままだと、手順書どおり空欄にした正しい作業が
    E010 を 1054 件出して publish に拒まれる ―― `accesses.crud` /
    `operates.permission` と同じ判断で、資料が言っているところまでを正とする。
    """
    findings = validate(_tested(model, tied=True))

    assert "E010" not in codes(findings)


def test_検証相手の無いテストケースはW031(model: mm.Metamodel) -> None:
    """**関係 0 本の全損に警告が 1 件も出ない**、を塞ぐ。

    517 件が `verifies` 0 本のまま lint / freeze / check --strict / publish の
    全部を通った ―― マトリクスの「（該当なし）」だけが痕跡で、原因（張り忘れか、
    受け皿が無いか）は成果物から区別できなかった。`constraint`（3.6.0）と同じ
    付け忘れである。
    """
    said = [f for f in validate(_tested(model, tied=False)) if f.code == "W031"]

    assert [f.target for f in said] == ["tcs-1（採番の安定性）"]


# ── 統合漏れの候補（W044） ──────────────────────────────────────
def _constraint(ident: str, name: str, statement: str,
                source: str | None = None) -> dict[str, Any]:
    """既定の出典は**アイテムごとに別**（＝互いに素）。二重登録の疑いは
    「別々の資料が同じことを書いた」形なので、こちらが検体の既定である。"""
    where = source or f"{ident}.md"
    return {"id": ident, "type": "constraint", "status": "review",
            "constraint_id": ident.upper(), "name": name,
            "statement": statement, "category": "技術",
            "source": [{"file": where, "anchor": "s1"}]}


def test_ほぼ同一の仕様文はW044になる(model: mm.Metamodel) -> None:
    """実測（r001）の形 ―― 同じ事実が要件定義書とプロジェクト計画書の両方に
    書かれ、concept 名が別々に付いて 2 件になった（制約 102 件中 6 組）。"""
    spec = Spec(metamodel=model, items=[
        _constraint("cst-1", "文字コード",
                    "データベースおよび連携ファイルの文字コードは UTF-8 とすること"),
        _constraint("cst-2", "文字コードの規約",
                    "データベースおよび連携ファイルの文字コードは UTF-8 とすること"),
    ], relations=[])

    found = [f for f in validate(spec) if f.code == "W044"]
    assert len(found) == 1
    assert found[0].level == "warn"
    assert "cst-2" in found[0].message           # 相方を名指しする
    assert "統合" in (found[0].hint or "")


def test_言い換えられた二重登録もW044になる(model: mm.Metamodel) -> None:
    """実測（r001）―― 同じ規則が画面仕様書と処理仕様書に別 ID で入っていた。
    差の全部が**語尾**に出るので、落としてから測らないと閾値に届かない。"""
    spec = Spec(metamodel=model, items=[
        _constraint("cst-1", "請求年月の形式",
                    "請求年月は YYYYMM 形式で入力すること"),
        _constraint("cst-2", "請求年月の書式",
                    "請求年月は YYYYMM 形式であること"),
    ], relations=[])

    found = [f for f in validate(spec) if f.code == "W044"]
    assert len(found) == 1
    assert "出典も重なりません" in found[0].message


def test_出典が重なる組はW044にならない(model: mm.Metamodel) -> None:
    """**同じシートの別の行から起きた 2 件は、資料が実際に 2 つ書いている。**
    閾値だけで測ると、定型文どうしがここで偽陽性になる。"""
    spec = Spec(metamodel=model, items=[
        _constraint("cst-1", "請求年月の形式",
                    "請求年月は YYYYMM 形式で入力すること", source="同じ.md"),
        _constraint("cst-2", "請求年月の書式",
                    "請求年月は YYYYMM 形式であること", source="同じ.md"),
    ], relations=[])

    assert "W044" not in codes(validate(spec))


def test_出典を持たない組はW044にならない(model: mm.Metamodel) -> None:
    """空集合どうしは形式的には互いに素だが、それは**証拠が無い**である ――
    実測（r001）で、食い違い検出が起こした課題 7 組がここに落ちた（仕様文の
    末尾が定型なので、名前が全く違っても一致度が閾値を越える）。"""
    left = _constraint("cst-1", "文字コード", "文字コードは UTF-8 とすること")
    right = _constraint("cst-2", "文字コードの規約", "文字コードは UTF-8 とすること")
    del left["source"], right["source"]

    assert "W044" not in codes(validate(Spec(metamodel=model,
                                             items=[left, right], relations=[])))


def test_定型文の言い換えはW044にならない(model: mm.Metamodel) -> None:
    """検証ルールの定型（「〜は未入力でないこと」）は似るのが普通 ―― ここまで
    言うと本当の二重登録が埋もれる。"""
    spec = Spec(metamodel=model, items=[
        _constraint("cst-1", "受注数量", "受注数量は未入力でないこと"),
        _constraint("cst-2", "納品希望日", "納品希望日は未入力でないこと"),
    ], relations=[])

    assert "W044" not in codes(validate(spec))


def test_並びの中の種別はW044を見ない(model: mm.Metamodel) -> None:
    """同じ「得意先コード」が複数テーブルの列として並ぶのは資料の写しとして
    正しい ―― ordered な関係の to になれる種別は対象外。"""
    def item(ident: str) -> dict[str, Any]:
        return {"id": ident, "type": "data-item", "status": "review",
                "name": "得意先コード", "data_type": "文字列",
                "statement": "得意先コードは得意先を一意に識別するコードであること"}

    spec = Spec(metamodel=model, items=[item("itm-1"), item("itm-2")],
                relations=[])

    assert "W044" not in codes(validate(spec))


def test_廃止したアイテムはW044を見ない(model: mm.Metamodel) -> None:
    spec = Spec(metamodel=model, items=[
        _constraint("cst-1", "文字コード", "文字コードは UTF-8 とすること"),
        {**_constraint("cst-2", "文字コード（旧）",
                       "文字コードは UTF-8 とすること"),
         "status": "deprecated"},
    ], relations=[])

    assert "W044" not in codes(validate(spec))


# ── 属性の欠落（E010 → W032） ───────────────────────────────────
def _typeless(model: mm.Metamodel, known_gaps: Any = None) -> Spec:
    """型の欄が資料に無いデータ項目 1 件（``data_type`` が ``required``）。

    帳票の出力項目シートには型の欄そのものが無い ―― 実案件で普通に起きる形で、
    実測（sales-corpus 30 冊）でも 14 件がこれだった。
    """
    item: dict[str, Any] = {
        "id": "itm-1", "type": "data-item", "status": "review",
        "name": "出荷指示番号",
        "statement": "出荷指示書の出荷指示番号は出荷指示を識別する番号であること"}
    if known_gaps is not None:
        item["known_gaps"] = known_gaps
    return Spec(metamodel=model, items=[item], relations=[])


def test_必須属性が無ければE010(model: mm.Metamodel) -> None:
    """検出そのものは正しい。**メタモデルを緩めて通してはいけない。**"""
    findings = validate(_typeless(model))

    assert "E010" in codes(findings)


def test_資料に欄が無い必須属性はknown_gapsでW032へ落ちる(
        model: mm.Metamodel) -> None:
    """**逃げ道が ``--force`` しか無いと、本物の error まで押し流される。**

    実測: sales-corpus 30 冊で ``data_type`` の E010 が 14 件出て publish が拒み、
    ``--force`` で通された結果、同時に鳴っていた W030 32 件・W031 27 件・W044 6 件が
    まとめて無音になった（`--force` を打った時点で警告一覧を読む動機が消える）。
    pack.yml が 3.2.0 / 3.4.0 / 3.5.0 / 3.8.0 と 4 度書き残した失敗と同じ型である。
    """
    findings = validate(_typeless(model, {
        "data_type": {"reason": "帳票一覧・レイアウトの出力項目シートに型の欄が無い",
                      "at": "2026-08-11"}}))

    assert "E010" not in codes(findings)
    gap = [f for f in findings if f.code == "W032"]
    assert len(gap) == 1 and "型の欄が無い" in gap[0].message
    assert "data_type" in gap[0].message
    assert gap[0].level == "warn"                 # 消しはしない ―― 理由つきで出続ける


def test_欄が埋まった属性の宣言はW033で古いと言う(model: mm.Metamodel) -> None:
    """次のラウンドで型が判明したら、宣言のほうを消させる。"""
    spec = _typeless(model, {"data_type": {"reason": "出力項目シートに型の欄が無い"}})
    spec.items[0]["data_type"] = "文字列"
    findings = validate(spec)

    assert "W032" not in codes(findings)
    assert "W033" in codes(findings)


def test_属性名の誤字はE018(model: mm.Metamodel) -> None:
    """関係型のときと同じ ―― **守っているつもりで守られていない**を作らせない。"""
    findings = validate(_typeless(model, {
        "datatype": {"reason": "型の欄が無い"}}))       # 正しくは data_type

    assert "E018" in codes(findings)
    assert "E010" in codes(findings)                    # 誤字なので落ちていない


def test_理由の無い属性の宣言はE019(model: mm.Metamodel) -> None:
    findings = validate(_typeless(model, {"data_type": {"at": "2026-08-11"}}))

    assert "E019" in codes(findings)


# ── 出典どうしの食い違い（W045） ────────────────────────────────
def _conflicted(model: mm.Metamodel) -> Spec:
    """``build`` が採らなかった値を持つデータ項目 1 件。"""
    return Spec(metamodel=model, items=[{
        "id": "itm-1", "type": "data-item", "status": "review",
        "name": "受注番号", "data_type": "文字列",
        "statement": "受注番号は受注を識別する番号であること",
        "conflicts": {"data_type": [
            {"value": "数値",
             "source": {"file": "資料/b.xlsx/項目一覧", "anchor": "s2-t1"}}]},
    }], relations=[])


def test_採らなかった値はW045で言い続ける(model: mm.Metamodel) -> None:
    """**build の warn は端末に流れて消える。**

    衝突そのものは `B022` / `B023` が正しく言っていた。だが build の出力は
    「何をしたか」であって、次に `check` を回した人には衝突があったことが見えない
    ―― 実測で `5.権限マトリクス` の △（部長職のみ可）が `4.セキュリティ方式` の
    description に上書きされ、B022 は 6 件鳴っていたが誰も読まなかった。
    """
    found = [f for f in validate(_conflicted(model)) if f.code == "W045"]

    assert len(found) == 1
    assert "数値" in found[0].message and "s2-t1" in found[0].message
    assert found[0].level == "warn"


def test_食い違いが無ければW045は出ない(model: mm.Metamodel) -> None:
    spec = _conflicted(model)
    del spec.items[0]["conflicts"]

    assert "W045" not in codes(validate(spec))


def test_conflictsはメタモデルに無い属性と言われない(model: mm.Metamodel) -> None:
    """``overridden`` / ``known_gaps`` と同じ予約キーである ―― `W010` を出すと、
    構築が毎回書くものについて「直せ」と言い続けることになる。"""
    assert "W010" not in codes(validate(_conflicted(model)))
