use super::*;
use std::{
    borrow::Cow,
    collections::{HashSet, hash_map},
    fmt::Write as _,
};

/// The complete mapping entries of an inspection, in order. The default
/// entries of an extraction, one or two for every cell, are kept as places in
/// it rather than as JSON, which would take a dozen allocated strings per
/// cell: an [`EntryRef`] reads their texts from the extraction they are of,
/// from [`Entries::names`] those it holds none of, and the rest from
/// [`DEFAULT_TEXTS`]. Entries stored in mappings.yml or generated for new
/// content are JSON.
#[derive(Default)]
pub(crate) struct Entries {
    order: Vec<Slot>,
    /// The JSON entries the order names by number: those stored in
    /// mappings.yml, whether or not they are kept, and those generated.
    json: Vec<Value>,
    /// The sheets of the extraction, by number.
    sheets: Vec<SheetNames>,
    /// The names of default entries the extraction holds none of, each
    /// written once: the rows of cells (`r<number>`), and the fields of
    /// formulas (`<cell ID>-formula`) and of shape texts
    /// (`shape-<drawing ID>-text`), whose rows they begin with.
    names: String,
}

/// The page and name of a sheet of the extraction.
struct SheetNames {
    page: Box<str>,
    name: Box<str>,
    /// Whether the page, block and field of each default entry of the sheet
    /// are no other default entry's: its page is no other sheet's, and the
    /// IDs of its cells and the keys of its shapes are each their own.
    unique: bool,
}

#[derive(Clone, Copy)]
enum Slot {
    Default(DefaultEntry),
    /// A JSON entry, by its number in [`Entries::json`].
    Json(u32),
}

/// A default entry of an extraction cell, formula, shape text or block, by
/// the sheet and the cell or drawing it is of. Its block, reason and
/// writeback are numbers of [`DEFAULT_TEXTS`].
#[derive(Clone, Copy)]
struct DefaultEntry {
    sheet: u32,
    /// The cell of a cell or formula entry, the drawing of a shape entry.
    item: u32,
    /// Where its name is in [`Entries::names`]: the row of a cell entry, the
    /// field of a formula or shape entry.
    name: (u32, u32),
    kind: DefaultKind,
    block: Text,
    reason: Text,
    writeback: Text,
}

#[derive(Clone, Copy, PartialEq)]
enum DefaultKind {
    Cell,
    Formula,
    Shape,
    Block,
}

/// The texts of default entries: their blocks, writebacks and reasons. An
/// entry holds each of its texts by its number in this list.
const DEFAULT_TEXTS: [&str; 18] = [
    "table-1",
    "formulas",
    "shapes",
    "extraction-notes",
    "cell",
    "excluded",
    "formula",
    "shape",
    "配列数式・スピルの計算結果のため書き戻し対象外",
    "データテーブルの計算結果のため書き戻し対象外",
    "ピボットテーブルの集計結果のため書き戻し対象外",
    "結合セルの左上以外のため書き戻し対象外",
    "原本から転記",
    "数式原文。配列数式・スピル・データテーブル・結合で隠れたセルのため書き戻し対象外",
    "数式原文。変更すると書き戻しで数式を置き換え、Excelで再計算",
    "図形の文字",
    "抽出範囲の申告",
    "表の構造",
];

/// A text of [`DEFAULT_TEXTS`], by its number.
#[derive(Clone, Copy)]
struct Text(u8);

impl Text {
    fn of(text: &str) -> Self {
        let number = DEFAULT_TEXTS
            .iter()
            .position(|listed| *listed == text)
            .unwrap_or_else(|| panic!("{text:?} is no text of default entries"));
        Self(number as u8)
    }

    fn get(self) -> &'static str {
        DEFAULT_TEXTS[usize::from(self.0)]
    }
}

impl Entries {
    pub(super) fn len(&self) -> usize {
        self.order.len()
    }

    /// The entry numbered `index`, its defaults read from `extraction`, the
    /// one they were made from.
    pub(super) fn get<'a>(&'a self, index: usize, extraction: &'a Extraction) -> EntryRef<'a> {
        EntryRef {
            entries: self,
            extraction,
            slot: self.order[index],
        }
    }

    /// The entries in order, as [`Entries::get`] gives them.
    pub(super) fn iter<'a>(
        &'a self,
        extraction: &'a Extraction,
    ) -> impl ExactSizeIterator<Item = EntryRef<'a>> + 'a {
        self.order.iter().map(move |&slot| EntryRef {
            entries: self,
            extraction,
            slot,
        })
    }

    fn push_json(&mut self, entry: Value) -> Result<()> {
        self.order.push(Slot::Json(u32::try_from(self.json.len())?));
        self.json.push(entry);
        Ok(())
    }
}

/// A mapping entry of [`Entries`], read with the extraction its default
/// entries are of.
#[derive(Clone, Copy)]
pub(crate) struct EntryRef<'a> {
    entries: &'a Entries,
    extraction: &'a Extraction,
    slot: Slot,
}

/// The position of a table entry: its row and column keys and the type of
/// its value.
pub(super) struct EntryPosition<'a> {
    pub row: &'a str,
    pub column: &'a str,
    pub kind: &'a str,
}

impl<'a> EntryRef<'a> {
    fn sheet(self, d: DefaultEntry) -> &'a SheetNames {
        &self.entries.sheets[d.sheet as usize]
    }

    fn cell(self, d: DefaultEntry) -> Cell<'a> {
        self.extraction.sheets[d.sheet as usize]
            .cells
            .get(d.item as usize)
            .expect("the cell of a default entry is one of its sheet")
    }

    fn drawing(self, d: DefaultEntry) -> &'a Value {
        &self.extraction.sheets[d.sheet as usize]["drawings"][d.item as usize]
    }

    fn name(self, d: DefaultEntry) -> &'a str {
        let (start, end) = d.name;
        &self.entries.names[start as usize..end as usize]
    }

    fn json_entry(self) -> Option<&'a Value> {
        match self.slot {
            Slot::Json(index) => Some(&self.entries.json[index as usize]),
            Slot::Default(_) => None,
        }
    }

    pub(super) fn page(self) -> Result<&'a str> {
        match self.slot {
            Slot::Json(_) => string(&self.json_entry().expect("a JSON entry")["page"]),
            Slot::Default(d) => Ok(&self.sheet(d).page),
        }
    }

    pub(super) fn block(self) -> Result<&'a str> {
        match self.slot {
            Slot::Json(_) => string(&self.json_entry().expect("a JSON entry")["block"]),
            Slot::Default(d) => Ok(d.block.get()),
        }
    }

    /// The field, `None` when it is null (the entry of a whole block).
    pub(super) fn field(self) -> Option<&'a str> {
        match self.slot {
            Slot::Json(_) => self.json_entry().expect("a JSON entry")["field"].as_str(),
            Slot::Default(d) => match d.kind {
                DefaultKind::Cell => Some(self.cell(d).id()),
                DefaultKind::Formula | DefaultKind::Shape => Some(self.name(d)),
                DefaultKind::Block => None,
            },
        }
    }

    pub(super) fn field_is_null(self) -> bool {
        match self.slot {
            Slot::Json(_) => self.json_entry().expect("a JSON entry")["field"].is_null(),
            Slot::Default(d) => d.kind == DefaultKind::Block,
        }
    }

    pub(super) fn reason(self) -> Result<&'a str> {
        match self.slot {
            Slot::Json(_) => string(&self.json_entry().expect("a JSON entry")["reason"]),
            Slot::Default(d) => Ok(d.reason.get()),
        }
    }

    pub(super) fn writeback(self) -> Result<&'a str> {
        match self.slot {
            Slot::Json(_) => string(&self.json_entry().expect("a JSON entry")["writeback"]),
            Slot::Default(d) => Ok(d.writeback.get()),
        }
    }

    pub(super) fn target(self) -> Result<Option<MappingTarget<'a>>> {
        let d = match self.slot {
            Slot::Json(_) => {
                return mapping_target(&self.json_entry().expect("a JSON entry")["target"]);
            }
            Slot::Default(d) => d,
        };
        let sheet = &self.sheet(d).name;
        Ok(match d.kind {
            DefaultKind::Cell | DefaultKind::Formula => Some(MappingTarget::Cell {
                sheet,
                cell: self.cell(d).address(),
            }),
            DefaultKind::Shape => Some(MappingTarget::Shape {
                sheet,
                shape: string(&self.drawing(d)["id"])?,
            }),
            DefaultKind::Block => None,
        })
    }

    /// The position of a table entry, `None` for an entry without one.
    pub(super) fn position(self) -> Result<Option<EntryPosition<'a>>> {
        let d = match self.slot {
            Slot::Json(_) => {
                let Some(position) =
                    self.json_entry().expect("a JSON entry")["position"].as_object()
                else {
                    return Ok(None);
                };
                return Ok(Some(EntryPosition {
                    row: string(&position["row"])?,
                    column: string(&position["column"])?,
                    kind: string(&position["type"])?,
                }));
            }
            Slot::Default(d) => d,
        };
        Ok(match d.kind {
            DefaultKind::Cell => {
                let cell = self.cell(d);
                Some(EntryPosition {
                    row: self.name(d),
                    column: cell
                        .address()
                        .trim_end_matches(|c: char| c.is_ascii_digit()),
                    kind: cell.value().kind(),
                })
            }
            DefaultKind::Formula => Some(EntryPosition {
                row: self.cell(d).id(),
                column: "formula",
                kind: "string",
            }),
            DefaultKind::Shape => {
                let field = self.name(d);
                Some(EntryPosition {
                    row: &field[..field.len() - SHAPE_FIELD.len()],
                    column: "text",
                    kind: "string",
                })
            }
            DefaultKind::Block => None,
        })
    }

    /// Whether the page, block and field of the entry are no other entry's:
    /// those of a default entry whose sheet makes them its own. A stored or
    /// generated entry may repeat another's, which is checked by them.
    pub(super) fn unique(self) -> bool {
        match self.slot {
            Slot::Json(_) => false,
            Slot::Default(d) => self.sheet(d).unique,
        }
    }

    /// The entry as mappings.yml writes it.
    pub(super) fn json(self) -> Cow<'a, Value> {
        let d = match self.slot {
            Slot::Json(_) => return Cow::Borrowed(self.json_entry().expect("a JSON entry")),
            Slot::Default(d) => d,
        };
        let sheet = &*self.sheet(d).name;
        let target = match d.kind {
            DefaultKind::Cell | DefaultKind::Formula => {
                json!({"sheet":sheet,"cell":self.cell(d).address()})
            }
            DefaultKind::Shape => json!({"sheet":sheet,"shape":self.drawing(d)["id"]}),
            DefaultKind::Block => Value::Null,
        };
        let mut entry = json!({"page":&*self.sheet(d).page,"block":d.block.get(),
            "field":self.field(),"reason":d.reason.get(),"target":target,
            "writeback":d.writeback.get()});
        if let Ok(Some(p)) = self.position() {
            entry["position"] = json!({"row":p.row,"column":p.column,"type":p.kind});
        }
        Cow::Owned(entry)
    }
}

/// What the field of a shape text entry adds to its row, the shape's key.
const SHAPE_FIELD: &str = "-text";

pub(super) fn target(value: &Value) -> Result<(&str, &str)> {
    Ok((string(&value["sheet"])?, string(&value["cell"])?))
}
pub(super) enum MappingTarget<'a> {
    Cell {
        sheet: &'a str,
        cell: &'a str,
    },
    InsertionRow {
        sheet: &'a str,
        insertion: &'a str,
        offset: u32,
        column: &'a str,
    },
    InsertionColumn {
        sheet: &'a str,
        insertion: &'a str,
        offset: u32,
        row: u32,
    },
    /// The text of an Excel shape, by its extraction ID (`<drawing part>#<id>`).
    Shape {
        sheet: &'a str,
        shape: &'a str,
    },
}
pub(super) fn mapping_target(value: &Value) -> Result<Option<MappingTarget<'_>>> {
    if value.is_null() {
        return Ok(None);
    }
    if value.get("shape").is_some() {
        return Ok(Some(MappingTarget::Shape {
            sheet: string(&value["sheet"])?,
            shape: string(&value["shape"])?,
        }));
    }
    if value.get("cell").is_some() {
        return Ok(Some(MappingTarget::Cell {
            sheet: string(&value["sheet"])?,
            cell: string(&value["cell"])?,
        }));
    }
    let sheet = string(&value["sheet"])?;
    let insertion = string(&value["insertion"])?;
    let offset = u32::try_from(
        value["offset"]
            .as_u64()
            .context("target offset must be integer")?,
    )?;
    if value.get("column").is_some() {
        return Ok(Some(MappingTarget::InsertionRow {
            sheet,
            insertion,
            offset,
            column: string(&value["column"])?,
        }));
    }
    Ok(Some(MappingTarget::InsertionColumn {
        sheet,
        insertion,
        offset,
        row: u32::try_from(
            value["row"]
                .as_u64()
                .context("target row must be integer")?,
        )?,
    }))
}
/// The sheet whose content page is `page`: the `page` a sheet names, or else
/// `sheet-<n>` for the n-th sheet.
pub(super) fn page_sheet(extraction: &Extraction, page: &str) -> Result<String> {
    let sheets = &extraction.sheets;
    if sheets
        .iter()
        .any(|sheet| sheet.fields.get("page").is_some())
    {
        return string(
            &sheets
                .iter()
                .find(|sheet| sheet["page"] == page)
                .with_context(|| format!("content page {page} has no worksheet"))?["name"],
        )
        .map(str::to_owned);
    }
    let index: usize = page
        .strip_prefix("sheet-")
        .context("structural content page must be a worksheet page")?
        .parse::<usize>()?
        .checked_sub(1)
        .context("invalid worksheet page")?;
    string(
        &extraction
            .sheets
            .get(index)
            .context("content page has no worksheet")?["name"],
    )
    .map(str::to_owned)
}
/// The insertion a row or column key of new table content names: `<operation
/// ID>-<n>` is the n-th row or column that operation inserts. Original rows
/// keep `r<number>` and original columns their letters, so a new position
/// never takes the key of an original one.
fn inserted_position(
    operations: &[excel::StructuralOperation],
    sheet: &str,
    key: &str,
    row: bool,
) -> Result<Option<(String, u32)>> {
    let Some((id, number)) = key.rsplit_once('-') else {
        return Ok(None);
    };
    let Some(operation) = operations
        .iter()
        .find(|operation| operation.sheet == sheet && operation.id == id)
    else {
        return Ok(None);
    };
    let Ok(number) = number.parse::<u32>() else {
        return Ok(None);
    };
    let axis = if row { "row" } else { "column" };
    ensure!(
        operation.insertion() && operation.row_operation() == row,
        "{axis} {key} names operation {id}, which does not insert {axis}s"
    );
    ensure!(
        (1..=operation.count).contains(&number),
        "{axis} {key} is outside the {} {axis}(s) operation {id} inserts",
        operation.count
    );
    Ok(Some((id.to_owned(), number - 1)))
}
/// The mapping entries import derives from `extraction`: one per cell and formula,
/// and one per page block. mappings.yml stores only entries that differ from these.
/// The list has room for `more` entries after them.
pub(super) fn default_entries(extraction: &Extraction, more: usize) -> Result<Entries> {
    let native_text = extraction["parser"]
        .as_str()
        .and_then(|parser| parser.split(';').nth(1))
        == Some("native-text/1");
    // An entry per cell and formula, made as large as that at once: grown by
    // doubling, the list of a large workbook held up to twice its entries.
    let mut order = Vec::with_capacity(
        extraction
            .sheets
            .iter()
            .map(|sheet| {
                let formulas = sheet.cells.iter().filter(|c| c.kind() == CellKind::Formula);
                let drawings = sheet["drawings"].as_array().map_or(0, Vec::len);
                sheet.cells.len() + formulas.count() + drawings + 4
            })
            .sum::<usize>()
            + more,
    );
    let mut names = String::new();
    let mut sheets: Vec<SheetNames> = Vec::with_capacity(extraction.sheets.len());
    for (i, sheet) in extraction.sheets.iter().enumerate() {
        let page: Box<str> = match sheet["page"].as_str() {
            Some(page) => page.into(),
            None => format!("sheet-{}", i + 1).into(),
        };
        let name: Box<str> = string(&sheet["name"])?.into();
        let number = u32::try_from(i)?;
        let entry = |item, name, kind, block, reason, writeback| {
            Slot::Default(DefaultEntry {
                sheet: number,
                item,
                name,
                kind,
                block: Text::of(block),
                reason: Text::of(reason),
                writeback: Text::of(writeback),
            })
        };
        // The names of the rows of the cells, each written once.
        let mut rows: HashMap<u32, (u32, u32)> = HashMap::new();
        let computed_ranges = excel::ComputedRanges::new(&sheet.fields)?;
        let merges = excel::Merges::new(&sheet["merges"])?;
        let cells = &sheet.cells;
        let mut has_formulas = false;
        for (item, c) in cells.iter().enumerate() {
            let item = u32::try_from(item)?;
            let address = c.address();
            let (_, row) = excel::coordinate(address)?;
            let computed = computed_ranges.find(address)?.map(|(kind, _)| kind);
            let hidden = merges.hiding(address)?.is_some();
            let reason = match computed {
                Some("array") => "配列数式・スピルの計算結果のため書き戻し対象外",
                Some("data_table") => "データテーブルの計算結果のため書き戻し対象外",
                Some(_) => "ピボットテーブルの集計結果のため書き戻し対象外",
                None if hidden => "結合セルの左上以外のため書き戻し対象外",
                None => "原本から転記",
            };
            let excluded = hidden
                || computed.is_some()
                || native_text
                || c.kind() == CellKind::Formula
                || c.kind() == CellKind::Error;
            let row = match rows.entry(row) {
                hash_map::Entry::Occupied(name) => *name.get(),
                hash_map::Entry::Vacant(name) => {
                    *name.insert(written(&mut names, format_args!("r{row}"))?)
                }
            };
            order.push(entry(
                item,
                row,
                DefaultKind::Cell,
                "table-1",
                reason,
                if excluded { "excluded" } else { "cell" },
            ));
            if c.kind() == CellKind::Formula {
                has_formulas = true;
                // The formula of an array, spill or data table spans its range, and
                // one hidden by a merge is not shown; those stay as they are.
                let (reason, writeback) = if computed.is_some() || hidden {
                    (
                        "数式原文。配列数式・スピル・データテーブル・結合で隠れたセルのため書き戻し対象外",
                        "excluded",
                    )
                } else {
                    (
                        "数式原文。変更すると書き戻しで数式を置き換え、Excelで再計算",
                        "formula",
                    )
                };
                let field = written(&mut names, format_args!("{}-formula", c.id()))?;
                order.push(entry(
                    item,
                    field,
                    DefaultKind::Formula,
                    "formulas",
                    reason,
                    writeback,
                ));
            }
        }
        let mut shapes = HashSet::new();
        let mut unique_shapes = true;
        for (item, drawing) in sheet["drawings"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            let Some(key) = shape_key(drawing)? else {
                continue;
            };
            let field = written(&mut names, format_args!("{key}{SHAPE_FIELD}"))?;
            unique_shapes &= shapes.insert(key);
            order.push(entry(
                u32::try_from(item)?,
                field,
                DefaultKind::Shape,
                "shapes",
                "図形の文字",
                if native_text { "excluded" } else { "shape" },
            ));
        }
        order.push(entry(
            0,
            (0, 0),
            DefaultKind::Block,
            "extraction-notes",
            "抽出範囲の申告",
            "excluded",
        ));
        for (block, present) in [
            ("table-1", !cells.is_empty()),
            ("formulas", has_formulas),
            ("shapes", !shapes.is_empty()),
        ] {
            if present {
                order.push(entry(
                    0,
                    (0, 0),
                    DefaultKind::Block,
                    block,
                    "表の構造",
                    "excluded",
                ));
            }
        }
        // The IDs of the cells in order, each once, and none the empty field
        // of a block.
        let mut ids: Vec<&str> = cells.iter().map(|c| c.id()).collect();
        ids.sort_unstable();
        let mut unique = unique_shapes
            && ids.first() != Some(&"")
            && ids.windows(2).all(|pair| pair[0] != pair[1]);
        for other in sheets.iter_mut().filter(|other| other.page == page) {
            other.unique = false;
            unique = false;
        }
        sheets.push(SheetNames { page, name, unique });
    }
    Ok(Entries {
        order,
        json: vec![],
        sheets,
        names: crate::data::exact_text(names),
    })
}

/// Writes `text` at the end of `names`, giving where it is.
fn written(names: &mut String, text: std::fmt::Arguments<'_>) -> Result<(u32, u32)> {
    let start = u32::try_from(names.len())?;
    names.write_fmt(text)?;
    Ok((start, u32::try_from(names.len())?))
}

/// The shapes of an Excel sheet that hold text, by their content key
/// (`shape-<drawing ID>`). A slide's shape text is laid out with the slide's
/// paragraphs instead.
pub(super) fn shape_texts(sheet: &Value) -> Result<Vec<(String, &Value)>> {
    let mut shapes = vec![];
    for drawing in sheet["drawings"].as_array().into_iter().flatten() {
        if let Some(key) = shape_key(drawing)? {
            shapes.push((key, drawing));
        }
    }
    Ok(shapes)
}

/// The content key of a drawing of [`shape_texts`], none for one it leaves out.
fn shape_key(drawing: &Value) -> Result<Option<String>> {
    if drawing["anchor"]["kind"] == "slide"
        || !drawing["text"]
            .as_str()
            .is_some_and(|text| !text.is_empty())
    {
        return Ok(None);
    }
    let id = string(&drawing["id"])?;
    let (_, raw) = id
        .rsplit_once('#')
        .context("shape ID without drawing part")?;
    Ok(Some(format!("shape-{raw}")))
}

/// The complete entries of stored `mappings`: the defaults of `extraction`, each
/// replaced by a stored entry with the same page, block and field, then the stored
/// entries that replace none. Regeneration drops those whose content is gone.
/// The stored entries are taken out of `mappings`, whose `entries` is left
/// empty: the complete entries are returned apart from it.
pub(super) fn with_default_entries(
    mut mappings: Value,
    extraction: &Extraction,
) -> Result<(Value, Entries)> {
    let Value::Array(stored) = mappings["entries"].take() else {
        bail!("mappings entries must be an array");
    };
    mappings["entries"] = json!([]);
    let mut replacements = BTreeMap::new();
    for (index, entry) in stored.iter().enumerate() {
        replacements.entry(entry_key(entry)?).or_insert(index);
    }
    let mut used = vec![false; stored.len()];
    let mut entries = default_entries(extraction, stored.len())?;
    if !replacements.is_empty() {
        for at in 0..entries.len() {
            let entry = entries.get(at, extraction);
            let key = (entry.page()?, entry.block()?, entry.field().unwrap_or(""));
            if let Some(index) = replacements.get(&key).copied() {
                used[index] = true;
                entries.order[at] = Slot::Json(u32::try_from(index)?);
            }
        }
    }
    entries.json = stored;
    for (index, used) in used.into_iter().enumerate() {
        if !used {
            entries.order.push(Slot::Json(u32::try_from(index)?));
        }
    }
    Ok((mappings, entries))
}

fn entry_key(entry: &Value) -> Result<(&str, &str, &str)> {
    Ok((
        string(&entry["page"])?,
        string(&entry["block"])?,
        entry["field"].as_str().unwrap_or(""),
    ))
}

/// A (page, block) or (row, column) pair borrowed from the content pages.
type Pair<'a> = (&'a str, &'a str);

/// Keeps the entries whose content still exists and maps new table cells. Takes
/// `mappings` by value: a large document has an entry for every cell, and copying
/// them, or keying them by owned strings, cost more than the checks themselves.
pub(super) fn regenerate_mappings(
    regenerated: Value,
    mut entries: Entries,
    extraction: &Extraction,
    pages: &[super::elements::GridPage],
    operations: &[excel::StructuralOperation],
) -> Result<(Value, Entries)> {
    // The pages by ID, each with whether each of its values is an entry's:
    // its values are in order, which a value is found in by halves, as a
    // large sheet has a value for each cell.
    let mut by_id: BTreeMap<&str, (&super::elements::GridPage, Vec<bool>)> = BTreeMap::new();
    // A page has its notes and blocks of rows, and no fields.
    let fields: BTreeMap<Pair, BTreeSet<&str>> = BTreeMap::new();
    let mut blocks = BTreeSet::new();
    for page in pages {
        let page_id = string(&page.fields["page_id"])?;
        debug_assert!(
            page.values.is_sorted_by(|a, b| {
                (&*a.block, &*a.row, &*a.column) <= (&*b.block, &*b.row, &*b.column)
            }),
            "the values of a page are in order"
        );
        ensure!(
            by_id
                .insert(page_id, (page, vec![false; page.values.len()]))
                .is_none(),
            "duplicate page ID"
        );
        blocks.insert((page_id, "extraction-notes"));
        for block in page.titles.keys() {
            blocks.insert((page_id, block.as_str()));
        }
        for value in &page.values {
            blocks.insert((page_id, &*value.block));
        }
    }
    let mut keep = vec![];
    for entry in entries.iter(extraction) {
        let page = entry.page()?;
        let block = entry.block()?;
        let kept = if let Some(position) = entry.position()? {
            let found = by_id.get_mut(page).and_then(|(grid, mapped)| {
                let at = grid
                    .values
                    .binary_search_by(|v| {
                        (&*v.block, &*v.row, &*v.column).cmp(&(
                            block,
                            position.row,
                            position.column,
                        ))
                    })
                    .ok()?;
                Some(&mut mapped[at])
            });
            match found {
                Some(mapped) => {
                    *mapped = true;
                    true
                }
                None => false,
            }
        } else if entry.field_is_null() {
            blocks.contains(&(page, block))
        } else {
            let field = entry.field().context("expected string")?;
            fields
                .get(&(page, block))
                .is_some_and(|values| values.contains(field))
        };
        keep.push(kept);
    }
    // The fields of the entries kept, which a generated entry's must not be:
    // made only for one, as a large sheet has an entry for each cell.
    let mut used_fields: Option<BTreeSet<&str>> = None;
    let mut generated = vec![];
    let mut generated_fields = BTreeSet::new();
    let workbook = identity::is_excel(extraction);
    for (page, (grid, mapped)) in &by_id {
        let page = *page;
        let sheet = page_sheet(extraction, page)?;
        for (value, _) in grid
            .values
            .iter()
            .zip(mapped)
            .filter(|(_, mapped)| !**mapped)
        {
            let (block, row, column) = (&*value.block, &*value.row, &*value.column);
            let value = &value.value;
            ensure!(
                value.is_null() || kind(value) != "object",
                "new table references require explicit mapping"
            );
            let target = match (
                inserted_position(operations, &sheet, row, true)?,
                inserted_position(operations, &sheet, column, false)?,
            ) {
                (Some((insertion, offset)), None) => {
                    excel::column_number(column).with_context(|| {
                        format!("{row}/{column}: an inserted row takes values in original columns (A, B, ...)")
                    })?;
                    json!({"sheet":sheet,"insertion":insertion,"offset":offset,"column":column})
                }
                (None, Some((insertion, offset))) => {
                    let row_number = row
                        .strip_prefix('r')
                        .and_then(|number| number.parse::<u32>().ok())
                        .filter(|number| *number > 0)
                        .with_context(|| {
                            format!("{row}/{column}: an inserted column takes values in original rows (r1, r2, ...)")
                        })?;
                    json!({"sheet":sheet,"insertion":insertion,"offset":offset,"row":row_number})
                }
                (Some(_), Some(_)) => bail!(
                    "{row}/{column}: a cell in both an inserted row and an inserted column cannot be written"
                ),
                // A value given to a cell of the workbook that holds none.
                (None, None)
                    if workbook
                        && block == "table-1"
                        && excel::column_number(column).is_ok()
                        && row
                            .strip_prefix('r')
                            .and_then(|number| number.parse::<u32>().ok())
                            .is_some_and(|number| number > 0 && format!("r{number}") == row) =>
                {
                    json!({"sheet":sheet,"cell":format!("{column}{}", &row[1..])})
                }
                (None, None) => bail!(
                    "{row}/{column} is not a cell of the original: edit only cells that hold a value, and name new rows and columns <operation ID>-<n> after the insert_rows or insert_columns operation that adds them"
                ),
            };
            let used_fields = match &mut used_fields {
                Some(used) => used,
                None => used_fields.insert(
                    entries
                        .iter(extraction)
                        .zip(&keep)
                        .filter(|(entry, kept)| **kept && !entry.field_is_null())
                        .map(|(entry, _)| entry.field().context("expected string"))
                        .collect::<Result<_>>()?,
                ),
            };
            let field_prefix = format!("generated-{row}-{column}");
            let mut field = field_prefix.clone();
            let mut suffix = 2;
            while used_fields.contains(field.as_str()) || generated_fields.contains(&field) {
                field = format!("{field_prefix}-{suffix}");
                suffix += 1;
            }
            generated_fields.insert(field.clone());
            let reason = if target.get("cell").is_some() {
                "value of a cell the original leaves empty"
            } else {
                "structural content generated mapping"
            };
            generated.push(json!({
                "page":page,
                "block":block,
                "field":field,
                "reason":reason,
                "target":target,
                "writeback":"cell",
                "position":{"row":row,"column":column,"type":kind(value)}
            }));
        }
    }
    let mut keep = keep.into_iter();
    entries
        .order
        .retain(|_| keep.next().expect("whether each entry is kept"));
    for entry in generated {
        entries.push_json(entry)?;
    }
    Ok((regenerated, entries))
}

/// The lists of mappings.yml, left out when empty.
const LISTS: [&str; 4] = ["entries", "omissions", "operations", "value_edits"];

/// Stored `mappings` with the empty lists [`stored_mappings`] leaves out in
/// place. A correction journal left out is made again by inspection.
pub(super) fn complete_mappings(mut mappings: Value) -> Value {
    for list in LISTS {
        if mappings.get(list).is_none() {
            mappings[list] = json!([]);
        }
    }
    mappings
}

/// Reads and validates mappings.yml at `path`, as [`complete_mappings`] gives it.
pub(super) fn read_mappings(path: &Path) -> Result<Value> {
    Ok(complete_mappings(read(path, Some("mappings"))?))
}

/// The bytes stored for `mappings` at `path`: without empty lists, nor a
/// correction journal that records nothing, not even how an import carried
/// it, which is made again for the extraction it is read with.
pub(super) fn stored_mappings(path: &Path, mappings: &Value) -> Result<Vec<u8>> {
    let mut stored = mappings.clone();
    let members = stored
        .as_object_mut()
        .context("mappings must be an object")?;
    members.retain(|key, value| {
        !(LISTS.contains(&key.as_str()) && value.as_array().is_some_and(Vec::is_empty))
    });
    if members.get("interpretation").is_some_and(|journal| {
        super::import::records_nothing(journal) && journal.get("carried").is_none()
    }) {
        members.remove("interpretation");
    }
    serialized(path, &stored)
}

/// The extraction as the slide operations of `operations` leave it (see
/// [`slide_view`]): the pages deleted slides leave out, and the copies inserted
/// slides add. The extraction itself when there are none.
pub(super) fn operated_extraction<'a>(
    extraction: &'a Extraction,
    operations: &Value,
) -> Result<Cow<'a, Extraction>> {
    let operations = array(operations)?;
    if !operations.iter().any(is_slide_operation) {
        return Ok(Cow::Borrowed(extraction));
    }
    let parsed = parse_slide_operations(operations, &extraction.sheets)?;
    let layouts = extraction["slide_layouts"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    Ok(Cow::Owned(Extraction {
        fields: extraction.fields.clone(),
        sheets: slide_view(&extraction.sheets, layouts, &parsed)?,
    }))
}

#[cfg(test)]
mod efficiency_tests {
    use super::*;

    #[test]
    #[ignore = "manual performance measurement"]
    fn benchmark_column_membership() {
        use std::hint::black_box;
        use std::time::Instant;

        let columns: Vec<Value> = (0..100).map(|index| json!(format!("c{index}"))).collect();
        let keys: Vec<String> = (0..1000)
            .flat_map(|_| (0..100).map(|index| format!("c{index}")))
            .collect();

        let start = Instant::now();
        let old = keys
            .iter()
            .filter(|key| columns.iter().any(|value| value == *key))
            .count();
        let old_ms = start.elapsed().as_millis();

        let start = Instant::now();
        let known: BTreeSet<String> = columns
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect();
        let new = keys
            .iter()
            .filter(|key| known.contains(key.as_str()))
            .count();
        let new_ms = start.elapsed().as_millis();
        assert_eq!(black_box(old), black_box(new));
        eprintln!(
            "column_membership old_ms={old_ms} new_ms={new_ms} cells={} columns={}",
            keys.len(),
            columns.len()
        );
    }
}
