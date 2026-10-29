//! The HTML page of `documents view`: the content of a document drawn as its
//! original lays it out. An Excel sheet is drawn on its grid of columns and
//! rows with their sizes, merges, fills, borders, alignment, fonts and
//! drawings; a Word document as paragraphs and tables on its paper; a
//! PowerPoint slide with each shape, picture, connector and table where the
//! slide places it; a PDF page with each string where the page shows it; a
//! text original as its lines or records. Each value is the content's, and
//! one the content changes is marked, with the original's value to compare.
//! The page needs nothing but itself: images are written into it.
mod drawings;
mod pdf;
mod sheet;
mod shell;
mod slides;
mod text;
mod word;

use crate::{document_source::Source, extraction::Extraction};
use anyhow::Result;
use serde_json::Value;
use std::{collections::BTreeMap, fmt::Write as _, path::Path, sync::Arc};

/// What a page is made from.
pub struct Document<'a> {
    pub id: &'a str,
    /// The original's path in the project.
    pub source: &'a str,
    pub proposal: bool,
    pub original: &'a Source,
    pub extraction: &'a Extraction,
    pub content: &'a Content,
    /// The folder of the document's image assets.
    pub assets: &'a Path,
    /// The row, column, slide and shape operations not yet applied.
    pub operations: usize,
    /// How each sheet is read, by sheet name.
    pub structure: &'a BTreeMap<String, Structure>,
}

/// How a page of the document is read: the elements the structure
/// interpretation finds on it and how the body (content YAML) holds them,
/// for a person to check what a reader of the body takes from it.
#[derive(Debug, Default, serde::Serialize)]
pub struct Structure {
    pub tables: Vec<TableStructure>,
    /// The cells read as text elements, by address.
    pub texts: Vec<String>,
    pub diagrams: Vec<DiagramStructure>,
    pub checkboxes: Vec<CheckboxStructure>,
}

/// A table: its number on the page, its place among the elements of the
/// body, its title and how each of its cells is read.
#[derive(Debug, serde::Serialize)]
pub struct TableStructure {
    pub number: usize,
    pub element: Option<usize>,
    pub title: String,
    pub headings: usize,
    pub cells: Vec<CellStructure>,
}

/// A cell of a table: its role and the cells heading it, from the structure
/// interpretation, and where the body puts it: a heading, the value of a key
/// in a node under other nodes, a mark, a category's value, or nowhere.
#[derive(Debug, serde::Serialize)]
pub struct CellStructure {
    pub address: String,
    pub role: String,
    pub headers: Vec<String>,
    pub shown: bool,
    pub heading: bool,
    pub key: Option<String>,
    pub mark: bool,
    pub category: bool,
    /// `title` or `note` for a text the table holds as its title or a note.
    pub description: Option<&'static str>,
    pub nodes: Vec<String>,
}

/// Drawings joined by lines, read as one element: its lines as the body
/// writes them, and its shapes by their drawing.
#[derive(Debug, serde::Serialize)]
pub struct DiagramStructure {
    pub number: usize,
    pub element: usize,
    pub lines: Vec<String>,
    /// Each shape: its key in the body, the end of its drawing's ID
    /// (`#2`) and its text.
    pub shapes: Vec<(String, String, String)>,
}

/// A check box read as an element of its own.
#[derive(Debug, serde::Serialize)]
pub struct CheckboxStructure {
    pub element: usize,
    pub text: String,
    pub checked: Value,
    pub linked_cell: Option<String>,
    pub at: Option<String>,
}

/// The values the content gives the cells and shapes of the original, by
/// sheet name and cell address or shape ID.
#[derive(Default)]
pub struct Content {
    pub cells: BTreeMap<(String, String), Shown>,
    pub shapes: BTreeMap<(String, String), Shown>,
    /// Values of rows and columns operations insert, which the original has
    /// no place for yet.
    pub additions: Vec<Addition>,
}

impl Content {
    fn cell(&self, sheet: &str, address: &str) -> Option<&Shown> {
        self.cells.get(&(sheet.to_owned(), address.to_owned()))
    }

    fn shape(&self, sheet: &str, id: &str) -> Option<&Shown> {
        self.shapes.get(&(sheet.to_owned(), id.to_owned()))
    }
}

/// A value of the content.
pub struct Shown {
    pub value: Value,
    /// The original's value, when the content's differs from it.
    pub original: Option<Value>,
    /// The value is a formula, written `=…`.
    pub formula: bool,
    /// The font the content gives the value, as the original's font is
    /// written; none for the original's.
    pub font: Option<Arc<Value>>,
    pub font_changed: bool,
}

impl Shown {
    fn changed(&self) -> bool {
        self.original.is_some() || self.font_changed
    }
}

/// A value of a row or column an operation inserts.
pub struct Addition {
    pub sheet: String,
    pub operation: String,
    pub field: String,
    pub value: Value,
}

/// A page and the number of differences it marks.
pub struct Page {
    pub html: String,
    pub differences: usize,
}

/// How the parts of a page are shown: as the program of the original shows
/// them in a browser.
#[derive(Clone, Copy, PartialEq)]
enum Layout {
    /// Excel: a sheet at a time, chosen by its tab, with a formula bar.
    Sheets,
    /// PowerPoint: a slide at a time, chosen from a rail of thumbnails, with
    /// its notes under it.
    Slides,
    /// Word: the document on its paper, its comments beside it.
    Paper,
    /// PDF: the pages one under another.
    Pages,
    /// Text: the lines or records as a table.
    Records,
}

/// The page of `document`.
pub fn page(document: &Document<'_>) -> Result<Page> {
    let mut out = Out::default();
    let (layout, sections) = match document.original {
        Source::Excel(book) => (Layout::Sheets, sheet::sheets(document, book, &mut out)?),
        original if original.format() == "pdf" => (Layout::Pages, pdf::pages(document, &mut out)?),
        original if original.office_part("ppt/presentation.xml").is_some() => {
            (Layout::Slides, slides::slides(document, &mut out)?)
        }
        original if original.office_part("word/document.xml").is_some() => {
            (Layout::Paper, word::document(document, &mut out)?)
        }
        _ => (Layout::Records, text::sheets(document, &mut out)?),
    };
    Ok(Page {
        differences: out.differences.len(),
        html: shell::shell(document, layout, &sections, &out),
    })
}

/// A part of the page: a sheet, slide or page of the original, or the whole
/// of a Word document.
struct Section {
    /// The name of the extraction's sheet it shows.
    sheet: String,
    title: String,
    body: String,
    /// The original hides it (a hidden sheet or slide).
    hidden: bool,
    /// A slide's notes.
    notes: String,
    /// The other pages of the body drawn in it, as a slide's notes or a Word
    /// document's header: the prefix of the places of their cells, the name
    /// of their sheet and what a reader calls them.
    parts: Vec<Part>,
    /// What stands beside the body, as a Word document's comments.
    aside: String,
}

impl Section {
    fn new(sheet: &str, body: String) -> Self {
        Self {
            sheet: sheet.to_owned(),
            title: sheet.to_owned(),
            body,
            hidden: false,
            notes: String::new(),
            parts: vec![],
            aside: String::new(),
        }
    }
}

/// Another page of the body drawn in a part of the page, its cells placed
/// as `<prefix><address>`, so that they are told from the part's own.
struct Part {
    prefix: String,
    sheet: String,
    label: &'static str,
}

impl Part {
    fn of(sheet: &str, label: &'static str) -> Self {
        Self {
            prefix: format!("{sheet}:"),
            sheet: sheet.to_owned(),
            label,
        }
    }
}

/// A difference the page marks: the anchor of its element, the part it is
/// on, where on it and what the content and the original say.
struct Difference {
    anchor: String,
    page: usize,
    place: String,
    value: String,
    note: String,
}

/// What the parts of a page collect as they are drawn.
#[derive(Default)]
struct Out {
    /// The number of the part being drawn.
    page: usize,
    differences: Vec<Difference>,
}

impl Out {
    /// The attributes of an element showing `shown`, at `place` of `section`:
    /// a marked difference is given an anchor, a mark and the original's
    /// value as its title.
    fn mark(&mut self, shown: Option<&Shown>, section: &str, place: &str) -> String {
        let Some(shown) = shown.filter(|s| s.changed()) else {
            return String::new();
        };
        let anchor = format!("d{}", self.differences.len() + 1);
        let mut note = vec![];
        if let Some(original) = &shown.original {
            note.push(format!("原本: {}", plain(original)));
        }
        if shown.font_changed {
            note.push("フォントを変更".to_owned());
        }
        let note = note.join(" / ");
        self.differences.push(Difference {
            anchor: anchor.clone(),
            page: self.page,
            place: format!("{section} {place}"),
            value: plain(&shown.value),
            note: note.clone(),
        });
        format!(
            r#" id="{anchor}" data-changed="1" title="{}""#,
            escape(&note)
        )
    }
}

/// `value` as text: a string as it is, null as nothing.
fn plain(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// `text` with the characters HTML gives a meaning escaped.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// A CSS colour of `RRGGBB`; none for automatic, mixed or unresolved.
fn color(value: &Value) -> Option<String> {
    let rgb = value.as_str()?;
    (rgb.len() == 6 && rgb.bytes().all(|b| b.is_ascii_hexdigit())).then(|| format!("#{rgb}"))
}

/// The CSS of a font as the extraction writes one (latin and East Asian
/// names, size in points, colour), with the emphasis a style or the font's
/// own marks give: what is unresolved is left to the page.
fn font_css(font: &Value, style: &Value) -> String {
    let mut css = String::new();
    let names: Vec<String> = [&font["latin"], &font["east_asian"]]
        .into_iter()
        .filter_map(Value::as_str)
        .filter(|name| *name != "mixed")
        .map(|name| format!("'{}'", name.replace(['\'', '\\', '<', '>', '"'], "")))
        .collect();
    if !names.is_empty() {
        let _ = write!(css, "font-family:{},sans-serif;", names.join(","));
    }
    if let Some(size) = font["size"].as_f64() {
        let _ = write!(css, "font-size:{size}pt;");
    }
    if let Some(rgb) = color(&font["color"]) {
        let _ = write!(css, "color:{rgb};");
    }
    let on = |key: &str| style[key] == true || font[key] == true;
    if on("bold") {
        css.push_str("font-weight:bold;");
    }
    if on("italic") {
        css.push_str("font-style:italic;");
    }
    match (on("underline"), on("strike")) {
        (true, true) => css.push_str("text-decoration:underline line-through;"),
        (true, false) => css.push_str("text-decoration:underline;"),
        (false, true) => css.push_str("text-decoration:line-through;"),
        (false, false) => {}
    }
    css
}

/// The font a cell of `sheet` shows: the content's when it gives one, else
/// the extraction's.
fn cell_font<'a>(
    sheet: &'a crate::extraction::Sheet,
    cell: Option<crate::extraction::Cell<'_>>,
    shown: Option<&'a Shown>,
) -> &'a Value {
    if let Some(font) = shown.and_then(|s| s.font.as_deref()) {
        return font;
    }
    cell.and_then(|c| crate::extraction::font(sheet, c).ok())
        .unwrap_or(&Value::Null)
}

/// The style a cell of `sheet` has in the extraction, null for none.
fn cell_style<'a>(
    sheet: &'a crate::extraction::Sheet,
    cell: Option<crate::extraction::Cell<'_>>,
) -> &'a Value {
    cell.and_then(|c| crate::extraction::style(sheet, c).ok())
        .unwrap_or(&Value::Null)
}

/// The text a value shows when it has no number format of its own.
fn text_of(shown: Option<&Shown>, cell: Option<crate::extraction::Cell<'_>>) -> String {
    match (shown, cell) {
        (Some(shown), _) => plain(&shown.value),
        (None, Some(cell)) => plain(&cell.value().to_json()),
        (None, None) => String::new(),
    }
}

/// An image asset of the document as a `data:` URL, none when it is not
/// kept or is not an image a page shows.
fn image_url(assets: &Path, name: &str) -> Option<String> {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let kind = match name.rsplit_once('.')?.1.to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        _ => return None,
    };
    if name.contains(['/', '\\']) || name.starts_with('.') {
        return None;
    }
    let bytes = std::fs::read(assets.join(name)).ok()?;
    Some(format!("data:{kind};base64,{}", STANDARD.encode(bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_escaped_and_fonts_written_as_css() {
        assert_eq!(
            escape(r#"<a href="x">&'"#),
            "&lt;a href=&quot;x&quot;&gt;&amp;&#39;"
        );
        let css = font_css(
            &serde_json::json!({"latin":"Calibri","east_asian":"游ゴシック","size":12,"color":"FF0000"}),
            &serde_json::json!({"bold":true,"strike":true}),
        );
        assert_eq!(
            css,
            "font-family:'Calibri','游ゴシック',sans-serif;font-size:12pt;color:#FF0000;font-weight:bold;text-decoration:line-through;"
        );
        assert_eq!(color(&serde_json::json!("auto")), None);
    }

    #[test]
    fn a_changed_value_is_marked_with_the_original() {
        let mut out = Out::default();
        let shown = Shown {
            value: serde_json::json!("新"),
            original: Some(serde_json::json!("旧")),
            formula: false,
            font: None,
            font_changed: false,
        };
        let same = Shown {
            original: None,
            ..Shown {
                value: serde_json::json!("同"),
                original: None,
                formula: false,
                font: None,
                font_changed: false,
            }
        };
        assert_eq!(out.mark(Some(&same), "S", "A1"), "");
        assert_eq!(
            out.mark(Some(&shown), "S", "A2"),
            r#" id="d1" data-changed="1" title="原本: 旧""#
        );
        let difference = &out.differences[0];
        assert_eq!(
            (
                difference.place.as_str(),
                difference.value.as_str(),
                difference.note.as_str()
            ),
            ("S A2", "新", "原本: 旧")
        );
    }
}
