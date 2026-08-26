"""書き戻しでコメントを保つ ―― **消えるのは値ではなく理由である。**

検体が突くのは 2 つ ―― 申し送りが残ること、そして**残せないときに値を守る**こと。
賢く保とうとして値が変わるのは、コメントが消えるよりはるかに悪い。
"""

from __future__ import annotations

from pathlib import Path

import yaml

from arp4 import comments, spec as spec_module, yamlio
from arp4.paths import Paths

_FILE = """\
# ファイル全体への申し送り（2026-08-20 の打合せ）。

# この 2 件は顧客との対応表。番号を動かさないこと。
- id: req-1
  type: requirement
  req_id: FR-001      # 契約書の別紙 3 と対応
  name: 受注する

# 実装確認まで済んでいる
- id: req-2
  type: requirement
  req_id: FR-002
  name: '受注 # 1 を数える'
"""


def _read(text: str) -> tuple[list[dict], comments.Comments]:
    records = yaml.safe_load(text)
    return records, comments.read(text, records)


def test_申し送りを拾って戻す() -> None:
    records, found = _read(_FILE)

    assert found.header == ["# ファイル全体への申し送り（2026-08-20 の打合せ）。"]
    assert found.before["req-1"] == ["# この 2 件は顧客との対応表。番号を動かさないこと。"]
    assert found.inline[("req-1", "req_id")] == "契約書の別紙 3 と対応"

    again = comments.render(records, found)
    assert "番号を動かさないこと" in again
    assert "req_id: FR-001  # 契約書の別紙 3 と対応" in again
    assert yaml.safe_load(again) == records          # 値は 1 つも変わらない


def test_ファイルへの申し送りと1件目への申し送りを分ける() -> None:
    """両方に入れると、**書き戻すたびに 1 つずつ増える。** 見分けは空行で付ける。"""
    records, found = _read(_FILE)
    again = comments.render(records, found)

    assert again.count("ファイル全体への申し送り") == 1


def test_値の中のシャープを註と読み違えない() -> None:
    """``名前: 受注 # 1`` を註と読むと、**資料の字が落ちる。**"""
    records, found = _read(_FILE)

    assert ("req-2", "name") not in found.inline
    assert yaml.safe_load(comments.render(records, found))[1]["name"] == "受注 # 1 を数える"


def test_レコードが増えても申し送りがずれない() -> None:
    """並び順で対応させると、1 件増えただけで**全部が 1 つ隣へずれる。**

    ずれたコメントは、消えたコメントより始末が悪い（誰も疑わない嘘になる）。
    """
    records, found = _read(_FILE)
    records.insert(0, {"id": "req-0", "type": "requirement", "name": "先頭に増えた"})

    again = comments.render(records, found)
    lines = again.splitlines()
    here = lines.index("- id: req-1")

    assert lines[here - 1] == "# この 2 件は顧客との対応表。番号を動かさないこと。"
    assert "req-0" in again and yaml.safe_load(again) == records


def test_関係は起点と終点で対応させる() -> None:
    """関係に ``id`` は無い ―― :func:`arp4.spec.relation_key` と同じ取り方にする。"""
    text = ("# 画面から要件へ\n"
            "- type: realizes\n"
            "  from: scr-1\n"
            "  to: req-1\n")
    records, found = _read(text)

    assert found.before["realizes|scr-1|req-1"] == ["# 画面から要件へ"]
    assert "# 画面から要件へ" in comments.render(records, found)


def test_コメントが無ければ素の書き出しと同じ() -> None:
    records, found = _read("- id: req-1\n  type: requirement\n")

    assert not found
    assert comments.render(records, found) is None      # 素の書き出しに任せる


def test_書き戻しで申し送りが残る(project: Paths) -> None:
    """`build` / `number` / 自動昇格が触った瞬間に消えていた道である。"""
    path = project.items / "requirement.yml"
    path.write_text(_FILE, encoding="utf-8", newline="\n")
    records = yamlio.load(path)
    spec = spec_module.Spec(metamodel=None, paths=project, items=records,
                            relations=[], item_files=[(path, records)],
                            relation_files=[])
    records[0]["status"] = "approved"

    spec_module.save_in_place(spec, {"req-1"})
    again = path.read_text(encoding="utf-8")

    assert "番号を動かさないこと" in again and "契約書の別紙 3 と対応" in again
    assert yaml.safe_load(again)[0]["status"] == "approved"


def test_組み直せなければ値を守る(project: Paths, monkeypatch) -> None:
    """**賢さで安全を担保しない。** 1 件でも違えば捨てて素の書き出しに戻す。"""
    path = project.items / "requirement.yml"
    path.write_text(_FILE, encoding="utf-8", newline="\n")
    records = yamlio.load(path)

    monkeypatch.setattr(comments, "render", lambda *a: "- id: 壊れた書き出し\n")
    kept = comments.dump(path, records)

    assert kept is False                                # 落ちたことを言う
    assert yamlio.load(path) == records                 # 値はそのまま
