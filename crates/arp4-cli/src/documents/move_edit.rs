//! Rows of an Excel worksheet moved, as Excel moves cut rows it inserts
//! elsewhere: recorded as the operations that leave the workbook as that
//! move does — rows inserted at the place, taking the formats, heights and
//! merges of the rows moved; their values and formulas copied there, and
//! their links, notes and validations set there; and the rows deleted —
//! with the formulas reading the rows, in them or elsewhere, given the text
//! Excel gives them, written as the cells stand after the move. A move
//! whose names, rules or drawings Excel would keep with the moved cells
//! otherwise than those operations do is refused. What the move meets is
//! read from the workbook every pending change writes, in its rows.
use super::sheet_edit::rejection;
use super::*;

pub struct MoveRequest {
    pub sheet: String,
    /// The first row moved, of the original sheet.
    pub from: u32,
    pub count: u32,
    /// Where the rows go: after or before a row of the original sheet, the key
    /// of one an operation inserted, or last.
    pub position: Position,
    pub id: Option<String>,
    pub reason: String,
    pub base: Option<String>,
    pub dry_run: bool,
    pub vba_unchanged: bool,
}

/// A link, note or validation of the rows moved, set again where they go.
struct Carried {
    kind: &'static str,
    /// Its cells, as the pending changes leave them.
    range: String,
    settings: Value,
}

/// A drawing on the rows moved, placed again where they go: its ID and
/// its corners, each a column, a row and the offsets in the cell, the rows
/// as the pending changes leave them.
struct Placed {
    drawing: String,
    corners: [(u32, u32, u64, u64); 2],
}

/// The first and last row of a range of a list of ranges, when it names rows.
fn rows_of(range: &str) -> Option<(u32, u32)> {
    let mut ends = range.split(':').map(|end| excel::coordinate(end).ok());
    let (_, a) = ends.next()??;
    let (_, b) = ends.next().unwrap_or(Some((0, a)))?;
    Some((a.min(b), a.max(b)))
}

/// `range`, of the rows from `first`, moved to the rows from `to`.
fn moved_range(range: &str, first: u32, to: u32) -> Result<String> {
    range
        .split(':')
        .map(|end| -> Result<String> {
            let (column, row) = excel::coordinate(end)?;
            Ok(format!(
                "{}{}",
                excel::column_name(column)?,
                to + row - first
            ))
        })
        .collect::<Result<Vec<_>>>()
        .map(|ends| ends.join(":"))
}

/// The files of the document folder `dir`, by name, to bring back.
fn snapshot(dir: &Path) -> Result<BTreeMap<String, Vec<u8>>> {
    crate::data::files(dir)?
        .into_iter()
        .map(|(name, path)| Ok((name, fs::read(path)?)))
        .collect()
}

/// Brings the document folder `dir` back to `files`.
fn restore(dir: &Path, files: &BTreeMap<String, Vec<u8>>) -> Result<()> {
    for (name, path) in crate::data::files(dir)? {
        if !files.contains_key(&name) {
            fs::remove_file(path)?;
        }
    }
    for (name, bytes) in files {
        let path = under(dir, name)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        replace(&path, bytes)?;
    }
    Ok(())
}

fn refuse<T>(message: String) -> Result<T> {
    Err(rejection("structural_edit_unsupported", message))
}

/// What the rows `first` to `last` of `sheet` of the workbook `now` meet when
/// they go to the row `at`, all as the pending changes leave them: the links,
/// notes and validations they carry, to set again where they go. A move that
/// formulas, names, rules or drawings would follow differently in Excel is
/// refused.
fn carried_with(
    now: &excel::Workbook,
    sheet: &str,
    (first, last): (u32, u32),
    at: u32,
) -> Result<(Vec<Carried>, Vec<Placed>)> {
    if (first..=last + 1).contains(&at) {
        return Err(rejection(
            "invalid_position",
            "the rows go to a place among themselves or where they are; name a row outside them",
        ));
    }
    let moved = |row: u32| (first..=last).contains(&row);
    // The rows the moved rows go between.
    let (above, below) = (at - 1, at);
    let goes_inside = |(a, b): (u32, u32)| a <= above && below <= b;
    let holds = |(a, b): (u32, u32)| a < first && last < b && goes_inside((a, b));
    // A reference to a range keeps its ends, as Excel keeps them, when its
    // first and last rows stay, or when the rows and their new place are
    // both inside it; otherwise Excel moves an end with its cells.
    let keeps_ends = |(a, b): (u32, u32)| {
        a < b && (!(moved(a) || moved(b)) || (a <= first && last <= b && goes_inside((a, b))))
    };
    let meets = |rows: Option<(u32, u32)>| rows.is_some_and(|(a, b)| a <= last && first <= b);
    // Defined names; a print area or title over whole rows moves as Excel
    // moves them.
    if let Some(bytes) = now.parts.get("xl/workbook.xml") {
        let workbook = roxmltree::Document::parse(std::str::from_utf8(bytes)?)?;
        for name in workbook
            .descendants()
            .filter(|n| n.tag_name().name() == "definedName")
        {
            let label = name.attribute("name").unwrap_or_default();
            if label.starts_with("_xlnm.") {
                continue;
            }
            let text = name.text().unwrap_or_default();
            for reference in excel::referenced(text).unwrap_or_default() {
                if reference.sheet.as_deref() == Some(sheet)
                    && meets(reference.rows)
                    && !reference.rows.is_some_and(keeps_ends)
                {
                    return refuse(format!(
                        "the name {label} refers to the rows moved ({text}); move them in Excel"
                    ));
                }
            }
        }
    }
    let index = now
        .sheets
        .iter()
        .position(|s| s["name"] == sheet)
        .context("moved sheet missing")?;
    let fields = &now.sheets[index];
    // Links, notes and rules stay on their cells in Excel: one on the rows
    // moved goes with them, and one the rows go into does not take them.
    // Rows inserted and deleted inside a range leave it as that.
    let crosses = |range: &str| {
        range.split_whitespace().any(|part| {
            rows_of(part).is_some_and(|(a, b)| {
                let entered = goes_inside((a, b));
                (meets(Some((a, b))) || entered) && !(a <= first && last <= b && entered)
            })
        })
    };
    let within = |range: &str| {
        range
            .split_whitespace()
            .all(|part| rows_of(part).is_some_and(|(a, b)| first <= a && b <= last))
    };
    let mut carried = vec![];
    for link in fields["links"].as_array().into_iter().flatten() {
        let range = string(&link["range"])?;
        if !crosses(range) {
            continue;
        }
        if !within(range) {
            return refuse(format!(
                "the link on {range} reaches out of the rows moved; move them in Excel"
            ));
        }
        // The rows inserted have the cells' formats already.
        let mut settings = json!({"keep_format":true});
        for key in ["location", "url", "tooltip"] {
            if link.get(key).is_some() {
                settings[key] = link[key].clone();
            }
        }
        carried.push(Carried {
            kind: "set_link",
            range: range.to_owned(),
            settings,
        });
    }
    for note in fields["comments"].as_array().into_iter().flatten() {
        let address = string(&note["address"])?;
        if !crosses(address) {
            continue;
        }
        if note["kind"] != "note" {
            return refuse(format!(
                "{address} holds a threaded comment, which ARP does not write; move the rows in Excel"
            ));
        }
        let mut settings = json!({"text":note["text"]});
        if let Some(author) = note["author"].as_str() {
            settings["author"] = json!(author);
        }
        carried.push(Carried {
            kind: "set_note",
            range: address.to_owned(),
            settings,
        });
    }
    for validation in fields["validations"].as_array().into_iter().flatten() {
        let range = string(&validation["range"])?;
        if !crosses(range) {
            continue;
        }
        // Formulas reading cells would read others where it is set again.
        let reads_cells = ["formula1", "formula2"].iter().any(|key| {
            validation[*key].as_str().is_some_and(|formula| {
                excel::referenced(formula).map_or(true, |references| !references.is_empty())
            })
        });
        if !within(range) || reads_cells {
            return refuse(format!(
                "the data validation on {range} {}; move them in Excel",
                if reads_cells {
                    "reads cells and covers rows moved"
                } else {
                    "holds the rows moved or the place they go to, but not both"
                }
            ));
        }
        let mut rule = validation.clone();
        if let Some(rule) = rule.as_object_mut() {
            rule.remove("range");
        }
        carried.push(Carried {
            kind: "set_validation",
            range: range.to_owned(),
            settings: json!({"rule":rule}),
        });
    }
    for format in fields["conditional_formats"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let range = string(&format["range"])?;
        if crosses(range) {
            return refuse(format!(
                "the conditional format on {range} holds the rows moved or the place they go to, but not both; move them in Excel"
            ));
        }
    }
    // Drawings that move with their cells and lie on the rows moved go with
    // them, as Excel moves them; those that keep their place stay.
    let mut placed = vec![];
    for drawing in fields["drawings"].as_array().into_iter().flatten() {
        let anchor = &drawing["anchor"];
        if !drawing["group"].is_null()
            || anchor["kind"] == "absoluteAnchor"
            || anchor["edit_as"] == "absolute"
        {
            continue;
        }
        let corner = |key: &str| -> Option<(u32, u32, u64, u64)> {
            let marker = &anchor[key];
            Some((
                u32::try_from(marker["col"].as_u64()? + 1).ok()?,
                u32::try_from(marker["row"].as_u64()? + 1).ok()?,
                marker["colOff"].as_u64().unwrap_or(0),
                marker["rowOff"].as_u64().unwrap_or(0),
            ))
        };
        let Some(from) = corner("from") else {
            continue;
        };
        let to = corner("to");
        // The last row it covers: a corner at the top of a row ends above it.
        let bottom = match to {
            Some((_, row, _, 0)) if row > from.1 => row - 1,
            Some((_, row, _, _)) => row,
            None => from.1,
        };
        let rows = (from.1, bottom);
        if !meets(Some(rows)) || holds(rows) {
            continue;
        }
        let name = drawing["name"].as_str().unwrap_or("unnamed");
        match to {
            Some(to) if first <= rows.0 && rows.1 <= last && anchor["kind"] == "twoCellAnchor" => {
                placed.push(Placed {
                    drawing: string(&drawing["id"])?.to_owned(),
                    corners: [from, to],
                });
            }
            _ => {
                return refuse(format!(
                    "a drawing ({name}) lies on rows moved and others; move them in Excel"
                ));
            }
        }
    }
    Ok((carried, placed))
}

impl Store {
    /// Moves rows of a sheet of an adopted workbook, or of the proposal of
    /// `id`, recording the insertion, copy and deletion that make the move.
    pub fn move_rows(&self, id: &str, proposal: bool, edit: &MoveRequest) -> Result<Value> {
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
                "rows are moved in Excel workbooks only; insert and delete the paragraphs or table rows of Word and PowerPoint",
            ));
        }
        if edit.count == 0 {
            return Err(rejection("invalid_count", "--count must be at least 1"));
        }
        if !extraction
            .sheets
            .iter()
            .any(|sheet| sheet["name"] == edit.sheet.as_str())
        {
            let names: Vec<_> = extraction
                .sheets
                .iter()
                .map(|s| s["name"].clone())
                .collect();
            return Err(Rejection(json!({"code":"sheet_not_found","message":format!("no worksheet {} in this workbook",edit.sheet),"sheets":names})).into());
        }
        // Where the rows are now.
        let (first, last) = (edit.from, edit.from + edit.count - 1);
        let operations = array(&inspected.mappings["operations"]).map_or(&[][..], Vec::as_slice);
        let structural = excel::parse_operations(operations, &extraction.sheets)?;
        let now_row = |row: u32| -> Result<Option<u32>> {
            excel::map_coordinate(&edit.sheet, &format!("A{row}"), &structural)?
                .map(|address| excel::coordinate(&address).map(|(_, row)| row))
                .transpose()
        };
        let (Some(top), Some(bottom)) = (now_row(first)?, now_row(last)?) else {
            return refuse(
                "a recorded operation deletes rows moved; undo it or move other rows".to_owned(),
            );
        };
        if bottom - top != last - first {
            return refuse(
                "a recorded operation inserts or deletes rows among the rows moved; move them in parts".to_owned(),
            );
        }
        // What is recorded on the rows: links, notes, rules and placements
        // show in the workbook the move reads, and go with the rows from
        // there; formats are recorded again where the rows go. Copies and
        // pictures are recorded again after the move.
        let mut formats = vec![];
        for (recorded, cells) in
            excel::recorded_on_rows(operations, &extraction.sheets, &edit.sheet, (top, bottom))?
        {
            let value = operations
                .iter()
                .find(|o| o["id"] == recorded.as_str())
                .context("recorded operation missing")?;
            match value["kind"].as_str().unwrap_or_default() {
                "format_cells" if value.get("from").is_none() && value.get("merge").is_none() => {
                    formats.push((value.clone(), cells));
                }
                "set_link"
                | "clear_links"
                | "set_note"
                | "clear_notes"
                | "set_validation"
                | "clear_validations"
                | "add_conditional_format"
                | "clear_conditional_formats"
                | "place_drawing" => {}
                _ => {
                    return refuse(format!(
                        "the operation {recorded} copies cells, takes a format from a cell, merges cells or adds a picture on rows moved, which would go with the rows; undo it, move the rows, and record it again"
                    ));
                }
            }
        }
        // The workbook every pending change writes, which the move meets.
        let now = self
            .written_workbook(&dir, &inspected)
            .map_err(|error| rejection("writeback_refused", format!("{error:#}")))?;
        let before = snapshot(&dir)?;
        let id_base = edit
            .id
            .clone()
            .unwrap_or_else(|| next_operation_id(operations, "move-rows"));
        let result = self
            .record_move(id, proposal, edit, &dir, &id_base, &now, (top, bottom))
            .and_then(|mut report| {
                let again = self.format_moved(
                    id,
                    proposal,
                    edit,
                    &id_base,
                    &formats,
                    (top, bottom),
                    &report,
                )?;
                if !again.is_empty() {
                    report["operations"]
                        .as_array_mut()
                        .context("move operations")?
                        .extend(again.into_iter().map(Value::from));
                }
                Ok(report)
            });
        match result {
            Ok(mut report) if edit.dry_run => {
                restore(&dir, &before)?;
                report["state"] = json!("planned");
                Ok(report)
            }
            Ok(report) => Ok(report),
            Err(error) => {
                restore(&dir, &before)?;
                Err(error)
            }
        }
    }

    /// The workbook export writes from the document folder `dir` now.
    fn written_workbook(&self, dir: &Path, inspected: &Inspection) -> Result<excel::Workbook> {
        let source = under(&self.root, string(&inspected.meta["source"]["path"])?)?;
        let book = Source::open(&source)?;
        let writeback = self.writeback(dir, inspected, &book)?;
        let stage = Stage::new_in(&under(&self.arp, "cache/export")?, &self.arp)?;
        let preview = stage
            .path()
            .join(source.file_name().context("missing filename")?);
        book.patch(&preview, &writeback.edits(inspected)?)?;
        match Source::open(&preview)? {
            Source::Excel(written) => Ok(written),
            Source::Text(_) => bail!("the written workbook is no workbook"),
        }
    }

    /// Records again where the rows `rows` went the formats `formats`
    /// recorded on them, each with its cells as the operations before the
    /// move leave them: the part of them on the rows. Returns their IDs.
    #[allow(clippy::too_many_arguments)]
    fn format_moved(
        &self,
        id: &str,
        proposal: bool,
        edit: &MoveRequest,
        id_base: &str,
        formats: &[(Value, String)],
        rows: (u32, u32),
        report: &Value,
    ) -> Result<Vec<String>> {
        let at = report["at"].as_u64().and_then(|at| u32::try_from(at).ok());
        let Some(at) = at else {
            return Ok(vec![]);
        };
        let mut recorded = vec![];
        for (value, cells) in formats {
            let source = string(&value["id"])?;
            for (index, part) in cells.split_whitespace().enumerate() {
                let (start, end) = part.split_once(':').unwrap_or((part, part));
                let (Ok((c0, r0)), Ok((c1, r1))) =
                    (excel::coordinate(start), excel::coordinate(end))
                else {
                    continue;
                };
                let (a, b) = (r0.max(rows.0), r1.min(rows.1));
                if a > b {
                    continue;
                }
                let range = format!(
                    "{}{}:{}{}",
                    excel::column_name(c0)?,
                    excel::moved_row(a, rows, at),
                    excel::column_name(c1)?,
                    excel::moved_row(b, rows, at)
                );
                let again = format!("{id_base}-{source}-{}", index + 1);
                let header = super::format_edit::SheetOperation {
                    sheet: &edit.sheet,
                    kind: "format_cells",
                    prefix: "format",
                    id: Some(&again),
                    reason: &edit.reason,
                    base: None,
                    dry_run: false,
                };
                self.record_sheet_operation(
                    id,
                    proposal,
                    &header,
                    |_, operation| {
                        for (key, field) in value.as_object().into_iter().flatten() {
                            if !matches!(key.as_str(), "id" | "kind" | "sheet" | "reason") {
                                operation[key] = field.clone();
                            }
                        }
                        operation["range"] = json!(range);
                        Ok(())
                    },
                    |values, inspected| {
                        excel::parse_format_operations(values, &inspected.extraction.sheets)
                            .map(|_| ())
                    },
                    super::format_edit::unchanged_content,
                )?;
                recorded.push(again);
            }
        }
        Ok(recorded)
    }

    /// Gives the formulas of the workbook `now` reading the rows moved
    /// (`rows`, before `at`) of the sheet `edit` moves them on, and those in them, the text Excel
    /// gives them where the operations recorded for the move give another,
    /// written as the cells stand after the operation `delete` ending it.
    /// Returns how many.
    #[allow(clippy::too_many_arguments)]
    fn follow_moved_rows(
        &self,
        id: &str,
        proposal: bool,
        edit: &MoveRequest,
        dir: &Path,
        now: &excel::Workbook,
        (rows, at): ((u32, u32), u32),
        delete: &str,
    ) -> Result<usize> {
        let sheet = edit.sheet.as_str();
        let inspected = self.inspect(dir, false)?;
        let extraction = &inspected.extraction;
        let operations = excel::parse_operations(
            array(&inspected.mappings["operations"])?,
            &extraction.sheets,
        )?;
        let layout = identity::load(dir, extraction, &Planned::new())?;
        let places = identity::formula_places(&layout)?;
        // The content's formulas by page and key, and whether each is one of
        // the original.
        let mut formulas: HashMap<(String, String), (Value, bool)> = HashMap::new();
        for (index, entry) in inspected.entries().enumerate() {
            if entry.block()? != "formulas" {
                continue;
            }
            if let (Some(position), Some(field)) = (entry.position()?, inspected.content.get(index))
            {
                formulas.insert(
                    (entry.page()?.to_owned(), position.row.to_owned()),
                    (field.value.clone(), entry.writeback()? == "formula"),
                );
            }
        }
        let reads_rows = |formula: &str| -> Result<bool> {
            Ok(excel::referenced(formula)?.iter().any(|reference| {
                reference
                    .rows
                    .is_some_and(|(a, b)| a <= rows.1 && rows.0 <= b)
            }))
        };
        let mut edits = vec![];
        for (source, cells) in now.sheets.iter().zip(&now.cells) {
            let name = string(&source["name"])?;
            let Some((index, extracted)) = extraction
                .sheets
                .iter()
                .enumerate()
                .find(|(_, s)| s["name"] == name)
            else {
                continue;
            };
            let page = identity::page_id(&extracted.fields, index);
            let own: Vec<excel::StructuralOperation> = operations
                .iter()
                .filter(|operation| operation.sheet == name)
                .cloned()
                .collect();
            for cell in cells {
                let Some(formula) = cell.formula.as_deref() else {
                    continue;
                };
                let (column, row) = excel::coordinate(&cell.address)?;
                let inside = name == sheet && (rows.0..=rows.1).contains(&row);
                if !inside && !reads_rows(formula)? {
                    continue;
                }
                let wanted = excel::moved_rows_formula(formula, name, sheet, rows, at)?;
                // The cell where it is after the move, and what it holds.
                let row = if name == sheet {
                    excel::moved_row(row, rows, at)
                } else {
                    row
                };
                let (row_key, column_key, original) =
                    super::copy_edit::keys_at(name, (column, row), &own)?;
                let key = identity::formula_name(&page, &row_key, &column_key);
                let Some((present, of_original)) = formulas.get(&(page.clone(), key.clone()))
                else {
                    continue;
                };
                let text = present.as_str().unwrap_or_default().trim_start_matches('=');
                // What the operations make of it: from the operation it is
                // written after, else from the original's places.
                let frame = places
                    .get(&page)
                    .and_then(|places| places.get(&key))
                    .and_then(|(_, _, frame)| frame.clone());
                let from = frame
                    .as_deref()
                    .and_then(|frame| operations.iter().position(|o| o.id == frame))
                    .map_or(0, |at| at + 1);
                if excel::rewrite_formula(text, name, &operations[from..])? == wanted {
                    continue;
                }
                let mut change = json!({"sheet":name,"before":present,"after":format!("={wanted}"),
                    "written_after":delete});
                match &original {
                    Some(address) if *of_original => change["cell"] = json!(address),
                    Some(address) => {
                        change["cell"] = json!(address);
                        change["formula"] = json!(true);
                    }
                    None => {
                        let (row, column) =
                            identity::identities_at(&layout, &page, &row_key, &column_key)?
                                .context("moved formula without its cell")?;
                        change["element"] = json!(elements::cell_id(&row, &column));
                        change["formula"] = json!(true);
                    }
                }
                edits.push(change);
            }
        }
        if edits.is_empty() {
            return Ok(0);
        }
        let count = edits.len();
        let request = json!({"schema_version":"1","base":inspected.fingerprint,
            "source_sha256":inspected.meta["source"]["sha256"],"actor":"documents rows move",
            "reason":edit.reason,"edits":edits});
        self.edit_values_with(id, proposal, &request, false, &[])?;
        Ok(count)
    }

    /// Records the operations of a move of the rows `top` to `bottom` of the
    /// workbook `now`; the folder is brought back by the caller when one is
    /// refused.
    #[allow(clippy::too_many_arguments)]
    fn record_move(
        &self,
        id: &str,
        proposal: bool,
        edit: &MoveRequest,
        dir: &Path,
        id_base: &str,
        now: &excel::Workbook,
        (top, bottom): (u32, u32),
    ) -> Result<Value> {
        let (first, last) = (edit.from, edit.from + edit.count - 1);
        let insert = format!("{id_base}-insert");
        let copy = format!("{id_base}-copy");
        let delete = format!("{id_base}-delete");
        let sheet_edit = |kind: EditKind, op: &str| SheetEdit {
            axis: Axis::Rows,
            kind,
            sheet: edit.sheet.clone(),
            id: Some(op.to_owned()),
            reason: edit.reason.clone(),
            base: None,
            dry_run: false,
            vba_unchanged: edit.vba_unchanged,
        };
        let inserted = self.edit_sheet(
            id,
            proposal,
            &sheet_edit(
                EditKind::Insert {
                    position: edit.position.clone(),
                    count: Some(edit.count),
                    style_from: Some(if first == last {
                        first.to_string()
                    } else {
                        format!("{first}:{last}")
                    }),
                    merges: true,
                    values: None,
                },
                &insert,
            ),
        )?;
        // Where the rows moved and the rows inserted are now.
        let mappings = read_mappings(&dir.join("mappings.yml"))?;
        let inspected = self.inspect(dir, false)?;
        let operations = excel::parse_operations(
            array(&mappings["operations"])?,
            &inspected.extraction.sheets,
        )?;
        let place = |address: String| -> Result<(u32, u32)> { excel::coordinate(&address) };
        let source_row = place(
            excel::map_coordinate(&edit.sheet, &format!("A{first}"), &operations)?
                .context("rows moved are deleted by a recorded operation")?,
        )?
        .1;
        let target_row = place(excel::resolve_insertion(
            &edit.sheet,
            &insert,
            0,
            Some("A"),
            None,
            &operations,
        )?)?
        .1;
        // Before the insertion, the rows go before the row it took.
        let (carried, placed) = carried_with(now, &edit.sheet, (top, bottom), target_row)?;
        // The cells of the rows across the columns the sheet uses.
        let index = now
            .sheets
            .iter()
            .position(|s| s["name"] == edit.sheet.as_str())
            .context("moved sheet missing")?;
        let mut last_column = now.cells[index]
            .iter()
            .filter_map(|cell| excel::coordinate(&cell.address).ok().map(|(c, _)| c))
            .max()
            .unwrap_or(1);
        for range in now.sheets[index]["merges"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            if let Ok((column, _)) = excel::coordinate(range.rsplit(':').next().unwrap_or(range)) {
                last_column = last_column.max(column);
            }
        }
        let last_column = excel::column_name(last_column.min(16_384))?;
        let copied = self.copy_cells(
            id,
            proposal,
            &CopyRequest {
                sheet: edit.sheet.clone(),
                range: format!("A{source_row}:{last_column}{}", source_row + bottom - top),
                to: format!("A{target_row}"),
                // The rows inserted have the formats and merges already, and
                // the cells copied from are deleted after.
                formats: false,
                values: true,
                id: Some(copy.clone()),
                reason: edit.reason.clone(),
                base: None,
                dry_run: false,
            },
        )?;
        let mut recorded = vec![insert.clone(), copy];
        for (index, object) in carried.iter().enumerate() {
            let object_id = format!("{id_base}-{}-{}", object.kind.replace('_', "-"), index + 1);
            let range = object
                .range
                .split_whitespace()
                .map(|part| moved_range(part, top, target_row))
                .collect::<Result<Vec<_>>>()?
                .join(" ");
            self.edit_cell_objects(
                id,
                proposal,
                &ObjectRequest {
                    sheet: edit.sheet.clone(),
                    range,
                    kind: object.kind.to_owned(),
                    settings: object.settings.clone(),
                    id: Some(object_id.clone()),
                    reason: edit.reason.clone(),
                    base: None,
                    dry_run: false,
                },
            )?;
            recorded.push(object_id);
        }
        // The drawings on the rows go where they go, before the rows are
        // deleted: a deletion then takes them where they are.
        for (index, drawing) in placed.iter().enumerate() {
            let place_id = format!("{id_base}-place-{}", index + 1);
            let cell = |(column, row, _, _): (u32, u32, u64, u64)| -> Result<String> {
                Ok(format!(
                    "{}{}",
                    excel::column_name(column)?,
                    target_row + row - top
                ))
            };
            let [a, b] = drawing.corners;
            self.place_drawing(
                id,
                proposal,
                &super::DrawingRequest {
                    sheet: edit.sheet.clone(),
                    drawing: drawing.drawing.clone(),
                    from: Some(cell(a)?),
                    to: Some(cell(b)?),
                    offsets: Some([(a.2, a.3), (b.2, b.3)]),
                    placement: None,
                    id: Some(place_id.clone()),
                    reason: edit.reason.clone(),
                    base: None,
                    dry_run: false,
                },
            )?;
            recorded.push(place_id);
        }
        let deleted = self.edit_sheet(
            id,
            proposal,
            &sheet_edit(
                EditKind::Delete {
                    from: first.to_string(),
                    count: edit.count,
                },
                &delete,
            ),
        )?;
        // The formulas reading the rows moved read them where Excel would.
        let followed = self.follow_moved_rows(
            id,
            proposal,
            edit,
            dir,
            now,
            ((top, bottom), target_row),
            &delete,
        )?;
        recorded.push(delete);
        Ok(json!({
            "state":"written",
            "operations":recorded,
            "at":target_row,
            "formulas":followed,
            "span":inserted["span"],
            "keys":inserted["keys"],
            "values":copied["values"],
            "deleted":deleted["span"],
            "content":deleted["content"],
            "check":deleted["check"],
        }))
    }
}
