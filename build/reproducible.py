"""生成した検体・見本を、**再生成しても同じバイト列**にする。

`examples/` の Excel は git に入っている（`arp4` を試す人が、Python を動かさずに
**開いて中身を確かめられる**ようにするため）。ところが生成器を回し直すと、
中身が 1 文字も変わっていなくても全部が「変更あり」になる ―― 実測で
`examples/kotonoha/build.py --clean` は 8 冊すべてを差分に出し、実際に違うのは
`docProps/core.xml` の保存時刻と zip のエントリ日時だけだった。

**そうなると「見本が古いかどうか」を誰も判定できない。** `git status` に 8 件
並んでいても、資料が変わったのか時計が進んだだけなのかが読めない ―― 差分が
常に出るものは、差分が出ても誰も見なくなる。

止めるところは 2 つしかない。

* **zip のエントリ日時**（:func:`freeze`）―― ``ZipFile.writestr`` は書いた時刻を
  そのまま焼き込む。ここを固定値にすると、同じ中身なら同じバイト列になる
* **ブックのプロパティ** ―― openpyxl は保存のたびに ``dcterms:created`` /
  ``dcterms:modified`` へ現在時刻を書く。既定値を置く :func:`stamp` は保存の
  **前**に、それでも上書きされる更新日時を正す :func:`restamp` は保存の**後**に
  呼ぶ（``save_workbook`` が書き出しの直前に現在時刻を入れるため）

**検体の主張は消さない。** 検体が自分でプロパティを書いているとき
（`tests/dataset/設計書.yml` の「最終更新者 川瀬」）は、そちらが正しい ――
そのパーツを差し替えるのは :func:`restamp` の**あと**なので、検体の値が勝つ。
順番を入れ替えると、**再現性の都合で検体の主張を機械が消す**ことになる。

実測（`tests/dataset` の 61 本）: 手を入れる前は 29 本が「中身は同じなのに毎回
違うバイト列」で、:func:`freeze` だけでは 22 本が残った。両方で 0 本になる。
"""

from __future__ import annotations

import datetime as dt
import re
import shutil
import zipfile
from pathlib import Path
from typing import Any, Callable

#: zip のエントリに焼く日時。**1980-01-01 は zip が表せる最小の日時**で、
#: 「これは実際の時刻ではない」と一目で分かる（再現可能な zip の慣習でもある）。
ZIP_EPOCH = (1980, 1, 1, 0, 0, 0)

#: ブックのプロパティに置く日時。パース結果の申告（`P005`）にそのまま出るので、
#: **見本を作った日**として読める値にしておく（`1980` は Excel の外の値である）。
BUILT_AT = dt.datetime(2026, 1, 1, 0, 0, 0)


def stamp(workbook: Any, at: dt.datetime = BUILT_AT) -> Any:
    """openpyxl のブックに**固定の作成日時**を置く。**保存の前に呼ぶ。**

    更新日時のほうはここでは決まらない ―― openpyxl の ``save_workbook`` は
    **保存の直前に現在時刻で上書きする**ので、そちらは :func:`restamp` が
    保存のあとで正す。
    """
    workbook.properties.created = at
    workbook.properties.modified = at
    return workbook


#: ``docProps/core.xml`` の日時。**中身は W3CDTF**（``2026-01-01T00:00:00Z``）。
_STAMPED = re.compile(
    r"(<dcterms:(?P<tag>created|modified)\b[^>]*>)[^<]*(</dcterms:(?P=tag)>)")


def restamp(path: Path, at: dt.datetime = BUILT_AT) -> Path:
    """保存済みのブックの**作成・更新日時を固定値に戻す**。

    **保存の直後に呼ぶ。** openpyxl は ``save_workbook`` の中で
    ``properties.modified`` を現在時刻に書き換えてから書き出すので、
    :func:`stamp` で置いた値は残らない ―― 実測で、ここを直すまでは
    22 冊が「中身は同じなのに毎回違うバイト列」のままだった。

    **あとから差し替えるパーツより先に呼ぶこと。** 検体が自分でプロパティを
    書いている（`tests/dataset/設計書.yml` の「最終更新者 川瀬」）ときは、
    そのパーツがこの値を上書きして勝つのが正しい ―― 検体の主張を再現性の
    都合で消してはいけない。
    """
    if not zipfile.is_zipfile(path):
        return path
    text = at.strftime("%Y-%m-%dT%H:%M:%SZ")
    return _rewrite(path, _CORE, lambda body: _STAMPED.sub(
        lambda m: f"{m.group(1)}{text}{m.group(3)}", body.decode("utf-8")
    ).encode("utf-8"))


#: ブックのプロパティ。**慣習の置き場**（関係から辿るのは読む側の仕事）。
_CORE = "docProps/core.xml"


def freeze(path: Path) -> Path:
    """zip（OOXML）のエントリ日時を固定して詰め直す。**中身は 1 バイトも変えない。**

    並びも圧縮方式も元のままにする ―― 並べ替えると、**中身が同じなのに
    バイト列が変わる**という直したい症状がそのまま残る。

    zip でないもの（`.csv` `.pdf` `.md`）はそのまま返す。あちらは書いた
    バイト列がそのまま残るので、はじめから再現する。
    """
    if not zipfile.is_zipfile(path):
        return path
    return _rewrite(path, None, None)


def _rewrite(path: Path, part: str | None,
             change: "Callable[[bytes], bytes] | None") -> Path:
    """zip を詰め直す。**並びも圧縮方式も変えない。**

    並べ替えると「中身が同じなのにバイト列が変わる」という直したい症状が
    そのまま残る。``part`` を渡すとそのパートだけ ``change`` に通す。
    エントリ日時は**いつでも**固定値にする ―― 詰め直した時点で元の日時は
    どのみち失われるので、ここで揃えておくのが素直である。
    """
    with zipfile.ZipFile(path) as source:
        entries = [(info, source.read(info.filename)) for info in source.infolist()]

    temporary = path.with_name(f"{path.name}.freezing")
    with zipfile.ZipFile(temporary, "w", zipfile.ZIP_DEFLATED) as target:
        for info, body in entries:
            if part is not None and change is not None and info.filename == part:
                body = change(body)
            frozen = zipfile.ZipInfo(info.filename, date_time=ZIP_EPOCH)
            frozen.compress_type = info.compress_type
            frozen.external_attr = info.external_attr
            frozen.internal_attr = info.internal_attr
            frozen.create_system = info.create_system
            target.writestr(frozen, body)
    shutil.move(str(temporary), str(path))
    return path
