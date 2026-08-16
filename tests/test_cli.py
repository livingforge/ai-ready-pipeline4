"""コンソールへ出すところの異常系 ―― **資料の文字は選べない。**

パースの側は「読めなかったものを黙らない」で一貫しているが、その申告を出す
ところで落ちれば結果は同じである ―― むしろ悪い。**全部読み終えたあとで、
最後の 1 行が出せずに落ちる**ので、やり直しても同じところで止まる。
"""

from __future__ import annotations

import io
import sys
from pathlib import Path

from arp4 import cli
from arp4 import paths as paths_module


def _console(encoding: str = "cp932") -> io.TextIOWrapper:
    """日本語 Windows の既定コンソールの代わり。"""
    return io.TextIOWrapper(io.BytesIO(), encoding=encoding, newline="\n")


def test_cp932に無い文字で処理が止まらない(monkeypatch) -> None:
    """**半角の ``¥``・絵文字・外字は客先のシート名に普通に入っている。**

    cp932 に無い文字を ``print`` すると ``UnicodeEncodeError`` で処理そのものが
    落ちる ―― 出力の都合で仕事が止まってはいけない。
    """
    out, err = _console(), _console()
    monkeypatch.setattr(sys, "stdout", out)
    monkeypatch.setattr(sys, "stderr", err)

    cli._resilient_output()
    print("見積書.xlsx/受¥一覧.md \U0001F600")     # 半角 ¥ と絵文字
    out.flush()

    assert "受" in out.buffer.getvalue().decode("cp932")


def test_出せなかった文字と資料のクエスチョンを混ぜない(monkeypatch) -> None:
    """``replace``（``?``）だと、**出せなかった文字と資料の ``?`` が混ざる。**

    しかも ``?`` は Windows のファイル名に使えないので、``受?一覧.md`` と出た
    ものを探しても**そんな名前のファイルは無い**。符号位置が残っていれば、
    どの文字だったかは読み直せる ―― 「資料に無い」と「機械が読めていない」を
    混ぜないのと同じ理屈である。
    """
    out = _console()
    monkeypatch.setattr(sys, "stdout", out)

    cli._resilient_output()
    print("受¥一覧 ―― 区分は ? のまま")
    out.flush()
    shown = out.buffer.getvalue().decode("cp932")

    assert "\\xa5" in shown                             # 出せなかった文字
    assert "区分は ? のまま" in shown                    # 資料にある ? は ? のまま


# ── arp4 model の採番行 ────────────────────────────────────────
def test_機械が振らないことと表示IDが無いことを混ぜない() -> None:
    """整理層が最初に読むのは `arp4 model` である。

    採番を外した種別（`message`）を「名前で参照する」と言うと、**資料から取って
    書くべき ID を書かないまま整理結果が出来上がる**。書かれなかったものは
    `E010` まで気づかれない。
    """
    manual = {"attributes": {"message_id": {"kind": "string", "required": True,
                                            "unique": True},
                             "body": {"kind": "string", "required": True}}}
    none = {"attributes": {"physical_name": {"kind": "string", "unique": True}}}

    assert "資料から取って整理層が書く" in cli._sequence_hint(manual)
    assert "表示 ID を持たない" in cli._sequence_hint(none)


def test_採番があるならその書式を出す() -> None:
    assert cli._sequence_hint({"sequence": {"attribute": "req_id", "by": "kind",
                                            "format": {"機能": "FR-{:03d}"}}}) == (
        "req_id = 機能: FR-{:03d}（kind ごと）")


# ── 資料のパス ―― 既定値で埋めない ──────────────────────────────
def test_資料のパスを省いたら次の一手まで言う(tmp_path: Path, capsys) -> None:
    """既定を `sources/` にしていたころは、**打ち間違えても 0 冊で正常終了**した。"""
    paths_module.create(tmp_path)

    for command in ("parse", "render"):
        assert cli.main([command, "--root", str(tmp_path)]) == 2
        said = capsys.readouterr().err
        assert f"arp4 {command} --root" in said         # 打ち直せる形で出す
        assert "sources/" in said                       # なぜ変わったかも言う


def test_基準は資料のパスの共通の親(tmp_path: Path) -> None:
    """**元の木をそのまま写す。** 渡したパス自身を基準にすると `src` が落ちて、
    別のフォルダの同名ファイルが同じ場所へ出る。
    """
    paths = paths_module.create(tmp_path)
    args = _args(source=[str(tmp_path / "src"), str(tmp_path / "ddl")], base=None)

    sources, base = cli._targets(args, paths)

    assert base == tmp_path
    assert [p.name for p in sources] == ["src", "ddl"]


def test_基準を明示したらそちらを使う(tmp_path: Path) -> None:
    paths = paths_module.create(tmp_path)
    args = _args(source=[str(tmp_path / "src" / "arp4")],
                 base=str(tmp_path / "src"))

    _, base = cli._targets(args, paths)

    assert base == tmp_path / "src"


def test_別ドライブに散っていても消さずに畳む(tmp_path: Path, monkeypatch) -> None:
    """共通の親が取れないときは根を基準にする ―― **平らになるが消えはしない。**"""
    paths = paths_module.create(tmp_path)
    monkeypatch.setattr(cli.os.path, "commonpath",
                        lambda paths_: (_ for _ in ()).throw(ValueError))
    args = _args(source=[str(tmp_path / "a.py")], base=None)

    _, base = cli._targets(args, paths)

    assert base == paths.root


def _args(**overrides):
    class _A:
        pass
    args = _A()
    for key, value in overrides.items():
        setattr(args, key, value)
    return args


# ── 終了コードの 3 値化（0-2） ──────────────────────────────────
def test_exitcodeはerrorとwarnを区別する() -> None:
    """error あり = 1 / warn のみ（--strict）= 2 / clean = 0。

    ``--strict`` で warn も 1 にすると、**CI から正常と異常が区別できない**
    （error 0 でも exit 1 になる）。
    """
    from arp4.finding import Finding

    error = [Finding("error", "E010", "x", "…")]
    warn = [Finding("warn", "W030", "x", "…")]

    assert cli._exit_code([], strict=False) == 0
    assert cli._exit_code([], strict=True) == 0
    assert cli._exit_code(warn, strict=False) == 0     # 止めない運用は変えない
    assert cli._exit_code(warn, strict=True) == 2      # warn のみは 2
    assert cli._exit_code(error, strict=False) == 1
    assert cli._exit_code(error + warn, strict=True) == 1


# ── number の出力サマリ（0-4） ──────────────────────────────────
def test_採番のサマリは種別ごとの件数と範囲(monkeypatch) -> None:
    """890 件を stdout に流さない ―― 種別ごとの範囲なら抜けと体系違いは読める。"""
    from arp4 import sequence

    assignments = [
        sequence.Assignment("mtd-1", "method", "method_id", "MTD-0001"),
        sequence.Assignment("mtd-2", "method", "method_id", "MTD-0002"),
        sequence.Assignment("mtd-3", "method", "method_id", "MTD-0010"),
        sequence.Assignment("req-1", "requirement", "req_id", "FR-001"),
        sequence.Assignment("req-2", "requirement", "req_id", "NFR-001"),
    ]
    lines = cli._number_summary(assignments)

    assert "  method.method_id: MTD-0001 〜 MTD-0010（3 件）" in lines
    # 接頭辞の違う体系（FR / NFR）は 1 つの範囲に畳まない。
    assert "  requirement.req_id: FR-001（1 件）" in lines
    assert "  requirement.req_id: NFR-001（1 件）" in lines


def test_サマリは重複を言う() -> None:
    """**頭で束ねる以上、件数を数えない限り衝突が見えない。**

    実測で 12 件の重複が `TC-0001 〜 TC-0010（12 件）` の 1 行に畳まれ、
    範囲と件数の食い違いだけが痕跡になっていた。
    """
    from arp4 import sequence

    lines = cli._number_summary([
        sequence.Assignment("tcs-1", "test-case", "test_id", "TC-0001"),
        sequence.Assignment("tcs-2", "test-case", "test_id", "TC-0002"),
        sequence.Assignment("tcs-3", "test-case", "test_id", "TC-0001"),
    ])

    assert lines == ["  test-case.test_id: TC-0001 〜 TC-0002（3 件）"
                     " ← 表示 ID が 1 件重複しています"]


def test_サマリは歯抜けを言わない() -> None:
    """埋めるのは空いているアイテムだけなので、**飛び飛びは正常な穴埋めである。**"""
    from arp4 import sequence

    lines = cli._number_summary([
        sequence.Assignment("mtd-1", "method", "method_id", "MTD-0003"),
        sequence.Assignment("mtd-2", "method", "method_id", "MTD-0009"),
    ])

    assert lines == ["  method.method_id: MTD-0003 〜 MTD-0009（2 件）"]
