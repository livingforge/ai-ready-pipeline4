# 設計書を検索する（利用者向けの手引き）

ARPに取り込んだ設計書を、端末から `arp4 documents search` で検索する方法を説明します。Agentに任せずに自分でコマンドを実行する方を想定しています。

索引の内部構造、順位付けの細かい規則、Agentが従う読み取り規則は[採用済み文書の検索](document-search.md)、引数の一覧は[CLIコマンド・引数一覧](../reference/commands.md)にあります。

## 何ができるか

- Excel・Word・PowerPoint・PDF・Markdown・CSVなどから取り込んだ設計書の本文と表の行を、日本語のキーワードで検索できます。
- 結果ごとに、原本のファイル、シート・セル、Markdownなら行番号を返します。原本の該当箇所をすぐ開けます。
- Excelのセルは、画面に見えるとおりの値で検索・表示されます。数式は計算結果、日付は `2026/10/10`、割合は `50%` のように、セルの表示形式を適用した文字列です。数式そのものでは検索できません。
- 表では、行の値と列見出しを合わせて検索できます。例えば「在庫照会」の行の「タイムアウト」列を、`在庫照会 タイムアウト` で探せます（[表を見出し込みで探す](#表を見出し込みで探す)）。
- 赤字・太字・取り消し線、特定のフォントやサイズの文字を含む箇所に絞れます。キーワードなしで「赤字の箇所をすべて」も探せます（[書式で探す](#書式で探す)）。

次のことはできません。

- **意味の近い言い換えは自動で探しません。** 「時間切れ」で「タイムアウト」を探すには[用語辞書](#用語辞書で言い換えを登録する)に登録します。
- **採用前の文書は検索できません。** 取り込んだだけの候補（`documents import` の直後）や、ARPで編集中でまだ原本へ書き戻していない値は検索対象外です。検索されるのは `documents adopt` で採用した内容です。
- **画像の中の文字を新たに読み取ることはしません。** 取り込み時に保存されたOCRの文字、図形の文字、コメントは検索されます。
- **検索順位は正しさを保証しません。** 上位の結果が承認済みの仕様という意味ではありません。採否は正本管理（registry）で確認します。

## 準備：索引を作る

検索は、事前に作った索引（`.arp/cache/search/index.sqlite`）を読むだけです。最初に一度、索引を作ります。

```powershell
arp4 documents search-refresh --root C:/project
```

```json
{"detail":"Search reads this index until the next search-refresh. External file changes are not checked during search.","failed":[],"failed_documents":0,"indexed_at_unix":1791596348,"indexed_documents":2,"ok":true,"refreshed_documents":2,"state":"indexed"}
```

`indexed_documents` が索引に入っている文書数、`refreshed_documents` が今回更新した文書数です。2回目以降は変わった文書だけを更新するので、すぐ終わります。

**検索の前に索引を更新する習慣をつけてください。** 検索コマンドは原本の変更を確認しません。次の操作の後は `search-refresh` を実行しないと、検索結果が古いままになります。

- 文書を採用（`documents adopt`）・削除・編集した後
- Excelなど外部のアプリで原本を変更した後
- `git pull` などで他の人の変更を取り込んだ後

索引は各自の手元にだけ作るキャッシュで、Gitでは管理しません。他の人と共有する必要はありません。

## 基本の検索

```powershell
arp4 documents search "注文登録 タイムアウト" --root C:/project
```

`--root` には文書プロジェクトのフォルダを指定します。プロジェクトのフォルダで実行する場合は `--root .` と書けます。

### 検索語の書き方

| やりたいこと | 書き方 | 説明 |
|---|---|---|
| 複数の語をすべて含む箇所を探す | `"注文登録 タイムアウト"` | 空白で区切った語はAND条件です。 |
| 識別子・パスを探す | `"/v1/orders"` `"user_id"` | 記号もそのまま書けます。大文字小文字まで一致すると上位に来ます。 |
| 短い語を探す | `"取消"` | 1〜2文字の日本語も検索できます。 |
| どちらかを含む箇所を探す | （できません） | `OR` は演算子ではなく、「OR」という語として扱われます。語ごとに別の検索をするか、[一括検索](#複数の検索をまとめて実行する)を使います。 |

全角と半角、大文字と小文字は区別しません。`ｖ１／ＯＲＤＥＲＳ` でも `/v1/orders` が見つかります。語の途中だけでも当たります（「取消」で「注文取消」が見つかります）。

検索語は全体を引用符で囲みます。1つの検索語は1〜256文字です。

### 文書やフォルダで絞り込む

```powershell
arp4 documents search "注文" --document 設計/注文API.md --root C:/project
arp4 documents search "注文" --document 設計 --root C:/project
```

`--document` には文書ID（原本フォルダからの相対パス）か、フォルダを指定します。フォルダは区切りの単位で一致し、`設計` を指定しても `設計書/…` は含みません。

## 結果の読み方

結果はJSONで返ります。実際の例（一部を省略）を示します。

```json
{
  "ok": true,
  "items": [
    {
      "document_id": "設計/注文API.md",
      "source_path": "docs/設計/注文API.md",
      "title": "設計/注文API.md / text / 注文API / 注文登録",
      "text": "タイムアウトは30秒とする。",
      "sources": [
        {"sheet": "text", "address": "A4",
         "position": {"line_start": 7, "line_end": 7, "headings": ["注文API", "注文登録"], "kind": "paragraph"}}
      ],
      "state": {"source_current": true, "source_missing": false, "reviewed": false,
                "content_differs_from_extraction": false},
      "structure_reviewed": false
    }
  ],
  "page": {"offset": 0, "returned": 1, "total": 1, "next_offset": null},
  "revision": "7356723b…",
  "summary": {"indexed_at_unix": 1791596348, "indexed_documents": 2, "source_checked_this_search": false}
}
```

まず見る項目は次のとおりです。

| 項目 | 意味 |
|---|---|
| `ok` | `true` なら検索は完了しています。`false` のときは[困ったとき](#困ったとき)を見てください。 |
| `items[].text` | 一致した本文です。構造解釈のない表の場合は1行の値を ` \| ` でつないで返します。構造解釈のある表の行は `values` で返します（[表を見出し込みで探す](#表を見出し込みで探す)）。 |
| `items[].source_path` | 原本ファイルのパスです。 |
| `items[].title` | 文書ID・シート・見出しをつないだ、その箇所の所在です。 |
| `items[].sources[]` | 原本内の位置です。下の表を参照してください。 |
| `page.total` | 一致した総件数です。 |
| `page.next_offset` | 続きがある場合の次の開始位置です。`null` なら最後のページです。 |

原本の該当箇所は、形式ごとに次の項目で探します。

| 原本の形式 | 位置の見方 |
|---|---|
| Excel | `sheet`（シート名）と `address`（セル番地）。表の行は、その行の各セルが並びます。 |
| Markdown・テキスト | `position.line_start`〜`line_end`（行番号）と `position.headings`（属する見出し）。`sheet: "text"` と `address` はARP内部の通し番号です。 |
| Word・PowerPoint・PDF | `address` は文字列の通し番号で、ページ上の位置ではありません。`title` に含まれる見出しや本文を手がかりに原本を探してください。 |

### 一覧で見る（PowerShell）

JSONのままでは読みにくい場合は、PowerShellで表に整形できます。構造解釈のある表の行は `document_id` や `sources` を `tables` 側に持つため、[表を見出し込みで探す](#表を見出し込みで探す)の例を使います。

```powershell
$r = arp4 documents search "注文" --root C:/project | ConvertFrom-Json
$r.items | Select-Object document_id,
    @{n='場所'; e={ ($_.sources | ForEach-Object { "$($_.sheet)!$($_.address)" }) -join ',' }},
    text | Format-Table -AutoSize
$r.page
```

日本語が文字化けする場合は、先に `[Console]::OutputEncoding = [Text.Encoding]::UTF8` を実行してください。

### 結果の並び順

上から順に、次の結果が先に並びます。

1. 識別子が大文字小文字まで完全に一致したもの
2. 検索語全体がそのまま本文に含まれるもの
3. 検索語全体がタイトル（文書ID・シート・見出し）に含まれるもの（表の列見出しは含みません）
4. それ以外は、一致の度合い（本文・見出し・表見出しの一致を重み付けした値）の高いもの

加えて、次の調整をします。結果を消すことはなく、総件数にも含まれます。

- **同じ文書の同じ文面は後ろへ回します。** 別シートに同じ記載がある場合などに、異なる記載を先に確認できるようにするためです。
- **見出しだけに一致した結果は最後に回します。** 本文に検索語がなく、見出しや列見出しだけに一致した結果には `heading_only` が付きます。「注文」のような大項目の語で検索したとき、その章の全段落が上位を占めないようにするためです。`heading_only.group_hits` は、同じ見出しの下で同様に一致した件数です。

## 表を見出し込みで探す

表の行は、行の値と、その行・列に対応付けた見出しを一緒に検索します。

```powershell
arp4 documents search "在庫照会 タイムアウト" --root C:/project
```

```json
{
  "tables": {
    "t1": {
      "document_id": "設計/API一覧.xlsx", "source_path": "docs/設計/API一覧.xlsx",
      "sheet": "API一覧", "element": "table-1",
      "columns": [
        {"column": "A", "headers": ["機能"]},
        {"column": "B", "headers": ["メソッド"]},
        {"column": "C", "headers": ["パス"]},
        {"column": "D", "headers": ["タイムアウト"]}
      ],
      "state": {"source_current": true, "…": "…"}, "structure_reviewed": false
    }
  },
  "items": [
    {"kind": "row", "table_ref": "t1", "row": 5,
     "values": ["在庫照会", "GET", "/v1/stock", "5秒"]}
  ]
}
```

行の値は `items[].values` に列の順で、表の情報は `tables` にまとめて返ります。各結果の `table_ref`（この例では `t1`）が、どの表かを示します。値と同じ位置の `columns` が、そのセルの列と見出しです。この例では、D5セルの「5秒」が「タイムアウト」の値です。同じ表の結果が多くても、表の情報は1回だけ載ります。

- 段組みの見出しは `["性能", "タイムアウト"]` のように、上の見出しから順に並びます。
- 縦に結合した項目名など、別の行にある行見出しがその行にかかる場合は、`row_context` に `{"headers": [...], "text": "..."}` の組で入ります。
- 空のセルは `""` です。読み取れなかったセルや未確認のセルは `unread` にセル番地ごとの状態が入り、抽出した文字があれば値はその文字です。抽出に文字がないセルと計算結果のない数式のセルは `null` になります。
- 入れ子の表では、親の表の見出しが `headings` に、その表が属する親の行の値が見出しとの組で `within` に、外側から順に入ります。

表の行を一覧で見るには、次のようにします。

```powershell
$r = arp4 documents search "タイムアウト" --root C:/project | ConvertFrom-Json
$r.items | Where-Object table_ref | ForEach-Object {
    $t = $r.tables.($_.table_ref)
    for ($i = 0; $i -lt $_.values.Count; $i++) {
        [pscustomobject]@{文書 = $t.document_id; セル = "$($t.sheet)!$($t.columns[$i].column)$($_.row)"
            見出し = $t.columns[$i].headers -join ' / '; 値 = $_.values[$i]}
    }
} | Format-Table -AutoSize
```

**見出しとの対応付けには構造解釈が必要です。** 構造解釈は、表のどこが見出しでどこが値かをARPに記録したものです。構造解釈のない表では、行の値だけが検索されます。上の例では、構造解釈がなければ `在庫照会 GET` では見つかっても、`在庫照会 タイムアウト` では見つかりません。構造解釈は通常、arp4-structureスキルでAgentに作成・レビューさせます（[Office・PDFの構造解釈](document-structure.md)）。構造解釈を保存した後も `search-refresh` を実行してください。

`structure_reviewed: false` は、見出しの対応付けがまだレビューされていないことを示します。機械推定のままの対応付けは誤っている場合があります。

CSVの先頭行は自動では見出しとみなしません。Markdownは本文の見出し階層（`#`、`##`）を見出しとして使います。

## 書式で探す

文字の色・フォント・サイズ・太字・取り消し線で結果を絞れます。キーワードを省くと、その書式の文字がある箇所をすべて返します。

```powershell
arp4 documents search --color red --root C:/project
arp4 documents search "秒" --color red --root C:/project
```

```json
{"items":[{"formatted":["B2"],"kind":"row","row":2,"table_ref":"t1","values":["注文登録","30","秒"]},{"formatted":["B4"],"kind":"row","row":4,"table_ref":"t1","values":["在庫引当","90","秒"]},{"formatted":["E6"],"kind":"block","text":"赤字は変更箇所", …}], "page":{"total":3, …}, …}
```

`formatted` が、指定した書式に一致したセルの番地です（図形の文字は抽出結果へのポインター）。表の行では、行の中のどのセルが赤いかをここで確認します。

| やりたいこと | 書き方 |
|---|---|
| 赤い文字（濃い赤なども含む） | `--color red` |
| 特定の色だけ | `--color FF0000` |
| 赤か青 | `--color red --color blue` |
| テーマの色（アクセント2など） | `--color accent2` |
| 一部の文字だけ色が違うセル（色を問わない） | `--color mixed` |
| フォント | `--font "MS ゴシック"` |
| 14ポイント以上 | `--size 14..` |
| 太字・斜体・下線 | `--bold`・`--italic`・`--underline` |
| 取り消し線（一重・二重） | `--strike` |
| 蛍光ペン（Word・PowerPoint） | `--highlight` |
| 塗りつぶしのあるセル | `--fill` |

色の系統には `red`・`orange`・`yellow`・`green`・`cyan`・`blue`・`purple`・`pink`・`gray`・`black`・`white` を使えます。自動色（Excelの「自動」）は `black` に入ります。書式はセルや段落の文字の一部だけに付いていても一致します（「仕様**書**」のように1文字だけ太字のセルも `--bold` で見つかります）。複数の条件を付けると、すべてを満たすセルがある箇所だけを返します。条件ごとに別の文字でもよく、`--bold --color red` は太字の部分と赤い部分を持つセルを返します。

どんな色やフォントが使われているかは、先に `search-formats` で確認できます。

```powershell
arp4 documents search-formats --root C:/project
```

```json
{"items":[{"cells":9,"family":"black","hits":4,"property":"color","value":"auto"},{"cells":2,"family":"red","hits":2,"property":"color","value":"FF0000"},{"cells":1,"family":"red","hits":1,"property":"color","value":"C00000"},{"cells":1,"hits":1,"property":"color","value":"mixed"},{"cells":3,"hits":1,"property":"font","value":"Meiryo UI"}, …], …}
```

`hits` はその値だけで絞ったときの件数、`cells` はその書式の文字があるセルの数です。一部の文字だけ赤いセルは、`mixed` と、`FF0000` などその文字の色の両方に数えます。

注意点:

- **見つかるのはセルまでです。** セルの一部だけが赤い場合も見つかりますが、どの文字が赤いかは原本で確認してください。
- **以前に取り込んだ文書は取り込み直してください。** 一部の文字だけの書式は取り込み時に記録します。この機能より前に取り込んだ文書は `documents import --force` で取り込み直し、`search-refresh` を実行するまで、一部だけの書式では見つかりません。
- 条件付き書式で赤く見えるセルや、表示形式（`[赤]`）で赤く表示される数値は対象外です。
- Markdown・CSVなどフォント情報のない文書、コメント、画像のOCR文字は書式では絞れません。
- 太字は、取り込み時にセルの書式として記録される形式（Excelのセル、Word・PowerPointの段落と表のセル）だけが対象です。図形の文字の太字は対象外です。

## 件数が多いとき

既定では1回に20件を返します。続きは `page.next_offset` と `revision` を使って取得します。

```powershell
arp4 documents search "注文" --limit 5 --root C:/project
# 応答の page.next_offset が 5、revision が "de4ff5…" だった場合
arp4 documents search "注文" --limit 5 --offset 5 --revision de4ff5… --root C:/project
```

続きを取得するときは、検索語と `--document` を最初と同じにします。`--revision` を付けると、途中で索引が更新された場合にエラーで知らせます。ずれたページを気づかずに読むのを防ぐためです。エラーになったら `--offset` と `--revision` を外して最初から検索し直してください。

全件をまとめて見たい場合は `--full` を使います。`--full` は全件を1回で返すため、`--limit`・`--offset` とは同時に指定できません。件数が多い場合は `--document` で絞ってから使ってください。`--full` では次の情報も追加されます。

| 項目 | 意味 |
|---|---|
| `extraction_path` | 抽出結果ファイルのパス |
| `sources[].text` | 出典ごとの本文 |
| `match` | 識別子の完全一致・本文の一致・見出しの一致のどれに当たったか |
| `score` | 一致度の参考値（有効数字3桁）。ほかの検索の値とは比べられません。 |
| `query_terms` | 用語辞書で展開した後の検索語 |

通常の表示では、1件や表の情報が長すぎると一部の項目を省きます。省いた項目は、その結果や表の情報の `omitted_fields` に示されます。この場合、`text` が途中で切れていても、原文がそこで終わっているとは限りません。`--document` で絞って `--full` で確認してください。

## 複数の検索をまとめて実行する

検索語を引用符で分けて並べると、最大32件の検索を1回で実行できます。

```powershell
arp4 documents search "注文登録 タイムアウト" "在庫" "認証 失敗" --limit 5 --root C:/project
```

結果は `queries` に入力順で並び、それぞれに `query` と `ok` が付きます。一致しなかった検索も省略されません。`--document`・`--limit`・`--offset` はすべての検索に共通です。「AまたはB」を調べたいときにも使えます。

## 用語辞書で言い換えを登録する

プロジェクト内で同じ意味として扱う語を `.arp/search-synonyms.yml` に登録できます。

```yaml
schema_version: '1'
groups:
  - [タイムアウト, 時間切れ]
  - [ユーザーID, user_id]
```

この例では、「時間切れ」で検索すると「タイムアウト」を含む箇所も見つかります。

- 展開は、空白で区切った語の1つが登録語と完全に一致したときだけです。`注文登録 時間切れ` は `注文登録` と（`タイムアウト` または `時間切れ`）の検索になります。「時間」のように登録語の一部だけでは展開しません。
- 展開した語をさらに展開することはありません。
- 「認証」と「認可」のように意味の違う語は同じ組に入れないでください。検索結果に無関係な箇所が混ざります。
- 辞書を変えても索引の更新は不要です。次の検索から反映されます。

このファイルはチームの共有設定としてGitで管理します。別のファイルを使う場合は `--synonyms <パス>` で指定します。

## 結果の状態を確認する

各結果の `state` と `structure_reviewed` は、**最後に `search-refresh` した時点**の状態です。構造解釈のある表の行では、`tables` の表の項目に載ります。

| 項目 | `true` / `false` のとき注意すること |
|---|---|
| `state.source_current` | `false` なら、原本が採用後に変更されています。検索結果は採用時点の内容で、原本の現在の記載とは異なる可能性があります。原本を再取り込みして採用してください。 |
| `state.source_missing` | `true` なら、原本ファイルが見つかりません。 |
| `state.reviewed` | `false` なら、その文書の抽出結果はまだレビューされていません。 |
| `state.content_differs_from_extraction` | `true` なら、ARP上で値を編集しており、原本へ書き戻していない変更があります。検索結果は編集前の内容です。 |
| `structure_reviewed` | `false` なら、表見出しの対応付けがレビュー前です。 |

例えば原本のMarkdownで「30秒」を「60秒」に書き換えた場合、`search-refresh` の後も「タイムアウトは30秒とする。」が返り、`source_current` が `false` になります。「60秒」で検索できるようになるのは、`documents import`・`record`・`adopt` で採用し直し、`search-refresh` した後です。

`summary.indexed_at_unix` は索引を最後に更新した時刻（Unix時間の秒）です。

## 困ったとき

| 症状・メッセージ | 対処 |
|---|---|
| `search index unavailable; run documents search-refresh` | 索引がまだありません。`documents search-refresh` を実行します。 |
| `search index version changed; run documents search-refresh`（または `invalid search index; …`） | 索引の形式が古いか、読めません。`documents search-refresh` を実行します。 |
| 原本を直したのに古い内容が出る | `search-refresh` をしていないか、変更した原本を採用し直していません。[結果の状態を確認する](#結果の状態を確認する)を参照してください。 |
| 表の列見出しの語で見つからない | その表に構造解釈がありません。[表を見出し込みで探す](#表を見出し込みで探す)を参照してください。 |
| `search revision changed; restart at offset 0 without --revision` | ページ送りの途中で索引が更新されました。`--offset` と `--revision` を外して最初から検索し直します。 |
| `the argument '--full' cannot be used with '--limit <LIMIT>'`（指定順により逆の文言。`--offset` も同様） | `--full` は全件を返すため、件数の指定とは併用できません。どちらかを外します。 |
| `search query must contain 1..256 characters` | 検索語が空か、長すぎます。書式だけで探す場合は検索語を省きます。 |
| `unknown color …` | `--color` に指定できない値です。`RRGGBB`、色の系統（`red` など）、テーマの色枠（`accent1` など）、`auto`、`mixed` のいずれかを指定します。 |
| `ok: false` で `failed` に文書が並ぶ | 一部の文書の検証に失敗し、その文書を除いた結果が返っています。**「該当なし」と判断しないでください。** `search-refresh` を実行し、それでも失敗する場合は該当文書の状態を `documents status` で確認します。 |
| 索引が壊れた様子がある | `documents search-refresh --rebuild` で索引を作り直します。索引は再生成できるキャッシュなので、作り直しても文書は失われません。 |

検索の時間を測りたい場合は `documents search` に `--profile` を付けます。検索結果とは別に、処理時間が標準エラーへ1行のJSONで出力されます。項目の意味は[採用済み文書の検索](document-search.md#処理時間の計測)を参照してください。
