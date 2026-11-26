//! A PDF page in its size: the images it draws, the lines and areas it
//! paints, and each string of its text where the page starts it, as wide as
//! the page draws it, in its font's size and colour, named by the cell of
//! the page's sheet it is of. A string is fitted to its width on the page,
//! so that a font the viewer lacks does not move the strings after it; one
//! the content changes keeps its own width. A string the page draws
//! invisibly, as the OCR text over a scanned image, is kept transparent,
//! where it can still be selected.
use super::*;
use crate::document_source::pdf_cell_strings;
use std::collections::BTreeSet;

/// Pixels of a point.
const PX: f64 = 4.0 / 3.0;

pub(super) fn pages(document: &Document<'_>, out: &mut Out) -> Result<Vec<Section>> {
    let pages = document.original.pdf_pages()?.unwrap_or_default();
    // The file of each image saved on import, by its name without extension.
    let files: BTreeMap<&str, &str> = document.extraction["assets"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|asset| {
            let path = asset["path"].as_str()?;
            Some((path.split_once('.')?.0, path))
        })
        .collect();
    // An image is written once, where a page first draws it; the pages
    // drawing it again use it from there.
    let mut written = BTreeSet::new();
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
        for image in &page.images {
            let Some(file) = files.get(image.asset.as_str()) else {
                continue;
            };
            let id = format!("pdf-{}", image.asset);
            if !written.contains(&id) {
                let Some(url) = unturned_image_url(document.assets, file) else {
                    continue;
                };
                let _ = write!(
                    html,
                    r#"<defs><image id="{id}" href="{url}" width="1" height="1" preserveAspectRatio="none"/></defs>"#
                );
                written.insert(id.clone());
            }
            // Adding 0 writes a negative zero as 0.
            let [a, b, c, d, e, f] = image.matrix.map(|n| n + 0.0);
            let _ = write!(
                html,
                r##"<use href="#{id}" transform="matrix({a:.4} {b:.4} {c:.4} {d:.4} {e:.4} {f:.4})"{}/>"##,
                // A stencil mask paints its dark samples; the rest shows through.
                if image.mask {
                    r#" style="mix-blend-mode:multiply""#
                } else {
                    ""
                }
            );
        }
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
        for (cell, pieces) in sheet.cells.iter().zip(&page.cells) {
            let shown = document.content.cell(name, cell.address());
            // The strings a change of the cell's text rewrites, as an
            // export writes them; a change it would refuse is shown whole
            // where the cell starts.
            let changed = shown.filter(|s| s.original.is_some()).map(|_| {
                let after = text_of(shown, Some(cell));
                pdf_cell_strings(pieces, &page.texts, &after).unwrap_or_else(|| {
                    pieces
                        .iter()
                        .enumerate()
                        .map(|(i, piece)| {
                            (
                                piece.string,
                                if i == 0 { after.clone() } else { String::new() },
                            )
                        })
                        .collect()
                })
            });
            let mark = out.mark(shown, name, cell.address());
            let mut strings = String::new();
            // What the frame of a changed cell is drawn around.
            let mut frame: Option<[f64; 4]> = None;
            for piece in pieces {
                let Some([x, y, width]) = page.places.get(piece.string).copied() else {
                    continue;
                };
                let font = &page.fonts[piece.string];
                let size = font["size"].as_f64().unwrap_or(12.0);
                let rewritten = changed.as_ref().and_then(|c| c.get(&piece.string));
                let text = rewritten.unwrap_or(&page.texts[piece.string]);
                let hidden = if page.invisible.contains(&piece.string) {
                    "fill-opacity:0;"
                } else {
                    ""
                };
                let fit = if width > 0.0 && rewritten.is_none() {
                    format!(r#" textLength="{width:.2}" lengthAdjust="spacingAndGlyphs""#)
                } else {
                    String::new()
                };
                let _ = write!(
                    strings,
                    r#"<text data-a="{}" x="{x:.2}" y="{y:.2}" font-size="{size:.2}" style="{}{hidden}"{fit}>{}</text>"#,
                    cell.address(),
                    text_css(font, cell_style(sheet, Some(cell))),
                    escape(text)
                );
                let wide = text.chars().count() as f64 * size;
                let right = x + width.max(wide.min(width * 2.0)).max(size);
                let bounds = frame.get_or_insert([x, y - size, right, y + size * 0.3]);
                *bounds = [
                    bounds[0].min(x),
                    bounds[1].min(y - size),
                    bounds[2].max(right),
                    bounds[3].max(y + size * 0.3),
                ];
            }
            match frame.filter(|_| !mark.is_empty()) {
                None => html.push_str(&strings),
                // A changed cell is framed where it stands on the page.
                Some([left, top, right, bottom]) => {
                    let note = shown
                        .and_then(|s| s.original.as_ref())
                        .map(|o| format!("原本: {}", plain(o)))
                        .unwrap_or_else(|| "フォントを変更".to_owned());
                    let _ = write!(
                        html,
                        r##"<g{mark}><title>{}</title><rect class="mark" x="{:.2}" y="{:.2}" width="{:.2}" height="{:.2}" fill="none" stroke="#e8590c" stroke-width="1.2"/>{strings}</g>"##,
                        escape(&note),
                        left - 1.0,
                        top,
                        right - left + 2.0,
                        bottom - top
                    );
                }
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
    let mut css = crate::fonts::pdf_css_families(font);
    if let Some(rgb) = color(&font["color"]) {
        css.push_str(&format!("fill:{rgb};"));
    }
    for part in font_css(&Value::Null, style).split_inclusive(';') {
        css.push_str(part);
    }
    css
}
