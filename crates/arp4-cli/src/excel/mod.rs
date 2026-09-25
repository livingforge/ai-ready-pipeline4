pub use coordinates::{
    column_name, column_number, coordinate, ensure_not_hidden, hiding_merge, map_coordinate,
    merges_after, resolve_insertion,
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

use crate::data::*;
use anyhow::{Context, Result, bail, ensure};
use roxmltree::{Document, Node};
use serde_json::{Value, json};
use std::{
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
    pub sheets: Vec<Value>,
    /// Chart, dialog and macro sheets listed in the workbook but not extracted.
    pub skipped_sheets: Vec<Value>,
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
    node.descendants()
        .filter(|n| {
            n.has_tag_name((NS, "t")) && !n.ancestors().any(|a| a.has_tag_name((NS, "rPh")))
        })
        .filter_map(|n| n.text())
        .collect()
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
    let trimmed = name.trim_end_matches([' ', '.']).len();
    let mut out = String::new();
    for (i, c) in name.char_indices() {
        if c < ' ' || "<>:\"/\\|?*%".contains(c) || i >= trimmed {
            out.push_str(&format!("%{:02X}", u32::from(c)))
        } else {
            out.push(c)
        }
    }
    if regex::Regex::new(r"(?i)^(CON|PRN|AUX|NUL|COM[1-9¹²³]|LPT[1-9¹²³])(?:\.|$)")
        .unwrap()
        .is_match(&out)
        || out.eq_ignore_ascii_case("assets")
    {
        let first = out.remove(0);
        out = format!("%{:02X}{out}", u32::from(first));
    }
    format!("{out}.yml")
}
