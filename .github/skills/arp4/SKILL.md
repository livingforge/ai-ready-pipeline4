---
name: arp4
description: 既存資産（Excel の設計書・ソースコード）を起点に、仕様をリレーショナルな正本データへ変換するパイプライン。機械が資料をパース結果（編集可・git 管理の Markdown）にし、エージェントがそれを読んで整理結果（型付きレコード）を書き、機械が凍結・検証してから正本（items / relations）を組み立て、設計書を生成する。意味の判断は整理層だけが行い、パースと構築は照合・採番・整合性しかやらない。「Excel の設計書を取り込んで」「既存資産から仕様を起こして」「仕様をデータとして管理して」「テーブル定義書・詳細設計書を生成して」「トレーサビリティを取って」で使う。
license: MIT
---

# arp4 ― 既存資産を仕様データの正本にする

設計書を書かない。仕様をデータとして持ち、設計書はそこから生成する。
起点になるのは既存資産で、Excel の設計書・ソースコード（Python / Java）・DDL・
Markdown の設計メモを指す。

```
既存資産（Excel・コード・DDL・Markdown）   いまある場所のまま
    │  arp4 parse             機械：資料 → パース結果（意味を判断しない）
    ▼
.arp/rounds/r001/parsed/**.md    編集可・git 管理
    │  arp4 draft             機械：コードの整理結果の骨格（文章は TODO で空く）
    │  ★ あなたの担当         ここだけが意味を判断する
    ▼
.arp/rounds/r001/organized/  ── arp4 freeze ──→ 凍結
    │  arp4 build             機械：マージ・採番・整合性（意味を判断しない）
    ▼
.arp/spec/  concepts.yml + items/ + relations/（＋ derived/ ＝ AI の解釈層）
    │  arp4 check → arp4 publish
    ▼
.arp/out/<工程>/   設計書 12 種 ＋ 決定記録 ＋ 点検の 2 枚（--audience stakeholder で 6 種）
```

**各ページは必要になったときに読む。先に全部読み込まない。**

## 打つ前に（置き場を自分で選ばない）

**`--root` を必ず明示する。**省くと置き場は cwd から決まる。エージェントは一時
フォルダで作業することがあり、そこで打つとエラーも警告も出ないまま、成果物が
セッションの終わりに消える場所へ出る。**根に指定するのは対象のリポジトリで、
あなたの作業場所ではない。**

```bash
R=/path/to/対象のリポジトリ                  # ★ あなたの作業場所ではない
arp4 init --root "$R"                        # ← 全コマンドに付ける
```

- **資料のパスも省けない。**元資料は動かさないので、arp4 には「どこを仕様として見るか」
  が分からない。渡すパスが「何を仕様として見るか」の宣言になる（根ごと渡すと
  `.venv/` も歩く。フォルダ構造は渡したパスの共通の親から写す）
- **資料だけでなくコードと DDL と設計メモも渡す。**テスト仕様書・テーブル定義書・
  課題管理表の語彙は `.sql` と `test_*.py` と `.md` にしか無い
- **arp4 が作るものは全部 `.arp/` の中に入る。**元資料を集める先（`sources/`）を作ると
  `sources.yml` の指紋が写しの指紋になり、上流が更新されても `G019` が出なくなる
- **`.arp/` を丸ごと `.gitignore` に足さない。**パース結果の「機械が出したもの」と
  「人が直したもの」の区別は初回コミットだけが持っている。無視した時点でその区別が
  消えるが、エラーにはならない（無視するなら `.arp/out/` だけ）

## あなたが担当するのは整理層だけ

`parse` / `build` / `check` / `publish` は機械の仕事である。**あなたがやるのは
`parsed/**.md` を読んで `organized/**.yml` を書くこと、この 1 つだけである。**
→ [docs/organize.md](docs/organize.md)（書き方）／[docs/sheets.md](docs/sheets.md)（資料の読み方）／
[docs/code.md](docs/code.md)（コード）／[docs/reconcile.md](docs/reconcile.md)（横断）

書き始める前に **`arp4 model`（語彙）と `arp4 schema`（形）を読む。**2 つは決める人が
違う。語彙（使ってよい `type` / `rel` / 属性）はプロジェクトが決め、形（どの欄が
要るか）は arp4 が決める。

## コマンド

| コマンド | 何をするか |
| --- | --- |
| `arp4 init` | `.arp/` の骨組みを作る |
| `arp4 model` | 使ってよい語彙（`--attributes` で属性・enum の値まで） |
| `arp4 schema` | 書いてよい形（語彙と対） |
| `arp4 parse <パス>…` | ① 既存資産 → パース結果 → [docs/parse.md](docs/parse.md) |
| `arp4 draft` | コードの整理結果の骨格を機械生成 → [docs/code.md](docs/code.md) |
| `arp4 auto <パス>…` | parse → publish を 1 コマンドで → [docs/code.md](docs/code.md) |
| `arp4 render <パス>…` | 読めなかった範囲を絵にする → [docs/parse.md](docs/parse.md) |
| `arp4 declare <型> --reason …` | 同じ構成のシートを一括で対象外宣言（表紙・改訂履歴） |
| `arp4 lint <パス>…` | 書いた 1 ファイルをその場で検査（`file:line`）→ [docs/freeze.md](docs/freeze.md) |
| `arp4 freeze --dry-run` | 残作業の一覧 → [docs/freeze.md](docs/freeze.md) |
| `arp4 freeze` | ② 凍結 |
| `arp4 build` | ③ 整理結果 → 正本 → [docs/build.md](docs/build.md) |
| `arp4 number` | 表示 ID を採番する（`check` の前に回す） |
| `arp4 check --strict` | 機械検証（`--summary` → `--code W043` で段階的に開く） |
| （手で書く）`spec/derived/` | ★ AI の解釈層。PM・顧客向けはここからしか出ない → [docs/derived.md](docs/derived.md) |
| `arp4 publish` | ⑥ 設計書を生成 → [docs/publish.md](docs/publish.md) |
| `arp4 lock` / `conform` | 標準パック準拠（CI 用） |

**検査系（`lint` / `check` / `conform`）の終了コードは 3 値である。**error = 1 /
warn のみ（`--strict`）= 2 / clean = 0。そのほかは 2 値（0 = 成功）で意味が違う。
`auto` だけ exit 3（あなたの手番）を持つ。

## 通しの手順

資産がコードだけなら 2 コマンドで終わる → [docs/code.md](docs/code.md)。

```bash
arp4 auto --root "$R" "$R"/src "$R"/tests --exclude "tests/dataset/正解/*"
#   → exit 3 で文章化スロットの一覧。<TODO …> を埋めてもう一度打つと exit 0
```

シート（Excel）が混ざるなら 1 コマンドずつ。

```bash
arp4 init   --root "$R"
arp4 model  --root "$R"                     # ★ 語彙
arp4 schema --root "$R"                     # ★ 形
arp4 parse  --root "$R" "$R"/src "$R"/ddl "$R"/資料      # ① パスは必須
arp4 draft  --root "$R"                     #   コードの骨格（シートは触らない）
arp4 render --root "$R" "$R"/資料           #   図形を絵にする（要 Excel）
arp4 declare --root "$R" 表紙 改訂履歴 --reason "仕様ではない"    # Excel のときだけ
arp4 freeze --root "$R" --dry-run           #   残作業の一覧
#   ★ parsed/ を読んで organized/ を書く（整理①）。1 ファイル書くたびに arp4 lint
#   ★ 全部書けたら横断整理（整理②）→ docs/reconcile.md
#     同一性・分類・矛盾に加えて「使い残し」（宣言済みだが 0 件の属性・関係）を必ず見る。
#     凍結すると整理結果はもう直せない
arp4 freeze --root "$R" --dry-run           #   ★ 書いたあとにもう一度（読めるかの確認）
arp4 freeze --root "$R"                     # ② 凍結
arp4 build  --root "$R"                     # ③ 正本へ
arp4 number --root "$R"                     #   採番（check より先）
arp4 check  --root "$R" --strict
arp4 check  --root "$R" --code W043 --code W047 --code W046   # ★ 空で出た列（種別.属性で出る）
#   ★ PM・顧客向けを出すなら .arp/spec/derived/ を書く → docs/derived.md
arp4 publish --root "$R"                    # ⑥
#   ★ 出したら 0_この設計書の穴.md と 0_元資料と設計書の対応.md を読む（⑦）
#     publish が通ったことは、書けているという意味ではない → docs/publish.md
```

### ⑦ 出したものを読む（ここまでが 1 ラウンド）

`publish` は正本のとおりに出すだけなので、**整理層の取りこぼしは止めずに素通りする。**
出したあとに次の 3 つを読み、直す先を仕分ける。

| 出たもの | 意味 | 直す先 |
| --- | --- | --- |
| `W043` 列が空だが**同じ名前**の別の欄に値がある | 指す先が 1 つに決まる。**書き先を間違えた** | 次のラウンドの整理① |
| `W047` 列が空で、母集合が `description` を使っている | **中身は照合していない。**「この列は空」と「誰かが `description` を持っている」しか言っていない | `description` を開く。別の値なら `W046` と同じ扱い |
| `W046` 様式にあるのに正本が値を持たない | 資料に無いのか、写していないのか**この時点では割れていない**。1 件ずつ資料に当たる | 資料に無ければ何もしない／あれば整理① |
| `P110` `P111` 正本にあるのに設計書に出ない関係・属性 | 書いたものの出口が無い | 様式に足すか、書くのをやめる |

**「資料に無い」と「写していない」は、機械には区別できない。**穴の 1 枚はどちらとも
言っていないので、**読む人が資料に当たるまで判定は終わっていない。**W047 / W046 が
出たまま配ると、読み手はどちらも「資料に無い」と受け取る。

**`W047` を `W043` として読まない。**この 2 つは件数の比が偏る ―― 実測（kotonoha r001）
では 9 件すべてが `W047` で、`W043` は 0 件だった。**`W047` の指し先（`description`）は
「値がそこにある」という意味ではない。**空の列の名前と `description` の中身を突き合わせる
処理は無く、母集合のどれか 1 件が `description` を持っていれば返る（実測では、`category`
を鳴らした `description` は別のレコードの「測り方」だった）。**「資料にはある」から
出発すると、「無い」という結論には辿り着けない** ―― 資料に無ければ空のままが正しい。

## 守ること

- **意味の判断は整理層だけ。**パース結果に書いていないことを整理結果に書かない。資料に
  無い桁数・物理名・優先度を推測で埋めない（空欄で出せば `E010` が出て人が埋める）
- **資料にある値は、宣言済みの欄へ入れる。**`description` / `note` は宣言済みの欄がどれも
  当たらない 1 列のためのものである。流すと設計書のその列は空で出て、**「資料に無い」と
  読まれる**（値はあるのに無いと報告されるのが、いちばん悪い形である）。書く前に
  `arp4 model --attributes` を引く。**この文書群に「その属性は無い」と書いてあっても、
  それを根拠にしない**（語彙は版で増える。実測で手順書のほうが古かった）
- **`publish` が通ることは完成ではない。**穴の 2 枚を読むまでが 1 ラウンドである（⑦）
- **アンカーを消さない。**パース結果は編集してよいが `<!-- a:… at=… -->` は残す。失われると
  元資料との照合が二度とできない。編集理由はコミットメッセージに残す（OCR の訂正と都合の
  いい書き換えは diff で区別がつかない）
- **凍結後は正本側で直す**（`overridden` / `known_gaps`。どちらも理由必須）
- **迷うものはまとめない。**同じ概念か迷う 2 つは別の `concept` のままにする
- **矛盾は自動解決しない。**食い違いは両論を残して課題にする
- **承認は人だけ**（すべて `status: review` で入る）。**`.arp/out/` は直接編集しない**

## 置き場

**全部 `.arp/` の中で、元資料には触らない。**

```
.arp/rounds/r001/parsed/**.md   ① の出力（編集可・git 管理）
.arp/rounds/r001/images/        シートの画像と render の絵（★ 開いて読むのはあなた）
.arp/rounds/r001/organized/     ★ あなたが書くところ
    **.yml               整理①（parsed と 1:1）
    _concepts.yml        整理②（同一性・用語・矛盾）
    _metamodel-add.yml   語彙の追加提案
.arp/rounds/r001/decisions.yml  機械（draft / build / auto）の判断の全件（事後拒否権の入口）
.arp/spec/                      正本（＋ derived/ ＝ AI の解釈層）
.arp/out/<番号>_<工程>/          生成物（直接編集しない）
.arp/out/_gate.json             通った条件の記録（publish が置く。設計書ではない）
.arp/out/findings.json          指摘の全件（check が置く。設計書ではない）
```

**`out/` のファイル数は設計書の数ではない。**上の 2 つの JSON が混じる（`publish` は
「うち設計書 N」と割って出す）。**設計書の種類を言うときは工程別の 12 種 ＋ 決定記録 ＋
点検の 2 枚**である（目次・HTML 版も同じ束に入るので、数はどれを数えたかを添えて言う）。

## 環境

```bash
arp4 --help     # ★ 最初に打つ
```

**通らなければ、そこで止めて利用者に伝える。**導入するのは利用者の仕事である。

**Windows で出力が文字化けしたら、先に端末の符号化を直す**（`arp4` は cp932 で出す）。
文字化けしたまま進むと、残作業も error も 1 行も読めない。`PYTHONIOENCODING=utf-8`
（Git Bash）／`[Console]::OutputEncoding = [Text.Encoding]::UTF8`（PowerShell）。

困ったとき → [docs/troubleshooting.md](docs/troubleshooting.md)
