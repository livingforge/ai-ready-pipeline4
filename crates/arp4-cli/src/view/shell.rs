//! The frame around the parts of a page, laid out as the program of the
//! original shows a document in a browser: a bar naming the document, an
//! Excel sheet at a time chosen by its tab under a formula bar that shows the
//! selected cell, a PowerPoint slide at a time chosen from thumbnails with its
//! notes under it, a Word document on its paper with its comments beside it,
//! and PDF pages one under another. The differences the content makes are
//! listed beside the document, each taking the reader to its place, and a
//! status bar sets the zoom. The structure view paints how a reader of the
//! body takes the document: each table's cells by their role, and for a
//! chosen cell its key, the cells heading it and the nodes it is under.
use super::*;

/// The page around `sections`, shown as `layout` shows them.
pub(super) fn shell(
    document: &Document<'_>,
    layout: Layout,
    sections: &[Section],
    out: &Out,
) -> String {
    let (name, accent) = match layout {
        Layout::Sheets => ("sheets", "#217346"),
        Layout::Slides => ("slides", "#c43e1c"),
        Layout::Paper => ("paper", "#2b579a"),
        Layout::Pages => ("pages", "#b30b00"),
        Layout::Records => ("records", "#3b5b7d"),
    };
    let file = document.id.rsplit('/').next().unwrap_or(document.id);
    let mut html = String::new();
    let _ = write!(
        html,
        r#"<!doctype html><html lang="ja"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>{}</title><style>:root{{--accent:{accent}}}{STYLE}</style></head><body data-layout="{name}"><header class="bar"><span class="name" title="{}">{}</span>"#,
        escape(file),
        escape(document.source),
        escape(file),
    );
    if document.proposal {
        html.push_str(
            r#"<span class="badge" title="取り込んだ候補（proposal）の内容です">候補</span>"#,
        );
    }
    if document.operations > 0 {
        let _ = write!(
            html,
            r#"<span class="badge warn" title="表示は原本の配置のままです。操作が加える行・列の値は各ページの末尾に一覧します">未適用の操作 {} 件</span>"#,
            document.operations
        );
    }
    html.push_str(r#"<span class="grow"></span>"#);
    html.push_str(r#"<div class="modes" role="group"><button data-mode="view" aria-pressed="true" title="原本の見た目で表示">表示</button><button data-mode="structure" aria-pressed="false" title="AIが本文YAMLから読み取る構造を表示">構造</button></div>"#);
    if !out.differences.is_empty() {
        let _ = write!(
            html,
            r#"<button id="diff-toggle" class="diff-button" aria-pressed="true" title="原本と異なる値の一覧">差異 {} 件</button>"#,
            out.differences.len()
        );
    }
    html.push_str("</header>");
    if layout == Layout::Sheets {
        html.push_str(
            r#"<div class="fbar"><div id="namebox" class="namebox"></div><span class="fx">fx</span><div id="formula" class="formula"></div></div>"#,
        );
    }
    html.push_str(r#"<div class="body">"#);
    if layout == Layout::Slides {
        html.push_str(r#"<nav class="rail">"#);
        for (index, section) in sections.iter().enumerate() {
            let _ = write!(
                html,
                r#"<button class="thumb{}" data-go="{index}" title="{}"><span class="n">{}</span><span class="mini"></span></button>"#,
                if section.hidden { " off" } else { "" },
                escape(&section.title),
                index + 1
            );
        }
        html.push_str("</nav>");
    }
    html.push_str(r#"<main id="stage" class="stage">"#);
    for (index, section) in sections.iter().enumerate() {
        let one_at_a_time = matches!(layout, Layout::Sheets | Layout::Slides);
        let _ = write!(
            html,
            r#"<section class="view" data-page="{index}"{}>"#,
            if one_at_a_time && index > 0 {
                " hidden"
            } else {
                ""
            }
        );
        if layout == Layout::Slides {
            let _ = write!(
                html,
                r#"<div class="fit"><div class="slide-box">{}</div></div><div class="notes-pane">{}</div>"#,
                section.body,
                if section.notes.is_empty() {
                    r#"<span class="hint">ノートはありません</span>"#.to_owned()
                } else {
                    section.notes.clone()
                }
            );
        } else {
            let _ = write!(html, r#"<div class="zoomable">{}</div>"#, section.body);
        }
        additions(&mut html, document, section);
        // How the part is read, and the other pages of the body drawn in it.
        let json = |sheet: &str| {
            document.structure.get(sheet).map(|structure| {
                serde_json::to_string(structure)
                    .unwrap_or_default()
                    .replace("</", "<\\/")
            })
        };
        if let Some(json) = json(&section.sheet) {
            let _ = write!(
                html,
                r#"<script type="application/json" class="structure">{json}</script>"#
            );
        }
        for part in &section.parts {
            if let Some(json) = json(&part.sheet) {
                let _ = write!(
                    html,
                    r#"<script type="application/json" class="structure-part" data-prefix="{}" data-label="{}">{json}</script>"#,
                    escape(&part.prefix),
                    part.label
                );
            }
        }
        html.push_str("</section>");
    }
    html.push_str("</main>");
    for section in sections.iter().filter(|s| !s.aside.is_empty()) {
        html.push_str(&section.aside);
    }
    html.push_str(READING);
    if !out.differences.is_empty() {
        html.push_str(r#"<aside id="diffs" class="diffs"><div class="diffs-head"><b>原本との差異</b><label><input type="checkbox" id="marks" checked> 枠を表示</label></div><ol>"#);
        for difference in &out.differences {
            let _ = write!(
                html,
                r#"<li><button data-go="{}" data-target="{}"><span class="place">{}</span><span class="value">{}</span><span class="note">{}</span></button></li>"#,
                difference.page,
                difference.anchor,
                escape(&difference.place),
                escape(&difference.value),
                escape(&difference.note)
            );
        }
        html.push_str("</ol></aside>");
    }
    html.push_str(r#"</div><footer class="status">"#);
    match layout {
        Layout::Sheets => {
            html.push_str(r#"<div class="tabs">"#);
            let marked: std::collections::BTreeSet<usize> =
                out.differences.iter().map(|d| d.page).collect();
            for (index, section) in sections.iter().enumerate() {
                // A hidden sheet is left out, as Excel leaves it, unless the
                // content changes it.
                if section.hidden && !marked.contains(&index) {
                    continue;
                }
                let _ = write!(
                    html,
                    r#"<button class="tab{}" data-go="{index}">{}</button>"#,
                    if section.hidden { " off" } else { "" },
                    escape(&section.title)
                );
            }
            html.push_str("</div>");
        }
        Layout::Slides => html.push_str(
            r#"<span id="where"></span><button id="notes-toggle" class="flat" aria-pressed="true">ノート</button>"#,
        ),
        Layout::Pages => html.push_str(r#"<span id="where"></span>"#),
        Layout::Paper | Layout::Records => {}
    }
    let _ = write!(
        html,
        r#"<span class="grow"></span><div class="zoom"><button class="flat" data-zoom="-1" title="縮小">−</button><button id="zoom-label" class="flat" title="100% に戻す">100%</button><button class="flat" data-zoom="1" title="拡大">＋</button></div></footer><script>{SCRIPT}</script></body></html>"#
    );
    html
}

/// The values of the rows and columns operations insert on the sheet of
/// `section`, listed under it.
fn additions(html: &mut String, document: &Document<'_>, section: &Section) {
    let additions: Vec<&Addition> = document
        .content
        .additions
        .iter()
        .filter(|a| a.sheet == section.sheet)
        .collect();
    if additions.is_empty() {
        return;
    }
    html.push_str(r#"<table class="records additions"><caption>未適用の操作が加える値</caption><tr><th>操作</th><th>項目</th><th>値</th></tr>"#);
    for addition in additions {
        let _ = write!(
            html,
            "<tr><td>{}</td><td>{}</td><td>{}</td></tr>",
            escape(&addition.operation),
            escape(&addition.field),
            escape(&plain(&addition.value))
        );
    }
    html.push_str("</table>");
}

/// The panel of the structure view: what the colours stand for, how the
/// chosen cell is read, and the elements of the shown part.
const READING: &str = r#"<aside id="reading" class="diffs reading" hidden><div class="diffs-head"><b>構造の読み取り</b></div><div class="legend"><span class="chip role-column_header">列見出し</span><span class="chip role-row_header">行見出し</span><span class="chip role-data">値</span><span class="chip role-text">本文</span><span class="chip role-category">区分</span><span class="chip role-mark">記号</span><span class="chip role-note">注記</span><span class="chip role-unassigned">未分類</span></div><div id="explain" class="explain"><p class="hint">セルや段落を選ぶと、AIが本文YAMLでどう読むか（キー・見出し・階層）を示します。</p></div><div class="outline-head">要素</div><div id="outline" class="outline"></div></aside>"#;

const STYLE: &str = r#"
:root{--mark:#e8590c;--ui:#f3f2f1;--ui-line:#d2d0ce;--ink:#252423;--muted:#605e5c;--canvas:#e6e6e6}
*{box-sizing:border-box}
html,body{height:100%}
body{margin:0;display:flex;flex-direction:column;color:var(--ink);font:13px/1.4 'Segoe UI','Yu Gothic UI',Meiryo,system-ui,sans-serif;background:var(--canvas)}
button{font:inherit;color:inherit}
.bar{display:flex;align-items:center;gap:10px;height:40px;padding:0 14px;background:var(--accent);color:#fff;flex:none}
.bar .name{font-weight:600;font-size:14px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
.badge{font-size:11px;padding:1px 7px;border-radius:9px;background:rgba(255,255,255,.22);white-space:nowrap}
.badge.warn{background:#fff4ce;color:#5c3c00}
.grow{flex:1}
.diff-button{border:1px solid rgba(255,255,255,.6);background:transparent;color:#fff;border-radius:3px;padding:3px 10px;cursor:pointer;white-space:nowrap}
.diff-button[aria-pressed=true]{background:#fff;color:var(--accent)}
.fbar{display:flex;align-items:stretch;height:28px;background:#fff;border-bottom:1px solid var(--ui-line);flex:none;font-size:13px}
.namebox{width:96px;padding:4px 8px;border-right:1px solid var(--ui-line);font-family:Consolas,'Segoe UI',monospace}
.fx{padding:4px 10px;color:var(--muted);font-style:italic;border-right:1px solid var(--ui-line)}
.formula{flex:1;padding:4px 8px;white-space:pre;overflow:hidden;text-overflow:ellipsis}
.formula .orig{margin-left:14px;color:var(--mark);font-size:12px}
.body{flex:1;min-height:0;display:flex}
.stage{flex:1;min-width:0;overflow:auto;position:relative}
[data-layout=sheets] .stage{background:#fff}
[data-layout=paper] .stage,[data-layout=pages] .stage,[data-layout=records] .stage{padding:24px}
.view[hidden]{display:none!important}
[data-layout=pages] .view{margin:0 auto 20px;width:max-content}
[data-layout=records] .view{margin-bottom:20px}
.status{display:flex;align-items:center;gap:8px;height:30px;padding:0 10px;background:var(--ui);border-top:1px solid var(--ui-line);flex:none;font-size:12px;color:var(--muted)}
.flat{border:0;background:transparent;padding:3px 8px;border-radius:3px;cursor:pointer}
.flat:hover{background:#e1dfdd}
.flat[aria-pressed=true]{color:var(--ink);font-weight:600}
.zoom{display:flex;align-items:center}
#zoom-label{min-width:52px}
.tabs{display:flex;overflow-x:auto;align-self:stretch;max-width:75%}
.tab{border:0;border-right:1px solid var(--ui-line);background:transparent;padding:0 16px;cursor:pointer;white-space:nowrap;color:var(--ink)}
.tab:hover{background:#e1dfdd}
.tab.on{background:#fff;color:var(--accent);font-weight:600;box-shadow:inset 0 -3px 0 var(--accent)}
.tab.off{font-style:italic;color:var(--muted)}
.diffs{width:300px;flex:none;background:#fff;border-left:1px solid var(--ui-line);display:flex;flex-direction:column}
.diffs[hidden]{display:none}
.diffs-head{display:flex;justify-content:space-between;align-items:center;padding:10px 12px;border-bottom:1px solid var(--ui-line)}
.diffs-head label{font-size:12px;color:var(--muted)}
.diffs ol{list-style:none;margin:0;padding:0;overflow:auto}
.diffs li button{display:block;width:100%;text-align:left;border:0;border-bottom:1px solid #edebe9;background:transparent;padding:8px 12px;cursor:pointer}
.diffs li button:hover{background:#f3f2f1}
.diffs .place{display:block;font-size:11px;color:var(--muted)}
.diffs .value{display:block;white-space:pre-wrap;overflow-wrap:anywhere}
.diffs .note{display:block;font-size:12px;color:var(--mark);white-space:pre-wrap;overflow-wrap:anywhere}
.comments{width:280px;flex:none;background:#faf9f8;border-left:1px solid var(--ui-line);overflow:auto;padding:12px}
.comments h3{font-size:12px;color:var(--muted);margin:0 0 8px;font-weight:600}
.comment{background:#fff;border:1px solid var(--ui-line);border-radius:4px;padding:8px 10px;margin-bottom:8px;white-space:pre-wrap;position:relative}
.rail{width:200px;flex:none;overflow:auto;background:var(--ui);border-right:1px solid var(--ui-line);padding:10px 8px}
.thumb{display:flex;gap:6px;width:100%;border:0;background:transparent;padding:4px;margin-bottom:6px;cursor:pointer;text-align:left}
.thumb .n{width:18px;text-align:right;color:var(--muted);font-size:12px}
.thumb .mini{position:relative;display:block;overflow:hidden;background:#fff;border:1px solid var(--ui-line);pointer-events:none}
.thumb.on .mini{outline:2px solid var(--accent);outline-offset:1px}
.thumb.off .mini{opacity:.45}
.thumb.off .n{text-decoration:line-through}
[data-layout=slides] .view{display:flex;flex-direction:column;height:100%}
.fit{flex:1;min-height:0;overflow:auto;display:flex;padding:24px}
.slide-box{position:relative;margin:auto;flex:none}
.slide-box>.slide{position:absolute;left:0;top:0;transform-origin:0 0}
.notes-pane{flex:none;height:110px;overflow:auto;background:#fff;border-top:1px solid var(--ui-line);padding:8px 14px;white-space:pre-wrap;font-size:13px}
.notes-pane .hint{color:var(--muted)}
body.no-notes .notes-pane{display:none}
[data-changed]{outline:2px solid var(--mark)!important;outline-offset:-2px;z-index:3}
[data-changed]::after{content:'';position:absolute;right:0;top:0;border-style:solid;border-width:0 7px 7px 0;border-color:transparent var(--mark) transparent transparent}
.flash{animation:flash 1.2s}
@keyframes flash{0%,60%{box-shadow:0 0 0 4px rgba(232,89,12,.45)}100%{box-shadow:none}}
body.plain [data-changed]{outline:none!important}
body.plain [data-changed]::after,body.plain .mark{display:none}
.grid{position:relative;background:#fff;font-family:'Yu Gothic','游ゴシック',Calibri,sans-serif;font-size:11pt;cursor:cell}
.grid .heads-x{position:sticky;top:0;z-index:6;height:20px}
.grid .heads-y{position:sticky;left:0;z-index:5;width:40px}
.grid .corner{position:sticky;left:0;top:0;z-index:7;width:40px;height:20px;background:#f3f3f3;border:solid #d0d0d0;border-width:0 1px 1px 0}
.grid .hd{position:absolute;background:#f3f3f3;border:solid #d0d0d0;border-width:0 1px 1px 0;color:#666;font:11px/1 'Segoe UI',system-ui,sans-serif;display:flex;align-items:center;justify-content:center;overflow:hidden}
.grid .hd.on{background:#d2d2d2;color:#217346;font-weight:600;box-shadow:inset 0 -2px 0 #217346}
.grid .heads-y .hd.on{box-shadow:inset -2px 0 0 #217346}
.grid .f{position:absolute}
.grid .c{position:absolute;display:flex;padding:0 3px;line-height:1.2;white-space:pre;overflow:hidden}
.grid .c.w{white-space:pre-wrap;overflow-wrap:anywhere}
.grid .sel{position:absolute;border:2px solid #217346;z-index:4;pointer-events:none;scroll-margin:28px 8px 8px 48px}
svg.lines{position:absolute;left:0;top:0;pointer-events:none;overflow:visible}
.shape{position:absolute;cursor:default}
.shape>svg{position:absolute;inset:0;width:100%;height:100%;overflow:visible}
.shape>.t{position:absolute;inset:0;display:flex;flex-direction:column;padding:3px 6px;white-space:pre-wrap;overflow-wrap:anywhere;line-height:1.2;overflow:hidden}
.shape img{position:absolute;inset:0;width:100%;height:100%;object-fit:fill}
.shape.ph{border:1px dashed #adb5bd;background:repeating-linear-gradient(45deg,#f8f9fa,#f8f9fa 6px,#eef0f2 6px,#eef0f2 12px);color:var(--muted);font-size:11px;display:flex;align-items:center;justify-content:center;text-align:center}
.slide{position:relative;background:#fff;overflow:hidden;box-shadow:0 1px 4px rgba(0,0,0,.3);font-family:'Yu Gothic','游ゴシック',sans-serif}
.slide table,.paper table{border-collapse:collapse;table-layout:fixed}
.slide td{padding:2px 5px;vertical-align:top;overflow:hidden;overflow-wrap:anywhere;position:relative}
.paper{position:relative;background:#fff;margin:0 auto;box-shadow:0 1px 4px rgba(0,0,0,.25);font-family:'Yu Mincho','游明朝','Century',serif;font-size:10.5pt}
.paper p{margin:0 0 .4em;white-space:pre-wrap;position:relative;min-height:1em}
.paper td{border:1px solid transparent;padding:2px 5px;vertical-align:top;white-space:pre-wrap;overflow-wrap:anywhere;position:relative}
.paper .band{color:var(--muted);font-size:9pt;padding:4px 0}
.paper .band.top{border-bottom:1px dashed var(--ui-line);margin-bottom:12px}
.paper .band.bottom{border-top:1px dashed var(--ui-line);margin-top:16px}
.paper .notes{border-top:1px solid #000;width:40%;margin-top:18px;padding-top:4px;font-size:9pt}
.paper figure{margin:8px 0}.paper figure img{max-width:100%}
.pdf{position:relative;background:#fff;box-shadow:0 1px 4px rgba(0,0,0,.3);overflow:hidden}
table.records{border-collapse:collapse;background:#fff}
table.records caption{text-align:left;color:var(--muted);font-size:12px;padding:6px 0}
table.records th,table.records td{border:1px solid var(--ui-line);padding:2px 6px;vertical-align:top;white-space:pre-wrap;position:relative}
table.records th{background:var(--ui);color:var(--muted);font-weight:normal;font-size:11px}
.additions{margin:16px}
:root{--r-column_header:rgba(47,128,237,.32);--r-row_header:rgba(39,174,96,.32);--r-data:rgba(242,153,74,.18);--r-text:rgba(120,120,120,.2);--r-note:rgba(155,81,224,.28);--r-unassigned:rgba(235,87,87,.32);--r-mark:rgba(140,90,40,.32);--r-category:rgba(0,150,160,.32)}
.modes{display:flex;border:1px solid rgba(255,255,255,.6);border-radius:3px;overflow:hidden;flex:none}
.modes button{border:0;background:transparent;color:#fff;padding:3px 12px;cursor:pointer}
.modes button[aria-pressed=true]{background:#fff;color:var(--accent);font-weight:600}
.reading{width:330px}
.legend{display:flex;flex-wrap:wrap;gap:4px;padding:8px 12px;border-bottom:1px solid var(--ui-line)}
.chip{display:inline-block;padding:1px 8px;border-radius:9px;font-size:11px;background:#edebe9;color:var(--ink)}
.chip.addr{font-family:Consolas,monospace;background:#fff;border:1px solid var(--ui-line)}
.chip.head{cursor:pointer;border:1px dashed #1565c0;background:rgba(21,101,192,.08);margin:0 4px 4px 0}
.role-column_header{background:var(--r-column_header)}.role-row_header{background:var(--r-row_header)}.role-data{background:var(--r-data)}.role-text{background:var(--r-text)}.role-note{background:var(--r-note)}.role-unassigned{background:var(--r-unassigned)}.role-mark{background:var(--r-mark)}.role-category{background:var(--r-category)}
.explain{overflow:auto;flex:none;max-height:55%}
.explain .hint{color:var(--muted);padding:12px;margin:0}
.ex-head{display:flex;gap:6px;padding:12px 12px 0}
.explain dl{display:grid;grid-template-columns:52px 1fr;gap:6px 8px;margin:0;padding:10px 12px 12px}
.explain dt{color:var(--muted);font-size:12px}
.explain dd{margin:0;white-space:pre-wrap;overflow-wrap:anywhere}
ol.path{margin:0;padding-left:18px}
ol.path li{padding:1px 0}
ol.path li:last-child{font-weight:600}
.outline-head{padding:8px 12px;border-top:1px solid var(--ui-line);border-bottom:1px solid var(--ui-line);font-weight:600;background:var(--ui)}
.outline{overflow:auto;flex:1}
.outline .item{display:block;width:100%;text-align:left;border:0;border-bottom:1px solid #edebe9;background:transparent;padding:8px 12px;cursor:pointer}
.outline .item:hover{background:#f3f2f1}
.outline .diagram ul{margin:4px 0 0;padding-left:18px;font-size:12px}
.overlay{position:absolute;left:0;top:0;width:100%;height:100%;pointer-events:none;z-index:2;display:none}
body.structure .overlay{display:block}
body.structure .grid .f{display:none}
.ov,.ov-table,.ov-head{position:absolute}
.ov.unshown{background-image:repeating-linear-gradient(45deg,transparent 0 4px,rgba(0,0,0,.14) 4px 6px)}
.ov-table{outline:2px solid #1f4e79;outline-offset:-1px}
.ov-table .tag,.read-tag{background:#1f4e79;color:#fff;font:11px/16px 'Segoe UI','Yu Gothic UI',sans-serif;padding:0 6px;white-space:nowrap;border-radius:2px 2px 0 0}
.ov-table .tag{position:absolute;left:-2px;top:-17px}
.read-tag{display:none;width:max-content;margin-top:6px}
body.structure .read-tag{display:block}
.ov-head{outline:2px dashed #1565c0;outline-offset:-2px;background:rgba(21,101,192,.14)}
body.structure [data-changed]{outline:none!important}
body.structure [data-changed]::after{display:none}
body.structure [data-role=column_header]{background-color:var(--r-column_header)!important}
body.structure [data-role=row_header]{background-color:var(--r-row_header)!important}
body.structure [data-role=data]{background-color:var(--r-data)!important}
body.structure [data-role=text]{background-color:var(--r-text)!important}
body.structure [data-role=note]{background-color:var(--r-note)!important}
body.structure [data-role=unassigned]{background-color:var(--r-unassigned)!important}
body.structure [data-role=mark]{background-color:var(--r-mark)!important}
body.structure [data-role=category]{background-color:var(--r-category)!important}
body.structure text[data-role=column_header]{fill:#1565c0}
body.structure text[data-role=row_header]{fill:#1e8e3e}
body.structure text[data-role=data]{fill:#b35c00}
body.structure text[data-role=unassigned]{fill:#c62828}
body.structure .read-table{outline:2px solid #1f4e79}
body.structure .is-head{outline:2px dashed #1565c0!important;outline-offset:-2px}
body.structure .picked{outline:2px solid #217346!important;outline-offset:-2px}
body.structure [data-a]{cursor:pointer}
body.structure .shape.in-diagram{outline:2px solid #7b1fa2;outline-offset:2px}
body.structure .shape.lit{outline-width:4px;background:rgba(123,31,162,.12)}
"#;

const SCRIPT: &str = r#"
(function () {
  var body = document.body, layout = body.dataset.layout;
  var stage = document.getElementById('stage');
  var views = Array.prototype.slice.call(document.querySelectorAll('.view'));
  var one = layout === 'sheets' || layout === 'slides';
  var steps = [0.5, 0.67, 0.75, 0.8, 0.9, 1, 1.1, 1.25, 1.5, 1.75, 2, 2.5];
  var step = steps.indexOf(1), current = 0;
  function zoom() { return steps[step]; }
  function all(selector, root) { return Array.prototype.slice.call((root || document).querySelectorAll(selector)); }

  // Excel: where the columns and rows of each sheet start, and its merges.
  var geometry = views.map(function (v) {
    var data = v.querySelector('script.geo');
    return data ? JSON.parse(data.textContent) : null;
  });
  var selected = [];
  function columnName(c) {
    var s = '';
    while (c > 0) { var m = (c - 1) % 26; s = String.fromCharCode(65 + m) + s; c = Math.floor((c - 1) / 26); }
    return s;
  }
  function place(address) {
    var m = /^([A-Z]+)(\d+)$/.exec(address || '');
    if (!m) return null;
    var c = 0;
    for (var i = 0; i < m[1].length; i++) c = c * 26 + m[1].charCodeAt(i) - 64;
    return [c, +m[2]];
  }
  function line(edges, at) {
    if (at < edges[1]) return 0;
    var lo = 1, hi = edges.length - 2;
    while (lo < hi) { var mid = (lo + hi + 1) >> 1; if (edges[mid] <= at) lo = mid; else hi = mid - 1; }
    return lo;
  }
  function area(g, c, r) {
    for (var i = 0; i < g.m.length; i++) {
      var m = g.m[i];
      if (c >= m[0] && c <= m[2] && r >= m[1] && r <= m[3]) return m;
    }
    return [c, r, c, r];
  }
  function select(index, at) {
    var view = views[index], g = geometry[index];
    if (!g || !at) return;
    var a = area(g, at[0], at[1]);
    selected[index] = [a[0], a[1]];
    var box = view.querySelector('.sel');
    box.style.left = g.x[a[0]] + 'px';
    box.style.top = g.y[a[1]] + 'px';
    box.style.width = (g.x[a[2] + 1] - g.x[a[0]]) + 'px';
    box.style.height = (g.y[a[3] + 1] - g.y[a[1]]) + 'px';
    box.hidden = false;
    var address = columnName(a[0]) + a[1];
    document.getElementById('namebox').textContent = address;
    var formula = document.getElementById('formula');
    formula.textContent = '';
    var cell = view.querySelector('.c[data-a="' + address + '"]');
    if (cell) {
      formula.appendChild(document.createTextNode(cell.dataset.r !== undefined ? cell.dataset.r : cell.textContent));
      if (cell.dataset.changed) {
        var original = document.createElement('span');
        original.className = 'orig';
        original.textContent = cell.title;
        formula.appendChild(original);
      }
    }
    if (mode === 'structure') explain(index, address);
    all('.hd.on', view).forEach(function (h) { h.classList.remove('on'); });
    for (var c = a[0]; c <= a[2]; c++) { var hc = view.querySelector('.hd[data-c="' + c + '"]'); if (hc) hc.classList.add('on'); }
    for (var r = a[1]; r <= a[3]; r++) { var hr = view.querySelector('.hd[data-r="' + r + '"]'); if (hr) hr.classList.add('on'); }
  }
  views.forEach(function (view, index) {
    var grid = view.querySelector('.grid');
    if (!grid || !geometry[index]) return;
    grid.addEventListener('mousedown', function (e) {
      if (e.target.closest('.shape') || e.target.closest('.hd') || e.target.closest('.corner')) return;
      var rect = grid.getBoundingClientRect(), z = zoom(), g = geometry[index];
      var c = line(g.x, (e.clientX - rect.left) / z), r = line(g.y, (e.clientY - rect.top) / z);
      if (c > 0 && r > 0) select(index, [c, r]);
    });
  });

  // PowerPoint: each slide fitted to the stage, and its thumbnail.
  function slideSize(slide) { return [parseFloat(slide.style.width), parseFloat(slide.style.height)]; }
  function fit() {
    var view = views[current];
    if (!view) return;
    var slide = view.querySelector('.slide'), holder = view.querySelector('.fit');
    if (!slide) return;
    var size = slideSize(slide);
    var s = Math.max(0.05, Math.min((holder.clientWidth - 48) / size[0], (holder.clientHeight - 48) / size[1])) * zoom();
    slide.style.transform = 'scale(' + s + ')';
    var box = view.querySelector('.slide-box');
    box.style.width = size[0] * s + 'px';
    box.style.height = size[1] * s + 'px';
  }
  all('.thumb').forEach(function (thumb) {
    var view = views[+thumb.dataset.go], slide = view && view.querySelector('.slide');
    if (!slide) return;
    var size = slideSize(slide), s = 160 / size[0];
    var mini = thumb.querySelector('.mini');
    mini.style.width = '160px';
    mini.style.height = size[1] * s + 'px';
    var copy = slide.cloneNode(true);
    all('[id]', copy).forEach(function (n) { n.removeAttribute('id'); });
    copy.style.position = 'absolute';
    copy.style.transformOrigin = '0 0';
    copy.style.transform = 'scale(' + s + ')';
    copy.style.boxShadow = 'none';
    mini.appendChild(copy);
  });
  window.addEventListener('resize', function () { if (layout === 'slides') fit(); });

  function where() {
    var label = document.getElementById('where');
    if (!label) return;
    if (layout === 'pages') {
      var top = stage.getBoundingClientRect().top + 40, found = views.length - 1;
      for (var i = 0; i < views.length; i++) if (views[i].getBoundingClientRect().bottom > top) { found = i; break; }
      current = found;
    }
    label.textContent = (layout === 'slides' ? 'スライド ' : 'ページ ') + (current + 1) + ' / ' + views.length;
  }
  stage.addEventListener('scroll', function () { if (layout === 'pages') where(); });

  function show(index) {
    if (index < 0 || index >= views.length) return;
    current = index;
    if (one) views.forEach(function (v, j) { v.hidden = j !== index; });
    if (mode === 'structure') { paint(index); outline(index); }
    all('.tab, .thumb').forEach(function (b) { b.classList.toggle('on', +b.dataset.go === index); });
    var on = document.querySelector('.tab.on, .thumb.on');
    if (on) on.scrollIntoView({ block: 'nearest', inline: 'nearest' });
    if (layout === 'slides') fit();
    if (layout === 'sheets') select(index, selected[index] || [1, 1]);
    if (layout === 'pages') views[index].scrollIntoView({ block: 'start' });
    where();
  }
  all('.tab, .thumb').forEach(function (b) { b.addEventListener('click', function () { show(+b.dataset.go); }); });

  function apply() {
    document.getElementById('zoom-label').textContent = Math.round(zoom() * 100) + '%';
    if (layout === 'slides') { fit(); return; }
    all('.zoomable').forEach(function (z) { z.style.zoom = zoom(); });
  }
  all('[data-zoom]').forEach(function (b) {
    b.addEventListener('click', function () {
      step = Math.max(0, Math.min(steps.length - 1, step + (+b.dataset.zoom)));
      apply();
    });
  });
  document.getElementById('zoom-label').addEventListener('click', function () { step = steps.indexOf(1); apply(); });

  var notes = document.getElementById('notes-toggle');
  if (notes) notes.addEventListener('click', function () {
    var hide = !body.classList.toggle('no-notes');
    notes.setAttribute('aria-pressed', String(hide));
    fit();
  });

  var diffs = document.getElementById('diffs'), toggle = document.getElementById('diff-toggle');
  if (toggle) toggle.addEventListener('click', function () {
    if (mode === 'structure') { toggle.setAttribute('aria-pressed', 'true'); setMode('view'); return; }
    diffs.hidden = !diffs.hidden;
    toggle.setAttribute('aria-pressed', String(!diffs.hidden));
    if (layout === 'slides') fit();
  });
  all('#diffs [data-target]').forEach(function (b) {
    b.addEventListener('click', function () {
      show(+b.dataset.go);
      var target = document.getElementById(b.dataset.target);
      if (!target) return;
      if (layout === 'sheets' && target.dataset.a) select(current, place(target.dataset.a));
      target.scrollIntoView({ block: 'center', inline: 'center' });
      target.classList.add('flash');
      setTimeout(function () { target.classList.remove('flash'); }, 1200);
    });
  });
  var marks = document.getElementById('marks');
  if (marks) marks.addEventListener('change', function () { body.classList.toggle('plain', !marks.checked); });

  document.addEventListener('keydown', function (e) {
    if (e.target.closest && e.target.closest('input, textarea')) return;
    if (layout === 'sheets') {
      var move = { ArrowLeft: [-1, 0], ArrowRight: [1, 0], ArrowUp: [0, -1], ArrowDown: [0, 1], Enter: [0, 1], Tab: [1, 0] }[e.key];
      var g = geometry[current];
      if (!move || !g) return;
      e.preventDefault();
      var at = selected[current] || [1, 1], a = area(g, at[0], at[1]);
      var c = move[0] > 0 ? a[2] : a[0], r = move[1] > 0 ? a[3] : a[1];
      do { c += move[0]; r += move[1]; }
      while (c >= 1 && r >= 1 && c < g.x.length - 1 && r < g.y.length - 1 && (g.x[c + 1] - g.x[c] <= 0 || g.y[r + 1] - g.y[r] <= 0));
      if (c < 1 || r < 1 || c >= g.x.length - 1 || r >= g.y.length - 1) return;
      select(current, [c, r]);
      views[current].querySelector('.sel').scrollIntoView({ block: 'nearest', inline: 'nearest' });
    } else if (layout === 'slides') {
      var step = { ArrowDown: 1, ArrowRight: 1, PageDown: 1, ArrowUp: -1, ArrowLeft: -1, PageUp: -1 }[e.key];
      if (step) { e.preventDefault(); show(current + step); }
    }
  });

  // Structure: how a reader of the body takes each element of a part.
  var mode = 'view';
  function json(view, selector) {
    var data = view.querySelector(selector);
    return data ? JSON.parse(data.textContent) : null;
  }
  var readings = views.map(function (v) { return json(v, 'script.structure'); });
  // Other pages of the body drawn in a part, as a slide's notes, place their
  // cells after a prefix of their own.
  var labels = {};
  var lookup = views.map(function (v, index) {
    var at = {};
    function add(r, prefix) {
      if (!r) return;
      r.tables.forEach(function (t) { t.cells.forEach(function (c) { at[prefix + c.address] = { table: t, cell: c }; }); });
      r.texts.forEach(function (a) { if (!at[prefix + a]) at[prefix + a] = { text: true }; });
    }
    add(readings[index], '');
    all('script.structure-part', v).forEach(function (part) {
      labels[part.dataset.prefix] = part.dataset.label;
      add(JSON.parse(part.textContent), part.dataset.prefix);
    });
    return at;
  });
  function placeName(address) {
    var cut = address.lastIndexOf(':');
    return cut < 0 ? address : (labels[address.slice(0, cut + 1)] || address.slice(0, cut)) + ' ' + address.slice(cut + 1);
  }
  var ROLES = { column_header: '列見出し', row_header: '行見出し', data: '値', text: '本文', note: '注記', unassigned: '未分類', mark: '記号', category: '区分' };
  var painted = [];
  function roleOf(info) {
    if (!info.cell) return 'text';
    return info.cell.mark ? 'mark' : info.cell.category ? 'category' : info.cell.role;
  }
  function textAt(index, address) {
    var el = views[index].querySelector('[data-a="' + address + '"]');
    return el ? el.textContent : '';
  }
  function rectOf(index, address) {
    var g = geometry[index], p = place(address);
    if (!g || !p) return null;
    var a = area(g, p[0], p[1]);
    return [g.x[a[0]], g.y[a[1]], g.x[a[2] + 1] - g.x[a[0]], g.y[a[3] + 1] - g.y[a[1]], a[0] + ',' + a[1]];
  }
  function box(layer, r, className, label) {
    var d = document.createElement('div');
    d.className = className;
    d.style.left = r[0] + 'px'; d.style.top = r[1] + 'px'; d.style.width = r[2] + 'px'; d.style.height = r[3] + 'px';
    if (label) { var t = document.createElement('span'); t.className = 'tag'; t.textContent = label; d.appendChild(t); }
    layer.appendChild(d);
  }
  function tableName(t) { return '表' + t.number + (t.title ? ' ' + t.title : ''); }
  function paint(index) {
    if (painted[index]) return;
    painted[index] = true;
    var view = views[index], r = readings[index] || { tables: [], texts: [], diagrams: [], checkboxes: [] };
    var grid = view.querySelector('.grid');
    if (grid && geometry[index]) {
      var layer = document.createElement('div');
      layer.className = 'overlay';
      grid.insertBefore(layer, grid.querySelector('.sel'));
      var seen = {};
      Object.keys(lookup[index]).forEach(function (address) {
        var info = lookup[index][address], rect = rectOf(index, address);
        if (!rect || seen[rect[4]]) return;
        seen[rect[4]] = true;
        box(layer, rect, 'ov role-' + roleOf(info) + (info.cell && !info.cell.shown ? ' unshown' : ''));
      });
      r.tables.forEach(function (t) {
        var x0 = Infinity, y0 = Infinity, x1 = 0, y1 = 0;
        t.cells.forEach(function (c) {
          var q = rectOf(index, c.address);
          if (!q) return;
          x0 = Math.min(x0, q[0]); y0 = Math.min(y0, q[1]); x1 = Math.max(x1, q[0] + q[2]); y1 = Math.max(y1, q[1] + q[3]);
        });
        if (x1 > x0) box(layer, [x0, y0, x1 - x0, y1 - y0], 'ov-table', tableName(t));
      });
      var heads = document.createElement('div');
      heads.className = 'heads-of';
      layer.appendChild(heads);
    } else {
      Object.keys(lookup[index]).forEach(function (address) {
        var info = lookup[index][address];
        all('[data-a="' + address + '"]', view).forEach(function (el) {
          el.dataset.role = roleOf(info);
        });
      });
      r.tables.forEach(function (t) {
        var first = t.cells.length && view.querySelector('[data-a="' + t.cells[0].address + '"]');
        var table = first && first.closest('table');
        if (!table || table.classList.contains('read-table')) return;
        table.classList.add('read-table');
        var tag = document.createElement('div');
        tag.className = 'read-tag';
        tag.textContent = tableName(t);
        // A table a slide places stands where its frame is, its name above it.
        if (table.style.position === 'absolute') {
          tag.style.position = 'absolute';
          tag.style.left = table.style.left;
          tag.style.top = (parseFloat(table.style.top) - 16) + 'px';
          tag.style.margin = '0';
        }
        table.parentNode.insertBefore(tag, table);
      });
    }
    r.diagrams.forEach(function (d) {
      d.shapes.forEach(function (s) {
        all('.shape[data-shape="' + s[1] + '"]', view).forEach(function (el) {
          el.classList.add('in-diagram');
          el.dataset.diagram = String(d.number);
        });
      });
    });
  }
  function headsOf(index, list) {
    var view = views[index], layer = view.querySelector('.heads-of');
    if (layer) {
      layer.textContent = '';
      list.forEach(function (a) { var q = rectOf(index, a); if (q) box(layer, q, 'ov-head'); });
      return;
    }
    all('.is-head', view).forEach(function (el) { el.classList.remove('is-head'); });
    list.forEach(function (a) { all('[data-a="' + a + '"]', view).forEach(function (el) { el.classList.add('is-head'); }); });
  }
  var explainer = document.getElementById('explain');
  function chip(text, className) {
    var c = document.createElement('span');
    c.className = 'chip ' + (className || '');
    c.textContent = text;
    return c;
  }
  function term(list, name, content) {
    var dt = document.createElement('dt'), dd = document.createElement('dd');
    dt.textContent = name;
    if (typeof content === 'string') dd.textContent = content; else dd.appendChild(content);
    list.appendChild(dt);
    list.appendChild(dd);
  }
  function explain(index, address) {
    explainer.textContent = '';
    var info = lookup[index][address];
    var head = document.createElement('div');
    head.className = 'ex-head';
    head.appendChild(chip(placeName(address), 'addr'));
    head.appendChild(info ? chip(ROLES[roleOf(info)] || roleOf(info), 'role-' + roleOf(info)) : chip('対象外'));
    explainer.appendChild(head);
    var list = document.createElement('dl'), value = textAt(index, address);
    explainer.appendChild(list);
    if (!info) {
      term(list, '読み方', '表・本文のどれにも属さないため、本文YAMLには現れません');
      headsOf(index, []);
      return;
    }
    if (info.text) {
      term(list, '要素', '本文（text）');
      term(list, '値', value || '（空）');
      headsOf(index, []);
      return;
    }
    var t = info.table, c = info.cell;
    term(list, '要素', '表' + t.number + (t.title ? '「' + t.title + '」' : '') + (t.element === null ? '' : ' ・ 本文YAML elements[' + t.element + ']'));
    term(list, '読み方',
      !c.shown ? '本文YAMLには現れません（区分の下の空欄など）'
        : c.heading ? '列の見出し（columns）。この文字が下の値のキーになります'
        : c.description === 'title' ? '表題（title）。表の見出しの前に置きます'
        : c.description === 'note' ? '注記（notes）。表の見出しの前に置きます'
        : c.mark ? '記号（marks）。キーを持ちません'
        : c.category ? '区分の節の値。キー「' + c.key + '」'
        : 'キー「' + c.key + '」の値');
    term(list, '値', value || '（空）');
    if (c.headers.length) {
      var heads = document.createElement('div');
      c.headers.forEach(function (a) {
        var h = chip((textAt(index, a) || '（空）') + '  ' + a, 'head');
        h.title = a + ' を選ぶ';
        h.addEventListener('click', function () { pick(index, a); });
        heads.appendChild(h);
      });
      term(list, '見出し', heads);
    }
    if (c.nodes.length) {
      var path = document.createElement('ol');
      path.className = 'path';
      ['rows'].concat(c.nodes).forEach(function (n) { var li = document.createElement('li'); li.textContent = n; path.appendChild(li); });
      term(list, '位置', path);
    }
    headsOf(index, c.headers);
  }
  function pick(index, address) {
    if (layout === 'sheets') {
      select(index, place(address));
      views[index].querySelector('.sel').scrollIntoView({ block: 'nearest', inline: 'nearest' });
      return;
    }
    var el = views[index].querySelector('[data-a="' + address + '"]');
    all('.picked').forEach(function (p) { p.classList.remove('picked'); });
    if (el) { el.classList.add('picked'); el.scrollIntoView({ block: 'center', inline: 'nearest' }); }
    explain(index, address);
  }
  function outline(index) {
    var list = document.getElementById('outline');
    list.textContent = '';
    var r = readings[index];
    if (!r || !(r.tables.length || r.texts.length || r.diagrams.length || r.checkboxes.length)) {
      list.appendChild(chip('このページに要素はありません'));
      return;
    }
    function item(text, go) {
      var b = document.createElement('button');
      b.className = 'item';
      b.textContent = text;
      b.addEventListener('click', go);
      list.appendChild(b);
      return b;
    }
    r.tables.forEach(function (t) {
      item(tableName(t) + ' — ' + t.cells.length + ' セル、' + (t.headings ? '見出し ' + t.headings + ' 行' : '見出しなし') + (t.element === null ? '' : ' ・ elements[' + t.element + ']'),
        function () { if (t.cells.length) pick(index, t.cells[0].address); });
    });
    if (r.texts.length) item('本文 — ' + r.texts.length + ' 段落', function () { pick(index, r.texts[0]); });
    r.diagrams.forEach(function (d) {
      var wrap = item('図' + d.number + ' ・ elements[' + d.element + ']', function () {});
      var lines = document.createElement('ul');
      d.lines.forEach(function (l) { var li = document.createElement('li'); li.textContent = l; lines.appendChild(li); });
      wrap.classList.add('diagram');
      wrap.appendChild(lines);
      wrap.addEventListener('mouseenter', function () { all('.shape[data-diagram="' + d.number + '"]', views[index]).forEach(function (el) { el.classList.add('lit'); }); });
      wrap.addEventListener('mouseleave', function () { all('.shape.lit', views[index]).forEach(function (el) { el.classList.remove('lit'); }); });
    });
    r.checkboxes.forEach(function (c) {
      item((c.checked === true ? '☑ ' : c.checked === false ? '☐ ' : '▣ ') + (c.text || '（ラベルなし）') + (c.linked_cell ? ' → ' + c.linked_cell : '') + ' ・ elements[' + c.element + ']', function () {});
    });
  }
  function setMode(next) {
    mode = next;
    body.classList.toggle('structure', next === 'structure');
    all('[data-mode]').forEach(function (b) { b.setAttribute('aria-pressed', String(b.dataset.mode === next)); });
    document.getElementById('reading').hidden = next !== 'structure';
    if (diffs) diffs.hidden = next === 'structure' || toggle.getAttribute('aria-pressed') !== 'true';
    if (next === 'structure') {
      paint(current);
      outline(current);
      var at = selected[current];
      if (layout === 'sheets' && at) explain(current, columnName(at[0]) + at[1]);
    }
    if (layout === 'slides') fit();
  }
  all('[data-mode]').forEach(function (b) { b.addEventListener('click', function () { setMode(b.dataset.mode); }); });
  stage.addEventListener('click', function (e) {
    if (mode !== 'structure' || layout === 'sheets') return;
    var el = e.target.closest('[data-a]');
    if (!el) return;
    all('.picked').forEach(function (p) { p.classList.remove('picked'); });
    el.classList.add('picked');
    explain(+el.closest('.view').dataset.page, el.dataset.a);
  });

  // A page wider than the stage is first shown at the width of the stage.
  if (layout === 'pages' || layout === 'paper') {
    var widest = 0;
    all('.zoomable').forEach(function (z) { widest = Math.max(widest, z.offsetWidth); });
    var room = stage.clientWidth - 48;
    while (widest * zoom() > room && step > 0) step--;
  }
  var first = document.querySelector('.tab, .thumb');
  show(first ? +first.dataset.go : 0);
  apply();
})();
"#;
