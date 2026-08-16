"""束の見取り図 ―― **正本にある関係だけを線にする。**

目次は「どの工程に何が出たか」を並べるが、**どの設計書がどの設計書を引いて
いるか**はどこにも出ていなかった。実測（r001）で詳細設計書は
トレーサビリティ・マトリクスから 163 か所参照されているのに、束を渡された人が
それを知る手段は 12 ファイルを開いて表示 ID を目で追うことしか無い。

**外部の描画ライブラリは使わない。** 依存は PyYAML 1 本という約束に加えて、
生成物は ``file://`` で開かれる ―― Mermaid を CDN から読む形も、図のデータを
別ファイルから ``fetch`` する形も、**CORS で黙って絵が出なくなる**。ここは
Python が SVG を直接組み、同じ HTML の中に置く。

**線は数えて引く。** 引くのは「片方の設計書が持つ表示 ID を、もう片方の升が
名指ししている」という事実だけである（:func:`arp4.publish._owners` が持ち主を
決め、升の文字がそれを指す）。近さ・名前の似かたで線を足さない ―― :mod:`arp4.parse`
が「座標から線を復元しない」を守っているのと同じ理由で、**足した瞬間に図は
資料が言っていないことを言い始める。**
"""

from __future__ import annotations

import html
from dataclasses import dataclass

from arp4 import page as page_module

#: 箱の大きさと間隔。工程が横、同じ工程の中が縦。
_W, _H, _GAP, _PITCH = 180, 42, 18, 232
#: 図の余白（左上）と、工程名を書く帯の高さ。
_PAD, _LANE = 14, 26


@dataclass(frozen=True)
class Node:
    """図の箱 1 つ。``key`` は辺を張るときの名前（設計書の題）。"""

    key: str
    label: str
    #: 箱の 2 行目（``24 表 / 267 行``）。**数えれば出るものだけ**。
    sub: str = ""
    href: str = ""


def documents(groups: list[tuple[str, list[Node]]],
              edges: set[tuple[str, str]]) -> str:
    """工程を横に並べた見取り図（``<figure>`` ごと返す）。

    ``groups`` は ``(工程, その工程の設計書)`` を V 字の順で並べたもの、
    ``edges`` は ``(引く側の key, 引かれる側の key)``。

    箱が 1 つも無いときは空を返す ―― **線の無い図を出さない**。図が出たのに
    線が 1 本も無いと、読み手には「参照が無い」と「図が壊れている」の区別が
    付かない（升目の凡例と同じ議論である → :func:`arp4.publish._legend`）。
    """
    placed: dict[str, tuple[float, float]] = {}
    tallest = max((len(nodes) for _, nodes in groups), default=0)
    if not tallest:
        return ""
    height = _PAD + _LANE + tallest * (_H + _GAP) + _PAD
    width = _PAD * 2 + max(len(groups), 1) * _PITCH - (_PITCH - _W)

    lanes: list[str] = []
    boxes: list[str] = []
    for column, (phase, nodes) in enumerate(groups):
        x = _PAD + column * _PITCH
        top = _PAD + _LANE
        lanes.append(f'<rect class="lane" x="{x - 6:.0f}" y="{_PAD - 4:.0f}" '
                     f'width="{_W + 12}" height="{height - _PAD * 2 + 8:.0f}" rx="4"/>')
        lanes.append(f'<text class="ph" x="{x:.0f}" y="{_PAD + 12:.0f}">'
                     f"{html.escape(phase)}</text>")
        for row, node in enumerate(nodes):
            y = top + row * (_H + _GAP)
            placed[node.key] = (x, y)
            boxes.append(_box(node, x, y))

    lines = [_edge(placed[a], placed[b]) for a, b in sorted(edges)
             if a in placed and b in placed and a != b]

    body = ("".join(lanes) + '<g class="edge-layer">' + "".join(lines) + "</g>"
            + "".join(boxes))
    return (f'<figure class="map"><svg xmlns="http://www.w3.org/2000/svg" '
            f'width="{width:.0f}" height="{height:.0f}" '
            f'viewBox="0 0 {width:.0f} {height:.0f}" role="img" '
            f'aria-label="設計書どうしの参照">{_MARKER}{body}</svg>'
            "<figcaption>矢印は「左の設計書の升が、右の設計書が持つ表示 ID を"
            "名指ししている」ことだけを表します。"
            f"線は {len(lines)} 本。正本に無い関係は引いていません。"
            "</figcaption></figure>")


#: 矢印の先。``context-stroke`` は使わない（対応しない閲覧環境がある）。
_MARKER = ('<defs><marker id="arp-head" viewBox="0 0 8 8" refX="7" refY="4" '
           'markerWidth="7" markerHeight="7" orient="auto-start-reverse">'
           '<path class="head" d="M0 0 L8 4 L0 8 z"/></marker></defs>')


def _box(node: Node, x: float, y: float) -> str:
    label = node.label
    # 長い題は**切らずに縮める**。切ると「トレーサビリティ…」が 2 つ並んだとき
    # どちらか分からなくなる ―― 図の箱は名前で引くためのものである。
    size = max(7.5, min(11.5, (_W - 16) / max(len(label), 1)))
    inner = (f'<rect class="box" x="{x:.0f}" y="{y:.0f}" width="{_W}" '
             f'height="{_H}" rx="4"/>'
             f'<text x="{x + 8:.0f}" y="{y + 18:.0f}" '
             f'style="font-size:{size:.1f}px">{html.escape(label)}</text>')
    if node.sub:
        inner += (f'<text class="sub" x="{x + 8:.0f}" y="{y + 32:.0f}">'
                  f"{html.escape(node.sub)}</text>")
    if node.href:
        # 見取り図の箱は**別の設計書へ出る**ので新しいタブで開く（→ arp4.page）。
        return (f'<a href="{html.escape(node.href)}"{page_module.NEW_TAB}>'
                f"<title>{html.escape(node.label)}</title>{inner}</a>")
    return inner


def _edge(source: tuple[float, float], target: tuple[float, float]) -> str:
    """箱から箱へ 1 本。**前へ進む線は右から左、戻る線は左へ回す。**

    戻る線（下流の設計書が上流を引く）を同じ向きで描くと、箱の上を通って
    どちらが起点か読めなくなる。工程の順に意味があるので、**向きが目に見える**
    ことのほうが線の短さより重い。
    """
    sx, sy = source
    tx, ty = target
    if tx > sx:
        start, end = (sx + _W, sy + _H / 2), (tx, ty + _H / 2)
        bend = min(60.0, (end[0] - start[0]) / 2 + 12)
        path = (f"M{start[0]:.0f} {start[1]:.0f} "
                f"C{start[0] + bend:.0f} {start[1]:.0f} "
                f"{end[0] - bend:.0f} {end[1]:.0f} {end[0]:.0f} {end[1]:.0f}")
    else:
        start, end = (sx, sy + _H / 2), (tx, ty + _H / 2)
        bend = 44 + abs(sy - ty) / 6
        path = (f"M{start[0]:.0f} {start[1]:.0f} "
                f"C{start[0] - bend:.0f} {start[1]:.0f} "
                f"{end[0] - bend:.0f} {end[1]:.0f} {end[0]:.0f} {end[1]:.0f}")
    return f'<path class="edge" d="{path}" marker-end="url(#arp-head)"/>'
