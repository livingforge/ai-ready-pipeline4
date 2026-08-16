"""YAML の読み書きと、**拡張子を 1 箇所で決める**ための層。

2 は ``.yaml`` を 9 箇所に直書きしていたので、拡張子を変えるだけで
9 箇所の修正が必要だった。3 は :data:`EXT` だけを見る。

読むときは ``.yml`` / ``.yaml`` の両方を受ける（既存資産と混在しても壊さない）。
**書くときは必ず** :data:`EXT` で書く。
"""

from __future__ import annotations

from pathlib import Path
from typing import Any

import yaml

#: 書き出しに使う拡張子。**ここだけが正**。
EXT = ".yml"

#: 読み込みで受け付ける拡張子。左が優先（同名なら ``.yml`` を採る）。
READ_EXTS = (".yml", ".yaml")


class YamlError(Exception):
    """YAML の読み書きで落ちたときの例外。**パスを必ず含める。**

    ``line`` は壊れている行（1 始まり・取れなければ ``None``）。PyYAML は
    例外の文章の中に ``line 12, column 5`` と書くが、**文章の中にある位置は
    機械が使えない** ―― 呼び出し側が正規表現で digging することになる。

    ``detail`` は PyYAML の言い分だけ（パスを含まない）。指摘に載せるときは
    ファイル名を :class:`arp4.finding.Finding` の ``file`` が持っているので、
    **文章にもう一度パスを書くと 1 件が 2 回名乗る。**
    """

    def __init__(self, message: str, *, line: int | None = None,
                 detail: str = "") -> None:
        super().__init__(message)
        self.line = line
        self.detail = detail or message


def _broken(path: Any, exc: yaml.YAMLError) -> YamlError:
    """PyYAML の例外を、**位置を構造として持つ**例外に直す。"""
    mark = getattr(exc, "problem_mark", None)
    line = mark.line + 1 if mark is not None else None
    return YamlError(f"YAML として壊れています: {path}\n{exc}",
                     line=line, detail=str(exc))


def load(path: Path) -> Any:
    """1 ファイル読む。空ファイルは ``None`` を返す。"""
    try:
        text = path.read_text(encoding="utf-8")
    except OSError as exc:
        raise YamlError(f"読めません: {path} ({exc})") from exc
    try:
        return yaml.safe_load(text)
    except yaml.YAMLError as exc:
        raise _broken(path, exc) from exc


class Marks:
    """YAML の要素ごとの行番号。**キーの並び**で引く。

    ``marks.line("records", 3, "statement")`` が ``records`` の 4 番目の
    ``statement:`` が書かれた行を返す。

    **引けなかったら親へ遡る。** 値がスカラだったり書式が壊れていたりして深い
    位置が取れないことは普通にあるが、そこで ``None`` を返すと**指摘から位置が
    消える** ―― 位置の無い指摘は、位置を持たせる前と同じものである。1 つ上の
    ``records[3]`` の行が出れば、人もエディタも十分たどり着ける。
    """

    def __init__(self, table: dict[tuple[Any, ...], int]) -> None:
        self._table = table

    def line(self, *keys: Any) -> int | None:
        path = tuple(keys)
        while path:
            found = self._table.get(path)
            if found is not None:
                return found
            path = path[:-1]
        return self._table.get(())

    def exact(self, *keys: Any) -> int | None:
        """**遡らない**引き方。「その欄が無い」と「その欄がここにある」を分ける。

        指摘を出すときは遡ってよい（1 つ上の行が出れば人は辿り着ける）が、
        **書き換えるときに遡ると別の行を書き換える。** ``attrs`` が無いレコードで
        遡ると、返るのはレコードの先頭行 ―― そこへ属性を足すと ``concept`` の行が
        壊れる。用途が違うものを 1 つの関数に畳まない。
        """
        return self._table.get(tuple(keys))

    def __len__(self) -> int:
        return len(self._table)


def _mark(node: yaml.Node, prefix: tuple[Any, ...],
          table: dict[tuple[Any, ...], int]) -> None:
    """ノードの木を歩いて行番号を集める。**先に書いたほうが勝つ。**

    連想配列の項目は**キーの行**を採る（値の行ではない）。``statement:`` が
    複数行にまたがるとき、値の行は 2 行目以降を指してしまい、「どの欄か」を
    探すのに読み手が上へ戻ることになる。
    """
    table.setdefault(prefix, node.start_mark.line + 1)
    if isinstance(node, yaml.MappingNode):
        for key_node, value_node in node.value:
            if not isinstance(key_node, yaml.ScalarNode):
                continue
            child = prefix + (key_node.value,)
            table.setdefault(child, key_node.start_mark.line + 1)
            _mark(value_node, child, table)
    elif isinstance(node, yaml.SequenceNode):
        for index, item in enumerate(node.value):
            _mark(item, prefix + (index,), table)


def load_marked(path: Path) -> tuple[Any, Marks]:
    """読むと同時に、**要素ごとの行番号**を持ち帰る。

    :func:`load` と**同じ結果**を返す（読み込んだ値には手を入れない）。行番号を
    値の側に埋め込む実装 ―― ``dict`` の派生型にして ``__line__`` を持たせる ――
    も広く使われているが、**その値はそのまま書き戻せない**（``yaml.safe_dump`` は
    派生型を知らないので落ちる）。``plan_declare`` は読んだものを書き戻すので、
    そこで壊れる。位置は値の外に置く。

    構文解析は 1 回だけ走らせる（``get_single_node`` してから
    ``construct_document`` するのが ``yaml.safe_load`` の中身そのもの）。
    """
    try:
        text = path.read_text(encoding="utf-8")
    except OSError as exc:
        raise YamlError(f"読めません: {path} ({exc})") from exc
    return marked(text, path)


def marked(text: str, where: Any = "<text>") -> tuple[Any, Marks]:
    """文字列から読む。**書く前に検算する**ために、ファイルと同じ経路を通す。

    ``arp4 lint --fix`` は直した結果を書く前に読み直して、**期待どおりの値に
    なっているかを確かめる**（→ :mod:`arp4.fix`）。そこで別の読み方をすると、
    検算しているつもりで違うものを見ることになる。
    """
    loader = yaml.SafeLoader(text)
    try:
        node = loader.get_single_node()
        table: dict[tuple[Any, ...], int] = {}
        if node is None:
            return None, Marks(table)
        _mark(node, (), table)
        return loader.construct_document(node), Marks(table)
    except yaml.YAMLError as exc:
        raise _broken(where, exc) from exc
    finally:
        loader.dispose()


def dumps(data: Any) -> str:
    """YAML 文字列にする。**日本語をエスケープせず、キー順を保つ。**"""
    return yaml.safe_dump(data, allow_unicode=True, sort_keys=False,
                          default_flow_style=False, width=100)


def dump(path: Path, data: Any) -> None:
    """1 ファイル書く。**拡張子は :data:`EXT` に強制する。**"""
    path = path.with_suffix(EXT)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(dumps(data), encoding="utf-8")


def find(directory: Path, stem: str) -> Path | None:
    """``<directory>/<stem>.yml`` を探す。無ければ ``.yaml`` も見る。"""
    for ext in READ_EXTS:
        candidate = directory / f"{stem}{ext}"
        if candidate.is_file():
            return candidate
    return None


def scan(directory: Path) -> list[Path]:
    """ディレクトリ直下の YAML を stem 順で返す。同名は ``.yml`` を優先する。"""
    if not directory.is_dir():
        return []
    chosen: dict[str, Path] = {}
    for ext in reversed(READ_EXTS):        # 後勝ちにして .yml で上書きする
        for path in sorted(directory.glob(f"*{ext}")):
            chosen[path.stem] = path
    return [chosen[stem] for stem in sorted(chosen)]


def scan_tree(directory: Path) -> list[Path]:
    """**配下すべて**の YAML をパス順で返す（同名は ``.yml`` を優先）。

    整理結果は元資料のフォルダ構造をそのまま写すので木になる。正本（``spec/``）は
    直下だけなので :func:`scan` を使う ―― 用途が違うものを 1 つの関数に畳まない。
    """
    if not directory.is_dir():
        return []
    chosen: dict[Path, Path] = {}
    for ext in reversed(READ_EXTS):        # 後勝ちにして .yml で上書きする
        for path in directory.rglob(f"*{ext}"):
            if path.is_file():
                chosen[path.with_suffix("")] = path
    return [chosen[key] for key in sorted(chosen)]
