---
name: arp4-setup
description: arp4 を打てる状態にする ―― 起動コマンドの決定・導入（pip install）・端末の符号化だけを扱う。「arp4 --help が通らない」「arp4 は認識されていません / command not found / not recognized」「python -m arp4 が No module named arp4」「arp4 を入れたい・環境を作りたい」「Windows で arp4 の出力が文字化けする・UnicodeEncodeError で止まる」で使う。arp4 のソースリポジトリの中にいても、入っているとは限らないので同じように使う。仕様パイプラインの進め方（parse・整理・build・publish）は arp4 スキルの担当で、こちらでは扱わない。
license: MIT
---

# arp4-setup ― arp4 を打てる状態にする

**ここは環境だけを見る。**仕様パイプラインの進め方は [arp4](../arp4/SKILL.md) にある。

やることは 1 つ ―― **通る起動コマンドを 1 つ決めて、利用者にも伝える。**

## 起動コマンドを 1 つ決める

`arp4` は console script なので、**入れた環境の `Scripts/`（Windows）／`bin/` の中に
しか無い。**「入っていない」と「PATH に無い」は別である。

```bash
A="arp4"            # ← 通ったものを入れる。以後は "$A" init --root … の形で打つ
"$A" --help
```

**venv を activate して済ませない。**エージェントは 1 コマンドごとに新しいシェルを
起こすので、activate は次のコマンドに残らない ―― 1 回目は通って 2 回目から
`not recognized` になる。**フルパスで持つ。**

## 探索順（上から。通ったら止める）

| # | 打つもの | 通ったときの意味 |
| --- | --- | --- |
| 1 | `arp4 --help` | PATH にある |
| 2 | `.venv/Scripts/arp4.exe --help`（Windows）／`.venv/bin/arp4 --help` | プロジェクトの仮想環境に入っている |
| 3 | `.venv/Scripts/python.exe -m arp4 --help`（ほかは `.venv/bin/python`） | 同上。console script が壊れている・PATH 側の別の版と競合している |
| 4 | `python -m arp4 --help` | いま `python` が指す先に入っている |

- **仮想環境の名前は `.venv` とは限らない**（`venv` / `env` / `.tox`）。ディレクトリを
  見てから 2・3 を打つ。**見当たらないときだけ 4 へ進む**
- **4 が通っても、次のシェルで通るとは限らない。**`python` の指す先はシェルによって
  違う（3.11 / 3.13 / 仮想環境）。通ったらその実体（`python -c "import sys;
  print(sys.executable)"`）をフルパスで持つ
- **全部だめなら導入する**（下）。ここで初めて「入っていない」と言える

**ソースリポジトリの中にいても、この順は飛ばさない。**arp4 のソースがあることと、
それが入っていることは別である ―― 実際、ソースリポジトリで `not recognized` が出て、
`.venv` に入っていた例がある。

## 導入する（環境を変えるので、断ってから）

**PyPI には出していない。**ソース（`pyproject.toml` に `name = "ai-ready-pipeline4"` が
書いてあるリポジトリ）を指して入れる。Python は **3.11 以上**。LLM の API 鍵は要らない。

```bash
cd <arp4 のソース>
python -m venv .venv
.venv/Scripts/python -m pip install -e ".[parse]"    # Windows。ほかは .venv/bin/python
.venv/Scripts/arp4 --help                            # ← これが通れば終わり
```

| extras | 何のために | 前提 |
| --- | --- | --- |
| （なし） | 検証・生成と、Word・PowerPoint・PDF・CSV の取り込み（PDF の読み手はここに入っている） | ― |
| `[parse]` | `arp4 parse` が **Excel** を読む（openpyxl） | ― |
| `[render]` | `arp4 render` がシートを画像化する | **Windows ＋ Microsoft Excel** |

**Excel の資料を扱うなら `[parse]` は要る。**`[render]` に代替は無く、入っていない環境
では図が `未読取` のまま次のラウンドへ持ち越される（黙って対象外にはならない）。

**ソースが見当たらないときは、そこで止めて利用者に伝える。**どの Python へ入れるかは
利用者の決めることで、勝手に増やすと**次に打つ人が別の arp4 を掴む。**

## 症状から引く

| 出たもの | 何が起きているか | 次にやること |
| --- | --- | --- |
| `arp4 : 用語 'arp4' は…認識されません` ／ `not recognized` ／ `command not found` | **入っていないとは限らない。**console script の置き場が PATH に無いだけ | 探索順 2・3。仮想環境があるならほぼこれ |
| `No module named arp4` | その `python` に入っていない。**ソースの直下で打っても同じ**（src レイアウトなので、カレントからは import できない） | 探索順 2・3 → だめなら導入 |
| pytest は通るのに `arp4` が起動しない | `pyproject.toml` の `pythonpath = ["src"]` は **pytest だけの設定**である。テストが通ることは、導入されていることの証明にならない | 同上 |
| 1 回目は通ったのに 2 回目から通らない | activate が次のシェルに残っていない | フルパスで持つ |
| 端末を変えたら通らなくなった | `python` の指す先がシェルごとに違う | 同上 |

## 出力が文字化けする（Windows）

**先に端末の符号化を直す。**`arp4` は cp932 で出すので、化けたまま進むと残作業も
error も 1 行も読めない。`UnicodeEncodeError` で止まるのも同じ原因である。

| 端末 | 打つもの |
| --- | --- |
| Git Bash | `export PYTHONIOENCODING=utf-8` |
| PowerShell | `[Console]::OutputEncoding = [Text.Encoding]::UTF8` |

## 終わり方

`<決めた起動コマンド> --help` が exit 0 で通ったら終わり。**決めたコマンドを利用者に
そのまま伝える**（次のセッションでまた探すことになる）。

そのうえで [arp4](../arp4/SKILL.md) へ戻る。**手順書の `arp4` は、決めたコマンドに
全部読み替える**（`--root` は読み替えではなく、必ず明示する側の話である）。
