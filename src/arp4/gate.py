"""``--force`` の痕跡 ―― **抜け道を選んだことを成果物に残す。**

``publish`` は error があれば止まる（``cli._publish``）。止まったときの逃げ道が
``--force`` で、これは要る ―― 資料が揃うまで設計書を 1 冊も出せないのでは、
レビューに掛けるものが無い。問題は**逃げたことがどこにも残らない**ことである。

実測（sales-corpus 30 冊・r001）で起きたこと::

    $ arp4 check
    error 14 / warn 83          ← E010 14・W030 32・W031 27・W032 18・W044 6
    $ arp4 publish --force      ← 通した
    → out/ の 12 文書には「アイテム 808 件 / 関係 1094 件」としか書かれていない

生成物を読んだ人には、**要件 43 件が設計要素に 1 つも繋がっていないこと**も、
**権限マトリクスが原典と逆になっていること**も見えない。警告は端末に流れて消えた。
``arp4 check`` を後から回せば分かるが、成果物を受け取った人は端末を持っていない。

そこで **``publish`` が通った条件そのものを生成物に書く**。

* ``forced`` なら **目次と全文書の冒頭に警告帯**を出す（脚注ではなく冒頭 ――
  読み飛ばせる位置に置くと、``--force`` の常態化を止められない）
* 通っていても未解決の warn があれば**フッタの 1 行**に件数を出す
* いずれも ``out/_gate.json`` に機械可読で残す（``arp4 check --gate`` が読む）

**「うるさくする」ことが目的ではない。** ``--force`` の costが「端末に 10 行流れる」
だけだと、次のラウンドでも同じ判断が繰り返される ―― 実際 3 度繰り返されている
（pack.yml 3.2.0 / 3.4.0 / 3.8.0 はいずれも「正しい作業が E010 で拒まれた」記録で
ある）。成果物が汚れるなら、``--force`` を打つ前に**正本を直す動機**が生まれる。

痕跡は次に通ったときに消える ―― :func:`record` は毎回上書きするので、error を
片付けて ``--force`` 無しで通せば帯も消える。**消し方が「直す」しか無い**のが要点で、
``_gate.json`` を手で消しても次の ``publish`` が書き直す。
"""

from __future__ import annotations

import json
from dataclasses import dataclass, field
from datetime import date
from pathlib import Path
from typing import Any, Iterable

from arp4 import page as page_module
from arp4.finding import Finding, order

#: 成果物と一緒に置く機械可読の記録。
FILENAME = "_gate.json"


@dataclass(frozen=True)
class Gate:
    """``publish`` が通った条件。

    ``counts`` はコード別の件数（``{"W030": 32, ...}``）。全文を持たないのは、
    生成物へ全部書くと本文より長くなるためで、**読む先は ``arp4 check``** である
    ―― 帯の役目は「読みに行かせる」ことであって、指摘そのものを載せることではない。
    """

    forced: bool = False
    errors: int = 0
    warns: int = 0
    counts: dict[str, int] = field(default_factory=dict)
    #: 冒頭に出す代表例（``--force`` のときだけ使う）。
    examples: list[str] = field(default_factory=list)
    at: str = ""

    @property
    def clean(self) -> bool:
        """未解決が 1 件も無い。"""
        return not self.forced and self.errors == 0 and self.warns == 0

    def breakdown(self) -> str:
        """``E010 14・W030 32`` のような内訳。**多い順ではなくコード順**にする
        ―― 件数順にすると版が変わるたびに並びが動き、差分が読めない。"""
        return "・".join(f"{code} {n}" for code, n in sorted(self.counts.items()))


def summarize(findings: Iterable[Finding], forced: bool,
              today: str | None = None) -> Gate:
    """検出の一覧から記録を作る。**判定はしない**（誰が通したかは呼ぶ側が知る）。"""
    listed = list(findings)
    counts: dict[str, int] = {}
    for finding in listed:
        counts[finding.code] = counts.get(finding.code, 0) + 1
    blocking = [f for f in listed if f.level == "error"]
    return Gate(
        forced=forced,
        errors=len(blocking),
        warns=sum(1 for f in listed if f.level == "warn"),
        counts=counts,
        examples=[f.render() for f in order(blocking)[:5]],
        at=today or date.today().isoformat())


def record(out_dir: Path, gate: Gate) -> Path:
    """``out/_gate.json`` を書く。**毎回上書きする**（古い痕跡を残さない）。"""
    path = out_dir / FILENAME
    path.parent.mkdir(parents=True, exist_ok=True)
    payload: dict[str, Any] = {
        "forced": gate.forced, "errors": gate.errors, "warns": gate.warns,
        "counts": gate.counts, "examples": gate.examples, "at": gate.at}
    # ensure_ascii は report と同じ理由（cp932 の端末へ流れても壊れない）。
    path.write_text(json.dumps(payload, ensure_ascii=True, indent=2) + "\n",
                    encoding="utf-8", newline="\n")
    return path


def load(out_dir: Path) -> Gate | None:
    """前回の ``publish`` が通った条件。無ければ ``None``。"""
    path = out_dir / FILENAME
    if not path.is_file():
        return None
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None
    if not isinstance(payload, dict):
        return None
    return Gate(forced=bool(payload.get("forced")),
                errors=int(payload.get("errors") or 0),
                warns=int(payload.get("warns") or 0),
                counts={str(k): int(v)
                        for k, v in (payload.get("counts") or {}).items()},
                examples=[str(x) for x in (payload.get("examples") or [])],
                at=str(payload.get("at") or ""))


def banner(gate: Gate | None, depth: int = 0) -> list[str]:
    """``--force`` で通したときに**冒頭へ**置く帯（Markdown）。

    ``depth`` は工程フォルダの深さ ―― 穴の一覧への相対リンクを合わせる。
    通常どおり通ったときは空を返す（本文の前に何も足さない）。
    """
    if gate is None or not gate.forced:
        return []
    up = "../" * depth
    lines = [
        "> [!CAUTION]",
        f"> **この設計書は未解決の指摘を残したまま `--force` で生成されています。**",
        f"> error {gate.errors} 件 / warn {gate.warns} 件（{gate.breakdown()}）。",
        ">",
        "> 未解決のまま出しているので、書かれていないことが「資料に無い」とは"
        "限りません。",
        f"> 穴の一覧: [{_HOLES}]({up}{_HOLES}) ／ 全文は `arp4 check` で出ます。",
        ""]
    return lines


def banner_html(gate: Gate | None, depth: int = 0) -> str:
    """:func:`banner` の HTML 版。空なら空文字。"""
    if gate is None or not gate.forced:
        return ""
    import html as _html
    up = "../" * depth
    return (
        '<div class="forced"><p><strong>この設計書は未解決の指摘を残したまま'
        " --force で生成されています。</strong></p>"
        f"<p>error {gate.errors} 件 / warn {gate.warns} 件"
        f"（{_html.escape(gate.breakdown())}）。未解決のまま出しているので、"
        "書かれていないことが「資料に無い」とは限りません。</p>"
        f'<p>穴の一覧: <a href="{up}{_HOLES_HTML}"{page_module.NEW_TAB}>{_HOLES}</a>'
        " ／ 全文は <code>arp4 check</code> で出ます。</p></div>")


def footnote(gate: Gate | None) -> str:
    """フッタに足す 1 文。``--force`` でなくても未解決があれば言う。

    **通ったことは「穴が無い」ことを意味しない。** W030（どこからも参照されない）は
    error ではないが、トレーサビリティが繋がっていないという事実そのものである
    ―― 実測で 32 件あり、生成物にはその旨が 1 文字も出ていなかった。
    """
    if gate is None or gate.clean:
        return ""
    if gate.forced:
        return (f"未解決の指摘 error {gate.errors} 件 / warn {gate.warns} 件を"
                f"残したまま `--force` で生成（{gate.breakdown()}）。")
    return (f"未解決の指摘 warn {gate.warns} 件（{gate.breakdown()}）。"
            "`arp4 check` で全文が出ます。")


#: 穴の一覧（→ :mod:`arp4.holes`）。帯からリンクするので名前をここに置く。
_HOLES = "0_この設計書の穴.md"
_HOLES_HTML = "0_この設計書の穴.html"
