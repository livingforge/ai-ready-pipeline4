//! An HTML page as a browser shows it without the page's style sheets: its
//! title in a bar above it, where a browser's tab shows it, then its blocks
//! in order in the browser's own font, each in the size and weight the
//! extraction gives it, code in a fixed-width font, a list item with its
//! bullet or number, lists, definitions, quotes and figures indented, and
//! its tables with their merged cells. A picture is drawn above the block
//! it belongs to; one the import did not keep shows its `src` instead.
use super::word::{Blocks, flow};
use super::*;
use crate::document_source::BlockShape;
use std::collections::BTreeMap as Map;

pub(super) fn page(document: &Document<'_>, out: &mut Out) -> Result<Vec<Section>> {
    let Some(sheet) = document.extraction.sheets.first() else {
        return Ok(vec![]);
    };
    out.page = 0;
    let mut pictures: Map<String, String> = Map::new();
    for image in sheet["images"].as_array().into_iter().flatten() {
        let Some(address) = image["address"].as_str() else {
            continue;
        };
        let html = pictures.entry(address.to_owned()).or_default();
        match image["asset"]
            .as_str()
            .and_then(|asset| image_url(document.assets, asset))
        {
            Some(url) => {
                let alt = image["alt"].as_str().unwrap_or_default();
                let _ = write!(
                    html,
                    r#"<figure><img src="{url}" alt="{}"></figure>"#,
                    escape(alt)
                );
            }
            None => {
                let src = image["src"].as_str().unwrap_or_default();
                let _ = write!(
                    html,
                    r#"<figure class="missing">画像（取り込んでいません）: {}</figure>"#,
                    escape(src)
                );
            }
        }
    }
    let page = Page {
        shapes: document.original.html_shapes()?.unwrap_or_default(),
        pictures,
    };
    let body = flow(document, sheet, "", out, &page)?;
    let name = sheet["name"].as_str().unwrap_or_default();
    Ok(vec![Section::new(
        name,
        format!(r#"<div class="web">{body}</div>"#),
    )])
}

/// The blocks of the page as a browser sets them off, and the pictures of
/// each, by address.
struct Page {
    shapes: Map<String, BlockShape>,
    pictures: Map<String, String>,
}

impl Blocks for Page {
    fn paragraph(&self, place: &str, address: &str, css: &str, mark: &str, text: &str) -> String {
        let shape = self.shapes.get(address);
        let kind = shape.map_or("text", |s| s.kind);
        let pictures = self.before(address);
        if kind == "title" {
            return format!(r#"{pictures}<div class="title" data-a="{place}"{mark}>{text}</div>"#);
        }
        let mut style = css.to_owned();
        if kind == "code" {
            // Browsers show code in their fixed-width font, at 13 pixels; a
            // Latin one first, where a Japanese one shows `\` as a yen sign.
            style.push_str("font-family:Consolas,'Courier New',monospace;font-size:13px;");
        }
        if let Some(shape) = shape.filter(|s| s.indent > 0) {
            let _ = write!(style, "margin-left:{}px;", 40 * shape.indent);
        }
        let marker = shape
            .and_then(|s| s.marker.as_deref())
            .map(|m| format!(r#"<span class="marker">{}</span>"#, escape(m)))
            .unwrap_or_default();
        format!(
            r#"{pictures}<p class="{kind}" data-a="{place}" style="{style}"{mark}>{marker}{text}</p>"#
        )
    }

    fn before(&self, address: &str) -> String {
        self.pictures.get(address).cloned().unwrap_or_default()
    }
}
