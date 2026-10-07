//! Shape operations on a PowerPoint document: a shape, picture or connector
//! changed, added or deleted, recorded in mappings.yml like other operations
//! after a trial write that checks them.
use super::sheet_edit::rejection;
use super::*;
use crate::document_source::{Edits, parse_shape_operations};

pub struct ShapeRequest {
    /// The operation without its ID and reason, as `documents schema mappings` gives it.
    pub operation: Value,
    pub id: Option<String>,
    pub reason: String,
    /// The content fingerprint the edit was planned against.
    pub base: Option<String>,
    pub dry_run: bool,
}

impl Store {
    /// Records a shape operation on a slide of an adopted presentation, or of
    /// the proposal of `id`, once a trial write of every operation succeeds.
    pub fn edit_shapes(&self, id: &str, proposal: bool, edit: &ShapeRequest) -> Result<Value> {
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
                "shape edits apply to PowerPoint presentations (.pptx)",
            ));
        }
        if !inspected.source_current {
            return Err(rejection(
                "source_changed",
                "the original changed or is missing; re-import it before editing shapes",
            ));
        }
        if edit.reason.trim().is_empty() {
            return Err(rejection("invalid_reason", "--reason must not be empty"));
        }
        let mappings_path = dir.join("mappings.yml");
        let mut mappings = read(&mappings_path, Some("mappings"))?;
        let recorded = array(&mappings["operations"])?.clone();
        let kind = string(&edit.operation["kind"])?.to_owned();
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
            None => next_operation_id(&recorded, &kind.replace('_', "-")),
        };
        let mut operation = edit.operation.clone();
        operation["id"] = json!(op_id);
        operation["reason"] = json!(edit.reason);
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
        validate("mappings", &json!({"schema_version":"1","entries":[],"tables":[],"omissions":[],"operations":values,"interpretation":mappings["interpretation"]}))
            .map_err(|error| rejection("invalid_shape", format!("{error:#}")))?;
        let operations = parse_shape_operations(&values)
            .map_err(|error| rejection("invalid_shape", format!("{error:#}")))?;
        // The slide must be one the operations leave.
        let operated = operated_extraction(&inspected.extraction, &json!(values))?;
        let slide = string(&operation["slide"])?;
        if slide.starts_with("notes-")
            || !array(&operated["sheets"])?
                .iter()
                .any(|sheet| sheet["name"] == slide)
        {
            return Err(rejection(
                "invalid_slide",
                format!(
                    "{slide} is not a slide here: name a slide of the original (slide-N) or one a slide operation inserted (its operation ID)"
                ),
            ));
        }
        let assets: Vec<String> = operations
            .iter()
            .filter_map(|o| o.asset().map(str::to_owned))
            .collect();
        let assets = self
            .image_assets(&dir, &assets)
            .map_err(|error| rejection("invalid_asset", format!("{error:#}")))?;
        // Every operation is written once to a scratch copy, which checks the
        // shapes they name and reads the result back.
        let source = under(&self.root, source_path)?;
        let stage = Stage::new_in(&under(&self.arp, "cache/export")?, &self.arp)?;
        let preview = stage
            .path()
            .join(source.file_name().context("missing filename")?);
        Source::open(&source)?
            .patch(
                &preview,
                &Edits {
                    operations: &values,
                    changes: &[],
                    formulas: &[],
                    fonts: &[],
                    assets: &assets,
                },
            )
            .map_err(|error| rejection("invalid_shape", format!("{error:#}")))?;
        mappings["operations"] = Value::Array(values);
        let mut planned = Planned::new();
        planned.insert(
            "mappings.yml".to_owned(),
            Some(serialized(&mappings_path, &mappings)?),
        );
        let result = self
            .inspect_with(&dir, false, &planned)
            .map_err(|error| rejection("validation_failed", format!("{error:#}")))?;
        let mut report = json!({"operation":operation});
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
