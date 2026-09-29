//! Row and column insertion and deletion on an Excel document. The operation in
//! mappings.yml and the content it adds or removes change together, are
//! validated like `check` before anything is written, and replace the
//! document's directory in one rename.
use super::*;
use crate::excel::{Merges, StructuralOperation};

/// Whether an edit inserts or deletes rows or columns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    Rows,
    Columns,
}

/// Where inserted rows or columns go, relative to a row or column given as a
/// number or letters of the original sheet, a key `<operation ID>-<n>` of one
/// inserted earlier, or `last`.
#[derive(Clone, Debug)]
pub enum Position {
    After(String),
    Before(String),
}

#[derive(Clone, Debug)]
pub enum EditKind {
    Insert {
        position: Position,
        count: Option<u32>,
        /// Rows only: the original row whose formatting the new rows take.
        style_from: Option<String>,
        /// One object per inserted row (keyed by column) or column (keyed by row).
        values: Option<Value>,
    },
    Delete {
        from: String,
        count: u32,
    },
}

pub struct SheetEdit {
    pub axis: Axis,
    pub kind: EditKind,
    pub sheet: String,
    pub id: Option<String>,
    pub reason: String,
    /// The content fingerprint the edit was planned against.
    pub base: Option<String>,
    pub dry_run: bool,
}

pub(super) fn rejection(code: &str, message: impl Into<String>) -> anyhow::Error {
    Rejection(json!({"code":code,"message":message.into()})).into()
}

/// A row or column named by the caller, before it is placed on the sheet.
enum Anchor {
    Original(u32),
    Inserted { id: String, offset: u32 },
    Last,
}

/// The sheet an edit applies to, with the operations recorded before it.
struct Sheet<'a> {
    name: &'a str,
    page: String,
    operations: &'a [StructuralOperation],
    /// The original cells by column, then row.
    cells: BTreeMap<u32, BTreeMap<u32, &'a Value>>,
}

impl<'a> Sheet<'a> {
    fn axis_name(rows: bool) -> &'static str {
        if rows { "row" } else { "column" }
    }

    fn label(position: u32, rows: bool) -> Result<String> {
        Ok(if rows {
            position.to_string()
        } else {
            excel::column_name(position)?
        })
    }

    /// Where an original row or column is after the recorded operations, if kept.
    fn placed(&self, position: u32, rows: bool) -> Result<Option<u32>> {
        let address = if rows {
            format!("A{position}")
        } else {
            format!("{}1", excel::column_name(position)?)
        };
        Ok(
            match excel::map_coordinate(self.name, &address, self.operations)? {
                Some(mapped) => {
                    let (column, row) = excel::coordinate(&mapped)?;
                    Some(if rows { row } else { column })
                }
                None => None,
            },
        )
    }

    /// Where the `offset`-th row or column an operation inserted is now.
    fn inserted(&self, id: &str, offset: u32, rows: bool) -> Result<u32> {
        let (column, row) = if rows {
            (Some("A"), None)
        } else {
            (None, Some(1))
        };
        let address =
            excel::resolve_insertion(self.name, id, offset, column, row, self.operations)?;
        let (column, row) = excel::coordinate(&address)?;
        Ok(if rows { row } else { column })
    }

    /// The last original row or column holding a value.
    fn last_original(&self, rows: bool) -> u32 {
        if rows {
            self.cells
                .values()
                .filter_map(|column| column.keys().next_back())
                .max()
                .copied()
                .unwrap_or(0)
        } else {
            self.cells.keys().next_back().copied().unwrap_or(0)
        }
    }

    /// The last row or column holding a value or added by an operation, after
    /// the recorded operations; 0 on an empty sheet.
    fn last(&self, rows: bool) -> Result<u32> {
        let mut last = 0;
        if rows {
            let occupied: BTreeSet<u32> = self
                .cells
                .values()
                .flat_map(|cells| cells.keys().copied())
                .collect();
            for row in occupied {
                if let Some(placed) = self.placed(row, true)? {
                    last = last.max(placed);
                }
            }
        } else {
            for &column in self.cells.keys() {
                if let Some(placed) = self.placed(column, false)? {
                    last = last.max(placed);
                }
            }
        }
        for operation in self.own(rows) {
            if operation.insertion() {
                for offset in 0..operation.count {
                    if let Ok(position) = self.inserted(&operation.id, offset, rows) {
                        last = last.max(position);
                    }
                }
            }
        }
        Ok(last)
    }

    /// The recorded operations on this sheet's rows or columns.
    fn own(&self, rows: bool) -> impl Iterator<Item = &StructuralOperation> {
        self.operations
            .iter()
            .filter(move |o| o.sheet == self.name && o.row_operation() == rows)
    }

    fn parse(&self, spec: &str, rows: bool) -> Result<Anchor> {
        let axis = Self::axis_name(rows);
        if spec == "last" {
            return Ok(Anchor::Last);
        }
        let original = if rows {
            spec.strip_prefix('r')
                .unwrap_or(spec)
                .parse::<u32>()
                .ok()
                .filter(|n| (1..=1_048_576).contains(n))
        } else {
            Some(spec.to_ascii_uppercase())
                .filter(|letters| letters.bytes().all(|b| b.is_ascii_uppercase()))
                .and_then(|letters| excel::column_number(&letters).ok())
        };
        if let Some(position) = original {
            return Ok(Anchor::Original(position));
        }
        if let Some((id, number)) = spec.rsplit_once('-')
            && let Ok(number) = number.parse::<u32>()
            && let Some(operation) = self.own(rows).find(|o| o.id == id && o.insertion())
            && (1..=operation.count).contains(&number)
        {
            return Ok(Anchor::Inserted {
                id: id.to_owned(),
                offset: number - 1,
            });
        }
        let form = if rows {
            "a row number of the original sheet (15 or r15)"
        } else {
            "column letters of the original sheet (C)"
        };
        Err(rejection(
            "invalid_position",
            format!(
                "{spec} is not {form}, the key <operation ID>-<n> of a {axis} inserted on {} by a recorded operation, or last",
                self.name
            ),
        ))
    }

    /// The position of `anchor` after the recorded operations. An original row
    /// or column must hold a value or lie above or left of one, and still exist.
    fn place(&self, anchor: &Anchor, rows: bool) -> Result<u32> {
        let axis = Self::axis_name(rows);
        match anchor {
            Anchor::Last => self.last(rows),
            Anchor::Inserted { id, offset } => self.inserted(id, *offset, rows),
            Anchor::Original(position) => {
                let last = self.last_original(rows);
                if *position > last {
                    return Err(rejection(
                        "invalid_position",
                        format!(
                            "{axis} {} of {} is past the last {axis} holding a value ({}); use last to add after it",
                            Self::label(*position, rows)?,
                            self.name,
                            if last == 0 {
                                "the sheet has none".to_owned()
                            } else {
                                Self::label(last, rows)?
                            }
                        ),
                    ));
                }
                self.placed(*position, rows)?.ok_or_else(|| {
                    rejection(
                        "invalid_position",
                        format!(
                            "{axis} {} of {} is deleted by a recorded operation",
                            Self::label(*position, rows).unwrap_or_default(),
                            self.name
                        ),
                    )
                })
            }
        }
    }

    /// The original row whose formatting a row inserted next to `anchor` takes
    /// by default: like Excel, the row above.
    fn default_style(&self, anchor: &Anchor, after: bool) -> Result<Option<u32>> {
        Ok(match anchor {
            Anchor::Original(row) if after => Some(*row),
            Anchor::Original(row) => {
                let above = row - 1;
                let adjacent = above > 0
                    && match (self.placed(above, true)?, self.placed(*row, true)?) {
                        (Some(above), Some(row)) => above + 1 == row,
                        _ => false,
                    };
                adjacent.then_some(above)
            }
            Anchor::Inserted { id, .. } => self
                .operations
                .iter()
                .find(|o| &o.id == id)
                .and_then(|o| o.style_from),
            Anchor::Last => {
                let last = self.last(true)?;
                let operations: Vec<_> = self.own(true).collect();
                let mut style = excel::original_position(last, &operations, true)
                    .filter(|row| *row > 0 && *row <= self.last_original(true));
                if style.is_none() {
                    for operation in self.own(true).filter(|o| o.insertion()) {
                        if (0..operation.count).any(|offset| {
                            self.inserted(&operation.id, offset, true).ok() == Some(last)
                        }) {
                            style = operation.style_from;
                        }
                    }
                }
                style
            }
        })
    }

    /// The nearest original cell in `column` at or above `row`, with its row.
    fn reference(&self, column: u32, row: u32) -> Option<(&'a Value, u32)> {
        self.cells
            .get(&column)?
            .range(..=row)
            .next_back()
            .map(|(row, cell)| (*cell, *row))
    }
}

/// Excel's serial day number for a date written `YYYY/M/D` or `YYYY-M-D`.
fn date_serial(text: &str, date1904: bool) -> Option<i64> {
    let parts: Vec<&str> = text.trim().split(['/', '-']).collect();
    let [year, month, day] = parts.as_slice() else {
        return None;
    };
    let (year, month, day): (i64, i64, i64) =
        (year.parse().ok()?, month.parse().ok()?, day.parse().ok()?);
    if !(1..=12).contains(&month) || !(1900..=9999).contains(&year) {
        return None;
    }
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let days_in_month = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if !(1..=days_in_month[(month - 1) as usize]).contains(&day) {
        return None;
    }
    // Days from 1970-01-01 (Howard Hinnant's days_from_civil).
    let days = |y: i64, m: i64, d: i64| {
        let y = if m <= 2 { y - 1 } else { y };
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146_097 + doe - 719_468
    };
    let serial = if date1904 {
        days(year, month, day) - days(1904, 1, 1)
    } else {
        // Serial 1 is 1900-01-01 and Excel counts a 29 February 1900 that never
        // was, so from March 1900 the serial is the day count from 1899-12-30.
        if (year, month) < (1900, 3) {
            return None;
        }
        days(year, month, day) - days(1899, 12, 30)
    };
    (serial >= 0).then_some(serial)
}

/// Whether an Excel number format shows a date: a day or year code outside
/// quoted text, escapes and bracketed sections.
fn date_format(format: &str) -> bool {
    let mut plain = String::new();
    let mut chars = format.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                for c in chars.by_ref() {
                    if c == '"' {
                        break;
                    }
                }
            }
            '[' => {
                for c in chars.by_ref() {
                    if c == ']' {
                        break;
                    }
                }
            }
            '\\' => {
                chars.next();
            }
            c => plain.push(c.to_ascii_lowercase()),
        }
    }
    plain.contains('y') || plain.contains('d')
}

/// The value converted to the type of the reference cell of its column, so a
/// number typed into a text column stays text and a date lands as a date.
fn converted(
    value: &Value,
    reference: Option<(&Value, u32)>,
    date1904: bool,
    place: &str,
    warnings: &mut Vec<String>,
) -> Result<Value> {
    ensure!(
        !value.is_object() && !value.is_array(),
        rejection(
            "invalid_values",
            format!("{place}: a value must be text, a number, true/false or null")
        )
    );
    let Some((cell, row)) = reference else {
        return Ok(value.clone());
    };
    let mismatch = |expected: &str| {
        rejection(
            "value_type_mismatch",
            format!(
                "{place}: {value} does not fit the {expected} values of row {row} in the same column"
            ),
        )
    };
    Ok(match (string(&cell["type"]).unwrap_or(""), value) {
        ("string", Value::String(_)) | ("number", Value::Number(_)) => value.clone(),
        ("boolean", Value::Bool(_)) => value.clone(),
        ("string", Value::Number(number)) => json!(number.to_string()),
        ("string", Value::Bool(_)) => return Err(mismatch("text")),
        ("number", Value::String(text)) => {
            let date = cell["number_format"].as_str().is_some_and(date_format);
            if date && let Some(serial) = date_serial(text, date1904) {
                json!(serial)
            } else if let Ok(integer) = text.trim().parse::<i64>() {
                json!(integer)
            } else if let Some(number) = text
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|n| n.is_finite())
                .and_then(serde_json::Number::from_f64)
            {
                Value::Number(number)
            } else if date {
                return Err(mismatch("date (write YYYY/MM/DD)"));
            } else {
                return Err(mismatch("number"));
            }
        }
        ("number", Value::Bool(_)) => return Err(mismatch("number")),
        ("boolean", Value::String(text)) => match text.trim().to_ascii_lowercase().as_str() {
            "true" => json!(true),
            "false" => json!(false),
            _ => return Err(mismatch("true/false")),
        },
        ("boolean", Value::Number(_)) => return Err(mismatch("true/false")),
        ("formula", _) => {
            warnings.push(format!(
                "{place}: row {row} of this column holds a formula; the new cell gets the value, not a formula"
            ));
            value.clone()
        }
        _ => value.clone(),
    })
}

/// A content value to add: the row key, the column key and the value.
type Addition = (String, String, Value);

struct Plan {
    operation: Value,
    additions: Vec<Addition>,
    conversions: Vec<Value>,
    warnings: Vec<String>,
    /// First and last position of the edited rows or columns on the final sheet.
    span: (u32, u32),
}

impl Store {
    /// Inserts or deletes rows or columns of an adopted document, or of the
    /// proposal of `id`, recording the operation and its content together.
    pub fn edit_sheet(&self, id: &str, proposal: bool, edit: &SheetEdit) -> Result<Value> {
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
        let parser = extraction["parser"].as_str().unwrap_or("");
        // Word and PowerPoint lay paragraphs and table rows out as rows.
        let laid_out = parser.contains(";word-blocks/") || parser.contains(";slide-blocks/");
        if !parser.contains(";cells/") && !laid_out {
            return Err(rejection(
                "unsupported_format",
                "row edits apply to Excel, Word and PowerPoint documents, column edits to Excel only",
            ));
        }
        if laid_out && edit.axis == Axis::Columns {
            return Err(rejection(
                "unsupported_format",
                "Word and PowerPoint lay out table columns themselves; insert or delete them in Word or PowerPoint and re-import",
            ));
        }
        // Slides a slide operation inserts take row edits like the others.
        let operated = operated_extraction(extraction, &inspected.mappings["operations"])?;
        let sheets = array(&operated["sheets"])?;
        let Some(index) = sheets.iter().position(|s| s["name"] == edit.sheet) else {
            let names: Vec<_> = sheets.iter().map(|s| s["name"].clone()).collect();
            return Err(Rejection(json!({"code":"sheet_not_found","message":format!("{} has no sheet {}", id, edit.sheet),"sheets":names})).into());
        };
        if !inspected.source_current {
            return Err(rejection(
                "source_changed",
                "the original changed or is missing; re-import it before editing rows or columns",
            ));
        }
        let mappings_path = dir.join("mappings.yml");
        let mut mappings = read(&mappings_path, Some("mappings"))?;
        let recorded = array(&mappings["operations"])?.clone();
        let repeated = match &edit.id {
            Some(op) => {
                identifier(op).map_err(|_| {
                    rejection(
                        "invalid_operation_id",
                        format!("--id {op} must match ^[a-zA-Z0-9][a-zA-Z0-9_-]*$"),
                    )
                })?;
                recorded.iter().position(|o| o["id"] == op.as_str())
            }
            None => None,
        };
        let prior_values = &recorded[..repeated.unwrap_or(recorded.len())];
        let prior = excel::parse_operations(prior_values, sheets)?;
        let op_id = match &edit.id {
            Some(op) => op.clone(),
            None => generated_id(&recorded, edit),
        };
        let mut cells: BTreeMap<u32, BTreeMap<u32, &Value>> = BTreeMap::new();
        for cell in array(&sheets[index]["cells"])? {
            let (column, row) = excel::coordinate(string(&cell["address"])?)?;
            cells.entry(column).or_default().insert(row, cell);
        }
        let sheet = Sheet {
            name: &edit.sheet,
            page: sheets[index]["page"]
                .as_str()
                .map_or_else(|| format!("sheet-{}", index + 1), str::to_owned),
            operations: &prior,
            cells,
        };
        let source = under(&self.root, string(&inspected.meta["source"]["path"])?)?;
        let book = Source::open(&source)?;
        let date1904 = matches!(&book, Source::Excel(workbook) if workbook.date1904);
        let plan = plan(&sheet, edit, &op_id, date1904)?;

        // Inspection already resolved page IDs to files. Only read pages this
        // operation may change or needs for its preview.
        let mut pages = BTreeMap::new();
        let page_name = inspected
            .page_files
            .get(&sheet.page)
            .cloned()
            .context("the sheet has no content page")?;
        pages.insert(
            page_name.clone(),
            read(&under(&dir, &page_name)?, Some("content"))?,
        );
        let mut new_values = prior_values.to_vec();
        new_values.push(plan.operation.clone());
        let operations = excel::parse_operations(&new_values, sheets)?;

        if let Some(index) = repeated {
            let present = plan.additions.iter().all(|(row, column, value)| {
                pages[&page_name]["blocks"]["table-1"]["rows"][row][column] == *value
            });
            if recorded[index] == plan.operation
                && present
                && deleted_content(&inspected, &sheet, &operations)?.is_empty()
            {
                return Ok(
                    json!({"state":"unchanged","operation":plan.operation,"span":span(&plan, edit.axis)?,"content":inspected.fingerprint}),
                );
            }
            return Err(Rejection(json!({"code":"operation_id_conflict","message":format!("operation {op_id} is already recorded with other settings or values; choose another --id, or omit --id to have one generated"),"recorded":recorded[index]})).into());
        }

        let mut changed = BTreeSet::new();
        for (page, block, row, column) in deleted_content(&inspected, &sheet, &operations)? {
            let name = inspected
                .page_files
                .get(&page)
                .cloned()
                .context("content page missing")?;
            if !pages.contains_key(&name) {
                pages.insert(name.clone(), read(&under(&dir, &name)?, Some("content"))?);
            }
            let rows = pages.get_mut(&name).unwrap()["blocks"][&block]["rows"]
                .as_object_mut()
                .context("content rows missing")?;
            if let Some(values) = rows.get_mut(&row).and_then(Value::as_object_mut) {
                values.remove(&column);
                if values.is_empty() {
                    rows.remove(&row);
                }
            }
            changed.insert(name);
        }
        if !plan.additions.is_empty() {
            let rows = pages.get_mut(&page_name).unwrap()["blocks"]["table-1"]["rows"]
                .as_object_mut()
                .ok_or_else(|| {
                    rejection(
                        "invalid_values",
                        format!(
                            "{} has no table content to add values to; insert without values",
                            edit.sheet
                        ),
                    )
                })?;
            for (row, column, value) in &plan.additions {
                rows.entry(row.clone()).or_insert_with(|| json!({}))[column] = value.clone();
            }
            changed.insert(page_name.clone());
        }
        // Values a merge grown by the insertion would hide.
        let merges = excel::merges_after(&sheets[index], &operations)?;
        let merges = Merges::new(&merges)?;
        for (row, column, _) in &plan.additions {
            let (axis_key, offset_key, row_axis) = match edit.axis {
                Axis::Rows => (row, column, true),
                Axis::Columns => (column, row, false),
            };
            let offset = axis_key
                .rsplit_once('-')
                .and_then(|(_, n)| n.parse::<u32>().ok())
                .context("inserted key")?
                - 1;
            let address = if row_axis {
                excel::resolve_insertion(
                    &edit.sheet,
                    &op_id,
                    offset,
                    Some(offset_key),
                    None,
                    &operations,
                )?
            } else {
                let original_row = offset_key.trim_start_matches('r').parse::<u32>()?;
                excel::resolve_insertion(
                    &edit.sheet,
                    &op_id,
                    offset,
                    None,
                    Some(original_row),
                    &operations,
                )?
            };
            if let Err(error) = merges.ensure_not_hidden(&edit.sheet, &address) {
                return Err(rejection(
                    "merged_non_anchor",
                    format!("{row}/{column}: {error:#}"),
                ));
            }
        }
        mappings["operations"] = Value::Array(new_values);

        let mut planned = BTreeMap::new();
        planned.insert(
            "mappings.yml".to_owned(),
            Some(serialized(&mappings_path, &mappings)?),
        );
        for name in &changed {
            planned.insert(
                name.clone(),
                Some(serialized(Path::new(name), &pages[name])?),
            );
        }
        book.ensure_row_edits_supported(&operations, array(&mappings["operations"])?)
            .map_err(|error| rejection("structural_edit_unsupported", format!("{error:#}")))?;
        let result = self
            .inspect_with(&dir, false, &planned)
            .map_err(|error| rejection("validation_failed", format!("{error:#}")))?;

        let mut report = json!({
            "operation":plan.operation,
            "span":span(&plan, edit.axis)?,
            "page":page_name,
            "keys":plan.additions.iter().map(|(row, column, _)| match edit.axis {
                Axis::Rows => row.clone(),
                Axis::Columns => column.clone(),
            }).collect::<BTreeSet<_>>(),
            "values":plan.additions.len(),
        });
        if !plan.conversions.is_empty() {
            report["converted"] = json!(plan.conversions);
        }
        if !plan.warnings.is_empty() {
            report["warnings"] = json!(plan.warnings);
        }
        if edit.axis == Axis::Rows {
            report["neighbors"] = neighbors(
                &pages[&page_name],
                &sheet,
                &operations,
                &op_id,
                plan.span,
                matches!(edit.kind, EditKind::Insert { .. }),
            )?;
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

    /// Replaces the document's directory with a copy holding the planned files.
    pub(super) fn commit(&self, dir: &Path, planned: &Planned, base: &str) -> Result<()> {
        let stage = Stage::new_in(&under(&self.arp, "work")?, &self.arp)?;
        let ready = stage.path().join("ready");
        fs::create_dir(&ready)?;
        for (name, path) in self.planned_files(dir, planned)? {
            let target = under(&ready, &name)?;
            match planned.get(&name) {
                Some(Some(bytes)) => replace(&target, bytes)?,
                _ => {
                    fs::create_dir_all(target.parent().context("missing parent")?)?;
                    fs::copy(&path, &target)?;
                }
            }
        }
        ensure!(
            self.fingerprint(dir)?.as_deref() == Some(base),
            "document changed during the edit"
        );
        let backup = stage.path().join("previous");
        fs::rename(dir, &backup)?;
        if let Err(error) = fs::rename(&ready, dir) {
            if let Err(restore) = fs::rename(&backup, dir) {
                let recovery = stage.keep();
                bail!(
                    "edit failed: {error}; rollback failed: {restore}; recovery data: {}",
                    recovery.display()
                );
            }
            return Err(error.into());
        }
        Ok(())
    }
}

/// `add-rows-1`, `del-cols-2`, ...: the first number no recorded operation uses.
fn generated_id(recorded: &[Value], edit: &SheetEdit) -> String {
    let prefix = match (&edit.kind, edit.axis) {
        (EditKind::Insert { .. }, Axis::Rows) => "add-rows",
        (EditKind::Delete { .. }, Axis::Rows) => "del-rows",
        (EditKind::Insert { .. }, Axis::Columns) => "add-cols",
        (EditKind::Delete { .. }, Axis::Columns) => "del-cols",
    };
    next_operation_id(recorded, prefix)
}

fn span(plan: &Plan, axis: Axis) -> Result<Value> {
    let rows = axis == Axis::Rows;
    Ok(json!({
        "first":Sheet::label(plan.span.0, rows)?,
        "last":Sheet::label(plan.span.1, rows)?,
    }))
}

fn plan(sheet: &Sheet<'_>, edit: &SheetEdit, id: &str, date1904: bool) -> Result<Plan> {
    let rows = edit.axis == Axis::Rows;
    let axis = Sheet::axis_name(rows);
    let kind = match (&edit.kind, rows) {
        (EditKind::Insert { .. }, true) => "insert_rows",
        (EditKind::Delete { .. }, true) => "delete_rows",
        (EditKind::Insert { .. }, false) => "insert_columns",
        (EditKind::Delete { .. }, false) => "delete_columns",
    };
    let mut operation = json!({"id":id,"kind":kind,"sheet":sheet.name,"reason":edit.reason});
    let mut additions = vec![];
    let mut conversions = vec![];
    let mut warnings = vec![];
    ensure!(
        !edit.reason.trim().is_empty(),
        rejection("invalid_reason", "--reason must not be empty")
    );
    let span = match &edit.kind {
        EditKind::Insert {
            position,
            count,
            style_from,
            values,
        } => {
            let (spec, after) = match position {
                Position::After(spec) => (spec, true),
                Position::Before(spec) => (spec, false),
            };
            let anchor = sheet.parse(spec, rows)?;
            let placed = sheet.place(&anchor, rows)?;
            let at = if after { placed + 1 } else { placed.max(1) };
            let values = match values {
                None => vec![],
                Some(Value::Array(items)) => items.clone(),
                Some(item @ Value::Object(_)) => vec![item.clone()],
                Some(_) => {
                    return Err(rejection(
                        "invalid_values",
                        format!("--values must hold a list with one object per inserted {axis}"),
                    ));
                }
            };
            let count = match (count, values.len()) {
                (Some(count), 0) => *count,
                (None, 0) => 1,
                (None, n) => u32::try_from(n)?,
                (Some(count), n) if *count as usize == n => *count,
                (Some(count), n) => {
                    return Err(rejection(
                        "count_mismatch",
                        format!("--count {count} does not match the {n} {axis}(s) in --values"),
                    ));
                }
            };
            ensure!(
                count >= 1,
                rejection("invalid_count", "--count must be at least 1")
            );
            operation["at"] = json!(at);
            operation["count"] = json!(count);
            let style = if rows {
                match style_from {
                    Some(spec) => match sheet.parse(spec, true)? {
                        Anchor::Original(row) => Some(row),
                        _ => {
                            return Err(rejection(
                                "invalid_position",
                                "--style-from names a row of the original sheet (15 or r15)",
                            ));
                        }
                    },
                    None => sheet.default_style(&anchor, after)?,
                }
            } else {
                ensure!(
                    style_from.is_none(),
                    rejection(
                        "invalid_arguments",
                        "--style-from applies to rows; inserted columns take the format of the column to their left, as in Excel"
                    )
                );
                None
            };
            if let Some(row) = style {
                operation["style_from"] = json!(row);
            }
            // A new row's values take the types of their columns. A new column
            // is new data, so its values stay as given.
            let reference_row = if rows { style } else { None };
            for (index, item) in values.iter().enumerate() {
                let key = format!("{id}-{}", index + 1);
                let Some(item) = item.as_object() else {
                    return Err(rejection(
                        "invalid_values",
                        format!("--values item {}: expected an object", index + 1),
                    ));
                };
                for (name, value) in item {
                    if value.is_null() {
                        continue;
                    }
                    let (row_key, column_key, reference) = if rows {
                        let column = name.to_ascii_uppercase();
                        let letters = column.bytes().all(|b| b.is_ascii_uppercase());
                        let number = excel::column_number(&column).ok().filter(|_| letters).ok_or_else(|| {
                            rejection(
                                "invalid_values",
                                format!("{key}/{name}: an inserted row takes values by column letters (A, B, ...)"),
                            )
                        })?;
                        if sheet.placed(number, false)?.is_none() {
                            return Err(rejection(
                                "invalid_values",
                                format!(
                                    "{key}/{column}: column {column} is deleted by a recorded operation"
                                ),
                            ));
                        }
                        let reference = reference_row.and_then(|row| sheet.reference(number, row));
                        (key.clone(), column, reference)
                    } else {
                        let row = name
                            .strip_prefix('r')
                            .unwrap_or(name)
                            .parse::<u32>()
                            .ok()
                            .filter(|n| (1..=1_048_576).contains(n))
                            .ok_or_else(|| {
                                rejection(
                                    "invalid_values",
                                    format!("{key}/{name}: an inserted column takes values by row numbers of the original sheet (8 or r8)"),
                                )
                            })?;
                        if sheet.placed(row, true)?.is_none() {
                            return Err(rejection(
                                "invalid_values",
                                format!(
                                    "{key}/r{row}: row {row} is deleted by a recorded operation"
                                ),
                            ));
                        }
                        (format!("r{row}"), key.clone(), None)
                    };
                    let place = format!("{row_key}/{column_key}");
                    let stored = converted(value, reference, date1904, &place, &mut warnings)?;
                    if stored != *value {
                        conversions.push(json!({"key":place,"from":value,"to":stored}));
                    }
                    additions.push((row_key, column_key, stored));
                }
            }
            (at, at + count - 1)
        }
        EditKind::Delete { from, count } => {
            ensure!(
                *count >= 1,
                rejection("invalid_count", "--count must be at least 1")
            );
            let Anchor::Original(first) = sheet.parse(from, rows)? else {
                return Err(rejection(
                    "invalid_position",
                    format!(
                        "--from names a {axis} of the original sheet; to take back inserted {axis}s, remove their operation"
                    ),
                ));
            };
            let at = sheet.place(&Anchor::Original(first), rows)?;
            for offset in 0..*count {
                let position = first
                    .checked_add(offset)
                    .filter(|p| *p <= if rows { 1_048_576 } else { 16_384 })
                    .context("deletion runs past the sheet")?;
                if sheet.placed(position, rows)? != Some(at + offset) {
                    return Err(rejection(
                        "invalid_position",
                        format!(
                            "{axis}s {}..{} of {} are not adjacent on the sheet: a recorded operation inserted or deleted {axis}s between them; delete them in separate steps",
                            Sheet::label(first, rows)?,
                            Sheet::label(position, rows)?,
                            sheet.name
                        ),
                    ));
                }
            }
            operation["at"] = json!(at);
            operation["count"] = json!(count);
            (at, at + count - 1)
        }
    };
    Ok(Plan {
        operation,
        additions,
        conversions,
        warnings,
        span,
    })
}

/// The content values (page, block, row, column) of cells the new operation
/// deletes. They leave the content with the cells, so none is silently dropped.
fn deleted_content(
    inspected: &Inspection,
    sheet: &Sheet<'_>,
    operations: &[StructuralOperation],
) -> Result<Vec<(String, String, String, String)>> {
    let gone = |target: &MappingTarget<'_>, operations: &[StructuralOperation]| -> Result<bool> {
        Ok(match target {
            MappingTarget::Cell { sheet: name, cell } => {
                excel::map_coordinate(name, cell, operations)?.is_none()
            }
            MappingTarget::InsertionRow {
                sheet: name,
                insertion,
                offset,
                column,
            } => excel::resolve_insertion(name, insertion, *offset, Some(column), None, operations)
                .is_err(),
            MappingTarget::InsertionColumn {
                sheet: name,
                insertion,
                offset,
                row,
            } => excel::resolve_insertion(name, insertion, *offset, None, Some(*row), operations)
                .is_err(),
            // A shape's anchor moves with the cells; its text stays.
            MappingTarget::Shape { .. } => false,
        })
    };
    let mut deleted = vec![];
    for entry in array(&inspected.mappings["entries"])? {
        let Some(position) = entry["position"].as_object() else {
            continue;
        };
        let Some(target) = mapping_target(&entry["target"])? else {
            continue;
        };
        let name = match &target {
            MappingTarget::Cell { sheet, .. }
            | MappingTarget::InsertionRow { sheet, .. }
            | MappingTarget::InsertionColumn { sheet, .. }
            | MappingTarget::Shape { sheet, .. } => *sheet,
        };
        if name != sheet.name {
            continue;
        }
        if gone(&target, operations)? && !gone(&target, sheet.operations)? {
            deleted.push((
                string(&entry["page"])?.to_owned(),
                string(&entry["block"])?.to_owned(),
                string(&position["row"])?.to_owned(),
                string(&position["column"])?.to_owned(),
            ));
        }
    }
    Ok(deleted)
}

/// The content rows next to the edited rows, so the caller can confirm the place.
fn neighbors(
    page: &Value,
    sheet: &Sheet<'_>,
    operations: &[StructuralOperation],
    id: &str,
    span: (u32, u32),
    inserted: bool,
) -> Result<Value> {
    let Some(rows) = page["blocks"]["table-1"]["rows"].as_object() else {
        return Ok(json!({}));
    };
    // Rows as they stand around the edit: after an insertion, or before a deletion.
    let operations = if inserted {
        operations
    } else {
        sheet.operations
    };
    let mut placed = BTreeMap::new();
    for key in rows.keys() {
        let row = if let Some(number) = key.strip_prefix('r').and_then(|n| n.parse::<u32>().ok()) {
            excel::map_coordinate(sheet.name, &format!("A{number}"), operations)?
                .map(|address| excel::coordinate(&address).map(|(_, row)| row))
                .transpose()?
        } else if let Some((op, number)) = key.rsplit_once('-')
            && op != id
            && let Ok(number) = number.parse::<u32>()
        {
            excel::resolve_insertion(sheet.name, op, number - 1, Some("A"), None, operations)
                .ok()
                .map(|address| excel::coordinate(&address).map(|(_, row)| row))
                .transpose()?
        } else {
            None
        };
        if let Some(row) = row {
            placed.insert(row, key);
        }
    }
    let describe = |(row, key): (&u32, &&String)| {
        let values: serde_json::Map<String, Value> = rows[key.as_str()]
            .as_object()
            .into_iter()
            .flatten()
            .take(4)
            .map(|(column, value)| {
                let value = match value.as_str() {
                    Some(text) if text.chars().count() > 40 => {
                        json!(format!("{}…", text.chars().take(40).collect::<String>()))
                    }
                    _ => value.clone(),
                };
                (column.clone(), value)
            })
            .collect();
        json!({"row":row,"key":key,"values":values})
    };
    let mut result = json!({});
    if let Some(above) = placed.range(..span.0).next_back() {
        result["above"] = describe(above);
    }
    if let Some(below) = placed.range(span.1 + 1..).next() {
        result["below"] = describe(below);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "manual sparse last row style performance measurement"]
    fn measure_sparse_last_row_style() {
        use std::{hint::black_box, time::Instant};

        let value = json!(1);
        let cells = BTreeMap::from([(1, BTreeMap::from([(900_000, &value)]))]);
        let operations = [StructuralOperation {
            sheet: "S".to_owned(),
            id: "add".to_owned(),
            kind: crate::excel::OperationKind::InsertRows,
            at: 900_001,
            count: 1,
            style_from: None,
        }];
        let sheet = Sheet {
            name: "S",
            page: "sheet-1".to_owned(),
            operations: &operations,
            cells,
        };
        let start = Instant::now();
        assert_eq!(
            black_box(sheet.default_style(&Anchor::Last, true).unwrap()),
            None
        );
        eprintln!("sparse_last_row_style_us={}", start.elapsed().as_micros());
    }

    #[test]
    #[ignore = "manual content page lookup performance measurement"]
    fn measure_content_page_lookup() {
        use std::{hint::black_box, time::Instant};

        let pages: BTreeMap<_, _> = (0..200)
            .map(|index| {
                (
                    format!("content/{index:03}.yml"),
                    json!({"page_id":format!("sheet-{index}")}),
                )
            })
            .collect();
        let page_names: BTreeMap<_, _> = pages
            .iter()
            .map(|(name, value)| (value["page_id"].as_str().unwrap().to_owned(), name.clone()))
            .collect();
        let requests: Vec<_> = (0..2_000).map(|_| "sheet-199").collect();
        let start = Instant::now();
        for page in &requests {
            black_box(
                pages
                    .iter()
                    .find(|(_, value)| value["page_id"] == *page)
                    .unwrap()
                    .0,
            );
        }
        let scan = start.elapsed();
        let start = Instant::now();
        for page in &requests {
            black_box(page_names.get(*page).unwrap());
        }
        eprintln!(
            "content_page_scan_ms={} indexed_ms={}",
            scan.as_millis(),
            start.elapsed().as_millis()
        );
    }

    #[test]
    #[ignore = "manual sheet position performance measurement"]
    fn measure_last_occupied_row() {
        use std::{hint::black_box, time::Instant};

        let value = json!(1);
        let cells: BTreeMap<_, _> = (1..=20)
            .map(|column| (column, (1..=2000).map(|row| (row, &value)).collect()))
            .collect();
        let sheet = Sheet {
            name: "S",
            page: "sheet-1".to_owned(),
            operations: &[],
            cells,
        };
        let mut original = Vec::new();
        let mut optimized = Vec::new();
        for _ in 0..5 {
            let start = Instant::now();
            let mut last = 0;
            for cells in sheet.cells.values() {
                for row in cells.keys() {
                    if let Some(placed) = sheet.placed(*row, true).unwrap() {
                        last = last.max(placed);
                    }
                }
            }
            original.push(start.elapsed().as_secs_f64() * 1000.0);
            let start = Instant::now();
            assert_eq!(black_box(sheet.last(true).unwrap()), last);
            optimized.push(start.elapsed().as_secs_f64() * 1000.0);
        }
        original.sort_by(f64::total_cmp);
        optimized.sort_by(f64::total_cmp);
        eprintln!(
            "sheet_last_original_ms={:.3} optimized_ms={:.3}",
            original[2], optimized[2]
        );
    }

    #[test]
    fn dates_become_excel_serials() {
        assert_eq!(date_serial("2025/11/21", false), Some(45982));
        assert_eq!(date_serial("1900-03-01", false), Some(61));
        assert_eq!(date_serial("1904/1/2", true), Some(1));
        assert_eq!(date_serial("2025/02/29", false), None);
        assert_eq!(date_serial("1900/02/28", false), None);
        assert_eq!(date_serial("21/11/2025", false), None);
        assert!(date_format("yyyy/m/d"));
        assert!(date_format("[$-ja-JP]ggge\"年\"m\"月\"d\"日\""));
        assert!(!date_format("#,##0"));
        assert!(!date_format("\"day\"0"));
        assert!(!date_format("h:mm"));
        assert!(!date_format("General"));
    }
}
