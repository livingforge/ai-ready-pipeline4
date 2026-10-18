//! Listing the pending operations of a document and taking one back. The
//! operation leaves mappings.yml with what it did to the content: the values
//! of the rows, columns and slides it inserted leave the content pages, and
//! those it deleted come back from the extraction. Like an edit, the result
//! is validated like `check` before anything is written.
use super::sheet_edit::rejection;
use super::*;
use crate::document_source::{is_presentation_operation, slide_order};

/// The kinds of operation that place rows, columns and pictures on a sheet
/// by positions the operations before them leave.
const POSITIONED: [&str; 6] = [
    "insert_rows",
    "delete_rows",
    "insert_columns",
    "delete_columns",
    "move_columns",
    "add_image",
];

/// The IDs of the operations of `recorded` that depend on the one at
/// `index`: a later operation placing rows, columns or pictures on its sheet
/// by positions it leaves, and any naming what it adds, by its ID.
fn dependents(recorded: &[Value], index: usize) -> Vec<String> {
    let operation = &recorded[index];
    let id = operation["id"].as_str().unwrap_or_default();
    let names = [id.to_owned(), format!("notes-{id}")];
    let positioned = |o: &Value| {
        o["kind"]
            .as_str()
            .is_some_and(|kind| POSITIONED.contains(&kind))
            && !is_presentation_operation(o)
    };
    recorded
        .iter()
        .enumerate()
        .filter(|&(at, other)| {
            if at == index {
                return false;
            }
            let after_on_sheet = at > index
                && positioned(operation)
                && positioned(other)
                && other["sheet"] == operation["sheet"];
            let naming = other.as_object().is_some_and(|fields| {
                fields.iter().any(|(key, value)| {
                    !matches!(key.as_str(), "id" | "kind" | "reason")
                        && value
                            .as_str()
                            .is_some_and(|text| names.iter().any(|n| n == text))
                })
            });
            after_on_sheet || naming
        })
        .filter_map(|(_, other)| other["id"].as_str().map(str::to_owned))
        .collect()
}

impl Store {
    /// The pending operations of an adopted document, or of the proposal of
    /// `id`, in the order apply writes them.
    pub fn operations(&self, id: &str, proposal: bool) -> Result<Vec<Value>> {
        let dir = self.structure_dir(id, proposal)?;
        Ok(array(&read_mappings(&dir.join("mappings.yml"))?["operations"])?.clone())
    }

    /// Takes the pending operation `operation` of an adopted document, or of
    /// the proposal of `id`, back with what it did to the content.
    pub fn remove_operation(
        &self,
        id: &str,
        proposal: bool,
        operation: &str,
        base: Option<&str>,
        dry_run: bool,
    ) -> Result<Value> {
        let dir = self.structure_dir(id, proposal)?;
        let inspected = self.inspect(&dir, false).map_err(|error| {
            rejection(
                "invalid_document",
                format!("the document fails check before the edit; fix that first: {error:#}"),
            )
        })?;
        if base.is_some_and(|base| base != inspected.fingerprint) {
            return Err(rejection(
                "base_changed",
                "the document changed after --base was read; read check --include-hashes again",
            ));
        }
        if !inspected.source_current {
            return Err(rejection(
                "source_changed",
                "the original changed or is missing; re-import it before editing operations",
            ));
        }
        let extraction = &inspected.extraction;
        let mappings_path = dir.join("mappings.yml");
        let mut mappings = read_mappings(&mappings_path)?;
        let recorded = array(&mappings["operations"])?.clone();
        let Some(index) = recorded.iter().position(|o| o["id"] == operation) else {
            let ids: Vec<&Value> = recorded.iter().map(|o| &o["id"]).collect();
            return Err(Rejection(json!({"code":"operation_not_found","message":format!("{id} has no pending operation {operation}"),"operations":ids})).into());
        };
        let removed = recorded[index].clone();
        let dependents = dependents(&recorded, index);
        if !dependents.is_empty() {
            return Err(Rejection(json!({"code":"operation_has_dependents","message":format!("operations {} depend on {operation}; remove them first, the last first", dependents.join(", ")),"dependents":dependents})).into());
        }
        let mut remaining = recorded.clone();
        remaining.remove(index);
        mappings["operations"] = Value::Array(remaining.clone());
        let kind = string(&removed["kind"])?;
        // The value edits of what the operation added go with it.
        let pages = [operation.to_owned(), format!("notes-{operation}")];
        let added = |edit: &Value| -> bool {
            let sheet = edit["sheet"].as_str().unwrap_or_default();
            match kind {
                "insert_slide" | "add_slide" => pages.iter().any(|page| page == sheet),
                "insert_rows" | "insert_columns" => {
                    removed["sheet"] == sheet
                        && edit["element"]
                            .as_str()
                            .and_then(elements::cell_axes)
                            .is_some_and(|(row, column)| {
                                [row, column].iter().any(|key| {
                                    key.strip_prefix(&format!("{operation}-"))
                                        .is_some_and(|n| n.parse::<u32>().is_ok())
                                })
                            })
                }
                _ => false,
            }
        };
        if let Some(records) = mappings["value_edits"].as_array_mut() {
            for record in records.iter_mut() {
                if let Some(edits) = record["edits"].as_array_mut() {
                    edits.retain(|edit| !added(edit));
                }
            }
            records.retain(|record| record["edits"].as_array().is_some_and(|e| !e.is_empty()));
        }
        let sheets = array(&extraction["sheets"])?;
        let before = operated_extraction(extraction, &Value::Array(recorded.clone()))?;
        let after = operated_extraction(extraction, &Value::Array(remaining.clone()))?;
        let page_of = |view: &Value, name: &str| -> Result<Option<(usize, String)>> {
            Ok(array(&view["sheets"])?
                .iter()
                .enumerate()
                .find(|(_, sheet)| sheet["name"] == name)
                .map(|(i, sheet)| (i, identity::page_id(sheet, i))))
        };
        // The stored layout without what the operation added to it.
        let mut stored = read(&dir.join("layout.yml"), Some("document-layout"))?;
        if let Some(history) = stored.get_mut("history").and_then(Value::as_array_mut) {
            history.retain(|record| record["id"] != operation);
        }
        let mut planned = Planned::new();
        let mut removed_pages = vec![];
        let mut restored_pages = vec![];
        if matches!(kind, "insert_slide" | "add_slide") {
            for name in [operation.to_owned(), format!("notes-{operation}")] {
                let Some((_, page)) = page_of(&before, &name)? else {
                    continue;
                };
                stored["sheets"]
                    .as_object_mut()
                    .context("layout sheets missing")?
                    .remove(&page);
                if let Some(file) = inspected.page_files.get(&page) {
                    planned.insert(file.clone(), None);
                    removed_pages.push(file.clone());
                }
            }
        }
        planned.insert(
            "layout.yml".into(),
            Some(serialized(Path::new("layout.yml"), &stored)?),
        );
        planned.insert(
            "mappings.yml".into(),
            Some(stored_mappings(&mappings_path, &mappings)?),
        );
        // A deleted slide's pages are not in the stored layout, which is read
        // as it stands and given them.
        let mut layout = if kind == "delete_slide" {
            identity::load(&dir, extraction, &Planned::new())?
        } else {
            identity::load(&dir, extraction, &planned)?
        };
        // The pages of a deleted slide and its notes come back as import makes
        // them, by their names and the places of their pages.
        let mut slide_pages = vec![];
        if kind == "delete_slide" {
            let slide = string(&removed["slide"])?;
            let order = slide_order(
                sheets,
                &crate::document_source::parse_slide_operations(&remaining, sheets)?,
            )?;
            let notes = order
                .into_iter()
                .find(|(name, _)| name == slide)
                .and_then(|(_, notes)| notes);
            for name in std::iter::once(slide.to_owned()).chain(notes) {
                if let Some((index, page)) = page_of(&after, &name)? {
                    layout["sheets"][&page] =
                        identity::initial_sheet(&array(&after["sheets"])?[index])?;
                    slide_pages.push((name, index, page));
                }
            }
            let operations = excel::parse_operations(&remaining, array(&after["sheets"])?)?;
            elements::geometry(&mut layout, &after, &operations)?;
        }
        // The interpretation of the extraction, which places cells that come
        // back in the tables they were in.
        let interpreted = match self.interpretation(&inspected)? {
            Some((structure, _)) => {
                let mut classified = layout.clone();
                elements::classify(&mut classified, array(&structure["elements"])?)?;
                Some(classified)
            }
            None => None,
        };
        let mut pages: BTreeMap<String, Value> = BTreeMap::new();
        let mut removed_values = 0;
        let mut restored_values = 0;
        // What comes back from the extraction: the cells of a page by block,
        // row and column, or every value of a page (none listed).
        let mut restored: Vec<Restored> = vec![];
        match kind {
            "insert_rows" | "insert_columns" | "delete_rows" | "delete_columns" => {
                let sheet = string(&removed["sheet"])?;
                let (_, page) = page_of(&before, sheet)?.context("operation sheet missing")?;
                let file = inspected
                    .page_files
                    .get(&page)
                    .context("operation sheet has no content page")?
                    .clone();
                let mut content = self.read_content(&dir, &file, extraction)?;
                let rows = kind.ends_with("_rows");
                if kind.starts_with("insert_") {
                    let prefix = format!("{operation}-");
                    let inserted = |key: &str| {
                        key.strip_prefix(&prefix)
                            .is_some_and(|n| n.parse::<u32>().is_ok())
                    };
                    // The values and their fonts, by row then column.
                    for (pointer, counted) in
                        [("/blocks/table-1/rows", true), ("/fonts/table-1", false)]
                    {
                        let Some(grid) =
                            content.pointer_mut(pointer).and_then(Value::as_object_mut)
                        else {
                            continue;
                        };
                        let before: usize = grid
                            .values()
                            .map(|values| values.as_object().map_or(0, |v| v.len()))
                            .sum();
                        if rows {
                            grid.retain(|key, _| !inserted(key));
                        } else {
                            for values in grid.values_mut() {
                                if let Some(values) = values.as_object_mut() {
                                    values.retain(|column, _| !inserted(column));
                                }
                            }
                            grid.retain(|_, values| {
                                values.as_object().is_none_or(|v| !v.is_empty())
                            });
                        }
                        if counted {
                            let left: usize = grid
                                .values()
                                .map(|values| values.as_object().map_or(0, |v| v.len()))
                                .sum();
                            removed_values += before - left;
                        }
                    }
                } else {
                    let own = |values: &[Value]| -> Result<Vec<excel::StructuralOperation>> {
                        Ok(excel::parse_operations(values, array(&before["sheets"])?)?
                            .into_iter()
                            .filter(|o| o.sheet == sheet)
                            .collect())
                    };
                    let with = own(&recorded)?;
                    let without = own(&remaining)?;
                    let source = array(&before["sheets"])?
                        .iter()
                        .find(|s| s["name"] == sheet)
                        .context("operation sheet missing")?;
                    let layout_sheet = &mut layout["sheets"][&page];
                    let mut cells = vec![];
                    let mut slots = BTreeSet::new();
                    for cell in array(&source["cells"])? {
                        let address = string(&cell["address"])?;
                        if excel::map_coordinate(sheet, address, &with)?.is_some()
                            || excel::map_coordinate(sheet, address, &without)?.is_none()
                        {
                            continue;
                        }
                        let (column, row) = excel::coordinate(address)?;
                        let (row, column) = (format!("r{row}"), excel::column_name(column)?);
                        let font = crate::extraction::font(source, cell)?;
                        if cell["type"] == "formula" {
                            let field = string(&cell["id"])?;
                            content["blocks"]["formulas"]["rows"][field]["formula"] =
                                json!(format!("={}", string(&cell["formula"])?));
                            if content["blocks"]["formulas"]["title"].is_null() {
                                content["blocks"]["formulas"]["title"] = json!("数式原文");
                            }
                            if !font.is_null() {
                                content["fonts"]["formulas"][field]["formula"] = font.clone();
                            }
                            // The cell takes its formula identity back.
                            let (Some(row_id), Some(column_id)) = (
                                identity::identity_of(layout_sheet, "rows", &row),
                                identity::identity_of(layout_sheet, "columns", &column),
                            ) else {
                                bail!(
                                    "{sheet}!{address} has no row or column identity to come back to"
                                );
                            };
                            let formulas = layout_sheet["formulas"]
                                .as_object_mut()
                                .context("layout formulas missing")?;
                            let identity = if formulas.contains_key(field) {
                                elements::token("formula", &[&row_id, &column_id])
                            } else {
                                field.to_owned()
                            };
                            formulas.insert(identity, json!({"row":row_id,"column":column_id}));
                        } else {
                            content["blocks"]["table-1"]["rows"][&row][&column] =
                                cell["value"].clone();
                            if content["blocks"]["table-1"]["title"].is_null() {
                                content["blocks"]["table-1"]["title"] = json!("本文");
                            }
                            if !font.is_null() {
                                content["fonts"]["table-1"][&row][&column] = font.clone();
                            }
                        }
                        if cell["type"] == "formula" {
                            slots.insert((
                                "formulas".to_owned(),
                                string(&cell["id"])?.to_owned(),
                                "formula".to_owned(),
                            ));
                        } else {
                            slots.insert(("table-1".to_owned(), row.clone(), column.clone()));
                        }
                        cells.push((row, column));
                        restored_values += 1;
                    }
                    let ids: Vec<String> = cells
                        .iter()
                        .filter_map(|(row, column)| {
                            Some(elements::cell_id(
                                &identity::identity_of(layout_sheet, "rows", row)?,
                                &identity::identity_of(layout_sheet, "columns", column)?,
                            ))
                        })
                        .collect();
                    if let Some(classified) = &interpreted {
                        elements::restore_groups(layout_sheet, &classified["sheets"][&page], &ids)?;
                    }
                    restored.push(Restored {
                        file: file.clone(),
                        sheet: sheet.to_owned(),
                        page: page.clone(),
                        slots: Some(slots),
                    });
                }
                pages.insert(file, content);
            }
            "delete_slide" => {
                let note = array(&extraction["findings"])?
                    .iter()
                    .find(|f| f["code"] == "R001")
                    .map(|f| f["message"].clone())
                    .unwrap_or(json!(""));
                for (name, index, page) in &slide_pages {
                    let source = &array(&after["sheets"])?[*index];
                    if let Some(classified) = &interpreted
                        && let Some(groups) = classified["sheets"][page].get("groups")
                    {
                        layout["sheets"][page]["groups"] = groups.clone();
                    }
                    let file = format!("content/{}", excel::filename(name));
                    ensure!(
                        !under(&dir, &file)?.exists(),
                        "{file} already exists; it would hold the restored slide"
                    );
                    restored_pages.push(file.clone());
                    restored.push(Restored {
                        file: file.clone(),
                        sheet: name.clone(),
                        page: page.clone(),
                        slots: None,
                    });
                    pages.insert(file, super::import::initial_page(source, page, &note)?);
                }
            }
            // The other operations change the original alone.
            _ => {}
        }
        // The value and font edits made to what comes back are made again.
        let reapplied = reapply(
            &mappings["value_edits"],
            &restored,
            &mut pages,
            &layout,
            &before,
        )?;
        for (name, page) in &pages {
            let body = elements::encode(page, &mut layout)?;
            planned.insert(name.clone(), Some(elements::page_bytes(&body, &layout)?));
        }
        planned.insert(
            "layout.yml".into(),
            Some(identity::stored(&mut layout, extraction)?),
        );
        let checked = self
            .inspect_with(&dir, false, &planned)
            .map_err(|error| rejection("validation_failed", format!("{error:#}")))?;
        let mut report = json!({"operation":removed,"content":checked.fingerprint});
        if removed_values > 0 {
            report["removed_values"] = json!(removed_values);
        }
        if restored_values > 0 {
            report["restored_values"] = json!(restored_values);
        }
        if reapplied > 0 {
            report["reapplied_edits"] = json!(reapplied);
        }
        if !removed_pages.is_empty() {
            report["removed_pages"] = json!(removed_pages);
        }
        if !restored_pages.is_empty() {
            report["restored_pages"] = json!(restored_pages);
        }
        if dry_run {
            report["state"] = json!("planned");
            return Ok(report);
        }
        self.commit(&dir, &planned, &inspected.fingerprint)?;
        report["state"] = json!("written");
        Ok(report)
    }
}

/// Content an operation taken back gives back from the extraction: on the
/// page `page` of `sheet`, kept in `file`, the values at `slots` (block, row
/// and column keys), or every value of the page.
struct Restored {
    file: String,
    sheet: String,
    page: String,
    slots: Option<BTreeSet<(String, String, String)>>,
}

/// Makes the value and font edits of `records` (the `value_edits` of
/// mappings) again on the content that comes back, in the order they were
/// made: each edit whose target comes back holding its `before` again.
/// Returns how many edits were made.
fn reapply(
    records: &Value,
    restored: &[Restored],
    pages: &mut BTreeMap<String, Value>,
    layout: &Value,
    extraction: &Value,
) -> Result<usize> {
    let mut count = 0;
    for edit in records
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|record| record["edits"].as_array().into_iter().flatten())
    {
        let sheet = edit["sheet"].as_str().unwrap_or_default();
        let Some(target) = restored.iter().find(|r| r.sheet == sheet) else {
            continue;
        };
        let layout_sheet = &layout["sheets"][&target.page];
        let slot = if let Some(cell) = edit["cell"].as_str() {
            let formula = array(&extraction["sheets"])?
                .iter()
                .filter(|s| s["name"] == sheet)
                .flat_map(|s| s["cells"].as_array().into_iter().flatten())
                .find(|c| c["address"] == cell && c["type"] == "formula");
            match formula {
                Some(c) => Some((
                    "formulas".to_owned(),
                    string(&c["id"])?.to_owned(),
                    "formula".to_owned(),
                )),
                None => {
                    let (column, row) = excel::coordinate(cell)?;
                    Some((
                        "table-1".to_owned(),
                        format!("r{row}"),
                        excel::column_name(column)?,
                    ))
                }
            }
        } else if let Some(element) = edit["element"].as_str() {
            let Ok((block, row, column)) = elements::Bindings::of(layout_sheet)?.get(element)
            else {
                continue;
            };
            identity::runtime_place(layout_sheet, &target.page, &block, &row, &column)?
                .map(|(row, column)| (block, row, column))
        } else {
            None
        };
        let Some((block, row, column)) = slot else {
            continue;
        };
        if target
            .slots
            .as_ref()
            .is_some_and(|slots| !slots.contains(&(block.clone(), row.clone(), column.clone())))
        {
            continue;
        }
        let Some(page) = pages.get_mut(&target.file) else {
            continue;
        };
        let mut made = false;
        if edit.get("before").is_some() {
            let present = &page["blocks"][&block]["rows"][&row][&column];
            if *present == edit["before"]
                || (present.is_number()
                    && edit["before"].is_number()
                    && present.as_f64() == edit["before"].as_f64())
            {
                page["blocks"][&block]["rows"][&row][&column] = edit["after"].clone();
                made = true;
            }
        }
        if let Some(font) = edit.get("font") {
            let present = page["fonts"][&block][&row][&column].clone();
            let matches = font["before"].as_object().is_some_and(|before| {
                before
                    .iter()
                    .all(|(property, value)| crate::fonts::same(&present[property.as_str()], value))
            });
            if matches && !present.is_null() {
                let change = crate::fonts::FontEdit::parse(&font["after"])?;
                page["fonts"][&block][&row][&column] = change.applied(&present);
                made = true;
            }
        }
        count += usize::from(made);
    }
    Ok(count)
}
