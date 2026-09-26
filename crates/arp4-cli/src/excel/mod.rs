pub use coordinates::{
    ComputedRanges, Merges, column_name, column_number, coordinate, map_coordinate, merges_after,
    resolve_insertion,
};
mod coordinates;
use coordinates::*;
pub use operations::{parse_image_operations, parse_operations};
mod operations;
pub use drawings::validate_image_asset;
mod drawings;
use drawings::*;
mod import;
mod visuals;
mod worksheet;
use worksheet::*;
mod package;
mod references;
use references::*;
mod relocation;
use package::*;
pub(crate) use package::{write_archive, write_unchanged};
use relocation::*;
mod render;
#[cfg(windows)]
mod render_native;
mod writeback;
pub use render::render;
#[cfg(windows)]
pub use render_native::worker as render_worker;
pub use writeback::ensure_not_table_label;

use crate::data::*;
use anyhow::{Context, Result, bail, ensure};
use roxmltree::{Document, Node};
use serde_json::{Value, json};
use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{Cursor, Read, Write},
    path::Path,
};
use zip::{ZipArchive, ZipWriter, write::SimpleFileOptions};

const NS: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const MS_REL: &str = "http://schemas.microsoft.com/office/2006/relationships";
const PKG_REL: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
const XDR: &str = "http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing";
const DRAWING: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
const MARKUP_COMPATIBILITY: &str = "http://schemas.openxmlformats.org/markup-compatibility/2006";
const X14: &str = "http://schemas.microsoft.com/office/spreadsheetml/2009/9/main";
const XM: &str = "http://schemas.microsoft.com/office/excel/2006/main";
const CONTENT_TYPES: &str = "http://schemas.openxmlformats.org/package/2006/content-types";
pub struct Workbook {
    pub raw: Vec<u8>,
    pub parts: BTreeMap<String, Vec<u8>>,
    /// Each worksheet's facts other than its cells: name, part, state, merges,
    /// drawings, tables, comments and computed ranges. `sheet_values` adds the cells.
    pub sheets: Vec<Value>,
    /// The cells of each worksheet, in the order of `sheets`.
    pub cells: Vec<Vec<Cell>>,
    /// Number format and appearance of each cell format (`cellXfs`).
    formats: Vec<CellFormat>,
    /// Chart, dialog and macro sheets listed in the workbook but not extracted.
    pub skipped_sheets: Vec<Value>,
    /// Date serials count days from 1904-01-01 (`workbookPr date1904`), not 1900.
    pub date1904: bool,
    /// Cells (`Sheet!A1`) whose value is a picture placed in the cell or a linked
    /// data type, kept as rich data outside the cell.
    pub rich_values: Vec<String>,
}
/// A cell that holds a value or a formula. Cells are typed rather than JSON
/// because a workbook has far more of them than any other fact, and writeback
/// reads only a few; the JSON form is built only for the extraction.
pub struct Cell {
    pub address: String,
    /// The extraction's `type`: `formula`, `string`, `number`, `boolean` or `error`.
    pub kind: &'static str,
    /// The value, or for a formula its cached result.
    pub value: Value,
    pub formula: Option<String>,
    /// Index of the cell's format in `cellXfs` (its `s` attribute).
    pub format: usize,
}
struct CellFormat {
    number_format: String,
    appearance: Value,
}
impl Workbook {
    /// The worksheets as the extraction records them, each with its cells.
    pub fn sheet_values(&self) -> Vec<Value> {
        self.sheets
            .iter()
            .zip(&self.cells)
            .enumerate()
            .map(|(index, (sheet, cells))| {
                let mut sheet = sheet.clone();
                sheet["cells"] = cells
                    .iter()
                    .map(|cell| self.cell_value(index, cell))
                    .collect();
                sheet
            })
            .collect()
    }
    fn cell_value(&self, sheet_index: usize, cell: &Cell) -> Value {
        let format = &self.formats[cell.format];
        json!({"id":format!("c-{}-{}",sheet_index+1,cell.address),"address":cell.address,"type":cell.kind,"value":cell.value,"cached":if cell.formula.is_some(){cell.value.clone()}else{Value::Null},"formula":cell.formula,"number_format":format.number_format,"style":format.appearance})
    }
}
fn xml(bytes: &[u8]) -> Result<Document<'_>> {
    Ok(Document::parse(std::str::from_utf8(bytes)?)?)
}
fn child<'a, 'b>(node: Node<'a, 'b>, name: &str) -> Option<Node<'a, 'b>> {
    node.children().find(|n| n.has_tag_name((NS, name)))
}
fn child_ns<'a, 'b>(node: Node<'a, 'b>, namespace: &str, name: &str) -> Option<Node<'a, 'b>> {
    node.children().find(|n| n.has_tag_name((namespace, name)))
}
/// Cell text from its `t` runs. Phonetic guides (`rPh`, furigana Excel keeps
/// from Japanese IME input) are readings, not part of the displayed value.
fn texts(node: Node<'_, '_>) -> String {
    let raw: String = node
        .descendants()
        .filter(|n| {
            n.has_tag_name((NS, "t")) && !n.ancestors().any(|a| a.has_tag_name((NS, "rPh")))
        })
        .filter_map(|n| n.text())
        .collect();
    decode_xstring(&raw).into_owned()
}

/// The UTF-16 unit of an `_xHHHH_` escape at the start of `text`.
fn escaped_unit(text: &str) -> Option<u16> {
    let bytes = text.as_bytes();
    (bytes.len() >= 7
        && bytes.starts_with(b"_x")
        && bytes[6] == b'_'
        && bytes[2..6].iter().all(u8::is_ascii_hexdigit))
    .then(|| u16::from_str_radix(&text[2..6], 16).ok())
    .flatten()
}

/// Excel stores characters XML cannot carry, such as the carriage return of a
/// pasted line break, as `_xHHHH_` (ST_Xstring), and a literal `_x` that would read
/// as one as `_x005F_x`. Unpaired surrogates keep their escape text.
fn decode_xstring(text: &str) -> Cow<'_, str> {
    if !text.contains("_x") {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("_x") {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let mut units = vec![];
        while let Some(unit) = escaped_unit(&rest[units.len() * 7..]) {
            units.push(unit);
        }
        if units.is_empty() {
            out.push_str("_x");
            rest = &rest[2..];
            continue;
        }
        for decoded in char::decode_utf16(units) {
            let width = decoded.as_ref().map_or(1, |c| c.len_utf16()) * 7;
            match decoded {
                Ok(c) => out.push(c),
                Err(_) => out.push_str(&rest[..width]),
            }
            rest = &rest[width..];
        }
    }
    out.push_str(rest);
    Cow::Owned(out)
}

/// Escapes text Excel would otherwise decode: every `_x` that starts an `_xHHHH_`
/// sequence gets the `_x005F` prefix. Other characters XML cannot carry are refused
/// before writing, and a carriage return is written as `&#13;`.
fn encode_xstring(text: &str) -> Cow<'_, str> {
    if !text.contains("_x") {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len() + 6);
    let mut rest = text;
    while let Some(at) = rest.find("_x") {
        out.push_str(&rest[..at]);
        if escaped_unit(&rest[at..]).is_some() {
            out.push_str("_x005F");
        }
        out.push_str("_x");
        rest = &rest[at + 2..];
    }
    out.push_str(rest);
    Cow::Owned(out)
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OperationKind {
    InsertRows,
    DeleteRows,
    InsertColumns,
    DeleteColumns,
}

#[derive(Clone, Debug)]
pub struct StructuralOperation {
    pub id: String,
    pub sheet: String,
    pub kind: OperationKind,
    pub at: u32,
    pub count: u32,
    pub style_from: Option<u32>,
}

#[derive(Clone, Debug)]
pub struct AnchorPoint {
    pub column: u32,
    pub row: u32,
    pub column_offset: u64,
    pub row_offset: u64,
}

#[derive(Clone, Debug)]
pub struct ImageOperation {
    pub id: String,
    pub sheet: String,
    pub asset: String,
    pub name: Option<String>,
    pub from: AnchorPoint,
    pub to: AnchorPoint,
}

impl StructuralOperation {
    pub fn row_operation(&self) -> bool {
        matches!(
            self.kind,
            OperationKind::InsertRows | OperationKind::DeleteRows
        )
    }
    pub fn insertion(&self) -> bool {
        matches!(
            self.kind,
            OperationKind::InsertRows | OperationKind::InsertColumns
        )
    }
}

pub fn filename(name: &str) -> String {
    static RESERVED: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?i)^(CON|PRN|AUX|NUL|COM[1-9¹²³]|LPT[1-9¹²³])(?:\.|$)").unwrap()
    });
    let trimmed = name.trim_end_matches([' ', '.']).len();
    let mut out = String::new();
    for (i, c) in name.char_indices() {
        if c < ' ' || "<>:\"/\\|?*%".contains(c) || i >= trimmed {
            out.push_str(&format!("%{:02X}", u32::from(c)))
        } else {
            out.push(c)
        }
    }
    if RESERVED.is_match(&out) || out.eq_ignore_ascii_case("assets") {
        let first = out.remove(0);
        out = format!("%{:02X}{out}", u32::from(first));
    }
    format!("{out}.yml")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excel_string_escapes_decode_and_literal_escapes_round_trip() {
        assert_eq!(decode_xstring("a_x000D_\nb"), "a\r\nb");
        assert_eq!(decode_xstring("_x005F_x0041_"), "_x0041_");
        assert_eq!(decode_xstring("_xD83D__xDE00_"), "😀");
        assert_eq!(decode_xstring("_xD83D_x"), "_xD83D_x");
        assert_eq!(decode_xstring("_x_x00_file_x0041"), "_x_x00_file_x0041");
        for text in ["ID_x0041_", "_x000D_", "a_xb", "_x005F_", "plain"] {
            assert_eq!(decode_xstring(&encode_xstring(text)), text);
        }
        assert_eq!(encode_xstring("ID_x0041_"), "ID_x005F_x0041_");
    }
}
