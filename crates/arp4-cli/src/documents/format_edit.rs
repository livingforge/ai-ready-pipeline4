//! The format of a range of cells of an Excel worksheet, changed as a
//! `format_cells` operation in mappings.yml after a trial write of every
//! pending change: a format copied from a cell, alignment, font emphasis,
//! fill, borders, number format, row heights, column widths and merges.
//! Values stay as they are.
use super::sheet_edit::rejection;
use super::*;

pub struct FormatRequest {
    pub sheet: String,
    /// The cells, as the operations recorded before leave them.
    pub range: String,
    /// The cell whose whole format the range takes.
    pub from: Option<String>,
    pub wrap: Option<bool>,
    pub shrink: Option<bool>,
    pub horizontal: Option<String>,
    pub vertical: Option<String>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub strike: Option<bool>,
    /// `none`, `single` or `double`.
    pub underline: Option<String>,
    /// `RRGGBB`, or `none` for no fill.
    pub fill: Option<String>,
    pub number_format: Option<String>,
    /// Lines as `edges:style[:RRGGBB]`, in the order they are drawn.
    pub borders: Vec<String>,
    /// Points, or `auto` for the height Excel fits to the text.
    pub row_height: Option<String>,
    pub column_width: Option<f64>,
    /// Merge the range, or take apart the merges inside it.
    pub merge: Option<bool>,
    pub id: Option<String>,
    pub reason: String,
    /// The content fingerprint the edit was planned against.
    pub base: Option<String>,
    pub dry_run: bool,
}

/// A color given as `RRGGBB`, in capitals as Excel writes it.
fn color(text: &str, option: &str) -> Result<String> {
    let upper = text.trim_start_matches('#').to_ascii_uppercase();
    if upper.len() == 6 && upper.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(upper)
    } else {
        Err(rejection(
            "invalid_format",
            format!("{option} takes a color as RRGGBB, such as D9D9D9; {text} is none"),
        ))
    }
}

/// A line of `--border` (`edges:style[:RRGGBB]`) as format_cells records it.
fn border_line(text: &str) -> Result<Value> {
    const EDGES: [&str; 9] = [
        "all",
        "outline",
        "inside",
        "horizontal",
        "vertical",
        "top",
        "bottom",
        "left",
        "right",
    ];
    const STYLES: [&str; 14] = [
        "none",
        "thin",
        "medium",
        "thick",
        "dashed",
        "dotted",
        "double",
        "hair",
        "mediumDashed",
        "dashDot",
        "mediumDashDot",
        "dashDotDot",
        "mediumDashDotDot",
        "slantDashDot",
    ];
    let mut parts = text.split(':');
    let (Some(edges), Some(style)) = (parts.next(), parts.next()) else {
        return Err(rejection(
            "invalid_format",
            format!("--border {text} must be edges:style[:RRGGBB], such as outline:thin"),
        ));
    };
    if !EDGES.contains(&edges) {
        return Err(rejection(
            "invalid_format",
            format!("--border edges {edges} must be one of {}", EDGES.join(", ")),
        ));
    }
    if !STYLES.contains(&style) {
        return Err(rejection(
            "invalid_format",
            format!(
                "--border style {style} must be one of {}",
                STYLES.join(", ")
            ),
        ));
    }
    let mut line = json!({"edges":edges,"style":style});
    if let Some(color_text) = parts.next() {
        line["color"] = json!(color(color_text, "--border")?);
    }
    if parts.next().is_some() {
        return Err(rejection(
            "invalid_format",
            format!("--border {text} must be edges:style[:RRGGBB]"),
        ));
    }
    Ok(line)
}

/// What every Excel sheet operation request gives besides its settings.
pub(super) struct SheetOperation<'a> {
    pub(super) sheet: &'a str,
    pub(super) kind: &'a str,
    /// The start of a generated operation ID.
    pub(super) prefix: &'a str,
    pub(super) id: Option<&'a str>,
    pub(super) reason: &'a str,
    pub(super) base: Option<&'a str>,
    pub(super) dry_run: bool,
}

/// The `content` of [`Store::record_sheet_operation`] for an operation that
/// changes no content: the original alone.
pub(super) fn unchanged_content(
    _: &Path,
    _: &Inspection,
    _: &[Value],
    _: &mut Planned,
) -> Result<Value> {
    Ok(json!({}))
}

impl Store {
    /// Records a change of the format of a range of cells of an adopted
    /// workbook, or of the proposal of `id`, once a trial write of every
    /// pending change with it succeeds.
    pub fn format_cells(&self, id: &str, proposal: bool, edit: &FormatRequest) -> Result<Value> {
        let header = SheetOperation {
            sheet: &edit.sheet,
            kind: "format_cells",
            prefix: "format",
            id: edit.id.as_deref(),
            reason: &edit.reason,
            base: edit.base.as_deref(),
            dry_run: edit.dry_run,
        };
        let settings = |_: &Inspection, operation: &mut Value| -> Result<()> {
            operation["range"] = json!(edit.range);
            for (key, value) in [
                ("from", edit.from.as_ref().map(|v| json!(v))),
                ("wrap", edit.wrap.map(|v| json!(v))),
                ("shrink", edit.shrink.map(|v| json!(v))),
                ("horizontal", edit.horizontal.as_ref().map(|v| json!(v))),
                ("vertical", edit.vertical.as_ref().map(|v| json!(v))),
                ("column_width", edit.column_width.map(|v| json!(v))),
                ("merge", edit.merge.map(|v| json!(v))),
                ("bold", edit.bold.map(|v| json!(v))),
                ("italic", edit.italic.map(|v| json!(v))),
                ("strike", edit.strike.map(|v| json!(v))),
                ("underline", edit.underline.as_ref().map(|v| json!(v))),
                (
                    "number_format",
                    edit.number_format.as_ref().map(|v| json!(v)),
                ),
            ] {
                if let Some(value) = value {
                    operation[key] = value;
                }
            }
            if let Some(fill) = &edit.fill {
                operation["fill"] = json!(if fill == "none" {
                    fill.clone()
                } else {
                    color(fill, "--fill")?
                });
            }
            if !edit.borders.is_empty() {
                let lines = edit
                    .borders
                    .iter()
                    .map(|line| border_line(line))
                    .collect::<Result<Vec<_>>>()?;
                operation["borders"] = json!(lines);
            }
            if let Some(height) = &edit.row_height {
                operation["row_height"] = if height == "auto" {
                    json!("auto")
                } else {
                    json!(height.parse::<f64>().map_err(|_| {
                        rejection("invalid_format", "--row-height must be points or auto")
                    })?)
                };
            }
            Ok(())
        };
        self.record_sheet_operation(
            id,
            proposal,
            &header,
            settings,
            |values, inspected| {
                excel::parse_format_operations(values, &inspected.extraction.sheets).map(|_| ())
            },
            unchanged_content,
        )
    }

    /// Records the Excel sheet operation of `header` with the `settings` it
    /// takes on an adopted workbook, or on the proposal of `id`, once the
    /// operations `parse` reads them and a trial write of every pending
    /// change with it succeeds. `content` writes into the files planned the
    /// content and layout the operations change, if any, and gives what the
    /// report adds of it.
    pub(super) fn record_sheet_operation(
        &self,
        id: &str,
        proposal: bool,
        header: &SheetOperation<'_>,
        settings: impl FnOnce(&Inspection, &mut Value) -> Result<()>,
        parse: impl FnOnce(&[Value], &Inspection) -> Result<()>,
        content: impl FnOnce(&Path, &Inspection, &[Value], &mut Planned) -> Result<Value>,
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
        if let Some(base) = header.base
            && base != inspected.fingerprint
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
                format!("{} operations apply to Excel workbooks only", header.kind),
            ));
        }
        if !inspected.source_current {
            return Err(rejection(
                "source_changed",
                "the original changed or is missing; re-import it before editing",
            ));
        }
        if header.reason.trim().is_empty() {
            return Err(rejection("invalid_reason", "--reason must not be empty"));
        }
        let sheets = &inspected.extraction.sheets;
        if !sheets.iter().any(|sheet| sheet["name"] == header.sheet) {
            let names: Vec<_> = sheets.iter().map(|sheet| sheet["name"].clone()).collect();
            return Err(Rejection(json!({"code":"sheet_not_found","message":format!("no worksheet {} in this workbook",header.sheet),"sheets":names})).into());
        }
        let mappings_path = dir.join("mappings.yml");
        let mut mappings = read_mappings(&mappings_path)?;
        let recorded = array(&mappings["operations"])?.clone();
        let op_id = match header.id {
            Some(op) => {
                identifier(op).map_err(|_| {
                    rejection(
                        "invalid_operation_id",
                        format!("--id {op} must match ^[a-zA-Z0-9][a-zA-Z0-9_-]*$"),
                    )
                })?;
                op.to_owned()
            }
            None => next_operation_id(&recorded, header.prefix),
        };
        let mut operation = json!({"id":op_id,"sheet":header.sheet,"reason":header.reason,
            "kind":header.kind});
        settings(&inspected, &mut operation)?;
        if let Some(existing) = recorded.iter().find(|o| o["id"] == op_id.as_str()) {
            if *existing == operation {
                return Ok(
                    json!({"state":"unchanged","operation":operation,"content":inspected.fingerprint}),
                );
            }
            return Err(Rejection(json!({"code":"operation_id_conflict","message":format!("operation {op_id} is already recorded with other settings; choose another --id, or omit --id to have one generated"),"recorded":existing})).into());
        }
        let mut values = recorded;
        values.push(operation.clone());
        validate(
            "mappings",
            &json!({"schema_version":"1","operations":values}),
        )
        .map_err(|error| rejection("invalid_operation", format!("{error:#}")))?;
        parse(&values, &inspected)
            .map_err(|error| rejection("invalid_operation", format!("{error:#}")))?;
        let mut planned = Planned::new();
        let changes = content(&dir, &inspected, &values, &mut planned)?;
        mappings["operations"] = Value::Array(values);
        planned.insert(
            "mappings.yml".to_owned(),
            Some(stored_mappings(&mappings_path, &mappings)?),
        );
        let checked = self
            .inspect_with(&dir, false, &planned)
            .map_err(|error| rejection("validation_failed", format!("{error:#}")))?;
        // Every pending change is written once to a scratch copy with the
        // operation, as export will write them.
        let source = under(&self.root, source_path)?;
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
        let mut report = json!({"operation":operation});
        if let Value::Object(changes) = changes {
            report
                .as_object_mut()
                .context("report must be an object")?
                .extend(changes);
        }
        if header.dry_run {
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
