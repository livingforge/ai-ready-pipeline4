pub use coordinates::{
    ComputedRanges, CoordinateMap, Merges, Origin, axis_key, column_name, column_number,
    coordinate, key_position, map_coordinate, merges_after, origin_of, resolve_insertion,
};
mod coordinates;
pub(crate) use coordinates::MergeState;
pub(crate) use coordinates::axis_deleted;
pub(crate) use coordinates::copied_merges;
use coordinates::*;
pub use operations::{
    parse_format_operations, parse_image_operations, parse_operations, parse_place_operations,
    recorded_on_rows,
};
pub(crate) use references::{
    moved_row, moved_rows_formula, referenced, rename_sheet, rename_tables, rewrite_formula,
    shift_relative,
};
mod operations;
pub use cell_objects::{CellObjectKind, CellObjectOperation, parse_cell_object_operations};
mod sheet_add;
pub use drawings::validate_image_asset;
pub(crate) use drawings::{drawing_deleted, project_anchor};
pub use sheet_add::{
    SheetChange, SheetOperation, copied_charts, copied_drawing_part, copied_formula, copied_tables,
    is_sheet_operation, parse_sheet_changes, parse_sheet_operations, sheet_name_problem,
    sheet_order, sheet_page, sheet_view,
};
mod drawings;
use drawings::*;
pub use formula_edit::check_formula;
mod formula_edit;
use formula_edit::*;
mod fonts;
mod import;
pub(crate) use import::SheetData;
pub mod number_format;
mod visuals;
pub(crate) use visuals::line_marks;
mod worksheet;
pub(crate) use worksheet::original_position;
use worksheet::*;
mod package;
mod references;
use references::*;
mod relocation;
mod sheet_objects;
use package::*;
pub(crate) use package::{
    archive_bytes, write_archive, write_archive_without, write_unchanged, xml_attr,
};
use relocation::*;
use sheet_objects::*;
mod calculation;
pub use calculation::calculate;
mod fit;
pub use fit::{FitTarget, Place, layout_warnings};
mod cell_objects;
mod column_move;
mod formatting;
mod placing;
mod render;
#[cfg(windows)]
mod render_native;
mod rules;
mod shifted;
mod text_edit;
use text_edit::SharedStrings;
mod vba;
pub use vba::VbaNotUpdated;
mod writeback;
pub use render::render;
#[cfg(windows)]
pub use render_native::calculation_worker;
#[cfg(windows)]
pub use render_native::worker as render_worker;
pub use table_labels::{ensure_table_label_edit, ensure_unique_table_headers, table_label};
mod table_labels;
use table_labels::*;

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
const DIAGRAM: &str = "http://schemas.openxmlformats.org/drawingml/2006/diagram";
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
    pub address: Box<str>,
    /// The value, or for a formula its cached result.
    pub value: Value,
    pub formula: Option<Box<str>>,
    /// The fonts the runs of rich text set; empty for text in the cell's font.
    pub runs: Box<[crate::fonts::RunFont]>,
    /// Index of the cell's format in `cellXfs` (its `s` attribute).
    pub format: u32,
    /// The extraction's `type`: `formula`, `string`, `number`, `boolean` or `error`.
    pub kind: crate::extraction::CellKind,
    /// Every run of the cell's text is bold, whatever the cell format says.
    pub bold_text: bool,
}
struct CellFormat {
    number_format: String,
    appearance: Value,
    font: crate::fonts::RunFont,
    /// The sides its border draws: left, right, top, bottom.
    sides: [bool; 4],
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
    /// The worksheets as an extraction holds them, their cells typed.
    pub fn extraction_sheets(&self) -> Result<Vec<crate::extraction::Sheet>> {
        self.sheets
            .iter()
            .zip(&self.cells)
            .enumerate()
            .map(|(index, (sheet, cells))| {
                let mut built = crate::extraction::Sheet::build(sheet.clone());
                for cell in cells {
                    let (style, font) = self.appearance(cell);
                    let typed = crate::extraction::NewCell {
                        id: &format!("c-{}-{}", index + 1, cell.address),
                        address: &cell.address,
                        kind: cell.kind,
                        value: &cell.value,
                        formula: cell.formula.as_deref(),
                        number_format: &self.formats[cell.format as usize].number_format,
                        font: None,
                        style: None,
                        position: None,
                    };
                    built.push(typed, Some(font), Some(style))?;
                }
                Ok(built.finish())
            })
            .collect()
    }
    /// The style and font of `cell`, as its format and runs give them.
    fn appearance(&self, cell: &Cell) -> (Value, Value) {
        let format = &self.formats[cell.format as usize];
        let mut style = format.appearance.clone();
        if cell.bold_text {
            style["bold"] = json!(true);
        }
        let runs: Vec<_> = cell
            .runs
            .iter()
            .map(|run| run.clone().or(&format.font))
            .collect();
        crate::fonts::element_emphasis(&runs, &format.font).mark(&mut style);
        (style, crate::fonts::element_font(&runs, &format.font))
    }
    fn cell_value(&self, sheet_index: usize, cell: &Cell) -> Value {
        let format = &self.formats[cell.format as usize];
        let (style, font) = self.appearance(cell);
        json!({"id":format!("c-{}-{}",sheet_index+1,cell.address),"address":cell.address,"type":cell.kind.as_str(),"value":cell.value,"cached":if cell.formula.is_some(){cell.value.clone()}else{Value::Null},"formula":cell.formula,"number_format":format.number_format,"style":style,"font":font})
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
/// Whether a font or run property such as `b` or `strike` is set.
/// ST_OnOff: `false` and `off` turn it off as `0` does.
fn on(properties: Node<'_, '_>, name: &str) -> bool {
    child(properties, name)
        .is_some_and(|n| !matches!(n.attribute("val"), Some("0" | "false" | "off")))
}

/// Whether the text is written in runs that are all bold. Authors set a
/// header apart this way without changing the cell format.
fn bold_runs(node: Node<'_, '_>) -> bool {
    let mut runs = node
        .children()
        .filter(|n| n.has_tag_name((NS, "r")))
        .peekable();
    runs.peek().is_some() && runs.all(|r| child(r, "rPr").is_some_and(|p| on(p, "b")))
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
    /// First column of the moved block in the final order.
    MoveColumns {
        to: u32,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct StructuralOperation {
    pub id: String,
    pub sheet: String,
    pub kind: OperationKind,
    pub at: u32,
    pub count: u32,
    pub style_from: Option<u32>,
    /// The number of rows from `style_from` an insert_rows takes its formats
    /// from in turn: the n-th inserted row the (n mod `style_rows`)-th of them.
    pub style_rows: u32,
    /// Whether the merges within the style rows are made again over each
    /// group of inserted rows, as Excel's Insert Copied Cells does.
    pub merges: bool,
    /// Whether the person editing accepted that the workbook's VBA code keeps
    /// the cell addresses it holds: ARP, like Excel, does not rewrite them.
    pub vba_unchanged: bool,
    /// For cells shifted within a band rather than whole rows or columns: the
    /// first and last column of rows shifted down or up, or the first and
    /// last row of columns shifted right or left, as the operations before
    /// leave them.
    pub band: Option<(u32, u32)>,
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
    /// The corners in the cells the row and column operations recorded
    /// before the picture leave; the ones recorded after it move it.
    pub from: AnchorPoint,
    pub to: AnchorPoint,
    /// How many row and column operations were recorded before the picture.
    pub preceding: usize,
}

/// A change of the format of a range of cells (`format_cells`), in the
/// cells the row and column operations recorded before it leave.
#[derive(Clone, Debug)]
pub struct FormatOperation {
    pub id: String,
    pub sheet: String,
    pub range: String,
    /// The cell whose whole format the range takes.
    pub from: Option<String>,
    pub alignment: Alignment,
    /// The font emphasis, fill and number format the cells take.
    pub look: CellLook,
    /// The lines drawn or taken away, in the order given.
    pub borders: Vec<BorderLine>,
    /// Points, or none for the height Excel fits to the text.
    pub row_height: Option<Option<f64>>,
    pub column_width: Option<f64>,
    /// Merge the range, or take apart the merges inside it.
    pub merge: Option<bool>,
    /// For a `copy_cells`, the top left cell the formats and merges of the
    /// range are copied to.
    pub copy_to: Option<String>,
    /// How many row and column operations were recorded before it.
    pub preceding: usize,
}

/// A drawing object moved, resized or given another placement
/// (`place_drawing`), in the cells the row and column operations recorded
/// before it leave.
#[derive(Clone, Debug, PartialEq)]
pub struct PlaceOperation {
    pub id: String,
    pub sheet: String,
    /// The extraction ID of the object anchored: `<drawing part>#<cNvPr id>`.
    pub drawing: String,
    /// The cells of the object's top left and bottom right corners.
    pub from: Option<(u32, u32)>,
    pub to: Option<(u32, u32)>,
    /// Where in those cells the corners are, across and down (EMU); the
    /// cells' top left when none.
    pub offsets: Option<[(u64, u64); 2]>,
    /// How later row and column changes move it: `twoCell` (move and
    /// size), `oneCell` (move) or `absolute` (stay).
    pub placement: Option<String>,
    /// How many row and column operations were recorded before it.
    pub preceding: usize,
}

/// Alignment settings a format operation changes; unset ones stay.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Alignment {
    pub wrap: Option<bool>,
    pub shrink: Option<bool>,
    pub horizontal: Option<String>,
    pub vertical: Option<String>,
}

impl Alignment {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// What a format operation sets of the cells' fonts, fill and number format;
/// none leaves it as it is.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CellLook {
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub strike: Option<bool>,
    /// `none`, `single` or `double`.
    pub underline: Option<String>,
    /// `RRGGBB`, or `none` for no fill.
    pub fill: Option<String>,
    pub number_format: Option<String>,
}

impl CellLook {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// A line a format operation draws on edges of its range, or takes away
/// (`style` none).
#[derive(Clone, Debug, PartialEq)]
pub struct BorderLine {
    /// `all`, `outline`, `inside`, `horizontal`, `vertical`, `top`,
    /// `bottom`, `left` or `right`.
    pub edges: String,
    pub style: String,
    /// `RRGGBB`; none for the automatic color.
    pub color: Option<String>,
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

    /// Whether whole rows or columns move, rather than the cells of a band.
    pub fn whole(&self) -> bool {
        self.band.is_none()
    }

    /// Whether the operation moves the cell at (`column`, `row`) along its
    /// axis: every cell for whole rows or columns, the cells whose other
    /// coordinate lies in the band for shifted cells.
    pub fn moves_cell(&self, column: u32, row: u32) -> bool {
        let across = if self.row_operation() { column } else { row };
        self.band
            .is_none_or(|(first, last)| (first..=last).contains(&across))
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

/// Equivalence is established only by the same reference translator the writer
/// uses. Errors and unproved transformations remain ordinary formula changes.
pub(crate) fn formula_reference_equivalent(
    before: &str,
    after: &str,
    sheet: &str,
    operations: &[StructuralOperation],
) -> bool {
    references::reference_equivalence(before, after, sheet, operations)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A workbook has a cell for each value of its sheets: one holds its
    /// value, address, formula and runs in no more than 88 bytes.
    #[test]
    fn a_worksheet_cell_is_small() {
        assert!(std::mem::size_of::<Cell>() <= 88);
    }

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
