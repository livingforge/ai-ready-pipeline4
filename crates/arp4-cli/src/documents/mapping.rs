use super::*;
use std::{borrow::Cow, sync::Arc};

/// A mapping entry. The default entries of an extraction, one or two for
/// every cell, are kept as what they are made from rather than as JSON, which
/// would take a dozen allocated strings per cell; they are read through the
/// accessors, or as JSON with [`Entry::json`]. Entries stored in mappings.yml
/// or generated for new content are JSON.
pub(crate) enum Entry {
    Json(Value),
    Default(DefaultEntry),
}

/// A default entry of an extraction cell, formula, shape text or block.
pub(crate) struct DefaultEntry {
    page: Arc<str>,
    sheet: Arc<str>,
    block: &'static str,
    field: Option<Box<str>>,
    reason: &'static str,
    writeback: &'static str,
    target: DefaultTarget,
    position: Option<DefaultPosition>,
}

enum DefaultTarget {
    None,
    Cell(Box<str>),
    Shape(Box<str>),
}

struct DefaultPosition {
    row: Box<str>,
    column: Cow<'static, str>,
    kind: &'static str,
}

/// The position of a table entry: its row and column keys and the type of
/// its value.
pub(super) struct EntryPosition<'a> {
    pub row: &'a str,
    pub column: &'a str,
    pub kind: &'a str,
}

impl Entry {
    pub(super) fn page(&self) -> Result<&str> {
        match self {
            Self::Json(e) => string(&e["page"]),
            Self::Default(d) => Ok(&d.page),
        }
    }

    pub(super) fn block(&self) -> Result<&str> {
        match self {
            Self::Json(e) => string(&e["block"]),
            Self::Default(d) => Ok(d.block),
        }
    }

    /// The field, `None` when it is null (the entry of a whole block).
    pub(super) fn field(&self) -> Option<&str> {
        match self {
            Self::Json(e) => e["field"].as_str(),
            Self::Default(d) => d.field.as_deref(),
        }
    }

    pub(super) fn field_is_null(&self) -> bool {
        match self {
            Self::Json(e) => e["field"].is_null(),
            Self::Default(d) => d.field.is_none(),
        }
    }

    pub(super) fn reason(&self) -> Result<&str> {
        match self {
            Self::Json(e) => string(&e["reason"]),
            Self::Default(d) => Ok(d.reason),
        }
    }

    pub(super) fn writeback(&self) -> Result<&str> {
        match self {
            Self::Json(e) => string(&e["writeback"]),
            Self::Default(d) => Ok(d.writeback),
        }
    }

    pub(super) fn target(&self) -> Result<Option<MappingTarget<'_>>> {
        match self {
            Self::Json(e) => mapping_target(&e["target"]),
            Self::Default(d) => Ok(match &d.target {
                DefaultTarget::None => None,
                DefaultTarget::Cell(cell) => Some(MappingTarget::Cell {
                    sheet: &d.sheet,
                    cell,
                }),
                DefaultTarget::Shape(shape) => Some(MappingTarget::Shape {
                    sheet: &d.sheet,
                    shape,
                }),
            }),
        }
    }

    /// The position of a table entry, `None` for an entry without one.
    pub(super) fn position(&self) -> Result<Option<EntryPosition<'_>>> {
        match self {
            Self::Json(e) => {
                let Some(position) = e["position"].as_object() else {
                    return Ok(None);
                };
                Ok(Some(EntryPosition {
                    row: string(&position["row"])?,
                    column: string(&position["column"])?,
                    kind: string(&position["type"])?,
                }))
            }
            Self::Default(d) => Ok(d.position.as_ref().map(|p| EntryPosition {
                row: &p.row,
                column: &p.column,
                kind: p.kind,
            })),
        }
    }

    /// The page, block and field that key the entry's value.
    pub(super) fn key(&self) -> Result<(String, String, String)> {
        Ok((
            self.page()?.into(),
            self.block()?.into(),
            self.field().unwrap_or("").into(),
        ))
    }

    /// The entry as mappings.yml writes it.
    pub(super) fn json(&self) -> Cow<'_, Value> {
        let d = match self {
            Self::Json(e) => return Cow::Borrowed(e),
            Self::Default(d) => d,
        };
        let target = match &d.target {
            DefaultTarget::None => Value::Null,
            DefaultTarget::Cell(cell) => json!({"sheet":&*d.sheet,"cell":&**cell}),
            DefaultTarget::Shape(shape) => json!({"sheet":&*d.sheet,"shape":&**shape}),
        };
        let mut entry = json!({"page":&*d.page,"block":d.block,"field":d.field.as_deref(),
            "reason":d.reason,"target":target,"writeback":d.writeback});
        if let Some(p) = &d.position {
            entry["position"] = json!({"row":&*p.row,"column":&*p.column,"type":p.kind});
        }
        Cow::Owned(entry)
    }
}

pub(super) fn key(entry: &Value) -> Result<(String, String, String)> {
    Ok((
        string(&entry["page"])?.into(),
        string(&entry["block"])?.into(),
        entry["field"].as_str().unwrap_or("").into(),
    ))
}
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
pub(super) fn page_sheet(extraction: &Value, page: &str) -> Result<String> {
    let sheets = array(&extraction["sheets"])?;
    if sheets.iter().any(|sheet| sheet.get("page").is_some()) {
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
        &array(&extraction["sheets"])?
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
pub(super) fn default_entries(extraction: &Value) -> Result<Vec<Entry>> {
    let native_text = extraction["parser"]
        .as_str()
        .and_then(|parser| parser.split(';').nth(1))
        == Some("native-text/1");
    let mut entries = vec![];
    for (i, sheet) in array(&extraction["sheets"])?.iter().enumerate() {
        let page: Arc<str> = match sheet["page"].as_str() {
            Some(page) => page.into(),
            None => format!("sheet-{}", i + 1).into(),
        };
        let name: Arc<str> = string(&sheet["name"])?.into();
        let entry = |block, field: Option<String>, reason, writeback, target, position| {
            Entry::Default(DefaultEntry {
                page: page.clone(),
                sheet: name.clone(),
                block,
                field: field.map(String::into_boxed_str),
                reason,
                writeback,
                target,
                position,
            })
        };
        let computed_ranges = excel::ComputedRanges::new(sheet)?;
        let merges = excel::Merges::new(&sheet["merges"])?;
        let cells = array(&sheet["cells"])?;
        let mut has_formulas = false;
        for c in cells {
            let address = string(&c["address"])?;
            let (_, row) = excel::coordinate(address)?;
            let column = address.trim_end_matches(|c: char| c.is_ascii_digit());
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
                || c["type"] == "formula"
                || c["type"] == "error";
            let id = string(&c["id"])?;
            entries.push(entry(
                "table-1",
                Some(id.to_owned()),
                reason,
                if excluded { "excluded" } else { "cell" },
                DefaultTarget::Cell(address.into()),
                Some(DefaultPosition {
                    row: format!("r{row}").into(),
                    column: Cow::Owned(column.to_owned()),
                    kind: kind(&c["value"]),
                }),
            ));
            if c["type"] == "formula" {
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
                entries.push(entry(
                    "formulas",
                    Some(format!("{id}-formula")),
                    reason,
                    writeback,
                    DefaultTarget::Cell(address.into()),
                    Some(DefaultPosition {
                        row: id.into(),
                        column: Cow::Borrowed("formula"),
                        kind: "string",
                    }),
                ));
            }
        }
        let mut has_shapes = false;
        for (key, drawing) in shape_texts(sheet)? {
            has_shapes = true;
            entries.push(entry(
                "shapes",
                Some(format!("{key}-text")),
                "図形の文字",
                if native_text { "excluded" } else { "shape" },
                DefaultTarget::Shape(string(&drawing["id"])?.into()),
                Some(DefaultPosition {
                    row: key.into(),
                    column: Cow::Borrowed("text"),
                    kind: "string",
                }),
            ));
        }
        entries.push(entry(
            "extraction-notes",
            None,
            "抽出範囲の申告",
            "excluded",
            DefaultTarget::None,
            None,
        ));
        for (block, present) in [
            ("table-1", !cells.is_empty()),
            ("formulas", has_formulas),
            ("shapes", has_shapes),
        ] {
            if present {
                entries.push(entry(
                    block,
                    None,
                    "表の構造",
                    "excluded",
                    DefaultTarget::None,
                    None,
                ));
            }
        }
    }
    Ok(entries)
}

/// The shapes of an Excel sheet that hold text, by their content key
/// (`shape-<drawing ID>`). A slide's shape text is laid out with the slide's
/// paragraphs instead.
pub(super) fn shape_texts(sheet: &Value) -> Result<Vec<(String, &Value)>> {
    let mut shapes = vec![];
    for drawing in sheet["drawings"].as_array().into_iter().flatten() {
        if drawing["anchor"]["kind"] != "slide"
            && drawing["text"]
                .as_str()
                .is_some_and(|text| !text.is_empty())
        {
            let id = string(&drawing["id"])?;
            let (_, raw) = id
                .rsplit_once('#')
                .context("shape ID without drawing part")?;
            shapes.push((format!("shape-{raw}"), drawing));
        }
    }
    Ok(shapes)
}

/// The complete entries of stored `mappings`: the defaults of `extraction`, each
/// replaced by a stored entry with the same page, block and field, then the stored
/// entries that replace none. Regeneration drops those whose content is gone.
/// The stored entries are taken out of `mappings`, whose `entries` is left
/// empty: the complete entries are returned apart from it.
pub(super) fn with_default_entries(
    mut mappings: Value,
    extraction: &Value,
) -> Result<(Value, Vec<Entry>)> {
    let Value::Array(stored) = mappings["entries"].take() else {
        bail!("mappings entries must be an array");
    };
    mappings["entries"] = json!([]);
    let mut replacements = BTreeMap::new();
    for (index, entry) in stored.iter().enumerate() {
        replacements.entry(entry_key(entry)?).or_insert(index);
    }
    let mut used = vec![false; stored.len()];
    let mut entries = vec![];
    for default in default_entries(extraction)? {
        let key = (
            default.page()?,
            default.block()?,
            default.field().unwrap_or(""),
        );
        match replacements.get(&key).copied() {
            Some(index) => {
                used[index] = true;
                entries.push(Entry::Json(stored[index].clone()));
            }
            None => entries.push(default),
        }
    }
    entries.extend(
        stored
            .into_iter()
            .zip(used)
            .filter_map(|(entry, used)| (!used).then_some(Entry::Json(entry))),
    );
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
    entries: Vec<Entry>,
    extraction: &Value,
    pages: &[Value],
    operations: &[excel::StructuralOperation],
) -> Result<(Value, Vec<Entry>)> {
    let mut positions: BTreeMap<Pair, BTreeMap<Pair, &Value>> = BTreeMap::new();
    let mut fields: BTreeMap<Pair, BTreeSet<&str>> = BTreeMap::new();
    let mut blocks = BTreeSet::new();
    for page in pages {
        let page_id = string(&page["page_id"])?;
        for (block, body) in page["blocks"]
            .as_object()
            .context("content blocks required")?
        {
            let block_key = (page_id, block.as_str());
            blocks.insert(block_key);
            if let Some(values) = body["fields"].as_object() {
                fields
                    .entry(block_key)
                    .or_default()
                    .extend(values.keys().map(String::as_str));
            }
            if let Some(rows) = body["rows"].as_object() {
                let table = positions.entry(block_key).or_default();
                for (row, values) in rows {
                    for (column, value) in values.as_object().context("invalid table row")? {
                        table.insert((row.as_str(), column.as_str()), value);
                    }
                }
            }
        }
    }
    let mut keep = vec![];
    let mut used_fields = BTreeSet::new();
    let mut mapped_positions = BTreeSet::new();
    for entry in &entries {
        let page = entry.page()?;
        let block = entry.block()?;
        let kept = if let Some(position) = entry.position()? {
            let cell = (position.row, position.column);
            let exists = positions
                .get(&(page, block))
                .is_some_and(|values| values.contains_key(&cell));
            if exists {
                mapped_positions.insert((page, block, cell.0, cell.1));
            }
            exists
        } else if entry.field_is_null() {
            blocks.contains(&(page, block))
        } else {
            let field = entry.field().context("expected string")?;
            fields
                .get(&(page, block))
                .is_some_and(|values| values.contains(field))
        };
        if kept && !entry.field_is_null() {
            used_fields.insert(entry.field().context("expected string")?);
        }
        keep.push(kept);
    }
    let mut generated = vec![];
    let mut generated_fields = BTreeSet::new();
    for ((page, block), values) in positions {
        let sheet = page_sheet(extraction, page)?;
        for ((row, column), value) in values {
            if mapped_positions.contains(&(page, block, row, column)) {
                continue;
            }
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
                (None, None) => bail!(
                    "{row}/{column} is not a cell of the original: edit only cells that hold a value, and name new rows and columns <operation ID>-<n> after the insert_rows or insert_columns operation that adds them"
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
            generated.push(Entry::Json(json!({
                "page":page,
                "block":block,
                "field":field,
                "reason":"structural content generated mapping",
                "target":target,
                "writeback":"cell",
                "position":{"row":row,"column":column,"type":kind(value)}
            })));
        }
    }
    let mut kept: Vec<Entry> = entries
        .into_iter()
        .zip(keep)
        .filter_map(|(entry, keep)| keep.then_some(entry))
        .collect();
    kept.extend(generated);
    Ok((regenerated, kept))
}

/// The lists of mappings.yml, left out when empty.
const LISTS: [&str; 3] = ["entries", "omissions", "operations"];

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
    extraction: &'a Value,
    operations: &Value,
) -> Result<Cow<'a, Value>> {
    let operations = array(operations)?;
    if !operations.iter().any(is_slide_operation) {
        return Ok(Cow::Borrowed(extraction));
    }
    let sheets = array(&extraction["sheets"])?;
    let parsed = parse_slide_operations(operations, sheets)?;
    let mut view = extraction.clone();
    let layouts = extraction["slide_layouts"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    view["sheets"] = Value::Array(slide_view(sheets, layouts, &parsed)?);
    Ok(Cow::Owned(view))
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
