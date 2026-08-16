"""正本の機械検証（構造）。

見るのは**メタモデルへの適合と参照整合性**だけである。読みやすさ（見出しの
切り詰め・助詞止め・重複）は別の検査に属する。

error はデータの不備を意味する。**メタモデルを緩めて通してはならない。**

検出コード::

    E000  ファイルの形が違う（spec.load が出す）
    E001  id が無い          E002  id が重複        E003  未定義の種別
    E004  status が不正
    E010  必須属性が無い      E011  enum 外          E012  一意違反
    E013  pattern 不一致      E014  型が違う         E015  multi なのに配列でない
    E016  overridden の宣言が壊れている（誤字・形が違う）
    E017  overridden に理由（reason）が無い
    E018  known_gaps の宣言が壊れている（誤字・形が違う）
    E019  known_gaps に理由（reason）が無い
          ※ known_gaps は関係型と属性名の両方を取る。E010 も宣言すれば W032 へ落ちる
    E020  未定義の関係型      E021/E022 参照先が無い
    E023/E024 種別が許されない E025 same_type_only   E026 自己参照
    E027  多重度違反
    W010  メタモデルに無い属性 W012 上書きした属性が空   W020 関係の重複
    W021  order の欠落・重複
    W030  どこからも参照されない（カバレッジ欠落）
    W031  自分から出る関係が 1 本も無い（warn_if_no_upstream）
    W032  違反だが known_gaps に載っている（理由つきで承知している）
    W033  known_gaps に載っているのに違反が無い（宣言が古い）
    W045  出典どうしで値が食い違い、build が採らなかった値がある（conflicts）
    W044  仕様文がほぼ同一のアイテムの組（統合漏れの候補）

``W032`` の対象は E027（多重度）・W031（要る関係が 0 本）・E010（必須属性が無い）で
ある。E010 を後から足したのは、**資料が持っていない値を必須にしていると逃げ道が
``--force`` しか無くなる**ためで、これは pack.yml が 3.2.0 / 3.4.0 / 3.5.0 / 3.8.0 と
4 度書き残した失敗と同じ型である（実測: sales-corpus 30 冊で ``data_type`` の E010 が
14 件出て publish が拒み、``--force`` で通されていた）。W031 側を
入れたのは、**「確かめたうえで相手が無い」を宣言する口が無かった**ため ―― 制約の
縛る先がラウンドの資料に入っていないとき、整理層が正しく処理しても warn は毎回
鳴り続け、処理済みと未処理が警告一覧の上で区別できなくなっていた（実測 1 件が
G020 と W031 の 2 回×全ラウンド）。known_gaps に理由つきで載せれば W032 に変わり、
相手が現れて関係を張れば W033 が「宣言はもう要らない」と言う。
"""

from __future__ import annotations

import re
from typing import Any

from arp4 import gaps
from arp4 import override
from arp4 import metamodel as mm
from arp4.finding import Finding, order
from arp4.spec import Spec


def validate(spec: Spec) -> list[Finding]:
    """構造を検証する。**同じデータからは同じ順序で同じ結果**を返す。"""
    findings: list[Finding] = []
    #: 実際に効いた欠落の宣言（known_gaps）。多重度とカバレッジの両方から集め、
    #: 残りを W033 で「もう要らない」と言う ―― 片方だけ見て言うと、W031 を
    #: 承知させた宣言が毎回「古い」扱いになる。
    used: set[tuple[str, str]] = set()
    findings += _items(spec, used)
    findings += _relations(spec)
    findings += _cardinality(spec, used)
    findings += _coverage(spec, used)
    findings += _stale_gaps(spec, used)
    findings += _near_duplicates(spec)
    return order(findings)


def _named(item: dict[str, Any]) -> str:
    """検出の宛先。**内部 ID に名前を添える。**

    id は内容ハッシュ（`ent-037a3625a979`）なので、**どのアイテムのことか人には
    分からない**。`W030` が 33 件並んだとき、どれを直せばよいかを決めるには正本を
    grep するしかなかった ―― 同じ状況で `publish` は畳んだ行・列を名前つきで
    並べているので（決定 14）、検証側だけが不親切だった。
    """
    item_id = str(item.get("id") or "")
    name = str(item.get("name") or "")
    if not item_id:
        return f"({str(item.get('type') or '種別なし')} の id なし)"
    return f"{item_id}（{name}）" if name else item_id


# ── アイテム ────────────────────────────────────────────────────
def _items(spec: Spec, used: set[tuple[str, str]]) -> list[Finding]:
    findings: list[Finding] = []
    seen: set[str] = set()
    unique: dict[tuple[str, str], dict[str, str]] = {}

    for item in spec.items:
        item_id = str(item.get("id") or "")
        type_name = str(item.get("type") or "")
        target = _named(item)

        if not item_id:
            findings.append(Finding("error", "E001", target, "id がありません"))
            continue
        if item_id in seen:
            findings.append(Finding("error", "E002", target, "id が重複しています"))
        seen.add(item_id)

        definition = spec.metamodel.item_types.get(type_name)
        if definition is None:
            findings.append(Finding("error", "E003", target,
                                    f"未定義の種別です: {type_name}"))
            continue

        status = str(item.get("status") or "")
        if status not in mm.STATUSES:
            findings.append(Finding("error", "E004", target,
                                    f"status が不正です: {status or '(なし)'}"))

        attributes = definition.get("attributes") or {}
        findings += _values(target, item, attributes, unique_key=type_name,
                            unique=unique, owner=item, owner_id=item_id,
                            used=used)
        findings += _overridden(target, item, attributes)
        findings += _conflicts(target, item)
        findings += gaps.check(item, target, spec.metamodel.relation_types,
                               attributes)

        unknown = set(item) - set(attributes) - mm.ITEM_RESERVED
        for name in sorted(unknown):
            findings.append(Finding("warn", "W010", target,
                                    f"メタモデルに無い属性です: {name}"))
    return findings


def _conflicts(target: str, item: dict[str, Any]) -> list[Finding]:
    """``build`` が採らなかった値（``W045``）。**端末に流れて消えていた。**

    衝突そのものは ``build`` が ``B022`` / ``B023`` で正しく言っていた。だが
    build の出力は「何をしたか」であって、次に ``check`` を回した人には**衝突が
    あったことが見えない** ―― 実測（sales-corpus 30 冊）で、``5.権限マトリクス``
    が書いた「与信保留の解除は △（部長職のみ可）」という description が
    ``4.セキュリティ方式`` の「営業部 120 名。…」に上書きされ、``△`` は正本からも
    生成物からも消えた。B022 は 6 件鳴っていたが、誰も読んでいない。

    **警告を消すのではなく、捨てた値を残して言い続ける。** 相補的な補足は
    ``merge: append`` で両方残るようになったので、ここへ来るのは**足し合わせられ
    ないもの**（スカラ属性と ``statement``）だけである ―― どちらが正かは意味の
    判断なので、機械は決めずに人へ渡す。
    """
    entries = item.get(mm_conflicts := "conflicts")
    if not isinstance(entries, dict):
        return []
    findings: list[Finding] = []
    for name, dropped in sorted(entries.items()):
        if not isinstance(dropped, list):
            continue
        for entry in dropped:
            if not isinstance(entry, dict):
                continue
            where = entry.get("source") or {}
            findings.append(Finding(
                "warn", "W045", target,
                f"{name} は出典どうしで食い違い、採らなかった値があります: "
                f"{str(entry.get('value'))!r}"
                f"（{where.get('file', '')}#{where.get('anchor', '')}）",
                hint="同じ事実なら片方の出典を参照だけのレコードにする。"
                     "違う事実なら課題（disputes）にして、どちらへも寄せない。"
                     f"採った値でよいなら overridden に理由を書く（{mm_conflicts} は"
                     "構築のたびに書き直される）"))
    return findings


def _overridden(target: str, item: dict[str, Any],
                attributes: dict[str, Any]) -> list[Finding]:
    """``overridden``（出典と異なる値にした記録）の妥当性。

    **理由の無い上書きは、単なる上書きと区別がつかない。** 誤字も同じで、
    守っているつもりで守られていない状態を作るので error にする。
    """
    if "overridden" not in item:
        return []
    if not isinstance(item["overridden"], (dict, list)):
        return [Finding("error", "E016", target,
                        "overridden は 属性名 → {was, reason, at} の連想配列で"
                        "書いてください")]

    findings = [Finding("error", "E016", target,
                        f"overridden に無い属性が挙がっています: {name}")
                for name in override.unknown(item, attributes)]
    findings += [Finding("error", "E017", target,
                         f"overridden に理由がありません: {name}"
                         "（reason は必須です）")
                 for name in override.missing_reason(item)]
    for name in override.names(item):
        if name in attributes and item.get(name) in (None, "", []):
            findings.append(Finding(
                "warn", "W012", target,
                f"{name} は上書きされているのに値がありません"))
    return findings


# ── 関係 ────────────────────────────────────────────────────────
def _relations(spec: Spec) -> list[Finding]:
    findings: list[Finding] = []
    by_id = spec.by_id
    seen: set[tuple[str, str, str]] = set()
    orders: dict[tuple[str, str], dict[Any, str]] = {}
    unique: dict[tuple[str, str], dict[str, str]] = {}

    for relation in spec.relations:
        type_name = str(relation.get("type") or "")
        source = str(relation.get("from") or "")
        target_id = str(relation.get("to") or "")
        label = f"{type_name} {source}→{target_id}"

        definition = spec.metamodel.relation_types.get(type_name)
        if definition is None:
            findings.append(Finding("error", "E020", label,
                                    f"未定義の関係型です: {type_name}"))
            continue

        key = (type_name, source, target_id)
        if key in seen:
            findings.append(Finding("warn", "W020", label, "関係が重複しています"))
        seen.add(key)

        status = str(relation.get("status") or "")
        if status and status not in mm.STATUSES:
            findings.append(Finding("error", "E004", label,
                                    f"status が不正です: {status}"))

        from_item, to_item = by_id.get(source), by_id.get(target_id)
        if from_item is None:
            findings.append(Finding("error", "E021", label,
                                    f"参照先が存在しません: {source}"))
        if to_item is None:
            findings.append(Finding("error", "E022", label,
                                    f"参照先が存在しません: {target_id}"))
        if from_item is None or to_item is None:
            continue

        allowed_from = definition.get("from") or []
        allowed_to = definition.get("to") or []
        if allowed_from and from_item.get("type") not in allowed_from:
            findings.append(Finding(
                "error", "E023", label,
                f"起点の種別が許されていません: {from_item.get('type')}"
                f"（{'、'.join(allowed_from)}）"))
        if allowed_to and to_item.get("type") not in allowed_to:
            findings.append(Finding(
                "error", "E024", label,
                f"終点の種別が許されていません: {to_item.get('type')}"
                f"（{'、'.join(allowed_to)}）"))
        if definition.get("same_type_only") and from_item.get("type") != to_item.get("type"):
            findings.append(Finding("error", "E025", label,
                                    "同一種別どうしでのみ張れる関係です"))
        if source == target_id:
            findings.append(Finding("error", "E026", label, "自分自身を参照しています"))

        attributes = definition.get("attributes") or {}
        findings += _values(label, relation, attributes,
                            unique_key=type_name, unique=unique)

        unknown = set(relation) - set(attributes) - mm.RELATION_RESERVED
        for name in sorted(unknown):
            findings.append(Finding("warn", "W010", label,
                                    f"メタモデルに無い属性です: {name}"))

        if definition.get("ordered"):
            bucket = orders.setdefault((type_name, source), {})
            value = relation.get("order")
            if value is None:
                findings.append(Finding("warn", "W021", label,
                                        "順序のある関係に order がありません"))
            elif value in bucket:
                findings.append(Finding("warn", "W021", label,
                                        f"order が {bucket[value]} と重複しています: {value}"))
            else:
                bucket[value] = label
    return findings


# ── 多重度 ──────────────────────────────────────────────────────
def _cardinality(spec: Spec, used: set[tuple[str, str]]) -> list[Finding]:
    """「列 0 本のテーブル」「項目 0 個の画面」を error にする。

    ``known_gaps`` に理由つきで載っているものは **warn へ落とす**（消しはしない）。
    資料が揃うまで ``publish --force`` を打ち続けるのは、``--force`` を常態化させて
    本物の error を見えなくするからである → :mod:`arp4.gaps`
    """
    findings: list[Finding] = []
    by_id = spec.by_id

    for type_name, definition in spec.metamodel.relation_types.items():
        cardinality = definition.get("cardinality") or {}
        for side in ("from", "to"):
            if side not in cardinality:
                continue
            try:
                low, high = mm.parse_cardinality(cardinality[side])
            except ValueError:
                continue            # 書式の不備は M008 が報告済み

            counted: dict[str, int] = {}
            for relation in spec.relations_of(type_name):
                item_id = str(relation.get(side) or "")
                if item_id in by_id:
                    counted[item_id] = counted.get(item_id, 0) + 1

            for item in spec.items:
                if item.get("type") not in (definition.get(side) or []):
                    continue
                item_id = str(item.get("id"))
                count = counted.get(item_id, 0)
                if low <= count and (high is None or count <= high):
                    continue
                trouble = (f"{type_name} の多重度違反です: {count} 件"
                           f"（{side}: {cardinality[side]}）")
                reason = gaps.reason(item, type_name)
                if not reason:
                    findings.append(Finding("error", "E027", item_id, trouble))
                    continue
                used.add((item_id, type_name))
                findings.append(Finding(
                    "warn", "W032", item_id,
                    f"{trouble}。known_gaps で承知している: {reason}"))

    return findings


# ── カバレッジ ──────────────────────────────────────────────────
def _coverage(spec: Spec, used: set[tuple[str, str]]) -> list[Finding]:
    """トレースの欠落。**error にはしない** ―― 資料が足りないだけのこともある。

    ただし承認の前に必ず見る。
    """
    referenced = {str(rel.get("to") or "") for rel in spec.relations}
    findings: list[Finding] = []

    for item in spec.items:
        definition = spec.metamodel.item_types.get(str(item.get("type"))) or {}
        item_id = str(item.get("id"))

        if definition.get("warn_if_unreferenced") and item_id not in referenced:
            findings.append(Finding(
                "warn", "W030", _named(item),
                "どの設計要素からも参照されていません（カバレッジ欠落）"))

        upstream = definition.get("warn_if_no_upstream")
        if upstream:
            has = any(str(rel.get("from")) == item_id
                      for rel in spec.relations_of(str(upstream)))
            if not has:
                # **「上流」と言わない。** この旗が見ているのは「自分が ``from`` の
                # 関係を 1 本も出していない」であって、その先が上流とは限らない
                # ―― ``realizes``（画面 → 要件）は上流だが、``constrains``
                # （制約 → モジュール）は**下流を縛る**。旗の名前は最初の使い道を
                # 写しただけなので、文言のほうを機構に合わせる。
                label = (spec.metamodel.relation_types.get(str(upstream))
                         or {}).get("label")
                trouble = (f"{upstream}"
                           + (f"（{label}）" if label else "")
                           + " が 1 本もありません")
                # 「確かめたうえで相手が無い」は known_gaps で宣言できる ――
                # 相手がラウンドの資料に入っていない制約は実在し、宣言の口が
                # 無いと正しく処理した warn が永久に鳴り続け、未処理と
                # 区別できなくなる。W032 は理由つきで出続ける（消しはしない）。
                reason = gaps.reason(item, str(upstream))
                if reason:
                    used.add((item_id, str(upstream)))
                    findings.append(Finding(
                        "warn", "W032", _named(item),
                        f"{trouble}。known_gaps で承知している: {reason}"))
                else:
                    findings.append(Finding("warn", "W031", _named(item), trouble))
    return findings


def _stale_gaps(spec: Spec, used: set[tuple[str, str]]) -> list[Finding]:
    """宣言したのに違反が無い ``known_gaps``（``W033``）。

    多重度（E027 → W032）とカバレッジ（W031 → W032）の**両方を見終えてから**
    数える。古い言い訳が正本に残り続けるほうが、error より始末が悪い。
    """
    findings: list[Finding] = []
    for item in spec.items:
        item_id = str(item.get("id"))
        for name in sorted(gaps.declared(item)):
            # 理由の無い宣言は E019 が言う。そこへ「違反はありません」を重ねると
            # 打ち手が 2 つに割れて読めなくなる。
            if not gaps.reason(item, name) or (item_id, name) in used:
                continue
            findings.append(Finding(
                "warn", "W033", item_id,
                f"known_gaps に {name} が載っていますが、違反はありません"
                "（埋まったなら宣言を消してください）"))
    return findings


# ── 属性値 ──────────────────────────────────────────────────────
def _values(target: str, record: dict[str, Any], attributes: dict[str, Any],
            unique_key: str, unique: dict[tuple[str, str], dict[str, str]],
            owner: dict[str, Any] | None = None, owner_id: str = "",
            used: set[tuple[str, str]] | None = None) -> list[Finding]:
    """値の検証。

    ``owner`` / ``used`` はアイテムのときだけ渡す（関係の属性に ``known_gaps`` は
    無い ―― 宣言はアイテムに書く）。必須属性が欠けていても**欠落を宣言してあれば
    ``W032``** に落ちる。落とすだけで消しはしないのは関係のときと同じで、
    「資料に無いと確かめた」と「まだ見ていない」を混ぜないためである。
    """
    findings: list[Finding] = []

    for name, attr in attributes.items():
        attr = attr or {}
        value = record.get(name)

        if value is None or value == "" or value == []:
            if attr.get("required"):
                reason = gaps.reason(owner, name) if owner is not None else ""
                if reason:
                    if used is not None:
                        used.add((owner_id, name))
                    findings.append(Finding(
                        "warn", "W032", target,
                        f"必須属性がありません: {name}。"
                        f"known_gaps で承知している: {reason}"))
                else:
                    findings.append(Finding("error", "E010", target,
                                            f"必須属性がありません: {name}"))
            continue

        kind = attr.get("kind")
        if kind == "bool" and not isinstance(value, bool):
            findings.append(Finding("error", "E014", target,
                                    f"{name} は真偽値でなければなりません: {value!r}"))
            continue
        if kind == "int" and (isinstance(value, bool) or not isinstance(value, int)):
            findings.append(Finding("error", "E014", target,
                                    f"{name} は整数でなければなりません: {value!r}"))
            continue

        if kind == "enum":
            findings += _enum(target, name, attr, value)
        elif kind == "string":
            pattern = attr.get("pattern")
            if pattern and not re.fullmatch(str(pattern), str(value)):
                findings.append(Finding("error", "E013", target,
                                        f"{name} が書式に合いません: {value}"
                                        f"（{pattern}）"))

        if attr.get("unique"):
            bucket = unique.setdefault((unique_key, name), {})
            previous = bucket.get(str(value))
            if previous is not None:
                findings.append(Finding("error", "E012", target,
                                        f"{name} が一意ではありません: {value}"
                                        f"（{previous} と重複）"))
            else:
                bucket[str(value)] = target
    return findings


def _enum(target: str, name: str, attr: dict[str, Any], value: Any) -> list[Finding]:
    values = attr.get("values") or []
    if attr.get("multi"):
        if not isinstance(value, list):
            return [Finding("error", "E015", target,
                            f"{name} は複数値なので配列で書いてください: {value!r}")]
        candidates = value
    else:
        if isinstance(value, list):
            return [Finding("error", "E015", target,
                            f"{name} は単一値です。配列では書けません: {value!r}")]
        candidates = [value]

    if attr.get("extensible"):
        return []
    return [Finding("error", "E011", target,
                    f"{name} が enum 外です: {candidate}"
                    f"（{'、'.join(map(str, values))}）")
            for candidate in candidates if candidate not in values]


# ── 統合漏れの候補 ──────────────────────────────────────────────
#: これ以上似ていたら「同じ事実の二重登録」を疑う。**判別は一致度だけでは
#: しない**（→ :func:`_near_duplicates` の「出典が互いに素か」）ので、閾値は
#: 言い換えに届く高さに置く。実測（r001・sales-corpus）で 0.92 では 0 組、
#: 0.75 で 4 組が挙がり、そのうち出典の重なる 2 組が偽陽性だった。
_DUPLICATE_RATIO = 0.75

#: 語尾を落としてから測る。日本の設計書は同じ規則を「〜で入力すること」
#: 「〜であること」と書き分けるので、**差の全部が語尾に出る** ―― 落とさないと
#: RUL-021 / RUL-033（請求年月の書式）は 90% ではなく 78% になる。
_TAIL = re.compile(r"(?:と?すること|であること|できること|とする|こと)[。．]?$")


def _source_keys(item: dict[str, Any]) -> set[str]:
    """このアイテムが名乗っている出典アンカー（``<写し>#<アンカー>``）。"""
    return {f"{entry.get('file')}#{entry.get('anchor')}"
            for entry in (item.get("source") or []) if isinstance(entry, dict)}


def _near_duplicates(spec: Spec) -> list[Finding]:
    """仕様文がほぼ同一のアイテムの組（``W044``）。**統合するかは人が決める。**

    実測（r001・sales-corpus 30 冊）: 制約 102 件のうち**同じ事実の二重登録が
    6 組**あった（文字コード・端末・帳票基盤・会計システム・稼働開始・基盤 ――
    要件定義書とプロジェクト計画書の両方に書かれ、concept 名が別々に付いた）。
    課題も 4 組が二重だった（資料由来と食い違い検出由来で別 ID になった）。
    同名でない重複は concept の台帳では拾えないので、**文の類似で候補を出して
    人に裁かせる** ―― 機械が黙って統合すると「別物である理由」が消える。

    **一致度だけでは決めない。** 閾値 0.92 は上の 6 組（ほぼ**同文**）に合わせて
    調律されていたので、**言い換えられた二重登録を 1 件も拾えなかった** ――
    実測（r001）で ``RUL-021 請求年月の形式``（「YYYYMM 形式で入力すること」）と
    ``RUL-033 請求年月の書式``（「YYYYMM 形式であること」）は画面仕様書と処理
    仕様書に別 ID で登録されている。閾値を 0.75 まで下げると 4 組が挙がるが、
    素の一致度だけでは**偽陽性が半分**になる。

    そこで判別軸を 1 つ足す ―― **出典アンカー集合が互いに素か**である::

        90%  RUL-021 / RUL-033   共通出典 0 件 → 二重登録（別々の資料が同じ規則を書いた）
        77%  RUL-024 / RUL-035   共通出典 0 件 → 二重登録
        75%  RUL-030 / RUL-025   共通出典 2 件 → 別物（同じシートに 2 行ある）
        75%  RUL-026 / RUL-063   共通出典 1 件 → 別物

    **同じシートの別の行から起きた 2 件は、資料が実際に 2 つ書いている**ので
    別物である（似ているのは定型文だから）。**出典が 1 つも重ならない 2 件は、
    同じことが 2 冊に書かれた**疑いがある ―― これが統合の候補である。
    偽陽性が落ちるぶんだけ閾値を下げられる、という関係になっている。

    **片方でも出典を持たないなら何も言わない。** 空集合どうしは形式的には互いに
    素だが、それは「別々の資料から来た」の証拠ではなく**証拠が無い**である ――
    実測（r001）で、食い違い検出が起こした ``open-issue`` 7 組がここに落ちた。
    あれらは出典を持たず、仕様文の末尾 34 字が定型（「（資料が 2 通りのことを
    言っている。…）」）なので、名前が全く違っても一致度が 0.75 を越える。

    **並びの中の 1 行として存在する種別は見ない**（``ordered: true`` の関係の
    ``to`` になれる種別 ―― データ項目・コード値・手順・メソッド等）。同じ
    「得意先コード」が複数テーブルの列として並ぶのは資料の写しとして正しく、
    類似はむしろ普通である ―― ここまで言うと警告が数百件になり、本当の
    二重登録が埋もれる。
    """
    structural = {type_name
                  for definition in spec.metamodel.relation_types.values()
                  if definition.get("ordered")
                  for type_name in (definition.get("to") or [])}
    findings: list[Finding] = []
    from difflib import SequenceMatcher

    for type_name in spec.metamodel.item_types:
        if type_name in structural:
            continue
        candidates = [
            (item, _TAIL.sub("", re.sub(r"\s+", "",
                                        str(item.get("statement") or ""))))
            for item in spec.of_type(type_name)
            if item.get("status") != "deprecated"]
        candidates = [(item, text) for item, text in candidates if len(text) >= 8]
        for i, (left, left_text) in enumerate(candidates):
            for right, right_text in candidates[i + 1:]:
                longer, shorter = max(len(left_text), len(right_text)), \
                    min(len(left_text), len(right_text))
                if shorter / longer < _DUPLICATE_RATIO:
                    continue                # 長さが違いすぎる組は測るまでもない
                ratio = SequenceMatcher(None, left_text, right_text).ratio()
                if ratio < _DUPLICATE_RATIO:
                    continue
                left_keys, right_keys = _source_keys(left), _source_keys(right)
                if not (left_keys and right_keys):
                    continue                # 出典が無い側があると互いに素が言えない
                if left_keys & right_keys:
                    continue                # 同じ資料の別の行 ―― 資料が 2 つ書いている
                findings.append(Finding(
                    "warn", "W044", _named(left),
                    f"仕様文が {_named(right)} とほぼ同一で、"
                    f"出典も重なりません（一致 {ratio:.0%}）",
                    hint="同じ事実なら 1 件に統合して出典を束ねる。"
                         "別物なら、違いが仕様文に出るよう書き分ける"))
    return findings
