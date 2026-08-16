"""生成物の HTML の**見た目**。設計書は Excel で読まれてきたので、そこへ寄せる。

置き場を :mod:`arp4.publish` から分けたのは、同じ見た目を使うところが 3 つに
増えたためである（設計書・穴の 1 枚・元資料の対応表）。実際 :mod:`arp4.holes` は
``<link rel="stylesheet" href="">`` という**空のリンク**を出していて、穴の 1 枚
だけが罫線も色も無い素の HTML だった ―― 束の中でいちばん先に読ませたい 1 枚が、
いちばん読みにくい形で出ていた。見た目を 1 か所に置けば、この形の抜けは起きない。

**なぜ Excel に寄せるか。** 日本の受託開発で設計書は Excel で回覧されてきたので、
読み手が最初に打つ操作は決まっている ―― 見出しを固定する、幅を広げる、絞り込む、
並べ替える。HTML はそのどれも持っていなかったので、**同じ内容なのに Excel より
読めない**。実測（``.arp/out`` 13 ファイル）で困っていたのは装飾ではなく 4 つ:

============================  ==================================================
見出しが流れる                詳細設計書に **145 行**の表があるのに ``sticky`` が無い
横に入らない                  ``max-width: 1180px`` に **17 列**のマトリクスを押し込む
行の高さが揃わない            **580 字**のセルが 1 行だけ縦に伸びて表が比べられない
どこを見ているか分からない    行にも列にも選択の印が無い
============================  ==================================================

**行番号の溝は付けない。** Excel の 1・2・3… に当たるものだが、arp4 は
「番号で引ける」を**表示 ID**（``FR-005``）に寄せると決めている（決定 17）。
行位置は再生成のたびに動くので、それを引用できる形で出すと **章番号を畳んだとき
（決定 18）に捨てたはずの「古くなる番号」**が表の中へ戻ってくる。代わりに
**表示 ID の列を固定**する ―― Excel の「先頭列の固定」と同じ操作で、しかも
固定される値のほうが安定している。
"""

from __future__ import annotations

import html

#: 上の帯（絞り込み）の高さ。見出しの ``sticky`` の位置と、見出しへ飛んだときの
#: ``scroll-margin`` がこれに合う ―― 3 か所が別々の値を持つと、番号で飛んだ行が
#: 帯の裏に隠れる。
_BAR = "2.5rem"

#: セルを畳む閾値（字数）。**超えたものだけ** ``.clip`` で包む。
#: 実測の最長は課題管理表の 580 字・要件定義書の 361 字で、1 行だけが縦に伸びると
#: 表は「並べて比べる道具」でなくなる。全セルを包むと 267 行 × 7 列ぶんの
#: ``<div>`` が増えるだけなので、長いものだけにする。
CLIP_AT = 120

#: 配色の選択を覚えておく鍵。**束の中の 14 枚で同じ鍵**を使うので、1 枚で
#: 切り替えれば次に開いた設計書も同じ配色で出る（``file://`` への保存を
#: 許さない閲覧環境ではそのページだけに効く ―― 例外は握り潰して黙って進む）。
THEME_KEY = "arp4-theme"

#: 暗い側の配色。**明るい側と同じ役割の分かれ方**を保つ（見出しは青、選択は緑）。
#: Excel の黒テーマから取っている（地 #1b1b1b・格子は地より少し明るい灰）。
_DARK_VARS = """\
--paper: #1b1b1b; --ink: #e6e6e6; --dim: #a6a6a6; --line: #6b6b6b;
    --grid: #3d3d3d; --head: #23394f; --headink: #cfe0f5;
    --chrome: #2b2b2b; --tab: #242424; --band: #212528;
    --sel: #22402f; --mark: #4a3f1a; --accent: #4cb07a; --link: #6cb6ff;"""


def _dark(scope: str) -> str:
    """暗い側の規則を、与えられた入れ物の下に組む。

    **同じ配色を 2 か所へ出すためにある。** 切り替えは 3 つの状態を持つ ――
    画面の設定に従う（既定）／明るいに固定／暗いに固定 ―― ので、暗い側は
    「画面が暗くて固定されていない」と「暗いに固定した」の両方で要る。
    手で 2 度書くと、**片方だけ直した日から 2 つの暗い画面が別物になる。**
    """
    return f"""\
{scope} {{ {_DARK_VARS} }}
{scope} table.grid .colpick {{ box-shadow: inset 0 0 0 99em rgba(76, 176, 122, .12); }}
{scope} .forced {{ background: #3a1a17; color: #f4c7c3; border-color: #d9534f; }}
{scope} .bar .sun {{ display: inline; }}
{scope} .bar .moon {{ display: none; }}"""


STYLE = f"""\
/* 色はExcel から取る。作った色を並べると「Excel 風」にはなっても、
 * 読み手が Excel で覚えている意味（この青は見出し・この緑は選択）が働かない。
 *
 * --head    #ddebf7  「青, アクセント1, 白 + 80%」。日本の設計書の見出し行の定番
 * --headink #1f3864  「青, アクセント1, 黒 + 50%」
 * --band    #eff5fc   テーブルスタイルの縞（既定の #d9e1f2 は 267 行だと重い）
 * --accent  #107c41   Excel の緑。選択と枠にだけ使う（Excel と同じ役目）
 * --link    #0563c1   Office のハイパーリンク色
 * --mark    #fff2cc  「黄, アクセント4, 白 + 80%」＝ 塗りつぶしの黄色
 * --chrome  #f3f2f1   リボン・シート見出しの地
 * --grid / --line     細罫線と太罫線。設計書の格子は黒に近い細線である
 *
 * 青（見出し）と緑（選択）を混ぜているのは Excel がそうだからである。* テーブルの書式は青系、選択・枠の固定まわりは緑系で、役割で色が分かれる。
 */
:root {{
  color-scheme: light dark;
  --paper: #ffffff; --ink: #1f1f1f; --dim: #595959;
  --line: #7f7f7f; --grid: #b4b4b4; --head: #ddebf7; --headink: #1f3864;
  --chrome: #f3f2f1; --tab: #e6e6e6;
  --band: #eff5fc; --sel: #dff0e4; --mark: #fff2cc;
  --accent: #107c41; --link: #0563c1;
  --bar: {_BAR};
}}
* {{ box-sizing: border-box; }}
body {{ margin: 0; padding: 0 0 2.9rem; background: var(--paper); color: var(--ink);
  font-family: "Yu Gothic", "Hiragino Kaku Gothic ProN", Meiryo, sans-serif;
  line-height: 1.7; }}
.wrap {{ max-width: 1240px; margin: 0 auto; padding: 1.2rem 1.4rem 2rem; }}
a {{ color: var(--link); }}
h1 {{ font-size: 1.55rem; margin: .5rem 0 1rem; padding-bottom: .4rem;
  border-bottom: 3px solid var(--accent); }}
h2 {{ margin-top: 2.4rem; padding-left: .6rem; border-left: 6px solid var(--accent);
  scroll-margin-top: 3.4rem; }}
h3 {{ margin-top: 1.6rem; color: var(--dim); scroll-margin-top: 3.4rem; }}
.meta {{ color: var(--dim); font-size: .85rem; }}
.empty {{ color: var(--dim); font-style: italic; }}

/* 上の帯。Excel の「データ」タブに当たるもの。表より上に固定する。*/
.bar {{ position: sticky; top: 0; z-index: 30; height: var(--bar);
  display: flex; align-items: center; gap: .5rem; padding: 0 .8rem;
  font-size: .82rem; background: var(--chrome); border-bottom: 1px solid var(--line); }}
.bar input, .bar button {{ font: inherit; padding: .12rem .5rem; color: var(--ink);
  background: var(--paper); border: 1px solid var(--line); border-radius: 3px; }}
.bar input {{ flex: 0 1 17rem; }}
/* 配色の切り替え。帯の右端。いま押すと何になるかを絵で出す
 * （暗い画面なら太陽 ＝ 明るくする、明るい画面なら月 ＝ 暗くする）。*/
.bar .theme {{ padding: .1rem .4rem; line-height: 1; }}
.bar .ico {{ display: inline; vertical-align: -2px; fill: currentColor; stroke: none; }}
.bar .ico .ray {{ fill: none; stroke: currentColor; stroke-width: 1.5;
  stroke-linecap: round; }}
/* 既定（明るい画面）は月だけ。`.sun` 単独では上の `.bar .ico` に詳細度で
 * 負けて両方出る。順番ではなく詳細度で決まるので、同じ深さで書く。*/
.bar .sun {{ display: none; }}
.bar button {{ cursor: pointer; }}
.bar .grow {{ flex: 1 1 auto; }}
.bar .hit {{ color: var(--dim); font-variant-numeric: tabular-nums; }}

/* 概要。数えれば出るものだけを置く（→ arp4.publish.Brief）。*/
.brief {{ margin: 1rem 0 1.6rem; padding: .8rem 1rem; font-size: .88rem;
  background: var(--chrome); border: 1px solid var(--line); }}
.brief dl {{ margin: 0; display: grid; gap: .5rem 1.2rem;
  grid-template-columns: repeat(auto-fit, minmax(11rem, 1fr)); }}
.brief dt {{ color: var(--dim); font-size: .78rem; }}
.brief dd {{ margin: 0; font-weight: 700; }}
.brief dd span {{ font-weight: 400; color: var(--dim); font-size: .82rem; }}
.brief p {{ margin: .7rem 0 0; color: var(--dim); font-size: .82rem; }}

/* 表。ここが本体。枠を固定し、幅は列に決めさせる。
 *
 * 縦長の表（実測で詳細設計書に 145 行）と横長の表（権限マトリクス 17 列）で
 * 要る仕掛けが違うので、囲いを 2 通りにする。両方を 1 つにできないのは、
 * ``overflow-x: auto`` を掛けた囲いは縦にも巻き取る箱になり、その中の
 * ``position: sticky`` が画面ではなく箱に対して効くためである。箱は縦に
 * 伸びきっているので、画面を送っても見出しは 1 度も張り付かない。いちばん
 * 効かせたいところで黙って効かなくなるので、囲いを分ける。
 *
 * .sheet       縦長の表。囲いは巻き取らず、見出しが画面に張り付く
 * .sheet.pan   横長の表。Excel の枠の固定と同じ窓（縦横に巻き取る）
 */
.sheet {{ margin: .7rem 0 1.4rem;
  background: var(--paper); border: 1px solid var(--line); }}
.sheet.pan {{ overflow: auto; max-height: calc(100vh - 5rem); }}
.sheet.none {{ display: none; }}
table.grid {{ border-collapse: separate; border-spacing: 0;
  width: 100%; font-size: .85rem; }}
.sheet.pan table.grid {{ width: max-content; min-width: 100%; }}
table.grid th, table.grid td {{ padding: .34rem .55rem; text-align: left;
  vertical-align: top; max-width: 34rem;
  border-right: 1px solid var(--grid); border-bottom: 1px solid var(--grid); }}
table.grid thead th {{ position: sticky; top: var(--bar); z-index: 3;
  background: var(--head); color: var(--headink); font-weight: 700;
  white-space: nowrap; cursor: pointer;
  user-select: none; border-bottom: 2px solid var(--line); }}
.sheet.pan table.grid thead th {{ top: 0; }}
table.grid thead th.up::after {{ content: " \\25b2"; color: var(--accent); }}
table.grid thead th.down::after {{ content: " \\25bc"; color: var(--accent); }}
table.grid tbody tr > * {{ background: var(--paper); }}
table.grid tbody tr:nth-child(even) > * {{ background: var(--band); }}
table.grid tbody tr:hover > * {{ background: var(--sel); }}
table.grid tbody tr.pick > * {{ background: var(--sel); }}
/* 先頭列の固定。Excel の「先頭列の固定」。固定するのは 1 列目である
 * （表示 ID の列が 1 列目とは限らず、3 列目を left: 0 で貼ると 1・2 列目の上に
 * 乗って升がずれる）。設計書の 1 列目はたいてい表示 ID なので、ずれる危険を
 * 冒してまで列を選び直す理由が無い。*/
table.grid td.k, table.grid thead th:first-child {{ position: sticky; left: 0;
  z-index: 2; white-space: nowrap;
  /* 枠の固定の境目は Excel と同じく太い線。ここから左は動かないという印。*/
  border-right: 2px solid var(--line); }}
table.grid td.k {{ font-weight: 700; }}
table.grid thead th:first-child {{ z-index: 4; }}
/* 選んだ列。どの背景の上でも同じ濃さで乗る（升の色を壊さない）。*/
table.grid .colpick {{ box-shadow: inset 0 0 0 99em rgba(16, 124, 65, .07); }}
td.src {{ font-size: .78rem; color: var(--dim); }}
td[id] {{ scroll-margin-top: 3.6rem; }}
td:target {{ outline: 2px solid var(--accent); outline-offset: -2px; }}
/* 縞より強く書く。`tbody tr:nth-child(even)` に負けると、番号で飛んだ行の
 * うち偶数行だけが光らない（半分の行でだけ壊れるので気づきにくい）。*/
table.grid tbody tr:has(td:target) > * {{ background: var(--mark); }}
/* 長いセル。既定で 3 行に畳み、押すと開く（Excel の行の高さ）。*/
.clip {{ display: -webkit-box; -webkit-box-orient: vertical; -webkit-line-clamp: 3;
  overflow: hidden; cursor: zoom-in; }}
.clip.open {{ display: block; -webkit-line-clamp: none; cursor: zoom-out; }}
body.tall .clip {{ display: block; -webkit-line-clamp: none; }}

/* シート見出し。Excel と同じく下に置く（上は表の見出しが使う）。*/
.tabs {{ position: fixed; left: 0; right: 0; bottom: 0; z-index: 40;
  display: flex; gap: 2px; overflow-x: auto; white-space: nowrap;
  padding: .25rem .5rem 0; background: var(--tab); border-top: 1px solid var(--line); }}
.tabs a {{ flex: 0 0 auto; padding: .2rem .75rem .25rem; font-size: .8rem;
  color: var(--dim); text-decoration: none; background: var(--chrome);
  border: 1px solid var(--line); border-bottom: none; border-radius: 4px 4px 0 0; }}
/* いま開いているシートは白地に緑の太字。Excel のシート見出しと同じ。*/
.tabs a.on {{ background: var(--paper); color: var(--accent); font-weight: 700; }}

/* 見取り図（→ arp4.figure）。正本にある関係だけを描く。*/
figure.map {{ margin: 1rem 0 1.6rem; overflow-x: auto; }}
figure.map svg {{ display: block; }}
figure.map figcaption {{ color: var(--dim); font-size: .82rem; margin-top: .4rem; }}
.map .box {{ fill: var(--head); stroke: var(--line); }}
.map a:hover .box {{ fill: var(--sel); }}
.map .lane {{ fill: var(--chrome); stroke: none; }}
.map text {{ font-family: inherit; font-size: 11px; fill: var(--headink); }}
.map .sub {{ font-size: 9.5px; fill: var(--dim); }}
.map .ph {{ font-size: 10px; fill: var(--dim); }}
.map .edge {{ stroke: var(--accent); fill: none; opacity: .5; }}
.map .head {{ fill: var(--accent); opacity: .5; }}

.cover {{ border: 2px solid var(--accent); padding: 1.2rem 1.4rem; margin: 1.2rem 0 1.8rem; }}
.cover dl {{ display: grid; grid-template-columns: 9rem 1fr; gap: .3rem 1rem; margin: 0; }}
.cover dt {{ color: var(--dim); }} .cover dd {{ margin: 0; }}
nav ol {{ margin: .3rem 0; padding-left: 0; }} nav a {{ text-decoration: none; }}
/* --force の痕跡。読み飛ばせない位置と色にする（→ arp4.gate）。
 * 赤は Excel の標準の赤（#c00000）。「エラー」と読める色を作らない。*/
.forced {{ border: 2px solid #c00000; background: #fbe5e5; color: #7b0000;
  padding: .9rem 1.1rem; margin: 1.2rem 0; border-radius: 4px; }}
.forced p {{ margin: .3rem 0; }}

/* 配色は 3 つの状態を持つ。既定は「画面の設定に従う」である。開いた人が
 * 何も選んでいないうちから、こちらの好みを押し付けない。
 *
 *   data-theme 無し   画面の設定に従う（下のメディアクエリ）
 *   data-theme=light  明るいに固定（画面が暗くても明るく出る）
 *   data-theme=dark   暗いに固定（画面が明るくても暗く出る）
 *
 * 固定を先に書かないこと。メディアクエリのほうが後だと、暗い画面で
 * `light` を選んだ人に効かない。*/
@media (prefers-color-scheme: dark) {{
{_dark(':root:not([data-theme="light"])')}
}}
{_dark(':root[data-theme="dark"]')}
/* 紙。帯とタブは出さず、見出しは各ページの頭で繰り返す。*/
@media print {{
  body {{ padding: 0; }}
  .bar, .tabs, .brief p {{ display: none; }}
  .wrap {{ max-width: none; padding: 0; }}
  .sheet {{ overflow: visible; border: none; }}
  table.grid {{ width: 100%; font-size: .76rem; }}
  table.grid thead {{ display: table-header-group; }}
  table.grid thead th, table.grid td.k {{ position: static; }}
  .clip {{ display: block; -webkit-line-clamp: none; }}
  h2 {{ page-break-before: always; }}
}}
"""

#: 画面の操作。**素の JavaScript 1 本**である ―― 依存は PyYAML だけという約束に
#: 加えて、生成物は ``file://`` で開かれるので CDN も ``fetch`` も使えない
#: （CORS で黙って死ぬ）。持たせたのは Excel を開いた人が最初に打つ 4 つだけ:
#: 絞り込み・並べ替え・行の高さ・選択の印。
SCRIPT = ("""\
(function () {
  var KEY = '""" + THEME_KEY + """';
  var all = function (s, r) { return [].slice.call((r || document).querySelectorAll(s)); };

  var root = document.documentElement;
  var theme = document.getElementById('arp-theme');
  if (theme) {
    theme.addEventListener('click', function () {
      var set = root.getAttribute('data-theme');
      var dark = set ? set === 'dark'
        : window.matchMedia('(prefers-color-scheme: dark)').matches;
      var next = dark ? 'light' : 'dark';
      root.setAttribute('data-theme', next);
      // 覚えられない閲覧環境（file:// への保存を許さない設定）でも止まらない。
      try { localStorage.setItem(KEY, next); } catch (e) {}
    });
  }

  var box = document.getElementById('arp-q');
  var hit = document.getElementById('arp-hit');
  var tall = document.getElementById('arp-tall');

  function filter() {
    var v = (box.value || '').trim().toLowerCase();
    var shown = 0, total = 0;
    all('table.grid').forEach(function (t) {
      var kept = 0;
      all('tbody tr', t).forEach(function (r) {
        total++;
        var ok = !v || r.textContent.toLowerCase().indexOf(v) >= 0;
        r.style.display = ok ? '' : 'none';
        if (ok) { kept++; shown++; }
      });
      var sheet = t.parentNode;
      if (sheet && sheet.classList) { sheet.classList.toggle('none', !!v && kept === 0); }
    });
    hit.textContent = v ? shown + ' / ' + total + ' \\u884c' : '';
  }
  if (box && hit) { box.addEventListener('input', filter); }
  if (tall) {
    tall.addEventListener('click', function () {
      var on = document.body.classList.toggle('tall');
      tall.textContent = on ? '\\u884c\\u3092\\u7573\\u3080' : '\\u884c\\u3092\\u5e83\\u3052\\u308b';
    });
  }

  var text = function (row, i) { var c = row.cells[i]; return c ? c.textContent.trim() : ''; };
  var NUM = /^[-+]?[0-9][0-9,.]*$/;
  all('table.grid').forEach(function (t) {
    var body = t.tBodies[0];
    if (!body) { return; }
    var origin = [].slice.call(body.rows);
    all('thead th', t).forEach(function (th, i) {
      th.addEventListener('click', function () {
        var dir = th.classList.contains('up') ? 'down'
                : th.classList.contains('down') ? '' : 'up';
        all('thead th', t).forEach(function (o) { o.classList.remove('up', 'down'); });
        if (dir) { th.classList.add(dir); }
        var rows = origin.slice();
        if (dir) {
          rows.sort(function (a, b) {
            var x = text(a, i), y = text(b, i), d;
            if (NUM.test(x) && NUM.test(y)) {
              d = parseFloat(x.replace(/,/g, '')) - parseFloat(y.replace(/,/g, ''));
            } else { d = x.localeCompare(y, 'ja'); }
            return dir === 'up' ? d : -d;
          });
        }
        rows.forEach(function (r) { body.appendChild(r); });
      });
    });
  });

  document.addEventListener('click', function (e) {
    var node = e.target;
    if (!node || !node.closest || node.closest('a') || node.closest('thead')) { return; }
    var cell = node.closest('td');
    var table = cell && cell.closest('table.grid');
    if (!table) { return; }
    all('.colpick', table).forEach(function (c) { c.classList.remove('colpick'); });
    all('tr.pick', table).forEach(function (r) { r.classList.remove('pick'); });
    cell.parentNode.classList.add('pick');
    var i = cell.cellIndex;
    all('tr', table).forEach(function (r) {
      if (r.cells[i]) { r.cells[i].classList.add('colpick'); }
    });
    var clip = cell.querySelector('.clip');
    if (clip) { clip.classList.toggle('open'); }
  });

  var tabs = all('.tabs a');
  if (tabs.length) {
    var heads = tabs.map(function (a) { return document.getElementById(a.hash.slice(1)); });
    var mark = function () {
      var y = window.pageYOffset + 90, at = 0;
      heads.forEach(function (h, i) { if (h && h.offsetTop <= y) { at = i; } });
      tabs.forEach(function (a, i) { a.classList.toggle('on', i === at); });
    };
    window.addEventListener('scroll', mark, { passive: true });
    mark();
  }
})();
""")


#: **ページの外へ出るリンクに付ける。** 設計書は「いま読んでいる表」に居場所が
#: あるものなので、出典を 1 つ確かめるたびに戻るボタンを押させると、どの行を
#: 見ていたかを読み手が覚えていることになる（実測でトレーサビリティ・マトリクスの
#: 1 行には表示 ID が最大 6 個並ぶ）。Excel でも他のブックへのハイパーリンクは
#: 別のウィンドウで開く。
#:
#: **ページの中の飛び先には付けない。** 目次から章へ、シート見出しから表へ、
#: 同じページの ``#FR-005`` へ ―― これは Excel でいうシートの移動であって、
#: 別のタブで開いたら**同じ文書が 2 つ開く**。``<base target="_blank">`` で
#: 一括指定できないのはこのためで、あれは同一ページの断片にも掛かる。
NEW_TAB = ' target="_blank"'


#: 選んだ配色を**描く前に**当てる 1 行。``<body>`` の中で当てると、暗い画面で
#: 「明るい」を選んだ人に**一瞬だけ暗い画面が出る**（ページを開くたびに光る）。
_PREFER = ("<script>try{var t=localStorage.getItem('" + THEME_KEY + "');"
           "if(t)document.documentElement.setAttribute('data-theme',t);}"
           "catch(e){}</script>")


def head(title: str) -> list[str]:
    """``<!doctype>`` から ``<body>`` まで。**体裁は 1 か所からしか出ない。**"""
    return ["<!doctype html>", '<html lang="ja"><head><meta charset="utf-8">',
            '<meta name="viewport" content="width=device-width,initial-scale=1">',
            f"<title>{html.escape(title)}</title>", _PREFER,
            f"<style>{STYLE}</style>", "</head><body>"]


#: 配色の切り替えの絵。**外部の字形に頼らない**（絵文字は環境で形も幅も変わり、
#: 帯の高さが動く）。いま押すと何になるかを描く ―― 暗い画面では太陽、
#: 明るい画面では月。
_SUN = ('<svg class="ico sun" viewBox="0 0 16 16" width="14" height="14"'
        ' aria-hidden="true"><circle cx="8" cy="8" r="3.1"/>'
        '<path class="ray" d="M8 .9v2.1M8 13v2.1M.9 8h2.1M13 8h2.1'
        'M3 3l1.5 1.5M11.5 11.5L13 13M13 3l-1.5 1.5M4.5 11.5L3 13"/></svg>')
_MOON = ('<svg class="ico moon" viewBox="0 0 16 16" width="14" height="14"'
         ' aria-hidden="true"><path d="M13.6 10.3A5.9 5.9 0 0 1 5.7 2.4'
         'a5.9 5.9 0 1 0 7.9 7.9z"/></svg>')

#: 帯の右端に置く配色の切り替え。**どのページにも出す**（表を持たない目次でも
#: 切り替えたい ―― そこだけ地の色が変わらないと、束の中で 1 枚だけ浮く）。
_THEME = ('<button id="arp-theme" class="theme" type="button"'
          ' title="配色を切り替える（明るい / 暗い）"'
          f' aria-label="配色を切り替える">{_SUN}{_MOON}</button>')


def toolbar(hint: str = "絞り込み（行を隠します）", filters: bool = True) -> str:
    """帯（ヘッダー）。``filters`` が偽なら配色の切り替えだけを出す。

    ボタンを増やさない ―― 並べ替えは見出しを押す、選択は升を押すで足りる。
    帯に並べたものが多いほど、読み手は「どれが表の状態を変えたか」を見失う。
    **表を持たないページでも帯は出す** ―― 配色の切り替えはどこにでも要る。
    """
    if not filters:
        return f'<div class="bar"><span class="grow"></span>{_THEME}</div>'
    return ('<div class="bar">'
            f'<input id="arp-q" type="search" placeholder="{html.escape(hint)}">'
            '<span id="arp-hit" class="hit"></span>'
            '<span class="grow"></span>'
            '<button id="arp-tall" type="button">行を広げる</button>'
            '<span class="hit">見出しを押すと並べ替え</span>'
            f"{_THEME}</div>")


def tabs(entries: list[tuple[str, str]]) -> str:
    """下端のシート見出し。``entries`` は ``(飛び先の id, 見出し)``。

    Excel はシート見出しを**下**に置く。上に置くと表の見出し（``sticky``）と
    重なるので、位置まで合わせるほうが読み手の手が動かない。
    """
    if len(entries) < 2:
        return ""                            # 1 枚しか無いならタブは意味を持たない
    links = "".join(f'<a href="#{html.escape(target)}">{html.escape(label)}</a>'
                    for target, label in entries)
    return f'<nav class="tabs">{links}</nav>'


def tail(sheet_tabs: str = "", script: bool = True) -> list[str]:
    parts = [sheet_tabs] if sheet_tabs else []
    if script:
        parts.append(f"<script>{SCRIPT}</script>")
    parts.append("</body></html>")
    return parts


#: これより列が多い表は「窓」にする（→ ``.sheet.pan``）。実測の分かれ目は
#: **8 列**である ―― 詳細設計書の 145 行の表は 7 列で画面幅に入り、CRUD 図
#: （15 列）と権限マトリクス（17 列）は入らない。列数で決めるのは、幅は描画して
#: みないと分からないのに対し、**列の多さは組み立てた時点で分かる**からである。
PAN_AT = 8


def sheet(columns: int) -> str:
    """表を包む囲いの開きタグ。列数で「窓」にするかを決める。"""
    return '<div class="sheet pan">' if columns > PAN_AT else '<div class="sheet">'


def grid(columns: list[str], rows: list[list[str]]) -> str:
    """素の文字列の表を 1 枚のシートに。**穴の 1 枚と元資料の対応表が使う。**

    設計書の表（:func:`arp4.publish._html`）はリンクとアンカーを持つので別に
    組むが、**見た目の側は 1 か所から出す** ―― 3 枚が別々に組んでいたころ、
    穴の 1 枚だけが罫線も固定も持っていなかった。
    """
    escape = html.escape
    head = "".join(f"<th>{escape(c)}</th>" for c in columns)
    body = ""
    for row in rows:
        body += "<tr>"
        for index, value in enumerate(row):
            text = str(value)
            klass = ' class="k"' if index == 0 else ""
            body += f"<td{klass}>{cell(text, escape(text))}</td>"
        body += "</tr>"
    return (f'{sheet(len(columns))}<table class="grid"><thead><tr>{head}</tr>'
            f"</thead><tbody>{body}</tbody></table></div>")


def cell(value: str, escaped: str) -> str:
    """升の中身。**長いものだけ畳む**（→ :data:`CLIP_AT`）。

    ``value`` は素の文字、``escaped`` は既に HTML になっているもの
    （表示 ID のリンクが入っている）。**長さは素の文字で測る** ―― タグの分まで
    数えると、リンクの多い升だけが理由なく畳まれる。
    """
    if len(value) <= CLIP_AT:
        return escaped
    return f'<div class="clip">{escaped}</div>'
