"""書き戻しでコメントを保つ ―― **消えるのは値ではなく理由である。**

正本（``.arp/spec/``）は人が読み書きする面でもある。``arp4 build`` / ``arp4 number``
/ 自動昇格は :func:`arp4.spec.save_in_place` でファイルを書き戻すが、YAML を
読み書きで往復すると**コメントだけが落ちる**。値は 1 つも変わらないので、
差分を見ても気づきにくい ―― 落ちるのは「なぜそうなっているか」だけである::

    # この 2 件は顧客との対応表（2026-08-20 の打合せ）。番号を動かさないこと。
    - id: req-1
      req_id: FR-001      # 契約書の別紙 3 と対応

``req_id`` の値は残り、**動かすなという申し送りと、契約書との対応だけが消える。**
`overridden` の ``reason`` を必須にし、`out_of_scope` の ``reason`` を必須にし、
`known_gaps` の ``reason`` を必須にしている道具が、**理由を書く場所を自分で
消していた**（実測 ―― `spec.py` は「コメントは失われる」と自認していた）。

## 拾うのは 3 つだけ

``header``   最初のレコードより前の塊（そのファイル全体への申し送り）
``before``   レコードの直前に続く塊（そのレコードへの申し送り）
``inline``   ``キー: 値`` の行末（その欄への註）

**レコードの対応は並び順ではなく鍵で取る。** 並びで取ると、レコードが 1 件
増えただけで**全部のコメントが 1 つ隣へずれる** ―― ずれたコメントは、消えた
コメントより始末が悪い（誰も疑わない嘘になる）。

## 相手にしていないもの

**整理結果（``organized/``）は相手にしていない。** あちらは最上位が連想配列
（``records:`` / ``out_of_scope:``）で、レコードの鍵の取り方から違う ―― 同じ
道具で両方を扱おうとすると、どちらの形も中途半端に読む器用な parser が 1 つ
できる。``arp4 declare`` の追記は**失われることを自分で断っている**ので、
少なくとも黙ってはいない（黙って落ちていたのは正本の側だけである）。

## 賢さで安全を担保しない

組み直した結果を**必ず読み戻し、レコードが 1 つでも違えば捨てて素の書き出しに
戻す**（:func:`arp4.fix.repair` と同じ規律）。コメントを保てなかったことより、
値が変わることのほうがはるかに悪い ―― ここは正本を書く道である。
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from typing import Any

import yaml

from arp4 import yamlio

#: 最上位のレコードの始まり（``- `` は列 0）。
_RECORD = re.compile(r"^- ")

#: コメントだけの行。
_COMMENT = re.compile(r"^\s*#")

#: ``キー: 値`` の行（``- `` 付きの 1 行目も受ける）。
_FIELD = re.compile(r"^(?:- |\s+)([A-Za-z_][\w-]*): (.+)$")

#: 行末のコメント。**``#`` の前に空白が要る**（YAML の規則そのもの）。
_TRAILING = re.compile(r"^\s+#\s?(.*)$")

#: 行末に註を付けられない値。ブロックスカラ・アンカー・参照の後ろに ``#`` を
#: 置くと YAML が壊れる（読み戻しで捨てられるが、そもそも作らない）。
_UNCOMMENTABLE = ("|", ">", "&", "*")


@dataclass
class Comments:
    """1 ファイルから拾ったコメント。**空なら素の書き出しと同じ結果になる。**"""

    header: list[str] = field(default_factory=list)
    before: dict[str, list[str]] = field(default_factory=dict)
    inline: dict[tuple[str, str], str] = field(default_factory=dict)

    def __bool__(self) -> bool:
        return bool(self.header or self.before or self.inline)


def key_of(record: dict[str, Any]) -> str:
    """レコードの鍵。アイテムは ``id``、関係は ``種別|起点|終点``。

    :func:`arp4.spec.relation_key` と同じ取り方である ―― **別の取り方をすると、
    書き戻しが守る単位と検査が見る単位がずれる。**
    """
    if record.get("id"):
        return str(record["id"])
    return "|".join(str(record.get(k) or "") for k in ("type", "from", "to"))


def read(text: str, records: list[dict[str, Any]]) -> Comments:
    """既にあるファイルの本文からコメントを拾う。

    ``records`` は**そのファイルを読んだ結果**である。値を突き合わせるのに要る
    ―― 行末の ``#`` が註なのか値の一部なのかは、値を知らないと決められない。
    """
    found = Comments()
    lines = text.splitlines()
    starts = [i for i, line in enumerate(lines) if _RECORD.match(line)]
    if not starts:
        return found

    # **ファイルへの申し送りと、1 件目への申し送りは別である。** 見分けは空行で
    # 付ける ―― レコードに続いている塊はそのレコードのもの、間が空いていれば
    # ファイルのものである。両方に入れると、書き戻すたびに 1 つずつ増える。
    attached = _block(lines, 0, starts[0])
    found.header = [line for line in lines[:starts[0] - len(attached)]
                    if _COMMENT.match(line)]
    bounds = list(zip(starts, starts[1:] + [len(lines)]))
    for index, (record, (start, end)) in enumerate(zip(records, bounds)):
        key = key_of(record)
        # 直前の塊は**そのレコードの持ち物**である。探す範囲は 1 つ前のレコードの
        # 始まりから下で、空行で切れていたらそこから下だけを持つ。
        above = _block(lines, starts[index - 1] if index else 0, start)
        if above:
            found.before[key] = above
        for line in lines[start:end]:
            name, note = _inline(line, record)
            if note:
                found.inline[(key, name)] = note
    return found


def _block(lines: list[str], start: int, end: int) -> list[str]:
    """``lines[start:end]`` の**末尾に続くコメント行**だけを取る。

    空行が入っていたらそこで切る ―― 離れて置かれた塊は、下のレコードのもの
    ではなく**その位置に置かれたもの**である（ファイルの頭なら申し送り）。
    """
    block: list[str] = []
    for line in reversed(lines[start:end]):
        if _COMMENT.match(line):
            block.insert(0, line)
            continue
        break                          # 空行でも切る（離れた塊は下のものではない）
    return block


def _inline(line: str, record: dict[str, Any]) -> tuple[str, str]:
    """行末の註を取る。**値の一部なら取らない。**

    判定は「その欄の値を書き出したものが、行の先頭に一致するか」で行う ――
    値に ``#`` が入っていれば YAML は引用符を付けるので、そちらは一致しない。
    正規表現で ``#`` を探すだけだと、``名前: 受注 # 1`` のような値を註と
    読み違えて**資料の字を落とす。**
    """
    found = _FIELD.match(line)
    if not found:
        return "", ""
    name = found.group(1)
    if name not in record:
        return "", ""
    rendered = yamlio.dumps({name: record[name]}).rstrip("\n")
    if rendered.count("\n"):                   # 複数行の値には付けない
        return "", ""
    body = line.lstrip("- ").lstrip()
    if not body.startswith(rendered):
        return "", ""
    tail = _TRAILING.match(body[len(rendered):])
    return (name, tail.group(1)) if tail else ("", "")


def render(records: list[dict[str, Any]], comments: Comments) -> str | None:
    """コメントを戻した本文。**戻せなければ ``None``**（素の書き出しに任せる）。

    レコード 1 件ずつ書き出して組む ―― まとめて書き出したものを切り分けるより、
    どこがどのレコードかが確実である（1 件ずつでも出力は 1 字も変わらない）。
    """
    if not comments:
        return None
    out: list[str] = list(comments.header)
    for record in records:
        key = key_of(record)
        out += comments.before.get(key, [])
        block = yamlio.dumps([record]).rstrip("\n").splitlines()
        out += [_attach(line, record, comments.inline.get(
            (key, _name_of(line)), "")) for line in block]

    text = "\n".join(out) + "\n"
    # **読み戻して検算する。** 1 件でも違えば捨てる ―― コメントを保てないより、
    # 値が変わることのほうがはるかに悪い。
    try:
        if yaml.safe_load(text) != records:
            return None
    except yaml.YAMLError:
        return None
    return text


def _name_of(line: str) -> str:
    found = _FIELD.match(line)
    return found.group(1) if found else ""


def _attach(line: str, record: dict[str, Any], note: str) -> str:
    if not note:
        return line
    found = _FIELD.match(line)
    if not found or found.group(2).startswith(_UNCOMMENTABLE):
        return line
    return f"{line}  # {note}"


def dump(path: Any, records: list[dict[str, Any]]) -> bool:
    """コメントを保って書き出す。**保てなければ素の書き出しに落ちる。**

    戻り値は「保てたか」―― 呼ぶ側が件数を言えるようにしてある（黙って落ちると、
    保てているのか落ちているのかが利用者から見えない）。

    既にあるファイルは**その場でもう一度読む**。書き出そうとしているレコードは
    採番などで既に書き換わっていることがあり、そちらと突き合わせると行末の註が
    どの欄のものか決まらない ―― 拾うのは**ディスクに書いてある姿**からである。
    """
    path = path.with_suffix(yamlio.EXT)
    if not path.is_file():
        yamlio.dump(path, records)
        return False
    try:
        text = path.read_text(encoding="utf-8")
        found = read(text, yaml.safe_load(text) or [])
    except (OSError, yaml.YAMLError):
        yamlio.dump(path, records)
        return False

    rebuilt = render(records, found)
    # **書く道の側でももう一度検算する。** 検算を :func:`render` の中だけに置くと、
    # そこを直した誰かが外したときに**正本が黙って壊れる** ―― ここは値を書く道
    # なので、番人は 2 つあってよい。
    if rebuilt is None or not _same(rebuilt, records):
        yamlio.dump(path, records)
        return False
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(rebuilt, encoding="utf-8", newline="\n")
    return True


def _same(text: str, records: list[dict[str, Any]]) -> bool:
    """組み直した本文が**元のレコードそのもの**か。"""
    try:
        return yaml.safe_load(text) == records
    except yaml.YAMLError:
        return False


def present(path: Any) -> bool:
    """そのファイルが**コメントを持っているか**（書き戻す前に聞く）。

    書いたあとに聞くと、落ちたばかりの結果を見て「元から無かった」と言うことに
    なる ―― 数え落としのほうが、数え過ぎより静かに間違う。
    """
    try:
        text = path.with_suffix(yamlio.EXT).read_text(encoding="utf-8")
    except OSError:
        return False
    return any(_COMMENT.match(line) or _TRAILING.search(line)
               for line in text.splitlines())
