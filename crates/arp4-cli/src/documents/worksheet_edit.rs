//! Worksheets added to an Excel workbook: an empty one or a copy of another
//! (see [`excel::sheet_view`]). Like a slide edit, the operation in
//! mappings.yml, the sheet's page of the layout and its content page are
//! written together, after the same validation as check and a trial write.
//! A copy's content page takes the current values of the page it copies,
//! edits included, and its formulas name the copy where they name the sheet.
use super::sheet_edit::rejection;
use super::*;

/// The kinds of operation on a sheet copied that are recorded on its copy
/// too: those that leave its rows, columns and cells where they are.
const COPIED: [&str; 12] = [
    "format_cells",
    "copy_cells",
    "set_link",
    "clear_links",
    "set_note",
    "clear_notes",
    "set_validation",
    "clear_validations",
    "add_conditional_format",
    "clear_conditional_formats",
    "place_drawing",
    "add_image",
];

/// The names of the worksheets the sheet operations `values` leave, in order.
fn sheet_order(original: &Extraction, values: &[Value]) -> Result<Vec<String>> {
    let added = excel::parse_sheet_operations(values, &original.sheets)?;
    let mut order = excel::sheet_order(&original.sheets, &added)?;
    for change in excel::parse_sheet_changes(values, &original.sheets)? {
        match change.name {
            Some(name) => {
                for sheet in order.iter_mut().filter(|sheet| **sheet == change.sheet) {
                    *sheet = name.clone();
                }
            }
            None => order.retain(|sheet| *sheet != change.sheet),
        }
    }
    Ok(order)
}

/// A worksheet of the original deleted, or renamed `name`.
pub struct SheetChangeEdit {
    pub sheet: String,
    /// The new name; none for a deletion.
    pub name: Option<String>,
    pub id: Option<String>,
    pub reason: String,
    pub base: Option<String>,
    pub dry_run: bool,
}

pub struct WorksheetEdit {
    pub name: String,
    /// The sheet copied; none for an empty sheet.
    pub from: Option<String>,
    /// The sheet the new one goes next to, and whether after it; none for last.
    pub next_to: Option<(String, bool)>,
    pub id: Option<String>,
    pub reason: String,
    pub base: Option<String>,
    pub dry_run: bool,
}

impl Store {
    /// Deletes or renames a worksheet of the original of an adopted workbook,
    /// or of the proposal of `id`: export does it after every other edit. A
    /// deleted sheet's content page goes, with the operations and value
    /// edits recorded on it.
    pub fn change_worksheet(
        &self,
        id: &str,
        proposal: bool,
        edit: &SheetChangeEdit,
    ) -> Result<Value> {
        let dir = if proposal {
            self.proposal(id)?
        } else {
            self.document(id)?
        };
        let inspected = self.inspect(&dir, false).map_err(|error| {
            rejection(
                "invalid_document",
                format!("the document fails check before the edit; fix that first: {error:#}"),
            )
        })?;
        if let Some(base) = &edit.base
            && *base != inspected.fingerprint
        {
            return Err(rejection(
                "base_changed",
                "the document changed after --base was read; read check --include-hashes again and plan the edit on the current content",
            ));
        }
        let extraction = &inspected.extraction;
        if !identity::is_excel(extraction) {
            return Err(rejection(
                "unsupported_format",
                "sheets are deleted and renamed in Excel workbooks; delete PowerPoint slides with documents slides",
            ));
        }
        if !inspected.source_current {
            return Err(rejection(
                "source_changed",
                "the original changed or is missing; re-import it before changing sheets",
            ));
        }
        if edit.reason.trim().is_empty() {
            return Err(rejection("invalid_reason", "--reason must not be empty"));
        }
        let mappings_path = dir.join("mappings.yml");
        let mut mappings = read_mappings(&mappings_path)?;
        let recorded = array(&mappings["operations"])?.clone();
        let kind = if edit.name.is_some() {
            "rename_sheet"
        } else {
            "delete_sheet"
        };
        let op_id = match &edit.id {
            Some(op) => {
                identifier(op).map_err(|_| {
                    rejection(
                        "invalid_operation_id",
                        format!("--id {op} must match ^[a-zA-Z0-9][a-zA-Z0-9_-]*$"),
                    )
                })?;
                op.clone()
            }
            None => next_operation_id(&recorded, kind.replace('_', "-").as_str()),
        };
        let mut operation = json!({"id":op_id,"kind":kind,"sheet":edit.sheet,"reason":edit.reason});
        if let Some(name) = &edit.name {
            operation["name"] = json!(name);
        }
        if let Some(existing) = recorded.iter().find(|o| o["id"] == op_id.as_str()) {
            if *existing == operation {
                return Ok(
                    json!({"state":"unchanged","operation":operation,"content":inspected.fingerprint}),
                );
            }
            return Err(Rejection(json!({"code":"operation_id_conflict","message":format!("operation {op_id} is already recorded with other settings; choose another --id, or omit --id to have one generated"),"recorded":existing})).into());
        }
        let mut values = recorded.clone();
        values.push(operation.clone());
        excel::parse_sheet_changes(&values, &extraction.sheets)
            .map_err(|error| rejection("invalid_sheet", format!("{error:#}")))?;
        let mut planned = Planned::new();
        let mut dropped: Vec<String> = vec![];
        let mut removed_pages = vec![];
        if edit.name.is_none() {
            let (index, sheet) = extraction
                .sheets
                .iter()
                .enumerate()
                .find(|(_, sheet)| {
                    sheet["name"] == edit.sheet.as_str() && sheet["added_by"].is_null()
                })
                .context("deleted sheet missing")?;
            let page = identity::page_id(&sheet.fields, index);
            // What is recorded on the sheet goes with it.
            dropped = values
                .iter()
                .filter(|o| !excel::is_sheet_operation(o) && o["sheet"] == edit.sheet.as_str())
                .filter_map(|o| o["id"].as_str().map(str::to_owned))
                .collect();
            values.retain(|o| {
                !o["id"]
                    .as_str()
                    .is_some_and(|id| dropped.iter().any(|d| d == id))
            });
            let mut layout = identity::load(&dir, extraction, &Planned::new())?;
            for (other, sheet) in &layout.sheets {
                let frames = sheet["formulas"]
                    .as_object()
                    .into_iter()
                    .flat_map(|f| f.values())
                    .filter_map(|target| target["written_after"].as_str());
                for frame in frames {
                    if dropped.iter().any(|d| d == frame) {
                        return Err(rejection(
                            "operation_has_dependents",
                            format!(
                                "formulas of {other} are written as the cells stand after {frame}, which goes with {}; clear them with documents values first",
                                edit.sheet
                            ),
                        ));
                    }
                }
            }
            layout.sheets.remove(&page);
            if let Some(file) = inspected.page_files.get(&page) {
                planned.insert(file.clone(), None);
                removed_pages.push(file.clone());
            }
            let entries: Vec<Value> = array(&mappings["entries"])?
                .iter()
                .filter(|e| e["page"] != page.as_str())
                .cloned()
                .collect();
            mappings["entries"] = json!(entries);
            let omissions: Vec<Value> = array(&mappings["omissions"])?
                .iter()
                .filter(|o| o["target"]["sheet"] != edit.sheet.as_str())
                .cloned()
                .collect();
            mappings["omissions"] = json!(omissions);
            if let Some(records) = mappings["value_edits"].as_array_mut() {
                for record in records.iter_mut() {
                    if let Some(edits) = record["edits"].as_array_mut() {
                        edits.retain(|e| e["sheet"] != edit.sheet.as_str());
                    }
                }
                records.retain(|record| record["edits"].as_array().is_some_and(|e| !e.is_empty()));
            }
            planned.insert(
                "layout.yml".into(),
                Some(identity::stored(layout, extraction)?),
            );
        }
        let order = sheet_order(inspected.original_extraction(), &values)?;
        mappings["operations"] = Value::Array(values);
        planned.insert(
            "mappings.yml".to_owned(),
            Some(stored_mappings(&mappings_path, &mappings)?),
        );
        let checked = self
            .inspect_with(&dir, false, &planned)
            .map_err(|error| rejection("validation_failed", format!("{error:#}")))?;
        let source = under(&self.root, string(&inspected.meta["source"]["path"])?)?;
        let book = Source::open(&source)?;
        let writeback = self
            .writeback(&dir, &checked, &book)
            .map_err(|error| rejection("writeback_refused", format!("{error:#}")))?;
        let stage = Stage::new_in(&under(&self.arp, "cache/export")?, &self.arp)?;
        let preview = stage
            .path()
            .join(source.file_name().context("missing filename")?);
        book.patch(&preview, &writeback.edits(&checked)?)
            .map_err(|error| rejection("writeback_refused", format!("{error:#}")))?;
        let mut report = json!({"operation":operation,"sheets":order});
        if !removed_pages.is_empty() {
            report["removed_pages"] = json!(removed_pages);
        }
        if !dropped.is_empty() {
            report["dropped_operations"] = json!(dropped);
        }
        if edit.dry_run {
            report["state"] = json!("planned");
            report["content"] = json!(checked.fingerprint);
            return Ok(report);
        }
        self.commit(&dir, &planned, &inspected.fingerprint)?;
        report["state"] = json!("written");
        report["content"] = json!(checked.fingerprint);
        let row = self.state_of(id, &dir, Ok(checked));
        self.remember_state(id, &dir, &row);
        Ok(report)
    }

    /// Adds an empty worksheet or a copy of one to an adopted workbook, or to
    /// the proposal of `id`, with its content page.
    pub fn add_worksheet(&self, id: &str, proposal: bool, edit: &WorksheetEdit) -> Result<Value> {
        let dir = if proposal {
            self.proposal(id)?
        } else {
            self.document(id)?
        };
        let inspected = self.inspect(&dir, false).map_err(|error| {
            rejection(
                "invalid_document",
                format!("the document fails check before the edit; fix that first: {error:#}"),
            )
        })?;
        if let Some(base) = &edit.base
            && *base != inspected.fingerprint
        {
            return Err(rejection(
                "base_changed",
                "the document changed after --base was read; read check --include-hashes again and plan the edit on the current content",
            ));
        }
        let extraction = &inspected.extraction;
        if !identity::is_excel(extraction) {
            return Err(rejection(
                "unsupported_format",
                "sheets are added to Excel workbooks; add PowerPoint slides with documents slides",
            ));
        }
        if !inspected.source_current {
            return Err(rejection(
                "source_changed",
                "the original changed or is missing; re-import it before adding sheets",
            ));
        }
        if edit.reason.trim().is_empty() {
            return Err(rejection("invalid_reason", "--reason must not be empty"));
        }
        let mappings_path = dir.join("mappings.yml");
        let mut mappings = read_mappings(&mappings_path)?;
        let recorded = array(&mappings["operations"])?.clone();
        let kind = if edit.from.is_some() {
            "copy_sheet"
        } else {
            "add_sheet"
        };
        let op_id = match &edit.id {
            Some(op) => {
                identifier(op).map_err(|_| {
                    rejection(
                        "invalid_operation_id",
                        format!("--id {op} must match ^[a-zA-Z0-9][a-zA-Z0-9_-]*$"),
                    )
                })?;
                op.clone()
            }
            None => next_operation_id(&recorded, kind.replace('_', "-").as_str()),
        };
        let mut operation = json!({"id":op_id,"kind":kind,"name":edit.name,"reason":edit.reason});
        if let Some(from) = &edit.from {
            operation["from"] = json!(from);
        }
        match &edit.next_to {
            Some((sheet, true)) => operation["after"] = json!(sheet),
            Some((sheet, false)) => operation["before"] = json!(sheet),
            None => {}
        }
        if let Some(existing) = recorded.iter().find(|o| o["id"] == op_id.as_str()) {
            if *existing == operation {
                return Ok(
                    json!({"state":"unchanged","operation":operation,"content":inspected.fingerprint}),
                );
            }
            return Err(Rejection(json!({"code":"operation_id_conflict","message":format!("operation {op_id} is already recorded with other settings; choose another --id, or omit --id to have one generated"),"recorded":existing})).into());
        }
        let mut values = recorded.clone();
        values.push(operation.clone());
        let parsed = excel::parse_sheet_operations(&values, &extraction.sheets)
            .map_err(|error| rejection("invalid_sheet", format!("{error:#}")))?;
        let added = parsed.last().context("sheet operation missing")?;
        let page_id = excel::sheet_page(&op_id);
        let after = operated_extraction(extraction, &json!(values))?;
        let sheet = after
            .sheets
            .iter()
            .find(|sheet| sheet["page"] == page_id.as_str())
            .context("added sheet missing")?;
        let file = format!("content/{}", excel::filename(&edit.name));
        if under(&dir, &file)?.exists() {
            return Err(rejection(
                "page_exists",
                format!("{file} already exists; choose another --name"),
            ));
        }
        let mut layout = identity::load(&dir, extraction, &Planned::new())?;
        let mut copied_operations = vec![];
        let page = match &added.from {
            Some(from) => {
                // A copy is made of the sheet as the original holds it, so its
                // page cannot hold the rows, columns and cells operations add to
                // or take from it; the formats, placements, pictures and what
                // cells carry recorded on it are recorded on the copy too.
                let edited: Vec<&Value> = recorded
                    .iter()
                    .filter(|o| o["sheet"] == from.as_str())
                    .collect();
                let structural: Vec<&str> = edited
                    .iter()
                    .filter(|o| !COPIED.contains(&o["kind"].as_str().unwrap_or_default()))
                    .filter_map(|o| o["id"].as_str())
                    .collect();
                if !structural.is_empty() {
                    return Err(rejection(
                        "invalid_sheet",
                        format!(
                            "{from} has row, column or cell operations ({}); copy the sheet before inserting or deleting its rows, columns or cells, or remove those operations",
                            structural.join(", ")
                        ),
                    ));
                }
                for operation in edited {
                    let mut again = operation.clone();
                    let source_id = string(&operation["id"])?;
                    again["id"] = json!(format!("{op_id}-{source_id}"));
                    again["sheet"] = json!(edit.name);
                    if let Some(drawing) = operation["drawing"].as_str()
                        && let Some((_, number)) = drawing.rsplit_once('#')
                    {
                        again["drawing"] =
                            json!(format!("{}#{number}", excel::copied_drawing_part(&op_id)));
                    }
                    copied_operations.push(again);
                }
                let (index, _) = extraction
                    .sheets
                    .iter()
                    .enumerate()
                    .find(|(_, sheet)| sheet["name"] == from.as_str())
                    .context("copied sheet missing")?;
                let source_page = identity::page_id(&extraction.sheets[index].fields, index);
                let source_file = inspected
                    .page_files
                    .get(&source_page)
                    .with_context(|| format!("{from} has no content page {source_page}"))?;
                // The copy's layout: the sheet's, its drawings the copy's own.
                let mut copy = layout.sheet(&source_page)?.to_value();
                if let Some(part) = extraction.sheets[index]["drawings"][0]["part"].as_str() {
                    let text = serde_json::to_string(&copy)?.replace(
                        &format!("{part}#"),
                        &format!("{}#", excel::copied_drawing_part(&op_id)),
                    );
                    copy = serde_json::from_str(&text)?;
                }
                copy["name"] = json!(edit.name);
                layout
                    .sheets
                    .insert(page_id.clone(), LayoutSheet::from_value(copy)?);
                // The page's values, its formulas named for the copy.
                let tables = excel::copied_tables(&extraction.sheets, &parsed)?;
                let names = tables.get(&op_id).map_or(&[][..], Vec::as_slice);
                let mut page = self.read_content(&dir, source_file, extraction)?;
                let prefix = |page: &str| format!("c-{}-", page.trim_start_matches("sheet-"));
                let (old, new) = (prefix(&source_page), prefix(&page_id));
                for pointer in ["/blocks/formulas/rows", "/fonts/formulas"] {
                    if let Some(rows) = page.pointer_mut(pointer).and_then(Value::as_object_mut) {
                        let taken = std::mem::take(rows);
                        for (key, mut value) in taken {
                            if pointer.starts_with("/blocks")
                                && let Some(text) = value["formula"].as_str()
                            {
                                let formula = text.trim_start_matches('=');
                                value["formula"] = json!(format!(
                                    "={}",
                                    excel::copied_formula(formula, from, &edit.name, names)?
                                ));
                            }
                            let key = match key.strip_prefix(&old) {
                                Some(rest) => format!("{new}{rest}"),
                                None => key,
                            };
                            rows.insert(key, value);
                        }
                    }
                }
                page["page_id"] = json!(page_id);
                page["title"] = json!(edit.name);
                page
            }
            None => {
                let note = array(&extraction["findings"])?
                    .iter()
                    .find(|f| f["code"] == "R001")
                    .map(|f| f["message"].clone())
                    .unwrap_or(json!(""));
                layout
                    .sheets
                    .insert(page_id.clone(), identity::initial_sheet(sheet)?);
                super::import::initial_page(sheet, &page_id, &note)?
            }
        };
        identity::check_axes(&layout)?;
        let body = elements::encode(page, &mut layout, None)?;
        let mut planned = Planned::new();
        planned.insert(file.clone(), Some(elements::content_bytes(&body, &layout)?));
        let copied: Vec<Value> = copied_operations.iter().map(|o| o["id"].clone()).collect();
        values.extend(copied_operations);
        mappings["operations"] = Value::Array(values);
        planned.insert(
            "mappings.yml".to_owned(),
            Some(stored_mappings(&mappings_path, &mappings)?),
        );
        planned.insert(
            "layout.yml".into(),
            Some(identity::stored(layout, extraction)?),
        );
        let checked = self
            .inspect_with(&dir, false, &planned)
            .map_err(|error| rejection("validation_failed", format!("{error:#}")))?;
        // Every pending change is written once to a scratch copy with the
        // sheet, as export will write them.
        let source = under(&self.root, string(&inspected.meta["source"]["path"])?)?;
        let book = Source::open(&source)?;
        let writeback = self
            .writeback(&dir, &checked, &book)
            .map_err(|error| rejection("writeback_refused", format!("{error:#}")))?;
        let stage = Stage::new_in(&under(&self.arp, "cache/export")?, &self.arp)?;
        let preview = stage
            .path()
            .join(source.file_name().context("missing filename")?);
        book.patch(&preview, &writeback.edits(&checked)?)
            .map_err(|error| rejection("writeback_refused", format!("{error:#}")))?;
        let mut report = json!({
            "operation":operation,
            "page":page_id,
            "added_pages":[file],
            "sheets":sheet_order(inspected.original_extraction(), array(&mappings["operations"])?)?,
        });
        if !copied.is_empty() {
            report["copied_operations"] = json!(copied);
        }
        if edit.dry_run {
            report["state"] = json!("planned");
            report["content"] = json!(checked.fingerprint);
            return Ok(report);
        }
        self.commit(&dir, &planned, &inspected.fingerprint)?;
        report["state"] = json!("written");
        report["content"] = json!(checked.fingerprint);
        let row = self.state_of(id, &dir, Ok(checked));
        self.remember_state(id, &dir, &row);
        Ok(report)
    }
}
