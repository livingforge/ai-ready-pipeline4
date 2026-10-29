//! A text original, or a part of a document without a layout of its own
//! (Word comments and notes): its cells as the rows and columns of a table,
//! a line of text or a CSV record to a row.
use super::*;
use crate::excel::{column_name, coordinate};
use crate::extraction::Sheet;
use std::collections::BTreeMap as Map;

pub(super) fn sheets(document: &Document<'_>, out: &mut Out) -> Result<Vec<Section>> {
    document
        .extraction
        .sheets
        .iter()
        .enumerate()
        .map(|(index, sheet)| {
            let name = sheet["name"].as_str().unwrap_or_default();
            out.page = index;
            Ok(Section::new(name, records(document, sheet, out)?))
        })
        .collect()
}

/// The cells of `sheet` as a table, with the column letters and row
/// numbers of their addresses.
pub(super) fn records(document: &Document<'_>, sheet: &Sheet, out: &mut Out) -> Result<String> {
    let name = sheet["name"].as_str().unwrap_or_default();
    let mut rows: Map<u32, Map<u32, String>> = Map::new();
    let mut columns = 0;
    let mut place = |address: &str| -> Option<(u32, u32)> {
        let (c, r) = coordinate(address).ok()?;
        columns = columns.max(c);
        rows.entry(r).or_default().insert(c, address.to_owned());
        Some((c, r))
    };
    for cell in &sheet.cells {
        place(cell.address());
    }
    for (sheet_name, address) in document.content.cells.keys() {
        if sheet_name == name {
            place(address);
        }
    }
    if rows.is_empty() {
        return Ok(r#"<p class="notice">文字はありません。</p>"#.to_owned());
    }
    let cells: std::collections::HashMap<&str, crate::extraction::Cell<'_>> =
        sheet.cells.iter().map(|c| (c.address(), c)).collect();
    let mut html = String::from(r#"<table class="records"><tr><th></th>"#);
    for c in 1..=columns {
        let _ = write!(html, "<th>{}</th>", column_name(c)?);
    }
    html.push_str("</tr>");
    for (r, row) in &rows {
        let _ = write!(html, "<tr><th>{r}</th>");
        for c in 1..=columns {
            let Some(address) = row.get(&c) else {
                html.push_str("<td></td>");
                continue;
            };
            let cell = cells.get(address.as_str()).copied();
            let shown = document.content.cell(name, address);
            let mark = out.mark(shown, name, address);
            let _ = write!(
                html,
                r#"<td data-a="{address}" style="{}"{mark}>{}</td>"#,
                font_css(cell_font(sheet, cell, shown), cell_style(sheet, cell)),
                escape(&text_of(shown, cell))
            );
        }
        html.push_str("</tr>");
    }
    html.push_str("</table>");
    Ok(html)
}
