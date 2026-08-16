"""``known_gaps`` ―― **「資料に定義が無い」ことを正本側で表明する。**

一覧には載っているのに定義シートが無いテーブル・帳票・画面は、実案件では必ず出る。
``check`` はこれを ``E027``（多重度違反）で正しく拾うが、**承知していることを
書く場所が無かった** ―― ``out_of_scope`` は整理層のアンカーに対するもので正本
アイテムには使えず、``overridden`` は値の上書きであって欠落の宣言ではない。

結果、資料が揃うまで ``publish --force`` を打ち続けることになり、
**``--force`` が常態化して本物の error を見落とす。** そこで欠落そのものを
記録できるようにする::

    - id: ent-5c54d300c228
      physical_name: M_PRICE
      known_gaps:
        has-column:
          reason: テーブル定義書に得意先別単価マスタの列定義シートが無い。先方へ依頼済み
          at: 2026-08-03

``reason`` は ``overridden`` と同じく**必須**である。空を許すと「承知している」と
「見落としている」の区別がつかず、E027 を黙らせるためだけのキーになる。

**黙って消さない。** 載っているものは error から warn（``W032``）へ落ちるだけで、
理由つきで出続ける。宣言したのに違反が無くなったら ``W033`` で「もう要らない」と
言う ―― 古い言い訳が正本に残り続けるほうが、error より始末が悪い。

``E027``（多重度）だけでなく ``W031``（``warn_if_no_upstream`` の関係が 0 本）にも
効く。**「確かめたうえで相手が無い」を宣言する口**が無いと、縛る先がラウンドの
資料に入っていない制約・検証相手を語彙が持たないテストケースで、正しく処理した
warn が永久に鳴り続け、警告一覧の中で処理済みと未処理が区別できなくなる
（実測で、正しく処理された 1 件が freeze と check の 2 か所で鳴り続けていた）::

    - id: cst-19
      known_gaps:
        constrains:
          reason: 縛る先（build/build.py）がこのラウンドの資料に入っていない
          at: 2026-08-10

**関係だけでなく属性の欠落も宣言できる**（``E010`` → ``W032``）。ここが無いあいだ、
必須属性が資料に無いという実在の状況には逃げ道が ``--force`` しか無かった ――
上に書いた「``--force`` が常態化して本物の error を見落とす」を、この module 自身が
関係については塞ぎ、属性については塞ぎ忘れていた。実測（sales-corpus 30 冊）で
``data_type`` の ``E010`` が 14 件出て publish が拒み、``--force`` で通されている
（帳票の出力項目シートには型の欄が無い ―― 資料が持っていない値である）::

    - id: itm-490d8e5ac870
      name: 出荷指示番号
      known_gaps:
        data_type:
          reason: 帳票一覧・レイアウトの出力項目シートに型の欄が無い
          at: 2026-08-11

``data-item.data_type`` の ``操作`` のような**逃がし値へ流すのとは違う**。逃がし値は
「型を持たない部品である」という判断であり、こちらは「資料が型を書いていない」という
事実の申告である ―― 前者は次のラウンドで拾い直す必要が無く、後者はある。
メタモデルの注記が言う「型が資料に無いだけのものをここへ逃がしてはいけない」の
**「ここへ逃がさない」の行き先がこれ**である。
"""

from __future__ import annotations

from typing import Any, Iterable

from arp4.finding import Finding

#: アイテムが持つキー（メタモデルの属性ではない）。
KEY = "known_gaps"


def declared(item: dict[str, Any]) -> dict[str, dict[str, Any]]:
    """関係型・属性名 → ``{reason, at}``。**形が壊れているものは含めない**（E018）。"""
    value = item.get(KEY)
    if not isinstance(value, dict):
        return {}
    return {str(name): entry for name, entry in value.items()
            if isinstance(entry, dict)}


def reason(item: dict[str, Any], relation_type: str) -> str:
    """その関係の欠落が表明されているか。無ければ空文字。"""
    entry = declared(item).get(relation_type)
    if entry is None:
        return ""
    return str(entry.get("reason") or "")


def check(item: dict[str, Any], target: str,
          relation_types: Iterable[str],
          attribute_names: Iterable[str] = ()) -> list[Finding]:
    """宣言そのものの妥当性。**誤字は「守っているつもりで守られていない」を作る。**

    ``attribute_names`` はそのアイテム種別が持つ属性名。関係型と同じ名前空間に
    置くのは、書く側から見て「**この欄が資料に無い**」の一言で足りるからである
    ―― ``known_gaps.relations`` / ``known_gaps.attributes`` と割ると、宣言のたびに
    どちらの器かを判断させることになり、``E018`` の誤字が増えるだけで得が無い。

    **名前が重なるのは 1 つだけ**である（``jp-sier-std`` 3.11.0 の 30 種別 × 属性と
    29 関係型で、``raises`` が「メッセージを出す」と ``method.raises``（例外）の
    両方にある）。検査は所属を見ないので誤検出にはならないが、``method`` に
    ``known_gaps.raises`` を書いた人がどちらを指したかは読み手から決まらない ――
    語彙を足すときは、関係型の名前を属性名とぶつけないこと。
    """
    if KEY not in item:
        return []
    value = item[KEY]
    if not isinstance(value, dict):
        return [Finding("error", "E018", target,
                        "known_gaps は 関係型または属性名 → {reason, at} の"
                        "連想配列で書いてください")]

    known = set(relation_types) | set(attribute_names)
    findings: list[Finding] = []
    for name, entry in value.items():
        name = str(name)
        if not isinstance(entry, dict):
            findings.append(Finding("error", "E018", target,
                                    f"known_gaps.{name} は "
                                    "{reason, at} の連想配列で書いてください"))
            continue
        if name not in known:
            findings.append(Finding(
                "error", "E018", target,
                f"known_gaps に無い関係型・属性名が挙がっています: {name}"))
        if not str(entry.get("reason") or "").strip():
            findings.append(Finding("error", "E019", target,
                                    f"known_gaps に理由がありません: {name}"
                                    "（reason は必須です）"))
    return findings
