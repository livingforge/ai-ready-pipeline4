//! A PNG picture added to an Excel worksheet over a cell range, recorded in
//! mappings.yml as an add_image operation after a trial write that checks it.
use super::sheet_edit::rejection;
use super::*;
use crate::document_source::Edits;

pub struct ImageRequest {
    pub sheet: String,
    /// The cells of the picture's top left and bottom right corners.
    pub from: String,
    pub to: String,
    /// The picture: assets/<file>.png in the document folder.
    pub asset: String,
    pub name: Option<String>,
    pub id: Option<String>,
    pub reason: String,
    /// The content fingerprint the edit was planned against.
    pub base: Option<String>,
    pub dry_run: bool,
}

impl Store {
    /// Records a picture added to a worksheet of an adopted workbook, or of the
    /// proposal of `id`, once a trial write of every operation succeeds.
    pub fn add_image(&self, id: &str, proposal: bool, edit: &ImageRequest) -> Result<Value> {
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
        let format = Path::new(source_path)
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !crate::document_source::is_excel(&format) {
            return Err(rejection(
                "unsupported_format",
                "images are added to Excel workbooks here; use documents shapes add-picture for PowerPoint slides",
            ));
        }
        if !inspected.source_current {
            return Err(rejection(
                "source_changed",
                "the original changed or is missing; re-import it before adding images",
            ));
        }
        if edit.reason.trim().is_empty() {
            return Err(rejection("invalid_reason", "--reason must not be empty"));
        }
        let mappings_path = dir.join("mappings.yml");
        let mut mappings = read_mappings(&mappings_path)?;
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
            None => next_operation_id(&recorded, "add-image"),
        };
        let mut operation = json!({"id":op_id,"sheet":edit.sheet,"reason":edit.reason,"kind":"add_image",
            "asset":edit.asset,"anchor":{"from":{"cell":edit.from},"to":{"cell":edit.to}}});
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
        let sheets = array(&inspected.extraction["sheets"])?;
        if !sheets
            .iter()
            .any(|sheet| sheet["name"] == edit.sheet.as_str())
        {
            let names: Vec<_> = sheets.iter().map(|sheet| sheet["name"].clone()).collect();
            return Err(Rejection(json!({"code":"sheet_not_found","message":format!("no worksheet {} in this workbook",edit.sheet),"sheets":names})).into());
        }
        let mut values = recorded.clone();
        values.push(operation.clone());
        validate(
            "mappings",
            &json!({"schema_version":"1","operations":values}),
        )
        .map_err(|error| rejection("invalid_image", format!("{error:#}")))?;
        let images = excel::parse_image_operations(&values, sheets)
            .map_err(|error| rejection("invalid_image", format!("{error:#}")))?;
        let assets: Vec<String> = images.iter().map(|o| o.asset.clone()).collect();
        let assets = self
            .image_assets(&dir, &assets)
            .map_err(|error| rejection("invalid_asset", format!("{error:#}")))?;
        // Every operation is written once to a scratch copy, which places the
        // picture the way export will.
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
            .map_err(|error| rejection("invalid_image", format!("{error:#}")))?;
        mappings["operations"] = Value::Array(values);
        let mut planned = Planned::new();
        planned.insert(
            "mappings.yml".to_owned(),
            Some(stored_mappings(&mappings_path, &mappings)?),
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
