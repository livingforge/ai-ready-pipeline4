//! A Word document on its paper: the width and margins of its last section,
//! its header and footer, and its paragraphs and tables in order, each table
//! with its merged cells, borders and shading. The extraction does not place
//! the pictures in the text, so they follow it. Comments and notes follow as
//! tables of their own.
use super::*;
use crate::excel::coordinate;
use crate::extraction::Sheet;
use roxmltree::Document as Xml;
use std::collections::{BTreeMap as Map, HashMap};

const WORD: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";

/// The paper of a section: its width and margins, in pixels.
struct Paper {
    width: f64,
    left: f64,
    right: f64,
    top: f64,
    bottom: f64,
}

/// The paper of the document's last section (the body's `sectPr`); A4 with
/// Word's margins when it gives none.
fn paper(document: &Document<'_>) -> Paper {
    // Twips: twentieths of a point, 15 to a pixel.
    let mut paper = Paper {
        width: 11_906.0,
        left: 1_440.0,
        right: 1_440.0,
        top: 1_440.0,
        bottom: 1_440.0,
    };
    if let Some(text) = document.original.office_part("word/document.xml")
        && let Ok(xml) = Xml::parse(&text)
        && let Some(section) = xml
            .descendants()
            .rfind(|n| n.has_tag_name((WORD, "sectPr")))
    {
        let twips = |name: &str, attribute: &str| {
            section
                .children()
                .find(|n| n.has_tag_name((WORD, name)))?
                .attribute((WORD, attribute))?
                .parse::<f64>()
                .ok()
        };
        paper.width = twips("pgSz", "w").unwrap_or(paper.width);
        paper.left = twips("pgMar", "left").unwrap_or(paper.left);
        paper.right = twips("pgMar", "right").unwrap_or(paper.right);
        paper.top = twips("pgMar", "top").unwrap_or(paper.top);
        paper.bottom = twips("pgMar", "bottom").unwrap_or(paper.bottom);
    }
    Paper {
        width: paper.width / 15.0,
        left: paper.left / 15.0,
        right: paper.right / 15.0,
        top: paper.top / 15.0,
        bottom: paper.bottom / 15.0,
    }
}

pub(super) fn document(document: &Document<'_>, out: &mut Out) -> Result<Vec<Section>> {
    let sheets = &document.extraction.sheets;
    let named = |prefix: &'static str| {
        sheets
            .iter()
            .filter(move |s| s["name"].as_str().is_some_and(|n| n.starts_with(prefix)))
    };
    let paper = paper(document);
    out.page = 0;
    // The header, footer and notes are pages of their own in the body.
    let mut parts = vec![];
    let mut part = |sheet: &Sheet, label: &'static str| {
        let part = Part::of(sheet["name"].as_str().unwrap_or_default(), label);
        let prefix = part.prefix.clone();
        parts.push(part);
        prefix
    };
    let mut html = format!(
        r#"<div class="paper" style="width:{:.0}px;padding:{:.0}px {:.0}px {:.0}px {:.0}px">"#,
        paper.width, paper.top, paper.right, paper.bottom, paper.left
    );
    for band in named("header-") {
        let prefix = part(band, "ヘッダー");
        let _ = write!(
            html,
            r#"<div class="band top">{}</div>"#,
            flow(document, band, &prefix, out)?
        );
    }
    if let Some(body) = named("document").next() {
        html.push_str(&flow(document, body, "", out)?);
    }
    // Footnotes and endnotes under a short rule, as Word sets them.
    for notes in named("footnotes-").chain(named("endnotes-")) {
        let prefix = part(notes, "脚注");
        let _ = write!(
            html,
            r#"<div class="notes">{}</div>"#,
            flow(document, notes, &prefix, out)?
        );
    }
    let images: Vec<String> = document.extraction["assets"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|asset| image_url(document.assets, asset["path"].as_str()?))
        .collect();
    if !images.is_empty() {
        html.push_str(r#"<div class="band bottom">図（本文中の位置は抽出結果にないため、ここにまとめています）</div>"#);
        for url in images {
            let _ = write!(html, r#"<figure><img src="{url}" alt=""></figure>"#);
        }
    }
    for band in named("footer-") {
        let prefix = part(band, "フッター");
        let _ = write!(
            html,
            r#"<div class="band bottom">{}</div>"#,
            flow(document, band, &prefix, out)?
        );
    }
    html.push_str("</div>");
    let mut section = Section::new("document", html);
    section.parts = parts;
    // Comments beside the paper, each in a box of its own.
    let mut comments = String::new();
    for sheet in named("comments-") {
        let name = sheet["name"].as_str().unwrap_or_default();
        for cell in &sheet.cells {
            let shown = document.content.cell(name, cell.address());
            let mark = out.mark(shown, name, cell.address());
            let _ = write!(
                comments,
                r#"<div class="comment"{mark}>{}</div>"#,
                escape(&text_of(shown, Some(cell)))
            );
        }
    }
    if !comments.is_empty() {
        section.aside = format!(r#"<aside class="comments"><h3>コメント</h3>{comments}</aside>"#);
    }
    Ok(vec![section])
}

/// The paragraphs and tables of a part, in the order of its rows: a row a
/// table takes is a row of the table, any other the paragraph in column A.
/// Each is placed by its address after `prefix`.
fn flow(document: &Document<'_>, sheet: &Sheet, prefix: &str, out: &mut Out) -> Result<String> {
    let name = sheet["name"].as_str().unwrap_or_default();
    let cells: HashMap<(u32, u32), crate::extraction::Cell<'_>> = sheet
        .cells
        .iter()
        .filter_map(|cell| Some((coordinate(cell.address()).ok()?, cell)))
        .collect();
    let last = cells.keys().map(|(_, r)| *r).max().unwrap_or(0);
    // Each table by its first row: its range.
    let tables: Map<u32, (u32, u32, u32, u32)> = sheet["tables"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|table| {
            let (first, end) = table["range"].as_str()?.split_once(':')?;
            let (c0, r0) = coordinate(first).ok()?;
            let (c1, r1) = coordinate(end).ok()?;
            Some((r0, (c0, r0, c1, r1)))
        })
        .collect();
    let merges: Vec<(u32, u32, u32, u32)> = sheet["merges"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| {
            let (first, end) = m.as_str()?.split_once(':')?;
            let (c0, r0) = coordinate(first).ok()?;
            let (c1, r1) = coordinate(end).ok()?;
            Some((c0, r0, c1, r1))
        })
        .collect();
    let mut html = String::new();
    let mut row = 1;
    while row <= last {
        if let Some(&(c0, r0, c1, r1)) = tables.get(&row) {
            html.push_str(r#"<table style="width:100%">"#);
            // A cell holding nothing has no style of its own; it takes the
            // borders its table draws.
            let ruled = (r0..=r1).any(|r| {
                (c0..=c1).any(|c| {
                    cell_style(sheet, cells.get(&(c, r)).copied())["border"]
                        .as_u64()
                        .is_some_and(|b| b > 0)
                })
            });
            for r in r0..=r1 {
                html.push_str("<tr>");
                for c in c0..=c1 {
                    let merge = merges.iter().find(|(m0, n0, m1, n1)| {
                        (*m0..=*m1).contains(&c) && (*n0..=*n1).contains(&r)
                    });
                    let span = match merge {
                        Some(&(m0, n0, m1, n1)) if (m0, n0) == (c, r) => {
                            format!(r#" colspan="{}" rowspan="{}""#, m1 - m0 + 1, n1 - n0 + 1)
                        }
                        Some(_) => continue,
                        None => String::new(),
                    };
                    let address = format!("{}{r}", crate::excel::column_name(c)?);
                    let cell = cells.get(&(c, r)).copied();
                    let shown = document.content.cell(name, &address);
                    let style = cell_style(sheet, cell);
                    let mut css = font_css(cell_font(sheet, cell, shown), style);
                    if style["border"].as_u64().is_some_and(|b| b > 0) || (cell.is_none() && ruled)
                    {
                        css.push_str("border-color:#000;");
                    }
                    if style["fill"].as_u64().is_some_and(|f| f > 0) {
                        css.push_str("background:#e7e6e6;");
                    }
                    let mark = out.mark(shown, name, &address);
                    let _ = write!(
                        html,
                        r#"<td{span} data-a="{prefix}{address}" style="{css}"{mark}>{}</td>"#,
                        escape(&text_of(shown, cell))
                    );
                }
                html.push_str("</tr>");
            }
            html.push_str("</table>");
            row = r1 + 1;
            continue;
        }
        let address = format!("A{row}");
        let cell = cells.get(&(1, row)).copied();
        let shown = document.content.cell(name, &address);
        let mark = out.mark(shown, name, &address);
        let _ = write!(
            html,
            r#"<p data-a="{prefix}{address}" style="{}"{mark}>{}</p>"#,
            font_css(cell_font(sheet, cell, shown), cell_style(sheet, cell)),
            escape(&text_of(shown, cell))
        );
        row += 1;
    }
    Ok(html)
}
