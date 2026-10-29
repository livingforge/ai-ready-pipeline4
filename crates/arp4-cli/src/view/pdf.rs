//! A PDF page in its size: the lines and areas it paints, and each string of
//! its text where the page starts it, as wide as the page draws it, in its
//! font's size and colour. A string is fitted to its width on the page, so
//! that a font the viewer lacks does not move the strings after it; one the
//! content changes keeps its own width.
use super::*;
use crate::excel::coordinate;

/// Pixels of a point.
const PX: f64 = 4.0 / 3.0;

pub(super) fn pages(document: &Document<'_>, out: &mut Out) -> Result<Vec<Section>> {
    let pages = document.original.pdf_pages()?.unwrap_or_default();
    let mut sections = vec![];
    for (index, sheet) in document.extraction.sheets.iter().enumerate() {
        let name = sheet["name"].as_str().unwrap_or_default();
        out.page = index;
        let Some(page) = pages.get(index) else {
            sections.push(Section::new(
                name,
                super::text::records(document, sheet, out)?,
            ));
            continue;
        };
        let mut html = format!(
            r#"<div class="pdf" style="width:{:.0}px;height:{:.0}px"><svg width="{:.0}" height="{:.0}" viewBox="0 0 {:.2} {:.2}">"#,
            page.width * PX,
            page.height * PX,
            page.width * PX,
            page.height * PX,
            page.width,
            page.height
        );
        for path in &page.paths {
            let _ = write!(
                html,
                r#"<path d="{}" fill="{}"{} stroke="{}" stroke-width="{:.2}"/>"#,
                path.outline,
                path.fill
                    .as_deref()
                    .map_or("none".to_owned(), |c| format!("#{c}")),
                if path.even_odd {
                    r#" fill-rule="evenodd""#
                } else {
                    ""
                },
                path.stroke
                    .as_deref()
                    .map_or("none".to_owned(), |c| format!("#{c}")),
                path.width
            );
        }
        for cell in &sheet.cells {
            let Ok((_, row)) = coordinate(cell.address()) else {
                continue;
            };
            let Some([x, y, width]) = page.places.get(row as usize - 1).copied() else {
                continue;
            };
            let shown = document.content.cell(name, cell.address());
            let font = cell_font(sheet, Some(cell), shown);
            let size = font["size"].as_f64().unwrap_or(12.0);
            let text = text_of(shown, Some(cell));
            let fit = if width > 0.0 && shown.is_none_or(|s| s.original.is_none()) {
                format!(r#" textLength="{width:.2}" lengthAdjust="spacingAndGlyphs""#)
            } else {
                String::new()
            };
            let string = format!(
                r#"<text data-a="{}" x="{x:.2}" y="{y:.2}" font-size="{size:.2}" style="{}"{fit}>{}</text>"#,
                cell.address(),
                text_css(font, cell_style(sheet, Some(cell))),
                escape(&text)
            );
            let mark = out.mark(shown, name, cell.address());
            if mark.is_empty() {
                html.push_str(&string);
            } else {
                // A changed string is framed where it stands on the page.
                let note = shown
                    .and_then(|s| s.original.as_ref())
                    .map(|o| format!("原本: {}", plain(o)))
                    .unwrap_or_else(|| "フォントを変更".to_owned());
                let wide = text.chars().count() as f64 * size;
                let _ = write!(
                    html,
                    r##"<g{mark}><title>{}</title><rect class="mark" x="{:.2}" y="{:.2}" width="{:.2}" height="{:.2}" fill="none" stroke="#e8590c" stroke-width="1.2"/>{string}</g>"##,
                    escape(&note),
                    x - 1.0,
                    y - size,
                    width.max(wide.min(width * 2.0)).max(size) + 2.0,
                    size * 1.3
                );
            }
        }
        html.push_str("</svg></div>");
        sections.push(Section::new(name, html));
    }
    Ok(sections)
}

/// The CSS of a string of an SVG page: its families, its colour as the
/// fill of its letters and its emphasis. Its size is the page's.
fn text_css(font: &Value, style: &Value) -> String {
    let mut css = families(font);
    if let Some(rgb) = color(&font["color"]) {
        css.push_str(&format!("fill:{rgb};"));
    }
    for part in font_css(&Value::Null, style).split_inclusive(';') {
        css.push_str(part);
    }
    css
}

/// The CSS families of a PDF font's names, each also as a viewer names the
/// font: PDF writes `MS-PGothic` and `Arial,Bold` for what a system calls
/// `MS PGothic` and `Arial`.
fn families(font: &Value) -> String {
    let mut names: Vec<String> = vec![];
    for name in [&font["latin"], &font["east_asian"]]
        .into_iter()
        .filter_map(Value::as_str)
    {
        let base = name.split(',').next().unwrap_or(name);
        for candidate in [base.to_owned(), base.replace('-', " ")] {
            let candidate = candidate.replace(['\'', '\\', '<', '>', '"'], "");
            if !names.contains(&candidate) {
                names.push(candidate);
            }
        }
    }
    if names.is_empty() {
        return String::new();
    }
    let quoted: Vec<String> = names.iter().map(|n| format!("'{n}'")).collect();
    format!("font-family:{},sans-serif;", quoted.join(","))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pdf_font_names_are_also_given_as_systems_name_them() {
        assert_eq!(
            families(&serde_json::json!({"latin":"MS-PGothic","east_asian":"Arial,Bold"})),
            "font-family:'MS-PGothic','MS PGothic','Arial',sans-serif;"
        );
    }
}
