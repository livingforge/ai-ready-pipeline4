"""② 整理結果 ―― **唯一、意味を判断する層**の成果物を読む。

書くのはエージェントである（コマンドではない）。ここが読むのは書かれたものだけで、
**推測も補完もしない。**

パース結果 1 ファイル → 整理結果 1 ファイルで対応する::

    rounds/2026-08-02/parsed/資料/A/基本設計書.xlsx/受注テーブル.md
    rounds/2026-08-02/organized/資料/A/基本設計書.xlsx/受注テーブル.yml

対応が 1:1 なので、書き込み競合が構造的に起きず、未整理のものが一目で分かり、
1 資料の再整理が局所的に済む。``source`` に書くのは**アンカーだけ**でよい
（ファイルは対応から決まる）。
"""

from __future__ import annotations

from dataclasses import dataclass, field
from fnmatch import fnmatch
from pathlib import Path
from typing import Any, Iterator

from arp4 import mdio, shape, yamlio
from arp4.finding import Finding
from arp4.paths import Round
from arp4.yamlio import YamlError

#: 整理②の出力（レコードではない）。
SPECIAL = ("_concepts", "_metamodel-add")


@dataclass(frozen=True)
class Ref:
    """本文から読み取った関係。**推測で張らない。**"""

    rel: str
    to: str                              # 相手の concept
    attrs: dict[str, Any] = field(default_factory=dict)
    #: 関係そのものについて資料が書いていること（「呼び出す目的」等）。
    #: **属性ではなく関係の本文**である ―― 語彙に受け皿を作るには相手ごとに
    #: 属性を足すことになり、資料が 1 列で書いているものが語彙側で散る。
    #: 実測（r001）で、モジュール一覧の「呼び出す目的」11 本（`BillingCloseBatch
    #: → TaxCalculator：請求単位の消費税額を計算する` 等）が**書く場所が無くて
    #: 丸ごと落ちていた** ―― しかもこの 1 本は課題 ISS-016（消費税の計算単位）の
    #: 証拠そのものである（→ 決定 72）。
    note: str = ""
    line: int = 0                        # この関係が書かれた行

    # 関係はレコード 1 件に何本でもぶら下がる（実測 20 本超）。``G003``（相手が
    # 実在しない）も ``G012``（向きが宣言と合わない）も**その 1 本の話**なので、
    # レコードの先頭行を指すと読み手が目で探し直すことになる。


@dataclass
class Record:
    """整理済みのレコード 1 件。

    形は 2 つある。**完全なレコード**（``type`` / ``name`` / ``statement`` が揃って
    いる）が concept を定義し、**参照だけのレコード**（3 つとも省いたもの）は
    「このアンカーも同じ concept の話をしている」とだけ言う。

    後者を許すのは、日本の設計書が**同じ対象を複数シートに書く**ためである。
    画面「受注入力」は画面一覧・機能一覧・画面仕様書・項目一覧の 4 シートに現れ、
    アンカーはレコードでしか解決できない（``G001``）。3 つを必須にしていた頃は
    4 シートぶんの ``statement`` を書かされ、**食い違うのが当たり前**になって
    ``B023``（長いほうを採る）が構造的に量産された ―― しかも「長いほう」は
    索引一覧の説明がテーブルの役割を押しのける程度には当てにならなかった。
    """

    concept: str
    anchor: str
    file: str                            # organized からの相対（拡張子なし）
    index: int
    type: str = ""
    name: str = ""
    statement: str = ""
    attrs: dict[str, Any] = field(default_factory=dict)
    refs: tuple[Ref, ...] = ()
    #: **「調べたうえで相手がいない」の宣言**。関係型・属性名 → ``{reason, at}``
    #: で、``build`` が正本の ``known_gaps`` へそのまま引き継ぐ（→ :mod:`arp4.gaps`）。
    #:
    #: ここが無いあいだ、整理層には**相手を探し尽くしたと言う場所が無かった** ――
    #: 正本の ``known_gaps`` は ``build`` を打った人の欄だが、分担しているときは
    #: 配る側が ``build`` を禁じる（凍結済みのラウンドを勝手に組み立てさせない）
    #: ので、担当は ``G020`` を warn のまま残して報告文に書くしかない。実測
    #: （8 冊 / 8 分担 × 2 周）で 80 件超が 2 周とも warn のまま親へ渡り、
    #: 8 人中 4 人が「宣言する場所が無い」と独立に報告した。
    known_gaps: dict[str, dict[str, Any]] = field(default_factory=dict)
    path: str = ""                       # プロジェクト根からの相対（拡張子つき）
    line: int = 0                        # このレコードが始まる行
    lines: dict[str, int] = field(default_factory=dict)   # 欄ごとの行

    # ``file`` と ``path`` は**役割が違う**ので畳まない。``file`` はパース結果との
    # 1:1 対応を決める鍵（拡張子が無いのはそのため）で、出典として正本にも載る。
    # ``path`` は指摘を出すための実ファイルの位置で、**開くためだけ**にある。
    # 1 つにすると、拡張子が ``.yaml`` の資料が混ざった瞬間に対応が切れる。

    @property
    def target(self) -> str:
        return f"{self.file}[{self.index}]"

    def line_of(self, key: str) -> int:
        """欄 1 つの行。**無ければレコードの先頭。**

        指摘は「``type`` が語彙に無い」のように**欄を名指しする**ので、行も欄を
        指せるほうがよい。レコードは 20 行を超えることがあり、先頭を指されると
        名指しされた欄を目で探し直すことになる。
        """
        return self.lines.get(key) or self.line

    @property
    def subject(self) -> str:
        """指摘の ``target`` に載せる「何の話か」。**アンカーで言う。**

        ``file[index]`` は位置を持たせる前の名残で、``path``/``line`` があるいまは
        位置の劣化版でしかない。読み手が次に開くのはパース結果の**アンカー**である。
        """
        return self.anchor or f"records[{self.index}]"

    @property
    def complete(self) -> bool:
        """concept を定義するレコードか（``type`` / ``name`` / ``statement`` あり）。"""
        return bool(self.type and self.name and self.statement)


#: ``out_of_scope`` の区分。**値はスキーマが持つ**（ここは引くだけ）。
#:
#: 検査の表と ``arp4 declare --kind`` の選択肢が別々に書かれていると、**CLI が
#: 自分の書いたものを自分で拒否する** ―― 一方に足してもう一方に足し忘れた瞬間に
#: そうなる。`declare` は既に同じ形の事故（`G015`）を起こしている。
SCOPE_KINDS = tuple(
    str(v) for v in
    ((shape.load().get("shapes") or {}).get("out_of_scope") or {})
    .get("keys", {}).get("kind", {}).get("values", []))

#: 既定の区分。
SCOPE_DEFAULT = SCOPE_KINDS[0]

#: 機械が読めていないだけの区分。**「資料に無い」と「機械が読めていない」は別物**で、
#: 前者は次のラウンドでも変わらないが、後者は**拾い直す対象**として残る（xlsx の
#: 図形で描かれた業務フロー・ER 図・画面レイアウト）。**どの値がどちらの意味かは
#: arp4 の都合**なので、値の一覧（スキーマ）とは別にここで名前を付ける。
UNREADABLE = "未読取"


@dataclass
class OutOfScope:
    """仕様にならないと宣言されたアンカー。**黙って飛ばさず理由を持つ。**"""

    anchor: str
    reason: str
    file: str
    kind: str = SCOPE_DEFAULT
    path: str = ""                       # プロジェクト根からの相対（拡張子つき）
    line: int = 0

    @property
    def unreadable(self) -> bool:
        """機械が読めていないだけ（資料に情報が無いわけではない）。"""
        return self.kind == UNREADABLE


@dataclass
class Organized:
    """ラウンド 1 つぶんの整理結果。"""

    records: list[Record] = field(default_factory=list)
    out_of_scope: list[OutOfScope] = field(default_factory=list)
    concepts: dict[str, Any] = field(default_factory=dict)
    metamodel_add: dict[str, Any] = field(default_factory=dict)
    files: list[str] = field(default_factory=list)
    #: ``files`` の要素 → プロジェクト根からの相対パス（指摘に載せる位置）。
    #: **レコードが 1 件も無いファイルにも位置が要る** ―― 対象外宣言だけの
    #: ファイルが孤児になったとき、指す先が無いと「どれを消すか」が言えない。
    locations: dict[str, str] = field(default_factory=dict)
    #: ``arp4 draft`` が生成したファイル（根に ``drafted:`` の印がある）。
    #: 抽出的文章の検査（``G027``）はここにだけ掛かる ―― 人が最初から書いた
    #: 整理結果に、draft の文章契約を後から強いない。
    drafted: set[str] = field(default_factory=set)
    #: 実際に読んだ予約名（``_concepts.yml`` / ``_metamodel-add.yml``）。
    #: **``files`` には入らない**（レコードではないので数えようがない）が、
    #: 読んだこと自体は出力に要る ―― 実測で `arp4 lint _concepts.yml` が
    #: 「整理結果 0 ファイル / レコード 0 / error 0 / warn 0」とだけ返し、
    #: 検査したのか素通りしたのかが**打った人から区別できなかった。**
    special: list[str] = field(default_factory=list)

    def __iter__(self) -> Iterator[Record]:
        return iter(self.records)

    @property
    def claimed(self) -> set[tuple[str, str]]:
        """``(ファイル, アンカー)`` の集合。**網羅の検査に使う。**"""
        return ({(r.file, r.anchor) for r in self.records}
                | {(o.file, o.anchor) for o in self.out_of_scope})


def load(round_: Round,
         only: Iterable[Path] | None = None) -> tuple[Organized, list[Finding]]:
    """``organized/`` を読む。**契約違反はここで全部数える。**

    ``only`` を渡すと、そのファイルだけを読む（``arp4 lint`` が使う）。整理結果は
    パース結果と 1:1 なので、**1 ファイルだけで決まる指摘は 1 ファイルだけ読めば
    出せる** ―― 200 ファイルの資料で 1 ファイルを直すたびに全部を読み直すのは、
    書いている最中に回せる速さではない。

    横断が要る指摘（``G001`` 未整理・``G003`` concept 実在）は**この読み方では
    出せない**。出せないものを出せるふりをしないのが要点で、``freeze`` との
    違いはそこにしかない（→ :func:`arp4.freeze.lint`）。
    """
    result = Organized()
    findings: list[Finding] = []
    directory = round_.organized
    if not directory.is_dir():
        return result, findings

    selected = None if only is None else {Path(p).resolve() for p in only}
    for path in yamlio.scan_tree(directory):
        if selected is not None and path.resolve() not in selected:
            continue
        stem = path.stem
        relative = path.relative_to(directory).with_suffix("").as_posix()
        location = path.relative_to(round_.root).as_posix()
        try:
            data, marks = yamlio.load_marked(path)
        except YamlError as exc:
            # **1 件ずつ止めない。** 200 ファイルあると「直す → また落ちる」を
            # 200 回繰り返すことになる。壊れている場所は全部まとめて出す。
            findings.append(Finding("error", "G014", "",
                                    f"YAML として壊れています。{exc.detail}",
                                    file=location, line=exc.line, hint=_BROKEN_HINT))
            continue

        if stem in SPECIAL:
            data = data or {}
            result.special.append(path.name)
            if stem == "_concepts":
                # **記録ファイルとして素通りさせない。** レコードには schema と
                # lint があるのに `_concepts.yml` だけ検査手段が無く、書いた内容が
                # 効いたか分かるのは build を打った後だった（→ schemas/concepts.yml）。
                findings += shape.check(data, marks, location,
                                        name="concepts").findings
                result.concepts = data if isinstance(data, dict) else {}
            else:
                # **隣のファイルだけ素通りさせない。** `freeze` が読むのは
                # `add_item_types` の 1 節だけで、`add_attributes:` などは
                # error も warn も無いまま読み飛ばされていた（実測で 3 ロットが
                # 独立に踏んだ）→ schemas/metamodel-add.yml
                findings += shape.check(data, marks, location,
                                        name="metamodel-add").findings
                result.metamodel_add = data if isinstance(data, dict) else {}
            continue
        # **予約名は `organized/` の直下にしか無い。** 深いところまで「`_` で
        # 始まるものは飛ばす」を効かせていたので、`__main__.py` や `__init__.py`
        # の整理結果が**黙って読み飛ばされていた** ―― アンカーは永久に未整理の
        # まま残り、`freeze` は「整理も対象外宣言もされていません」と言い続ける。
        # 書いた本人からは正しく書いたようにしか見えないので、いちばん追えない。
        #
        # ただし **`__main__.py` / `__init__.py` は Python のほぼ全パッケージが
        # 持つ名前**である。整理結果はパース結果と名前で 1:1（:func:`parsed_path`）
        # なので相方は `organized/__main__.py.yml` しかありえず、ここで飛ばすと
        # **書きようがなくなる** ―― `arp4 declare` すら「1 ファイルを書きました」と
        # 報告したあとに読み飛ばされ（:func:`plan_declare` は同じ名前で書く）、
        # アンカーは永久に未整理のまま残る。**CLI が自分の書いたものを自分で
        # 拒否する**ので、書いた本人からは何が悪いのか分からない。
        #
        # **相方のパース結果があるなら資料の整理結果である。** 判定を名前の形から
        # 「元の資料がそこにあるか」へ移す ―― 資料名は資料の都合で決まるので、
        # こちらの命名規則で弾いてよいものではない。
        #
        # 相方の無い直下の `_` 付きは**飛ばすが必ず言う**（`_concept.yml` のような
        # 打ち間違いなら直せる）。
        #
        # **コードは `G032` である**（決定 23 では `G015` だった）。`G015` は
        # `freeze` の「絵があるのに `未読取`」が同じ綴りを使っており、**どちらも
        # warn なので段でも見分けが付かなかった** ―― `P003` が「非表示のシート」と
        # 「節の見出しが ASCII だけ」の 2 つを持っていたのと同じ形である（決定 78）。
        # 引いた読み手が別の指摘の直し方を読むので、**後から入ったこちらを移した**
        # （→ 決定 102）。
        if (path.parent == directory and stem.startswith("_")
                and not parsed_path(round_, relative).is_file()):
            findings.append(Finding(
                "warn", "G032", "",
                f"予約名ではないので読みませんでした（予約名は {' / '.join(SPECIAL)}）"
                "。資料の整理結果なら `_` で始まらない名前にしてください",
                file=location))
            continue
        if stem.startswith("."):
            continue

        result.files.append(relative)
        result.locations[relative] = location
        if isinstance(data, dict) and data.get("drafted"):
            result.drafted.add(relative)
        if data is None:
            # 空ファイルは空である（**契約違反ではない**）。書き始める前に置き場
            # だけ作ることがあるので、ここを error にすると作業の順序を縛る。
            continue
        # **形の検査はスキーマが持つ。** ここは通ったものを組み立てるだけで、
        # 「どの欄が要るか」を 1 行も知らない（→ :mod:`arp4.shape`）。
        report = shape.check(data, marks, location)
        findings += report.findings
        if not report.ok:
            continue
        _records(result, data, report, relative, location, marks)
        _out_of_scope(result, data, report, relative, location, marks)

    return result, findings


#: ``G014`` に添えるヒント。**実測でいちばん多い 3 つ**だけを言う（→ docs/organized.md）。
_BROKEN_HINT = ("フロー記法（{ … }）の値に `:`＋空白・カンマ・`{` が入っていませんか"
                "（「（固定: 130010）」「1,200,000/年」「{0}を入力してください。」）"
                "。引用符で囲むかブロック記法にしてください")


def _records(result: Organized, data: dict[str, Any], report: shape.Report,
             relative: str, location: str, marks: yamlio.Marks) -> None:
    """スキーマを通ったレコードを組み立てる。**契約の判断はここでしない。**"""
    for index, record in report.kept(("records",), data.get("records")):
        line = marks.line("records", index)
        result.records.append(Record(
            concept=str(record.get("concept") or ""),
            anchor=str((record.get("source") or {}).get("anchor") or ""),
            file=relative, index=index,
            type=str(record.get("type") or ""),
            name=str(record.get("name") or ""),
            statement=str(record.get("statement") or ""),
            attrs=dict(record.get("attrs") or {}),
            refs=_refs(record, report, index, marks),
            known_gaps=_known_gaps(record, report, index),
            path=location, line=line or 0,
            lines={key: found for key in record
                   if (found := marks.line("records", index, key)) is not None}))


def _refs(record: dict[str, Any], report: shape.Report, index: int,
          marks: yamlio.Marks) -> tuple[Ref, ...]:
    kept = report.kept(("records", index, "refs"), record.get("refs"))
    # 落ちた要素のぶんを詰めないので、``position`` はスキーマが見た添字と揃う
    # ―― 揃っていないと ``G012`` が別の関係の行を指す。
    return tuple(Ref(rel=str(entry["rel"]), to=str(entry["to"]),
                     attrs=dict(entry.get("attrs") or {}),
                     note=str(entry.get("note") or ""),
                     line=marks.line("records", index, "refs", position) or 0)
                 for position, entry in kept)


#: 正本の ``known_gaps`` へ運ぶ欄。**ここに無い欄は運ばない** ―― スキーマは
#: 見慣れない欄を通す（``unknown: ignore``）が、正本へ入れてよいかは別の話で、
#: :func:`arp4.gaps.check` が知らない欄を持ち込めば ``E018`` になる。
_GAP_KEYS = ("reason", "at")


def _known_gaps(record: dict[str, Any], report: shape.Report,
                index: int) -> dict[str, dict[str, Any]]:
    """レコードの ``known_gaps``。**形はスキーマが見た後**（落ちたものは来ない）。

    名前（関係型・属性名）が語彙にあるかは見ない ―― 語彙はメタモデルが持つので、
    判定は :func:`arp4.freeze._gap_names`（``G031``）の仕事である。
    """
    kept = report.kept_keys(("records", index, "known_gaps"),
                            record.get("known_gaps"))
    # 値は文字にして運ぶ ―― ``at: 2026-08-16`` を YAML は日付として読むので、
    # そのまま渡すと**正本の同じ欄に Python の date が混ざる**（人が正本側で
    # 書いた宣言は文字のこともあり、同じ欄の型が書いた人によって割れる）。
    return {name: {key: str(entry[key]) for key in _GAP_KEYS if entry.get(key)}
            for name, entry in kept}


def _out_of_scope(result: Organized, data: dict[str, Any], report: shape.Report,
                  relative: str, location: str, marks: yamlio.Marks) -> None:
    for index, entry in report.kept(("out_of_scope",), data.get("out_of_scope")):
        result.out_of_scope.append(OutOfScope(
            anchor=str(entry["anchor"]), reason=str(entry.get("reason") or ""),
            file=relative, kind=str(entry.get("kind") or SCOPE_DEFAULT),
            path=location, line=marks.line("out_of_scope", index) or 0))


# ── 一括の対象外宣言 ────────────────────────────────────────────
@dataclass
class Declaration:
    """1 ファイルぶんの対象外宣言。**まだ書いていない。**"""

    path: Path                           # 書き先（organized の yml）
    file: str                            # parsed からの相対（拡張子なし）
    anchors: list[str]
    data: dict[str, Any]                 # 書き戻す中身（既存を含む）
    existed: bool = False


def plan_declare(round_: Round, patterns: list[str], reason: str,
                 kind: str = SCOPE_DEFAULT) -> tuple[list[Declaration], list[Finding]]:
    """同じ構成のシートをまとめて対象外にする案を作る。

    表紙・改訂履歴は**資料の数だけ同じ宣言が要る**（25 冊なら 50 ファイル）。
    1 枚ずつ書くのは意味の判断ではなく作業なので、機械にやらせる。ただし
    **理由は人が与える** ―― 理由を機械が埋めたら、宣言は黙って飛ばすのと同じになる。
    """
    result, findings = load(round_)
    claimed = result.claimed
    plans: list[Declaration] = []

    for path in mdio.scan(round_.parsed):
        relative = path.relative_to(round_.parsed).with_suffix("").as_posix()
        if not _matches(relative, patterns):
            continue
        anchors = [a.id for a in mdio.read(path).anchors
                   if (relative, a.id) not in claimed]
        if not anchors:
            continue

        target = round_.organized / f"{relative}{yamlio.EXT}"
        loaded = yamlio.load(target) if target.is_file() else None
        data = dict(loaded) if isinstance(loaded, dict) else {}
        entry: dict[str, Any] = {"anchor": "", "reason": reason}
        if kind != SCOPE_DEFAULT:
            entry["kind"] = kind
        data["out_of_scope"] = list(data.get("out_of_scope") or []) + [
            {**entry, "anchor": anchor} for anchor in anchors]
        plans.append(Declaration(path=target, file=relative, anchors=anchors,
                                 data=data, existed=target.is_file()))
    return plans, findings


def write_declarations(plans: list[Declaration]) -> list[Path]:
    """案どおりに書く。**既存ファイルのコメントは失われる**（YAML の往復）。"""
    written: list[Path] = []
    for plan in plans:
        yamlio.dump(plan.path, plan.data)
        written.append(plan.path)
    return written


def _matches(relative: str, patterns: list[str]) -> bool:
    """パス全体でも、シート名だけでも当てられるようにする。"""
    name = relative.rsplit("/", 1)[-1]
    return any(fnmatch(relative, pattern) or fnmatch(name, pattern)
               for pattern in patterns)


def parsed_path(round_: Round, file: str) -> Path:
    """整理結果のファイル名 → 対応するパース結果。**1:1 対応が前提。**"""
    return round_.parsed / f"{file}{mdio.EXT}"


def organized_path(round_: Round, parsed: Path) -> Path:
    """パース結果 → 対応する整理結果。"""
    relative = parsed.relative_to(round_.parsed).with_suffix(yamlio.EXT)
    return round_.organized / relative


def yaml_files(round_: Round) -> list[Path]:
    """**レコードの**整理結果の実ファイル一覧。予約名も凍結マニフェストも外す。

    ``--fix`` が書き換えてよい対象と揃える ―― :func:`arp4.fix.repair` が知って
    いるのは ``records:`` の形だけなので、予約名（整理②の出力）を混ぜると
    「検査していないファイルを直す」ことになる。

    **``load`` が読む対象とは揃っていない。** ``load`` は予約名も読んで
    ``shape.check`` に掛ける（``result.special``）―― 検査する対象は
    :func:`lintable` のほうである。
    """
    directory = round_.organized
    return [path for path in yamlio.scan_tree(directory)
            if path.stem not in SPECIAL and not path.stem.startswith(".")]


def special_files(round_: Round) -> list[Path]:
    """予約名の実ファイル一覧（``_concepts.yml`` / ``_metamodel-add.yml``）。"""
    directory = round_.organized
    return [path for path in yamlio.scan_tree(directory)
            if path.stem in SPECIAL]


def lintable(round_: Round) -> list[Path]:
    """``arp4 lint`` が読む対象の全部。**予約名を含む。**

    ディレクトリを渡されたときの展開に使う。:func:`yaml_files` だけで展開して
    いたので、``arp4 lint <organized>`` は ``_concepts.yml`` を**一度も読まず**、
    ``G002``（``new`` の型が語彙に無い）も ``G021``（``assign`` の相手が台帳に
    無い）も黙って出なかった ―― 手順書はディレクトリ単位で回せと言っているので、
    運用上ここが既定の打ち方である。
    """
    return yaml_files(round_) + special_files(round_)
