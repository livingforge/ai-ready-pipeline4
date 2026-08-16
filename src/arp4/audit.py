"""**出来上がった設計書のほうを検査する（``P1xx``）。**

**単独のコマンドではない** ―― ``arp4 check`` が全検査の一部として、``arp4 publish``
が生成する束と同じ母集合に対して回す（→ 決定 68 / 87）。

``publish.lint`` は既にあるが、見ているのは**文書定義**（どの章にどの列を出すか）
であって、**組み上がった表**ではない。だから次のことが起きた ―― いずれも規則は
先に書かれていたのに、規則を確かめる者がいなかった::

    publish.md:388 「列見出しは日本語」        → `7.1 business` `7.2 calculation` が出荷
    publish.md:345 「母集合そのものを並べない」 → トレーサビリティ §4 が 80 行すべて `―`
    CLAUDE.md 「資料に無いと読めていないを混ぜない」
                                              → 権限マトリクスの空欄が
                                                「不可」と「記載なし」の両方を表す

**規則を文章で強めても直らない。** レビューで指摘された 16 件のうち 14 件は
publish 層の出力の形の問題で、整理層（エージェント）の判断ミスは 3 件だった
―― プロンプトを厚くするのは、いちばん効かないところを厚くすることになる。

検査は :class:`~arp4.publish.Block`（描画の直前の、文字列まで落ちた章）に対して
行う。Markdown と HTML の両方を舐めるのではなく**共通の材料**を見るので、片方
だけ直って片方が腐る形にならない。

========  ==================================================================
``P101``  母集合をそのまま並べただけの表（結論の列が全行空）
``P102``  升目の凡例が無い（**空欄の意味が書かれていない**）
``P103``  節の見出しが ASCII だけ（enum の生値がそのまま出ている）
``P104``  同じ本文が何度も繰り返されている（表の外に出すべき定型文）
``P105``  争点（``disputes``）のあるアイテムが、印も無く並んでいる
``P106``  同じアイテムが複数の設計書に全文で重複している
``P107``  同じ種別なのに出典列を出す設計書と出さない設計書がある
``P108``  「未分類」の節が大きすぎる（分類が目次として働いていない）
``P109``  **禁止を書ける升目に、禁止が 1 件も無い**（写し漏れの疑い）
``P110``  正本にあるのに、どの様式も出していない関係型
``P111``  正本に値があるのに、**どの設計書の列にも出ない属性**
========  ==================================================================

``W046``（畳んだ列）だけは :mod:`arp4.publish` が出す ―― 判定の材料が
:func:`publish._trim` にあるので説明もあちらに置き、ここは**組み上げた章を
渡すだけ**である（束を 3 度組み立てないため）。

**番号が ``P1xx`` なのは、``parse`` の ``P0xx`` と衝突したからである。** どちらも
「紙（Paper）の側の指摘」のつもりで P を採ったが、``parse`` は既に ``P001``〜
``P014`` を使っており、**同じ ``P003`` が「非表示のシート」と「節の見出しが
ASCII だけ」の 2 つの意味を持っていた** ―― 配布ドキュメントでも `parse.md` が
前者を、`publish.md` が後者を、**同じ綴りで**説明していた。番号は「どのコマンドが
出したか」を覚えていないと引けない識別子になってしまい、`troubleshooting.md` を
引いた読み手は**別の指摘の直し方を読む。** 段（error/warn）が違えば別物だと
気づけるが、ここは両方 warn なので気づく手がかりも無い。

**段はすべて warn である。** ここが見ているのは「読み手が誤読しうる形」であって
データの不備ではない ―― error にすると ``--force`` を押す理由をまた作ってしまい、
:mod:`arp4.gate` が塞いだ穴を別の場所に開け直すことになる。
"""

from __future__ import annotations

import re
from collections import Counter, defaultdict
from collections.abc import Iterable
from typing import Any

from arp4 import metamodel as mm
from arp4 import publish as publish_module
from arp4 import sequence as sequence_module
from arp4.finding import Finding, order
from arp4.publish import Block
from arp4.spec import Spec

#: 空欄として扱う見た目（``_BLANK`` と揃える）。
_BLANK = {"", "―", "-", "—", "‐"}

#: 同じ本文が何回出たら「表の外へ出すべき」と言うか。実測（r001 の画面一覧）で
#: 共通方式の転記が 12 回、テーブル定義書の known_gaps 脚注が 10 回だった。
_REPEAT = 4

#: ``group_by`` が値を持たない行を集める節の見出し（``publish._items_blocks``）。
_UNCLASSIFIED = "未分類"

#: 「未分類」が章のうちこの割合を超えたら言う。実測（r001）で制約は 21/70 = 30%
#: で 2 番目に大きい節になり、業務要件は 14/44 = 32% だった。1/4 を超えると、
#: 目次から現在地を掴めないという判断（節の数が 4〜7 なので、平均を超える）。
_UNCLASSIFIED_RATIO = 0.25

#: 「全文で重複」と言うのに要る、一致する列の数。ID と名称だけの一致（索引）は
#: 重複ではない ―― 実測で、重複していた業務ルールは 6 列すべてが一致していた。
_SAME_COLUMNS = 3

#: 様式の ``columns`` には無く、**機械が末尾へ足す**列。列の顔ぶれを比べるとき
#: （:func:`_duplicated`）は数えない ―― 様式が選んだ列ではないので、「同じ列を
#: 選んでいる」の証拠にならない。「課題」は争点のある行がある表にだけ付く
#: （:func:`publish._dispute_marks`）。
_APPENDED_COLUMNS = frozenset({"課題"})

#: 節を持たないが、**別の形で設計書に出ている**関係型（→ :func:`_unpublished`）。
#: ``disputes`` は争点のある行に付く「課題」列として出る
#: （:func:`publish._dispute_marks`）ので、節が無いことは穴ではない。
_SURFACED_ELSEWHERE = {"disputes"}

#: 繰り返しとして数える最短の長さ。短い語（``PDF``・``必須``・パッケージ名・
#: メッセージ本文）は繰り返して当然なので、**文の長さ**で切る。実測で問題に
#: なった転記は 100 字前後、正当な繰り返しは 24〜40 字だった。
_REPEAT_MIN = 60

#: 自由記述の受け皿（:data:`_FREE_TEXT`）だけに当てる、下げた閾値。
#: **受け皿に同じ短い定型句が並ぶのは、enum の値が並ぶのとは別の話である** ――
#: 実測（r001）の「利用権限 営業・管理者」（11 字）は 13 画面の `補足` 列に
#: 並びながら 60 字に届かず、1 件も数えられていなかった。
_REPEAT_MIN_FREE = 10

#: 語彙が「何を書いてもよい」と宣言している欄。ここに定型句が並ぶのは転記である。
_FREE_TEXT = {"description", "note"}


def audit(spec: Spec, full: bool = False,
          names: Iterable[str] | None = None) -> list[Finding]:
    """設計書を組み立てて、**出来上がりを**検査する。

    ``names`` / ``full`` は :func:`publish.publish` と同じ意味で、**生成する束と
    同じ母集合を見る**ためにある。``--document`` で 1 冊だけ出したときに束ぜんぶを
    検査すると、その 1 冊の冒頭に**出していない設計書の件数**が刻まれる ――
    ゲートは「この生成物が通った条件」の記録なので、母集合がずれると読めなくなる。
    """
    definitions = publish_module.catalog(spec)
    wanted = set(names or [])
    if wanted:
        definitions = [d for d in definitions
                       if d.get("name") in wanted or d.get("title") in wanted]
    prepared: list[tuple[dict[str, Any], list[Block]]] = [
        (definition, publish_module._blocks(spec, definition, full))
        for definition in definitions]

    findings: list[Finding] = []
    for definition, blocks in prepared:
        title = str(definition.get("title") or definition.get("name"))
        for block in blocks:
            findings += _population(title, definition, block)
            findings += _legend(title, definition, block)
            findings += _negative_absent(spec, title, definition, block)
            findings += _ascii_heading(title, block)
        findings += _repeats(title, blocks)
        findings += _disputed(spec, title, blocks)
        findings += _unclassified(title, blocks)
    findings += _duplicated(prepared)
    findings += _trace_columns(prepared)
    findings += _unpublished(spec, prepared)
    findings += _unsurfaced(spec, prepared)
    findings += publish_module.folded(prepared)
    return order(findings)


def _section_of(definition: dict[str, Any], block: Block) -> dict[str, Any]:
    """その章を出した文書定義の節。

    ``group_by`` で割れた節は見出しが変わるので、**章の見出しで引けるとは限らない**
    ―― 引けなければ空を返し、節の宣言を要る検査は黙る（誤検出より取りこぼし）。
    """
    for section in definition.get("sections") or []:
        if str(section.get("heading") or "") == block.heading:
            return section
    return {}


def _section_kind(definition: dict[str, Any], block: Block) -> str:
    """その章の種別（``items`` / ``relation`` / ``matrix`` / ``trace``）。"""
    section = _section_of(definition, block)
    return str(section.get("kind") or "items") if section else ""


# ── P101 母集合をそのまま並べた表 ───────────────────────────────
def _population(title: str, definition: dict[str, Any],
                block: Block) -> list[Finding]:
    """結論の列が**全行空**なら、それは調べた結果ではなく母集合の写しである。

    トレースの表は空欄そのものが結論なので畳まない（``_trim`` の免除）。だが
    **100% 空は結論ではない** ―― 実測でトレーサビリティ・マトリクスの
    「要件 → テストケース」80 行と「モジュール → テストケース」18 行が全行 `―` で
    出た。同じ文書の末尾では「未検証の要件」が*対象データが無いので省略*されて
    おり、**同じ事実から逆の判断が 2 つ出ている。**

    見るのは**結論の列だけ**である。トレースの表の「種別」「名称」は母集合の
    属性であって結論ではないので、そこが埋まっていることは何の反証にもならない
    ―― 全列が空のときだけ言う作りにすると、この 98 行が素通りする。

    結論の列は ``linked``（``_trace_blocks`` が組み立てる列）で探す。**最終列と
    決め打ちにしない** ―― 章側は ``columns`` の並びを自由に決められるので、
    位置で当てると定義を並べ替えただけで検査が黙る。
    """
    if block.heading_only or len(block.rows) < 2:
        return []
    section = _section_of(definition, block)
    if str(section.get("kind") or "") != "trace":
        return []
    columns = [str(c) for c in (section.get("columns") or [])]
    if "linked" not in columns:
        return []                       # 対応を出していない章は結論を持たない
    index = columns.index("linked")
    if index >= len(block.columns):
        return []
    if not all(str(row[index]).strip() in _BLANK
               for row in block.rows if index < len(row)):
        return []
    return [Finding(
        "warn", "P101", f"{title} / {block.heading}",
        f"{len(block.rows)} 行すべてで「{block.columns[index]}」が空です"
        "（母集合をそのまま並べています）",
        hint="1 件も対応が無いなら、章ごと省略して理由を書く。"
             "対応があるはずなら、関係が張られていない（W030 / W031 を見る）")]


# ── P102 升目の凡例 ─────────────────────────────────────────────
def _legend(title: str, definition: dict[str, Any],
            block: Block) -> list[Finding]:
    """**空欄の意味が書かれていない対応表**を出さない。

    升は「関係がある」しか言えない。原典が ``○ / △ / ×`` の 3 値でも、関係の
    有無に写した時点で ``×``（不可）と「記載が無い」は同じ空欄になる ――
    実測（r001 の権限マトリクス）で ``△＝部長職のみ可`` は正本からも消え、
    ``×`` は空欄になり、凡例そのものも無かった。
    """
    if _section_kind(definition, block) != "matrix" or not block.rows:
        return []
    if any("空欄" in note for note in block.notes):
        return []
    return [Finding(
        "warn", "P102", f"{title} / {block.heading}",
        "升目の表に凡例がありません（空欄が何を意味するかが書かれていません）",
        hint="関係の有無しか言えないことを凡例に書く。"
             "「不可」を表したいなら、関係の属性か別の関係型で表す")]


# ── P109 否定を書ける升目に、否定が 1 件も無い ──────────────────
def _negative_absent(spec: Spec, title: str, definition: dict[str, Any],
                     block: Block) -> list[Finding]:
    """**禁止を書ける語彙があるのに、升に 1 件も出ていない**升目を言う。

    ``P102`` は凡例の有無を見るが、凡例があっても中身が空なことはある。実測
    （r001 の権限マトリクス）―― メタモデルに ``不可`` を足した（決定 71）あとも
    ``operates`` 38 本すべてが ``permission`` を持たず、原典の ``×`` 16 升は
    ふたたび消えた。凡例は :func:`publish._legend` が正直に書くようになったが、
    **正直な凡例は「落ちた」とは言わない** ―― 落ちたことを言うのはここである。

    **鳴っても誤りとは限らない。** 原典に禁止が 1 つも無ければこれが正しい姿で、
    そのときは黙って無視してよい ―― 機械は原典を見ていないので、
    「禁止が無い」と「禁止を写していない」の区別は付けられない。区別が付く者に
    渡すのがこの warn の仕事である（``W030`` と同じ立ち位置）。

    否定を宣言していない属性では黙る ―― 語彙が無いなら凡例が既にそう断っており、
    ここで重ねて言うと**語彙の穴と写し漏れが同じ音になる。**
    """
    if _section_kind(definition, block) != "matrix" or not block.rows:
        return []
    section = _section_of(definition, block)
    relation_type = str(section.get("relation") or "")
    cell_attribute = section.get("cell")
    if not relation_type or not cell_attribute:
        return []
    attribute = ((spec.metamodel.relation_types.get(relation_type) or {})
                 .get("attributes") or {}).get(cell_attribute) or {}
    negative = [str(v) for v in (attribute.get("negative") or [])]
    if not negative:
        return []
    for relation in spec.relations_of(relation_type):
        if relation.get("status") == "deprecated":
            continue
        value = relation.get(cell_attribute)
        values = value if isinstance(value, list) else [value]
        if any(str(v) in negative for v in values if v is not None):
            return []
    listed = "・".join(negative)
    return [Finding(
        "warn", "P109", f"{title} / {block.heading}",
        f"{listed} を書ける升目ですが、{relation_type}.{cell_attribute} に"
        f"{listed} が 1 件もありません",
        hint=f"原典が × や「不可」を書いているなら、関係を張って "
             f"{cell_attribute} に写す（張らないで表すと空欄が「不可」と"
             f"「記載なし」の 2 つを表す）。原典に禁止が無いならこのままでよい")]


# ── P103 節の見出しが ASCII だけ ────────────────────────────────
def _ascii_heading(title: str, block: Block) -> list[Finding]:
    """``group_by`` が enum の生値をそのまま見出しにしていないか。

    ``publish.md`` の「列見出しは日本語」は**列しか見ていなかった**。実測で
    基本設計書の業務ルールが ``7.1 business`` … ``7.7 未分類`` と英語と日本語の
    混在で出た。値そのものは訳さない（``E011`` が自分の出力を弾く）ので、
    直す場所はメタモデルの ``value_labels`` である。
    """
    heading = block.heading.strip()
    if block.level < 3 or not heading or not heading.isascii():
        return []
    if not re.fullmatch(r"[A-Za-z][A-Za-z0-9_\-]*", heading):
        return []                       # 番号・記号だけの見出しは別の話
    return [Finding(
        "warn", "P103", f"{title} / {heading}",
        f"節の見出しが enum の生値のままです: {heading}",
        hint="メタモデルの当該属性に value_labels を足す"
             "（値は訳さない。見出しだけ差し替える）")]


# ── P104 同じ本文の繰り返し ─────────────────────────────────────
def _repeats(title: str, blocks: list[Block]) -> list[Finding]:
    """定型文が表の中で何度も繰り返されていないか。

    実測で、画面一覧の補足「方式の 2 行は本冊子の N 画面すべてに同じ文が置かれて
    おり…」が 12 回出た（しかも N が 4 と 5 で揺れていた）。読み手が列を縦に
    読めなくなるうえ、**同じ文が並ぶと差のある行が埋もれる。**

    **数えるのは列である**（塊ではない）。塊ごとに数えていたころ、`group_by` で
    17 の節に割れた表では同じ定型文が節の数だけ分散し、**どの節でも閾値に
    届かなかった** ―― 実測（r001）で「利用権限 営業・管理者」は 13 画面の
    `補足` 列にあったのに 1 件しか数えられていない。列は節をまたいで 1 本なので、
    読み手が縦に読むときの単位も列である。

    **自由記述の列だけ閾値を下げる。** :data:`_REPEAT_MIN`（60 字）は enum の
    値や物理名のような**短い鍵**を落とすためのもので、そこは繰り返して当然で
    ある。だが受け皿（`description` / `note`）に同じ短い定型句が並ぶのは別の
    話で、そこは 60 字に届かないまま素通りしていた（:data:`_REPEAT_MIN_FREE`）。

    **出典列は数えない。** 同じシートから来た行が同じアンカーを出すのは当たり前で、
    そこを咎めると本物（補足・説明の定型文）が件数に埋もれる ―― 実測で、
    数えたときの 54 件のうち 45 件が出典列だった。
    """
    #: (列の並び, 列の位置) → その列の本文の出現数。**列の並びで束ねる**ので、
    #: `group_by` で割れた節は 1 本の列として数えられ、別の章とは混ざらない。
    counters: dict[tuple[tuple[str, ...], int], Counter[str]] = defaultdict(Counter)
    named: dict[tuple[tuple[str, ...], int], tuple[str, str]] = {}
    for block in blocks:
        shape = tuple(block.columns)
        for row in block.rows:
            for index, cell in enumerate(row):
                if index in block.source_columns or index >= len(shape):
                    continue
                text = str(cell).strip()
                leaf = (block.paths[index].rsplit(".", 1)[-1]
                        if index < len(block.paths) else "")
                floor = _REPEAT_MIN_FREE if leaf in _FREE_TEXT else _REPEAT_MIN
                if len(text) >= floor and text not in _BLANK:
                    counters[(shape, index)][text] += 1
                    named[(shape, index)] = (shape[index], block.heading)

    findings: list[Finding] = []
    for key, counter in counters.items():
        column, heading = named[key]
        # **列 1 件で言う。** 同じ列に定型文が 2 種類あっても打ち手は 1 つ
        # （その列を表の外へ出す）なので、種類の数だけ行を並べても増えない。
        repeated = [(text, count) for text, count in counter.most_common()
                    if count >= _REPEAT]
        if not repeated:
            continue
        text, count = repeated[0]
        others = (f"（ほか {len(repeated) - 1} 種）" if len(repeated) > 1 else "")
        findings.append(Finding(
            "warn", "P104", f"{title} / {heading}",
            f"列「{column}」に同じ本文が {count} 回出ています{others}: "
            f"{text[:40]}…",
            hint="行ごとに違わないものは表の外（章の前書きか脚注）へ"
                 "1 度だけ書く"))
    return findings


# ── P105 争点のあるアイテムに印が無い ───────────────────────────
def _disputed(spec: Spec, title: str, blocks: list[Block]) -> list[Finding]:
    """``disputes`` の相手が、印も無く並んでいないか。

    実測で、基本設計書の業務ルールには**互いに矛盾する 4 組**が並列に載った
    （引当のタイミング・消費税の計算単位・請求の締め日・受注取消の期限）。
    課題管理表では拾えているのに、当の基本設計書ではただの規則である ――
    読み手は両方を確定仕様として受け取る。関係は既に張ってあるので、
    足りないのは**印を出すこと**だけである。
    """
    disputed = {str(r.get("to")) for r in spec.relations_of("disputes")
                if r.get("status") != "deprecated"}
    if not disputed:
        return []
    display = {str(i.get("id")): _display_id(i) for i in spec.items}
    wanted = {display[i] for i in disputed if display.get(i)}
    if not wanted:
        return []

    seen: set[str] = set()
    for block in blocks:
        if block.id_column is None:
            continue
        for row in block.rows:
            if block.id_column < len(row):
                value = str(row[block.id_column]).strip()
                if value in wanted and not _marked(row):
                    seen.add(value)
    if not seen:
        return []
    listed = "・".join(sorted(seen)[:8])
    return [Finding(
        "warn", "P105", title,
        f"争点のあるアイテムが印無しで出ています（{len(seen)} 件）: {listed}",
        hint="課題（open-issue）へ飛べる印を行に出す。"
             "課題管理表にしか無いと、この設計書だけ読んだ人は確定仕様と受け取る")]


def _marked(row: list[str]) -> bool:
    return any("課題" in str(cell) or "ISS-" in str(cell) for cell in row)


def _display_id(item: dict[str, Any]) -> str:
    for key, value in item.items():
        if str(key).endswith("_id") and value:
            return str(value)
    return ""


# ── P108 「未分類」の節が大きすぎる ─────────────────────────────
def _unclassified(title: str, blocks: list[Block]) -> list[Finding]:
    """分類が目次として働いているか。

    ``organize.md`` は既に「**未分類のまま残すのは資料に区分が無いときだけ**」と
    書いていて、整理層はそれに従っている ―― 資料の側に区分が無いのは事実である。
    それでも**目次としては働いていない**: 実測（r001）で要件定義書の制約 70 件中
    21 件が「8.4 未分類」に入り、8.1 技術（40 件）に次ぐ 2 番目の節になった。
    中身の 12 件は「〜のドメイン」で、`データ項目` という区分で括れるものだった
    （``category`` は ``extensible`` なので、資料に無い区分を足す口はある）。

    **プロンプトを強めても直らない類の問題である** ―― 書いてある指示には従って
    いて、足りないのは「これで目次になっているか」を後から見る目である。だから
    件数で言う。閾値（:data:`_UNCLASSIFIED_RATIO`）を割るのは判断ではなく、
    「2 番目に大きい節が未分類」なら誰が見ても目次として弱いという事実である。
    """
    # **章ごとに数える。** 文書全体を分母にすると、要件定義書は制約 21 件を
    # 要件・機能要件・非機能要件を含む 200 行超で割ることになり、2 番目に大きい
    # 節が未分類でも 1 割を切って黙る。目次で隣に並ぶのは同じ章の節だけである。
    chapters: list[tuple[str, list[Block]]] = []
    for block in blocks:
        if block.level == 2:
            chapters.append((block.heading, []))
        elif block.level == 3 and block.rows and chapters:
            chapters[-1][1].append(block)

    findings: list[Finding] = []
    for chapter, sections in chapters:
        if len(sections) < 2:
            continue                    # 節に割っていない章は目次を作っていない
        total = sum(len(b.rows) for b in sections)
        for block in sections:
            if block.heading != _UNCLASSIFIED or not total:
                continue
            share = len(block.rows) / total
            if share < _UNCLASSIFIED_RATIO:
                continue
            findings.append(Finding(
                "warn", "P108", f"{title} / {chapter} / {block.heading}",
                f"「{_UNCLASSIFIED}」に {len(block.rows)} 件"
                f"（この章の {share:.0%}）が入っています"
                "（分類が目次として働いていません）",
                hint="資料に区分が無いなら、括れる区分を category に足す"
                     "（extensible なので語彙の宣言は要らない）。"
                     "本当に括れないものだけを未分類に残す"))
    return findings


# ── P106 同じアイテムが複数の設計書に全文で出る ─────────────────
def _duplicated(
        prepared: list[tuple[dict[str, Any], list[Block]]]) -> list[Finding]:
    """同じ行が 2 冊に丸ごと載っていないか。

    実測で、業務ルール 145 件が基本設計書「業務ルール」と詳細設計書「実装ルール」に
    **全文で重複**していた（詳細設計書だけにある規則は 0 件 ―― 完全な部分集合）。
    選定の基準は書かれておらず、片方を直したときにもう片方が古くなる。

    **同じ ID が 2 冊に出ること自体は咎めない。** トレーサビリティ・マトリクスは
    要件 80 件を並べるのが仕事で、あれは重複ではなく索引である ―― 見るのは
    **列の顔ぶれまで同じか**（:data:`_SAME_COLUMNS` 列以上が一致するか）で、
    索引は ID と名称しか共有しないので当たらない。

    **機械が足した列は顔ぶれに数えない**（:data:`_APPENDED_COLUMNS`）。
    「課題」は様式の ``columns`` には無く、争点のある行がある表に
    :func:`publish._dispute_marks` が末尾へ付ける印である ―― これを数えると
    索引の共有列が ID と名称の 2 つから 3 つに増え、**上の但し書きが崩れる。**
    実測（kotonoha r001）で、トレーサビリティ・マトリクスの索引 2 章が
    要件定義書と「全文で重複」として鳴った（23 件のうち 20 件がこれ）。
    印が付くのは争点のある行だけなので、**同じ 2 章でも争点が 0 件のうちは
    鳴らない** ―― 資料に食い違いが出た瞬間に鳴りはじめる出方だった。
    """
    where: dict[str, list[tuple[str, tuple[str, ...]]]] = defaultdict(list)
    for definition, blocks in prepared:
        title = str(definition.get("title") or definition.get("name"))
        for block in blocks:
            if block.id_column is None or len(block.columns) < 3:
                continue
            shape = tuple(c for c in block.columns
                          if c not in _APPENDED_COLUMNS)
            for row in block.rows:
                if block.id_column < len(row):
                    value = str(row[block.id_column]).strip()
                    if value and value not in _BLANK:
                        where[value].append((title, shape))

    pairs: dict[tuple[str, ...], list[str]] = defaultdict(list)
    for value, appearances in where.items():
        for i, (left, left_shape) in enumerate(appearances):
            for right, right_shape in appearances[i + 1:]:
                if left == right:
                    continue
                shared = set(left_shape) & set(right_shape)
                if len(shared) >= _SAME_COLUMNS:
                    pairs[tuple(sorted((left, right)))].append(value)
    return [Finding(
        "warn", "P106", " / ".join(titles),
        f"同じアイテム {len(values)} 件が両方の設計書に全文で出ています"
        "（列の顔ぶれも同じです）",
        hint="片方は表示 ID で参照するだけにする。"
             "両方に全文を置くと、直したときに片方が古くなる")
        for titles, values in sorted(pairs.items()) if len(values) >= _REPEAT]


# ── P107 出典列の有無が揺れている ───────────────────────────────
def _unpublished(spec: Spec,
                 prepared: list[tuple[dict[str, Any], list[Block]]]
                 ) -> list[Finding]:
    """**正本にあるのに、どの様式も出していない関係型。**

    穴の 1 枚は「正本に何が無いか」を言い、元資料の対応表は「届いた資料のどれが
    使われなかったか」を言う。**どちらも「正本にあるのに設計書へ出ていない」を
    言わない** ―― 3 つ目の面である。

    実測（r001）で 293 本が誰にも読めなかった::

        constrains  227 本  制約が何を縛るか
        refines      31 本  業務要件と機能要件の対応
        interfaces   20 本  外部連携が扱う実体
        triggers      8 本  ジョブの先行後続
        uses-code     7 本  データ項目が使うコード

    いちばん多い ``constrains`` が出ないのは滑稽ですらある ―― 縛る先を**持って
    いる** 227 本は 1 行も出ないのに、縛る先を**持っていない**制約 45 件だけが
    穴の 1 枚に理由つきで並んでいた。整理層が丁寧に張った関係ほど静かに消える。

    ここが見るのは**様式が拾っているか**だけで、正本の側は 1 つも疑っていない。
    """
    used = {name
            for definition, _ in prepared
            for section in definition.get("sections") or []
            for name in publish_module.relation_names(section)}
    used |= _SURFACED_ELSEWHERE
    findings: list[Finding] = []
    for name in sorted(spec.metamodel.relation_types):
        if name in used:
            continue
        live = [r for r in spec.relations_of(name)
                if r.get("status") != "deprecated"]
        if not live:
            continue                       # 使われていない語彙は様式の問題ではない
        label = (spec.metamodel.relation_types.get(name) or {}).get("label")
        what = f"{name}（{label}）" if label else name
        findings.append(Finding(
            "warn", "P110", what,
            f"正本に {len(live)} 本ありますが、どの設計書にも出ていません",
            hint="様式（パックの documents/*.yml）に節を足す。"
                 "整理層が張った関係は、出す先が無いと気づかれずに消える"))
    return findings


# ── P111 正本に値があるのに、どの列にも出ない属性 ───────────────
def _unsurfaced(spec: Spec,
                prepared: list[tuple[dict[str, Any], list[Block]]]
                ) -> list[Finding]:
    """**``P110`` の粒度を関係型から属性へ下げる。**

    ``P110`` が見るのは ``section.relation`` の集合だけで、``columns`` を
    1 つも見ていない ―― **節さえあれば属性が全部落ちても黙る。** 実測（r001）で
    正本の ``description`` 628 件のうち **528 件（84%）がどの生成物の本文にも
    現れなかった**のに、error も warn も 1 件も出なかった::

        displays            154/154   画面項目の初期値・物理名／帳票項目の取得元
        data-item            81/81    レイアウト上の必須区分
        operates             56/56    権限マトリクスの元の機能行名
        process-step         35/35    各ステップの参照/更新テーブル・例外時の動作
        requirement          15/36    非機能要件の確認方法・確認する工程（全 15 件）
        external-interface    5/5     異常時の扱い・再送/再実行（全 IF）

    **種別×属性では数えない。** 数えるのは**そのレコードが出たか**である ――
    非機能要件 15 件の ``description`` は非機能要件の節に列が無いので出ないが、
    機能要件の節には ``description`` 列があるので、種別で見ると「出ている」ことに
    なる。同じ属性でも、節が違えば出るものと出ないものがある。

    ``description`` を除外しないのが要点である。予約キー（:data:`ITEM_RESERVED` /
    :data:`RELATION_RESERVED`）は管理用なので数えないが、``description`` だけは
    **補足の受け皿として全種別に開いている**以上、出稿を必ず見る ―― 開いている
    受け皿に書けて、どこにも出ない、という状態を残さない。

    ここが見るのは**様式が拾っているか**だけである。直す先は 2 つあり、どちらを
    選ぶかは人が決める ―― 様式に列を足すか、整理層がそこへ書くのをやめるか。
    """
    printed, exempt = _surfaced(spec, prepared)
    findings: list[Finding] = []
    for (owner, attribute), holders in sorted(_valued(spec).items()):
        if (owner, attribute) in exempt or f"{owner}.{attribute}" in _SURFACED_KEYS:
            continue
        missing = holders - printed.get((owner, attribute), set())
        if not missing:
            continue
        findings.append(Finding(
            "warn", "P111", f"{owner}.{attribute}",
            f"正本に {len(missing)} 件ありますが、どの設計書にも出ていません",
            hint="様式（パックの documents/*.yml）の列に足す。"
                 "足す先が無いなら、整理層がそこへ書くのをやめる"
                 "（書く先が要るなら _metamodel-add.yml へ提案する）"))
    return findings


#: 正本が持つが**属性ではない**キー。``description`` は除く ―― 補足の受け皿と
#: して全種別に開いている以上、出稿を見る対象である（→ :func:`_unsurfaced`）。
_MANAGEMENT_KEYS = (mm.ITEM_RESERVED | mm.RELATION_RESERVED) - {"description"}

#: 列に出ていなくても**別の経路で設計書に出ている**属性（:data:`_SURFACED_ELSEWHERE`
#: の属性版）。``business-flow.steps`` は「業務フローの手順」が関係から同じことを
#: 出すので、列に無いことは穴ではない（→ ``requirement-spec.yml`` の注記）。
_SURFACED_KEYS = {"business-flow.steps"}


def _valued(spec: Spec) -> dict[tuple[str, str], set[int]]:
    """種別×属性 → **値を持つレコード**の識別。廃止は数えない。"""
    found: dict[tuple[str, str], set[int]] = defaultdict(set)
    for record in list(spec.items) + list(spec.relations):
        if record.get("status") == "deprecated":
            continue
        owner = str(record.get("type") or "")
        for key, value in record.items():
            key = str(key)
            if key in _MANAGEMENT_KEYS or value in (None, "", [], False):
                continue
            found[(owner, key)].add(id(record))
    return found


def _surfaced(spec: Spec, prepared: list[tuple[dict[str, Any], list[Block]]]
              ) -> tuple[dict[tuple[str, str], set[int]], set[tuple[str, str]]]:
    """様式が印字するもの ―― ``(種別×属性 → 出たレコード, 数えない種別×属性)``。

    2 つ目（除外）は**列以外の経路で出るもの**である。``group_by`` の値は節の
    見出しとして、``where`` の値は節そのものとして印字されるので、列に無いことは
    穴ではない ―― ここを混ぜると、要件定義書が節に割っている ``kind`` や
    ``subsystem`` が毎回鳴る。
    """
    printed: dict[tuple[str, str], set[int]] = defaultdict(set)
    exempt: set[tuple[str, str]] = set()
    by_id = spec.by_id

    def mark_reference(item: dict[str, Any] | None) -> None:
        """相手を「指す」列（``from`` / ``to``・升の見出し）が印字するもの。"""
        if item is None:
            return
        type_name = str(item.get("type") or "")
        definition = spec.metamodel.item_types.get(type_name) or {}
        printed[(type_name, "name")].add(id(item))
        display = sequence_module.display_attribute(definition)
        if display:
            printed[(type_name, str(display))].add(id(item))

    for definition, _blocks in prepared:
        for section in definition.get("sections") or []:
            kind = str(section.get("kind") or "items")
            columns = [str(c) for c in (section.get("columns") or [])]
            if kind in ("items", "trace"):
                type_name = str(section.get("type") or "")
                rows = publish_module._live(spec, type_name, section.get("where"))
                for column in columns:
                    if "." in column:
                        continue            # アイテムの表に端点の列は書けない
                    for row in rows:
                        printed[(type_name, column)].add(id(row))
                exempt |= {(type_name, key) for key in _elsewhere_keys(section)}
            if kind in ("relation", "matrix", "trace"):
                # trace は関係を配列で書ける（→ :func:`publish.relation_names`）。
                names = publish_module.relation_names(section)
                exempt |= {(name, key) for name in names
                           for key in _elsewhere_keys(section)}
                if kind == "trace":
                    continue                # 列はアイテム側で数え済み
                relation_type = names[0] if names else ""
                relations = publish_module._live_relations(spec, relation_type)
                if kind == "matrix":
                    cell = str(section.get("cell") or "")
                    for relation in relations:
                        if cell:
                            printed[(relation_type, cell)].add(id(relation))
                        # 升の行と列の見出しは両端の表示 ID と名称で出る。
                        for side in ("from", "to"):
                            mark_reference(by_id.get(str(relation.get(side))))
                    continue
                for column in columns:
                    for relation in relations:
                        if column in ("from", "to"):
                            mark_reference(by_id.get(str(relation.get(column))))
                        elif "." in column:
                            side, attribute = column.split(".", 1)
                            end = (by_id.get(str(relation.get(side)))
                                   if side in ("from", "to") else None)
                            if end is not None:
                                printed[(str(end.get("type")), attribute)].add(id(end))
                        else:
                            printed[(relation_type, column)].add(id(relation))
    return printed, exempt


def _elsewhere_keys(section: dict[str, Any]) -> set[str]:
    """列でない経路で印字される属性名（``group_by`` / ``where`` / ``labels``）。"""
    keys: set[str] = set()
    group_by = section.get("group_by")
    if group_by and str(group_by) != "from":
        keys.add(str(group_by))
    where = section.get("where")
    keys |= {str(k) for k in (where if isinstance(where, dict) else {})}
    labels = section.get("labels")
    for key in (labels if isinstance(labels, dict) else {}):
        keys.add(str(key).split(".", 1)[-1])
    return keys


def _trace_columns(
        prepared: list[tuple[dict[str, Any], list[Block]]]) -> list[Finding]:
    """同じ種別を出しているのに、出典を出す設計書と出さない設計書がある。

    実測で、出典列を持つのは 3 文書だけだった ―― テーブル定義書・画面帳票項目
    定義書・権限マトリクス・CRUD 図・トレーサビリティ・課題管理表は 0 件で、
    どのシートの何行目から来た値かを辿れない。**同じアイテムなのに、開いた
    設計書によって追跡できたりできなかったりする。**
    """
    with_source: dict[str, set[str]] = defaultdict(set)
    without: dict[str, set[str]] = defaultdict(set)
    for definition, _ in prepared:
        title = str(definition.get("title") or definition.get("name"))
        for section in definition.get("sections") or []:
            type_name = str(section.get("type") or "")
            # **一覧の章だけを比べる。** トレースは索引であって仕様書ではなく、
            # 出典はトレースの先（一覧のほう）にあれば足りる ―― ここを混ぜると
            # 「索引に出典が無い」が毎回鳴り、本物（同じ一覧なのに揃っていない）が
            # 埋もれる。
            if not type_name or str(section.get("kind") or "items") != "items":
                continue
            columns = [str(c) for c in (section.get("columns") or [])]
            (with_source if "source" in columns else without)[type_name].add(title)

    findings: list[Finding] = []
    for type_name in sorted(set(with_source) & set(without)):
        shown = "・".join(sorted(with_source[type_name]))
        hidden = "・".join(sorted(without[type_name]))
        findings.append(Finding(
            "warn", "P107", type_name,
            f"出典列の有無が揃っていません（出す: {shown} ／ 出さない: {hidden}）",
            hint="同じ種別なら揃える。出さない側を開いた読み手は出典を辿れない"))
    return findings
