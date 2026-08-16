"""``arp4 lint --fix`` ―― **機械的に確実なものだけを直す。**

自動修正は範囲を狭く切れるかどうかが全部である。arp4 には越えてはいけない線が
1 本あり ―― **意味の判断は整理層だけ** ―― 機械がそれを越えると、越えたことが
誰にも見えない（直った結果しか残らない）。

直すのは 1 種類だけである。

``G008``  レコード直下に書かれた**宣言済みの属性**を ``attrs`` の中へ移す

**語彙に無い名前は動かさない。** ``桁数`` と書いてあるものを ``attrs`` へ移しても
``build`` が捨てる先が変わるだけで、**直っていないのに指摘が消える**。名前の
取り違えは ``G016`` が言い、直すのは書いた側である（``length`` なのか
``precision`` なのかは資料を読まないと決まらない）。

## 直さないと決めたもの

``G014``（YAML が壊れている）
    **ファイルが構文解析できないので、どんな直しも本文の推測になる。** 実測で
    いちばん多い指摘なので直したくなるが、``（固定: 130010）`` のどこからどこまでが
    値なのかは**壊れた構文からは決められない** ―― 引用符を当てる位置を 1 つ
    間違えると、資料の値が黙って別のものになる。位置とヒントを出すところで止める。

``G011``（``kind`` が不正）
    表記ゆれを寄せるつもりだったが、**寄せる相手が決まらない。** YAML の平文
    スカラは前後の空白を自分で落とすので、空白ゆれはそもそもここへ届かない。
    残るのは ``対象外外`` のような別の語で、それをどちらに寄せるかは意味の判断である。

``G016``（宣言に無い属性名）
    同上。名前を当てにいくのは意味の判断である。

## 書く前に読み直して検算する

直した結果を**必ず読み直し、期待した値と 1 文字でも違えば書かない。** 行を
いじる実装である以上、器用にやるほど壊し方も器用になる ―― 賢さで安全を担保
しない。期待値は「読み込み済みの値に Python でその移動を施したもの」で、
**両者が完全に一致したときだけ**書き込む。

値は**生のテキストのまま運ぶ**（読み直して書き戻さない）。``yaml.safe_dump`` で
書き戻すと、`` 1,200,000/年 `` の引用の有無やコメントが失われる ―― 整理結果は
人とエージェントが読む面なので、**直した覚えのない差分を出さない。**

1 回に 1 つだけ直して読み直す。まとめて直すと行番号が互いにずれ、**ずれたことに
気づく手立てが無い**（そこがいちばん静かに壊れる）。
"""

from __future__ import annotations

import copy
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from arp4 import shape, yamlio
from arp4.finding import Finding
from arp4.metamodel import Metamodel
from arp4.yamlio import YamlError

#: 1 ファイルで直す回数の上限。**進まなくなったら止まる**ので保険である。
_LIMIT = 200


@dataclass(frozen=True)
class Fix:
    """直した 1 件。**何をどこで直したかを必ず言う。**"""

    code: str
    file: str
    line: int
    what: str

    def render(self) -> str:
        return f"[fixed] {self.code} {self.file}:{self.line}: {self.what}"

    def as_dict(self) -> dict[str, Any]:
        return {"code": self.code, "file": self.file, "line": self.line,
                "what": self.what}


def repair(path: Path, model: Metamodel, location: str,
           write: bool = True) -> tuple[list[Fix], list[Finding]]:
    """1 ファイルを直せるだけ直す。**検算を通ったものだけ書く。**"""
    applied: list[Fix] = []
    refused: list[Finding] = []
    try:
        # **改行の種類を保つ。** `arp4 declare` が書いたファイルは Windows では
        # CRLF なので、素直に読み書きすると**直した覚えのない差分**が全行に出る。
        # （`Path.read_text` が `newline` を受けるのは 3.13 から ―― 3.11 で動く）
        with path.open(encoding="utf-8", newline="") as handle:
            original = handle.read()
    except OSError as exc:
        return applied, [Finding("error", "G014", "", f"読めません: {exc}",
                                 file=location)]
    crlf = "\r\n" in original
    text = original.replace("\r\n", "\n")

    for _ in range(_LIMIT):
        try:
            data, marks = yamlio.marked(text, path)
        except YamlError:
            break                             # G014 は lint 側が言う。直さない
        if not isinstance(data, dict):
            break
        found = _candidate(text, data, marks, model, location)
        if found is None:
            break
        fix, changed, expected = found
        trouble = _verify(changed, expected, path, fix)
        if trouble is not None:
            refused.append(trouble)
            break                             # 同じところで足踏みしない
        text = changed
        applied.append(fix)

    if applied and write:
        path.write_text(text, encoding="utf-8",
                        newline="\r\n" if crlf else "\n")
    return applied, refused


def _verify(text: str, expected: Any, path: Path,
            fix: Fix) -> Finding | None:
    """**直した結果を読み直す。** 期待と 1 文字でも違えば書かない。"""
    try:
        after, _ = yamlio.marked(text, path)
    except YamlError as exc:
        return Finding("warn", "G017", "", f"直せませんでした（{fix.what}）"
                       f"。直した結果が YAML として読めません: {exc.detail}",
                       file=fix.file, line=fix.line)
    if after != expected:
        return Finding("warn", "G017", "", f"直せませんでした（{fix.what}）"
                       "。直した結果が期待した値と違います（書き込んでいません）",
                       file=fix.file, line=fix.line)
    return None


# ── 直す候補を 1 つ見つける ────────────────────────────────────
def _candidate(text: str, data: dict[str, Any], marks: yamlio.Marks,
               model: Metamodel,
               location: str) -> tuple[Fix, str, Any] | None:
    """``(直し, 直したあとの全文, 期待する値)``。**1 回に 1 つだけ返す。**"""
    declared_keys = set(((shape.load().get("shapes") or {}).get("record") or {})
                        .get("keys") or {})
    records = data.get("records")
    if not isinstance(records, list):
        return None

    for index, record in enumerate(records):
        if not isinstance(record, dict):
            continue
        attributes = _attributes(record, model)
        for key in record:
            if key in declared_keys or key not in attributes:
                continue                      # 語彙に無い名前は動かさない
            made = _move(text, marks, index, str(key), location)
            if made is None:
                continue
            fix, changed = made
            expected = copy.deepcopy(data)
            moved = expected["records"][index]
            value = moved.pop(key)
            attrs = moved.get("attrs")
            moved["attrs"] = {**attrs, key: value} if isinstance(attrs, dict) \
                else {key: value}
            return fix, changed, expected
    return None


def _attributes(record: dict[str, Any], model: Metamodel) -> dict[str, Any]:
    """そのレコードの種別が宣言している属性。**種別が決まらなければ空。**"""
    mapped = model.for_fact(str(record.get("type") or "")) if record.get("type") else None
    if mapped is None:
        return {}
    return (model.item_types.get(mapped[0]) or {}).get("attributes") or {}


def _move(text: str, marks: yamlio.Marks, index: int, key: str,
          location: str) -> tuple[Fix, str] | None:
    """レコード直下の 1 行を ``attrs`` の中へ移す。**値は生のテキストのまま。**"""
    lines = text.split("\n")
    # **遡って引かない。** 遡ると別の行を書き換える（→ Marks.exact）。
    row = marks.exact("records", index, key)
    if row is None or row > len(lines):
        return None

    source = lines[row - 1]
    head, sep, rest = source.partition(f"{key}:")
    if not sep or head.strip() or not rest.strip():
        # 行頭が `key:` でない（コメント中・複数行の値）ものは触らない。
        return None
    indent, raw = head, rest.strip()

    target = marks.exact("records", index, "attrs")
    what = f"{key} を attrs へ移しました"
    fix = Fix("G008", location, row, what)

    if target is None:                        # attrs がまだ無い ―― その場で作る
        lines[row - 1] = f"{indent}attrs: {{ {key}: {raw} }}"
        return fix, "\n".join(lines)

    attrs_line = lines[target - 1]
    body = attrs_line.rstrip()
    if body.endswith("}"):                    # フロー記法 ―― 閉じ括弧の前へ足す
        inner = body[:-1].rstrip()
        joiner = "" if inner.endswith("{") else ","
        lines[target - 1] = f"{inner}{joiner} {key}: {raw} }}"
        del lines[row - 1]
        return fix, "\n".join(lines)

    if body.endswith("attrs:"):               # ブロック記法 ―― 子として足す
        child = f"{' ' * (len(attrs_line) - len(attrs_line.lstrip()) + 2)}{key}: {raw}"
        lines.insert(target, child)
        del lines[row if row > target else row - 1]
        return fix, "\n".join(lines)
    return None
