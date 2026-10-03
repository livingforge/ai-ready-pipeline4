//! Coordinate-free editable document bodies and their separately stored bindings.
use super::*;

pub(super) fn token(kind: &str, parts: &[&str]) -> String {
    // Keep the immutable identity components readable. Escape the separator
    // and escape marker so user-supplied operation IDs cannot alias each other.
    let parts: Vec<String> = parts
        .iter()
        .map(|part| {
            part.bytes()
                .map(|byte| match byte {
                    b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'.' | b'/' | b'#' => {
                        char::from(byte).to_string()
                    }
                    _ => format!("%{byte:02X}"),
                })
                .collect()
        })
        .collect();
    format!("{kind}_{}", parts.join("_"))
}
fn cell_id(row: &str, column: &str) -> String {
    token("cell", &[row, column])
}

/// Attach the interpreted table membership to immutable internal grid identities.
/// Text cells remain individually reversible: no lossy concatenation is performed.
pub(super) fn classify(layout: &mut Value, structure: &[Value]) -> Result<()> {
    for sheet in layout["sheets"]
        .as_object_mut()
        .context("layout sheets missing")?
        .values_mut()
    {
        let mut groups = json!({});
        let positions = |axis: &str| -> Result<BTreeMap<u64, String>> {
            sheet[axis]
                .as_object()
                .context("layout axis missing")?
                .iter()
                .map(|(id, e)| {
                    Ok((
                        e["position"].as_u64().context("position missing")?,
                        id.clone(),
                    ))
                })
                .collect()
        };
        let rows = positions("rows")?;
        let columns = positions("columns")?;
        let mut ids = BTreeMap::new();
        for element in structure.iter().filter(|e| e["sheet"] == sheet["name"]) {
            for cell in array(&element["cells"])? {
                let (column, row) = excel::coordinate(string(&cell["address"])?)?;
                if let (Some(row), Some(column)) =
                    (rows.get(&u64::from(row)), columns.get(&u64::from(column)))
                {
                    ids.insert(string(&cell["id"])?.to_owned(), cell_id(row, column));
                }
            }
        }
        for element in structure
            .iter()
            .filter(|e| e["sheet"] == sheet["name"] && e["kind"] == "table")
        {
            let cells = array(&element["cells"])?;
            let Some(first) = cells.iter().find_map(|c| ids.get(c["id"].as_str()?)) else {
                continue;
            };
            let table = token("table", &[first]);
            for cell in cells {
                if let Some(id) = ids.get(string(&cell["id"])?) {
                    let headers: Vec<_> = array(&cell["headers"])?
                        .iter()
                        .filter_map(|h| h.as_str().and_then(|h| ids.get(h)))
                        .cloned()
                        .collect();
                    groups[id] = json!({"table":table,"role":cell["role"],"headers":headers});
                }
            }
        }
        sheet["groups"] = groups;
    }
    Ok(())
}

pub(super) fn geometry(
    layout: &mut Value,
    extraction: &Value,
    operations: &[excel::StructuralOperation],
) -> Result<()> {
    for (i, source) in array(&extraction["sheets"])?.iter().enumerate() {
        layout["sheets"][identity::page_id(source, i)]["merges"] =
            json!(excel::merges_after(source, operations)?);
    }
    super::visual_elements::refresh(layout, extraction, operations)?;
    Ok(())
}

/// Encode the writer's private grid into reading-order elements, updating only
/// the separate layout record with physical targets and a checked body shape.
pub(super) fn encode(page: &Value, layout: &mut Value) -> Result<Value> {
    let grid = identity::grid(page, layout, false)?;
    let page_id = string(&page["page_id"])?;
    let sheet = &mut layout["sheets"][page_id];
    let mut spans = BTreeMap::new();
    for range in array(&sheet["merges"])? {
        let (start, end) = string(range)?
            .split_once(':')
            .context("invalid merge range")?;
        let (x, y) = excel::coordinate(start)?;
        let (right, bottom) = excel::coordinate(end)?;
        spans.insert(
            (u64::from(y), u64::from(x)),
            (u64::from(bottom), u64::from(right)),
        );
    }
    // Pending insertions inside an existing table stay in that table. Their
    // semantic role needs interpretation; do not copy a neighbouring header.
    let mut bounds: BTreeMap<String, (u64, u64, u64, u64)> = BTreeMap::new();
    if let Some(rows) = grid["blocks"]["table-1"]["rows"].as_object() {
        for (row, values) in rows {
            let y = sheet["rows"][row]["position"]
                .as_u64()
                .context("row position missing")?;
            for column in values.as_object().context("grid row missing")?.keys() {
                let x = sheet["columns"][column]["position"]
                    .as_u64()
                    .context("column position missing")?;
                if let Some(table) = sheet["groups"][cell_id(row, column)]["table"].as_str() {
                    let b = bounds.entry(table.to_owned()).or_insert((y, x, y, x));
                    b.0 = b.0.min(y);
                    b.1 = b.1.min(x);
                    b.2 = b.2.max(y);
                    b.3 = b.3.max(x);
                }
            }
        }
        for (row, values) in rows {
            let y = sheet["rows"][row]["position"].as_u64().unwrap();
            for column in values.as_object().unwrap().keys() {
                let inserted = [(&sheet["rows"], row), (&sheet["columns"], column)]
                    .iter()
                    .any(|(axis, id)| axis[*id]["key"].as_str().is_some_and(|k| k.contains('-')));
                let id = cell_id(row, column);
                if !inserted || !sheet["groups"][&id].is_null() {
                    continue;
                }
                let x = sheet["columns"][column]["position"].as_u64().unwrap();
                let candidates: Vec<_> = bounds
                    .iter()
                    .filter(|(_, b)| b.0 <= y && y <= b.2 && b.1 <= x && x <= b.3)
                    .collect();
                if let [(table, _)] = candidates.as_slice() {
                    sheet["groups"][&id] = json!({"table":table,"role":"unassigned","headers":[]});
                }
            }
        }
    }
    let mut bindings = json!({});
    let mut placed: Vec<((u64, u64), u64, Value)> = vec![];
    type TableCell = (u64, u64, String, String, Value);
    let mut tables: BTreeMap<String, Vec<TableCell>> = BTreeMap::new();
    for (block, body) in grid["blocks"].as_object().context("grid blocks missing")? {
        let Some(rows) = body["rows"].as_object() else {
            continue;
        };
        for (row, values) in rows {
            for (column, value) in values.as_object().context("grid row missing")? {
                let (axis_row, axis_column) = if block == "formulas" {
                    let target = &sheet["formulas"][row];
                    (
                        string(&target["row"])?.to_owned(),
                        string(&target["column"])?.to_owned(),
                    )
                } else {
                    (row.clone(), column.clone())
                };
                let id = if block == "shapes" {
                    token("shape", &[row, column])
                } else {
                    cell_id(&axis_row, &axis_column)
                };
                ensure!(bindings.get(&id).is_none(), "duplicate element binding");
                bindings[&id] = json!({"block":block,"row":row,"column":column});
                let mut y = sheet["rows"][&axis_row]["position"]
                    .as_u64()
                    .unwrap_or(u64::MAX);
                let mut x = sheet["columns"][&axis_column]["position"]
                    .as_u64()
                    .unwrap_or(0);
                let visual = if block == "shapes" {
                    sheet["visuals"]
                        .as_object()
                        .context("visual bindings missing")?
                        .iter()
                        .find(|(_, v)| v["text_key"] == *row)
                        .map(|(id, v)| (id.clone(), v.clone()))
                } else {
                    None
                };
                if let Some((_, v)) = &visual {
                    let position = super::visual_elements::position(v);
                    y = position.0;
                    x = position.1;
                }
                let group = &sheet["groups"][&id];
                let mut cell = json!({"id":id});
                let field = if block == "formulas" {
                    "formula"
                } else {
                    "value"
                };
                cell[field] = value.clone();
                if let Some(table) = group["table"].as_str() {
                    cell["role"] = group["role"].clone();
                    cell["headers"] = group["headers"].clone();
                    tables.entry(table.to_owned()).or_default().push((
                        y,
                        x,
                        axis_row,
                        axis_column,
                        cell,
                    ));
                } else {
                    let mut element = if block == "formulas" {
                        json!({"id":id,"type":"formula","formula":value})
                    } else {
                        json!({"id":id,"type":"text","text":value})
                    };
                    if let Some((reference, _)) = visual {
                        element["visual"] = json!(reference);
                    }
                    placed.push(((y, x), spans.get(&(y, x)).map_or(y, |s| s.0), element));
                }
            }
        }
    }
    for (id, mut cells) in tables {
        cells.sort_by_key(|(y, x, _, _, _)| (*y, *x));
        let first = (cells[0].0, cells[0].1);
        let end = cells
            .iter()
            .map(|c| spans.get(&(c.0, c.1)).map_or(c.0, |s| s.0))
            .max()
            .unwrap();
        let logical_rows: BTreeSet<_> = cells.iter().map(|c| c.0).collect();
        let logical_columns: BTreeSet<_> = cells.iter().map(|c| c.1).collect();
        let mut columns = BTreeMap::new();
        let mut rows: BTreeMap<u64, Value> = BTreeMap::new();
        let present: BTreeSet<_> = cells
            .iter()
            .map(|c| c.4["id"].as_str().unwrap().to_owned())
            .collect();
        for (y, x, row, column, mut cell) in cells {
            if let Some((bottom, right)) = spans.get(&(y, x)) {
                cell["row_span"] = json!(logical_rows.range(y..=*bottom).count());
                cell["column_span"] = json!(logical_columns.range(x..=*right).count());
            }
            let column = token("column", &[&column]);
            columns.insert(x, json!({"id":column}));
            cell["column"] = json!(column);
            cell["headers"]
                .as_array_mut()
                .unwrap()
                .retain(|h| h.as_str().is_some_and(|h| present.contains(h)));
            let row = rows
                .entry(y)
                .or_insert_with(|| json!({"id":token("record", &[&row]),"cells":[]}));
            row["cells"].as_array_mut().unwrap().push(cell);
        }
        placed.push((first,end,json!({"id":id,"type":"table","columns":columns.into_values().collect::<Vec<_>>(),"rows":rows.into_values().collect::<Vec<_>>()})));
    }
    for (reference, visual) in sheet["visuals"]
        .as_object()
        .context("visual bindings missing")?
    {
        let position = super::visual_elements::position(visual);
        let end = visual["anchor"]["to"]["row"]
            .as_u64()
            .and_then(|n| n.checked_add(1))
            .unwrap_or(position.0)
            .max(position.0);
        placed.push((
            position,
            end,
            json!({"id":reference,"type":visual["kind"],"ref":reference}),
        ));
    }
    placed.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| b.2.get("ref").is_some().cmp(&a.2.get("ref").is_some()))
            .then_with(|| a.2["id"].as_str().cmp(&b.2["id"].as_str()))
    });
    let mut elements = vec![];
    let mut previous: Option<(u64, String)> = None;
    let mut blanks = json!({});
    for ((y, _), end, element) in placed {
        let id = string(&element["id"])?.to_owned();
        if let Some((last, before)) = &previous
            && y != u64::MAX
            && *last != u64::MAX
            && y > last.saturating_add(1)
        {
            let gap = token("gap", &[before, &id]);
            blanks[&gap] = json!({"first_row":last+1,"last_row":y-1});
            elements.push(json!({"id":gap,"type":"blank"}));
        }
        previous = Some((
            previous.as_ref().map_or(end, |(last, _)| (*last).max(end)),
            id,
        ));
        elements.push(element);
    }
    let mut out = page.clone();
    out.as_object_mut().unwrap().remove("blocks");
    out["schema_version"] = json!("4");
    let (body, metadata) = crate::document_body::split(&json!(elements))?;
    out["elements"] = body;
    sheet["element_metadata"] = metadata;
    sheet["bindings"] = bindings;
    sheet["metadata_sha256"] = json!(hash(&encoded(&sheet["element_metadata"])));
    sheet["blanks"] = blanks;
    sheet["notes"] = grid["blocks"]["extraction-notes"].clone();
    sheet["block_titles"] = Value::Object(
        grid["blocks"]
            .as_object()
            .unwrap()
            .iter()
            .filter(|(_, body)| body["rows"].is_object())
            .map(|(id, body)| (id.clone(), body["title"].clone()))
            .collect(),
    );
    validate("content", &out)?;
    Ok(out)
}

/// Bind values to recorded slots. Shape changes are refused; same-shape manual
/// permutations are value edits and cannot move the recorded identities.
pub(super) fn decode(page: &Value, layout: &Value) -> Result<Value> {
    ensure!(
        page["schema_version"] == "4",
        "Document content must use document elements; re-import the original"
    );
    let sheet = &layout["sheets"][string(&page["page_id"])?];
    ensure!(
        hash(&encoded(&sheet["element_metadata"])) == sheet["metadata_sha256"],
        "element structure differs from bindings; use structure-save or row/column operations"
    );
    let mut expanded = page.clone();
    expanded["elements"] =
        crate::document_body::join(&page["elements"], &sheet["element_metadata"])?;
    super::visual_elements::validate_references(&expanded, sheet)?;
    let mut out = page.clone();
    out.as_object_mut().unwrap().remove("elements");
    out["blocks"] = json!({"extraction-notes":sheet["notes"]});
    for (block, title) in sheet["block_titles"]
        .as_object()
        .context("block titles missing")?
    {
        out["blocks"][block] = json!({"title":title,"rows":{}});
    }
    let mut used = BTreeSet::new();
    let mut put = |id: &str, value: &Value| -> Result<()> {
        ensure!(used.insert(id.to_owned()), "duplicate element cell ID");
        let binding = &sheet["bindings"][id];
        let block = string(&binding["block"])?;
        let row = string(&binding["row"])?;
        let column = string(&binding["column"])?;
        if out["blocks"][block].is_null() {
            out["blocks"][block] = json!({"title":match block {"table-1"=>"本文","formulas"=>"数式原文",_=>"図形の文字"},"rows":{}});
        }
        out["blocks"][block]["rows"][row][column] = value.clone();
        Ok(())
    };
    for element in array(&expanded["elements"])? {
        match string(&element["type"])? {
            "text" => put(string(&element["id"])?, &element["text"])?,
            "formula" => put(string(&element["id"])?, &element["formula"])?,
            "table" => {
                for row in array(&element["rows"])? {
                    for cell in array(&row["cells"])? {
                        put(
                            string(&cell["id"])?,
                            cell.get("formula").unwrap_or(&cell["value"]),
                        )?;
                    }
                }
            }
            "blank" | "image" | "chart" | "drawing" => {}
            _ => bail!("unknown element type"),
        }
    }
    ensure!(
        used.len()
            == sheet["bindings"]
                .as_object()
                .context("bindings missing")?
                .len(),
        "element binding coverage mismatch"
    );
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::token;

    #[test]
    fn readable_ids_keep_component_boundaries_and_literal_escapes_distinct() {
        let inputs = [
            vec!["add_row-1", "B"],
            vec!["add", "row-1_B"],
            vec!["add%5Frow-1", "B"],
            vec!["", "A"],
            vec!["_A"],
            vec!["追加-1", "B"],
        ];
        let ids: std::collections::BTreeSet<_> =
            inputs.iter().map(|parts| token("cell", parts)).collect();
        assert_eq!(ids.len(), inputs.len());
        assert_eq!(token("cell", &["r7", "B"]), "cell_r7_B");
    }
}
