//! Slide insertion and deletion on a PowerPoint document. Like a row edit, the
//! operation in mappings.yml and the content pages it adds or removes change
//! together, are validated like `check` before anything is written, and
//! replace the document's directory in one rename.
use super::sheet_edit::rejection;
use super::*;
use crate::document_source::{SlidePosition, slide_order};

pub enum SlideEditKind {
    /// A copy of slide `from`, with its notes page, placed next to a slide.
    Insert {
        from: String,
        position: SlidePosition,
    },
    Delete {
        slide: String,
    },
}

pub struct SlideEdit {
    pub kind: SlideEditKind,
    pub id: Option<String>,
    pub reason: String,
    /// The content fingerprint the edit was planned against.
    pub base: Option<String>,
    pub dry_run: bool,
}

impl Store {
    /// Inserts a copy of a slide or deletes a slide of an adopted presentation,
    /// or of the proposal of `id`, recording the operation with the content
    /// pages it adds or removes.
    pub fn edit_slides(&self, id: &str, proposal: bool, edit: &SlideEdit) -> Result<Value> {
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
        let source_path = string(&inspected.meta["source"]["path"])?;
        if !Path::new(source_path)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("pptx"))
        {
            return Err(rejection(
                "unsupported_format",
                "slide edits apply to PowerPoint presentations (.pptx)",
            ));
        }
        if !inspected.source_current {
            return Err(rejection(
                "source_changed",
                "the original changed or is missing; re-import it before editing slides",
            ));
        }
        if edit.reason.trim().is_empty() {
            return Err(rejection("invalid_reason", "--reason must not be empty"));
        }
        let mappings_path = dir.join("mappings.yml");
        let mut mappings = read(&mappings_path, Some("mappings"))?;
        let recorded = array(&mappings["operations"])?.clone();
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
            None => {
                let prefix = match edit.kind {
                    SlideEditKind::Insert { .. } => "add-slide",
                    SlideEditKind::Delete { .. } => "del-slide",
                };
                next_operation_id(&recorded, prefix)
            }
        };
        let operation = match &edit.kind {
            SlideEditKind::Insert { from, position } => {
                let mut operation =
                    json!({"id":op_id,"kind":"insert_slide","from":from,"reason":edit.reason});
                match position {
                    SlidePosition::After(slide) => operation["after"] = json!(slide),
                    SlidePosition::Before(slide) => operation["before"] = json!(slide),
                }
                operation
            }
            SlideEditKind::Delete { slide } => {
                json!({"id":op_id,"kind":"delete_slide","slide":slide,"reason":edit.reason})
            }
        };
        if let Some(existing) = recorded.iter().find(|o| o["id"] == op_id.as_str()) {
            if *existing == operation {
                return Ok(
                    json!({"state":"unchanged","operation":operation,"content":inspected.fingerprint}),
                );
            }
            return Err(Rejection(json!({"code":"operation_id_conflict","message":format!("operation {op_id} is already recorded with other settings; choose another --id, or omit --id to have one generated"),"recorded":existing})).into());
        }

        let sheets = array(&inspected.extraction["sheets"])?;
        let prior = parse_slide_operations(&recorded, sheets)?;
        let mut values = recorded.clone();
        values.push(operation.clone());
        let operations = parse_slide_operations(&values, sheets)
            .map_err(|error| rejection("invalid_slide", format!("{error:#}")))?;
        let before = slide_view(sheets, &prior)?;
        let after = slide_view(sheets, &operations)?;
        let page_of = |view: &[Value], name: &str| {
            view.iter()
                .find(|sheet| sheet["name"] == name)
                .and_then(|sheet| sheet["page"].as_str())
                .map(str::to_owned)
        };
        let file_of = |page: &str| inspected.page_files.get(page).cloned();
        let mut planned = Planned::new();
        let mut layout = identity::load(&dir, &inspected.extraction, &planned)?;
        let mut added = vec![];
        let mut removed = vec![];
        let tables = mappings["tables"].as_array().cloned().unwrap_or_default();
        // The slide and its notes page, as the recorded operations leave them.
        let with_notes = |slide: &str| -> Result<Vec<String>> {
            let order = slide_order(sheets, &prior)?;
            let notes = order
                .into_iter()
                .find(|(name, _)| name == slide)
                .and_then(|(_, notes)| notes);
            Ok(std::iter::once(slide.to_owned()).chain(notes).collect())
        };
        // The row operations recorded on the pages of a slide.
        let row_operations = |pages: &[String]| -> Vec<String> {
            recorded
                .iter()
                .filter(|o| {
                    !is_slide_operation(o)
                        && o["sheet"]
                            .as_str()
                            .is_some_and(|sheet| pages.iter().any(|p| p == sheet))
                })
                .filter_map(|o| o["id"].as_str().map(str::to_owned))
                .collect()
        };
        let mut dropped = vec![];
        match &edit.kind {
            SlideEditKind::Insert { from, .. } => {
                // A copy is made of the slide as the original holds it, so its
                // content cannot hold the rows operations add to or take from it.
                let edited = row_operations(&with_notes(from)?);
                if !edited.is_empty() {
                    return Err(rejection(
                        "invalid_slide",
                        format!(
                            "{from} has row operations ({}); copy the slide before inserting or deleting its rows, or remove those operations",
                            edited.join(", ")
                        ),
                    ));
                }
                // The copy takes the page's current values, edits included.
                let mut new_tables = tables.clone();
                for source in with_notes(from)? {
                    let name = if source == *from {
                        op_id.clone()
                    } else {
                        format!("notes-{op_id}")
                    };
                    let source_page = page_of(&before, &source).context("copied page missing")?;
                    let file = file_of(&source_page)
                        .with_context(|| format!("{source} has no content page {source_page}"))?;
                    let page_id = page_of(&after, &name).context("inserted page missing")?;
                    let mut page = read(&under(&dir, &file)?, Some("content"))?;
                    layout["sheets"][&page_id] = layout["sheets"][&source_page].clone();
                    layout["sheets"][&page_id]["name"] = json!(name);
                    page["page_id"] = json!(page_id);
                    page["title"] = json!(name);
                    let target = format!("content/{name}.yml");
                    if under(&dir, &target)?.exists() {
                        return Err(rejection(
                            "page_exists",
                            format!("{target} already exists; choose another --id"),
                        ));
                    }
                    planned.insert(target.clone(), Some(serialized(Path::new(&target), &page)?));
                    added.push(target);
                    for table in tables.iter().filter(|t| t["page"] == source_page.as_str()) {
                        let mut table = table.clone();
                        table["page"] = json!(page_id);
                        new_tables.push(table);
                    }
                }
                mappings["tables"] = json!(new_tables);
            }
            SlideEditKind::Delete { slide } => {
                let names = with_notes(slide)?;
                // Rows of a deleted slide go with it.
                dropped = row_operations(&names);
                values.retain(|o| {
                    !o["id"]
                        .as_str()
                        .is_some_and(|id| dropped.iter().any(|d| d == id))
                });
                let gone: BTreeSet<String> = names
                    .iter()
                    .filter_map(|name| page_of(&before, name))
                    .collect();
                for page in &gone {
                    layout["sheets"].as_object_mut().unwrap().remove(page);
                    if let Some(file) = file_of(page) {
                        planned.insert(file.clone(), None);
                        removed.push(file);
                    }
                }
                let kept = |value: &Value| !gone.contains(value.as_str().unwrap_or(""));
                mappings["tables"] = json!(
                    tables
                        .into_iter()
                        .filter(|t| kept(&t["page"]))
                        .collect::<Vec<_>>()
                );
                let entries = array(&mappings["entries"])?
                    .iter()
                    .filter(|e| kept(&e["page"]))
                    .cloned()
                    .collect::<Vec<_>>();
                mappings["entries"] = json!(entries);
                let omissions = array(&mappings["omissions"])?
                    .iter()
                    .filter(|o| {
                        !o["target"]["sheet"]
                            .as_str()
                            .is_some_and(|sheet| names.iter().any(|name| name == sheet))
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                mappings["omissions"] = json!(omissions);
            }
        }
        mappings["operations"] = Value::Array(values.clone());
        planned.insert(
            "mappings.yml".to_owned(),
            Some(serialized(&mappings_path, &mappings)?),
        );
        planned.insert(
            "layout.yml".into(),
            Some(serialized(Path::new("layout.yml"), &layout)?),
        );
        Source::open(&under(&self.root, source_path)?)?
            .ensure_slide_edits_supported(&values)
            .map_err(|error| rejection("structural_edit_unsupported", format!("{error:#}")))?;
        let result = self
            .inspect_with(&dir, false, &planned)
            .map_err(|error| rejection("validation_failed", format!("{error:#}")))?;
        let order = slide_order(sheets, &operations)?;
        let mut report = json!({
            "operation":operation,
            "added_pages":added,
            "removed_pages":removed,
            "slides":order.iter().map(|(slide, _)| slide).collect::<Vec<_>>(),
        });
        if !dropped.is_empty() {
            report["dropped_operations"] = json!(dropped);
        }
        if edit.dry_run {
            report["state"] = json!("planned");
            report["content"] = json!(result.fingerprint);
            return Ok(report);
        }
        self.commit(&dir, &planned, &inspected.fingerprint)?;
        report["state"] = json!("written");
        report["content"] = json!(result.fingerprint);
        Ok(report)
    }
}
