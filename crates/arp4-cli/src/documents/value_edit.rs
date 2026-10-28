use super::sheet_edit::rejection;
use super::*;
use crate::fonts::FontEdit;
use std::collections::HashMap;

/// The most edits of a refused request tried one at a time, to name those
/// the original's writer refuses.
const ISOLATED_EDITS: usize = 32;

/// Where an edit writes: a value of the grid of a content page, by its block
/// and the keys of its row and column, and the mapping entry writing it
/// back, none for a cell the content holds no value for yet.
struct Slot {
    page: String,
    file: String,
    block: String,
    row: String,
    column: String,
    entry: Option<usize>,
}

/// What the edits of a request leave: the files to write, the document as
/// they leave it, and what the original's writer reported writing them.
struct PlannedValues {
    planned: Planned,
    /// The document as the edits leave it; none when they change nothing.
    checked: Option<Inspection>,
    verification: Value,
    /// The edits whose text an estimate shows cut off, or growing a row.
    layout_warnings: Vec<Value>,
    unchanged: bool,
    new_cells: usize,
}

/// A refusal of the edit at `index` of the request, naming its target.
fn refusal(code: &str, index: usize, edit: &Value, message: impl Into<String>) -> anyhow::Error {
    Rejection(
        json!({"code":code,"message":message.into(),"edit":index,"target":refusal_target(edit)}),
    )
    .into()
}

/// Whether the content value `current` is `expected`, numbers as numbers.
fn same_value(current: &Value, expected: &Value) -> bool {
    current == expected
        || (current.is_number() && expected.is_number() && current.as_f64() == expected.as_f64())
}

/// Whether `bytes` are those of the file `name` of `dir`, line endings aside.
fn holds(dir: &Path, name: &str, bytes: &[u8]) -> bool {
    fs::read(dir.join(name)).is_ok_and(|current| {
        current == bytes
            || String::from_utf8(current)
                .is_ok_and(|text| text.replace("\r\n", "\n").as_bytes() == bytes)
    })
}

impl Store {
    /// Edits values, formulas, shape texts and fonts of an adopted document,
    /// or of the proposal of `id`, and gives values to workbook cells the
    /// original leaves empty, as the `value-edits` contract describes, with
    /// the operations recorded before them. The request is recorded in
    /// mappings.yml with the content it changes.
    pub fn edit_values(
        &self,
        id: &str,
        proposal: bool,
        request: &Value,
        dry_run: bool,
    ) -> Result<Value> {
        self.edit_values_with(id, proposal, request, dry_run, &[])
    }

    /// [`Store::edit_values`] recording the sheet `operations` with the
    /// edits, both checked and written together.
    pub(super) fn edit_values_with(
        &self,
        id: &str,
        proposal: bool,
        request: &Value,
        dry_run: bool,
        operations: &[Value],
    ) -> Result<Value> {
        validate("value-edits", request)
            .map_err(|error| rejection("invalid_request", format!("{error:#}")))?;
        let dir = self.structure_dir(id, proposal)?;
        let inspected = self.inspect(&dir, false).map_err(|error| {
            rejection(
                "invalid_document",
                format!("the document fails check before the edit; fix that first: {error:#}"),
            )
        })?;
        if !inspected.source_current {
            return Err(rejection(
                "source_changed",
                "the original changed or is missing; re-import it before editing values",
            ));
        }
        if request["base"] != inspected.fingerprint.as_str()
            || request["source_sha256"] != inspected.meta["source"]["sha256"]
        {
            return Err(rejection(
                "base_changed",
                "base or source_sha256 is not the document's current content or original; read check --include-hashes again and plan the edits on the current content",
            ));
        }
        let source = under(&self.root, string(&inspected.meta["source"]["path"])?)?;
        let book = Source::open(&source)?;
        if book.parser() == "native-text/1" {
            return Err(rejection(
                "unsupported_format",
                "text documents are edited in the original and imported again",
            ));
        }
        let edits = array(&request["edits"])?;
        let all: Vec<usize> = (0..edits.len()).collect();
        let mut plan = match self.plan_values(&dir, &inspected, &book, request, &all, operations) {
            Ok(plan) => plan,
            Err(error) => {
                // Name the edits the writer refuses on their own.
                let refused = crate::data::rejection(&error)
                    .is_some_and(|payload| payload["code"] == "writeback_refused");
                if refused && edits.len() > 1 && edits.len() <= ISOLATED_EDITS {
                    let failing: Vec<usize> = all
                        .iter()
                        .copied()
                        .filter(|&index| {
                            self.plan_values(&dir, &inspected, &book, request, &[index], operations)
                                .is_err()
                        })
                        .collect();
                    let mut payload = crate::data::rejection(&error).unwrap().clone();
                    payload["edits"] = json!(failing);
                    return Err(Rejection(payload).into());
                }
                return Err(error);
            }
        };
        ensure!(
            hash(&fs::read(&source)?) == request["source_sha256"],
            "source changed during value edit"
        );
        let state = if plan.unchanged {
            "unchanged"
        } else if dry_run {
            "planned"
        } else {
            self.commit(&dir, &plan.planned, &inspected.fingerprint)?;
            "written"
        };
        let content = plan
            .checked
            .as_ref()
            .map_or(&inspected.fingerprint, |checked| &checked.fingerprint);
        let mut result = json!({"state":state,"content":content,"edits":edits.len(),"verification":plan.verification,"layout_status":"unconfirmed"});
        if plan.new_cells > 0 {
            result["new_cells"] = json!(plan.new_cells);
        }
        if !plan.layout_warnings.is_empty() {
            result["layout_warnings"] = json!(plan.layout_warnings);
        }
        // The written content was inspected when planned; its status is made
        // from that inspection rather than by inspecting the document again.
        if state == "written"
            && let Some(checked) = plan.checked.take()
        {
            let row = self.state_of(id, &dir, Ok(checked));
            self.remember_state(id, &dir, &row);
        }
        Ok(result)
    }

    /// Plans the edits at `indices` of `request` on the document in `dir`:
    /// the content pages, layout and mappings they leave, checked like
    /// `check`, and written by the original's writer to a scratch file with
    /// every other pending change, as export writes them.
    fn plan_values(
        &self,
        dir: &Path,
        inspected: &Inspection,
        book: &Source,
        request: &Value,
        indices: &[usize],
        operations: &[Value],
    ) -> Result<PlannedValues> {
        let edits = array(&request["edits"])?;
        let extraction = &inspected.extraction;
        let workbook = identity::is_excel(extraction);
        let mut current = identity::load(dir, extraction, &Planned::new())?;
        // Cells past the rows and columns the extraction's cells reach take
        // values too, given the identities they lack.
        let mut reached: Vec<(String, String)> = vec![];
        if workbook {
            let page_of: BTreeMap<String, String> = current
                .sheets
                .iter()
                .map(|(page, sheet)| Ok((string(&sheet["name"])?.to_owned(), page.clone())))
                .collect::<Result<_>>()?;
            for &index in indices {
                let edit = &edits[index];
                let (Some(sheet), Some(cell)) = (edit["sheet"].as_str(), edit["cell"].as_str())
                else {
                    continue;
                };
                let (Some(page), Ok(_)) = (page_of.get(sheet), excel::coordinate(cell)) else {
                    continue;
                };
                let lines = identity::KeyLines::new(current.sheet(page)?);
                let (column, row) = excel::coordinate(cell)?;
                if lines.get("rows", &format!("r{row}")).is_some()
                    && lines.get("columns", &excel::column_name(column)?).is_some()
                {
                    continue;
                }
                if identity::reach_cell(&mut current, page, cell)? {
                    reached.push((page.clone(), cell.to_owned()));
                }
            }
        }
        let current = current;
        let originals: BTreeSet<&str> = extraction
            .sheets
            .iter()
            .filter_map(|sheet| sheet["name"].as_str())
            .collect();
        // The pages by sheet name, and the entries by what they write.
        let mut pages_by_name = BTreeMap::new();
        for (page, sheet) in &current.sheets {
            pages_by_name.insert(string(&sheet["name"])?, page.as_str());
        }
        let mut positioned: HashMap<(&str, &str, &str, &str), usize> = HashMap::new();
        let mut targeted: HashMap<(&str, &str), Vec<usize>> = HashMap::new();
        let mut shaped: HashMap<(&str, &str), usize> = HashMap::new();
        for (index, entry) in inspected.entries().enumerate() {
            if let Some(position) = entry.position()? {
                positioned.insert(
                    (entry.page()?, entry.block()?, position.row, position.column),
                    index,
                );
            }
            match entry.target()? {
                Some(MappingTarget::Cell { sheet, cell }) => {
                    targeted.entry((sheet, cell)).or_default().push(index)
                }
                Some(MappingTarget::Shape { sheet, shape }) if entry.writeback()? == "shape" => {
                    shaped.insert((sheet, shape), index);
                }
                _ => {}
            }
        }
        let extracted: std::collections::HashSet<(&str, &str)> = extraction
            .sheets
            .iter()
            .flat_map(|sheet| {
                let name = sheet["name"].as_str().unwrap_or_default();
                sheet.cells.iter().map(move |cell| (name, cell.address()))
            })
            .collect();
        let mut pages: BTreeMap<String, Value> = BTreeMap::new();
        // The blocks of the pages whose values were cleared, which leave out
        // the rows holding no value once all edits are made.
        let mut emptied = BTreeSet::new();
        let mut written = BTreeSet::new();
        let mut new_cells: BTreeMap<String, Vec<String>> = BTreeMap::new();
        // The formulas given to cells holding none, by page, row and column
        // identity, and those of them cleared, by page and key.
        let mut new_formulas: Vec<(String, String, String)> = vec![];
        let mut dropped_formulas: Vec<(String, String)> = vec![];
        // The formulas given, by page and key, with the operation each is
        // written after.
        let mut framed: Vec<(String, String, Option<String>)> = vec![];
        // The edits changing their value or font, the ones mappings records.
        let mut changing = Vec::new();
        // Where each edit writes, by page, block, row and column.
        let mut places = BTreeMap::new();
        // What the edits look up in each page's layout and each sheet's
        // merges, read once for all of them.
        let mut lookups: HashMap<&str, identity::SheetLookup<'_>> = HashMap::new();
        let mut merged: HashMap<&str, Option<excel::Merges<'_>>> = HashMap::new();
        for &index in indices {
            let edit = &edits[index];
            let sheet = string(&edit["sheet"])?;
            let refuse = |code: &str, message: String| refusal(code, index, edit, message);
            let Some(&page) = pages_by_name.get(sheet) else {
                return Err(Rejection(json!({"code":"sheet_not_found","message":format!("the document has no sheet or page {sheet}"),"edit":index,"sheets":pages_by_name.keys().collect::<Vec<_>>()})).into());
            };
            let layout_sheet = current.sheet(page)?;
            let lookup = lookups
                .entry(page)
                .or_insert_with(|| identity::SheetLookup::new(layout_sheet, page));
            let file = inspected
                .page_files
                .get(page)
                .with_context(|| format!("{sheet} has no content page"))?;
            let slot_of = |block: &str, row: &str, column: &str, entry: Option<usize>| Slot {
                page: page.to_owned(),
                file: file.clone(),
                block: block.to_owned(),
                row: row.to_owned(),
                column: column.to_owned(),
                entry,
            };
            let from_entry = |index: usize| -> Result<Slot> {
                let entry = inspected.entry(index);
                let position = entry.position()?.context("entry without position")?;
                Ok(slot_of(
                    entry.block()?,
                    position.row,
                    position.column,
                    Some(index),
                ))
            };
            let slot = if let Some(cell) = edit["cell"].as_str() {
                let found = targeted
                    .get(&(sheet, cell))
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                let editable = found.iter().copied().find(|&i| {
                    inspected
                        .entry(i)
                        .writeback()
                        .is_ok_and(|w| matches!(w, "cell" | "formula" | "new_formula"))
                });
                match (editable, found.first()) {
                    (Some(entry), _) => from_entry(entry)?,
                    (None, Some(&entry)) => {
                        return Err(refuse(
                            "not_editable",
                            format!(
                                "{sheet}!{cell} is not written back: {}",
                                inspected.entry(entry).reason()?
                            ),
                        ));
                    }
                    (None, None) => {
                        let (column, row) = excel::coordinate(cell)?;
                        let (row, column) = (format!("r{row}"), excel::column_name(column)?);
                        let known = lookup.identity("rows", &row).is_some()
                            && lookup.identity("columns", &column).is_some();
                        if !workbook || !originals.contains(sheet) || !known {
                            let mut error = json!({"code":"target_not_found","message":format!(
                                "{sheet}!{cell} holds no value the document can edit: it lies outside the rows and columns of the sheet, an operation deletes it, or the format writes back only the text extracted; name the cells of inserted rows and slides by element, as elements-read gives them"
                            ),"edit":index,"target":refusal_target(edit)});
                            if workbook
                                && let Some(moved) = moved_cell(sheet, cell, &current.operations)?
                            {
                                error["moved"] = moved;
                            }
                            return Err(Rejection(error).into());
                        }
                        if !merged.contains_key(sheet) {
                            let merges = extraction
                                .sheets
                                .iter()
                                .find(|s| s["name"] == sheet)
                                .map(|s| excel::Merges::new(&s["merges"]))
                                .transpose()?;
                            merged.insert(sheet, merges);
                        }
                        if let Some(range) = merged[sheet]
                            .as_ref()
                            .map(|merges| merges.hiding(cell))
                            .transpose()?
                            .flatten()
                        {
                            return Err(refuse(
                                "not_editable",
                                format!(
                                    "{sheet}!{cell} is hidden by the merged range {range}; Excel shows only its top-left cell, so write the value there"
                                ),
                            ));
                        }
                        slot_of("table-1", &row, &column, None)
                    }
                }
            } else if let Some(shape) = edit["shape"].as_str() {
                let entry = shaped.get(&(sheet, shape)).copied().ok_or_else(|| {
                    refuse(
                        "target_not_found",
                        format!("{sheet} has no editable text of shape {shape}: it holds no text, or a row or column deletion takes it with the cells holding it, as Excel does"),
                    )
                })?;
                from_entry(entry)?
            } else {
                let element = string(&edit["element"])?;
                let (block, row, column) = lookup
                    .binding(element)?
                    .map_err(|error| refuse("target_not_found", format!("{error:#}")))?;
                let Some((row_key, column_key)) = lookup.place(&block, &row, &column)? else {
                    return Err(refuse(
                        "target_not_found",
                        format!(
                            "{sheet} has no row or column of element {element}: an operation deletes it, or it names no place of the page"
                        ),
                    ));
                };
                match positioned
                    .get(&(page, block.as_str(), row_key.as_str(), column_key.as_str()))
                    .copied()
                {
                    Some(entry) => from_entry(entry)?,
                    None => {
                        let inserted = row_key.contains('-') || column_key.contains('-');
                        if block != "table-1" || !(workbook || inserted) {
                            return Err(refuse(
                                "target_not_found",
                                format!(
                                    "element {element} of {sheet} holds no text the document can edit"
                                ),
                            ));
                        }
                        slot_of(&block, &row_key, &column_key, None)
                    }
                }
            };
            // A formula given to a cell holding none: the cell becomes a
            // formula of the page, written at its place.
            let mut slot = slot;
            let written_after = edit["written_after"].as_str();
            if let Some(frame) = written_after {
                if edit["formula"] != true && slot.block != "formulas" {
                    return Err(refuse(
                        "invalid_request",
                        "written_after goes with a formula: one given with formula: true, or the edit of an existing formula".into(),
                    ));
                }
                if !current
                    .operations
                    .iter()
                    .any(|operation| operation.id == frame)
                {
                    return Err(refuse(
                        "invalid_request",
                        format!(
                            "written_after {frame} names no row, column or cell operation recorded"
                        ),
                    ));
                }
            }
            if edit["formula"] == true {
                if !workbook {
                    return Err(refuse(
                        "invalid_request",
                        "formula applies to the cells of an Excel workbook".into(),
                    ));
                }
                let formula = edit["after"].as_str().filter(|text| text.starts_with('='));
                let Some(formula) = formula else {
                    return Err(refuse(
                        "value_type_mismatch",
                        "after must be a formula, text starting with =".into(),
                    ));
                };
                if let Err(error) = excel::check_formula(&formula[1..]) {
                    return Err(refuse("value_type_mismatch", format!("{error:#}")));
                }
                if slot.block == "table-1" {
                    let entry = slot.entry.map(|index| inspected.entry(index));
                    let original = match entry.map(EntryRef::target).transpose()?.flatten() {
                        Some(MappingTarget::Cell { sheet, cell }) => {
                            extracted.contains(&(sheet, cell))
                        }
                        Some(MappingTarget::Shape { .. }) => true,
                        _ => false,
                    };
                    if original {
                        return Err(refuse(
                            "not_editable",
                            "a new formula goes into a cell the original leaves empty, or one an operation inserts; replace an existing formula without formula, and give a value cell its formula in Excel".into(),
                        ));
                    }
                    let (Some(row), Some(column)) = (
                        lookup.identity("rows", &slot.row),
                        lookup.identity("columns", &slot.column),
                    ) else {
                        return Err(refuse(
                            "target_not_found",
                            format!("{sheet} has no row or column identity for the cell"),
                        ));
                    };
                    let key = identity::formula_name(page, &slot.row, &slot.column);
                    new_formulas.push((page.to_owned(), row, column));
                    if !pages.contains_key(&slot.file) {
                        pages.insert(
                            slot.file.clone(),
                            self.read_content(dir, &slot.file, extraction)?,
                        );
                    }
                    let content = pages.get_mut(&slot.file).unwrap();
                    // A value given to the cell before goes; the formula takes its place.
                    if let Some(values) = content
                        .pointer_mut(&format!("/blocks/table-1/rows/{}", slot.row))
                        .and_then(Value::as_object_mut)
                    {
                        values.remove(&slot.column);
                    }
                    if content["blocks"]["formulas"].is_null() {
                        content["blocks"]["formulas"] = json!({"title":"数式原文","rows":{}});
                    }
                    slot.block = "formulas".into();
                    slot.row = key;
                    slot.column = "formula".into();
                    slot.entry = None;
                }
            }
            // A formula edited is written as the cells stand after the
            // operation it names, or as the original's places give them.
            if slot.block == "formulas" && edit.get("after").is_some() {
                framed.push((
                    slot.page.clone(),
                    slot.row.clone(),
                    written_after.map(str::to_owned),
                ));
            }
            places.insert(
                index,
                (
                    slot.page.clone(),
                    slot.block.clone(),
                    slot.row.clone(),
                    slot.column.clone(),
                ),
            );
            if !written.insert((
                slot.page.clone(),
                slot.block.clone(),
                slot.row.clone(),
                slot.column.clone(),
            )) {
                return Err(refuse(
                    "duplicate_target",
                    "another edit of the request names the same value; give its value and its font in one edit".into(),
                ));
            }
            let entry = slot.entry.map(|index| inspected.entry(index));
            let writeback = entry.map(EntryRef::writeback).transpose()?;
            if let (Some(entry), Some("excluded" | "pending")) = (entry, writeback) {
                return Err(refuse(
                    "not_editable",
                    format!("the value is not written back: {}", entry.reason()?),
                ));
            }
            if !pages.contains_key(&slot.file) {
                pages.insert(
                    slot.file.clone(),
                    self.read_content(dir, &slot.file, extraction)?,
                );
            }
            let content = pages.get_mut(&slot.file).unwrap();
            if edit.get("before").is_some() {
                let present =
                    content["blocks"][&slot.block]["rows"][&slot.row][&slot.column].clone();
                if !same_value(&present, &edit["before"]) {
                    let mut error = json!({"code":"before_mismatch","message":"before is not the current value; read the content again","edit":index,"target":refusal_target(edit),"current":present});
                    if let Some(cell) = edit["cell"].as_str()
                        && let Some(moved) = moved_cell(sheet, cell, &current.operations)?
                    {
                        error["moved"] = moved;
                    }
                    return Err(Rejection(error).into());
                }
                let after = &edit["after"];
                if !same_value(&present, after) {
                    changing.push(index);
                }
                let expected = match (writeback, entry) {
                    // A new formula is cleared back to an empty cell.
                    (Some("new_formula"), _) if after.is_null() => None,
                    (Some("formula" | "new_formula"), _) => (!after
                        .as_str()
                        .is_some_and(|text| text.starts_with('=')))
                    .then_some(
                        "a formula, text starting with = as the cell's formula is; the cell shows its result",
                    ),
                    (Some("shape"), _) => (!after.is_string()).then_some("text"),
                    _ if !workbook => (!after.is_string()).then_some("text; give \"\" to clear it"),
                    (Some(_), Some(entry)) => {
                        let kind = entry.position()?.context("entry without position")?.kind;
                        (!after.is_null() && super::kind(after) != kind).then_some(match kind {
                            "number" => "a number, as the cell holds",
                            "boolean" => "true or false, as the cell holds",
                            _ => "text, as the cell holds; quote digits to keep them text",
                        })
                    }
                    _ => None,
                };
                if let Some(expected) = expected {
                    return Err(Rejection(json!({"code":"value_type_mismatch","message":format!("after must be {expected}"),"edit":index,"target":refusal_target(edit),"after":after})).into());
                }
                // A cell holding no value in the original, or one an operation
                // inserts, is left empty again when its value is cleared.
                let holds_none = match entry.map(EntryRef::target).transpose()?.flatten() {
                    None => true,
                    Some(MappingTarget::Cell { sheet, cell }) => {
                        !extracted.contains(&(sheet, cell))
                    }
                    Some(MappingTarget::Shape { .. }) => false,
                    Some(_) => true,
                };
                if after.is_null() && holds_none && slot.block == "formulas" {
                    // A new formula cleared leaves its cell empty again.
                    dropped_formulas.push((slot.page.clone(), slot.row.clone()));
                }
                if after.is_null() && holds_none {
                    for pointer in [
                        format!("/blocks/{}/rows/{}", slot.block, slot.row),
                        format!("/fonts/{}/{}", slot.block, slot.row),
                    ] {
                        if let Some(values) =
                            content.pointer_mut(&pointer).and_then(Value::as_object_mut)
                        {
                            values.remove(&slot.column);
                        }
                    }
                    emptied.insert((slot.file.clone(), slot.block.clone()));
                } else {
                    if slot.entry.is_none() && slot.block == "formulas" {
                        let (row, column) = new_formulas
                            .last()
                            .map(|(_, row, column)| (row.clone(), column.clone()))
                            .context("new formula without its cell")?;
                        new_cells
                            .entry(slot.page.clone())
                            .or_default()
                            .push(elements::cell_id(&row, &column));
                    } else if slot.entry.is_none() {
                        let id = elements::cell_id(
                            &lookup
                                .identity("rows", &slot.row)
                                .unwrap_or_else(|| slot.row.clone()),
                            &lookup
                                .identity("columns", &slot.column)
                                .unwrap_or_else(|| slot.column.clone()),
                        );
                        new_cells.entry(slot.page.clone()).or_default().push(id);
                        if content["blocks"][&slot.block].is_null() {
                            content["blocks"][&slot.block] = json!({"title":"本文","rows":{}});
                        }
                    }
                    content["blocks"][&slot.block]["rows"][&slot.row][&slot.column] = after.clone();
                }
            }
            if edit.get("font").is_some() {
                let target = entry.map(EntryRef::target).transpose()?.flatten();
                let original = match &target {
                    Some(
                        MappingTarget::Cell { sheet, .. } | MappingTarget::Shape { sheet, .. },
                    ) => originals.contains(sheet),
                    _ => false,
                };
                let font = match slot.entry {
                    Some(index) if original && book.font_edits_supported() => inspected
                        .content
                        .get(index)
                        .and_then(|field| field.font.as_deref()),
                    _ => None,
                };
                let Some(font) = font else {
                    return Err(refuse(
                        "font_unsupported",
                        "fonts are edited on the cells and shape texts of an Excel original and the paragraphs and table cells of a Word or PowerPoint original; set the fonts of other text in Office".into(),
                    ));
                };
                let before = edit["font"]["before"].as_object().context("font before")?;
                let after = edit["font"]["after"].as_object().context("font after")?;
                if !before.keys().eq(after.keys()) {
                    return Err(refuse(
                        "invalid_request",
                        "font before must give the current value of each property after changes"
                            .into(),
                    ));
                }
                if let Some(property) = before
                    .iter()
                    .find(|(property, value)| !crate::fonts::same(&font[property.as_str()], value))
                    .map(|(property, _)| property)
                {
                    return Err(Rejection(json!({"code":"before_mismatch","message":format!("font before {property} is not the current font"),"edit":index,"target":refusal_target(edit),"current":font})).into());
                }
                let change = FontEdit::parse(&edit["font"]["after"])
                    .map_err(|error| refuse("invalid_request", format!("{error:#}")))?;
                if matches!(book, Source::Excel(_)) && edit.get("shape").is_none() {
                    crate::fonts::excel_name(&change)
                        .map_err(|error| refuse("invalid_request", format!("{error:#}")))?;
                }
                let applied = change.applied(font);
                if !crate::fonts::same_font(&applied, font) && changing.last() != Some(&index) {
                    changing.push(index);
                }
                elements::set_grid_font(content, &slot.block, &slot.row, &slot.column, applied);
            }
        }
        for (file, block) in emptied {
            if let Some(rows) = pages
                .get_mut(&file)
                .and_then(|content| content.pointer_mut(&format!("/blocks/{block}/rows")))
                .and_then(Value::as_object_mut)
            {
                rows.retain(|_, values| values.as_object().is_none_or(|v| !v.is_empty()));
            }
        }
        let mut layout = identity::load(dir, extraction, &Planned::new())?;
        for (page, cell) in &reached {
            identity::reach_cell(&mut layout, page, cell)?;
        }
        for (page, row, column) in &new_formulas {
            identity::add_formula(&mut layout, page, row, column)?;
        }
        for (page, key, written_after) in &framed {
            if !dropped_formulas.iter().any(|(p, k)| p == page && k == key) {
                identity::frame_formula(&mut layout, page, key, written_after.as_deref())?;
            }
        }
        for (page, key) in &dropped_formulas {
            identity::drop_formula(&mut layout, page, key)?;
        }
        // The groups of the pages that take new cells, by page.
        let mut page_groups = BTreeMap::new();
        for (page, cells) in &new_cells {
            let sheet = layout.sheet(page)?;
            let mut groups = elements::metadata_groups(&sheet.metadata)?;
            elements::adopt_cells(sheet, &mut groups, cells)?;
            page_groups.insert(page.clone(), groups);
        }
        let mut planned = Planned::new();
        for (name, page) in pages {
            let groups = page_groups.remove(string(&page["page_id"])?);
            let body = elements::encode(page, &mut layout, groups)?;
            planned.insert(name, Some(elements::content_bytes(&body, &layout)?));
        }
        planned.insert(
            "layout.yml".into(),
            Some(identity::stored(layout, extraction)?),
        );
        let unchanged = operations.is_empty()
            && planned.iter().all(|(name, bytes)| {
                bytes
                    .as_deref()
                    .is_some_and(|bytes| holds(dir, name, bytes))
            });
        if unchanged {
            return Ok(PlannedValues {
                planned,
                checked: None,
                verification: json!({}),
                layout_warnings: vec![],
                unchanged,
                new_cells: 0,
            });
        }
        // Edits giving a value or font its current one are not recorded.
        if !changing.is_empty() || !operations.is_empty() {
            let mappings_path = dir.join("mappings.yml");
            let mut mappings = read_mappings(&mappings_path)?;
            if !changing.is_empty() {
                mappings["value_edits"]
                    .as_array_mut()
                    .context("value edits record")?
                    .push(json!({"actor":request["actor"],"reason":request["reason"],
                        "edits":changing.iter().map(|&i| edits[i].clone()).collect::<Vec<_>>()}));
            }
            mappings["operations"]
                .as_array_mut()
                .context("operations record")?
                .extend(operations.iter().cloned());
            planned.insert(
                "mappings.yml".into(),
                Some(stored_mappings(&mappings_path, &mappings)?),
            );
        }
        let checked = self
            .inspect_with(dir, false, &planned)
            .map_err(|error| rejection("validation_failed", format!("{error:#}")))?;
        // The original's writer writes the edits with every other pending
        // change, as export will, before any managed file changes.
        let writeback = self
            .writeback(dir, &checked, book)
            .map_err(|error| rejection("writeback_refused", format!("{error:#}")))?;
        let stage = Stage::new_in(&under(&self.arp, "cache/export")?, &self.arp)?;
        let source = under(&self.root, string(&inspected.meta["source"]["path"])?)?;
        let preview = stage
            .path()
            .join(source.file_name().context("missing filename")?);
        let verification = book
            .patch(&preview, &writeback.edits(&checked)?)
            .map_err(|error| rejection("writeback_refused", format!("{error:#}")))?;
        let layout_warnings = if matches!(book, Source::Excel(_)) {
            // Each changed text at its place in the written workbook: the
            // edit's value, its entry's field, and the change of that field.
            let mut fields = HashMap::new();
            for entry in checked.entries() {
                if let Some(position) = entry.position()? {
                    fields.insert(
                        (entry.page()?, entry.block()?, position.row, position.column),
                        entry.json()["field"].as_str().map(str::to_owned),
                    );
                }
            }
            let changes: HashMap<&str, &Value> = writeback
                .changes
                .iter()
                .filter_map(|change| Some((change["field"].as_str()?, change)))
                .collect();
            let mut targets = vec![];
            for &index in &changing {
                let edit = &edits[index];
                let (Some(after), Some((page, block, row, column))) =
                    (edit["after"].as_str(), places.get(&index))
                else {
                    continue;
                };
                // A formula shows its result, which Excel calculates.
                if block == "formulas" {
                    continue;
                }
                let Some(change) = fields
                    .get(&(page.as_str(), block.as_str(), row.as_str(), column.as_str()))
                    .and_then(|field| field.as_deref())
                    .and_then(|field| changes.get(field))
                else {
                    continue;
                };
                let place = match (change["cell"].as_str(), change["shape"].as_str()) {
                    (Some(cell), _) => excel::Place::Cell(cell),
                    (None, Some(shape)) => excel::Place::Shape(shape),
                    (None, None) => continue,
                };
                targets.push(excel::FitTarget {
                    edit: index,
                    sheet: string(&change["sheet"])?,
                    place,
                    before: edit["before"].as_str(),
                    after,
                });
            }
            if targets.is_empty() {
                vec![]
            } else {
                excel::layout_warnings(&excel::Workbook::open(&preview)?, &targets)?
            }
        } else {
            vec![]
        };
        Ok(PlannedValues {
            planned,
            checked: Some(checked),
            verification,
            layout_warnings,
            unchanged,
            new_cells: new_cells.values().map(Vec::len).sum(),
        })
    }
}

/// The target an edit names, as a refusal gives it.
/// Where the original cell `cell` of `sheet` is shown with the pending row,
/// column and cell `operations`, and which cell is shown at that address,
/// when either differs: `cell` names the original's address, while export,
/// `format` and `cells` name the places the operations leave.
fn moved_cell(
    sheet: &str,
    cell: &str,
    operations: &[excel::StructuralOperation],
) -> Result<Option<Value>> {
    if !operations.iter().any(|operation| operation.sheet == sheet) {
        return Ok(None);
    }
    let shown = excel::map_coordinate(sheet, cell, operations)?;
    let at = match excel::origin_of(sheet, excel::coordinate(cell)?, operations)? {
        Some(excel::Origin::Original(address)) => json!({"cell":address}),
        Some(excel::Origin::Inserted {
            operation, offset, ..
        }) => json!({"inserted_by":operation,"offset":offset}),
        None => json!({"inserted":true}),
    };
    if shown.as_deref() == Some(cell) && at["cell"] == cell {
        return Ok(None);
    }
    Ok(Some(
        json!({"shown_at":shown,"address_shows":at,"hint":"cell names the address of the original, as extraction-read gives it; the pending operations show the original cell at shown_at (null: deleted), and the address holds what address_shows names. Name the cell by its original address, or by element as elements-read gives it"}),
    ))
}

fn refusal_target(edit: &Value) -> Value {
    let mut target = json!({"sheet":edit["sheet"]});
    for key in ["cell", "shape", "element"] {
        if let Some(value) = edit.get(key) {
            target[key] = value.clone();
        }
    }
    target
}

/// The shapes of each sheet by sheet name and extraction ID.
pub(super) fn shape_drawings(extraction: &Extraction) -> Result<BTreeMap<(&str, &str), &Value>> {
    let mut shapes = BTreeMap::new();
    for sheet in &extraction.sheets {
        for drawing in sheet["drawings"].as_array().into_iter().flatten() {
            shapes.insert((string(&sheet["name"])?, string(&drawing["id"])?), drawing);
        }
    }
    Ok(shapes)
}

/// The font changes to write back: each original cell or shape whose current
/// font (that of its content value) differs from the font extracted from the
/// original, with the properties that changed.
pub(super) fn font_changes(inspected: &Inspection) -> Result<Vec<Value>> {
    let (extraction, fields) = (&inspected.extraction, &inspected.content);
    if fields.values().all(|field| field.font.is_none()) {
        return Ok(vec![]);
    }
    let mut cells = BTreeMap::new();
    for sheet in &extraction.sheets {
        for cell in &sheet.cells {
            cells.insert((string(&sheet["name"])?, cell.address()), (sheet, cell));
        }
    }
    let shapes = shape_drawings(extraction)?;
    let originals: BTreeSet<&str> = extraction
        .sheets
        .iter()
        .filter_map(|sheet| sheet["name"].as_str())
        .collect();
    let mut changes = vec![];
    for (index, entry) in inspected.entries().enumerate() {
        let entry = entry.json();
        let entry = &*entry;
        if !matches!(
            entry["writeback"].as_str(),
            Some("cell" | "formula" | "shape")
        ) {
            continue;
        }
        let Some(current) = fields.get(index).and_then(|field| field.font.as_ref()) else {
            continue;
        };
        // A slide copy has no original page; it takes the fonts of the page it
        // copies, which are written before the copy is made.
        if entry["target"]["sheet"]
            .as_str()
            .is_some_and(|sheet| !originals.contains(sheet))
        {
            continue;
        }
        let (target, original) = match mapping_target(&entry["target"])? {
            Some(MappingTarget::Cell { sheet, cell }) => (json!({"sheet":sheet,"cell":cell}), {
                let (sheet, cell) = cells.get(&(sheet, cell)).context("missing cell")?;
                crate::extraction::font(sheet, *cell)?
            }),
            Some(MappingTarget::Shape { sheet, shape }) => (
                json!({"sheet":sheet,"shape":shape}),
                &shapes
                    .get(&(sheet, shape))
                    .with_context(|| format!("shape {shape} on {sheet} is missing"))?["font"],
            ),
            _ => continue,
        };
        ensure!(
            !original.is_null(),
            "{} has a font the original does not record; re-import the original",
            entry["field"]
        );
        let edit = FontEdit::between(original, current)?;
        if !edit.is_empty() {
            let mut change = target;
            change["before"] = original.clone();
            change["after"] = edit.json();
            change["field"] = entry["field"].clone();
            changes.push(change);
        }
    }
    Ok(changes)
}
