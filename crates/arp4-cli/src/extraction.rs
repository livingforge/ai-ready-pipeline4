//! The form of an extraction, stored and in memory. An [`Extraction`] keeps
//! the cells of each [`Sheet`] typed ([`Cell`]), as a document has far more
//! cells than any other fact, and the rest of it as JSON. Each sheet keeps
//! its fonts and styles in tables that its cells refer to by index, and a
//! cell keeps no cached formula result, which is its value. A cell's font and
//! style are read through [`font`] and [`style`]; a cell is hashed or compared
//! as [`cell`] gives it, with its font, style and cache in place, so that cell
//! hashes do not depend on how the extraction stores them. [`view`] gives the
//! whole extraction in that expanded form.
//!
//! A sheet an extraction is built from, such as one of the sheets of an
//! original or a slide a slide operation adds, holds its fonts and styles in
//! its cells: [`Sheet::from_value`] puts them in tables.
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
use std::ops::{Index, IndexMut};
use std::path::Path;
use std::sync::Arc;

/// The members whose items the stored encoding writes one per line.
const LINES: &[&str] = &["cells", "drawings"];

/// The type of a cell's value, its `type`: `null` is a cell holding only a
/// format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellKind {
    String,
    Number,
    Boolean,
    Null,
    Formula,
    Error,
}

impl CellKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Number => "number",
            Self::Boolean => "boolean",
            Self::Null => "null",
            Self::Formula => "formula",
            Self::Error => "error",
        }
    }

    pub fn parse(name: &str) -> Result<Self> {
        Ok(match name {
            "string" => Self::String,
            "number" => Self::Number,
            "boolean" => Self::Boolean,
            "null" => Self::Null,
            "formula" => Self::Formula,
            "error" => Self::Error,
            _ => bail!("unknown cell type {name}"),
        })
    }
}

/// A cell of an extraction sheet: its value, for a formula its cached
/// result, and its font and style as indexes into the tables of its sheet.
#[derive(Clone, Debug, PartialEq)]
pub struct Cell {
    /// Shared with what refers to the cell, such as its default mapping entry.
    pub id: Arc<str>,
    pub address: Arc<str>,
    pub kind: CellKind,
    pub value: Value,
    pub formula: Option<Box<str>>,
    /// Shared by the cells of a sheet with the same format.
    pub number_format: Arc<str>,
    pub font: Option<usize>,
    pub style: Option<usize>,
    /// Where the text of a text original lies.
    pub position: Option<Box<Value>>,
}

impl Cell {
    /// The cell as JSON, with its font and style as indexes.
    pub fn to_value(&self) -> Value {
        let mut value = json!({"id":&*self.id,"address":&*self.address,"type":self.kind.as_str(),
            "formula":self.formula.as_deref(),"number_format":&*self.number_format});
        value["value"] = self.value.clone();
        if let Some(font) = self.font {
            value["font"] = json!(font);
        }
        if let Some(style) = self.style {
            value["style"] = json!(style);
        }
        if let Some(position) = &self.position {
            value["position"] = (**position).clone();
        }
        value
    }

    /// The cached result a cell carries in its expanded form: its value for a
    /// formula, null otherwise.
    pub fn cached(&self) -> &Value {
        if self.kind == CellKind::Formula {
            &self.value
        } else {
            &Value::Null
        }
    }
}

/// A sheet of an extraction: its cells, the fonts and styles they refer to,
/// and its other members (`fields`), such as its name, merges and drawings.
#[derive(Clone, Debug, PartialEq)]
pub struct Sheet {
    pub fields: Value,
    pub cells: Vec<Cell>,
    pub fonts: Vec<Value>,
    pub styles: Vec<Value>,
}

impl Index<&str> for Sheet {
    type Output = Value;
    fn index(&self, key: &str) -> &Value {
        debug_assert!(
            !matches!(key, "cells" | "fonts" | "styles"),
            "sheet {key} are typed"
        );
        &self.fields[key]
    }
}

impl IndexMut<&str> for Sheet {
    fn index_mut(&mut self, key: &str) -> &mut Value {
        debug_assert!(
            !matches!(key, "cells" | "fonts" | "styles"),
            "sheet {key} are typed"
        );
        &mut self.fields[key]
    }
}

/// The members of a sheet other than its cells, which code reading only its
/// name, merges, drawings and such sees alike in a sheet of an extraction and
/// in a sheet of an original given as JSON with its cells.
pub trait Fields: Clone {
    fn fields(&self) -> &Value;
    fn fields_mut(&mut self) -> &mut Value;
    /// The sheet with `fields` and the `cells` given as JSON with their fonts
    /// and styles in place, as a slide made from a layout starts.
    fn with_cells(fields: Value, cells: &Value) -> Result<Self>;
}

impl Fields for Value {
    fn fields(&self) -> &Value {
        self
    }
    fn fields_mut(&mut self) -> &mut Value {
        self
    }
    fn with_cells(mut fields: Value, cells: &Value) -> Result<Self> {
        fields["cells"] = cells.clone();
        Ok(fields)
    }
}

impl Fields for Sheet {
    fn fields(&self) -> &Value {
        &self.fields
    }
    fn fields_mut(&mut self) -> &mut Value {
        &mut self.fields
    }
    fn with_cells(fields: Value, cells: &Value) -> Result<Self> {
        Sheet::from_value(Value::with_cells(fields, cells)?, 0)
    }
}

/// An extraction: its sheets and its other members (`fields`), such as its
/// parser, findings and original.
#[derive(Clone, Debug, PartialEq)]
pub struct Extraction {
    pub fields: Value,
    pub sheets: Vec<Sheet>,
}

impl Index<&str> for Extraction {
    type Output = Value;
    fn index(&self, key: &str) -> &Value {
        debug_assert!(key != "sheets", "extraction sheets are typed");
        &self.fields[key]
    }
}

impl IndexMut<&str> for Extraction {
    fn index_mut(&mut self, key: &str) -> &mut Value {
        debug_assert!(key != "sheets", "extraction sheets are typed");
        &mut self.fields[key]
    }
}

/// Values stored once, in order of first use, and referred to by index.
#[derive(Default)]
struct Table {
    list: Vec<Value>,
    index: HashMap<String, usize>,
}

impl Table {
    fn add(&mut self, value: Value) -> usize {
        let next = self.list.len();
        let index = *self
            .index
            .entry(serde_json::to_string(&value).unwrap())
            .or_insert(next);
        if index == next {
            self.list.push(value);
        }
        index
    }
}

/// The string member `key` of a cell given as JSON.
fn text(cell: &Map<String, Value>, key: &str) -> Result<Arc<str>> {
    Ok(cell
        .get(key)
        .and_then(Value::as_str)
        .with_context(|| format!("cell {key} must be a string"))?
        .into())
}

/// The index into a font or style table that a cell given as JSON holds.
fn table_index(value: &Value) -> Result<usize> {
    value
        .as_u64()
        .and_then(|index| usize::try_from(index).ok())
        .context("table index must be a number")
}

impl Sheet {
    /// The `index`th sheet of an extraction given as JSON: with its fonts and
    /// styles in tables or, as a sheet an extraction is built from, in its
    /// cells with their cached results. A cell without a formula, number
    /// format or ID has none, the General format and the ID an Excel cell
    /// takes from its sheet and address.
    pub fn from_value(sheet: Value, index: usize) -> Result<Self> {
        let Value::Object(mut fields) = sheet else {
            bail!("sheet must be an object")
        };
        let Some(Value::Array(cells)) = fields.remove("cells") else {
            bail!("extraction cells missing")
        };
        let tabled = fields.contains_key("fonts");
        let mut fonts = Table::default();
        let mut styles = Table::default();
        if tabled {
            for (key, table) in [("fonts", &mut fonts), ("styles", &mut styles)] {
                match fields.remove(key) {
                    Some(Value::Array(list)) => table.list = list,
                    _ => bail!("sheet {key} must be a list"),
                }
            }
        }
        let mut formats: HashMap<String, Arc<str>> = HashMap::new();
        let cells = cells
            .into_iter()
            .map(|cell| typed_cell(cell, index, tabled, &mut formats, &mut fonts, &mut styles))
            .collect::<Result<_>>()?;
        Ok(Self {
            fields: Value::Object(fields),
            cells,
            fonts: fonts.list,
            styles: styles.list,
        })
    }

    /// A sheet with `fields` and no cells yet, to [`SheetBuilder::push`] them
    /// on.
    pub fn build(fields: Value) -> SheetBuilder {
        SheetBuilder {
            sheet: Self {
                fields,
                cells: vec![],
                fonts: vec![],
                styles: vec![],
            },
            fonts: Table::default(),
            styles: Table::default(),
            formats: HashMap::new(),
        }
    }

    /// The sheet as JSON, with its fonts and styles in tables.
    pub fn to_value(&self) -> Value {
        let mut sheet = self.fields.clone();
        sheet["cells"] = Value::Array(self.cells.iter().map(Cell::to_value).collect());
        sheet["fonts"] = Value::Array(self.fonts.clone());
        sheet["styles"] = Value::Array(self.styles.clone());
        sheet
    }
}

/// The cell `cell` of the `index`th sheet, given as JSON as
/// [`Sheet::from_value`] reads it: with indexes into its sheet's font and
/// style tables when `tabled`, and otherwise with its font and style, which
/// go into `fonts` and `styles`. Its number format is shared through
/// `formats` with the other cells of its sheet.
fn typed_cell(
    cell: Value,
    index: usize,
    tabled: bool,
    formats: &mut HashMap<String, Arc<str>>,
    fonts: &mut Table,
    styles: &mut Table,
) -> Result<Cell> {
    let Value::Object(mut cell) = cell else {
        bail!("cell must be an object")
    };
    let address = text(&cell, "address")?;
    let kind = CellKind::parse(&text(&cell, "type")?)?;
    let id = match cell.get("id") {
        None => excel_cell_id(index, &address).into(),
        Some(_) => text(&cell, "id")?,
    };
    for key in ["id", "address", "type"] {
        cell.remove(key);
    }
    let value = cell.remove("value").context("cell value missing")?;
    let formula = match cell.remove("formula") {
        None | Some(Value::Null) => None,
        Some(Value::String(formula)) => Some(formula.into_boxed_str()),
        Some(_) => bail!("cell formula must be a string"),
    };
    let number_format = match cell.remove("number_format") {
        None => GENERAL.to_owned(),
        Some(Value::String(format)) => format,
        Some(_) => bail!("cell number format must be a string"),
    };
    let number_format = formats
        .entry(number_format)
        .or_insert_with_key(|format| format.as_str().into())
        .clone();
    let mut typed = Cell {
        id,
        address,
        kind,
        value,
        formula,
        number_format,
        font: None,
        style: None,
        position: cell.remove("position").map(Box::new),
    };
    let (font, style) = (cell.remove("font"), cell.remove("style"));
    if tabled {
        typed.font = font.as_ref().map(table_index).transpose()?;
        typed.style = style.as_ref().map(table_index).transpose()?;
    } else {
        // Dropped only when expanding gives it back exactly.
        let stored = cell.remove("cached").unwrap_or(Value::Null);
        ensure!(
            stored == *typed.cached(),
            "cached result of {} is not its value",
            typed.address
        );
        typed.font = font.map(|font| fonts.add(font));
        typed.style = style.map(|style| styles.add(style));
    }
    if let Some(key) = cell.keys().next() {
        bail!("unknown member {key} of cell {}", typed.address);
    }
    Ok(typed)
}

/// A sheet being built cell by cell from what an original holds, with its
/// fonts and styles put in tables in order of first use, as
/// [`Sheet::from_value`] puts those of a sheet given as JSON.
pub struct SheetBuilder {
    sheet: Sheet,
    fonts: Table,
    styles: Table,
    formats: HashMap<Box<str>, Arc<str>>,
}

impl SheetBuilder {
    /// The number format `format`, shared by the cells that have it.
    pub fn number_format(&mut self, format: &str) -> Arc<str> {
        if let Some(shared) = self.formats.get(format) {
            return shared.clone();
        }
        let shared: Arc<str> = format.into();
        self.formats.insert(format.into(), shared.clone());
        shared
    }

    /// Adds `cell` with its font and style.
    pub fn push(&mut self, mut cell: Cell, font: Option<Value>, style: Option<Value>) {
        cell.font = font.map(|font| self.fonts.add(font));
        cell.style = style.map(|style| self.styles.add(style));
        self.sheet.cells.push(cell);
    }

    pub fn finish(mut self) -> Sheet {
        self.sheet.fonts = self.fonts.list;
        self.sheet.styles = self.styles.list;
        self.sheet
    }
}

impl Extraction {
    /// The extraction given as JSON, each sheet as [`Sheet::from_value`]
    /// reads it.
    pub fn from_value(extraction: Value) -> Result<Self> {
        let Value::Object(mut fields) = extraction else {
            bail!("extraction must be an object")
        };
        let sheets = match fields.remove("sheets") {
            Some(Value::Array(sheets)) => sheets
                .into_iter()
                .enumerate()
                .map(|(index, sheet)| Sheet::from_value(sheet, index))
                .collect::<Result<_>>()?,
            _ => bail!("extraction sheets missing"),
        };
        Ok(Self {
            fields: Value::Object(fields),
            sheets,
        })
    }

    /// The member at the JSON `pointer`, as [`Value::pointer`] finds it in
    /// [`Extraction::to_value`]; none in the cells of a sheet or their tables,
    /// which no pointer names.
    pub fn pointer(&self, pointer: &str) -> Option<&Value> {
        let Some(rest) = pointer.strip_prefix("/sheets/") else {
            return self.fields.pointer(pointer);
        };
        let (index, member) = rest.split_once('/')?;
        if matches!(member.split('/').next(), Some("cells" | "fonts" | "styles")) {
            return None;
        }
        self.sheets
            .get(index.parse::<usize>().ok()?)?
            .fields
            .pointer(&format!("/{member}"))
    }

    /// The extraction as JSON, each sheet as [`Sheet::to_value`] gives it.
    pub fn to_value(&self) -> Value {
        let mut extraction = self.fields.clone();
        extraction["sheets"] = Value::Array(self.sheets.iter().map(Sheet::to_value).collect());
        extraction
    }
}

/// The extraction a test gives as JSON, with what its sheets and cells leave
/// out made up: no cells, a null value, the type of the value, or of the
/// formula, it holds, and the cached result of a formula.
#[cfg(test)]
pub(crate) fn sample(mut extraction: Value) -> Extraction {
    for sheet in extraction["sheets"].as_array_mut().into_iter().flatten() {
        if sheet.get("cells").is_none() {
            sheet["cells"] = json!([]);
        }
        for cell in sheet["cells"].as_array_mut().unwrap() {
            if cell.get("value").is_none() {
                cell["value"] = Value::Null;
            }
            if cell.get("type").is_none() {
                cell["type"] = json!(if cell["formula"].is_string() {
                    "formula"
                } else {
                    match &cell["value"] {
                        Value::String(_) => "string",
                        Value::Number(_) => "number",
                        Value::Bool(_) => "boolean",
                        _ => "null",
                    }
                });
            }
            if cell["type"] == "formula" && cell.get("cached").is_none() {
                cell["cached"] = cell["value"].clone();
            }
        }
    }
    Extraction::from_value(extraction).unwrap()
}

/// The entry `index` of a font or style `table`, null for none.
fn lookup<'a>(table: &'a [Value], index: Option<usize>, key: &str) -> Result<&'a Value> {
    match index {
        None => Ok(&Value::Null),
        Some(index) => table
            .get(index)
            .with_context(|| format!("unknown {key} index {index}")),
    }
}

/// The font of a cell of `sheet`, null for none.
pub fn font<'a>(sheet: &'a Sheet, cell: &Cell) -> Result<&'a Value> {
    lookup(&sheet.fonts, cell.font, "font")
}

/// The style of a cell of `sheet`, null for none.
pub fn style<'a>(sheet: &'a Sheet, cell: &Cell) -> Result<&'a Value> {
    lookup(&sheet.styles, cell.style, "style")
}

/// A cell of `sheet` as JSON with its font, style and cached result in place.
pub fn cell(sheet: &Sheet, cell: &Cell) -> Result<Value> {
    let mut expanded = cell.to_value();
    if cell.font.is_some() {
        expanded["font"] = font(sheet, cell)?.clone();
    }
    if cell.style.is_some() {
        expanded["style"] = style(sheet, cell)?.clone();
    }
    expanded["cached"] = cell.cached().clone();
    Ok(expanded)
}

/// The extraction as JSON with every cell as [`cell`] gives it.
pub fn view(extraction: &Extraction) -> Result<Value> {
    let mut view = extraction.fields.clone();
    view["sheets"] = Value::Array(
        extraction
            .sheets
            .iter()
            .map(sheet_view)
            .collect::<Result<_>>()?,
    );
    Ok(view)
}

/// [`view`] of `extraction`, each cell made as it is written: the cells of a
/// large sheet with their fonts and styles in place took several times the
/// memory of the extraction.
pub fn view_lazy(extraction: &Extraction) -> data::Lazy<'_> {
    use data::Lazy;
    let mut members = Lazy::members(&extraction.fields, &[]);
    members.push((
        "sheets",
        Lazy::Items(Box::new(extraction.sheets.iter().map(|sheet| {
            let mut members = Lazy::members(&sheet.fields, &[]);
            members.push((
                "cells",
                Lazy::Items(Box::new(
                    sheet.cells.iter().map(|c| Ok(Lazy::Value(cell(sheet, c)?))),
                )),
            ));
            Ok(Lazy::Object(members))
        }))),
    ));
    Lazy::Object(members)
}

/// The cells of a sheet [`check_view`] checks together.
const VIEW_PART: usize = 4096;

/// Checks [`view`] of `extraction` with `check` a part at a time: the
/// extraction without sheets, then with one sheet holding at most
/// [`VIEW_PART`] of its cells, as no part of the contract relates the items
/// of a list.
pub fn check_view(extraction: &Extraction, check: impl Fn(&Value) -> Result<()>) -> Result<()> {
    let mut part = extraction.fields.clone();
    part["sheets"] = json!([]);
    check(&part)?;
    for sheet in &extraction.sheets {
        let mut chunks: Vec<&[Cell]> = sheet.cells.chunks(VIEW_PART).collect();
        if chunks.is_empty() {
            chunks.push(&[]);
        }
        let mut copy = sheet.fields.clone();
        for chunk in chunks {
            copy["cells"] = Value::Array(
                chunk
                    .iter()
                    .map(|c| cell(sheet, c))
                    .collect::<Result<_>>()?,
            );
            part["sheets"] = Value::Array(vec![copy]);
            check(&part)?;
            copy = part["sheets"][0].take();
        }
    }
    Ok(())
}

/// `sheet` as JSON with every cell as [`cell`] gives it.
pub fn sheet_view(sheet: &Sheet) -> Result<Value> {
    let mut copy = sheet.fields.clone();
    copy["cells"] = Value::Array(
        sheet
            .cells
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

/// Whether the extraction with `fields` was read from an Excel workbook,
/// whose sheets have every list of [`SHEET_LISTS`].
fn excel(fields: &Value) -> bool {
    fields["parser"]
        .as_str()
        .is_some_and(|parser| parser.contains(";cells/"))
}

/// A cell of the `index`th sheet as it is stored: without a null formula, the
/// General number format or an ID [`excel_cell_id`] gives.
fn stored_cell(cell: &Cell, index: usize) -> Value {
    let mut stored = json!({"address":&*cell.address,"type":cell.kind.as_str()});
    stored["value"] = cell.value.clone();
    if let Some(formula) = &cell.formula {
        stored["formula"] = json!(&**formula);
    }
    if &*cell.number_format != GENERAL {
        stored["number_format"] = json!(&*cell.number_format);
    }
    if *cell.id != *excel_cell_id(index, &cell.address) {
        stored["id"] = json!(&*cell.id);
    }
    if let Some(font) = cell.font {
        stored["font"] = json!(font);
    }
    if let Some(style) = cell.style {
        stored["style"] = json!(style);
    }
    if let Some(position) = &cell.position {
        stored["position"] = (**position).clone();
    }
    stored
}

/// The extraction as it is stored: without its `document_id` and `source`,
/// empty assets or empty lists of an Excel sheet; with its findings as
/// [`stored_findings`] and its print settings as [`stored_print_settings`]
/// give them; each drawing as [`stored_drawing`] gives it, and each cell as
/// [`stored_cell`] gives it.
struct StoredForm {
    /// The members of the extraction besides its sheets.
    top: Map<String, Value>,
    /// The members of each sheet besides its cells, fonts and styles.
    sheets: Vec<Value>,
}

impl StoredForm {
    /// The stored form of `extraction`, made a cell at a time.
    fn lazy<'a>(&'a self, extraction: &'a Extraction) -> data::Lazy<'a> {
        use data::Lazy;
        let mut members: Vec<_> = self
            .top
            .iter()
            .map(|(key, value)| (key.as_str(), Lazy::Borrowed(value)))
            .collect();
        members.push((
            "sheets",
            Lazy::Items(Box::new(
                extraction.sheets.iter().zip(&self.sheets).enumerate().map(
                    |(index, (sheet, fields))| {
                        let mut members = Lazy::members(fields, &[]);
                        members.push((
                            "cells",
                            Lazy::Items(Box::new(
                                sheet
                                    .cells
                                    .iter()
                                    .map(move |c| Ok(Lazy::Value(stored_cell(c, index)))),
                            )),
                        ));
                        members.push(("fonts", Lazy::Value(Value::Array(sheet.fonts.clone()))));
                        members.push(("styles", Lazy::Value(Value::Array(sheet.styles.clone()))));
                        Ok(Lazy::Object(members))
                    },
                ),
            )),
        ));
        Lazy::Object(members)
    }

    /// Checks the stored form of `extraction` against its contract a part at
    /// a time: without sheets, then with one sheet holding at most
    /// [`VIEW_PART`] of its cells, as no part of the contract relates the
    /// cells of a sheet.
    fn check(&self, extraction: &Extraction) -> Result<()> {
        let mut part = Value::Object(self.top.clone());
        part["sheets"] = json!([]);
        data::validate("extraction", &part)?;
        for (index, (sheet, fields)) in extraction.sheets.iter().zip(&self.sheets).enumerate() {
            let mut chunks: Vec<&[Cell]> = sheet.cells.chunks(VIEW_PART).collect();
            if chunks.is_empty() {
                chunks.push(&[]);
            }
            let mut copy = fields.clone();
            copy["fonts"] = Value::Array(sheet.fonts.clone());
            copy["styles"] = Value::Array(sheet.styles.clone());
            for chunk in chunks {
                copy["cells"] = chunk.iter().map(|c| stored_cell(c, index)).collect();
                part["sheets"] = Value::Array(vec![copy]);
                data::validate("extraction", &part)?;
                copy = part["sheets"][0].take();
            }
        }
        Ok(())
    }
}

/// The members [`StoredForm`] keeps of `extraction`.
fn stored_form(extraction: &Extraction) -> Result<StoredForm> {
    let fields = &extraction.fields;
    let excel = excel(fields);
    let mut form = Map::new();
    for (key, value) in fields.as_object().context("extraction must be an object")? {
        match key.as_str() {
            SOURCE | DOCUMENT => {}
            ASSETS if value.as_array().is_some_and(Vec::is_empty) => {}
            FINDINGS => {
                if let Some(findings) = stored_findings(fields)? {
                    form.insert(key.clone(), findings);
                }
            }
            PRINT_SETTINGS => {
                form.insert(key.clone(), stored_print_settings(value)?);
            }
            _ => {
                form.insert(key.clone(), value.clone());
            }
        }
    }
    let mut sheets = vec![];
    for sheet in &extraction.sheets {
        let mut stored = sheet.fields.clone();
        let members = stored.as_object_mut().context("sheet must be an object")?;
        if excel {
            members.retain(|key, value| {
                !(SHEET_LISTS.contains(&key.as_str())
                    && value.as_array().is_some_and(Vec::is_empty))
            });
        }
        if let Some(drawings) = members.get_mut("drawings").and_then(Value::as_array_mut) {
            for drawing in drawings {
                stored_drawing(
                    drawing
                        .as_object_mut()
                        .context("drawing must be an object")?,
                );
            }
        }
        sheets.push(stored);
    }
    Ok(StoredForm { top: form, sheets })
}

/// Puts back in place what [`stored_form`] leaves out but the document, the
/// original and what it leaves out of each cell, which [`Sheet::from_value`]
/// gives back: the empty assets and lists, the findings and print settings,
/// and what it leaves out of each drawing. The original, which the note of
/// the findings depends on, is in place already.
fn expand(extraction: &mut Value) -> Result<()> {
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
    for sheet in sheets {
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
    }
    Ok(())
}

/// Validates `extraction` and returns the bytes stored for it.
pub fn stored(extraction: &Extraction) -> Result<Vec<u8>> {
    ensure!(
        extraction.fields.get(SOURCE).is_some(),
        "extraction source missing"
    );
    // Encoded a cell at a time: the stored form of every cell of a large
    // sheet held several times the memory of the extraction.
    let form = stored_form(extraction)?;
    form.check(extraction)?;
    data::encoded_lazy_lines(form.lazy(extraction), LINES)
}

/// The hash of the bytes stored for `extraction`: the key a document records.
pub fn hash(extraction: &Extraction) -> Result<String> {
    let form = stored_form(extraction)?;
    data::hash_encoded_lazy_lines(form.lazy(extraction), LINES)
}

/// The extraction a stored one read as JSON gives, with its document and
/// original in place.
fn expanded(mut extraction: Value) -> Result<Extraction> {
    expand(&mut extraction)?;
    Extraction::from_value(extraction)
}

/// Reads and validates a stored extraction, with the original it was made
/// from as the `document.yml` beside it records.
pub fn read(path: &Path) -> Result<Extraction> {
    let mut extraction = read_text(path, &read_file(path)?)?;
    attach_source(path, &mut extraction)?;
    expanded(extraction)
}

/// [`read`] for the record of a document, whose hash is the key the document
/// records: it must be in the stored encoding of this version. Its cells are
/// made typed as they are read, as [`read_known_record`] makes them: read as
/// JSON first, a large extraction took several times their memory. Its
/// contract is checked a part at a time on the stored form they give, and its
/// bytes against that form as it is encoded.
pub fn read_record(path: &Path) -> Result<Extraction> {
    let text = read_file(path)?;
    let extraction = typed_record(path, &text)?;
    let form = stored_form(&extraction)?;
    form.check(&extraction)?;
    ensure!(
        data::is_encoded_lazy_lines(form.lazy(&extraction), LINES, text.as_bytes())?,
        "{} is not stored one cell per line, the format of this version; re-import the original with documents import --force",
        path.display()
    );
    Ok(extraction)
}

/// [`read_record`] for a stored extraction whose bytes an inspection of this
/// program found valid: its contract and form are not checked again, and its
/// cells are made typed as they are read rather than read as JSON first,
/// which took several times the memory of the typed cells.
pub fn read_known_record(path: &Path) -> Result<Extraction> {
    typed_record(path, &read_file(path)?)
}

/// What a sheet stored before font tables lacks, as a message of [`known`].
const FONTS_IN_CELLS: &str = "keeps fonts in every cell, the format before font tables; re-import the original with documents import --force";

/// The stored extraction `text` of the record at `path`, its cells typed as
/// they are read, with its document and original in place.
fn typed_record(path: &Path, text: &str) -> Result<Extraction> {
    let mut deserializer = serde_json::Deserializer::from_str(text);
    let (mut fields, mut sheets) =
        serde::Deserializer::deserialize_map(&mut deserializer, known::Stored).map_err(
            |error| {
                if error.to_string().starts_with(FONTS_IN_CELLS) {
                    anyhow::anyhow!("{} {FONTS_IN_CELLS}", path.display())
                } else {
                    error.into()
                }
            },
        )?;
    deserializer.end()?;
    ensure!(
        fields.get(SOURCE).is_none() && fields.get(DOCUMENT).is_none(),
        "{} records its document or original, which document.yml records in this version; re-import the original with documents import --force",
        path.display()
    );
    // Expanded as a stored extraction read as JSON is, with its sheets
    // without their cells.
    fields["sheets"] = Value::Array(
        sheets
            .iter_mut()
            .map(|sheet| std::mem::take(&mut sheet.fields))
            .collect(),
    );
    attach_source(path, &mut fields)?;
    expand(&mut fields)?;
    let Some(Value::Array(members)) = fields
        .as_object_mut()
        .context("extraction must be an object")?
        .remove("sheets")
    else {
        bail!("extraction sheets missing")
    };
    for (sheet, members) in sheets.iter_mut().zip(members) {
        sheet.fields = members;
    }
    Ok(Extraction { fields, sheets })
}

/// Reads a stored extraction, whose fonts and styles are in tables, with
/// its cells typed as each is read (see [`read_known_record`]).
mod known {
    use super::*;
    use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};

    /// The members of the extraction besides its sheets, and its sheets.
    pub(super) struct Stored;

    impl<'de> Visitor<'de> for Stored {
        type Value = (Value, Vec<Sheet>);
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a stored extraction")
        }
        fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
            let mut fields = Map::new();
            let mut sheets = None;
            while let Some(key) = access.next_key::<String>()? {
                if key == "sheets" {
                    sheets = Some(access.next_value_seed(Sheets)?);
                } else {
                    fields.insert(key, access.next_value()?);
                }
            }
            let sheets = sheets.ok_or_else(|| de::Error::custom("extraction sheets missing"))?;
            Ok((Value::Object(fields), sheets))
        }
    }

    struct Sheets;

    impl<'de> DeserializeSeed<'de> for Sheets {
        type Value = Vec<Sheet>;
        fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
            d.deserialize_seq(self)
        }
    }

    impl<'de> Visitor<'de> for Sheets {
        type Value = Vec<Sheet>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("the sheets of a stored extraction")
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
            let mut sheets = vec![];
            while let Some(sheet) = access.next_element_seed(SheetSeed(sheets.len()))? {
                sheets.push(sheet);
            }
            Ok(sheets)
        }
    }

    /// The sheet numbered `.0` from 0.
    struct SheetSeed(usize);

    impl<'de> DeserializeSeed<'de> for SheetSeed {
        type Value = Sheet;
        fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
            d.deserialize_map(self)
        }
    }

    impl<'de> Visitor<'de> for SheetSeed {
        type Value = Sheet;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a sheet of a stored extraction")
        }
        fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
            let mut fields = Map::new();
            let mut cells = None;
            while let Some(key) = access.next_key::<String>()? {
                if key == "cells" {
                    cells = Some(access.next_value_seed(Cells(self.0))?);
                } else {
                    fields.insert(key, access.next_value()?);
                }
            }
            let cells = cells.ok_or_else(|| de::Error::custom("extraction cells missing"))?;
            if !fields.contains_key("fonts") {
                return Err(de::Error::custom(FONTS_IN_CELLS));
            }
            let mut list = |key: &str| match fields.remove(key) {
                Some(Value::Array(list)) => Ok(list),
                _ => Err(de::Error::custom(format!("sheet {key} must be a list"))),
            };
            let (fonts, styles) = (list("fonts")?, list("styles")?);
            Ok(Sheet {
                fields: Value::Object(fields),
                cells,
                fonts,
                styles,
            })
        }
    }

    /// The cells of the sheet numbered `.0` from 0.
    struct Cells(usize);

    impl<'de> DeserializeSeed<'de> for Cells {
        type Value = Vec<Cell>;
        fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
            d.deserialize_seq(self)
        }
    }

    impl<'de> Visitor<'de> for Cells {
        type Value = Vec<Cell>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("the cells of a sheet")
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
            let mut formats = HashMap::new();
            // A stored sheet keeps its fonts and styles in its tables.
            let (mut fonts, mut styles) = (Table::default(), Table::default());
            let mut cells = Vec::with_capacity(access.size_hint().unwrap_or(0));
            while let Some(cell) = access.next_element::<Value>()? {
                cells.push(
                    typed_cell(cell, self.0, true, &mut formats, &mut fonts, &mut styles)
                        .map_err(|error| de::Error::custom(format!("{error:#}")))?,
                );
            }
            Ok(cells)
        }
    }
}

/// [`read`] for the `text` of a stored extraction and the `document.yml`
/// (`meta`) it was stored with, such as an earlier version of both.
pub fn parse(text: &str, meta: &Value) -> Result<Extraction> {
    let mut extraction = read_text(Path::new("extraction.json"), text)?;
    attach(&mut extraction, meta);
    expanded(extraction)
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
            .is_none_or(|sheets| sheets.iter().all(|sheet| sheet.get("fonts").is_some())),
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
        let mut extraction = extraction;
        // A cell with a font, a style and a place in a text original.
        let mut placed = cell("text-2", "A2", "number", json!(4.5), Value::Null, "0.0");
        placed["font"] = json!({"latin":"Calibri","east_asian":"Calibri","size":11,"color":"auto"});
        placed["style"] = json!({"bold":true,"fill":0,"border":1,"strike":false});
        placed["position"] = json!({"kind":"paragraph","headings":["1. 概要"],"line_start":3,
            "line_end":3,"byte_start":10,"byte_end":20,"columns":[]});
        extraction["sheets"][1]["cells"]
            .as_array_mut()
            .unwrap()
            .push(placed);
        let extraction = Extraction::from_value(extraction).unwrap();
        let bytes = stored(&extraction).unwrap();
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
        assert_eq!(read_record(&path).unwrap(), extraction);
        assert_eq!(read_known_record(&path).unwrap(), extraction);
        let stored_back = read_record(&path).unwrap();
        assert_eq!(
            view_lazy(&stored_back).into_value().unwrap(),
            view(&stored_back).unwrap()
        );
        check_view(&stored_back, |part| data::validate("extraction-view", part)).unwrap();
        data::validate("extraction-view", &view(&stored_back).unwrap()).unwrap();
        assert_eq!(
            view(&read(&path).unwrap()).unwrap(),
            view(&extraction).unwrap()
        );
    }
}
