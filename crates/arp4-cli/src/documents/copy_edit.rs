//! A range of Excel cells copied to another place of its sheet, as Excel
//! pastes it: the values and formulas, those relative references move by the
//! distance pasted, as value edits, and the formats and merges as a
//! `copy_cells` operation written on export, recorded together.
use super::format_edit::{SheetOperation, unchanged_content};
use super::sheet_edit::rejection;
use super::*;

pub struct CopyRequest {
    pub sheet: String,
    /// The cells copied, as the operations recorded before leave them.
    pub range: String,
    /// The top left cell of the place pasted to, as the operations recorded
    /// before leave it.
    pub to: String,
    /// Copy the formats and merges.
    pub formats: bool,
    /// Copy the values and formulas.
    pub values: bool,
    pub id: Option<String>,
    pub reason: String,
    pub base: Option<String>,
    pub dry_run: bool,
}

/// The keys a page names the cell at final (`column`, `row`) of `sheet` by,
/// with its original address when it is a cell of the original.
pub(super) fn keys_at(
    sheet: &str,
    (column, row): (u32, u32),
    operations: &[excel::StructuralOperation],
) -> Result<(String, String, Option<String>)> {
    Ok(match excel::origin_of(sheet, (column, row), operations)? {
        Some(excel::Origin::Original(address)) => {
            let (original_column, original_row) = excel::coordinate(&address)?;
            (
                format!("r{original_row}"),
                excel::column_name(original_column)?,
                Some(address),
            )
        }
        Some(excel::Origin::Inserted {
            operation,
            offset,
            across,
        }) => {
            let rows = operations
                .iter()
                .find(|o| o.id == operation)
                .context("inserting operation missing")?
                .row_operation();
            let key = format!("{operation}-{}", offset + 1);
            if rows {
                (key, excel::column_name(across)?, None)
            } else {
                (format!("r{across}"), key, None)
            }
        }
        None => (
            excel::axis_key(row, operations, true)?,
            excel::axis_key(column, operations, false)?,
            None,
        ),
    })
}

impl Store {
    /// Copies a range of cells of an adopted workbook, or of the proposal of
    /// `id`, to another place of its sheet: its values and formulas as value
    /// edits, its formats and merges as a `copy_cells` operation, checked
    /// and written together.
    pub fn copy_cells(&self, id: &str, proposal: bool, edit: &CopyRequest) -> Result<Value> {
        if !edit.formats && !edit.values {
            return Err(rejection(
                "invalid_arguments",
                "copy the values, the formats or both",
            ));
        }
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
                "cells are copied in Excel workbooks only",
            ));
        }
        let Some(index) = extraction
            .sheets
            .iter()
            .position(|sheet| sheet["name"] == edit.sheet.as_str())
        else {
            let names: Vec<_> = extraction
                .sheets
                .iter()
                .map(|s| s["name"].clone())
                .collect();
            return Err(Rejection(json!({"code":"sheet_not_found","message":format!("no worksheet {} in this workbook",edit.sheet),"sheets":names})).into());
        };
        let page = identity::page_id(&extraction.sheets[index].fields, index);
        let invalid = |message: String| rejection("invalid_range", message);
        let bounds = |text: &str| -> Result<((u32, u32), (u32, u32))> {
            let (start, end) = text.split_once(':').unwrap_or((text, text));
            let (a, b) = (excel::coordinate(start), excel::coordinate(end));
            let (Ok((c0, r0)), Ok((c1, r1))) = (a, b) else {
                return Err(invalid(format!(
                    "{text} is no range of cells, such as B5:F8"
                )));
            };
            Ok(((c0.min(c1), c0.max(c1)), (r0.min(r1), r0.max(r1))))
        };
        let (columns, rows) = bounds(&edit.range)?;
        let (to_column, to_row) = excel::coordinate(&edit.to)
            .map_err(|_| invalid(format!("--to {} is no cell, such as B20", edit.to)))?;
        let mappings = read_mappings(&dir.join("mappings.yml"))?;
        let recorded = array(&mappings["operations"])?.clone();
        let all_operations = excel::parse_operations(&recorded, &extraction.sheets)?;
        let operations: Vec<excel::StructuralOperation> = all_operations
            .iter()
            .filter(|operation| operation.sheet == edit.sheet)
            .cloned()
            .collect();
        // Formulas pasted are written as the cells stand after the last row,
        // column or cell operation.
        let frame = all_operations.last().map(|operation| operation.id.clone());
        if edit.reason.trim().is_empty() {
            return Err(rejection("invalid_reason", "--reason must not be empty"));
        }
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
            None => next_operation_id(&recorded, "copy"),
        };
        let operation = json!({"id":op_id,"kind":"copy_cells","sheet":edit.sheet,"reason":edit.reason,
            "range":edit.range,"to":edit.to});
        if let Some(existing) = recorded.iter().find(|o| o["id"] == op_id.as_str()) {
            if *existing == operation {
                return Ok(
                    json!({"state":"unchanged","operation":operation,"content":inspected.fingerprint}),
                );
            }
            return Err(Rejection(json!({"code":"operation_id_conflict","message":format!("operation {op_id} is already recorded with other settings; choose another --id, or omit --id to have one generated"),"recorded":existing})).into());
        }
        // The content's values and formulas of the sheet, by place.
        let mut values: HashMap<(String, String, String), Value> = HashMap::new();
        for (entry_index, entry) in inspected.entries().enumerate() {
            if entry.page()? != page {
                continue;
            }
            if let (Some(position), Some(field)) =
                (entry.position()?, inspected.content.get(entry_index))
            {
                values.insert(
                    (
                        entry.block()?.to_owned(),
                        position.row.to_owned(),
                        position.column.to_owned(),
                    ),
                    field.value.clone(),
                );
            }
        }
        let layout = identity::load(&dir, extraction, &Planned::new())?;
        let rows_by_key = identity::key_identities(layout.sheet(&page)?, "rows");
        let columns_by_key = identity::key_identities(layout.sheet(&page)?, "columns");
        let places = identity::formula_places(&layout)?;
        // A cell's formula, value and name, and for a formula given to a cell
        // the original leaves empty, the operation it is written after.
        type Held = (Value, Value, Value, Option<Option<String>>);
        let held = |(column, row): (u32, u32)| -> Result<Held> {
            let (row_key, column_key, original) = keys_at(&edit.sheet, (column, row), &operations)?;
            let key = identity::formula_name(&page, &row_key, &column_key);
            let given = places
                .get(&page)
                .and_then(|places| places.get(&key))
                .map(|(_, _, written_after)| written_after.clone());
            let formula = values
                .get(&("formulas".to_owned(), key, "formula".to_owned()))
                .cloned()
                .unwrap_or(Value::Null);
            let value = values
                .get(&("table-1".to_owned(), row_key.clone(), column_key.clone()))
                .cloned()
                .unwrap_or(Value::Null);
            // A cell is named by its original address, else by element.
            let target = match original {
                Some(address) => json!({"cell":address}),
                None => match (rows_by_key.get(&row_key), columns_by_key.get(&column_key)) {
                    (Some(row), Some(column)) => {
                        json!({"element":elements::cell_id(row, column)})
                    }
                    _ => Value::Null,
                },
            };
            Ok((formula, value, target, given))
        };
        // Where the operations a formula moves with start.
        let moving_from = |written_after: &Option<String>| -> usize {
            written_after
                .as_deref()
                .and_then(|frame| all_operations.iter().position(|o| o.id == frame))
                .map_or(0, |at| at + 1)
        };
        let mut edits = vec![];
        if edit.values {
            let (down, right) = (
                i64::from(to_row) - i64::from(rows.0),
                i64::from(to_column) - i64::from(columns.0),
            );
            for r in 0..=rows.1 - rows.0 {
                for c in 0..=columns.1 - columns.0 {
                    let (source, target) =
                        ((columns.0 + c, rows.0 + r), (to_column + c, to_row + r));
                    let (formula, value, _, source_given) = held(source)?;
                    let (present_formula, present_value, place, given) = held(target)?;
                    let place_name = format!("{}{}", excel::column_name(target.0)?, target.1);
                    let mut edit_of = |before: &Value, after: Value, formula: bool| -> Result<()> {
                        if *before == after {
                            return Ok(());
                        }
                        if place.is_null() {
                            return Err(invalid(format!(
                                "{place_name} has no row or column the content names; copy within the sheet's cells"
                            )));
                        }
                        let mut edit = json!({"sheet":edit.sheet,"before":before,"after":after});
                        for (key, value) in place.as_object().into_iter().flatten() {
                            edit[key] = value.clone();
                        }
                        if formula {
                            edit["formula"] = json!(true);
                            if let Some(frame) = &frame {
                                edit["written_after"] = json!(frame);
                            }
                        }
                        edits.push(edit);
                        Ok(())
                    };
                    match (formula.as_str(), present_formula.is_null()) {
                        (Some(text), _) => {
                            // The formula as the cells stand now, moved by the
                            // distance pasted.
                            let now = excel::rewrite_formula(
                                text.trim_start_matches('='),
                                &edit.sheet,
                                &all_operations[moving_from(&source_given.flatten())..],
                            )?;
                            let moved = excel::shift_relative(&now, down, right)?;
                            let after = json!(format!("={moved}"));
                            if given.is_none() && !present_formula.is_null() && frame.is_some() {
                                return Err(rejection(
                                    "not_editable",
                                    format!(
                                        "{place_name} holds a formula of the original, written as the original's places give it, which a formula pasted after rows, columns or cells are inserted or deleted cannot replace; edit it in Excel"
                                    ),
                                ));
                            }
                            if present_formula.is_null() || given.is_some() {
                                if !present_value.is_null() {
                                    return Err(rejection(
                                        "not_editable",
                                        format!(
                                            "{place_name} holds a value, which a copied formula would replace; clear it with documents values first"
                                        ),
                                    ));
                                }
                                edit_of(&present_formula, after, true)?;
                            } else {
                                edit_of(&present_formula, after, false)?;
                            }
                        }
                        (None, false) => {
                            return Err(rejection(
                                "not_editable",
                                format!(
                                    "{place_name} holds a formula, which a copied value would replace; edit it in Excel"
                                ),
                            ));
                        }
                        (None, true) => edit_of(&present_value, value, false)?,
                    }
                }
            }
        }
        let added: Vec<Value> = if edit.formats {
            vec![operation.clone()]
        } else {
            vec![]
        };
        if edits.is_empty() {
            if !edit.formats {
                return Ok(
                    json!({"state":"unchanged","operation":operation,"content":inspected.fingerprint}),
                );
            }
            let header = SheetOperation {
                sheet: &edit.sheet,
                kind: "copy_cells",
                prefix: "copy",
                id: Some(&op_id),
                reason: &edit.reason,
                base: edit.base.as_deref(),
                dry_run: edit.dry_run,
            };
            let mut report = self.record_sheet_operation(
                id,
                proposal,
                &header,
                |_, operation| {
                    operation["range"] = json!(edit.range);
                    operation["to"] = json!(edit.to);
                    Ok(())
                },
                |values, inspected| {
                    excel::parse_format_operations(values, &inspected.extraction.sheets).map(|_| ())
                },
                unchanged_content,
            )?;
            report["values"] = json!(0);
            return Ok(report);
        }
        let request = json!({"schema_version":"1","base":inspected.fingerprint,
            "source_sha256":inspected.meta["source"]["sha256"],"actor":"documents cells copy",
            "reason":edit.reason,"edits":edits});
        let mut report = self.edit_values_with(id, proposal, &request, edit.dry_run, &added)?;
        if edit.formats {
            report["operation"] = operation;
        }
        report["values"] = json!(edits.len());
        Ok(report)
    }
}
