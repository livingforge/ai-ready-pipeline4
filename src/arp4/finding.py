"""検出 1 件の型。メタモデル検査（M）とデータ検証（E/W）で共用する。

``level`` は ``error`` / ``warn`` の 2 値だけにする。3 値以上にすると
「どれを CI で止めるか」の判断がコードから消えて運用に漏れる。

``file`` / ``line`` は**指摘の物理的な位置**である。``target``（種別名・アイテム
ID・アンカー）とは役割が違う ―― 前者は「どこを開くか」、後者は「何の話か」。
2 つを 1 つの文字列に畳んでいたときは、``資料/A/受注テーブル[3]``（レコードの
添字）・``__main__.py:i1``（アンカー）・``ent-037a3625a979``（内部 ID）が同じ欄に
入っていて、**どれもエディタから開けなかった** ―― 添字は行ではないので数え直しが
要り、内部 ID に至っては人が正本を全文検索するしかない。

``file`` は**プロジェクト根からの相対**で書く。ラウンドや ``organized/`` からの
相対にすると、読み手が頭の中で連結しないと開けない ―― 位置を持たせる意味が消える。

位置が付いたものは ``file:line`` の形で出るので、端末とエディタのリンク検出が
そのまま効く（先頭に置き換えず ``[level] CODE`` の後ろに置くのは、**既存の出力の
読み方を変えないため**である。リンク検出は行頭でなくても当たる）。
"""

from __future__ import annotations

from typing import Any, Iterable, NamedTuple


class Finding(NamedTuple):
    """検出 1 件。"""

    level: str                  # "error" / "warn"
    code: str                   # M001 / E010 / W030 …
    target: str                 # 種別名・アイテム ID・アンカー（**何の話か**）
    message: str
    file: str | None = None     # プロジェクト根からの相対（**どこを開くか**）
    line: int | None = None     # 1 始まり
    hint: str | None = None     # 次の一手。--fix の種でもある

    @property
    def where(self) -> str:
        """出力に載せる場所。**位置が取れていれば位置を、無ければ target を出す。**

        両方あるときに ``file:line target`` と並べるのは、位置だけでは
        「何の話か」が言えないためである（1 行に 2 件の指摘が乗ることがある）。
        """
        if not self.file:
            return self.target
        location = f"{self.file}:{self.line}" if self.line else self.file
        return f"{location} {self.target}" if self.target else location

    @property
    def head(self) -> str:
        """``hint`` を除いた 1 行。

        ``hint`` は**規則ごとの定数**なので、一覧では最後に 1 度だけ出したい
        （→ :mod:`arp4.digest`）。1 件だけを出す場所は :meth:`render` のままで
        よいので、**畳める側だけを切り出す。**
        """
        return f"[{self.level}] {self.code} {self.where}: {self.message}"

    def render(self) -> str:
        """人が読む 1 件。``hint`` があるときだけ 2 行になる。"""
        return f"{self.head}\n    → {self.hint}" if self.hint else self.head

    def at(self, file: str | None = None, line: int | None = None, *,
           hint: str | None = None) -> "Finding":
        """位置を後から付ける。

        指摘を組み立てる場所（何が悪いかを知っている）と、位置を知っている場所
        （読み込み層）が離れていることがあるので、**組み立てと位置付けを分ける。**
        """
        return self._replace(file=file if file is not None else self.file,
                             line=line if line is not None else self.line,
                             hint=hint if hint is not None else self.hint)

    def as_dict(self) -> dict[str, Any]:
        """機械可読の 1 件。**位置が無い指摘は欄ごと落とす**（null を並べない）。"""
        data: dict[str, Any] = {"level": self.level, "code": self.code,
                                "target": self.target, "message": self.message}
        if self.file:
            data["file"] = self.file
        if self.line:
            data["line"] = self.line
        if self.hint:
            data["hint"] = self.hint
        return data


def counts(findings: Iterable[Finding]) -> dict[str, int]:
    """レベル別の件数。``{"error": 0, "warn": 0}`` を必ず含む。"""
    result = {"error": 0, "warn": 0}
    for finding in findings:
        result[finding.level] = result.get(finding.level, 0) + 1
    return result


def order(findings: Iterable[Finding]) -> list[Finding]:
    """**同じ入力からは同じ順序**で返す（差分をノイズにしないため）。

    ファイルと行を code の次に見るので、**同じファイルの指摘がまとまり、その中は
    上から下へ並ぶ。** 直す人はファイルを開いた順に潰していくので、並びがファイルを
    行き来すると同じファイルを何度も開くことになる。
    """
    return sorted(findings, key=lambda f: (f.level != "error", f.code,
                                           f.file or "", f.line or 0,
                                           f.target, f.message))
