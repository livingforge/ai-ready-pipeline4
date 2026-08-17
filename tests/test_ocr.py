"""貼り付け画像の中の文字（`arp4 ocr`）。**engine は起こさない。**

`tests/conftest.py` が Windows OCR を差し替えているのと同じ理由である ―― CI には
engine が無く、Windows でも入っている言語パックで読める字が変わる。ここで見るのは
**engine が返したものをどう扱うか**（報告の読み方・詰め方・読み置き・落ちたときの
言い分）で、engine そのものの精度ではない。

報告の書式（`L` / `I` / `T` / `E` / `X`）は :data:`arp4.ocr._SCRIPT` と対である。
片方だけ変えると**読めた字が黙って消える**ので、検体を実物の書式で書いてある。
"""

from __future__ import annotations

import pytest

from arp4 import ocr


# ── 読んだ字の寄せ戻し ──────────────────────────────────────────
def test_全角に挟まれた空白は詰める() -> None:
    """**engine は日本語を 1 文字ずつ語に割る。** 資料にそう書いてあったことは無い。"""
    assert ocr._tighten("受 注 番 号") == "受注番号"
    assert ocr._tighten("得 意 先 コ ー ド") == "得意先コード"


def test_英数との間の空白は残す() -> None:
    """**そこは engine の割り方が正しいことが多い。** 詰めると本当の区切りが消える
    ―― `ORDER 001` が `ORDER001` になると、元の 2 語には戻せない。
    """
    assert ocr._tighten("受 注 番 号 ORDER-001") == "受注番号 ORDER-001"
    assert ocr._tighten("Hello OCR test") == "Hello OCR test"
    assert ocr._tighten("16 桁") == "16 桁"


# ── 報告の読み方 ────────────────────────────────────────────────
def test_画像ごとに読めた行を割り当てる() -> None:
    trouble, got = ocr._harvest(
        "L\tja\nI\t0\nT\t受 注 番 号\nT\tORDER-001\nI\t1\nT\t社 名\n", 2)

    assert trouble == ""
    assert got[0].lines == ("受注番号", "ORDER-001") and got[0].language == "ja"
    assert got[1].lines == ("社名",)
    assert [one.trouble for one in got] == ["", ""]


def test_字の出なかった画像は理由なしの空で返す() -> None:
    """**「engine は動いたが字は無かった」は失敗ではない。** 画像が絵だった
    というだけで、次にやること（開いて見る）は読めなかったときと違う。
    """
    _, got = ocr._harvest("L\tja\nI\t0\n", 1)

    assert got[0].lines == () and got[0].trouble == ""


def test_1枚だけ落ちても残りは読めたまま返す() -> None:
    _, got = ocr._harvest(
        "L\tja\nI\t0\nE\tコンポーネントが見つかりません。\nI\t1\nT\t受 注\n", 2)

    assert "コンポーネント" in got[0].trouble and got[0].lines == ()
    assert got[1].lines == ("受注",) and got[1].trouble == ""


def test_途中で止まったぶんは字が無かったことにしない() -> None:
    """**いちばん静かな嘘になるところ。** 番号（`I`）の出てこなかった画像は
    engine が見てすらいないので、空で返すと「絵だった」と読まれる。
    """
    _, got = ocr._harvest("L\tja\nI\t0\nT\t受 注\n", 3)

    assert got[0].lines == ("受注",)
    assert got[1].trouble and got[2].trouble               # 見ていないと言う
    assert got[1].lines == () and got[2].lines == ()


def test_engineごと落ちたら全部に理由を付けて持ち帰る() -> None:
    trouble, got = ocr._harvest(
        "X\tこの Windows には OCR の言語パックが入っていません\n", 2)

    assert "言語パック" in trouble
    assert all("言語パック" in one.trouble for one in got)


def test_読めた字が種別の行に化けない() -> None:
    """報告の行は必ず種別で始まる。**裸で並べると OCR の出した文字列が
    ``I`` や ``X`` に見える**（`X` は「環境ごと駄目でした」の合図である）。
    """
    _, got = ocr._harvest("L\tja\nI\t0\nT\tX\tI\tL\n", 1)

    assert got[0].lines == ("X\tI\tL",) and got[0].trouble == ""


# ── 実体の見分け ────────────────────────────────────────────────
def test_拡張子は中身から付ける() -> None:
    assert ocr._suffix(b"\x89PNG\r\n\x1a\n....") == ".png"
    assert ocr._suffix(b"\xff\xd8\xff\xe0....") == ".jpg"
    assert ocr._suffix("何でもないバイト列".encode("utf-8")) == ".bin"


# ── 入口 ────────────────────────────────────────────────────────
def test_渡した名前は必ず全部返る(monkeypatch: pytest.MonkeyPatch) -> None:
    """**落とすと「読めなかった画像」と「渡し忘れた画像」が区別できない。**"""
    monkeypatch.setattr(ocr, "_unavailable", lambda: "")
    monkeypatch.setattr(ocr, "_run",
                        lambda bodies: ("", [ocr.Reading(lines=("字",))
                                             for _ in bodies]))
    got = ocr.read({"a.png": b"\x89PNG1", "b.png": b"\x89PNG2"})

    assert set(got) == {"a.png", "b.png"}


def test_同じ実体は1度しか読まない(monkeypatch: pytest.MonkeyPatch) -> None:
    """会社ロゴ・帳票の枠は**1 冊の中で何十回も貼り回される。**"""
    calls: list[int] = []

    def _run(bodies: list[bytes]) -> tuple[str, list[ocr.Reading]]:
        calls.append(len(bodies))
        return "", [ocr.Reading(lines=("社名",)) for _ in bodies]

    monkeypatch.setattr(ocr, "_unavailable", lambda: "")
    monkeypatch.setattr(ocr, "_run", _run)
    same = {"1.png": b"\x89PNGlogo", "2.png": b"\x89PNGlogo"}
    ocr.read(same)
    ocr.read(same)                                    # 2 度目は読み置きから

    assert calls == [1]


def test_1枚も返ってこなくても名前は落とさない(
        monkeypatch: pytest.MonkeyPatch) -> None:
    """起動できない・時間切れのとき、engine は 1 件も返さない。

    埋めずに返すと**渡した名前が丸ごと落ちる** ―― 呼び出し側から見ると
    「読めなかった画像」ではなく「渡し忘れた画像」になる。
    """
    monkeypatch.setattr(ocr, "_unavailable", lambda: "")
    monkeypatch.setattr(ocr, "_run", lambda bodies: ("時間内に終わりませんでした", []))
    got = ocr.read({"a.png": b"\x89PNG"})

    assert got["a.png"].trouble == "時間内に終わりませんでした"


def test_使えない環境では理由を全部の画像に付ける(
        monkeypatch: pytest.MonkeyPatch) -> None:
    """**黙って空を返さない。** 空は「画像に文字が無かった」に見える。"""
    monkeypatch.setattr(ocr, "_unavailable", lambda: "Windows ではありません")
    got = ocr.read({"a.png": b"\x89PNG"})

    assert got["a.png"].trouble == "Windows ではありません"
    assert ocr.trouble() == "Windows ではありません"     # 走らせた人にも言う（P016）
