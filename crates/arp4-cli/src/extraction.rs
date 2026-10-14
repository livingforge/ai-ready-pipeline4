//! The form of an extraction, stored and in memory alike. Each sheet keeps its
//! fonts and styles in tables that its cells refer to by index, and a cell
//! keeps no cached formula result, which is its value. A cell's font and style
//! are read through [`font`] and [`style`]; a cell is hashed or compared as
//! [`cell`] gives it, with its font, style and cache in place, so that cell
//! hashes do not depend on how the extraction stores them. [`view`] gives the
//! whole extraction in that expanded form.
//!
//! A sheet without a `fonts` table, such as one an extraction is built from or
//! a slide a slide operation adds, holds its fonts and styles in its cells.
//!
//! The stored encoding keeps each cell and each drawing on one line
//! ([`LINES`]), so that a changed cell or a moved shape is one changed line of
//! a Git diff.
//!
//! The document an extraction belongs to (`document_id`) and the original it
//! was made from (`source`, its path and hash) are recorded once, in the
//! `document.yml` beside it: the stored extraction and its hash leave them
//! out, so an original saved again without a change keeps its extraction. A
//! stored extraction also leaves out what follows from the rest
//! ([`stored_form`]): no assets, and the empty lists of an Excel sheet; the
//! text its note (finding R001) begins with, which the format of the original
//! fixes, and the note itself when nothing is added to that text; each print
//! setting once with the parts that have it; in a drawing, the part its ID
//! names and the members without a value; in a cell, no formula, the General
//! number format and the ID an Excel cell takes from its sheet and address.
//! Reading a stored extraction puts all of it back in place.
use crate::data::{self, array};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::path::Path;

static NULL: Value = Value::Null;

/// The members whose items the stored encoding writes one per line.
const LINES: &[&str] = &["cells", "drawings"];

/// Values stored once, in order of first use, and referred to by index.
#[derive(Default)]
struct Table {
    list: Vec<Value>,
    index: HashMap<String, usize>,
}

impl Table {
    fn add(&mut self, value: Value) -> Value {
        let next = self.list.len();
        let index = *self
            .index
            .entry(serde_json::to_string(&value).unwrap())
            .or_insert(next);
        if index == next {
            self.list.push(value);
        }
        json!(index)
    }
}

/// The cached result an expanded cell carries: its value for a formula,
/// null otherwise.
fn cached(cell: &Map<String, Value>) -> Value {
    if cell.get("type").is_some_and(|kind| kind == "formula") {
        cell.get("value").cloned().unwrap_or(Value::Null)
    } else {
        Value::Null
    }
}

/// Whether `sheet` keeps its fonts and styles in tables.
fn tabled(sheet: &Value) -> bool {
    sheet.get("fonts").is_some()
}

/// Moves the fonts and styles of every sheet's cells into tables, in place.
pub fn compact(extraction: &mut Value) -> Result<()> {
    for sheet in extraction["sheets"]
        .as_array_mut()
        .context("extraction sheets missing")?
    {
        if tabled(sheet) {
            continue;
        }
        let (mut fonts, mut styles) = (Table::default(), Table::default());
        for cell in sheet["cells"]
            .as_array_mut()
            .context("extraction cells missing")?
        {
            let cell = cell.as_object_mut().context("cell must be an object")?;
            // Dropped only when expanding gives it back exactly.
            let stored = cell.remove("cached").unwrap_or(Value::Null);
            ensure!(
                stored == cached(cell),
                "cached result of {} is not its value",
                cell.get("address")
                    .and_then(Value::as_str)
                    .unwrap_or("a cell")
            );
            if let Some(font) = cell.remove("font") {
                cell.insert("font".into(), fonts.add(font));
            }
            if let Some(style) = cell.remove("style") {
                cell.insert("style".into(), styles.add(style));
            }
        }
        sheet["fonts"] = Value::Array(fonts.list);
        sheet["styles"] = Value::Array(styles.list);
    }
    Ok(())
}

/// The font (`key` "font") or style ("style") a cell of `sheet` refers to, or
/// null for none.
fn lookup<'a>(sheet: &'a Value, cell: &'a Value, key: &str) -> Result<&'a Value> {
    let Some(value) = cell.get(key) else {
        return Ok(&NULL);
    };
    if !tabled(sheet) {
        return Ok(value);
    }
    let index = value.as_u64().context("table index must be a number")?;
    usize::try_from(index)
        .ok()
        .and_then(|index| sheet[&format!("{key}s")].get(index))
        .with_context(|| format!("unknown {key} index {index}"))
}

/// The font of a cell of `sheet`, null for none.
pub fn font<'a>(sheet: &'a Value, cell: &'a Value) -> Result<&'a Value> {
    lookup(sheet, cell, "font")
}

/// The style of a cell of `sheet`, null for none.
pub fn style<'a>(sheet: &'a Value, cell: &'a Value) -> Result<&'a Value> {
    lookup(sheet, cell, "style")
}

/// A copy of a cell of `sheet` with its font, style and cached result in place.
pub fn cell(sheet: &Value, cell: &Value) -> Result<Value> {
    let mut expanded = cell.clone();
    if tabled(sheet) {
        let fields = expanded.as_object_mut().context("cell must be an object")?;
        for key in ["font", "style"] {
            if fields.contains_key(key) {
                fields.insert(key.into(), lookup(sheet, cell, key)?.clone());
            }
        }
        let cached = cached(fields);
        fields.insert("cached".into(), cached);
    }
    Ok(expanded)
}

/// A copy of the extraction with every cell as [`cell`] gives it.
pub fn view(extraction: &Value) -> Result<Value> {
    let mut view = json!({});
    for (key, value) in extraction
        .as_object()
        .context("extraction must be an object")?
    {
        if key != "sheets" {
            view[key] = value.clone();
        }
    }
    view["sheets"] = Value::Array(
        array(&extraction["sheets"])?
            .iter()
            .map(sheet_view)
            .collect::<Result<_>>()?,
    );
    Ok(view)
}

/// A copy of `sheet` with every cell as [`cell`] gives it.
pub fn sheet_view(sheet: &Value) -> Result<Value> {
    let mut copy = json!({});
    for (key, value) in sheet.as_object().context("sheet must be an object")? {
        if !matches!(key.as_str(), "cells" | "fonts" | "styles") {
            copy[key] = value.clone();
        }
    }
    copy["cells"] = Value::Array(
        array(&sheet["cells"])?
            .iter()
            .map(|c| cell(sheet, c))
            .collect::<Result<_>>()?,
    );
    Ok(copy)
}

/// The members the stored extraction leaves to `document.yml`.
const SOURCE: &str = "source";
const DOCUMENT: &str = "document_id";

/// The images of the original, left out when there are none.
const ASSETS: &str = "assets";

/// The number format a stored cell leaves out.
const GENERAL: &str = "General";

/// The ID an Excel cell takes from its sheet, the `index`th, and its address.
fn excel_cell_id(index: usize, address: &str) -> String {
    format!("c-{}-{address}", index + 1)
}

/// The findings of an extraction, left out when they are the note alone.
const FINDINGS: &str = "findings";

/// The code of the finding every extraction has: the note on what it reads
/// and leaves out of its original.
const NOTE: &str = "R001";

/// The finding the stored findings stand for when left out: the note with
/// nothing added to the text of its format.
fn plain_note() -> Value {
    json!([{"code":NOTE,"level":"warning","message":""}])
}

/// The text the note of an extraction begins with, which the format of its
/// original fixes, or `None` for an extraction naming no original.
fn base_note(extraction: &Value) -> Option<&'static str> {
    let path = extraction[SOURCE]["path"].as_str()?;
    let format = Path::new(path).extension()?.to_str()?.to_lowercase();
    Some(crate::document_source::base_note(&format))
}

/// The findings as stored: the note without the text its format begins with
/// ([`base_note`]), and none at all when that leaves [`plain_note`].
fn stored_findings(extraction: &Value) -> Result<Option<Value>> {
    let Some(findings) = extraction.get(FINDINGS) else {
        return Ok(None);
    };
    let mut findings = findings.clone();
    if let Some(base) = base_note(extraction) {
        for finding in findings
            .as_array_mut()
            .context("extraction findings must be a list")?
            .iter_mut()
            .filter(|finding| finding["code"] == NOTE)
        {
            let message = data::string(&finding["message"])?;
            let added = message
                .strip_prefix(base)
                .context("the note of the extraction does not begin with the note of its format")?;
            finding["message"] = json!(added);
        }
    }
    Ok((findings != plain_note()).then_some(findings))
}

/// Puts back the findings [`stored_findings`] leaves out of the extraction
/// read, whose original it names.
fn expand_findings(extraction: &mut Value) -> Result<()> {
    if extraction.get(FINDINGS).is_none() {
        extraction[FINDINGS] = plain_note();
    }
    let base = base_note(extraction);
    for finding in extraction[FINDINGS]
        .as_array_mut()
        .context("extraction findings must be a list")?
        .iter_mut()
        .filter(|finding| finding["code"] == NOTE)
    {
        let base = base.context("the extraction names no original for its note")?;
        let added = data::string(&finding["message"])?;
        finding["message"] = json!(format!("{base}{added}"));
    }
    Ok(())
}

/// The print settings of an extraction, each Open XML element with its part.
const PRINT_SETTINGS: &str = "print_settings";

/// The print `settings` as stored: each element once with the parts that
/// have it, in order of first use (`merge`), or, when that cannot give the
/// order back, each element of each part on its own.
fn stored_print_settings(settings: &Value) -> Result<Value> {
    let settings = array(settings)?;
    let grouped = |merge: bool| -> Result<Vec<Value>> {
        let mut groups: Vec<Value> = vec![];
        let mut index: HashMap<(&str, &str), usize> = HashMap::new();
        for setting in settings {
            let key = (
                data::string(&setting["component"])?,
                data::string(&setting["xml"])?,
            );
            let part = &setting["part"];
            match index.get(&key) {
                Some(&at) if merge => groups[at]["parts"]
                    .as_array_mut()
                    .context("print setting parts missing")?
                    .push(part.clone()),
                _ => {
                    index.insert(key, groups.len());
                    groups.push(json!({"component":setting["component"],"xml":setting["xml"],"parts":[part]}));
                }
            }
        }
        Ok(groups)
    };
    for merge in [true, false] {
        let groups = Value::Array(grouped(merge)?);
        if expanded_print_settings(&groups)? == *settings {
            return Ok(groups);
        }
    }
    bail!("print settings of a part are not together")
}

/// The print settings [`stored_print_settings`] gives back: the parts in
/// order of first use, each with its elements in the order stored.
fn expanded_print_settings(groups: &Value) -> Result<Vec<Value>> {
    let groups = array(groups)?;
    let mut parts: Vec<&Value> = vec![];
    for group in groups {
        for part in array(&group["parts"])? {
            if !parts.contains(&part) {
                parts.push(part);
            }
        }
    }
    let mut settings = vec![];
    for part in parts {
        for group in groups {
            if array(&group["parts"])?.contains(part) {
                settings
                    .push(json!({"part":part,"component":group["component"],"xml":group["xml"]}));
            }
        }
    }
    Ok(settings)
}

/// The members every drawing has, left out with these values: what a
/// drawing without a macro, control, name, description, text, group,
/// transform, geometry, connection or image has.
fn drawing_defaults() -> [(&'static str, Value); 11] {
    [
        ("macro", Value::Null),
        ("control", Value::Null),
        ("name", json!("")),
        ("description", json!("")),
        ("text", json!("")),
        ("group", Value::Null),
        ("transform", Value::Null),
        ("geometry", Value::Null),
        ("connections", json!([])),
        ("image", Value::Null),
        ("linked_image", Value::Null),
    ]
}

/// The part a drawing ID `<part>#<id>` names.
fn drawing_part(id: &str) -> Option<&str> {
    id.rsplit_once('#').map(|(part, _)| part)
}

/// A drawing as stored: without the members [`drawing_defaults`] gives and
/// the part its ID names.
fn stored_drawing(drawing: &mut Map<String, Value>) {
    for (key, default) in drawing_defaults() {
        if drawing.get(key) == Some(&default) {
            drawing.remove(key);
        }
    }
    let derived = drawing
        .get("id")
        .and_then(Value::as_str)
        .and_then(drawing_part)
        .map(str::to_owned);
    if derived.is_some() && drawing.get("part").and_then(Value::as_str) == derived.as_deref() {
        drawing.remove("part");
    }
}

/// Puts back what [`stored_drawing`] leaves out of a drawing.
fn expand_drawing(drawing: &mut Map<String, Value>) -> Result<()> {
    for (key, default) in drawing_defaults() {
        drawing.entry(key).or_insert(default);
    }
    if !drawing.contains_key("part") {
        let id = drawing
            .get("id")
            .and_then(Value::as_str)
            .context("drawing ID missing")?;
        let part = drawing_part(id).with_context(|| format!("drawing {id} names no part"))?;
        drawing.insert("part".into(), json!(part));
    }
    Ok(())
}

/// The lists every sheet of an Excel extraction has, left out when empty.
const SHEET_LISTS: [&str; 6] = [
    "hidden_rows",
    "hidden_columns",
    "drawings",
    "tables",
    "comments",
    "computed",
];

/// Whether `extraction` was read from an Excel workbook, whose sheets have
/// every list of [`SHEET_LISTS`].
fn excel(extraction: &Value) -> bool {
    extraction["parser"]
        .as_str()
        .is_some_and(|parser| parser.contains(";cells/"))
}

/// The extraction as it is stored: without its `document_id` and `source`,
/// empty assets or empty lists of an Excel sheet; with its findings as
/// [`stored_findings`] and its print settings as [`stored_print_settings`]
/// give them; each drawing as [`stored_drawing`] gives it, and each cell of
/// the `index`th sheet without a null formula, the General number format or
/// an ID [`excel_cell_id`] gives. Copied, as the expanded form is the one
/// read.
fn stored_form(extraction: &Value) -> Result<Value> {
    let excel = excel(extraction);
    let mut form = Map::new();
    for (key, value) in extraction
        .as_object()
        .context("extraction must be an object")?
    {
        match key.as_str() {
            SOURCE | DOCUMENT => {}
            ASSETS if value.as_array().is_some_and(Vec::is_empty) => {}
            FINDINGS => {
                if let Some(findings) = stored_findings(extraction)? {
                    form.insert(key.clone(), findings);
                }
            }
            PRINT_SETTINGS => {
                form.insert(key.clone(), stored_print_settings(value)?);
            }
            "sheets" => {
                let mut sheets = vec![];
                for (index, sheet) in array(value)?.iter().enumerate() {
                    let mut stored = sheet.clone();
                    if excel && let Some(members) = stored.as_object_mut() {
                        members.retain(|key, value| {
                            !(SHEET_LISTS.contains(&key.as_str())
                                && value.as_array().is_some_and(Vec::is_empty))
                        });
                    }
                    if let Some(drawings) = stored.get_mut("drawings").and_then(Value::as_array_mut)
                    {
                        for drawing in drawings {
                            stored_drawing(
                                drawing
                                    .as_object_mut()
                                    .context("drawing must be an object")?,
                            );
                        }
                    }
                    for cell in stored["cells"]
                        .as_array_mut()
                        .context("extraction cells missing")?
                    {
                        let cell = cell.as_object_mut().context("cell must be an object")?;
                        if cell.get("formula").is_some_and(Value::is_null) {
                            cell.remove("formula");
                        }
                        if cell.get("number_format").and_then(Value::as_str) == Some(GENERAL) {
                            cell.remove("number_format");
                        }
                        let derived = cell
                            .get("address")
                            .and_then(Value::as_str)
                            .map(|address| excel_cell_id(index, address));
                        if derived.is_some()
                            && cell.get("id").and_then(Value::as_str) == derived.as_deref()
                        {
                            cell.remove("id");
                        }
                    }
                    sheets.push(stored);
                }
                form.insert(key.clone(), Value::Array(sheets));
            }
            _ => {
                form.insert(key.clone(), value.clone());
            }
        }
    }
    Ok(Value::Object(form))
}

/// Puts back in place what [`stored_form`] leaves out but the document and
/// the original: the empty assets and lists, the findings and print
/// settings, and what it leaves out of each drawing and cell. The original,
/// which the note of the findings depends on, is in place already.
fn expand_cells(extraction: &mut Value) -> Result<()> {
    if extraction.get(ASSETS).is_none() {
        extraction[ASSETS] = json!([]);
    }
    expand_findings(extraction)?;
    if let Some(settings) = extraction.get(PRINT_SETTINGS) {
        extraction[PRINT_SETTINGS] = Value::Array(expanded_print_settings(settings)?);
    }
    let excel = excel(extraction);
    let Some(sheets) = extraction["sheets"].as_array_mut() else {
        return Ok(());
    };
    for (index, sheet) in sheets.iter_mut().enumerate() {
        if excel {
            for list in SHEET_LISTS {
                if sheet.get(list).is_none() {
                    sheet[list] = json!([]);
                }
            }
        }
        if let Some(drawings) = sheet.get_mut("drawings").and_then(Value::as_array_mut) {
            for drawing in drawings {
                expand_drawing(
                    drawing
                        .as_object_mut()
                        .context("drawing must be an object")?,
                )?;
            }
        }
        for cell in sheet["cells"]
            .as_array_mut()
            .context("extraction cells missing")?
        {
            let cell = cell.as_object_mut().context("cell must be an object")?;
            cell.entry("formula").or_insert(Value::Null);
            cell.entry("number_format")
                .or_insert_with(|| json!(GENERAL));
            if !cell.contains_key("id") {
                let address = cell
                    .get("address")
                    .and_then(Value::as_str)
                    .context("cell address missing")?;
                let id = excel_cell_id(index, address);
                cell.insert("id".into(), json!(id));
            }
        }
    }
    Ok(())
}

/// Compacts and validates `extraction` and returns the bytes stored for it.
pub fn stored(extraction: &mut Value) -> Result<Vec<u8>> {
    compact(extraction)?;
    ensure!(
        extraction.get(SOURCE).is_some(),
        "extraction source missing"
    );
    let form = stored_form(extraction)?;
    data::validate("extraction", &form)?;
    Ok(data::encoded_lines(&form, LINES))
}

/// The hash of the bytes stored for `extraction`: the key a document records.
pub fn hash(extraction: &Value) -> Result<String> {
    if array(&extraction["sheets"])?.iter().all(tabled) {
        return Ok(data::hash_encoded_lines(&stored_form(extraction)?, LINES));
    }
    let mut stored = extraction.clone();
    compact(&mut stored)?;
    Ok(data::hash_encoded_lines(&stored_form(&stored)?, LINES))
}

/// Reads and validates a stored extraction, with the original it was made
/// from as the `document.yml` beside it records.
pub fn read(path: &Path) -> Result<Value> {
    let mut extraction = read_text(path, &read_file(path)?)?;
    attach_source(path, &mut extraction)?;
    expand_cells(&mut extraction)?;
    Ok(extraction)
}

/// [`read`] for the record of a document, whose hash is the key the document
/// records: it must be in the stored encoding of this version.
pub fn read_record(path: &Path) -> Result<Value> {
    let text = read_file(path)?;
    let mut extraction = read_text(path, &text)?;
    ensure!(
        data::encoded_lines(&extraction, LINES) == text.replace("\r\n", "\n").as_bytes(),
        "{} is not stored one cell per line, the format of this version; re-import the original with documents import --force",
        path.display()
    );
    attach_source(path, &mut extraction)?;
    expand_cells(&mut extraction)?;
    Ok(extraction)
}

/// [`read`] for the `text` of a stored extraction and the `document.yml`
/// (`meta`) it was stored with, such as an earlier version of both.
pub fn parse(text: &str, meta: &Value) -> Result<Value> {
    let mut extraction = read_text(Path::new("extraction.json"), text)?;
    attach(&mut extraction, meta);
    expand_cells(&mut extraction)?;
    Ok(extraction)
}

/// Puts in place the document and the original that `meta`, a
/// `document.yml`, records.
fn attach(extraction: &mut Value, meta: &Value) {
    extraction[DOCUMENT] = meta[DOCUMENT].clone();
    extraction[SOURCE] = meta[SOURCE].clone();
}

/// Puts in place the document and the original of the stored extraction at
/// `path`: the `document_id` and `source` of the `document.yml` beside it.
fn attach_source(path: &Path, extraction: &mut Value) -> Result<()> {
    let meta = data::read(&path.with_file_name("document.yml"), Some("document"))
        .with_context(|| format!("{} has no document.yml beside it", path.display()))?;
    attach(extraction, &meta);
    Ok(())
}

fn read_file(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))
}

fn read_text(path: &Path, text: &str) -> Result<Value> {
    let extraction = data::parse(text, true)?;
    ensure!(
        extraction.get(SOURCE).is_none() && extraction.get(DOCUMENT).is_none(),
        "{} records its document or original, which document.yml records in this version; re-import the original with documents import --force",
        path.display()
    );
    ensure!(
        extraction["sheets"]
            .as_array()
            .is_none_or(|sheets| sheets.iter().all(tabled)),
        "{} keeps fonts in every cell, the format before font tables; re-import the original with documents import --force",
        path.display()
    );
    data::validate("extraction", &extraction)?;
    Ok(extraction)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_cells_leave_out_what_reading_puts_back() {
        let source = json!({"path":"book.xlsx","sha256":"a".repeat(64)});
        let cell = |id: &str,
                    address: &str,
                    kind: &str,
                    value: Value,
                    formula: Value,
                    format: &str| {
            json!({"id":id,"address":address,"type":kind,"value":value,"cached":if formula.is_null(){Value::Null}else{value.clone()},"formula":formula,"number_format":format})
        };
        let note = format!("{}added", crate::document_source::base_note("xlsx"));
        let setting =
            |part: &str, xml: &str| json!({"part":part,"component":"pageMargins","xml":xml});
        let drawing = json!({"id":"xl/drawings/drawing1.xml#2","part":"xl/drawings/drawing1.xml","kind":"shape",
            "macro":null,"control":null,"name":"","description":"","text":"","anchor":{"kind":"twoCellAnchor"},
            "group":null,"transform":null,"geometry":"rect","connections":[],"image":null,"linked_image":null});
        let extraction = json!({"schema_version":"2","document_id":"book.xlsx","source":source,"parser":"test",
            "findings":[{"level":"warning","code":"R001","message":note}],"assets":[],
            "print_settings":[setting("xl/worksheets/sheet1.xml","<a/>"),setting("xl/worksheets/sheet2.xml","<a/>"),setting("xl/worksheets/sheet2.xml","<b/>")],
            "sheets":[
                {"name":"A","part":"xl/worksheets/sheet1.xml","state":"visible","merges":[],"drawings":[drawing],"cells":[cell("c-1-A1","A1","string",json!("x"),Value::Null,"General")]},
                {"name":"B","part":"xl/worksheets/sheet2.xml","state":"visible","merges":[],"cells":[
                    cell("c-2-B2","B2","formula",json!(3),json!("1+2"),"0.00"),
                    cell("text-1","A1","string",json!("y"),Value::Null,""),
                    cell("c-1-C3","C3","string",json!("z"),Value::Null,"General")]}]});
        let mut expanded = extraction.clone();
        let bytes = stored(&mut expanded).unwrap();
        let text = String::from_utf8(bytes.clone()).unwrap();
        assert!(!text.contains("\"source\"") && !text.contains("General"));
        assert!(!text.contains("c-1-A1") && !text.contains("c-2-B2"));
        assert!(text.contains("text-1") && text.contains("c-1-C3") && text.contains("1+2"));
        // The note keeps only what the original adds to the text of its
        // format, a print setting is stored once and a drawing keeps no part
        // or empty member.
        assert!(
            !text.contains(crate::document_source::base_note("xlsx")) && text.contains("added")
        );
        assert_eq!(text.matches("<a/>").count(), 1);
        assert!(!text.contains("\"xl/drawings/drawing1.xml\"") && !text.contains("linked_image"));
        assert_eq!(hash(&extraction).unwrap(), data::hash(&bytes));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("extraction.json");
        std::fs::write(&path, &bytes).unwrap();
        data::write(
            &dir.path().join("document.yml"),
            &json!({"schema_version":"1","document_id":"book.xlsx","source":extraction["source"],"extraction":data::hash(&bytes)}),
        )
        .unwrap();
        assert_eq!(read_record(&path).unwrap(), expanded);
        assert_eq!(
            view(&read(&path).unwrap()).unwrap(),
            view(&extraction).unwrap()
        );
    }
}
