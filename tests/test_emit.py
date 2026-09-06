"""``arp4 emit`` ―― 正本から**決まっているものだけ**をコードにする。

見るものは 4 つ。**決まっているものは出す**（DDL の桁・主キー・外部キー・索引、
型付きの引数）、**決まっていないものは印を付けて数える**（黙って `Object` に
逃がさない）、**本体は書かない**（手続きの中身は疑似コードにしかない）、
**同一入力から同一出力**。

`publish` と同じ位置にある生成なので、確かめ方も同じ ―― 正本を組み立てて
出力の字を見る。
"""

from __future__ import annotations

from typing import Any

import pytest

from arp4 import emit, pack
from arp4 import metamodel as mm
from arp4.spec import Spec


@pytest.fixture(scope="module")
def profiles() -> dict[str, dict[str, Any]]:
    chain, findings = pack.resolve_chain("jp-sier-std")
    assert not [f for f in findings if f.level == "error"]
    found = pack.languages(chain)
    assert set(found) >= {"SQL", "Java", "Python", "TypeScript",
                          "JavaScript", "C"}
    return found


def _spec(model: mm.Metamodel, items: list[dict], relations: list[dict]) -> Spec:
    return Spec(metamodel=model, items=items, relations=relations)


# ── DDL ─────────────────────────────────────────────────────────
_TABLE = [
    {"id": "ent-1", "type": "entity", "status": "review", "name": "受注",
     "statement": "受注を保持すること", "physical_name": "orders"},
    {"id": "ent-2", "type": "entity", "status": "review", "name": "得意先",
     "statement": "得意先を保持すること", "physical_name": "customers"},
    {"id": "itm-1", "type": "data-item", "status": "review", "name": "受注番号",
     "statement": "受注を識別すること", "data_type": "文字列", "length": 10},
    {"id": "itm-2", "type": "data-item", "status": "review", "name": "受注金額",
     "statement": "税込の受注金額であること", "data_type": "数値",
     "length": 12, "decimals": 2},
    {"id": "itm-3", "type": "data-item", "status": "review", "name": "明細数",
     "statement": "明細の本数であること", "data_type": "数値", "length": 3},
    {"id": "itm-4", "type": "data-item", "status": "review", "name": "確定ボタン",
     "statement": "受注を確定すること", "data_type": "操作"},
    {"id": "idx-1", "type": "index", "status": "review", "name": "idx_orders_no",
     "statement": "受注番号で引けること", "columns": "order_no",
     "uniqueness": "一意"},
]
_TABLE_RELATIONS = [
    {"type": "has-column", "from": "ent-1", "to": "itm-1", "status": "review",
     "order": 1, "physical_name": "order_no", "pk": True},
    {"type": "has-column", "from": "ent-1", "to": "itm-2", "status": "review",
     "order": 2, "physical_name": "amount", "not_null": True,
     "default_value": "0"},
    {"type": "has-column", "from": "ent-1", "to": "itm-3", "status": "review",
     "order": 3, "physical_name": "line_count"},
    {"type": "has-column", "from": "ent-1", "to": "itm-4", "status": "review",
     "order": 4, "physical_name": "confirm"},
    {"type": "has-index", "from": "ent-1", "to": "idx-1", "status": "review",
     "order": 1},
    {"type": "references", "from": "ent-1", "to": "ent-2", "status": "review",
     "fk_columns": "customer_id", "cardinality": "1対多"},
]


def _ddl(model: mm.Metamodel, profiles) -> str:
    result = emit.plan(_spec(model, _TABLE, _TABLE_RELATIONS), profiles)
    found = [e for e in result.of_kind("ddl") if e.relative == "ddl/orders.sql"]
    assert found, [e.relative for e in result.emitted]
    return found[0].text


def test_DDLは桁と主キーと外部キーと索引まで出る(model: mm.Metamodel,
                                                  profiles) -> None:
    """**ここは正本で完全に決まっている。** 桁・小数・PK・FK・一意性まで。"""
    text = _ddl(model, profiles)

    assert "CREATE TABLE orders (" in text
    assert "order_no" in text and "VARCHAR(10)" in text
    assert "NUMERIC(12,2)" in text                 # 小数ありは桁と小数の 2 つ
    assert "DEFAULT 0" in text
    assert "CONSTRAINT pk_orders PRIMARY KEY (order_no)" in text
    assert "CONSTRAINT fk_orders_customers FOREIGN KEY (customer_id)" in text
    assert "REFERENCES customers" in text
    assert "CREATE UNIQUE INDEX idx_orders_no ON orders (order_no);" in text


def test_小数の無い数は整数の綴りにする(model: mm.Metamodel, profiles) -> None:
    """``NUMERIC(3,0)`` と書くと現場の DDL（`INTEGER`）と綴りが揃わない。"""
    text = _ddl(model, profiles)

    assert "NUMERIC(3)" in text
    assert "NUMERIC(3,0)" not in text


def test_値を持たない部品は列にせず本数を申告する(model: mm.Metamodel,
                                                  profiles) -> None:
    """**`操作` は画面のボタンであって列ではない**（`data_type` のコメント）。

    黙って落とすと「資料に無い」と「機械が落とした」が混ざる ―― 落としたことを
    生成物の側に書く。
    """
    text = _ddl(model, profiles)

    assert "confirm" not in text
    assert "列にしなかった項目 1 件" in text and "確定ボタン" in text


def test_生成物は出どころと型の対応表を名乗る(model: mm.Metamodel,
                                              profiles) -> None:
    """**`数値` を `NUMERIC` にしたのは arp4 である。**黙って選ばない。"""
    text = _ddl(model, profiles)

    assert emit.BANNER in text
    assert "型の対応: jp-sier-std/languages/sql.yml" in text
    assert "受注（ent-1）" in text


# ── 骨格 ────────────────────────────────────────────────────────
def _module(language: str, path: str, extra_items=(), extra_relations=()):
    items = [
        {"id": "mod-1", "type": "module", "status": "review", "name": "受注登録",
         "statement": "受注を登録すること", "module_id": "MOD-001",
         "class_name": "orders.OrderService", "package": "orders",
         "language": language, "source_path": path},
        {"id": "mtd-1", "type": "method", "status": "review", "name": "受注を登録する",
         "statement": "受注を 1 件登録すること", "method_id": "MTD-0001",
         "method_name": "register", "visibility": "public", "returns": "long"},
        {"id": "prm-1", "type": "parameter", "status": "review", "name": "受注番号",
         "statement": "登録する受注の番号であること", "param_name": "orderNo",
         "impl_type": "String"},
        {"id": "prm-2", "type": "parameter", "status": "review", "name": "メモ",
         "statement": "任意の覚え書きであること", "param_name": "note"},
        *extra_items,
    ]
    relations = [
        {"type": "has-method", "from": "mod-1", "to": "mtd-1",
         "status": "review", "order": 1},
        {"type": "has-parameter", "from": "mtd-1", "to": "prm-1",
         "status": "review", "order": 1},
        {"type": "has-parameter", "from": "mtd-1", "to": "prm-2",
         "status": "review", "order": 2},
        *extra_relations,
    ]
    return items, relations


def _text(model: mm.Metamodel, profiles, language: str, path: str,
          extra_items=(), extra_relations=()) -> tuple[str, emit.Result]:
    items, relations = _module(language, path, extra_items, extra_relations)
    result = emit.plan(_spec(model, items, relations), profiles)
    found = [e for e in result.of_kind("module") if e.relative == path]
    assert found, [e.relative for e in result.emitted]
    return found[0].text, result


def test_Javaの骨格は型付きの呼び出しになる(model: mm.Metamodel,
                                             profiles) -> None:
    text, _ = _text(model, profiles, "Java", "src/orders/OrderService.java")

    assert "package orders;" in text
    assert "public class OrderService {" in text
    assert "public long register(String orderNo," in text
    assert "/** 受注を 1 件登録すること */" in text
    # **本体は書かない。** 手続きの中身は疑似コードにしかない。
    assert "UnsupportedOperationException" in text


def test_Pythonの骨格はselfと註釈で出る(model: mm.Metamodel, profiles) -> None:
    text, _ = _text(model, profiles, "Python", "orders/service.py")

    assert "class OrderService:" in text
    assert "def register(self, orderNo: String" in text
    assert "NotImplementedError" in text


def test_Cにはクラスが無いので関数で出る(model: mm.Metamodel, profiles) -> None:
    """**言語の形は emit が持つ**（対応表は型の綴りしか持たない）。"""
    text, _ = _text(model, profiles, "C", "src/orders.c")

    assert "class" not in text
    assert "long register(String orderNo" in text


def test_型を書けない言語では決まっていませんと言わない(model: mm.Metamodel,
                                                        profiles) -> None:
    """``typed: false``。**言えないことと決まっていないことを混ぜない。**"""
    text, result = _text(model, profiles, "JavaScript", "src/orders.js")

    assert emit.UNKNOWN not in text
    assert not [n for n in result.notes if "型" in n]
    assert "register(orderNo, note)" in text


def test_型が決まらない引数は印を付けて数える(model: mm.Metamodel,
                                              profiles) -> None:
    """**黙って `Object` に逃がさない。** 逃がすと生成物から穴が見えなくなる。"""
    text, result = _text(model, profiles, "Java", "src/orders/OrderService.java")

    assert emit.UNKNOWN in text                    # note（型の指し先が無い）
    assert [n for n in result.notes if "型が正本で決まっていない引数が 1 件" in n]


def test_typed_asで指した型は対応表で綴る(model: mm.Metamodel, profiles) -> None:
    """**正本の語彙で指してあれば、言語ごとの綴りは対応表が決める。**"""
    item = {"id": "itm-9", "type": "data-item", "status": "review",
            "name": "備考", "statement": "覚え書きであること",
            "data_type": "文字列", "length": 40}
    link = {"type": "typed-as", "from": "prm-2", "to": "itm-9",
            "status": "review"}
    text, result = _text(model, profiles, "Java", "src/orders/OrderService.java",
                         [item], [link])

    assert "String note" in text
    assert emit.UNKNOWN not in text
    assert not [n for n in result.notes if "型が正本で決まっていない" in n]


def test_疑似コードはdocコメントへ出る(model: mm.Metamodel, profiles) -> None:
    """**本体は書かないが、書く人が読むものは渡す。**"""
    step = {"id": "pst-1", "type": "process-step", "status": "review",
            "name": "在庫を引き当てる", "statement": "在庫を引き当てること",
            "step_id": "PS-001", "pseudo": "for 明細 in 受注.明細: 引当(明細)"}
    link = {"type": "has-process-step", "from": "mtd-1", "to": "pst-1",
            "status": "review", "order": 1}
    text, _ = _text(model, profiles, "Java", "src/orders/OrderService.java",
                    [step], [link])

    assert "PS-001 for 明細 in 受注.明細: 引当(明細)" in text


# ── コード定義とメッセージ ──────────────────────────────────────
def test_コード値とメッセージは機械的な綴りの定数にする(model: mm.Metamodel,
                                                        profiles) -> None:
    """**名称から英字を作らない。** 作ると綴りを案件ごとに選ぶことになる。"""
    items, relations = _module("Java", "src/orders/OrderService.java")
    items += [
        {"id": "cdm-1", "type": "code-master", "status": "review",
         "name": "機密区分", "statement": "機密の区分であること",
         "code_id": "CD-001", "physical_name": "SECRECY"},
        {"id": "cdv-1", "type": "code-value", "status": "review", "name": "極秘",
         "statement": "極秘であること", "value": "30"},
        {"id": "msg-1", "type": "message", "status": "review",
         "name": "在庫不足", "statement": "在庫が足りないこと",
         "message_id": "E-0007", "severity": "エラー",
         "body": "在庫が不足しています"},
    ]
    relations += [{"type": "has-value", "from": "cdm-1", "to": "cdv-1",
                   "status": "review", "order": 1}]
    result = emit.plan(_spec(model, items, relations), profiles)

    codes = result.of_kind("code")[0].text
    assert 'public static final String SECRECY_V_30 = "30";' in codes
    assert "極秘" in codes                            # 名称は注記として残す

    messages = result.of_kind("message")[0].text
    assert 'public static final String E_0007 = "在庫が不足しています";' in messages


# ── 再現性 ──────────────────────────────────────────────────────
def test_同一入力から同一出力(model: mm.Metamodel, profiles) -> None:
    """**バイト一致**（`draft` / `design` と同じ土台）。"""
    spec = _spec(model, _TABLE, _TABLE_RELATIONS)

    first = emit.plan(spec, profiles)
    second = emit.plan(spec, profiles)

    assert [(e.relative, e.text) for e in first.emitted] \
        == [(e.relative, e.text) for e in second.emitted]


def test_プログラム設計の欄が無ければ出さない(model: mm.Metamodel,
                                              profiles) -> None:
    """**空を出さない。** 出す先も言語も正本が言っていないので、言うだけにする。"""
    spec = _spec(model, [
        {"id": "mod-1", "type": "module", "status": "review", "name": "受注登録",
         "statement": "受注を登録すること", "module_id": "MOD-001"}], [])
    result = emit.plan(spec, profiles)

    assert not result.of_kind("module")
    assert [n for n in result.notes if "実装言語を名乗っているモジュールが 0 件" in n]


# ── 型の対応は正本が先 ──────────────────────────────────────────
def _mapping(language: str, logical: str, physical: str,
             default: str = "") -> dict[str, Any]:
    item = {"id": f"ims-{logical}", "type": "implementation-standard",
            "status": "review", "standard_id": "IMP-01",
            "standard_kind": "型写像", "language": language,
            "name": f"{language} の {logical}",
            "statement": f"{language} では {logical} を {physical} と綴ること",
            "logical_type": logical, "physical_type": physical}
    if default:
        item["default_type"] = default
    return item


def test_正本が決めた綴りが同梱の表より優先される(model: mm.Metamodel,
                                                  profiles) -> None:
    """**案件が決めた綴りが生成に効かないなら、決める意味が無い。**

    散文の `rule` では機械が読めないので、写像は欄（論理型・綴り）で持つ。
    """
    items = _TABLE + [_mapping("SQL", "文字列", "NVARCHAR({length})", "NVARCHAR")]
    result = emit.plan(_spec(model, items, _TABLE_RELATIONS), profiles)
    text = [e for e in result.of_kind("ddl")
            if e.relative == "ddl/orders.sql"][0].text

    assert "NVARCHAR(10)" in text                  # 正本が勝つ
    assert "VARCHAR(10)" not in text.replace("NVARCHAR(10)", "")
    # 決めていない型は同梱の表のまま（**全部を決めさせない**）。
    assert "NUMERIC(12,2)" in text


def test_どちらの表を使ったかを生成物が名乗る(model: mm.Metamodel,
                                              profiles) -> None:
    """**arp4 が選んだことを黙ると「資料にそう書いてあった」と読まれる。**"""
    plain = _ddl(model, profiles)
    assert "型の対応: jp-sier-std/languages/sql.yml" in plain

    items = _TABLE + [_mapping("SQL", "文字列", "NVARCHAR({length})")]
    result = emit.plan(_spec(model, items, _TABLE_RELATIONS), profiles)
    text = [e for e in result.of_kind("ddl")
            if e.relative == "ddl/orders.sql"][0].text
    assert "正本の実装規約（型写像 1 行）" in text
    assert "jp-sier-std/languages/sql.yml" in text  # 残りはこちらから来ている


def test_欄が欠けている写像は使わない(model: mm.Metamodel, profiles) -> None:
    """**決めていない行は「決めた」に数えない。** 空欄で上書きすると型が消える。"""
    half = _mapping("SQL", "文字列", "NVARCHAR({length})")
    half.pop("physical_type")
    result = emit.plan(_spec(model, _TABLE + [half], _TABLE_RELATIONS), profiles)
    text = [e for e in result.of_kind("ddl")
            if e.relative == "ddl/orders.sql"][0].text

    assert "VARCHAR(10)" in text
    assert "型の対応: jp-sier-std/languages/sql.yml" in text


def test_言語が違う写像は混ざらない(model: mm.Metamodel, profiles) -> None:
    """`Java` の綴りが DDL に出てはいけない（**同じ論理型でも表は別**）。"""
    items = _TABLE + [_mapping("Java", "文字列", "String")]
    result = emit.plan(_spec(model, items, _TABLE_RELATIONS), profiles)
    text = [e for e in result.of_kind("ddl")
            if e.relative == "ddl/orders.sql"][0].text

    assert "String" not in text
    assert "VARCHAR(10)" in text


def test_桁が無いときの綴りは別の欄から取る(model: mm.Metamodel,
                                            profiles) -> None:
    """SQL の `整数` は `NUMERIC({length})` → `INTEGER` で**綴りそのものが変わる**。"""
    item = {"id": "itm-9", "type": "data-item", "status": "review",
            "name": "区分", "statement": "区分であること", "data_type": "数値"}
    column = {"type": "has-column", "from": "ent-1", "to": "itm-9",
              "status": "review", "order": 9, "physical_name": "kind"}
    items = _TABLE + [item, _mapping("SQL", "整数", "NUM({length})", "SMALLINT")]
    result = emit.plan(_spec(model, items, _TABLE_RELATIONS + [column]), profiles)
    text = [e for e in result.of_kind("ddl")
            if e.relative == "ddl/orders.sql"][0].text

    assert "kind" in text and "SMALLINT" in text   # 桁が無いのでこちら
    assert "NUM(3)" in text                        # 桁があるほうは form


def test_同梱の表を使ったときは端末でも言う(model: mm.Metamodel,
                                            profiles) -> None:
    """**生成物の頭に書くだけでは足りない。** 端末で 1 度も言わないと、
    「案件が決めた綴りで出た」と読まれる。"""
    plain = emit.plan(_spec(model, _TABLE, _TABLE_RELATIONS), profiles)
    assert [n for n in plain.notes if "arp4 同梱の表を使いました" in n]

    items = _TABLE + [_mapping("SQL", "文字列", "NVARCHAR({length})"),
                      _mapping("SQL", "数値", "DECIMAL({length},{decimals})"),
                      _mapping("SQL", "整数", "INT"),
                      _mapping("SQL", "日付", "DATE")]
    decided = emit.plan(_spec(model, items, _TABLE_RELATIONS), profiles)
    assert not [n for n in decided.notes if "SQL" in n]
