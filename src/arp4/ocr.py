"""貼り付け画像の中の文字を読む ―― **Windows OCR**（パースの一部）。

長いあいだ arp4 は「画像の中身は機械には取れていません。読むのは整理層です」と
申告して、実体を ``images/`` へ出すところまでで手を止めていた。**それは 1 段
足りなかった。**

**文字しか無い平坦な画像なら、OCR だけで足りる。** 実案件の設計書に貼ってある
画像の多くは、表・画面・帳票を**そのまま撮ったもの**である ―― そこに描かれて
いるのは絵ではなく字で、字であるなら機械が読める。読んでおけば整理層は

- 画像を 1 枚ずつ開かずに済む（開くのは、読んだ字だけでは足りない絵だけでよい）
- ``s<番号>-o1`` を**出典として指せる**（画像は開けても、開いた中身は引用できない）
- パース結果を ``grep`` できる（「受注番号」がどの資料に出てくるかに画像も入る）

の 3 つを得る。**画像が読めないのではなく、読んだものを渡していなかった。**

**読んだ字は「資料に書いてある字」ではない。** OCR は必ず読み違える（実測では
``ORDER-001`` が ``ORDER-OOI`` になり、網点の掛かった社章は ``T 鋤 Lロら 工 TEC``
になった）。だからパース結果では**別のアンカー**（``o1``）に置き、セルの値とも
代替テキスト（人が書いた説明）とも混ぜない ―― 出自が違うものを同じ出典にすると、
整理層は「資料にそう書いてある」と読む。

**なぜ Windows OCR か。** Windows に最初から入っており、追加のインストールも
ネットワークも要らず、同じ画像からは同じ結果が出る。パースは「意味を判断しない」
層なので、読むのは**転記の域を出ない相手**でよい ―― ここに LLM を置くと、
パース層が資料に書いていないことを書き始める。

**なぜ PowerShell 経由か。** WinRT（``Windows.Media.Ocr``）を Python から直に
呼ぶには ``winsdk`` / ``winrt-*`` の追加インストールが要る。**「常に含める」を
pip の追加インストールに条件付けると、実際にはほとんどの環境で含まれない** ――
Windows PowerShell 5.1 は WinRT を呼べるので、追加インストールなしで届く道は
こちらしかない。起動は**ブック 1 冊につき 1 回**にまとめてある（実測で 6 枚 1 秒）。

**小さい画像は拡大してから読む。** Excel に貼られたスクリーンショットは縮めて
貼られていることが多く、そのままでは字が潰れる（実測 300×100 の画面コピーは
拡大なしだと 1 行落ちた）。倍率は 2 の冪だけにしてある ―― **同じ画像からは
同じ結果が出る**ことのほうが、あと一息の精度より大事である。

**使えない環境では黙って劣化させない。** Windows でない・言語パックが無い・
PowerShell が無いときは理由を持ち帰り、パース結果とコンソール（``P016``）の
両方に出す。空の ``o1`` は「画像に文字が無かった」と読めてしまい、それは
arp4 がいちばん避けたい嘘（「資料に無い」と「機械が読めていない」の取り違え）
そのものである。
"""

from __future__ import annotations

import hashlib
import os
import re
import shutil
import subprocess
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import Mapping

#: 拡大したあとの長辺の目安（px）。これ以上に伸ばしても字は増えず、読む時間だけ
#: 伸びる（Windows OCR は 1 枚あたり ``MaxImageDimension`` = 10000 px まで）。
_TARGET_PX = 1600

#: 拡大の上限（倍）。2 の冪だけを使うのは**結果を決定的にする**ためである。
_MAX_SCALE = 4

#: 起動ぶんの持ち時間（秒）と、1 枚あたりの持ち時間（秒）。**待ち続けない** ――
#: パースが画像 1 枚で止まると、読めた 29 冊まで出てこない。
_TIMEOUT_BASE = 30
_TIMEOUT_EACH = 10

#: 読み終わった画像 ``{バイト列の指紋: 読んだもの}``。**同じ実体は 1 度しか
#: 読まない** ―― 会社ロゴ・帳票の枠は 1 冊の中で何十回も貼り回される。
_SEEN: dict[str, "Reading"] = {}

#: 環境そのものが理由で読めなかったとき、その理由（1 度でも起きたら残る）。
#: 画像 1 枚ごとの失敗（:attr:`Reading.trouble`）とは別で、こちらは**次に
#: やることが人の環境の話になる**（→ ``P016``）。
_TROUBLE = ""


@dataclass(frozen=True)
class Reading:
    """画像 1 枚から**機械が読んだ字**。資料に書いてある字そのものではない。"""

    #: 読めた行。**engine の出した順**（おおむね上から下）。
    lines: tuple[str, ...] = ()
    #: 読んだ言語（``ja`` / ``en-US``）。**同じ画像でも環境で変わる**ので出す。
    language: str = ""
    #: 読めなかった理由。空なら engine は動いた（``lines`` が空なら
    #: 「動いたが字は見つからなかった」＝**絵である**）。
    trouble: str = ""

    @property
    def text(self) -> str:
        return "\n".join(self.lines)


def read(bodies: Mapping[str, bytes]) -> dict[str, Reading]:
    """画像を**まとめて**読む。``{名前: 読んだもの}`` を**必ず全件**返す。

    渡した名前が結果から落ちることはない ―― 落とすと、呼び出し側は「読めなかった
    画像」と「渡し忘れた画像」を区別できなくなる。
    """
    global _TROUBLE
    if not bodies:
        return {}
    digests = {name: hashlib.sha256(body).hexdigest()
               for name, body in bodies.items()}
    # **同じ実体は 1 度だけ読む。** 並びは指紋順（呼び出し側の辞書の順に
    # 引きずられると、同じブックから毎回違う順で読むことになる）。
    fresh = sorted({digest for digest in digests.values() if digest not in _SEEN})
    if fresh:
        by_digest = {digest: body for name, body in bodies.items()
                     if (digest := digests[name]) in fresh}
        why = _unavailable()
        if why:
            _TROUBLE = why
            for digest in fresh:
                _SEEN[digest] = Reading(trouble=why)
        else:
            why, got = _run([by_digest[digest] for digest in fresh])
            if why:
                _TROUBLE = why
            for digest, reading in zip(fresh, got):
                _SEEN[digest] = reading
            # **1 枚も返ってこないことがある**（起動できない・時間切れ）。
            # 埋めずに返すと呼び出し側の名前が丸ごと落ちるので、**渡した名前は
            # 必ず全部返る**という約束のほうが先に破れる。
            for digest in fresh:
                _SEEN.setdefault(digest, Reading(
                    trouble=why or "Windows OCR から結果が返りませんでした"))
    return {name: _SEEN[digest] for name, digest in digests.items()}


def trouble() -> str:
    """環境そのものが理由で読めなかったときの理由（無ければ空）。"""
    return _TROUBLE


def forget() -> None:
    """読み置きを捨てる。**テストのためにある**（環境も理由も持ち越さない）。"""
    global _TROUBLE
    _SEEN.clear()
    _TROUBLE = ""


# ── 環境 ────────────────────────────────────────────────────────
def _unavailable() -> str:
    if os.name != "nt":
        return ("Windows ではないので Windows OCR は使えません"
                "（画像の中の文字は誰も読んでいません）")
    if _powershell() is None:
        return ("Windows PowerShell（powershell.exe）が見つかりませんでした"
                "（Windows OCR を呼ぶ道がここにしかありません）")
    return ""


def _powershell() -> str | None:
    """``powershell.exe`` の場所。**``pwsh`` では代われない。**

    WinRT の射影（``Add-Type -AssemblyName System.Runtime.WindowsRuntime``）は
    Windows PowerShell 5.1（.NET Framework）にしか無い ―― PATH の先頭に
    PowerShell 7 が居る環境は珍しくないので、**まず絶対パスで探す。**
    """
    built = (Path(os.environ.get("SystemRoot", r"C:\Windows")) / "System32"
             / "WindowsPowerShell" / "v1.0" / "powershell.exe")
    if built.is_file():
        return str(built)
    return shutil.which("powershell")


# ── 実行 ────────────────────────────────────────────────────────
def _run(bodies: list[bytes]) -> tuple[str, list[Reading]]:
    """``powershell.exe`` を 1 回起こして全部読む。``(環境の理由, 1 枚ずつ)``。"""
    exe = _powershell()
    if exe is None:                                    # 直前に消えた
        return _unavailable(), []
    with tempfile.TemporaryDirectory(prefix="arp4-ocr-") as where:
        room = Path(where)
        listed: list[str] = []
        for index, body in enumerate(bodies):
            # **拡張子は中身から付ける。** ``BitmapDecoder`` は中身で判別するが、
            # 判別できなかったときのエラー文に名前が出るので、そこで嘘をつかない。
            one = room / f"{index:04d}{_suffix(body)}"
            one.write_bytes(body)
            listed.append(str(one))
        script = room / "ocr.ps1"
        # **BOM を付けて書く。** Windows PowerShell 5.1 は BOM の無いファイルを
        # ANSI として読むので、この中の日本語（読めなかった理由）が化ける。
        script.write_text(_SCRIPT, encoding="utf-8-sig", newline="\r\n")
        images = room / "images.txt"
        images.write_text("\n".join(listed), encoding="utf-8", newline="\n")
        report = room / "report.txt"
        try:
            done = subprocess.run(
                [exe, "-NoProfile", "-NonInteractive", "-ExecutionPolicy",
                 "Bypass", "-File", str(script), "-List", str(images),
                 "-Out", str(report)],
                capture_output=True,
                timeout=_TIMEOUT_BASE + _TIMEOUT_EACH * len(bodies))
        except subprocess.TimeoutExpired:
            return (f"Windows OCR が時間内に終わりませんでした"
                    f"（画像 {len(bodies)} 枚）", [])
        except OSError as exc:
            return f"Windows PowerShell を起動できませんでした（{exc}）", []
        if not report.is_file():
            return _died(done), []
        return _harvest(report.read_text(encoding="utf-8-sig"), len(bodies))


def _died(done: "subprocess.CompletedProcess[bytes]") -> str:
    """報告のファイルすら出なかったとき。**言い分をそのまま持ち帰る。**"""
    said = (done.stderr or b"").decode("utf-8", "replace").strip()
    said = " ".join(said.split())[:200]
    return ("Windows OCR を呼べませんでした"
            + (f"（{said}）" if said else f"（終了コード {done.returncode}）"))


def _harvest(report: str, count: int) -> tuple[str, list[Reading]]:
    """報告を読む。``(環境の理由, 画像 1 枚ずつ)`` で、**必ず ``count`` 件返す。**

    行の頭 1 文字が種別である（``L`` 言語 / ``I`` 画像の番号 / ``T`` 読めた行 /
    ``E`` その 1 枚の理由 / ``X`` 環境の理由）。読めた字を裸で並べないのは、
    **OCR の出した文字列が種別の行に化ける**のを避けるためである。

    **途中で止まったぶんは「字が無かった」にしない。** 番号（``I``）の出て
    こなかった画像は engine が見てすらいないので、空の :class:`Reading` を
    返すと「絵だった」と読まれる ―― いちばん静かな嘘になる。
    """
    language = ""
    outer = ""
    lines: list[list[str]] = [[] for _ in range(count)]
    troubles = [""] * count
    started = [False] * count
    at = -1
    for row in report.splitlines():
        tag, _, rest = row.partition("\t")
        if tag == "L":
            language = rest.strip()
        elif tag == "X":
            outer = rest.strip()
        elif tag == "I":
            at = int(rest) if rest.strip().isdigit() else -1
            if 0 <= at < count:
                started[at] = True
        elif tag == "T" and 0 <= at < count:
            tight = _tighten(rest)
            if tight:
                lines[at].append(tight)
        elif tag == "E" and 0 <= at < count:
            troubles[at] = rest.strip()

    stopped = outer or "Windows OCR が最後まで走りませんでした"
    return outer, [
        Reading(lines=tuple(lines[index]), language=language,
                trouble=troubles[index] if started[index] else stopped)
        for index in range(count)]


#: 全角の字（漢字・かな・全角記号）。**この間の空白だけを詰める。**
_WIDE = ("\u3000-\u303f\u3040-\u309f\u30a0-\u30ff\u3400-\u4dbf"
         "\u4e00-\u9fff\uf900-\ufaff\uff00-\uffef")

#: 全角と全角に挟まれた空白。Windows OCR は日本語を**1 文字ずつ語に割る**ので、
#: 素の行は ``受 注 番 号`` になる ―― 資料にそう書いてあったことは一度も無い。
_GAP = re.compile(f"(?<=[{_WIDE}])[ \u3000]+(?=[{_WIDE}])")


def _tighten(line: str) -> str:
    """``受 注 番 号 ORDER-001`` → ``受注番号 ORDER-001``。

    **字の種類だけで決める**（辞書も文脈も見ない）ので、判断ではなく転記の
    寄せ戻しである ―― :mod:`arp4.parse` が日付を画面の表記へ直すのと同じ。
    英数との間の空白は**残す** ―― そこは engine の割り方が正しいことも多く、
    詰めると ``16 桁`` が ``16桁`` になるだけでなく ``ORDER 001`` のような
    本当の区切りまで消える。
    """
    return _GAP.sub("", line.strip())


#: 先頭のバイト列 → 拡張子。**中身で判る形だけ**（``BitmapDecoder`` が開ける）。
_MAGIC = (
    (b"\x89PNG\r\n\x1a\n", ".png"),
    (b"\xff\xd8\xff", ".jpg"),
    (b"GIF8", ".gif"),
    (b"BM", ".bmp"),
    (b"II*\x00", ".tif"),
    (b"MM\x00*", ".tif"),
    (b"RIFF", ".webp"),
    (b"\x01\x00\x00\x00", ".emf"),
    (b"\xd7\xcd\xc6\x9a", ".wmf"),
)


def _suffix(body: bytes) -> str:
    for magic, suffix in _MAGIC:
        if body.startswith(magic):
            return suffix
    return ".bin"


#: 読む本体。**ここだけが Windows に触る。**
#:
#: 報告はファイルへ書く（標準出力ではない）―― コンソールの文字コードは環境で
#: 変わり、日本語がそこで化けると**読めた字と読めなかった字の区別が付かない。**
_SCRIPT = r"""param([string]$List, [string]$Out)
$ErrorActionPreference = 'Stop'
$writer = New-Object System.IO.StreamWriter($Out, $false,
    (New-Object System.Text.UTF8Encoding($false)))
function Say([string]$line) { $writer.WriteLine($line) }

try {
  Add-Type -AssemblyName System.Runtime.WindowsRuntime
  $null = [Windows.Media.Ocr.OcrEngine, Windows.Foundation, ContentType=WindowsRuntime]
  $null = [Windows.Graphics.Imaging.BitmapDecoder, Windows.Foundation, ContentType=WindowsRuntime]
  $null = [Windows.Storage.StorageFile, Windows.Foundation, ContentType=WindowsRuntime]
  $asTask = ([System.WindowsRuntimeSystemExtensions].GetMethods() | Where-Object {
      $_.Name -eq 'AsTask' -and $_.GetParameters().Count -eq 1 -and
      $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncOperation`1' })[0]

  function Await($operation, $type) {
    $task = $asTask.MakeGenericMethod($type).Invoke($null, @($operation))
    $task.Wait(-1) | Out-Null
    $task.Result
  }

  # 利用者の言語で読む。無ければ入っている言語のどれかで読む（言語タグは
  # 報告に出す ―― 同じ画像でも環境が違えば読めた字が変わる）。
  $engine = [Windows.Media.Ocr.OcrEngine]::TryCreateFromUserProfileLanguages()
  if (-not $engine) {
    foreach ($language in [Windows.Media.Ocr.OcrEngine]::AvailableRecognizerLanguages) {
      $engine = [Windows.Media.Ocr.OcrEngine]::TryCreateFromLanguage($language)
      if ($engine) { break }
    }
  }
  if (-not $engine) {
    Say "X`tこの Windows には OCR の言語パックが入っていません（設定 > 時刻と言語 > 言語と地域 > 言語のオプション > 基本的なタイピング）"
    exit 0
  }
  Say ("L`t" + $engine.RecognizerLanguage.LanguageTag)
  $limit = [Windows.Media.Ocr.OcrEngine]::MaxImageDimension

  $index = -1
  foreach ($path in [System.IO.File]::ReadAllLines($List, [System.Text.Encoding]::UTF8)) {
    $index += 1
    if (-not $path) { continue }
    Say ("I`t" + $index)
    $stream = $null
    $bitmap = $null
    try {
      $file = Await ([Windows.Storage.StorageFile]::GetFileFromPathAsync($path)) ([Windows.Storage.StorageFile])
      $stream = Await ($file.OpenAsync([Windows.Storage.FileAccessMode]::Read)) ([Windows.Storage.Streams.IRandomAccessStream])
      $decoder = Await ([Windows.Graphics.Imaging.BitmapDecoder]::CreateAsync($stream)) ([Windows.Graphics.Imaging.BitmapDecoder])
      $width = [double]$decoder.PixelWidth
      $height = [double]$decoder.PixelHeight
      $ratio = 1.0
      if ($width -gt $limit -or $height -gt $limit) {
        $ratio = [Math]::Min($limit / $width, $limit / $height)   # 大きすぎると読めない
      } else {
        while (($ratio -lt __MAX_SCALE__) -and
               (([Math]::Max($width, $height) * $ratio * 2) -le __TARGET_PX__)) {
          $ratio = $ratio * 2                                     # 小さすぎると潰れる
        }
      }
      if ($ratio -ne 1.0) {
        $transform = New-Object Windows.Graphics.Imaging.BitmapTransform
        $transform.ScaledWidth = [uint32][Math]::Max(1, [Math]::Round($width * $ratio))
        $transform.ScaledHeight = [uint32][Math]::Max(1, [Math]::Round($height * $ratio))
        $transform.InterpolationMode = [Windows.Graphics.Imaging.BitmapInterpolationMode]::Fant
        $bitmap = Await ($decoder.GetSoftwareBitmapAsync(
            [Windows.Graphics.Imaging.BitmapPixelFormat]::Bgra8,
            [Windows.Graphics.Imaging.BitmapAlphaMode]::Premultiplied,
            $transform,
            [Windows.Graphics.Imaging.ExifOrientationMode]::RespectExifOrientation,
            [Windows.Graphics.Imaging.ColorManagementMode]::ColorManageToSRgb)) ([Windows.Graphics.Imaging.SoftwareBitmap])
      } else {
        $bitmap = Await ($decoder.GetSoftwareBitmapAsync()) ([Windows.Graphics.Imaging.SoftwareBitmap])
      }
      $result = Await ($engine.RecognizeAsync($bitmap)) ([Windows.Media.Ocr.OcrResult])
      foreach ($line in $result.Lines) { Say ("T`t" + $line.Text) }
    } catch {
      # **いちばん奥の言い分を出す。** ``Await`` は ``Task.Wait`` なので、
      # 表の例外は必ず「1 以上のエラーが発生しました」になる ―― それでは
      # 「開けない画像」と「大きすぎる画像」の区別が読み手に付かない。
      Say ("E`t" + ($_.Exception.GetBaseException().Message -replace "`r?`n", ' '))
    } finally {
      if ($bitmap) { $bitmap.Dispose() }
      if ($stream) { $stream.Dispose() }
    }
  }
} catch {
  Say ("X`t" + ($_.Exception.Message -replace "`r?`n", ' '))
} finally {
  $writer.Flush()
  $writer.Close()
}
""".replace("__MAX_SCALE__", str(_MAX_SCALE)).replace("__TARGET_PX__", str(_TARGET_PX))
