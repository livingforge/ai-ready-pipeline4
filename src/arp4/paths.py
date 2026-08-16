"""置き場 ―― **arp4 が作るものは、1 つ残らず ``.arp/`` の中に。**

::

    <プロジェクト>/                 ← 配布先。arp4 は**ここに何も作らない**
    ├── src/  docs/  ddl/           元資料。既にある場所を parse が直接読む
    └── .arp/
        ├── rounds/
        │   └── r001/               ラウンド（ディレクトリで切る）
        │       ├── round.yml       いつ開いたか（名前に日付を持たせない代わり）
        │       ├── sources.yml     **撮った時点の原本の指紋**（上流のずれの検出）
        │       ├── parsed/**.md    機械抽出。**編集可**・git 管理
        │       └── organized/      整理結果（凍結後は編集しない）
        │           ├── **.yml      整理①
        │           ├── _concepts.yml   整理②の差分
        │           ├── _metamodel-add.yml
        │           └── .frozen.yml 凍結マニフェスト
        ├── spec/                   **正本**
        │   ├── metamodel.yml
        │   ├── concepts.yml        同一性の台帳（ラウンドをまたぐ）
        │   ├── items/*.yml
        │   └── relations/*.yml
        ├── out/                    生成物（直接編集しない）
        └── .gitignore              ``out/`` **だけ**を無視する

**``.arp/`` は「機械のもの」ではない。** 中の ``rounds/`` は人とエージェントが
編集し、git で管理する（決定 3）―― 畳んだのは持ち主が変わったからではなく、
**arp4 が配布されるツールだから**である。``rounds/`` も ``sources/`` も一般名詞で、
配布先の直下に置くと相手の持ち物と衝突する。だから ``.gitignore`` は ``out/`` だけを
無視する ―― ``.arp/`` を丸ごと無視すると、**編集可・git 管理という決定 3 の前提が
黙って消える**（→ ``docs/decisions.md`` 決定 47）。

**元資料は移動させない。** 既存資産は配布先に既にあるので、``arp4 parse src/ ddl/``
のようにその場所を直接指す。``sources/`` という置き場を作らせていたころは、資料が
原本と写しの 2 か所に増え、``sources.yml`` の指紋が**写しの指紋**になっていた。

``--root`` を省くと **cwd から上方探索**する。ただしエージェントに手順を渡すときは
**必ず ``--root`` を明示させる** ―― 省くと cwd 次第で置き場が動き、一時フォルダで
作業した結果が誰にも見えない場所に出る。

ラウンド名は **連番**（``r001``）である。日付名（``2026-08-02``）にしていたときは、
**同じ資料を別の日に処理し直すと別ラウンドになり、同じ作業が二重に走った**。
ラウンドは「いつ作業したか」ではなく「**どの資料の版を扱っているか**」の単位なので、
時刻から切り離す。いつ開いたかは :data:`ROUND_META`（``round.yml``）に残る。
"""

from __future__ import annotations

import datetime as _datetime
import re
from dataclasses import dataclass
from pathlib import Path

#: 状態ディレクトリの名前。**ここだけが正**。
ARP_DIR = ".arp"

#: ラウンドの入れ物。**``.arp/`` の下**である（直下に置くと配布先と衝突する）。
ROUNDS_DIR = "rounds"

#: 凍結マニフェストの名前。
FROZEN = ".frozen.yml"

#: ラウンドのメタ情報（開いた日・起こした理由）。**凍結ハッシュの対象外**。
ROUND_META = "round.yml"

#: 撮った時点の原本の指紋。**凍結ハッシュの対象外**（``organized/`` の外に置く）。
#:
#: 凍結は「整理結果が凍結後に書き換わっていないか」（:func:`arp4.freeze.verify`）を
#: 見ているが、**上流 ―― 原本のほう ―― は誰も見ていなかった。** 資料が差し替わっても
#: パース結果は古いまま残り、整理層は渡されたものを正しく整理し、正本は静かに
#: 古い版の写しになる。実測（自身のソース 26 ファイル）で 2 ファイルがずれており、
#: 気付いたのは**生成物を人が読み直したとき**だった。
SOURCE_PRINTS = "sources.yml"

#: 連番ラウンドの名前。
_SEQUENCE = re.compile(r"^r(\d+)$")


def sequence_name(number: int) -> str:
    """``1`` → ``r001``。3 桁を超えても崩れない（``r1000``）。"""
    return f"r{number:03d}"


class ArpNotFound(FileNotFoundError):
    """``.arp/`` が見つからないとき。**次の一手まで書く。**"""

    def __init__(self, start: Path) -> None:
        super().__init__(
            f"{ARP_DIR}/ が見つかりません（{start} から上へ探しました）\n"
            f"  arp4 init で作成できます: arp4 init --root <プロジェクト>")


class LegacyLayout(FileNotFoundError):
    """直下に ``rounds/`` を置いていたころのプロジェクト。**黙って読まない。**

    片方だけ読むと、**書き込み先と読み込み先が別々になる** ―― ``freeze`` は
    新しい置き場を空だと言い、``parse`` は古いほうへ足し続ける。どちらも
    エラーにはならないので、気づく手がかりが件数しか残らない。

    移動は人がやる。``git mv`` なら履歴が繋がり、**アンカーの照合が生き残る**
    （機械が移すと、履歴を残すかどうかを機械が決めることになる）。
    """

    def __init__(self, root: Path) -> None:
        super().__init__(
            f"{root / ROUNDS_DIR} は古い置き場です"
            f"（いまは {Path(ARP_DIR) / ROUNDS_DIR} に置きます）\n"
            f"  移してください: git mv {ROUNDS_DIR} {Path(ARP_DIR) / ROUNDS_DIR}\n"
            f"  片方だけ残すと、parse の書き込み先と freeze の読み込み先が"
            f"別々になります")


@dataclass(frozen=True)
class Round:
    """ラウンド 1 つ。``name`` は連番（``r001``）である。

    日付名（``2026-08-02``）も**読める**（過去に起こしたラウンドを壊さない）が、
    新しく起こすときは連番になる。
    """

    root: Path
    name: str

    @property
    def dir(self) -> Path:
        return self.root / ARP_DIR / ROUNDS_DIR / self.name

    @property
    def meta(self) -> Path:
        """``round.yml``。**organized/ の外**に置く（凍結ハッシュに混ぜない）。"""
        return self.dir / ROUND_META

    @property
    def prints(self) -> Path:
        """``sources.yml``。**``organized/`` の外**に置く（凍結ハッシュに混ぜない）。

        名前が ``sources`` なのは中身が原本の一覧だからである ―― 置き場ではなく
        「**どの版を撮ったか**」を言う。原本は配布先の元の場所にあるので、
        ここに並ぶのは ``arp4 parse`` に渡されたパスである。
        """
        return self.dir / SOURCE_PRINTS

    @property
    def parsed(self) -> Path:
        return self.dir / "parsed"

    @property
    def images(self) -> Path:
        """機械が読めなかった範囲の絵。**パース結果の隣**に置く。

        ``parsed/<ブック>/<シート>.md`` に対して ``images/<ブック>/<シート>-1.png``
        と並ぶので、どの絵がどのシートのものかを人が突き合わせずに済む。
        """
        return self.dir / "images"

    @property
    def organized(self) -> Path:
        return self.dir / "organized"

    @property
    def frozen(self) -> Path:
        return self.organized / FROZEN

    @property
    def concepts(self) -> Path:
        """整理②の出力（正本の concepts.yml への差分提案）。"""
        return self.organized / "_concepts.yml"

    @property
    def metamodel_add(self) -> Path:
        """語彙の追加提案。人が承認するまで凍結が通らない。"""
        return self.organized / "_metamodel-add.yml"

    def is_frozen(self) -> bool:
        return self.frozen.is_file()

    def exists(self) -> bool:
        return self.dir.is_dir()

    def open(self, reason: str = "", today: str | None = None) -> Path:
        """``round.yml`` を置く（**既にあれば触らない**）。

        名前から日付を外した代わりに、いつ開いたかはここに残す。冪等なので、
        同じラウンドへ parse をやり直しても開始日は動かない。
        """
        self.dir.mkdir(parents=True, exist_ok=True)
        if self.meta.is_file():
            return self.meta
        started = today or _datetime.date.today().isoformat()
        lines = ["# ラウンドのメタ情報。arp4 が開いたときに 1 度だけ書く。\n",
                 f"round: {self.name}\n", f"started_at: '{started}'\n"]
        if reason:
            lines.append(f"reason: {reason}\n")
        self.meta.write_text("".join(lines), encoding="utf-8", newline="\n")
        return self.meta


@dataclass(frozen=True)
class Paths:
    """プロジェクトの置き場。``root`` は ``.arp/`` の親である。"""

    root: Path

    # ── 作業面（``.arp/`` の中だが、人とエージェントが編集する）──────
    @property
    def rounds_dir(self) -> Path:
        return self.arp / ROUNDS_DIR

    @property
    def legacy_rounds_dir(self) -> Path:
        """**直下に置いていたころの** ``rounds/``。読まないが、見つけたら言う。"""
        return self.root / ROUNDS_DIR

    def round(self, name: str) -> Round:
        return Round(self.root, name)

    def rounds(self) -> list[Round]:
        """古い順。**名前で並べる**ので、日付名なら時系列になる。"""
        if not self.rounds_dir.is_dir():
            return []
        return [Round(self.root, d.name)
                for d in sorted(self.rounds_dir.iterdir()) if d.is_dir()]

    def latest_round(self) -> Round | None:
        found = self.rounds()
        return found[-1] if found else None

    def open_round(self) -> Round | None:
        """**まだ凍結していない**いちばん新しいラウンド。

        「作業中のラウンド」がこれである。``parse`` の既定の宛先はここで、
        **日付が変わっても宛先は変わらない。**
        """
        latest = self.latest_round()
        if latest is None or latest.is_frozen():
            return None
        return latest

    def new_round(self, name: str | None = None) -> Round:
        """新しいラウンド。既定は**連番の次**（``r001`` → ``r002``）。

        日付にしない理由は :mod:`arp4.paths` の冒頭に書いた。連番は既存の
        日付名ラウンドより後ろに並ぶ（``"2026-08-02" < "r001"``）ので、
        途中で切り替えても「いちばん新しいラウンド」がずれない。
        """
        if name:
            return Round(self.root, name)
        numbers = [int(m.group(1)) for m in
                   (_SEQUENCE.match(r.name) for r in self.rounds()) if m]
        return Round(self.root, sequence_name(max(numbers, default=0) + 1))

    # ── 機械が構築するもの ──────────────────────────────────────
    @property
    def arp(self) -> Path:
        return self.root / ARP_DIR

    @property
    def spec(self) -> Path:
        return self.arp / "spec"

    @property
    def items(self) -> Path:
        return self.spec / "items"

    @property
    def relations(self) -> Path:
        return self.spec / "relations"

    @property
    def out(self) -> Path:
        return self.arp / "out"

    # ── ファイル ────────────────────────────────────────────────
    @property
    def metamodel(self) -> Path:
        return self.spec / "metamodel.yml"

    @property
    def concepts(self) -> Path:
        """同一性の台帳。**ラウンドをまたいで 1 本**（正本の一部）。"""
        return self.spec / "concepts.yml"

    @property
    def lock(self) -> Path:
        return self.spec / "pack.lock"

    @property
    def display(self) -> Path:
        return self.arp / "display.yml"

    @property
    def publish(self) -> Path:
        return self.arp / "publish.yml"

    @property
    def policy(self) -> Path:
        """``arp4 auto`` の方針（自動昇格 ``auto_approve`` 等）。**既定は無し**
        ―― 昇格はプロジェクトが明示的に選んだときだけ有効になる。"""
        return self.arp / "policy.yml"

    def exists(self) -> bool:
        return self.spec.is_dir()


def resolve(start: Path | str | None = None) -> Paths:
    """``.arp/`` を探して :class:`Paths` にする。

    ``start`` に渡せるのは 3 つのどれでもよい ―― プロジェクト根 / ``.arp`` そのもの /
    その配下のどこか。**利用者に区別させない。**
    """
    origin = Path(start).resolve() if start else Path.cwd().resolve()

    for candidate in (origin, *origin.parents):
        if candidate.name == ARP_DIR and candidate.is_dir():
            return _checked(Paths(candidate.parent))
        if (candidate / ARP_DIR).is_dir():
            return _checked(Paths(candidate))
    raise ArpNotFound(origin)


def _checked(paths: Paths) -> Paths:
    """古い置き場が残っていたら止める。→ :class:`LegacyLayout`"""
    if is_legacy(paths.legacy_rounds_dir):
        raise LegacyLayout(paths.root)
    return paths


def is_legacy(rounds: Path) -> bool:
    """``rounds/`` が**arp4 の**古い置き場か。

    名前だけでは決めない ―― ``rounds`` は一般名詞なので、配布先が自分の都合で
    持っているだけのことがある（**それを塞ぐために ``.arp/`` へ畳んだのに、
    名前で撥ねたら同じ衝突を別の形で起こす**）。中にラウンドの目印
    （``round.yml`` か ``parsed/``）を持つディレクトリがあるときだけ本物とみなす。
    """
    if not rounds.is_dir():
        return False
    return any((child / ROUND_META).is_file() or (child / "parsed").is_dir()
               for child in rounds.iterdir() if child.is_dir())


def create(root: Path | str) -> Paths:
    """骨組みを作る（既にあるものは壊さない）。

    **``sources/`` は作らない。** 元資料は配布先の元の場所にあり、``arp4 parse``
    がそこを直接読む（→ モジュール冒頭「元資料は移動させない」）。
    """
    paths = Paths(Path(root).resolve())
    if is_legacy(paths.legacy_rounds_dir):
        raise LegacyLayout(paths.root)
    for directory in (paths.rounds_dir, paths.spec,
                      paths.items, paths.relations, paths.out):
        directory.mkdir(parents=True, exist_ok=True)

    if not paths.metamodel.exists():
        paths.metamodel.write_text(
            "# プロジェクトのメタモデル。標準パックを継承し、追加と厳格化だけを書く。\n"
            "extends: jp-sier-std\n"
            "version: 3\n", encoding="utf-8", newline="\n")

    if not paths.concepts.exists():
        paths.concepts.write_text(
            "# 同一性の台帳。整理②が提案し、arp4 build が追記する。\n"
            "# concept を item の ID と同一視しない（凍結物を無傷に保つ間接層）。\n"
            "[]\n", encoding="utf-8", newline="\n")

    gitignore = paths.arp / ".gitignore"
    if not gitignore.exists():
        gitignore.write_text(
            "# 無視するのは out/ だけである。\n"
            "#\n"
            "# .arp/ を丸ごと無視しないこと。rounds/ のパース結果と整理結果は\n"
            "# 人とエージェントが編集し、git で差分を読むためにある。無視すると\n"
            "# 「機械が最初に出したもの」と「人が直したもの」の区別が消える。\n"
            "# spec/ は正本なので、なおさら追跡する。\n"
            "out/\n", encoding="utf-8", newline="\n")
    return paths
