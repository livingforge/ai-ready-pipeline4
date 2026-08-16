"""検体に貼る画像を**手で描く** ―― 絵柄そのものが検体の一部である。

長いあいだ、ここは灰色の矩形 1 枚だった。理由は「実体が要るだけ」で、
``r:embed`` の先が無い画像を Excel が**壊れたファイルとして修復**してしまい、
開いた人が図の有無を確かめられなくなるからである ―― 絵柄に意味は無かった。

**「OCR でも読めない画像」を突く検体では、それでは足りない。** パースは
「貼り付け画像の中身は取れていません。撮り直しても読めるようにはなりません」と
申告するが、**その申告が正しいかどうかを開いた人が判定できない** ―― 灰色の矩形は
「読めない」のではなく「何も描いていない」で、両者はまったく違う。実物の設計書に
貼ってあるのは**人には何の資料か分かり、機械には読めない**もの（網点の掛かった
社章、サーバ室で撮った機器ラック、コピー機を通した画面のハードコピー、斜めに歪んだ
帳票のスキャン）であって、そこを写していない検体では「読めないこと」を確かめようがない。

だから 4 通りを描く。**どれも LLM も外部の画像も使わない** ―― バイナリを
リポジトリに置かない約束は画像にも掛かるので、中身がコードで読める形でしか
置けない（`tests/dataset.py` と同じ理屈である）。

| 絵柄 | 実物での姿 | なぜ機械に読めないか |
| --- | --- | --- |
| ``ロゴ`` | 印刷物から取り込んだ社章 | **網点**（ハーフトーン）に潰れる。文字は人には読める |
| ``写真`` | サーバ室で撮った機器ラック | 陰影と粒。ラベルは小さく傾いている |
| ``画面`` | 現行画面のハードコピー | コピー機を通しているので**網点とかすれ**が乗る |
| ``帳票`` | 帳票見本のスキャン | 斜行・かすれ・地紋 |

**手書きは描かない。** 一度は結線を手で書き込んだ写真・朱書きの入った画面コピーを
描いていたが、**実物にはめったに無い書かれ方**である ―― 検体が写すのは「実案件の
設計書によくある書かれ方」であって、珍しい書かれ方を写すと検体としての価値が下がる
（そこを直しても、直った先が実物で起きない）。読めなさは手書きではなく**取り込み方**
（網点・かすれ・斜行・陰影）が作る ―― こちらのほうが実物では圧倒的に多い。

**大きさは控えめにしてある**（長辺 :data:`_MAX` px）。貼り先の図形へ引き伸ばされる
ので解像度は要らないし、検体はテストのたびに組み直されるものなので、描くのに
かかる時間がそのままテストの時間になる ―― 同じ絵は :data:`_MADE` に取っておく。
"""

from __future__ import annotations

import struct
import zlib

#: 描く絵の長辺（px）。**これ以上は要らない** ―― 貼り先へ引き伸ばされるうえに、
#: 網点・かすれ・斜行という「読めなさ」は解像度を上げても消えない。
_MAX = 360

#: 描いた絵の取り置き ``{(絵柄, 幅, 高さ): PNG}``。検体は 1 セッションで何度も
#: 組み直される（テストの module ごと）ので、同じ絵を描き直さない。
_MADE: dict[tuple[str, int, int], bytes] = {}

_BLACK = (0, 0, 0)

#: パッチケーブルの色。**4 色しかない**のが実物で、そのせいで写真から
#: 「どの口とどの口か」を追えない（同じ色が 1 枚に何本も走る）。
_CABLES = ((52, 86, 190), (200, 176, 56), (70, 150, 96), (90, 92, 98))
_WHITE = (255, 255, 255)

#: 5×7 の欧文書体。**日本語は描けない**ので、絵の中の文字は欧文の社名・画面 ID
#: だけである（実物の社章もそうなっている）。行は上から 7 本、1 が点。
_FONT = {
    "A": "01110 10001 10001 11111 10001 10001 10001",
    "B": "11110 10001 10001 11110 10001 10001 11110",
    "C": "01110 10001 10000 10000 10000 10001 01110",
    "D": "11110 10001 10001 10001 10001 10001 11110",
    "E": "11111 10000 10000 11110 10000 10000 11111",
    "F": "11111 10000 10000 11110 10000 10000 10000",
    "G": "01110 10001 10000 10111 10001 10001 01111",
    "H": "10001 10001 10001 11111 10001 10001 10001",
    "I": "11111 00100 00100 00100 00100 00100 11111",
    "J": "00111 00010 00010 00010 00010 10010 01100",
    "K": "10001 10010 10100 11000 10100 10010 10001",
    "L": "10000 10000 10000 10000 10000 10000 11111",
    "M": "10001 11011 10101 10101 10001 10001 10001",
    "N": "10001 11001 10101 10011 10001 10001 10001",
    "O": "01110 10001 10001 10001 10001 10001 01110",
    "P": "11110 10001 10001 11110 10000 10000 10000",
    "Q": "01110 10001 10001 10001 10101 10010 01101",
    "R": "11110 10001 10001 11110 10100 10010 10001",
    "S": "01111 10000 10000 01110 00001 00001 11110",
    "T": "11111 00100 00100 00100 00100 00100 00100",
    "U": "10001 10001 10001 10001 10001 10001 01110",
    "V": "10001 10001 10001 10001 10001 01010 00100",
    "W": "10001 10001 10001 10101 10101 11011 10001",
    "X": "10001 10001 01010 00100 01010 10001 10001",
    "Y": "10001 10001 01010 00100 00100 00100 00100",
    "Z": "11111 00001 00010 00100 01000 10000 11111",
    "0": "01110 10001 10011 10101 11001 10001 01110",
    "1": "00100 01100 00100 00100 00100 00100 01110",
    "2": "01110 10001 00001 00110 01000 10000 11111",
    "3": "11111 00010 00100 00010 00001 10001 01110",
    "4": "00010 00110 01010 10010 11111 00010 00010",
    "5": "11111 10000 11110 00001 00001 10001 01110",
    "6": "00110 01000 10000 11110 10001 10001 01110",
    "7": "11111 00001 00010 00100 01000 01000 01000",
    "8": "01110 10001 10001 01110 10001 10001 01110",
    "9": "01110 10001 10001 01111 00001 00010 01100",
    "-": "00000 00000 00000 11111 00000 00000 00000",
    ".": "00000 00000 00000 00000 00000 01100 01100",
    "/": "00001 00010 00010 00100 01000 01000 10000",
    ":": "00000 01100 01100 00000 01100 01100 00000",
    " ": "00000 00000 00000 00000 00000 00000 00000",
}


class _Random:
    """**同じ検体からは同じ絵が出る**ための、種を持つ擬似乱数。

    かすれ・地紋・陰影の揺れは乱数で作るが、組み直すたびに絵が変わると
    「前と同じ検体か」を人が言えなくなる（差分がノイズになるのと同じ理屈）。
    """

    def __init__(self, seed: int) -> None:
        self.state = seed & 0xFFFFFFFF or 1

    def next(self, span: int) -> int:
        self.state = (1103515245 * self.state + 12345) & 0x7FFFFFFF
        return self.state % span if span else 0

    def jitter(self, span: int) -> int:
        return self.next(2 * span + 1) - span


class _Canvas:
    """RGB の画布。**依存を増やさない**ので、点を置くところから自分で書く。"""

    def __init__(self, width: int, height: int,
                 colour: tuple[int, int, int] = _WHITE) -> None:
        self.width, self.height = width, height
        self.body = bytearray(bytes(colour) * (width * height))

    def dot(self, x: int, y: int, colour: tuple[int, int, int]) -> None:
        if 0 <= x < self.width and 0 <= y < self.height:
            at = (y * self.width + x) * 3
            self.body[at:at + 3] = bytes(colour)

    def get(self, x: int, y: int) -> tuple[int, int, int]:
        at = (y * self.width + x) * 3
        return (self.body[at], self.body[at + 1], self.body[at + 2])

    def box(self, x0: int, y0: int, x1: int, y1: int,
            colour: tuple[int, int, int], fill: bool = True) -> None:
        if not fill:
            self.line(x0, y0, x1, y0, colour)
            self.line(x0, y1, x1, y1, colour)
            self.line(x0, y0, x0, y1, colour)
            self.line(x1, y0, x1, y1, colour)
            return
        row = bytes(colour) * max(0, min(x1, self.width - 1) - max(x0, 0) + 1)
        for y in range(max(y0, 0), min(y1, self.height - 1) + 1):
            at = (y * self.width + max(x0, 0)) * 3
            self.body[at:at + len(row)] = row

    def line(self, x0: int, y0: int, x1: int, y1: int,
             colour: tuple[int, int, int], thick: int = 1) -> None:
        steps = max(abs(x1 - x0), abs(y1 - y0), 1)
        for step in range(steps + 1):
            x = x0 + (x1 - x0) * step // steps
            y = y0 + (y1 - y0) * step // steps
            for dx in range(thick):
                for dy in range(thick):
                    self.dot(x + dx, y + dy, colour)

    def stroke(self, points: list[tuple[int, int]],
               colour: tuple[int, int, int], thick: int = 2) -> None:
        """折れ線（配線の束など）。太さは端で細らせない ―― 絵の細工より、
        **何が写っているか**が分かることのほうが検体の中身である。"""
        for (x0, y0), (x1, y1) in zip(points, points[1:]):
            self.line(x0, y0, x1, y1, colour, thick)

    def text(self, x: int, y: int, words: str,
             colour: tuple[int, int, int], scale: int = 2) -> None:
        """5×7 の欧文。**人には読めるが、この先で潰す**（網点・かすれ）。"""
        at = x
        for letter in words.upper():
            rows = _FONT.get(letter, _FONT[" "]).split()
            for row, bits in enumerate(rows):
                for column, bit in enumerate(bits):
                    if bit == "1":
                        self.box(at + column * scale, y + row * scale,
                                 at + column * scale + scale - 1,
                                 y + row * scale + scale - 1, colour)
            at += 6 * scale

    def speckle(self, seed: int, density: int,
                colour: tuple[int, int, int] = (96, 96, 96)) -> None:
        """かすれと汚れ。**スキャンした紙には必ず乗っている。**"""
        random = _Random(seed)
        for _ in range((self.width * self.height) // max(density, 1)):
            self.dot(random.next(self.width), random.next(self.height), colour)

    def shade(self, seed: int, depth: int) -> None:
        """陰影（写真の明暗）。**平らな灰色との違いはここにしか出ない。**

        **揺らすのは行ごとで、画素ごとではない。** 画素ごとに乱数を振ると
        絵は砂嵐になり、しかも PNG が圧縮できなくなる ―― 検体 1 冊が
        104KB から 195KB に膨らんでいた。粒は :meth:`speckle`（疎な点）が
        受け持つので、ここは陰影だけを作ればよい。
        """
        random = _Random(seed)
        for y in range(self.height):
            fall = depth * y // max(self.height, 1) - random.jitter(3)
            for x in range(self.width):
                red, green, blue = self.get(x, y)
                self.dot(x, y, (_clamp(red - fall), _clamp(green - fall),
                                _clamp(blue - fall)))

    def screen(self, pitch: int = 4) -> None:
        """**網点。** 印刷物を取り込んだ絵はここで白黒の点の粗密になる ――
        人には同じ絵に見えるが、文字の輪郭は点に割れる（OCR が落ちる所以）。
        """
        matrix = _bayer(pitch)
        size = len(matrix)
        for y in range(self.height):
            row = matrix[y % size]
            for x in range(self.width):
                red, green, blue = self.get(x, y)
                light = (red * 299 + green * 587 + blue * 114) // 1000
                self.dot(x, y, _WHITE if light > row[x % size] else _BLACK)

    def png(self) -> bytes:
        raw = b"".join(
            b"\x00" + bytes(self.body[y * self.width * 3:(y + 1) * self.width * 3])
            for y in range(self.height))
        return (b"\x89PNG\r\n\x1a\n"
                + _chunk(b"IHDR", struct.pack(">IIBBBBB", self.width,
                                              self.height, 8, 2, 0, 0, 0))
                + _chunk(b"IDAT", zlib.compress(raw, 6))
                + _chunk(b"IEND", b""))


def _clamp(value: int) -> int:
    return 0 if value < 0 else (255 if value > 255 else value)


def _chunk(tag: bytes, data: bytes) -> bytes:
    blob = tag + data
    return struct.pack(">I", len(data)) + blob + struct.pack(">I", zlib.crc32(blob))


def _bayer(order: int) -> list[list[int]]:
    """網点の閾値。2 の冪で再帰的に作る（表を書き写さない）。"""
    matrix = [[0]]
    while len(matrix) < order:
        size = len(matrix)
        matrix = [[matrix[y % size][x % size] * 4
                   + (0 if (y < size and x < size) else
                      2 if (y < size) else 3 if (x < size) else 1)
                   for x in range(size * 2)] for y in range(size * 2)]
    span = len(matrix) ** 2
    return [[value * 255 // span for value in row] for row in matrix]


# ── 絵柄 ────────────────────────────────────────────────────────
def _logo(canvas: _Canvas) -> None:
    """印刷物から取り込んだ社章。**社名はこの絵の中にしか無い。**"""
    width, height = canvas.width, canvas.height
    canvas.box(0, 0, width - 1, height - 1, (244, 244, 240))
    mid = height // 2
    # 菱形の紋に山を重ねる（**輪郭が網点で割れる**ところが検体の中身である）
    span = min(width, height) // 3
    centre, top = width // 2, mid - span // 2 - 6
    for edge in range(3):
        canvas.stroke([(centre - span + edge, top + span // 2),
                       (centre, top - edge), (centre + span - edge,
                                              top + span // 2),
                       (centre, top + span - edge // 2),
                       (centre - span + edge, top + span // 2)],
                      (40 + edge * 8, 60 + edge * 8, 110), 2)
    canvas.stroke([(centre - span // 2, top + span // 2),
                   (centre, top + span // 5), (centre + span // 2,
                                               top + span // 2)],
                  (40, 60, 110), 3)
    canvas.text(width // 8, mid + span // 2 + 8, "TOWA LOGITEC", (24, 24, 40), 2)
    canvas.text(width // 8, mid + span // 2 + 30, "CO. LTD.", (72, 72, 88), 1)
    canvas.speckle(7, 400, (170, 170, 170))
    canvas.screen(4)                                   # ここで文字が点に割れる


def _photo(canvas: _Canvas) -> None:
    """サーバ室で撮った機器ラック。**書き込みは無い。**

    一度は結線を手で書き込んだ写真を描いていたが、**実物にはめったに無い
    書かれ方**である ―― 設計書に貼ってあるのはたいてい撮ったままの写真で、
    結線は（読める形で）別の図に描いてある。検体が写すのは「実案件によくある
    書かれ方」なので、珍しい書かれ方を写すとそのぶん値打ちが下がる。

    それでも**機械には読めない**。機器名はラベルにしか写っておらず、小さく、
    陰影に沈み、粒が乗っている ―― 読めなさを作るのは手書きではなく**撮り方**
    である（実物でもそちらが圧倒的に多い）。
    """
    width, height = canvas.width, canvas.height
    canvas.box(0, 0, width - 1, height - 1, (150, 152, 155))
    canvas.box(0, height - height // 6, width - 1, height - 1, (128, 130, 134))
    # **機器名は写真の中にしか無い。** 代替テキストがそう言っているのだから、
    # 写真の側にも無ければ検体の主張のほうが嘘になる。
    labels = (("LB-01", "FW-01", "AP-01", "AP-02", "DB-01", "DB-02",
               "BAT-01", "NAS-01"),
              ("LB-02", "FW-02", "AP-03", "AP-04", "DB-03", "DB-04",
               "MON-01", "KVM-01"))
    for side in (0, 1):                                # ラック 2 本
        x0 = width // 12 + side * width // 2
        x1 = x0 + width // 3
        canvas.box(x0, height // 8, x1, height - height // 5, (88, 90, 94))
        canvas.text(x0 + 8, height // 8 - 10, f"RACK-{side + 1}", (60, 60, 66), 1)
        for unit in range(8):                          # 1U ごとの機器
            y0 = height // 8 + 6 + unit * (height * 2 // 3) // 8
            canvas.box(x0 + 5, y0, x1 - 5, y0 + 10, (120, 124, 130))
            # **ラベルは口の反対側に置く。** 2 本のラックは向かい合う側に口が
            # あるので、機器名を同じ位置に置くとケーブルが名前を覆う
            canvas.text(x0 + (9 if side == 0 else 42), y0 + 2,
                        labels[side][unit], (56, 58, 64), 1)
            canvas.box(x0 + (106 if side == 0 else 12), y0 + 3,
                       x0 + (110 if side == 0 else 16), y0 + 6,
                       (200, 90, 60))                                    # LED
            canvas.box(x0 + 5, y0 + 11, x1 - 5, y0 + 12, (70, 72, 76))   # 段の影
    # ── 結線 ―― **写真で結線を持っているのはパッチケーブルそのものである。**
    #
    # 手で書き込んだ線をやめたときに、結線ごと絵から消えていた。実物のラック
    # 写真に結線が写っていないことはなく（写っているからこそ現場は写真を撮る）、
    # **それでも「どの口とどの口か」は読み取れない** ―― 束になって重なり、色は
    # 4 色しかなく、奥の 1 本は前の 1 本に隠れる。図形で描かれた構成図が別に
    # 要るのはそのためで、この検体はその対比を 1 シートの中に持っている。
    #
    # **ケーブルは宙で終わらない。** 必ずどこかの口に挿さっているか、画面の外
    # （床下・天井・隣の部屋）へ抜けている ―― 途中で切れている線は、写真では
    # なく描き損ないに見える。口は 2 本のラックの**向かい合う側**に置いてある
    # ので、渡りがラックの面を横切らない。
    floor = height - height // 5 + 6
    ports: dict[tuple[int, int], tuple[int, int]] = {}
    bundles: dict[tuple[int, int], int] = {}
    for side in (0, 1):
        x0 = width // 12 + side * width // 2
        x1 = x0 + width // 3
        for unit in range(8):
            y = height // 8 + 11 + unit * (height * 2 // 3) // 8
            port = (x1 - 31 if side == 0 else x0 + 31, y - 3)
            canvas.box(port[0] - 3, port[1] - 2, port[0] + 3, port[1] + 2,
                       (40, 42, 46))                   # 差込口
            ports[(side, unit)] = port
            bundles[(side, unit)] = (x1 + 3 + unit % 3 if side == 0
                                     else x0 - 5 - unit % 3)

    for (side, unit), port in ports.items():
        colour = _CABLES[(unit + side) % len(_CABLES)]
        bundle = bundles[(side, unit)]
        # 口 → 束 → 床 → **画面の外**（束はどれも床下へ抜けていく）
        canvas.stroke([port, (bundle, port[1] + 4), (bundle, floor),
                       (bundle + (14 if side else -14), floor + 10),
                       (width if side else -1, height - 4 - unit)], colour, 2)

    # ラックを跨ぐ渡り。**両端とも口に挿さる**（垂れるので図には見えない）
    for order, (from_unit, to_unit) in enumerate(((0, 1), (2, 4), (5, 6))):
        left, right = ports[(0, from_unit)], ports[(1, to_unit)]
        sag = 8 + order * 5
        canvas.stroke([left, (left[0] + 24, left[1] + sag // 2),
                       ((left[0] + right[0]) // 2, max(left[1], right[1]) + sag),
                       (right[0] - 24, right[1] + sag // 2), right],
                      _CABLES[(order + 2) % len(_CABLES)], 2)
    canvas.shade(11, 60)
    canvas.speckle(31, 900, (200, 200, 205))           # 撮影時の粒


def _hardcopy(canvas: _Canvas) -> None:
    """現行画面のハードコピー。**紙に出してコピー機を通したもの。**

    朱書きを描いていた版をやめた理由は :func:`_photo` と同じである ―― 実物で
    圧倒的に多いのは「印刷して綴じたものを取り込み直した画面」で、読めなくして
    いるのは人の手ではなく**機械の通し方**（網点・かすれ・傾き）である。
    """
    width, height = canvas.width, canvas.height
    canvas.box(0, 0, width - 1, height - 1, (250, 250, 252))
    canvas.box(6, 6, width - 7, 24, (120, 130, 160))               # タイトルバー
    canvas.text(12, 11, "SCR001 ORDER ENTRY", _WHITE, 1)
    for row in range(6):                                           # 入力欄
        y = 38 + row * (height - 52) // 6
        canvas.text(14, y + 3, "ITEM", (90, 90, 96), 1)
        canvas.box(52, y, width - 26, y + 12, _WHITE)
        canvas.box(52, y, width - 26, y + 12, (170, 172, 178), fill=False)
        canvas.box(56, y + 4, 56 + 30 + row * 9, y + 8, (176, 178, 184))
    canvas.box(width - 120, height - 26, width - 60, height - 12, (200, 202, 208))
    canvas.text(width - 112, height - 22, "REGIST", (70, 70, 76), 1)
    canvas.speckle(53, 700, (205, 205, 210))                       # かすれ
    canvas.shade(59, 18)                                           # 綴じ側の影
    canvas.screen(4)                                               # 網点に潰れる


def _form(canvas: _Canvas) -> None:
    """帳票見本のスキャン。**斜行している**（実物の取り込みは必ず傾く）。"""
    width, height = canvas.width, canvas.height
    canvas.box(0, 0, width - 1, height - 1, (252, 251, 246))
    slope = height // 40 or 1                          # 斜行
    canvas.text(14, 12, "SEIWA 2026", (60, 60, 64), 2)
    for row in range(9):                               # 罫線（斜めに引かれる）
        y = 40 + row * (height - 52) // 9
        canvas.line(10, y, width - 10, y + slope, (120, 120, 126))
    for column in range(4):                            # 縦罫
        x = 10 + column * (width - 20) // 4
        canvas.line(x, 40, x + slope, height - 12, (120, 120, 126))
    random = _Random(61)
    for row in range(9):                               # 文字に見えて文字でない
        y = 40 + row * (height - 52) // 9
        for column in range(4):
            x = 16 + column * (width - 20) // 4
            canvas.box(x, y + 4 + row * slope // 9, x + 20 + random.next(30),
                       y + 8 + row * slope // 9, (105, 105, 112))
    canvas.speckle(67, 220, (185, 185, 190))           # かすれ
    canvas.shade(71, 25)


_PATTERNS = {"ロゴ": _logo, "写真": _photo, "画面": _hardcopy, "帳票": _form}


def draw(kind: str, width: int, height: int) -> bytes:
    """絵柄と大きさ → PNG。**絵柄を書かない検体は灰色の矩形のまま**である。

    そこ（`画面仕様書.xlsx` など）で突いているのは「画像の中身は取れない」こと
    そのものなので、絵柄に意味は無い ―― 意味を持たせるのは、**読めないことを
    人が確かめる**必要のある検体だけでよい。
    """
    width = max(48, min(width or _MAX, _MAX))
    height = max(32, min(height or _MAX // 2, _MAX))
    key = (kind, width, height)
    if key not in _MADE:
        canvas = _Canvas(width, height, (200, 200, 200))
        pattern = _PATTERNS.get(kind)
        if pattern:
            pattern(canvas)
        _MADE[key] = canvas.png()
    return _MADE[key]
