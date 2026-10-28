//! Cells of an Excel worksheet inserted or deleted, the cells beside them
//! shifted down, right, up or left within the rows or columns of the range,
//! as Excel's Insert and Delete do. The `insert_cells` or `delete_cells`
//! operation in mappings.yml and the content values it moves or removes are
//! written together after a trial write and the same validation as check: a
//! value is shown where its cell ends, and an inserted cell is empty, ready
//! for a value.
use super::sheet_edit::rejection;
use super::*;
use crate::excel::StructuralOperation;

pub struct CellRequest {
    pub sheet: String,
    /// The cells, as the operations recorded before leave them.
    pub range: String,
    /// Insert cells, or delete them.
    pub insertion: bool,
    /// down or right for an insertion, up or left for a deletion.
    pub shift: String,
    pub id: Option<String>,
    pub reason: String,
    /// The content fingerprint the edit was planned against.
    pub base: Option<String>,
    pub dry_run: bool,
    /// The person editing accepted that the workbook's VBA code keeps the
    /// cell addresses it holds.
    pub vba_unchanged: bool,
}

impl Store {
    /// Inserts or deletes cells of an adopted workbook, or of the proposal of
    /// `id`, shifting the cells beside them, once a trial write of every
    /// pending change with it succeeds.
    pub fn shift_cells(&self, id: &str, proposal: bool, edit: &CellRequest) -> Result<Value> {
        let kind = if edit.insertion {
            "insert_cells"
        } else {
            "delete_cells"
        };
        let header = format_edit::SheetOperation {
            sheet: &edit.sheet,
            kind,
            prefix: if edit.insertion {
                "cells-in"
            } else {
                "cells-out"
            },
            id: edit.id.as_deref(),
            reason: &edit.reason,
            base: edit.base.as_deref(),
            dry_run: edit.dry_run,
        };
        let settings = |_: &Inspection, operation: &mut Value| -> Result<()> {
            operation["range"] = json!(edit.range);
            operation["shift"] = json!(edit.shift);
            if edit.vba_unchanged {
                operation["vba_unchanged"] = json!(true);
            }
            Ok(())
        };
        let parse = |values: &[Value], inspected: &Inspection| {
            excel::parse_operations(values, &inspected.extraction.sheets).map(|_| ())
        };
        let content =
            |dir: &Path, inspected: &Inspection, values: &[Value], planned: &mut Planned| {
                // A shift Excel would refuse, or take a merge apart for, is
                // found as the cells are placed, before the trial write.
                self.shifted_content(dir, inspected, values, &edit.sheet, planned)
                    .map_err(|error| {
                        if error.downcast_ref::<Rejection>().is_some() {
                            error
                        } else {
                            rejection("writeback_refused", format!("{error:#}"))
                        }
                    })
            };
        self.record_sheet_operation(id, proposal, &header, settings, parse, content)
    }

    /// Writes into `planned` the content page of `sheet` and the layout as
    /// the last of the operations `values`, which shifts cells, leaves them:
    /// each value of a cell it moves at the row and column the cell ends in,
    /// and those of the cells it deletes removed, which the report lists.
    fn shifted_content(
        &self,
        dir: &Path,
        inspected: &Inspection,
        values: &[Value],
        sheet: &str,
        planned: &mut Planned,
    ) -> Result<Value> {
        let extraction = &inspected.extraction;
        let sheets = &extraction.sheets;
        let operations = excel::parse_operations(values, sheets)?;
        let prior = excel::parse_operations(&values[..values.len() - 1], sheets)?;
        let operation = operations.last().context("the operation recorded")?;
        let source = under(&self.root, string(&inspected.meta["source"]["path"])?)?;
        Source::open(&source)?
            .ensure_row_edits_supported(&operations, values)
            .map_err(|error| {
                if error.downcast_ref::<excel::VbaNotUpdated>().is_some() {
                    let vba = Source::open(&source)
                        .ok()
                        .and_then(|book| book.vba_report());
                    Rejection(
                        json!({"code":"vba_not_updated","message":format!("{error:#}"),"vba":vba}),
                    )
                    .into()
                } else {
                    rejection("structural_edit_unsupported", format!("{error:#}"))
                }
            })?;
        let index = sheets
            .iter()
            .position(|s| s["name"] == sheet)
            .context("no such worksheet")?;
        let page_id = identity::page_id(&sheets[index].fields, index);
        let page_name = inspected
            .page_files
            .get(&page_id)
            .cloned()
            .context("the sheet has no content page")?;
        let mut layout = identity::load(dir, extraction, &Planned::new())?;
        let mut page = self.read_content_grid_with(dir, &page_name, extraction, &layout)?;
        let before = layout.sheet(&page_id)?.clone();
        identity::arrange(&mut layout, extraction, &operations)?;
        let (moved, deleted) = shift_values(
            &mut page,
            sheet,
            &page_id,
            (&before, layout.sheet_mut(&page_id)?),
            (&prior, &operations),
            operation,
        )?;
        let after = layout.sheet(&page_id)?;
        let mut groups = elements::metadata_groups(&after.metadata)?;
        carry_tables(&mut groups, (&before, after), &moved);
        let body = elements::encode_runtime(page, &mut layout, Some(groups))?;
        planned.insert(page_name, Some(elements::content_bytes(&body, &layout)?));
        planned.insert(
            "layout.yml".into(),
            Some(identity::stored(layout, extraction)?),
        );
        Ok(json!({"moved_values":moved.len(),"deleted":deleted}))
    }
}

/// The operation that takes the cells `operation` shifts back where they
/// were: a deletion of the cells it inserts, or an insertion of those it
/// deletes.
pub(super) fn inverse(operation: &StructuralOperation) -> StructuralOperation {
    use excel::OperationKind::*;
    let kind = match operation.kind {
        InsertRows => DeleteRows,
        DeleteRows => InsertRows,
        InsertColumns => DeleteColumns,
        DeleteColumns => InsertColumns,
        ref other => other.clone(),
    };
    StructuralOperation {
        kind,
        ..operation.clone()
    }
}

/// The row and column keys of a place on a content page.
type Place = (String, String);

/// The places a value moved from and to.
type Move = (Place, Place);

/// Moves the values of the content `page` of `sheet` (page `page_id`) as
/// `shift` moves their cells: read with the layout sheet `before` the
/// operations `prior` leave, and placed in the layout sheet `after` the
/// operations `following` leave, which gains the rows and columns the cells
/// reach that it has none for. Gives the places (row and column keys) each
/// value that moved left and took, and the values of the cells `shift`
/// deletes, which leave the page.
pub(super) fn shift_values(
    page: &mut elements::GridPage,
    sheet: &str,
    page_id: &str,
    (before, after): (&LayoutSheet, &mut LayoutSheet),
    (prior, following): (&[StructuralOperation], &[StructuralOperation]),
    shift: &StructuralOperation,
) -> Result<(Vec<Move>, Vec<Value>)> {
    let renames = identity::formula_renames(before, after, page_id)?;
    // The cached results of formulas, at the places of their cells, are not
    // the content's: a deleted one goes with its formula.
    let caches: std::collections::HashSet<(&str, &str)> = before["formulas"]
        .as_object()
        .context("layout formulas missing")?
        .values()
        .filter_map(|target| {
            Some((
                before.rows.key(target["row"].as_str()?)?,
                before.columns.key(target["column"].as_str()?)?,
            ))
        })
        .collect();
    let own: Vec<StructuralOperation> = following
        .iter()
        .filter(|o| o.sheet == sheet)
        .cloned()
        .collect();
    let shifted = std::slice::from_ref(shift);
    let mut lines = identity::KeyLines::new(after);
    let mut deleted = vec![];
    let mut moved = vec![];
    let mut kept = Vec::with_capacity(page.values.len());
    for mut value in std::mem::take(&mut page.values) {
        let gone = |value: &elements::GridValue| json!({"block":&*value.block,"row":&*value.row,"column":&*value.column,"value":value.value});
        match &*value.block {
            "table-1" => {
                let (Some(row), Some(column)) = (
                    excel::key_position(sheet, &value.row, prior, true)?,
                    excel::key_position(sheet, &value.column, prior, false)?,
                ) else {
                    kept.push(value);
                    continue;
                };
                if !shift.moves_cell(column, row) {
                    kept.push(value);
                    continue;
                }
                let address = format!("{}{row}", excel::column_name(column)?);
                let Some(mapped) = excel::map_coordinate(sheet, &address, shifted)? else {
                    if !caches.contains(&(&*value.row, &*value.column)) {
                        deleted.push(gone(&value));
                    }
                    continue;
                };
                let (column, row) = excel::coordinate(&mapped)?;
                let row_key = excel::axis_key(row, &own, true)?;
                let column_key = excel::axis_key(column, &own, false)?;
                lines.line(after, "rows", &row_key, row);
                lines.line(after, "columns", &column_key, column);
                if *value.row != *row_key || *value.column != *column_key {
                    moved.push((
                        (value.row.to_string(), value.column.to_string()),
                        (row_key.clone(), column_key.clone()),
                    ));
                }
                value.row = row_key.into();
                value.column = column_key.into();
                kept.push(value);
            }
            "formulas" => match renames.get(&*value.row) {
                Some(key) => {
                    value.row = key.as_str().into();
                    kept.push(value);
                }
                None => deleted.push(gone(&value)),
            },
            _ => kept.push(value),
        }
    }
    page.values = kept;
    page.order();
    Ok((moved, deleted))
}

/// Carries the table of each cell `moves` moves, from its place (row and
/// column keys) in the layout sheet `before` to its place in `after`, into
/// `groups`, those of `after`: a cell moved to a place no table holds stays in
/// the table it was in, as the table Excel finds there takes it in, and one
/// moved into a table's place is in that table.
pub(super) fn carry_tables(
    groups: &mut elements::Groups,
    (before, after): (&LayoutSheet, &LayoutSheet),
    moves: &[(Place, Place)],
) {
    let identities =
        |sheet: &LayoutSheet| ["rows", "columns"].map(|axis| identity::key_identities(sheet, axis));
    let (from, to) = (identities(before), identities(after));
    let place = |identities: &[HashMap<String, String>; 2], (row, column): &Place| {
        Some((
            identities[0].get(row)?.clone(),
            identities[1].get(column)?.clone(),
        ))
    };
    let tables: Vec<_> = moves
        .iter()
        .map(|(old, _)| {
            let (row, column) = place(&from, old)?;
            groups.get_at(&row, &column)?.table.clone()
        })
        .collect();
    for ((_, new), table) in moves.iter().zip(tables) {
        let (Some(table), Some((row, column))) = (table, place(&to, new)) else {
            continue;
        };
        if groups.get_at(&row, &column).is_none() {
            groups.insert_at(
                &row,
                &column,
                elements::Group {
                    table: Some(table),
                    ..elements::Group::default()
                },
            );
        }
    }
}

/// Gives the cells the cell insertion `insertion` adds to `page` of `sheet`
/// (page `page_id`) the values and fonts of the cells of the `source` sheet
/// they come from under the operations `following`, as import gives them:
/// those of the cells a deletion taken back restores. Placed in the layout
/// sheet `after` those operations leave; gives the places (block, row and
/// column keys) of the values given.
pub(super) fn restore_cells(
    page: &mut elements::GridPage,
    sheet: &str,
    page_id: &str,
    source: &crate::extraction::Sheet,
    after: &mut LayoutSheet,
    following: &[StructuralOperation],
    insertion: &StructuralOperation,
) -> Result<BTreeSet<(String, String, String)>> {
    let (first, last) = insertion.band.context("cells shifted within a band")?;
    let own: Vec<StructuralOperation> = following
        .iter()
        .filter(|o| o.sheet == sheet)
        .cloned()
        .collect();
    let cells: HashMap<&str, Cell<'_>> = source.cells.iter().map(|c| (c.address(), c)).collect();
    let mut font_table = std::mem::take(&mut page.font_table);
    let mut font_of = |font: Value| -> Result<Option<u32>> {
        if font.is_null() {
            return Ok(None);
        }
        if !font_table.is_array() {
            font_table = json!([]);
        }
        let table = font_table.as_array_mut().context("font table")?;
        let index = match table.iter().position(|kept| *kept == font) {
            Some(index) => index,
            None => {
                table.push(font);
                table.len() - 1
            }
        };
        Ok(Some(u32::try_from(index)?))
    };
    let mut places = BTreeSet::new();
    let mut lines = identity::KeyLines::new(after);
    let mut restored = vec![];
    for along in insertion.at..insertion.at + insertion.count {
        for across in first..=last {
            let (column, row) = if insertion.row_operation() {
                (across, along)
            } else {
                (along, across)
            };
            let Some(excel::Origin::Original(address)) =
                excel::origin_of(sheet, (column, row), following)?
            else {
                continue;
            };
            let Some(cell) = cells.get(address.as_str()) else {
                continue;
            };
            let row_key = excel::axis_key(row, &own, true)?;
            let column_key = excel::axis_key(column, &own, false)?;
            lines.line(after, "rows", &row_key, row);
            lines.line(after, "columns", &column_key, column);
            let font = font_of(crate::extraction::font(source, *cell)?.clone())?;
            let formula = cell.kind() == CellKind::Formula;
            restored.push(elements::GridValue {
                block: "table-1".into(),
                row: row_key.as_str().into(),
                column: column_key.as_str().into(),
                value: cell.value().to_json(),
                font: if formula { None } else { font },
            });
            places.insert(("table-1".to_owned(), row_key.clone(), column_key.clone()));
            if formula {
                let key = identity::formula_key(after, page_id, cell.id())?
                    .context("a restored formula has a place")?;
                restored.push(elements::GridValue {
                    block: "formulas".into(),
                    row: key.as_str().into(),
                    column: "formula".into(),
                    value: json!(format!(
                        "={}",
                        cell.formula().context("formula cell without formula")?
                    )),
                    font,
                });
                places.insert(("formulas".to_owned(), key, "formula".to_owned()));
            }
        }
    }
    page.font_table = font_table;
    for (block, title) in [("table-1", "本文"), ("formulas", "数式原文")] {
        if restored.iter().any(|value| &*value.block == block) && !page.titles.contains_key(block) {
            page.titles.insert(block.to_owned(), json!(title));
        }
    }
    page.values.extend(restored);
    page.order();
    Ok(places)
}
