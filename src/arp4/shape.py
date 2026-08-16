"""形の検査 ―― **宣言（``schemas/*.yml``）が正で、ここは実行するだけ。**

語彙（使ってよい種別・関係・属性）はメタモデルが持ち、**形**（どの欄が要るか・
どこに何を書くか）はスキーマが持つ。持ち主が違うので分けてある ―― 語彙は
プロジェクトが決め、形は arp4 が決める。

手続きで書いていたころ、整理結果の契約は :mod:`arp4.organized` の関数の中と
``docs/organize.md`` の例示の 2 か所にあった。**同じ規則が 2 か所にあると、同じ
問題が形を変えて戻る**（``G002`` が ``B013`` を取りこぼしていたのと同じ形）――
しかも例示は実行されないので、古くなっても誰も気づかない。

汎用のスキーマ言語にはしない。**いまある検査 1 つにつき構文 1 つ**しか用意せず、
JSON Schema も持ち込まない（依存は PyYAML 1 本のまま。``dependentRequired`` の
ような構文を使えても、``together`` 1 つで足りるものに 200 行の実装は釣り合わない）。

規律が 2 つある。

**error を出した要素は落とす。** その先を見ても、既に壊れているものへ重ねて
指摘を出すだけになる。手続き版の ``continue`` をそのまま宣言に移したものである。

**warn は落とさない。** 「書けているが置き場所が違う」（``G008``）を捨てると、
**資料が黙って消える** ―― 直せる形で残っているものを機械が捨ててよい理由が無い。
"""

from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from arp4 import yamlio
from arp4.finding import Finding

#: スキーマの置き場。``arp4 schema`` がそのまま出す。
SCHEMAS = Path(__file__).resolve().parent / "schemas"


@dataclass
class Report:
    """形の検査の結果。"""

    findings: list[Finding] = field(default_factory=list)
    #: error を出した要素の位置（``("records", 3)``）。**呼び出し側は組み立てない。**
    #: 根が落ちたときは ``()`` が入る。
    dropped: set[tuple[Any, ...]] = field(default_factory=set)

    @property
    def ok(self) -> bool:
        """根が形になっているか。**落ちていたら中身を読みに行かない。**"""
        return () not in self.dropped

    def kept(self, at: tuple[Any, ...], values: Any) -> list[tuple[int, Any]]:
        """落とさなかった要素を ``(添字, 値)`` で返す。``at`` はその配列の位置。

        添字を返すのは、**落ちた要素のぶんを詰めない**ためである。``records[3]``
        が落ちたときに 4 番目が 3 番になると、指摘の位置と整理結果の行が
        ずれて、直す人が別のレコードを見ることになる。
        """
        if not isinstance(values, list):
            return []
        return [(index, value) for index, value in enumerate(values)
                if at + (index,) not in self.dropped]

    def kept_keys(self, at: tuple[Any, ...], values: Any) -> list[tuple[str, Any]]:
        """``mapping`` の落とさなかった要素を ``(名前, 値)`` で返す。

        :meth:`kept` の連想配列版である。**書く側が名前を決める**節（``known_gaps``
        の関係型・属性名）は添字を持たないので、落ちたかどうかは名前で引く。
        """
        if not isinstance(values, dict):
            return []
        return [(str(name), value) for name, value in values.items()
                if at + (str(name),) not in self.dropped]


_CACHE: dict[str, dict[str, Any]] = {}


def load(name: str = "organized") -> dict[str, Any]:
    """スキーマを読む。**プロセスで 1 回**（読み込みごとに 200 回開かない）。"""
    if name not in _CACHE:
        _CACHE[name] = yamlio.load(SCHEMAS / f"{name}{yamlio.EXT}") or {}
    return _CACHE[name]


def names() -> list[str]:
    """あるスキーマの名前。**引数の候補を実体から出す** ―― 手で並べると、
    スキーマを足したときに `arp4 schema` からだけ引けないものができる
    （`metamodel-add.yml` は実際にそうなっていた）。
    """
    return sorted(path.stem for path in SCHEMAS.glob(f"*{yamlio.EXT}"))


def text(name: str = "organized") -> str:
    """スキーマの原文。``arp4 schema`` が出す ―― **書く側が読むのはこれ**である。

    畳んだ結果ではなく原文を出すのは、**なぜそう決めたかがコメントにしか
    書いていない**ためである（値だけ出すと「何を書けばよいか」は分かっても
    「なぜ 3 つ揃えるのか」が落ちる）。
    """
    return (SCHEMAS / f"{name}{yamlio.EXT}").read_text(encoding="utf-8")


def check(data: Any, marks: yamlio.Marks, location: str,
          name: str = "organized") -> Report:
    """1 ファイルの形を見る。"""
    schema = load(name)
    report = Report()
    if not _node(data, schema.get("root") or {}, (), "", schema,
                 location, marks, report):
        report.dropped.add(())
    return report


# ── 1 つの節 ────────────────────────────────────────────────────
def _node(value: Any, shape: dict[str, Any], path: tuple[Any, ...],
          inherited: str, schema: dict[str, Any], location: str,
          marks: yamlio.Marks, report: Report) -> bool:
    """節 1 つを見る。**error を出したら False**（呼び出し側が落とす）。"""
    line = marks.line(*path)
    subject = _subject(value, shape, path, inherited)

    if not isinstance(value, dict):
        _emit(report, shape.get("shape"), subject, location, line, path,
              value=repr(value))
        return False

    # 必須の欄は**宣言した順に**見て、1 つ落ちたらそこで止める。全部並べると
    # 「concept が無い」と「statement だけ無い」が同時に出て、打ち手が割れる。
    for block in shape.get("required") or []:
        keys = [str(k) for k in (block.get("keys") or [])]
        missing = [key for key in keys if not _dig(value, key)]
        if not missing:
            continue
        if block.get("together") and len(missing) == len(keys):
            continue                      # 3 つとも省いた ―― 参照だけのレコード
        _emit(report, block, subject, location, line, path,
              missing="、".join(missing), value=repr(value))
        return False

    declared = shape.get("keys") or {}
    for key, rule in declared.items():
        if key not in value:
            continue
        _key(value[key], rule or {}, path + (key,), subject, schema, location,
             marks, report)

    unknown = shape.get("unknown")
    if isinstance(unknown, dict):
        stray = sorted(set(value) - set(declared))
        if stray:
            # 見慣れない欄は**その欄の行**を指す（節の先頭ではない）。レコードは
            # 20 行を超えることがあり、先頭を指されると目で探し直すことになる。
            _emit(report, unknown, subject, location,
                  marks.line(*path, stray[0]), path,
                  keys="、".join(stray))
    return True


def _key(value: Any, rule: dict[str, Any], path: tuple[Any, ...],
         inherited: str, schema: dict[str, Any], location: str,
         marks: yamlio.Marks, report: Report) -> None:
    """宣言された欄 1 つ。``values``（決められた値）・``sequence``（要素の形）・
    ``mapping``（**名前は書く側が決める**連想配列の、値の形）。
    """
    values = rule.get("values")
    if values is not None and value not in values:
        _emit(report, rule, inherited, location, marks.line(*path), path,
              value=str(value), values="、".join(str(v) for v in values))
        return

    element = rule.get("sequence") or rule.get("mapping")
    if not element:
        return
    #: ``mapping`` は名前で引く（``known_gaps`` の関係型・属性名）。名前が語彙に
    #: あるかは**ここでは見ない** ―― 語彙はメタモデルが持ち、形はスキーマが持つ
    #: （持ち主が違うものを 1 つの宣言に混ぜない → :mod:`arp4.freeze` の ``G031``）。
    associative = not rule.get("sequence")
    if isinstance(value, dict if associative else list):
        keys: list[Any] = list(value) if associative else list(range(len(value)))
    else:
        _emit(report, rule, str(path[-1]), location, marks.line(*path), path,
              value=repr(value))
        return

    shape = (schema.get("shapes") or {}).get(element) or {}
    for key in keys:
        at = path + (str(key) if associative else key,)
        if not _node(value[key], shape, at, inherited, schema,
                     location, marks, report):
            report.dropped.add(at)


# ── 部品 ────────────────────────────────────────────────────────
def _dig(value: Any, dotted: str) -> Any:
    """``source.anchor`` のような入れ子を 1 つの名前で引く。

    入れ子の形をもう 1 段宣言すれば同じことはできるが、そうすると指摘が
    ``source`` の話になって「**必須の欄がありません: concept、source.anchor**」と
    1 件にまとめられなくなる ―― 直す人が見るのはレコード 1 件である。
    """
    for part in dotted.split("."):
        if not isinstance(value, dict):
            return None
        value = value.get(part)
    return value


def _subject(value: Any, shape: dict[str, Any], path: tuple[Any, ...],
             inherited: str) -> str:
    """指摘の「何の話か」。**アンカーで言えるなら必ずアンカーで言う。**"""
    declared = shape.get("subject")
    if declared == "inherit":
        return inherited
    if declared and isinstance(value, dict):
        found = _dig(value, str(declared))
        if found:
            return str(found)
    if declared == "":
        return ""
    return _where(path)


def _where(path: tuple[Any, ...]) -> str:
    """``("records", 3)`` → ``records[3]``。**名前が無いときの呼び名。**"""
    if not path:
        return ""
    if len(path) >= 2 and isinstance(path[-1], int):
        return f"{path[-2]}[{path[-1]}]"
    return str(path[-1])


def _emit(report: Report, rule: dict[str, Any] | None, subject: str,
          location: str, line: int | None, path: tuple[Any, ...],
          **values: str) -> None:
    """指摘 1 件。**コードも文言もスキーマが持つ**（実装は埋めるだけ）。"""
    if not rule:
        return
    message = str(rule.get("message") or "")
    for key, value in values.items():
        message = message.replace("{" + key + "}", value)
    report.findings.append(Finding(
        str(rule.get("level") or "error"), str(rule.get("code") or "G000"),
        subject, message, file=location, line=line))
