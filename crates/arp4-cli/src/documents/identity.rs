//! Private grid projection for writeback. Persisted bodies are document elements;
//! all grid identities and physical positions live in layout.yml.
use super::*;

pub(super) fn is_excel(extraction: &Value) -> bool {
    extraction["parser"]
        .as_str()
        .is_some_and(|p| p.contains(";cells/"))
}

pub(super) fn page_id(sheet: &Value, index: usize) -> String {
    sheet["page"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| format!("sheet-{}", index + 1))
}

pub(super) fn initial(extraction: &Value) -> Result<Value> {
    let mut sheets = json!({});
    for (index, sheet) in array(&extraction["sheets"])?.iter().enumerate() {
        sheets[page_id(sheet, index)] = initial_sheet(sheet)?;
    }
    let mut layout = json!({"schema_version":"4","document_id":extraction["document_id"],"source_sha256":extraction["source"]["sha256"],"sheets":sheets,"history":[]});
    reorder(&mut layout)?;
    Ok(layout)
}

/// The layout of a page before its content is encoded: its rows, columns and
/// formulas by their positions in the extraction `sheet`.
pub(super) fn initial_sheet(sheet: &Value) -> Result<Value> {
    {
        let mut rows = json!({});
        let mut columns = json!({});
        let mut formulas = json!({});
        let mut max_column = 0;
        for cell in array(&sheet["cells"])? {
            let (column, row) = excel::coordinate(string(&cell["address"])?)?;
            rows[format!("r{row}")] = json!({"key":format!("r{row}"),"position":row});
            max_column = max_column.max(column);
            if cell["type"] == "formula" {
                formulas[string(&cell["id"])?] =
                    json!({"row":format!("r{row}"),"column":excel::column_name(column)?});
            }
        }
        for column in 1..=max_column {
            let key = excel::column_name(column)?;
            columns[&key] = json!({"key":key,"position":column});
        }
        Ok(
            json!({"name":sheet["name"],"rows":rows,"columns":columns,"formulas":formulas,"row_order":[],"column_order":[],"element_metadata":[],"bindings":{},"groups":{},"notes":{},"metadata_sha256":hash(&encoded(&json!([]))),"blanks":{},"merges":sheet["merges"],"block_titles":{},"visuals":{}}),
        )
    }
}

pub(super) fn reorder(layout: &mut Value) -> Result<()> {
    for sheet in layout["sheets"]
        .as_object_mut()
        .context("layout sheets missing")?
        .values_mut()
    {
        for (axis, order) in [("rows", "row_order"), ("columns", "column_order")] {
            let mut entries = vec![];
            let mut positions = BTreeSet::new();
            let mut keys = BTreeSet::new();
            for (id, entry) in sheet[axis].as_object().context("layout axis missing")? {
                identifier(id)?;
                ensure!(keys.insert(string(&entry["key"])?), "duplicate layout key");
                let position = entry["position"]
                    .as_u64()
                    .context("layout position missing")?;
                ensure!(positions.insert(position), "duplicate layout position");
                entries.push((position, id.clone()));
            }
            entries.sort();
            sheet[order] = json!(entries.into_iter().map(|(_, id)| id).collect::<Vec<_>>());
        }
    }
    Ok(())
}

pub(super) fn load(dir: &Path, extraction: &Value, planned: &Planned) -> Result<Value> {
    let layout = read_planned(
        "layout.yml",
        &dir.join("layout.yml"),
        planned,
        "document-layout",
    )
    .context("Document element bindings missing; re-import the original")?;
    ensure!(
        layout["document_id"] == extraction["document_id"]
            && layout["source_sha256"] == extraction["source"]["sha256"],
        "layout source mismatch"
    );
    let mut ordered = layout.clone();
    reorder(&mut ordered)?;
    ensure!(ordered == layout, "layout presentation order mismatch");
    let mappings = read_planned(
        "mappings.yml",
        &dir.join("mappings.yml"),
        planned,
        "mappings",
    )?;
    let operated = operated_extraction(extraction, &mappings["operations"])?;
    let operations =
        excel::parse_operations(array(&mappings["operations"])?, array(&operated["sheets"])?)?;
    update(&mut ordered, &operations)?;
    elements::geometry(&mut ordered, &operated, &operations)?;
    // mappings operations are the writeback authority. Project them even when
    // authored directly; the CLI saves the resulting layout atomically and
    // apply persists it through reimport. Only the current element format is accepted.
    Ok(ordered)
}

fn axis_keys(sheet: &Value, axis: &str, to_runtime: bool) -> Result<BTreeMap<String, String>> {
    sheet[axis]
        .as_object()
        .context("layout axis missing")?
        .iter()
        .map(|(id, entry)| {
            let key = string(&entry["key"])?;
            Ok(if to_runtime {
                (id.clone(), key.to_owned())
            } else {
                (key.to_owned(), id.clone())
            })
        })
        .collect()
}

fn formula_keys(sheet: &Value, page: &str, to_runtime: bool) -> Result<BTreeMap<String, String>> {
    let rows = axis_keys(sheet, "rows", true)?;
    let columns = axis_keys(sheet, "columns", true)?;
    sheet["formulas"]
        .as_object()
        .context("layout formulas missing")?
        .iter()
        .map(|(id, target)| {
            let row = rows
                .get(string(&target["row"])?)
                .context("formula row identity missing")?;
            let col = columns
                .get(string(&target["column"])?)
                .context("formula column identity missing")?;
            let key = format!(
                "c-{}-{}{}",
                page.trim_start_matches("sheet-"),
                col,
                row.trim_start_matches('r')
            );
            Ok(if to_runtime {
                (id.clone(), key)
            } else {
                (key, id.clone())
            })
        })
        .collect()
}

/// Translate internal grid identities to the writer coordinate keys.
pub(super) fn grid(page: &Value, layout: &Value, to_runtime: bool) -> Result<Value> {
    ensure!(
        page["schema_version"] == "4",
        "Document body requires document elements"
    );
    let page_id = string(&page["page_id"])?;
    let sheet = &layout["sheets"][page_id];
    ensure!(
        sheet.is_object() && sheet["name"] == page["title"],
        "layout page mismatch"
    );
    let rows = axis_keys(sheet, "rows", to_runtime)?;
    let columns = axis_keys(sheet, "columns", to_runtime)?;
    let formulas = formula_keys(sheet, page_id, to_runtime)?;
    let mut out = page.clone();
    if let Some(body) = page["blocks"]["table-1"]["rows"].as_object() {
        let mut converted = json!({});
        for (row, values) in body {
            let target_row = rows
                .get(row)
                .context("unknown row identity: deleted by operations or unmapped")?;
            for (column, value) in values.as_object().context("invalid body row")? {
                let target_column = columns.get(column).context("unknown column identity")?;
                converted[target_row][target_column] = value.clone();
            }
        }
        out["blocks"]["table-1"]["rows"] = converted;
    }
    if let Some(body) = page["blocks"]["formulas"]["rows"].as_object() {
        let mut converted = json!({});
        for (field, value) in body {
            converted[formulas.get(field).with_context(|| {
                format!(
                    "{}!{} is deleted or has unknown formula identity",
                    sheet["name"].as_str().unwrap_or("sheet"),
                    field.rsplit('-').next().unwrap_or(field)
                )
            })?] = value.clone();
        }
        out["blocks"]["formulas"]["rows"] = converted;
    }
    // Fonts follow the values they belong to; one whose row, column or formula
    // an operation removed is dropped with its value.
    if let Some(fonts) = page["fonts"].as_object() {
        let mut converted = json!({});
        for (block, body) in fonts {
            let body = body.as_object().context("invalid font block")?;
            match block.as_str() {
                "table-1" => {
                    for (row, values) in body {
                        let Some(target_row) = rows.get(row) else {
                            continue;
                        };
                        for (column, font) in values.as_object().context("invalid font row")? {
                            if let Some(target_column) = columns.get(column) {
                                converted["table-1"][target_row][target_column] = font.clone();
                            }
                        }
                    }
                }
                "formulas" => {
                    for (field, font) in body {
                        if let Some(target) = formulas.get(field) {
                            converted["formulas"][target] = font.clone();
                        }
                    }
                }
                _ => converted[block] = Value::Object(body.clone()),
            }
        }
        out["fonts"] = converted;
    }
    if !to_runtime {
        // Formula caches are extraction metadata, not editable body values.
        for target in sheet["formulas"]
            .as_object()
            .context("layout formulas missing")?
            .values()
        {
            let row = string(&target["row"])?;
            let column = string(&target["column"])?;
            if let Some(values) = out["blocks"]["table-1"]["rows"][row].as_object_mut() {
                values.remove(column);
            }
        }
        if let Some(rows) = out["blocks"]["table-1"]["rows"].as_object_mut() {
            rows.retain(|_, values| values.as_object().is_none_or(|v| !v.is_empty()));
        }
    }
    Ok(out)
}

pub(super) fn runtime(page: &Value, layout: &Value, extraction: &Value) -> Result<Value> {
    let decoded = super::elements::decode(page, layout)?;
    let mut page = grid(&decoded, layout, true)?;
    if is_excel(extraction) {
        let index: usize = string(&page["page_id"])?
            .trim_start_matches("sheet-")
            .parse()?;
        let source = array(&extraction["sheets"])?
            .get(index - 1)
            .context("missing layout sheet")?;
        let fields = formula_keys(
            &layout["sheets"][string(&page["page_id"])?],
            string(&page["page_id"])?,
            true,
        )?;
        for cell in array(&source["cells"])? {
            if cell["type"] == "formula" && fields.values().any(|field| cell["id"] == *field) {
                let (column, row) = excel::coordinate(string(&cell["address"])?)?;
                let row = format!("r{row}");
                let column = excel::column_name(column)?;
                ensure!(
                    page["blocks"]["table-1"]["rows"]
                        .get(&row)
                        .and_then(|r| r.get(&column))
                        .is_none(),
                    "{}!{} is not written back: formula caches belong to extraction metadata, not body YAML",
                    source["name"].as_str().unwrap_or("sheet"),
                    cell["address"].as_str().unwrap_or("cell")
                );
                if page["blocks"]["table-1"].is_null() {
                    page["blocks"]["table-1"] = json!({"title":"本文","rows":{}});
                }
                page["blocks"]["table-1"]["rows"][&row][&column] = cell["value"].clone();
            }
        }
    }
    Ok(page)
}

impl Store {
    pub(super) fn read_content(&self, dir: &Path, name: &str, extraction: &Value) -> Result<Value> {
        let page = read(&under(dir, name)?, Some("content"))?;
        runtime(&page, &load(dir, extraction, &Planned::new())?, extraction)
    }
}

/// Refresh placements without renaming existing identities or body keys.
pub(super) fn update(layout: &mut Value, operations: &[excel::StructuralOperation]) -> Result<()> {
    for operation in operations {
        let history = layout["history"]
            .as_array_mut()
            .context("layout history missing")?;
        {
            let kind = match operation.kind {
                excel::OperationKind::InsertRows => "insert_rows",
                excel::OperationKind::DeleteRows => "delete_rows",
                excel::OperationKind::InsertColumns => "insert_columns",
                excel::OperationKind::DeleteColumns => "delete_columns",
                excel::OperationKind::MoveColumns { .. } => "move_columns",
            };
            let mut record = json!({"id":operation.id,"sheet":operation.sheet,"kind":kind,"at":operation.at,"count":operation.count,"reason":"ARP recorded structural operation"});
            if let excel::OperationKind::MoveColumns { to } = operation.kind {
                record["to"] = json!(to);
            }
            if let Some(style_from) = operation.style_from {
                record["style_from"] = json!(style_from);
            }
            if let Some(existing) = history.iter().find(|o| o["id"] == operation.id) {
                ensure!(
                    *existing == record,
                    "operation history mismatch; do not rewrite applied operations"
                );
            } else {
                history.push(record);
            }
        }
    }
    for sheet in layout["sheets"]
        .as_object_mut()
        .context("layout sheets missing")?
        .values_mut()
    {
        let name = string(&sheet["name"])?.to_owned();
        for (axis, rows) in [("rows", true), ("columns", false)] {
            let axis_operations: Vec<_> = operations
                .iter()
                .filter(|op| op.row_operation() == rows)
                .cloned()
                .collect();
            let entries = sheet[axis].as_object_mut().context("layout axis missing")?;
            for entry in entries.values_mut() {
                let key = string(&entry["key"])?;
                let address = if rows {
                    format!("A{}", key.trim_start_matches('r'))
                } else {
                    format!("{key}1")
                };
                if !key.contains('-') {
                    if let Some(mapped) = excel::map_coordinate(&name, &address, &axis_operations)?
                    {
                        let (column, row) = excel::coordinate(&mapped)?;
                        entry["position"] = json!(if rows { row } else { column });
                    } else {
                        entry["position"] = Value::Null;
                    }
                }
            }
            entries.retain(|_, e| !e["position"].is_null());
            for op in operations
                .iter()
                .filter(|o| o.sheet == name && o.row_operation() == rows && o.insertion())
            {
                for offset in 0..op.count {
                    let id = format!("{}-{}", op.id, offset + 1);
                    let resolved = excel::resolve_insertion(
                        &name,
                        &op.id,
                        offset,
                        if rows { Some("A") } else { None },
                        if rows { None } else { Some(1) },
                        &axis_operations,
                    );
                    if let Ok(address) = resolved {
                        let (column, row) = excel::coordinate(&address)?;
                        if let Some(present) = entries.get(&id) {
                            ensure!(
                                present["key"] == id,
                                "operation identity already used; choose a fresh --id"
                            );
                        }
                        entries.insert(
                            id.clone(),
                            json!({"key":id,"position":if rows{row}else{column}}),
                        );
                    } else {
                        entries.remove(&id);
                    }
                }
            }
        }
        let kept_rows: BTreeSet<_> = sheet["rows"]
            .as_object()
            .context("layout rows missing")?
            .keys()
            .cloned()
            .collect();
        let kept_columns: BTreeSet<_> = sheet["columns"]
            .as_object()
            .context("layout columns missing")?
            .keys()
            .cloned()
            .collect();
        sheet["formulas"]
            .as_object_mut()
            .context("layout formulas missing")?
            .retain(|_, target| {
                target["row"]
                    .as_str()
                    .is_some_and(|row| kept_rows.contains(row))
                    && target["column"]
                        .as_str()
                        .is_some_and(|col| kept_columns.contains(col))
            });
    }
    reorder(layout)
}

/// The trusted apply path knows the placement of every retained identity.
/// External sources have no such proof: only one-cell changes on the same grid
/// are accepted; structural or multi-cell changes require an explicit mapping.
pub(super) fn reimport(dir: &Path, extraction: &Value, trusted: bool) -> Result<Value> {
    if !dir.join("document.yml").exists() {
        return initial(extraction);
    }
    let before = read(&dir.join("extraction.json"), Some("extraction"))?;
    let mut layout = load(dir, &before, &Planned::new())?;
    if !trusted {
        let mappings = read(&dir.join("mappings.yml"), Some("mappings"))?;
        ensure!(
            array(&mappings["operations"])?.is_empty(),
            "ambiguous correspondence: apply recorded operations before reimport; pending identities must not be guessed"
        );
    }
    if trusted {
        let mappings = read(&dir.join("mappings.yml"), Some("mappings"))?;
        let operations =
            excel::parse_operations(array(&mappings["operations"])?, array(&before["sheets"])?)?;
        update(&mut layout, &operations)?;
    } else if before["sheets"] != extraction["sheets"] {
        let old = array(&before["sheets"])?;
        let new = array(&extraction["sheets"])?;
        ensure!(
            old.len() == new.len(),
            "ambiguous correspondence: external sheet structure changed"
        );
        let mut changed = 0;
        for (a, b) in old.iter().zip(new) {
            ensure!(
                a["name"] == b["name"] && a["merges"] == b["merges"],
                "ambiguous correspondence: external sheet structure changed"
            );
            let ac = array(&a["cells"])?;
            let bc = array(&b["cells"])?;
            ensure!(
                ac.len() == bc.len(),
                "ambiguous correspondence: external row/column structure changed; no identity guessing"
            );
            for (x, y) in ac.iter().zip(bc) {
                ensure!(
                    x["address"] == y["address"] && x["type"] == y["type"],
                    "ambiguous correspondence: external coordinates or cell types changed"
                );
                changed += usize::from(
                    (x["type"] != "formula" && x["value"] != y["value"])
                        || x["formula"] != y["formula"],
                );
            }
        }
        ensure!(
            changed <= 1,
            "ambiguous correspondence: external multi-cell edit could be a column move; no matching by duplicate headers"
        );
    }
    let new_sheets = array(&extraction["sheets"])?;
    ensure!(
        layout["sheets"]
            .as_object()
            .context("layout sheets missing")?
            .len()
            == new_sheets.len(),
        "layout sheet coverage mismatch"
    );
    for (index, source) in new_sheets.iter().enumerate() {
        let sheet = &mut layout["sheets"][format!("sheet-{}", index + 1)];
        ensure!(
            sheet["name"] == source["name"],
            "layout sheet order changed"
        );
        for (axis, rows) in [("rows", true), ("columns", false)] {
            for entry in sheet[axis]
                .as_object_mut()
                .context("layout axis missing")?
                .values_mut()
            {
                let position = u32::try_from(
                    entry["position"]
                        .as_u64()
                        .context("layout position missing")?,
                )?;
                entry["key"] = json!(if rows {
                    format!("r{position}")
                } else {
                    excel::column_name(position)?
                });
            }
        }
        let identities = |axis: &str| -> Result<BTreeMap<u64, String>> {
            sheet[axis]
                .as_object()
                .context("layout axis missing")?
                .iter()
                .map(|(id, e)| {
                    Ok((
                        e["position"].as_u64().context("layout position missing")?,
                        id.clone(),
                    ))
                })
                .collect()
        };
        let rows = identities("rows")?;
        let columns = identities("columns")?;
        let mut formula_cells = BTreeSet::new();
        for cell in array(&source["cells"])? {
            let (column, row) = excel::coordinate(string(&cell["address"])?)?;
            let (Some(row), Some(column)) =
                (rows.get(&u64::from(row)), columns.get(&u64::from(column)))
            else {
                bail!("layout does not cover imported cell; correspondence not proven");
            };
            if cell["type"] == "formula" {
                formula_cells.insert((row.clone(), column.clone()));
            }
        }
        // The formulas of this version: a cell keeps its formula identity, one
        // that holds a value now loses it, and a formula the writer added, such
        // as a table's calculated column in an inserted row, takes a new one.
        let formulas = sheet["formulas"]
            .as_object_mut()
            .context("layout formulas missing")?;
        formulas.retain(|_, target| {
            formula_cells.remove(&(
                target["row"].as_str().unwrap_or("").to_owned(),
                target["column"].as_str().unwrap_or("").to_owned(),
            ))
        });
        for (row, column) in formula_cells {
            let id = elements::token("formula", &[&row, &column]);
            ensure!(
                !formulas.contains_key(&id),
                "formula identity {id} already used"
            );
            formulas.insert(id, json!({"row":row,"column":column}));
        }
    }
    layout["source_sha256"] = extraction["source"]["sha256"].clone();
    reorder(&mut layout)?;
    Ok(layout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deleting_first_row_keeps_original_and_inserted_columns() {
        let extraction = json!({"document_id":"book.xlsx","source":{"sha256":"hash"},
        "sheets":[{"name":"Sheet1","merges":[],"cells":[
            {"address":"A1","type":"string"},{"address":"B2","type":"string"}
        ]}]});
        let mut layout = initial(&extraction).unwrap();
        let operations = excel::parse_operations(
            &[
                json!({"id":"add","kind":"insert_columns","sheet":"Sheet1","at":2,"count":1,"reason":"test"}),
                json!({"id":"delete","kind":"delete_rows","sheet":"Sheet1","at":1,"count":1,"reason":"test"}),
            ],
            extraction["sheets"].as_array().unwrap(),
        ).unwrap();
        update(&mut layout, &operations).unwrap();
        let sheet = &layout["sheets"]["sheet-1"];
        assert_eq!(sheet["column_order"], json!(["A", "add-1", "B"]));
        assert_eq!(sheet["row_order"], json!(["r2"]));
        assert_eq!(sheet["rows"]["r2"]["position"], 1);
    }
}
