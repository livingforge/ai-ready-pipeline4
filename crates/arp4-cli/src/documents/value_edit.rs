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
        let mut plan = match self.plan_values(&dir, &inspected, &book, request, &all) {
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
                            self.plan_values(&dir, &inspected, &book, request, &[index])
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
    ) -> Result<PlannedValues> {
        let edits = array(&request["edits"])?;
        let extraction = &inspected.extraction;
        let workbook = identity::is_excel(extraction);
        let current = identity::load(dir, extraction, &Planned::new())?;
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
        // The edits changing their value or font, the ones mappings records.
        let mut changing = Vec::new();
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
                        .is_ok_and(|w| matches!(w, "cell" | "formula"))
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
                            return Err(refuse(
                                "target_not_found",
                                format!(
                                    "{sheet}!{cell} holds no value the document can edit: it lies outside the rows and columns of the sheet, an operation deletes it, or the format writes back only the text extracted; name the cells of inserted rows and slides by element, as elements-read gives them"
                                ),
                            ));
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
                        format!("{sheet} has no editable text of shape {shape}"),
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
                    return Err(Rejection(json!({"code":"before_mismatch","message":"before is not the current value; read the content again","edit":index,"target":refusal_target(edit),"current":present})).into());
                }
                let after = &edit["after"];
                if !same_value(&present, after) {
                    changing.push(index);
                }
                let expected = match (writeback, entry) {
                    (Some("formula"), _) => (!after
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
                    if slot.entry.is_none() {
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
        let unchanged = planned.iter().all(|(name, bytes)| {
            bytes
                .as_deref()
                .is_some_and(|bytes| holds(dir, name, bytes))
        });
        if unchanged {
            return Ok(PlannedValues {
                planned,
                checked: None,
                verification: json!({}),
                unchanged,
                new_cells: 0,
            });
        }
        // Edits giving a value or font its current one are not recorded.
        if !changing.is_empty() {
            let mappings_path = dir.join("mappings.yml");
            let mut mappings = read_mappings(&mappings_path)?;
            mappings["value_edits"]
                .as_array_mut()
                .context("value edits record")?
                .push(json!({"actor":request["actor"],"reason":request["reason"],
                    "edits":changing.iter().map(|&i| edits[i].clone()).collect::<Vec<_>>()}));
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
        Ok(PlannedValues {
            planned,
            checked: Some(checked),
            verification,
            unchanged,
            new_cells: new_cells.values().map(Vec::len).sum(),
        })
    }
}

/// The target an edit names, as a refusal gives it.
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
