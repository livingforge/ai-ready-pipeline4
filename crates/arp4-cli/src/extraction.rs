//! The form of an extraction, stored and in memory. An [`Extraction`] keeps
//! the cells of each [`Sheet`] typed ([`Cell`]), as a document has far more
//! cells than any other fact, and the rest of it as JSON. Each sheet keeps
//! its fonts and styles in tables that its cells refer to by index, and a
//! cell keeps no cached formula result, which is its value. A cell's font and
//! style are read through [`font`] and [`style`]; a cell is hashed or compared
//! as [`cell`] gives it, with its font, style and cache in place, so that cell
//! hashes do not depend on how the extraction stores them. [`write_reading`]
//! writes the extraction for reading, as `documents extraction-read` does: as
//! it is stored, with each font and style written out in place of its index.
//!
//! A sheet an extraction is built from, such as one of the sheets of an
//! original or a slide a slide operation adds, holds its fonts and styles in
//! its cells: [`Sheet::from_value`] puts them in tables.
//!
//! The stored encoding keeps each cell, drawing, font, style, print setting,
//! table and grid run on one line ([`LINES`]), and the merges of a sheet
//! together on one, so that a changed cell or a moved shape is one changed
//! line of a Git diff.
//!
//! The document an extraction belongs to (`document_id`) and the original it
//! was made from (`source`, its path and hash) are recorded once, in the
//! `document.yml` beside it: the stored extraction and its hash leave them
//! out, so an original saved again without a change keeps its extraction. A
//! stored extraction also leaves out what follows from the rest
//! ([`stored_form`]): no assets, and the empty lists of an Excel sheet; the
//! text its note (finding R001) begins with, which the format of the original
//! fixes, and the note itself when nothing is added to that text; the print
//! settings once for the parts that have the same ones, without the element
//! name each begins with; in a drawing, the part its ID names and the members
//! without a value; in a cell, no formula, the type its value and formula
//! give, the number format and ID its original gives a cell by default, and
//! the font and style most cells of its sheet have, which the sheet names
//! once ([`Defaults`]). Reading a stored extraction puts all of it back in
//! place.
use crate::data::{self, array};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::ops::{Index, IndexMut};
use std::path::Path;

mod cells;
use cells::CellsBuilder;
pub use cells::{Cell, CellNumber, CellValue, Cells, NewCell};

/// The members whose items the stored encoding writes one per line: of a
/// sheet, and the runs of its grid (`columns`, `rows`).
const LINES: &[&str] = &[
    "cells",
    "drawings",
    "fonts",
    "styles",
    "tables",
    "columns",
    "rows",
    "print_settings",
];

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

    /// The type a cell with `formula` and `value` has unless it names
    /// another, as an error does: a formula's, or that of the value.
    fn of(formula: Option<&str>, value: &Value) -> Result<Self> {
        Ok(match (formula, value) {
            (Some(_), _) => Self::Formula,
            (None, Value::String(_)) => Self::String,
            (None, Value::Number(_)) => Self::Number,
            (None, Value::Bool(_)) => Self::Boolean,
            (None, Value::Null) => Self::Null,
            (None, _) => bail!("cell value must be a scalar"),
        })
    }
}

impl Cell<'_> {
    /// The cell as JSON, with its font and style as indexes.
    pub fn to_value(&self) -> Value {
        let mut value = json!({"id":self.id(),"address":self.address(),"type":self.kind().as_str(),
            "formula":self.formula(),"number_format":self.number_format()});
        value["value"] = self.value().to_json();
        if let Some(font) = self.font() {
            value["font"] = json!(font);
        }
        if let Some(style) = self.style() {
            value["style"] = json!(style);
        }
        if let Some(position) = self.position() {
            value["position"] = position.clone();
        }
        value
    }
}

/// A sheet of an extraction: its cells, the fonts and styles they refer to,
/// and its other members (`fields`), such as its name, merges and drawings.
#[derive(Clone, Debug, PartialEq)]
pub struct Sheet {
    pub fields: Value,
    pub cells: Cells,
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
    /// A copy of the sheet whose cells are named for the page `page` of an
    /// Excel workbook (`c-<page>-<address>`, the page without `sheet-`) and
    /// hold the formulas `formula` makes of theirs.
    fn copied(&self, page: &str, formula: &dyn Fn(&str) -> Result<String>) -> Result<Self>;
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
    fn copied(&self, page: &str, formula: &dyn Fn(&str) -> Result<String>) -> Result<Self> {
        let mut copy = self.clone();
        for cell in copy["cells"].as_array_mut().into_iter().flatten() {
            let address = cell["address"].as_str().context("cell without address")?;
            cell["id"] = json!(excel_page_cell_id(page, address));
            if let Some(text) = cell["formula"].as_str() {
                cell["formula"] = json!(formula(text)?);
            }
        }
        Ok(copy)
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
    fn copied(&self, page: &str, formula: &dyn Fn(&str) -> Result<String>) -> Result<Self> {
        let mut builder = CellsBuilder::default();
        for cell in self.cells.iter() {
            let id = excel_page_cell_id(page, cell.address());
            let value = cell.value().to_json();
            let text = cell.formula().map(formula).transpose()?;
            builder.push(NewCell {
                id: &id,
                address: cell.address(),
                kind: cell.kind(),
                value: &value,
                formula: text.as_deref(),
                number_format: cell.number_format(),
                font: cell.font(),
                style: cell.style(),
                position: cell.position().cloned(),
            })?;
        }
        Ok(Sheet {
            fields: self.fields.clone(),
            cells: builder.finish(),
            fonts: self.fonts.clone(),
            styles: self.styles.clone(),
        })
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
fn text<'a>(cell: &'a Map<String, Value>, key: &str) -> Result<&'a str> {
    cell.get(key)
        .and_then(Value::as_str)
        .with_context(|| format!("cell {key} must be a string"))
}

/// The index into a font or style table that a cell given as JSON holds.
fn table_index(value: &Value) -> Result<usize> {
    value
        .as_u64()
        .and_then(|index| usize::try_from(index).ok())
        .context("table index must be a number")
}

/// The number format and ID a cell of the `sheet`th sheet takes when it names
/// none, as the format of its original gives them: in an Excel workbook
/// (`excel`) the General format and the ID [`excel_cell_id`] gives, in other
/// originals no format and the ID `text-<n>` of the `n`th cell of the page.
/// A stored cell naming no font or style takes the `font` and `style` of its
/// sheet's table that the sheet names ([`CELL_FONT`], [`CELL_STYLE`]).
#[derive(Clone, Copy)]
struct Defaults {
    sheet: usize,
    excel: bool,
    font: Option<usize>,
    style: Option<usize>,
}

/// The members of a stored sheet naming the font and the style of its tables
/// that its cells naming none have: the ones most of its cells have.
const CELL_FONT: &str = "cell_font";
const CELL_STYLE: &str = "cell_style";

impl Defaults {
    /// The defaults of the `sheet`th sheet of the extraction whose members
    /// besides its sheets are `fields`, with no font or style.
    fn of(fields: &Value, sheet: usize) -> Self {
        Self {
            sheet,
            excel: excel(fields),
            font: None,
            style: None,
        }
    }

    /// These defaults with the font and style that the members `fields` of a
    /// stored sheet name.
    fn with_formats(self, fields: &Map<String, Value>) -> Result<Self> {
        let index = |key: &str| fields.get(key).map(table_index).transpose();
        Ok(Self {
            font: index(CELL_FONT)?,
            style: index(CELL_STYLE)?,
            ..self
        })
    }

    /// The ID of the cell at `address`, the `position`th of its sheet.
    fn id(self, address: &str, position: usize) -> String {
        if self.excel {
            excel_cell_id(self.sheet, address)
        } else {
            format!("text-{}", position + 1)
        }
    }

    fn number_format(self) -> &'static str {
        if self.excel { GENERAL } else { "" }
    }
}

impl Sheet {
    /// The `index`th sheet of an extraction given as JSON: with its fonts and
    /// styles in tables or, as a sheet an extraction is built from, in its
    /// cells with their cached results. A cell without a formula, type,
    /// number format or ID has none, the type of its value, the General
    /// format and the ID an Excel cell takes from its sheet and address.
    pub fn from_value(sheet: Value, index: usize) -> Result<Self> {
        Self::with_defaults(
            sheet,
            Defaults {
                sheet: index,
                excel: true,
                font: None,
                style: None,
            },
        )
    }

    /// [`Sheet::from_value`] with the number format and ID of a cell naming
    /// none from `defaults`.
    fn with_defaults(sheet: Value, defaults: Defaults) -> Result<Self> {
        let Value::Object(mut fields) = sheet else {
            bail!("sheet must be an object")
        };
        let Some(Value::Array(cells)) = fields.remove("cells") else {
            bail!("extraction cells missing")
        };
        let tabled = fields.contains_key("fonts");
        let mut fonts = Table::default();
        let mut styles = Table::default();
        let mut defaults = defaults;
        if tabled {
            for (key, table) in [("fonts", &mut fonts), ("styles", &mut styles)] {
                match fields.remove(key) {
                    Some(Value::Array(list)) => table.list = list,
                    _ => bail!("sheet {key} must be a list"),
                }
            }
            defaults = defaults.with_formats(&fields)?;
            fields.remove(CELL_FONT);
            fields.remove(CELL_STYLE);
        }
        let mut built = CellsBuilder::default();
        for cell in cells {
            typed_cell(cell, defaults, tabled, &mut built, &mut fonts, &mut styles)?;
        }
        Ok(Self {
            fields: Value::Object(fields),
            cells: built.finish(),
            fonts: fonts.list,
            styles: styles.list,
        })
    }

    /// A sheet with `fields` and no cells yet, to [`SheetBuilder::push`] them
    /// on.
    pub fn build(fields: Value) -> SheetBuilder {
        SheetBuilder {
            fields,
            cells: CellsBuilder::default(),
            fonts: Table::default(),
            styles: Table::default(),
        }
    }

    /// The sheet as JSON, with its fonts and styles in tables.
    pub fn to_value(&self) -> Value {
        let mut sheet = self.fields.clone();
        sheet["cells"] = Value::Array(self.cells.iter().map(|c| c.to_value()).collect());
        sheet["fonts"] = Value::Array(self.fonts.clone());
        sheet["styles"] = Value::Array(self.styles.clone());
        sheet
    }
}

/// Adds to `built` the cell `cell`, given as JSON as [`Sheet::from_value`]
/// reads it, with what it leaves out from `defaults`: with indexes into its
/// sheet's font and style tables when `tabled`, and otherwise with its font
/// and style, which go into `fonts` and `styles`.
fn typed_cell(
    mut cell: Value,
    defaults: Defaults,
    tabled: bool,
    built: &mut CellsBuilder,
    fonts: &mut Table,
    styles: &mut Table,
) -> Result<()> {
    let Value::Object(cell) = &mut cell else {
        bail!("cell must be an object")
    };
    let position = cell.remove("position");
    let address = text(cell, "address")?;
    let derived;
    let id = match cell.get("id") {
        None => {
            derived = defaults.id(address, built.count());
            derived.as_str()
        }
        Some(_) => text(cell, "id")?,
    };
    let value = cell.get("value").context("cell value missing")?;
    let formula = match cell.get("formula") {
        None | Some(Value::Null) => None,
        Some(Value::String(formula)) => Some(formula.as_str()),
        Some(_) => bail!("cell formula must be a string"),
    };
    let kind = match cell.get("type") {
        None => CellKind::of(formula, value)?,
        Some(_) => CellKind::parse(text(cell, "type")?)?,
    };
    let number_format = match cell.get("number_format") {
        None => defaults.number_format(),
        Some(Value::String(format)) => format.as_str(),
        Some(_) => bail!("cell number format must be a string"),
    };
    let (font, style) = (cell.get("font"), cell.get("style"));
    let (font, style) = if tabled {
        // A cell naming none has its sheet's, and one naming null has none.
        let index = |member: Option<&Value>, default: Option<usize>| match member {
            None => Ok(default),
            Some(Value::Null) => Ok(None),
            Some(index) => table_index(index).map(Some),
        };
        (index(font, defaults.font)?, index(style, defaults.style)?)
    } else {
        // Dropped only when expanding gives it back exactly.
        let cached = cell.get("cached").unwrap_or(&Value::Null);
        let expected = if kind == CellKind::Formula {
            value
        } else {
            &Value::Null
        };
        ensure!(
            cached == expected,
            "cached result of {address} is not its value"
        );
        (
            font.map(|font| fonts.add(font.clone())),
            style.map(|style| styles.add(style.clone())),
        )
    };
    let known = |key: &str| {
        matches!(
            key,
            "id" | "address" | "type" | "value" | "formula" | "number_format" | "font" | "style"
        ) || (!tabled && key == "cached")
    };
    if let Some(key) = cell.keys().find(|key| !known(key)) {
        bail!("unknown member {key} of cell {address}");
    }
    built.push(NewCell {
        id,
        address,
        kind,
        value,
        formula,
        number_format,
        font,
        style,
        position,
    })
}

/// A sheet being built cell by cell from what an original holds, with its
/// fonts and styles put in tables in order of first use, as
/// [`Sheet::from_value`] puts those of a sheet given as JSON.
pub struct SheetBuilder {
    fields: Value,
    cells: CellsBuilder,
    fonts: Table,
    styles: Table,
}

impl SheetBuilder {
    /// Adds `cell` with its font and style, which go into the tables of the
    /// sheet in place of the indexes `cell` gives.
    pub fn push(
        &mut self,
        cell: NewCell<'_>,
        font: Option<Value>,
        style: Option<Value>,
    ) -> Result<()> {
        self.cells.push(NewCell {
            font: font.map(|font| self.fonts.add(font)),
            style: style.map(|style| self.styles.add(style)),
            ..cell
        })
    }

    pub fn finish(self) -> Sheet {
        Sheet {
            fields: self.fields,
            cells: self.cells.finish(),
            fonts: self.fonts.list,
            styles: self.styles.list,
        }
    }
}

impl Extraction {
    /// The extraction given as JSON, each sheet as [`Sheet::from_value`]
    /// reads it.
    pub fn from_value(extraction: Value) -> Result<Self> {
        Self::with_defaults(extraction, |_, sheet| Defaults {
            sheet,
            excel: true,
            font: None,
            style: None,
        })
    }

    /// The extraction stored as JSON, whose cells take what they leave out
    /// from the format of its original ([`Defaults::of`]).
    fn from_stored(extraction: Value) -> Result<Self> {
        Self::with_defaults(extraction, Defaults::of)
    }

    fn with_defaults(
        extraction: Value,
        defaults: impl Fn(&Value, usize) -> Defaults,
    ) -> Result<Self> {
        let Value::Object(mut fields) = extraction else {
            bail!("extraction must be an object")
        };
        let sheets = fields.remove("sheets");
        let fields = Value::Object(fields);
        let sheets = match sheets {
            Some(Value::Array(sheets)) => sheets
                .into_iter()
                .enumerate()
                .map(|(index, sheet)| Sheet::with_defaults(sheet, defaults(&fields, index)))
                .collect::<Result<_>>()?,
            _ => bail!("extraction sheets missing"),
        };
        Ok(Self { fields, sheets })
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
pub fn font<'a>(sheet: &'a Sheet, cell: Cell<'_>) -> Result<&'a Value> {
    lookup(&sheet.fonts, cell.font(), "font")
}

/// The style of a cell of `sheet`, null for none.
pub fn style<'a>(sheet: &'a Sheet, cell: Cell<'_>) -> Result<&'a Value> {
    lookup(&sheet.styles, cell.style(), "style")
}

/// A cell of `sheet` as JSON with its font, style and cached result in place.
pub fn cell(sheet: &Sheet, cell: Cell<'_>) -> Result<Value> {
    let mut expanded = cell.to_value();
    if cell.font().is_some() {
        expanded["font"] = font(sheet, cell)?.clone();
    }
    if cell.style().is_some() {
        expanded["style"] = style(sheet, cell)?.clone();
    }
    expanded["cached"] = cell.cached().to_json();
    Ok(expanded)
}

/// The cells of a sheet [`StoredForm::check`] checks together.
const VIEW_PART: usize = 4096;

/// The cells in parts of at most [`VIEW_PART`], one empty part for none.
fn parts(cells: &Cells) -> impl Iterator<Item = cells::Iter<'_>> {
    (0..cells.len().div_ceil(VIEW_PART).max(1))
        .map(move |part| cells.range(part * VIEW_PART, (part + 1) * VIEW_PART))
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

/// The ID of the cell at `address` of a sheet the operation `page` adds.
fn excel_page_cell_id(page: &str, address: &str) -> String {
    format!("c-{page}-{address}")
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

/// The print settings of an extraction, each Open XML element with its part
/// and its name (`component`), the parts in order of their names.
const PRINT_SETTINGS: &str = "print_settings";

/// The name of the element the Open XML fragment `xml` begins with, without
/// its prefix: the `component` of a print setting.
fn element_name(xml: &str) -> Option<&str> {
    let name = xml.strip_prefix('<')?;
    let name = &name[..name.find([' ', '/', '>']).unwrap_or(name.len())];
    let name = name.rsplit_once(':').map_or(name, |(_, local)| local);
    (!name.is_empty()).then_some(name)
}

/// The print `settings` as stored: the parts that have the same elements in
/// the same order as one group of their names (`parts`) and the XML of those
/// elements (`xml`), the groups in order of their first part.
fn stored_print_settings(settings: &Value) -> Result<Value> {
    let settings = array(settings)?;
    // Each part, in order, with the XML of its elements.
    let mut parts: Vec<(&Value, Vec<&Value>)> = vec![];
    for setting in settings {
        let (part, xml) = (&setting["part"], &setting["xml"]);
        match parts.last_mut() {
            Some((last, elements)) if *last == part => elements.push(xml),
            _ => parts.push((part, vec![xml])),
        }
    }
    let mut groups: Vec<(Vec<&Value>, Vec<&Value>)> = vec![];
    for (part, elements) in parts {
        match groups.iter_mut().find(|(_, other)| *other == elements) {
            Some((names, _)) => names.push(part),
            None => groups.push((vec![part], elements)),
        }
    }
    let stored = Value::Array(
        groups
            .into_iter()
            .map(|(parts, xml)| json!({"parts":parts,"xml":xml}))
            .collect(),
    );
    ensure!(
        expanded_print_settings(&stored)? == *settings,
        "print settings are not each part's elements together, the parts in order of their names, each setting named by its element"
    );
    Ok(stored)
}

/// The print settings [`stored_print_settings`] gives back: each part in
/// order of its name with its elements in the order stored.
fn expanded_print_settings(groups: &Value) -> Result<Vec<Value>> {
    let mut parts: Vec<(&str, &Value)> = vec![];
    for group in array(groups)? {
        for part in array(&group["parts"])? {
            parts.push((data::string(part)?, &group["xml"]));
        }
    }
    parts.sort_by_key(|(part, _)| *part);
    let mut settings = vec![];
    for (part, elements) in parts {
        for xml in array(elements)? {
            let xml = data::string(xml)?;
            let component = element_name(xml)
                .with_context(|| format!("a print setting of {part} is not an element"))?;
            settings.push(json!({"part":part,"component":component,"xml":xml}));
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
const SHEET_LISTS: [&str; 9] = [
    "hidden_rows",
    "hidden_columns",
    "drawings",
    "tables",
    "comments",
    "links",
    "validations",
    "conditional_formats",
    "computed",
];

/// Whether the extraction with `fields` was read from an Excel workbook,
/// whose sheets have every list of [`SHEET_LISTS`].
fn excel(fields: &Value) -> bool {
    fields["parser"]
        .as_str()
        .is_some_and(|parser| parser.contains(";cells/"))
}

/// A cell as it is stored: without a null formula, the type its value and
/// formula give ([`CellKind::of`]), or the number format, ID, font and style
/// `defaults` give; with a null font or style when it has none of the one
/// `defaults` give.
fn stored_cell(cell: Cell<'_>, defaults: Defaults) -> Value {
    let value = cell.value().to_json();
    let mut stored = json!({"address":cell.address()});
    if CellKind::of(cell.formula(), &value).ok() != Some(cell.kind()) {
        stored["type"] = json!(cell.kind().as_str());
    }
    stored["value"] = value;
    if let Some(formula) = cell.formula() {
        stored["formula"] = json!(formula);
    }
    if cell.number_format() != defaults.number_format() {
        stored["number_format"] = json!(cell.number_format());
    }
    if cell.id() != defaults.id(cell.address(), cell.index()) {
        stored["id"] = json!(cell.id());
    }
    for (key, index, default) in [
        ("font", cell.font(), defaults.font),
        ("style", cell.style(), defaults.style),
    ] {
        if index != default {
            stored[key] = json!(index);
        }
    }
    if let Some(position) = cell.position() {
        stored["position"] = position.clone();
    }
    stored
}

/// A cell as [`write_reading`] writes it: as [`stored_cell`] gives it, with
/// the font and style it names written out in place of their indexes.
fn reading_cell(sheet: &Sheet, cell: Cell<'_>, defaults: Defaults) -> Result<Value> {
    let mut reading = stored_cell(cell, defaults);
    for (key, table) in [("font", &sheet.fonts), ("style", &sheet.styles)] {
        if let Some(index) = reading.get(key).and_then(Value::as_u64) {
            reading[key] = lookup(table, Some(index as usize), key)?.clone();
        }
    }
    Ok(reading)
}

/// The font and style most of the cells of `sheet` have, none when most
/// have none: when as many have one as another, the earlier in its table.
fn most_used(sheet: &Sheet) -> (Option<usize>, Option<usize>) {
    let most = |of: &dyn Fn(Cell<'_>) -> Option<usize>| {
        let mut counts: HashMap<Option<usize>, usize> = HashMap::new();
        for c in sheet.cells.iter() {
            *counts.entry(of(c)).or_default() += 1;
        }
        counts
            .into_iter()
            .max_by_key(|&(index, count)| (count, std::cmp::Reverse(index)))
            .and_then(|(index, _)| index)
    };
    (most(&|c| c.font()), most(&|c| c.style()))
}

/// The merges of a sheet, which the line encodings write on one line.
const MERGES: &str = "merges";

/// The members of the sheet `fields`, its merges on one line.
fn sheet_members(fields: &Value) -> Vec<(&str, data::Lazy<'_>)> {
    let mut members = data::Lazy::members(fields, &[MERGES]);
    if let Some(merges) = fields.get(MERGES) {
        members.push((MERGES, data::Lazy::Line(merges.clone())));
    }
    members
}

/// The extraction as it is stored: without its `document_id` and `source`,
/// empty assets or empty lists of an Excel sheet; with its findings as
/// [`stored_findings`] and its print settings as [`stored_print_settings`]
/// give them; each drawing as [`stored_drawing`] gives it, and each cell as
/// [`stored_cell`] gives it, with what its sheet's cells take when they name
/// none ([`Defaults`]).
struct StoredForm {
    /// The members of the extraction besides its sheets.
    top: Map<String, Value>,
    /// The members of each sheet besides its cells, fonts and styles.
    sheets: Vec<Value>,
    defaults: Vec<Defaults>,
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
                        let defaults = self.defaults[index];
                        let mut members = sheet_members(fields);
                        members.push((
                            "cells",
                            Lazy::Items(Box::new(
                                sheet
                                    .cells
                                    .iter()
                                    .map(move |c| Ok(Lazy::Value(stored_cell(c, defaults)))),
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
            let defaults = self.defaults[index];
            let mut copy = fields.clone();
            copy["fonts"] = Value::Array(sheet.fonts.clone());
            copy["styles"] = Value::Array(sheet.styles.clone());
            for chunk in parts(&sheet.cells) {
                copy["cells"] = chunk.map(|c| stored_cell(c, defaults)).collect();
                part["sheets"] = Value::Array(vec![copy]);
                data::validate("extraction", &part)?;
                copy = part["sheets"][0].take();
            }
        }
        Ok(())
    }

    /// The members of the reading form of `extraction` besides its sheets:
    /// those stored, with its document, its original, its findings in full
    /// and its assets.
    fn reading_top(&self, extraction: &Extraction) -> Value {
        let mut top = self.top.clone();
        for key in [DOCUMENT, SOURCE, FINDINGS, ASSETS] {
            if let Some(value) = extraction.fields.get(key) {
                top.insert(key.into(), value.clone());
            }
        }
        Value::Object(top)
    }

    /// The members of the `index`th sheet of the reading form besides its
    /// cells: those stored, with the font and style its cells naming none
    /// have written out in place of their indexes.
    fn reading_sheet(&self, extraction: &Extraction, index: usize) -> Result<Value> {
        let (sheet, defaults) = (&extraction.sheets[index], self.defaults[index]);
        let mut fields = self.sheets[index].clone();
        for (key, table, default) in [
            (CELL_FONT, &sheet.fonts, defaults.font),
            (CELL_STYLE, &sheet.styles, defaults.style),
        ] {
            if default.is_some() {
                fields[key] = lookup(table, default, key)?.clone();
            }
        }
        Ok(fields)
    }

    /// The reading form of `extraction` from its members `top` and its
    /// sheets' `sheets`, made a cell at a time.
    fn reading<'a>(
        &'a self,
        extraction: &'a Extraction,
        top: &'a Value,
        sheets: &'a [Value],
    ) -> data::Lazy<'a> {
        use data::Lazy;
        let mut members = Lazy::members(top, &[]);
        members.push((
            "sheets",
            Lazy::Items(Box::new(
                extraction.sheets.iter().zip(sheets).enumerate().map(
                    move |(index, (sheet, fields))| {
                        let defaults = self.defaults[index];
                        let mut members = sheet_members(fields);
                        members.push((
                            "cells",
                            Lazy::Items(Box::new(
                                sheet.cells.iter().map(move |c| {
                                    Ok(Lazy::Value(reading_cell(sheet, c, defaults)?))
                                }),
                            )),
                        ));
                        Ok(Lazy::Object(members))
                    },
                ),
            )),
        ));
        Lazy::Object(members)
    }

    /// Checks the reading form of `extraction`, of `top` and `sheets`, against
    /// the extraction-view contract a part at a time, as [`StoredForm::check`]
    /// checks the stored form.
    fn check_reading(&self, extraction: &Extraction, top: &Value, sheets: &[Value]) -> Result<()> {
        let mut part = top.clone();
        part["sheets"] = json!([]);
        data::validate("extraction-view", &part)?;
        for (index, (sheet, fields)) in extraction.sheets.iter().zip(sheets).enumerate() {
            let defaults = self.defaults[index];
            let mut copy = fields.clone();
            for chunk in parts(&sheet.cells) {
                copy["cells"] = chunk
                    .map(|c| reading_cell(sheet, c, defaults))
                    .collect::<Result<_>>()?;
                part["sheets"] = Value::Array(vec![copy]);
                data::validate("extraction-view", &part)?;
                copy = part["sheets"][0].take();
            }
        }
        Ok(())
    }
}

/// The members of the reading form of `extraction` besides the cells of its
/// sheets, and its stored form, which gives them.
fn reading_form(extraction: &Extraction) -> Result<(StoredForm, Value, Vec<Value>)> {
    let form = stored_form(extraction)?;
    let top = form.reading_top(extraction);
    let sheets = (0..extraction.sheets.len())
        .map(|index| form.reading_sheet(extraction, index))
        .collect::<Result<_>>()?;
    Ok((form, top, sheets))
}

/// Writes `extraction` to `out` for reading, as `documents extraction-read`
/// writes it (the extraction-view contract): as it is stored, with its
/// document, original, findings and assets, and each font and style written
/// out in place of its index, the ones most cells of a sheet have once for
/// the sheet (`cell_font`, `cell_style`). Checked a part at a time and made
/// as it is written.
pub fn write_reading(extraction: &Extraction, out: &Path) -> Result<()> {
    let (form, top, sheets) = reading_form(extraction)?;
    form.check_reading(extraction, &top, &sheets)?;
    data::write_lazy_lines(out, form.reading(extraction, &top, &sheets), LINES)
}

/// The reading form [`write_reading`] writes of `extraction`, as JSON.
pub fn reading(extraction: &Extraction) -> Result<Value> {
    let (form, top, sheets) = reading_form(extraction)?;
    form.reading(extraction, &top, &sheets).into_value()
}

/// The extraction its reading form `reading` gives back: each cell's font
/// and style in place, from its sheet when the cell names none, and each
/// formula's cached result, its value.
pub fn from_reading(mut reading: Value) -> Result<Extraction> {
    expand_members(&mut reading)?;
    let sheets = reading["sheets"]
        .as_array_mut()
        .context("extraction sheets missing")?;
    for sheet in sheets {
        let sheet = sheet.as_object_mut().context("sheet must be an object")?;
        let defaults = [
            ("font", sheet.remove(CELL_FONT)),
            ("style", sheet.remove(CELL_STYLE)),
        ];
        let cells = sheet
            .get_mut("cells")
            .and_then(Value::as_array_mut)
            .context("extraction cells missing")?;
        for cell in cells {
            let cell = cell.as_object_mut().context("cell must be an object")?;
            for (key, default) in &defaults {
                match cell.get(*key) {
                    Some(Value::Null) => {
                        cell.remove(*key);
                    }
                    Some(_) => {}
                    None => {
                        if let Some(default) = default {
                            cell.insert((*key).into(), default.clone());
                        }
                    }
                }
            }
            if cell.get("formula").is_some_and(Value::is_string) {
                let value = cell.get("value").cloned().unwrap_or(Value::Null);
                cell.insert("cached".into(), value);
            }
        }
    }
    Extraction::from_stored(reading)
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
    let mut defaults = vec![];
    for (index, sheet) in extraction.sheets.iter().enumerate() {
        let mut stored = sheet.fields.clone();
        let members = stored.as_object_mut().context("sheet must be an object")?;
        let (font, style) = most_used(sheet);
        for (key, default) in [(CELL_FONT, font), (CELL_STYLE, style)] {
            if let Some(default) = default {
                members.insert(key.into(), json!(default));
            }
        }
        defaults.push(Defaults {
            font,
            style,
            ..Defaults::of(fields, index)
        });
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
    Ok(StoredForm {
        top: form,
        sheets,
        defaults,
    })
}

/// Puts back in place what [`stored_form`] leaves out but the document, the
/// original and what it leaves out of each cell, which [`Defaults`] and
/// [`CellKind::of`] give back: the empty assets and lists, the findings and print settings,
/// and what it leaves out of each drawing. The original, which the note of
/// the findings depends on, is in place already.
fn expand(extraction: &mut Value) -> Result<()> {
    expand_findings(extraction)?;
    expand_members(extraction)
}

/// Puts back what [`expand`] puts back but the findings, which the reading
/// form keeps in full.
fn expand_members(extraction: &mut Value) -> Result<()> {
    if extraction.get(ASSETS).is_none() {
        extraction[ASSETS] = json!([]);
    }
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
    Extraction::from_stored(extraction)
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
                    // Stored in order of their names, the members read by
                    // now name the parser, which the cells take what they
                    // leave out from.
                    let fields = Value::Object(fields.clone());
                    sheets = Some(access.next_value_seed(Sheets(&fields))?);
                } else {
                    fields.insert(key, access.next_value()?);
                }
            }
            let sheets = sheets.ok_or_else(|| de::Error::custom("extraction sheets missing"))?;
            Ok((Value::Object(fields), sheets))
        }
    }

    /// The sheets of the extraction whose members besides them are `.0`.
    struct Sheets<'a>(&'a Value);

    impl<'de> DeserializeSeed<'de> for Sheets<'_> {
        type Value = Vec<Sheet>;
        fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
            d.deserialize_seq(self)
        }
    }

    impl<'de> Visitor<'de> for Sheets<'_> {
        type Value = Vec<Sheet>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("the sheets of a stored extraction")
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
            let mut sheets = vec![];
            while let Some(sheet) =
                access.next_element_seed(SheetSeed(Defaults::of(self.0, sheets.len())))?
            {
                sheets.push(sheet);
            }
            Ok(sheets)
        }
    }

    /// A sheet whose cells take what they leave out from `.0`.
    struct SheetSeed(Defaults);

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
                    // Stored in order of their names, the members naming the
                    // font and style its cells take are read by now.
                    let defaults = self.0.with_formats(&fields).map_err(de::Error::custom)?;
                    cells = Some(access.next_value_seed(Cells(defaults))?);
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
            fields.remove(CELL_FONT);
            fields.remove(CELL_STYLE);
            Ok(Sheet {
                fields: Value::Object(fields),
                cells,
                fonts,
                styles,
            })
        }
    }

    /// The cells of a sheet, which take what they leave out from `.0`.
    struct Cells(Defaults);

    impl<'de> DeserializeSeed<'de> for Cells {
        type Value = super::Cells;
        fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
            d.deserialize_seq(self)
        }
    }

    impl<'de> Visitor<'de> for Cells {
        type Value = super::Cells;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("the cells of a sheet")
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
            // A stored sheet keeps its fonts and styles in its tables.
            let (mut fonts, mut styles) = (Table::default(), Table::default());
            let mut built = CellsBuilder::default();
            while let Some(cell) = access.next_element::<Value>()? {
                typed_cell(cell, self.0, true, &mut built, &mut fonts, &mut styles)
                    .map_err(|error| de::Error::custom(format!("{error:#}")))?;
            }
            Ok(built.finish())
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
    fn print_settings_are_stored_once_for_the_parts_that_have_the_same() {
        let setting = |part: &str, xml: &str| json!({"part":part,"component":element_name(xml).unwrap(),"xml":xml});
        let (margins, setup) = (
            r#"<pageMargins left="0.7"/>"#,
            r#"<pageSetup orientation="landscape"/>"#,
        );
        let settings = json!([
            setting(
                "word/document.xml",
                r#"<w:sectPr><w:pgSz w:w="11906"/></w:sectPr>"#
            ),
            setting("xl/worksheets/sheet1.xml", margins),
            setting("xl/worksheets/sheet1.xml", setup),
            setting("xl/worksheets/sheet10.xml", margins),
            setting("xl/worksheets/sheet2.xml", margins),
            setting("xl/worksheets/sheet2.xml", setup),
        ]);
        let stored = stored_print_settings(&settings).unwrap();
        assert_eq!(
            stored,
            json!([
                {"parts":["word/document.xml"],"xml":[r#"<w:sectPr><w:pgSz w:w="11906"/></w:sectPr>"#]},
                {"parts":["xl/worksheets/sheet1.xml","xl/worksheets/sheet2.xml"],"xml":[margins,setup]},
                {"parts":["xl/worksheets/sheet10.xml"],"xml":[margins]},
            ])
        );
        assert_eq!(
            expanded_print_settings(&stored).unwrap(),
            array(&settings).unwrap()[..]
        );
        assert_eq!(settings[0]["component"], "sectPr");
        // Settings that the parts in order of their names cannot give back,
        // or named otherwise than by their element, are refused.
        let unordered = json!([setting("b.xml", margins), setting("a.xml", margins)]);
        assert!(stored_print_settings(&unordered).is_err());
        let renamed = json!([{"part":"a.xml","component":"margins","xml":margins}]);
        assert!(stored_print_settings(&renamed).is_err());
    }

    /// The stored bytes of `extraction`, after checking that every way of
    /// reading them back gives it.
    fn stored_and_read_back(extraction: &Extraction) -> String {
        let bytes = stored(extraction).unwrap();
        assert_eq!(hash(extraction).unwrap(), data::hash(&bytes));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("extraction.json");
        std::fs::write(&path, &bytes).unwrap();
        data::write(
            &dir.path().join("document.yml"),
            &json!({"schema_version":"1","document_id":extraction["document_id"],"source":extraction["source"],"extraction":data::hash(&bytes)}),
        )
        .unwrap();
        assert_eq!(&read_record(&path).unwrap(), extraction);
        assert_eq!(&read_known_record(&path).unwrap(), extraction);
        assert_eq!(&read(&path).unwrap(), extraction);
        // The reading form is checked as it is written, and gives the
        // extraction back.
        let out = dir.path().join("reading.json");
        write_reading(extraction, &out).unwrap();
        let written = data::read(&out, Some("extraction-view")).unwrap();
        assert_eq!(written, reading(extraction).unwrap());
        assert_eq!(&from_reading(written).unwrap(), extraction);
        String::from_utf8(bytes).unwrap()
    }

    /// A cell given as JSON with its cached result, as an original gives it.
    fn cell(
        id: &str,
        address: &str,
        kind: &str,
        value: Value,
        formula: Value,
        format: &str,
    ) -> Value {
        json!({"id":id,"address":address,"type":kind,"value":value,"cached":if formula.is_null(){Value::Null}else{value.clone()},"formula":formula,"number_format":format})
    }

    #[test]
    fn stored_cells_leave_out_what_reading_puts_back() {
        let source = json!({"path":"book.xlsx","sha256":"a".repeat(64)});
        let note = format!("{}added", crate::document_source::base_note("xlsx"));
        let setting = |part: &str, xml: &str| json!({"part":part,"component":element_name(xml).unwrap(),"xml":xml});
        let drawing = json!({"id":"xl/drawings/drawing1.xml#2","part":"xl/drawings/drawing1.xml","kind":"shape",
            "macro":null,"control":null,"name":"","description":"","text":"","anchor":{"kind":"twoCellAnchor"},
            "group":null,"transform":null,"geometry":"rect","connections":[],"image":null,"linked_image":null});
        let extraction = json!({"schema_version":"4","document_id":"book.xlsx","source":source,"parser":"arp4-rust/test;cells/1",
            "findings":[{"level":"warning","code":"R001","message":note}],"assets":[],
            "print_settings":[setting("xl/worksheets/sheet1.xml","<a/>"),setting("xl/worksheets/sheet1.xml","<b/>"),
                setting("xl/worksheets/sheet2.xml","<a/>"),setting("xl/worksheets/sheet2.xml","<b/>")],
            "sheets":[
                {"name":"A","part":"xl/worksheets/sheet1.xml","state":"visible","merges":[],"drawings":[drawing],"cells":[cell("c-1-A1","A1","string",json!("x"),Value::Null,"General")]},
                {"name":"B","part":"xl/worksheets/sheet2.xml","state":"visible","merges":[],"cells":[
                    cell("c-2-B2","B2","formula",json!(3),json!("1+2"),"0.00"),
                    cell("text-2","A1","string",json!("y"),Value::Null,""),
                    cell("c-1-C3","C3","string",json!("z"),Value::Null,"General"),
                    cell("c-2-D4","D4","error",json!("#DIV/0!"),Value::Null,"General")]}]});
        let mut extraction = extraction;
        // A cell with a font, a style and a place in a text original.
        let mut placed = cell("text-5", "A2", "number", json!(4.5), Value::Null, "0.0");
        placed["font"] = json!({"latin":"Calibri","east_asian":"Calibri","size":11,"color":"auto"});
        placed["style"] = json!({"bold":true,"fill":0,"border":1,"strike":false});
        placed["position"] = json!({"kind":"paragraph","headings":["1. 概要"],"line_start":3,
            "line_end":3,"byte_start":10,"byte_end":20,"columns":[]});
        extraction["sheets"][1]["cells"]
            .as_array_mut()
            .unwrap()
            .push(placed);
        for sheet in extraction["sheets"].as_array_mut().unwrap() {
            for list in SHEET_LISTS {
                if sheet.get(list).is_none() {
                    sheet[list] = json!([]);
                }
            }
        }
        let extraction = Extraction::from_value(extraction).unwrap();
        let text = stored_and_read_back(&extraction);
        assert!(!text.contains("\"source\"") && !text.contains("General"));
        // An Excel cell keeps no ID its sheet and address give, and no type
        // its value or formula gives: an error keeps its type.
        assert!(!text.contains("c-1-A1") && !text.contains("c-2-B2"));
        assert!(text.contains("text-2") && text.contains("c-1-C3") && text.contains("1+2"));
        assert_eq!(text.matches("\"type\"").count(), 1);
        assert!(text.contains(r##""type": "error", "value": "#DIV/0!""##));
        // The note keeps only what the original adds to the text of its
        // format, the print settings of both sheets are stored once and a
        // drawing keeps no part or empty member.
        assert!(
            !text.contains(crate::document_source::base_note("xlsx")) && text.contains("added")
        );
        assert_eq!(text.matches("<a/>").count(), 1);
        assert!(!text.contains("\"component\""));
        assert!(!text.contains("\"xl/drawings/drawing1.xml\"") && !text.contains("linked_image"));
        // A font, a style and a print setting are each one line.
        assert!(text.contains(
            r#"{"color": "auto", "east_asian": "Calibri", "latin": "Calibri", "size": 11}"#
        ));
        assert!(text.contains(r#"{"parts": ["xl/worksheets/sheet1.xml", "xl/worksheets/sheet2.xml"], "xml": ["<a/>", "<b/>"]}"#));
    }

    #[test]
    fn stored_cells_of_other_originals_leave_out_the_ids_of_their_places() {
        let source = json!({"path":"spec.docx","sha256":"b".repeat(64)});
        let note = crate::document_source::base_note("docx");
        let extraction = json!({"schema_version":"4","document_id":"spec.docx","source":source,
            "parser":"arp4-rust/test;word-blocks/1","findings":[{"level":"warning","code":"R001","message":note}],"assets":[],
            "sheets":[
                {"name":"document","part":"word/document.xml","state":"visible","merges":[],"cells":[
                    cell("text-1","A1","string",json!("概要"),Value::Null,""),
                    cell("text-2","A2","string",json!("本文"),Value::Null,""),
                    cell("text-9","A3","string",json!("移した段落"),Value::Null,""),
                    cell("text-4","A4","number",json!(4),Value::Null,"General")]},
                {"name":"header-1","part":"word/header1.xml","state":"visible","merges":[],"cells":[
                    cell("text-1","A1","string",json!("見出し"),Value::Null,"")]}]});
        let extraction = Extraction::from_value(extraction).unwrap();
        let text = stored_and_read_back(&extraction);
        // The nth cell of a page keeps no ID text-<n> and no empty number
        // format: a cell whose ID its place does not give keeps it, and
        // General is a number format of its own.
        assert_eq!(text.matches("\"id\"").count(), 1);
        assert!(text.contains(r#""id": "text-9""#));
        assert_eq!(text.matches("\"number_format\"").count(), 1);
        assert!(text.contains(r#""number_format": "General""#));
        assert!(!text.contains("\"type\""));
    }

    #[test]
    fn stored_cells_leave_out_the_font_and_style_most_of_their_sheet_have() {
        let source = json!({"path":"book.xlsx","sha256":"c".repeat(64)});
        let note = crate::document_source::base_note("xlsx");
        let (small, large) = (
            json!({"latin":"Meiryo","east_asian":"Meiryo","size":9,"color":"auto"}),
            json!({"latin":"Meiryo","east_asian":"Meiryo","size":14,"color":"auto"}),
        );
        let (plain, bold) = (
            json!({"bold":false,"fill":0,"border":1,"strike":false}),
            json!({"bold":true,"fill":2,"border":1,"strike":false}),
        );
        let formatted = |address: &str, value: &str, font: &Value, style: &Value| {
            let mut c = cell(
                &format!("c-1-{address}"),
                address,
                "string",
                json!(value),
                Value::Null,
                "General",
            );
            c["font"] = font.clone();
            c["style"] = style.clone();
            c
        };
        let mut sheet = json!({"name":"S","part":"xl/worksheets/sheet1.xml","state":"visible",
            "merges":["A1:C1","B4:C4"],"cells":[
                formatted("A1", "見出し", &large, &bold),
                formatted("A2", "1", &small, &plain),
                formatted("B2", "2", &small, &plain),
                formatted("A3", "3", &small, &plain),
                cell("c-1-B3", "B3", "string", json!("書式なし"), Value::Null, "General")]});
        for list in SHEET_LISTS {
            sheet[list] = json!([]);
        }
        let extraction = Extraction::from_value(json!({"schema_version":"4","document_id":"book.xlsx",
            "source":source,"parser":"arp4-rust/test;cells/1",
            "findings":[{"level":"warning","code":"R001","message":note}],"assets":[],"sheets":[sheet]}))
        .unwrap();
        let text = stored_and_read_back(&extraction);
        // The sheet names the font and style most of its cells have, which
        // those cells leave out; a cell without any has null.
        assert!(text.contains(r#""cell_font": 1,"#) && text.contains(r#""cell_style": 1,"#));
        assert!(text.contains(r#"{"address": "A2", "value": "1"}"#));
        assert!(text.contains(r#"{"address": "A1", "font": 0, "style": 0, "value": "見出し"}"#));
        assert!(
            text.contains(r#"{"address": "B3", "font": null, "style": null, "value": "書式なし"}"#)
        );
        // The merges of a sheet are one line.
        assert!(text.contains(r#""merges": ["A1:C1", "B4:C4"]"#));
        // The reading form writes them out in place of their indexes.
        let reading = reading(&extraction).unwrap();
        let sheet = &reading["sheets"][0];
        assert_eq!(sheet["cell_font"], small);
        assert_eq!(sheet["cell_style"], plain);
        assert_eq!(sheet["cells"][0]["font"], large);
        assert!(sheet["cells"][1].get("font").is_none() && sheet["cells"][1].get("id").is_none());
        assert!(sheet["cells"][4]["style"].is_null());
        assert!(sheet.get("fonts").is_none() && reading["findings"][0]["message"] == note);
    }
}
