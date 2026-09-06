"""``arp4 design`` ―― 正本からプログラム設計の骨格を**決定的に**生成する。

``draft`` の鏡像である。あちらは**パース結果 → 整理結果**（コードの塊を読んで
モジュール・メソッドを起こす）で、こちらは**正本 → 整理結果**（詳細設計まで
揃ったアイテムを読んで、プログラム設計の欄と引数を起こす）。どちらも
「規則になるものは機械が実行し、LLM の仕事は日本語の文章化だけに絞る」という
同じ線引きに立っている。

生成するもの（すべて metamodel.yml「プログラム設計」の実装である）::

    module       language / source_path     出典のファイルの拡張子とパスの転記
    method       method_name / visibility   シグネチャの頭の転記
    parameter    引数 1 本 = 1 レコード      シグネチャの括弧の中を割る（4 言語）
    typed-as     引数 → データ項目・表・コード 型名が正本に **1 つだけ**当たるとき
    returns-type メソッド → 同上             同上
    query        CRUD 1 升 = 1 レコード      accesses の C/R/U/D を操作に開く

**決めないものは決めずに申告する**（``draft`` と同じ規律）。

* **実装規約は 1 件も作らない。** 言語も配置も命名も正本のどこにも書いておらず、
  作れば全部が作文になる ―― コーディング規約は**資料である**（規約の文書・
  ``.editorconfig``・既存コード）。無いなら「無い」と言うのが正しい出力で、
  ``parse`` に渡す先を指す。
* **エンドポイントも作らない。** 画面が叩く口の数は画面からは決まらない。
* **疑似コードも埋めない。** 手順の中身は原本にしか無い。

**同一入力に対して出力はバイト一致する。** 時刻・乱数・辞書順の揺れを含まない。
**書いたものは上書きしない** ―― 相方の生成物が既にあれば飛ばす。
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from arp4 import decisions, yamlio
from arp4.concepts import Concept
from arp4.paths import Round
from arp4.spec import Spec

#: 文章化スロット。``draft`` と同じ印で、``freeze`` の G026 が数える。
TODO = "<TODO 出典 {at}>"

#: ``draft`` が書いたファイルの印。``design`` の出力にも付ける ―― 文章化
#: スロットの lint（G027）は印のあるファイルにだけ掛かるので、同じ契約に乗せる。
MARK = "drafted"

#: 生成物の置き場（整理結果の下）。**パース結果と 1:1 の名前を使わない** ――
#: そこは ``draft`` と整理層のもので、``design`` が後から重ねると人が書いた
#: 文章を潰す。1 段掘って別の名前空間にする。
DIR = "_program"

#: 拡張子 → 実装言語。``module.language`` の enum に合わせる。**当てにいかない**
#: ―― 拡張子は資料のパスに書いてあり、推測が要らない唯一の手がかりである。
LANGUAGES = {".py": "Python", ".pyi": "Python",
             ".java": "Java",
             ".ts": "TypeScript", ".tsx": "TypeScript", ".mts": "TypeScript",
             ".js": "JavaScript", ".jsx": "JavaScript", ".mjs": "JavaScript",
             ".c": "C", ".h": "C"}

#: CRUD の 1 文字 → 問い合わせの操作。``accesses.crud`` は multi なので 1 升に
#: 複数入る（``CR`` は 2 本の問い合わせである）。
OPERATIONS = {"C": "INSERT", "R": "SELECT", "U": "UPDATE", "D": "DELETE"}

#: Java / TS の可視性キーワード。
_VISIBILITY = ("public", "protected", "private")

#: 型名から落とす飾り。``List<Order>`` の中身ではなく**外側**を見る ―― 引数が
#: 「受注の列」であることと「受注」であることは違うが、``typed-as`` が指せるのは
#: 相手 1 つなので、外側で当たらなければ張らない（中身で当てにいかない）。
_DECOR = re.compile(r"^(?:const\s+|final\s+|readonly\s+)+")

#: 引数の升を分けるときに数える括弧。
_OPEN, _CLOSE = "([{<", ")]}>"


@dataclass
class Designed:
    """生成 1 ファイルぶん。**まだ書いていない。**"""

    path: Path
    file: str                            # organized からの相対（拡張子なし）
    data: dict[str, Any]
    todo: int = 0

    @property
    def records(self) -> int:
        return len(self.data.get("records") or [])


@dataclass
class Result:
    """design 1 回ぶんの結果。"""

    designed: list[Designed] = field(default_factory=list)
    skipped: list[str] = field(default_factory=list)      # 既に書いてある
    decisions: list[dict[str, Any]] = field(default_factory=list)
    #: 決めなかったことの申告。**件数ではなく、次にどこへ行くかを言う。**
    notes: list[str] = field(default_factory=list)

    @property
    def todo(self) -> int:
        return sum(d.todo for d in self.designed)


# ── 入口 ────────────────────────────────────────────────────────
def plan(spec: Spec, round_: Round, concepts: dict[str, Concept]) -> Result:
    """正本とラウンドから生成計画を作る。**書き込みはしない。**"""
    result = Result()
    concept_of = {c.item: c.concept for c in concepts.values() if c.item}
    maker = _Maker(spec, concept_of, _anchors(round_))
    maker.run()

    for file in sorted(maker.buckets):
        target = round_.organized / DIR / f"{file}{yamlio.EXT}"
        relative = f"{DIR}/{file}"
        if target.is_file():
            result.skipped.append(relative)
            continue
        records = maker.buckets[file]
        result.designed.append(Designed(
            path=target, file=relative, data={MARK: True, "records": records},
            todo=sum(str(r.get("statement") or "").startswith("<TODO")
                     for r in records)))
    result.decisions = [e for e in maker.decisions
                        if e.get("target") not in set(result.skipped)]
    result.notes = maker.notes
    return result


def write(round_: Round, result: Result) -> list[Path]:
    """計画どおりに書き、決定ログを残す。"""
    written: list[Path] = []
    for designed in result.designed:
        yamlio.dump(designed.path, designed.data)
        written.append(designed.path)
    # ``draft`` と同じく**置き換え**（追記ではない）。何度でも回すので、回すたびに
    # 同じ判断が二重に積まれてはいけない ―― 飛ばしたファイルの判断は残す。
    made = {d.file for d in result.designed}
    decisions.replace(
        round_, "design", result.decisions,
        replaced=lambda e: str(e.get("target") or "") in made)
    return written


def _anchors(round_: Round) -> dict[str, tuple[str, str]]:
    """このラウンドの整理結果から **concept → (整理結果の相対名, アンカー)**。

    プログラム設計の欄には資料の出典が無い ―― 引数は「シグネチャにそう書いて
    ある」ので、**そのシグネチャを載せた塊**を出典にするのが正しい。塊の番地は
    ラウンドごとに決まるので、**このラウンドが持っている concept だけ**を対象に
    する（持っていないものは黙って落とさず、件数で申告する）。
    """
    found: dict[str, tuple[str, str]] = {}
    directory = round_.organized
    if not directory.is_dir():
        return found
    for path in sorted(yamlio.scan_tree(directory)):
        relative = path.relative_to(directory).with_suffix("").as_posix()
        # **凍結の目録も `records` を持つ**（件数の整数）。`organized.load` が
        # 隠しファイルを読まないのと同じ判定をここでも掛ける ―― 読むと
        # 「レコードの配列」のつもりで整数を歩くことになる。
        if (relative.startswith(f"{DIR}/") or path.name.startswith("_")
                or path.name.startswith(".")):
            continue
        data = yamlio.load(path)
        if not isinstance(data, dict):
            continue
        for record in data.get("records") or []:
            if not isinstance(record, dict):
                continue
            concept = str(record.get("concept") or "")
            anchor = str((record.get("source") or {}).get("anchor") or "")
            if concept and anchor and concept not in found:
                found[concept] = (relative, anchor)
    return found


# ── 生成 ────────────────────────────────────────────────────────
class _Maker:
    """正本 1 本ぶんの生成。状態は**この 1 回の中**にしか無い。"""

    def __init__(self, spec: Spec, concept_of: dict[str, str],
                 anchor_of: dict[str, tuple[str, str]]) -> None:
        self.spec = spec
        self.concept_of = concept_of
        self.known = set(concept_of.values())
        self.anchor_of = anchor_of
        self.by_id = spec.by_id
        self.buckets: dict[str, list[dict[str, Any]]] = {}
        self.decisions: list[dict[str, Any]] = []
        self.notes: list[str] = []
        self.types = _TypeIndex(spec, concept_of)
        self.missing: set[str] = set()   # このラウンドに出典の無い concept

    # ── 道具 ────────────────────────────────────────────────────
    def _place(self, file: str, record: dict[str, Any]) -> None:
        self.buckets.setdefault(file, []).append(record)

    def _decide(self, file: str, what: str, why: str, confidence: str) -> None:
        entry = decisions.entry("design", what, why, confidence)
        entry["target"] = f"{DIR}/{file}"    # 置き換えの単位（生成したファイル）
        self.decisions.append(entry)

    def _seat(self, item: dict[str, Any]) -> tuple[str, str, str] | None:
        """アイテム → ``(concept, 整理結果の相対名, アンカー)``。無ければ None。"""
        concept = self.concept_of.get(str(item.get("id")))
        if not concept:
            return None
        seat = self.anchor_of.get(concept)
        if seat is None:
            self.missing.add(concept)
            return None
        return concept, seat[0], seat[1]

    # ── 本体 ────────────────────────────────────────────────────
    def run(self) -> None:
        self._modules()
        self._queries()
        self._declare()

    def _modules(self) -> None:
        methods = {str(m.get("id")): m for m in self.spec.of_type("method")}
        owned: dict[str, list[str]] = {}
        for relation in self.spec.relations_of("has-method"):
            owned.setdefault(str(relation.get("from")), []).append(
                str(relation.get("to")))

        for module in sorted(self.spec.of_type("module"),
                             key=lambda i: str(i.get("id"))):
            seat = self._seat(module)
            if seat is None:
                continue
            concept, file, anchor = seat
            self._module_record(module, concept, file, anchor)
            language = _language_of(module)
            for method_id in sorted(owned.get(str(module.get("id")), [])):
                method = methods.get(method_id)
                if method is not None:
                    self._method(method, file, language)

    def _module_record(self, module: dict[str, Any], concept: str,
                       file: str, anchor: str) -> None:
        """**出典のパスの転記だけ。** 拡張子から言語、そのパスから出す先。"""
        source = _first_source(module)
        attrs: dict[str, Any] = {}
        language = LANGUAGES.get(Path(source).suffix.lower(), "") if source else ""
        if language and not module.get("language"):
            attrs["language"] = language
        if source and not module.get("source_path"):
            attrs["source_path"] = source
        if not attrs:
            return
        self._place(file, {"concept": concept, "source": {"anchor": anchor},
                           "attrs": attrs})
        self._decide(
            file,
            f"{concept} に {'・'.join(f'{k}={v}' for k, v in attrs.items())} を付けた",
            "出典のファイルのパスと拡張子の転記（推測ではない）", decisions.SURE)

    def _method(self, method: dict[str, Any], file: str, language: str) -> None:
        seat = self._seat(method)
        if seat is None:
            return
        concept, _where, anchor = seat
        signature = str(method.get("signature") or "")
        attrs: dict[str, Any] = {}

        name = _method_name(signature)
        if name and not method.get("method_name"):
            attrs["method_name"] = name
        visibility = _visibility(signature, name, language)
        if visibility and not method.get("visibility"):
            attrs["visibility"] = visibility

        refs: list[dict[str, Any]] = []
        target = self.types.resolve(str(method.get("returns") or ""))
        if target and not _has(self.spec, "returns-type", str(method.get("id"))):
            refs.append({"rel": "returns-type", "to": target})

        for index, token in enumerate(arguments(signature, language), start=1):
            child = self._parameter(concept, token, index, language, anchor,
                                    file, method)
            if child:
                refs.append({"rel": "has-parameter", "to": child})

        if not attrs and not refs:
            return
        record: dict[str, Any] = {"concept": concept,
                                  "source": {"anchor": anchor}}
        if attrs:
            record["attrs"] = attrs
        if refs:
            record["refs"] = refs
        self._place(file, record)

    def _parameter(self, owner: str, token: str, index: int, language: str,
                   anchor: str, file: str, method: dict[str, Any]) -> str:
        """引数 1 本 = 1 レコード。**シグネチャの升を割っただけ**である。"""
        parsed = split_argument(token, language)
        if parsed is None:
            return ""
        name, type_text, default, optional = parsed
        concept = f"{owner.replace('c-mtd-', 'c-prm-', 1)}.{name}"
        if concept in self.known:
            return concept                 # 既に正本にある（作り直さない）

        attrs: dict[str, Any] = {"param_name": name}
        if default:
            attrs["default_value"] = default
        if optional:
            attrs["optional"] = True
        refs: list[dict[str, Any]] = []
        target = self.types.resolve(type_text)
        if target:
            refs.append({"rel": "typed-as", "to": target})
        elif type_text:
            # **正本に相手が無い型だけを文字列で持つ**（``Path`` / ``list[str]``）。
            attrs["impl_type"] = type_text

        record: dict[str, Any] = {
            "concept": concept, "type": "引数", "name": name,
            "statement": TODO.format(at=_at(method, index)),
            "attrs": attrs, "source": {"anchor": anchor}}
        if refs:
            record["refs"] = refs
        self._place(file, record)
        return concept

    def _queries(self) -> None:
        """CRUD 1 升 = 問い合わせ 1 件。**``accesses`` を操作に開いただけ**である。

        どの列をどの条件で引くかは正本のどこにも無いので**作らない** ―― ここで
        作るのは「決めるべき問い合わせの一覧」であって、問い合わせそのものでは
        ない。開いた根拠（CRUD の升）は決定ログに残す。
        """
        for relation in sorted(self.spec.relations_of("accesses"),
                               key=lambda r: (str(r.get("from")),
                                              str(r.get("to")))):
            module = self.by_id.get(str(relation.get("from")))
            entity = self.by_id.get(str(relation.get("to")))
            if module is None or entity is None:
                continue
            if str(module.get("type")) != "module":
                continue                   # 処理単位からの CRUD は実装ではない
            seat = self._seat(module)
            target = self.concept_of.get(str(entity.get("id")))
            if seat is None or not target:
                continue
            concept, file, anchor = seat
            crud = relation.get("crud") or []
            for letter in [c for c in ("C", "R", "U", "D") if c in crud]:
                self._query(concept, target, entity, letter, anchor, file)

    def _query(self, owner: str, entity_concept: str, entity: dict[str, Any],
               letter: str, anchor: str, file: str) -> None:
        operation = OPERATIONS[letter]
        physical = str(entity.get("physical_name") or entity.get("name") or "")
        concept = (f"c-qry-{owner.replace('c-mod-', '', 1)}"
                   f".{physical}.{operation}")
        if concept in self.known:
            return
        self._place(file, {
            "concept": concept, "type": "問い合わせ",
            "name": f"{entity.get('name')}の{operation}",
            "statement": TODO.format(at=f"{physical} の {operation}"),
            "attrs": {"operation": operation},
            "refs": [{"rel": "queries", "to": entity_concept}],
            "source": {"anchor": anchor}})
        self._decide(
            file,
            f"{owner} → {physical} の CRUD「{letter}」を {operation} 1 件に開いた",
            "どの列をどの条件で引くかは正本に無いので、骨格だけを起こした",
            decisions.GUESS)

    def _declare(self) -> None:
        """**決めなかったことを申告する。** 件数ではなく、次にどこへ行くかを言う。"""
        if not list(self.spec.of_type("implementation-standard")):
            self.notes.append(
                "実装規約が正本に 0 件です。言語・配置・命名・型写像は正本の"
                "どこにも書いていないので design は作りません ―― コーディング規約は"
                "資料です（規約の文書・.editorconfig・既存コード）。"
                "arp4 parse に渡してから整理してください")
        if not list(self.spec.of_type("endpoint")):
            self.notes.append(
                "エンドポイントが正本に 0 件です。画面が叩く口の数は画面からは"
                "決まらないので design は作りません ―― API の資料"
                "（OpenAPI・IF 一覧）があれば parse に渡してください")
        blank = [i for i in self.spec.of_type("process-step")
                 if not i.get("pseudo")]
        if blank:
            self.notes.append(
                f"疑似コードの無い処理ステップが {len(blank)} 件あります。"
                "手順の中身は原本にしかないので design は埋めません")
        if self.missing:
            self.notes.append(
                f"このラウンドに出典の無い concept が {len(self.missing)} 件"
                "あります（別のラウンドで起こしたモジュール）。同じ資産を"
                "このラウンドへ arp4 parse すると起こせます ―― 引数の出典は"
                "シグネチャを載せた塊なので、番地の無いラウンドでは書けません")


# ── 型の照合 ────────────────────────────────────────────────────
class _TypeIndex:
    """型名 → concept。**1 つだけ当たるときしか答えない。**

    ``draft`` の取り込みの解決と同じ規律である ―― 同じ名前に候補が 2 つあるもの
    は関係を張らず、``impl_type`` の文字列として残す。**当てにいって半分間違える
    より、当てないほうがよい。**
    """

    def __init__(self, spec: Spec, concept_of: dict[str, str]) -> None:
        index: dict[str, set[str]] = {}
        for type_name in ("data-item", "entity", "code-master"):
            for item in spec.of_type(type_name):
                concept = concept_of.get(str(item.get("id")))
                if not concept:
                    continue
                for key in ("name", "physical_name"):
                    value = str(item.get(key) or "").strip()
                    if value:
                        index.setdefault(value.casefold(), set()).add(concept)
        self.index = index

    def resolve(self, text: str) -> str:
        bare = _bare_type(text)
        if not bare:
            return ""
        found = self.index.get(bare.casefold()) or set()
        return next(iter(found)) if len(found) == 1 else ""


def _bare_type(text: str) -> str:
    """型の飾りを落とす。**中身は見ない**（``List<Order>`` は ``List`` である）。"""
    bare = _DECOR.sub("", str(text or "").strip())
    for cut in ("<", "[", "("):
        if cut in bare:
            bare = bare.split(cut, 1)[0]
    return bare.strip().strip("*&? ").strip()


# ── シグネチャを割る ────────────────────────────────────────────
def _method_name(signature: str) -> str:
    """宣言の頭から呼ぶ名前を取る。**括弧の直前の識別子**である。"""
    head = str(signature or "").split("(", 1)[0]
    found = re.findall(r"[A-Za-z_$][A-Za-z0-9_$]*", head)
    return found[-1] if found else ""


def _visibility(signature: str, name: str, language: str) -> str:
    """公開範囲。**宣言に書いてあるものの転記**で、名前からの推測はしない。

    例外は Python / JavaScript / TypeScript の先頭 ``_`` と ``#`` である ――
    あれは規約ではなく**言語の慣習として宣言と同じ強さ**で読まれており、
    ``draft`` が「公開名だけ起こす」の判定に既に使っている。
    """
    head = str(signature or "").split("(", 1)[0]
    for keyword in _VISIBILITY:
        if re.search(rf"\b{keyword}\b", head):
            return keyword
    if language == "Java":
        return "package" if head.strip() else ""   # 修飾子なし ＝ パッケージ内
    if language == "C":
        return "package" if re.search(r"\bstatic\b", head) else "public"
    if not name:
        return ""
    if name.startswith("#") or name.startswith("__"):
        return "private"
    return "private" if name.startswith("_") else "public"


def arguments(signature: str, language: str) -> list[str]:
    """括弧の中を、**深さを見て**升に割る。"""
    text = str(signature or "")
    if "(" not in text:
        return []
    depth = 0
    inner = ""
    for char in text[text.find("("):]:
        if char in _OPEN:
            depth += 1
            if depth == 1:
                continue
        elif char in _CLOSE:
            depth -= 1
            if depth == 0:
                break
        inner += char

    out: list[str] = []
    depth = 0
    token = ""
    for char in inner:
        if char in _OPEN:
            depth += 1
        elif char in _CLOSE:
            depth -= 1
        if char == "," and depth == 0:
            out.append(token)
            token = ""
            continue
        token += char
    out.append(token)
    if language == "C" and [t.strip() for t in out] == ["void"]:
        return []                          # ``f(void)`` は引数 0 本である
    return [t for t in out if t.strip()]


def split_argument(token: str, language: str
                    ) -> tuple[str, str, str, bool] | None:
    """升 1 つ → ``(引数名, 型, 既定値, 省略可)``。割れなければ None。"""
    text = token.strip()
    if not text or text in ("*", "/", "..."):
        return None                        # 区切りそのもの（Python の ``*`` / ``/``）
    default = ""
    if "=" in text:
        head, default = text.split("=", 1)
        text, default = head.strip(), default.strip()

    if language in ("Python", "TypeScript", "JavaScript"):
        text = text.lstrip("*.")           # ``*args`` / ``**kw`` / ``...rest``
        name, _, type_text = text.partition(":")
        name, optional = name.strip(), False
        if name.endswith("?"):             # TS の省略可
            name, optional = name[:-1].strip(), True
        if not _identifier(name):
            return None
        return name, type_text.strip(), default, optional or bool(default)

    # Java / C ―― **最後の識別子が名前**で、その手前が型である。
    text = re.sub(r"@\w+(\([^)]*\))?", " ", text)      # 注釈を落とす
    text = text.replace("...", " ")                     # 可変長
    if language == "Java":
        # ``final`` は**引数の修飾子であって型ではない** ―― 落とさないと
        # ``impl_type`` に `final String` が残り、同じ型が 2 通りの綴りで並ぶ。
        # C の ``const`` は落とさない（``const char *`` は型そのものである）。
        text = re.sub(r"\bfinal\b", " ", text)
    parts = [p for p in re.split(r"\s+", text.strip()) if p]
    if not parts:
        return None
    last = parts[-1]
    name = last.lstrip("*&").rstrip("[]")
    type_text = " ".join(parts[:-1]) + ("*" if last.startswith("*") else "")
    if not _identifier(name):
        return None
    return name, type_text.strip(), default, bool(default)


def _identifier(name: str) -> bool:
    return bool(re.fullmatch(r"[A-Za-z_$][A-Za-z0-9_$]*", name or ""))


# ── 小道具 ──────────────────────────────────────────────────────
def _first_source(item: dict[str, Any]) -> str:
    for entry in item.get("source") or []:
        file = str((entry or {}).get("file") or "")
        if file:
            return file
    return ""


def _at(item: dict[str, Any], index: int) -> str:
    source = _first_source(item)
    return f"{source} の第 {index} 引数" if source else f"第 {index} 引数"


def _has(spec: Spec, relation: str, from_id: str) -> bool:
    return any(str(r.get("from")) == from_id for r in spec.relations_of(relation))


def _language_of(module: dict[str, Any]) -> str:
    """モジュールの言語。**宣言があればそれ、無ければ出典の拡張子。**"""
    declared = str(module.get("language") or "")
    if declared:
        return declared
    source = _first_source(module)
    return LANGUAGES.get(Path(source).suffix.lower(), "") if source else ""
