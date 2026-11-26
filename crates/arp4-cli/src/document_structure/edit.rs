//! Validated ID-based interpretation edits; extraction and evidence stay CLI-owned.
use super::*;

pub(super) fn schema() -> Value {
    let schema = super::schema();
    // The definitions an edit request is made of: the others describe what
    // the CLI stores.
    let mut used = BTreeSet::new();
    let mut pending = vec!["edit".to_owned()];
    while let Some(name) = pending.pop() {
        if !used.insert(name.clone()) {
            continue;
        }
        references(&schema["$defs"][&name], &mut pending);
    }
    let defs: serde_json::Map<String, Value> = used
        .into_iter()
        .map(|name| {
            let definition = schema["$defs"][&name].clone();
            (name, definition)
        })
        .collect();
    json!({"$schema":schema["$schema"],"$ref":"#/$defs/edit","$defs":defs})
}

/// The change of an item that is kept, told by what differs: under
/// `changed` each member with its values before and after, and under `cells`
/// each cell that differs, by its address, with a cell added or removed
/// having no value on the other side.
fn differences(change: &Value) -> Value {
    let (before, after) = (&change["before"], &change["after"]);
    let mut out = json!({"collection":change["collection"],"id":change["id"]});
    let mut changed = serde_json::Map::new();
    let names: BTreeSet<&String> = before
        .as_object()
        .into_iter()
        .chain(after.as_object())
        .flat_map(|members| members.keys())
        .filter(|name| name.as_str() != "cells")
        .collect();
    for name in names {
        if before.get(name) != after.get(name) {
            changed.insert(
                name.clone(),
                json!({"before":before.get(name),"after":after.get(name)}),
            );
        }
    }
    if !changed.is_empty() {
        out["changed"] = Value::Object(changed);
    }
    let by_address = |side: &Value| -> BTreeMap<String, Value> {
        side["cells"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|cell| {
                let mut cell = cell.clone();
                let address = cell.as_object_mut()?.remove("address")?;
                Some((address.as_str()?.to_owned(), cell))
            })
            .collect()
    };
    let (earlier, later) = (by_address(before), by_address(after));
    let mut cells = vec![];
    // In the order of the item after the change, then the cells taken out.
    for cell in after["cells"].as_array().into_iter().flatten() {
        let Some(address) = cell["address"].as_str() else {
            continue;
        };
        if earlier.get(address) != later.get(address) {
            cells.push(
                json!({"address":address,"before":earlier.get(address),"after":later.get(address)}),
            );
        }
    }
    for (address, cell) in &earlier {
        if !later.contains_key(address) {
            cells.push(json!({"address":address,"before":cell,"after":null}));
        }
    }
    if !cells.is_empty() {
        out["cells"] = json!(cells);
    }
    out
}

/// The definitions `schema` refers to, by name.
fn references(schema: &Value, names: &mut Vec<String>) {
    match schema {
        Value::Object(members) => {
            for (key, value) in members {
                match (key.as_str(), value.as_str()) {
                    ("$ref", Some(reference)) => {
                        if let Some(name) = reference.strip_prefix("#/$defs/") {
                            names.push(name.to_owned());
                        }
                    }
                    _ => references(value, names),
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|item| references(item, names)),
        _ => {}
    }
}

/// The ID a cell at `address` of the sheet named `sheet` has unless it is
/// given another: `s<N>-<address>`, N the sheet's number from 1.
fn default_cell_id(ext: &Extraction, sheet: &Value, address: &Value) -> Option<String> {
    let number = ext.sheets.iter().position(|s| s["name"] == *sheet)? + 1;
    Some(format!("s{number}-{}", address.as_str()?))
}

/// `element` as read returns it and an edit takes it (`edit_element`): each
/// cell without the members that have their default, which most have.
pub(super) fn compact_element(ext: &Extraction, element: &mut Value) {
    let sheet = element["sheet"].clone();
    for cell in element["cells"].as_array_mut().into_iter().flatten() {
        let default = default_cell_id(ext, &sheet, &cell["address"]);
        let Some(cell) = cell.as_object_mut() else {
            continue;
        };
        if default.is_some() && cell.get("id").and_then(Value::as_str) == default.as_deref() {
            cell.remove("id");
        }
        if cell
            .get("headers")
            .and_then(Value::as_array)
            .is_some_and(Vec::is_empty)
        {
            cell.remove("headers");
        }
        if cell.get("text_state").is_some_and(|state| state == "read") {
            cell.remove("text_state");
        }
    }
}

/// The element [`compact_element`] was made of: each cell with every member.
fn complete_element(ext: &Extraction, element: &mut Value) -> Result<()> {
    let sheet = element["sheet"].clone();
    let element_id = element["id"].clone();
    for cell in element["cells"].as_array_mut().into_iter().flatten() {
        if cell.get("id").is_none() {
            let id = default_cell_id(ext, &sheet, &cell["address"]).with_context(|| {
                format!(
                    "element {element_id} names the sheet {sheet}, which the document does not have"
                )
            })?;
            cell["id"] = json!(id);
        }
        if cell.get("headers").is_none() {
            cell["headers"] = json!([]);
        }
        if cell.get("text_state").is_none() {
            cell["text_state"] = json!("read");
        }
    }
    Ok(())
}

pub(super) fn execute(
    root: &Path,
    extraction_path: &Path,
    structure: &Path,
    input: &Path,
    dry_run: bool,
) -> Result<Value> {
    let request = read(input, None)?;
    jsonschema::validator_for(&schema())?
        .validate(&request)
        .map_err(|e| anyhow::anyhow!("invalid structure edit: {e}"))?;
    let ext = extraction(extraction_path)?;
    let before = read(structure, None)?;
    validate(root, &ext, &before)?;
    ensure!(
        request["revision"] == hash(&encoded(&before)),
        "structure revision changed; read again"
    );
    let mut after = before.clone();
    let mut changes = vec![];
    for collection in ["elements", "visuals"] {
        let Some(edits) = request.get(collection) else {
            continue;
        };
        // The elements given as read returns them, with every member.
        let upserts: Vec<Value> = edits["upsert"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|item| {
                let mut item = item.clone();
                if collection == "elements" {
                    complete_element(&ext, &mut item)?;
                }
                Ok(item)
            })
            .collect::<Result<_>>()?;
        let items = after[collection].as_array_mut().unwrap();
        let mut seen = BTreeSet::new();
        for id in edits["remove"].as_array().into_iter().flatten() {
            let id = string(id)?;
            ensure!(seen.insert(id), "duplicate edit target: {collection}/{id}");
            let index = items
                .iter()
                .position(|e| e["id"] == id)
                .with_context(|| format!("unknown removal target: {collection}/{id}"))?;
            let previous = items.remove(index);
            changes.push(json!({"collection":collection,"id":id,"before":previous,"after":null}));
        }
        for item in &upserts {
            let id = string(&item["id"])?;
            ensure!(seen.insert(id), "duplicate edit target: {collection}/{id}");
            if let Some(index) = items.iter().position(|e| e["id"] == id) {
                if items[index] != *item {
                    changes.push(
                        json!({"collection":collection,"id":id,"before":items[index],"after":item}),
                    );
                    items[index] = item.clone();
                }
            } else {
                items.push(item.clone());
                changes.push(json!({"collection":collection,"id":id,"before":null,"after":item}));
            }
        }
    }
    if !changes.is_empty() {
        after["review"] = json!({"status":"pending"});
    }
    let mut report = validate(root, &ext, &after)?;
    ensure!(
        read(structure, None)? == before,
        "structure changed during edit; read again"
    );
    ensure!(
        extraction(extraction_path)? == ext,
        "extraction changed during edit; read again"
    );
    if !dry_run && !changes.is_empty() {
        write(structure, &after)?;
    }
    report["state"] = json!(if dry_run {
        "planned"
    } else if changes.is_empty() {
        "unchanged"
    } else {
        "written"
    });
    report["base_revision"] = request["revision"].clone();
    report["revision"] = json!(hash(&encoded(&after)));
    // The elements changed are told as read tells them, and of an item that
    // stays, what differs.
    for change in &mut changes {
        if change["collection"] == "elements" {
            for side in ["before", "after"] {
                if change[side].is_object() {
                    compact_element(&ext, &mut change[side]);
                }
            }
        }
        if change["before"].is_object() && change["after"].is_object() {
            *change = differences(change);
        }
    }
    report["changes"] = json!(changes);
    report["unresolved"] = super::unresolved_by_state(&report["unresolved"]);
    Ok(report)
}
