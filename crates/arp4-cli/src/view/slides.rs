//! A PowerPoint slide in the presentation's slide size, each shape, picture,
//! connector and table where the slide places it, each paragraph of a
//! shape's text in its own font, and the slide's notes under it.
use super::drawings::{Defaults, Rect};
use super::*;
use crate::excel::{column_name, coordinate};
use crate::extraction::Sheet;
use roxmltree::Document as Xml;
use std::collections::HashMap;

const PRESENTATION: &str = "http://schemas.openxmlformats.org/presentationml/2006/main";
const DRAWING: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
/// Pixels of a point, and EMUs of a pixel.
const PX: f64 = 4.0 / 3.0;
const EMU_PER_PIXEL: f64 = 9525.0;

pub(super) fn slides(document: &Document<'_>, out: &mut Out) -> Result<Vec<Section>> {
    // The slide size, 16:9 when the presentation gives none.
    let (width, height) = document
        .original
        .office_part("ppt/presentation.xml")
        .and_then(|text| {
            let xml = Xml::parse(&text).ok()?;
            let size = xml
                .descendants()
                .find(|n| n.has_tag_name((PRESENTATION, "sldSz")))?;
            Some((
                size.attribute("cx")?.parse::<f64>().ok()? / EMU_PER_PIXEL,
                size.attribute("cy")?.parse::<f64>().ok()? / EMU_PER_PIXEL,
            ))
        })
        .unwrap_or((1280.0, 720.0));
    let sheets = &document.extraction.sheets;
    let notes: HashMap<&str, &Sheet> = sheets
        .iter()
        .filter_map(|s| Some((s["name"].as_str()?.strip_prefix("notes-")?, s)))
        .collect();
    let mut sections = vec![];
    for sheet in sheets {
        let name = sheet["name"].as_str().unwrap_or_default();
        if name.starts_with("notes-") {
            continue;
        }
        out.page = sections.len();
        let mut html =
            format!(r#"<div class="slide" style="width:{width:.0}px;height:{height:.0}px">"#);
        html.push_str(&shapes(document, sheet, (width, height), out)?);
        html.push_str("</div>");
        let mut section = Section::new(name, html);
        let number = name.strip_prefix("slide-").unwrap_or(name);
        if let Some(note) = notes.get(number) {
            let part = Part::of(note["name"].as_str().unwrap_or_default(), "ノート");
            section.notes = paragraphs(
                document,
                note,
                &cells_of(note),
                &all(note),
                &part.prefix,
                out,
            );
            section.parts.push(part);
        }
        section.hidden = sheet["state"] == "hidden";
        if section.hidden {
            section.title = format!("{name}（非表示）");
        }
        sections.push(section);
    }
    Ok(sections)
}

fn cells_of(sheet: &Sheet) -> HashMap<&str, crate::extraction::Cell<'_>> {
    sheet.cells.iter().map(|c| (c.address(), c)).collect()
}

/// The addresses of every cell of `sheet`, in order.
fn all(sheet: &Sheet) -> Vec<String> {
    sheet.cells.iter().map(|c| c.address().to_owned()).collect()
}

/// The paragraphs at `addresses`, each in its own font and named by its
/// address after `prefix`.
fn paragraphs(
    document: &Document<'_>,
    sheet: &Sheet,
    cells: &HashMap<&str, crate::extraction::Cell<'_>>,
    addresses: &[String],
    prefix: &str,
    out: &mut Out,
) -> String {
    let name = sheet["name"].as_str().unwrap_or_default();
    let mut html = String::new();
    for address in addresses {
        let cell = cells.get(address.as_str()).copied();
        let shown = document.content.cell(name, address);
        let mark = out.mark(shown, name, address);
        let text = text_of(shown, cell);
        let _ = write!(
            html,
            r#"<div data-a="{prefix}{address}" style="position:relative;{}"{mark}>{}</div>"#,
            font_css(cell_font(sheet, cell, shown), cell_style(sheet, cell)),
            if text.is_empty() {
                "<br>".to_owned()
            } else {
                escape(&text)
            }
        );
    }
    html
}

/// The drawings of a slide, in the order it lists them: later ones on top.
fn shapes(
    document: &Document<'_>,
    sheet: &Sheet,
    size: (f64, f64),
    out: &mut Out,
) -> Result<String> {
    let cells = cells_of(sheet);
    let mut tables: Vec<&Value> = sheet["tables"].as_array().into_iter().flatten().collect();
    let mut html = String::new();
    for drawing in sheet["drawings"].as_array().into_iter().flatten() {
        let anchor = &drawing["anchor"];
        let rect = match (anchor["left"].as_f64(), anchor["top"].as_f64()) {
            (Some(left), Some(top)) => Rect {
                x: left * PX,
                y: top * PX,
                w: anchor["width"].as_f64().unwrap_or(0.0) * PX,
                h: anchor["height"].as_f64().unwrap_or(0.0) * PX,
            },
            // A placeholder without a place of its own takes its layout's.
            _ if drawing["placeholder"].is_object() => {
                placeholder_rect(document, sheet, &drawing["placeholder"], size)
            }
            _ => continue,
        };
        let rotation = anchor["rotation"].as_f64().unwrap_or(0.0);
        if drawing["kind"] == "graphic"
            && let Some(at) = tables.iter().position(|t| t["name"] == drawing["name"])
        {
            let table = tables.remove(at);
            html.push_str(&self::table(
                document, sheet, &cells, drawing, table, rect, out,
            )?);
            continue;
        }
        let addresses: Vec<String> = drawing["paragraphs"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| p.as_str().map(str::to_owned))
            .collect();
        let text = paragraphs(document, sheet, &cells, &addresses, "", out);
        // A placeholder and a text box hold text without a shape of their
        // own; the text of another shape is centred in it.
        let bare = drawing["placeholder"].is_object()
            || (drawing["fill"] == "none" && drawing["line"]["color"] == "none");
        let defaults = if bare {
            Defaults {
                fill: "none",
                line: "none",
                horizontal: "left",
                vertical: "flex-start",
            }
        } else {
            Defaults {
                fill: "#ffffff",
                line: "#404040",
                horizontal: "center",
                vertical: "center",
            }
        };
        // The end of the drawing's ID names the shape in a diagram (`s3`).
        let shape = drawing["id"]
            .as_str()
            .and_then(|id| id.rsplit_once('#'))
            .map(|(_, number)| format!(r##" data-shape="#{number}""##))
            .unwrap_or_default();
        html.push_str(&drawings::drawing(
            drawing,
            rect,
            &text,
            &shape,
            rotation,
            document.assets,
            &defaults,
        ));
    }
    Ok(html)
}

/// A table of a slide in the box of its graphic frame, its columns and rows
/// in the sizes the frame gives them.
fn table(
    document: &Document<'_>,
    sheet: &Sheet,
    cells: &HashMap<&str, crate::extraction::Cell<'_>>,
    drawing: &Value,
    table: &Value,
    rect: Rect,
    out: &mut Out,
) -> Result<String> {
    let name = sheet["name"].as_str().unwrap_or_default();
    let Some((first, last)) = table["range"].as_str().and_then(|r| r.split_once(':')) else {
        return Ok(String::new());
    };
    let ((c0, r0), (c1, r1)) = (coordinate(first)?, coordinate(last)?);
    let (columns, heights) = frame_grid(document, sheet, drawing);
    let merges: Vec<(u32, u32, u32, u32)> = sheet["merges"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| {
            let (a, b) = m.as_str()?.split_once(':')?;
            let ((m0, n0), (m1, n1)) = (coordinate(a).ok()?, coordinate(b).ok()?);
            Some((m0, n0, m1, n1))
        })
        .collect();
    let mut html = format!(
        r#"<table style="position:absolute;left:{:.1}px;top:{:.1}px;width:{:.1}px">"#,
        rect.x, rect.y, rect.w
    );
    if columns.len() == (c1 - c0 + 1) as usize {
        html.push_str("<colgroup>");
        for width in &columns {
            let _ = write!(html, r#"<col style="width:{width:.1}px">"#);
        }
        html.push_str("</colgroup>");
    }
    for r in r0..=r1 {
        let height = heights
            .get((r - r0) as usize)
            .map(|h| format!(r#" style="height:{h:.1}px""#))
            .unwrap_or_default();
        let _ = write!(html, "<tr{height}>");
        for c in c0..=c1 {
            let span = match merges
                .iter()
                .find(|(m0, n0, m1, n1)| (*m0..=*m1).contains(&c) && (*n0..=*n1).contains(&r))
            {
                Some(&(m0, n0, m1, n1)) if (m0, n0) == (c, r) => {
                    format!(r#" colspan="{}" rowspan="{}""#, m1 - m0 + 1, n1 - n0 + 1)
                }
                Some(_) => continue,
                None => String::new(),
            };
            let address = format!("{}{r}", column_name(c)?);
            let cell = cells.get(address.as_str()).copied();
            let shown = document.content.cell(name, &address);
            let style = cell_style(sheet, cell);
            let mut css = font_css(cell_font(sheet, cell, shown), style);
            css.push_str(if style["border"].as_u64().is_some_and(|b| b > 0) {
                "border:1px solid #7f7f7f;"
            } else {
                "border:1px solid #d9d9d9;"
            });
            if style["fill"].as_u64().is_some_and(|f| f > 0) {
                css.push_str("background:#e7e6e6;");
            }
            let mark = out.mark(shown, name, &address);
            let _ = write!(
                html,
                r#"<td{span} data-a="{address}" style="{css}"{mark}>{}</td>"#,
                escape(&text_of(shown, cell))
            );
        }
        html.push_str("</tr>");
    }
    html.push_str("</table>");
    Ok(html)
}

/// The box a placeholder without a place of its own takes: that of the
/// placeholder of its kind (and number) on the slide's layout, else on the
/// layout's master, else the band PowerPoint gives a title or the area it
/// gives a body.
fn placeholder_rect(
    document: &Document<'_>,
    sheet: &Sheet,
    placeholder: &Value,
    (width, height): (f64, f64),
) -> Rect {
    let kind = placeholder["type"].as_str().unwrap_or("body");
    let title = matches!(kind, "title" | "ctrTitle");
    let index = placeholder["index"].as_u64().map(|i| i.to_string());
    let original = document.original;
    let layout = sheet["part"]
        .as_str()
        .and_then(|part| original.office_related(part, "/slideLayout"));
    let master = layout
        .as_deref()
        .and_then(|part| original.office_related(part, "/slideMaster"));
    for part in layout.iter().chain(master.iter()) {
        let Some(text) = original.office_part(part) else {
            continue;
        };
        let Ok(xml) = Xml::parse(&text) else {
            continue;
        };
        let found = xml
            .descendants()
            .filter(|n| n.tag_name().name() == "sp")
            .filter_map(|shape| {
                let ph = shape.descendants().find(|n| n.tag_name().name() == "ph")?;
                let own = ph.attribute("type").unwrap_or("body");
                let same = if title {
                    matches!(own, "title" | "ctrTitle")
                } else {
                    own == kind && (index.is_none() || ph.attribute("idx") == index.as_deref())
                };
                same.then_some(shape)
            })
            .find_map(|shape| {
                let xfrm = shape
                    .descendants()
                    .find(|n| n.has_tag_name((DRAWING, "xfrm")))?;
                let value = |tag: &str, name: &str| -> Option<f64> {
                    let node = xfrm.children().find(|n| n.has_tag_name((DRAWING, tag)))?;
                    Some(node.attribute(name)?.parse::<f64>().ok()? / EMU_PER_PIXEL)
                };
                Some(Rect {
                    x: value("off", "x")?,
                    y: value("off", "y")?,
                    w: value("ext", "cx")?,
                    h: value("ext", "cy")?,
                })
            });
        if let Some(rect) = found {
            return rect;
        }
    }
    if title {
        Rect {
            x: width * 0.07,
            y: height * 0.06,
            w: width * 0.86,
            h: height * 0.17,
        }
    } else {
        Rect {
            x: width * 0.07,
            y: height * 0.27,
            w: width * 0.86,
            h: height * 0.64,
        }
    }
}

/// The widths of the columns and heights of the rows of the table in a
/// slide's graphic frame (`a:gridCol`, `a:tr`), in pixels; empty when the
/// slide does not give them.
fn frame_grid(document: &Document<'_>, sheet: &Sheet, drawing: &Value) -> (Vec<f64>, Vec<f64>) {
    let found = || -> Option<(Vec<f64>, Vec<f64>)> {
        let part = sheet["part"].as_str()?;
        let id = drawing["id"].as_str()?.rsplit_once('#')?.1;
        let text = document.original.office_part(part)?;
        let xml = Xml::parse(&text).ok()?;
        let frame = xml
            .descendants()
            .filter(|n| n.tag_name().name() == "cNvPr" && n.attribute("id") == Some(id))
            .find_map(|n| {
                n.ancestors()
                    .find(|a| a.tag_name().name() == "graphicFrame")
            })?;
        let emus = |tag: &str, attribute: &str| -> Vec<f64> {
            frame
                .descendants()
                .filter(|n| n.has_tag_name((DRAWING, tag)))
                .filter_map(|n| n.attribute(attribute)?.parse::<f64>().ok())
                .map(|emu| emu / EMU_PER_PIXEL)
                .collect()
        };
        Some((emus("gridCol", "w"), emus("tr", "h")))
    };
    found().unwrap_or_default()
}
