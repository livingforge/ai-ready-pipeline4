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

/// The members of a cell that read returns from the original beside those of
/// the interpretation: an edit takes them back and leaves them unread.
const ORIGINAL: &[&str] = &[
    "value",
    "formula",
    "cached",
    "type",
    "number_format",
    "position",
    "style",
    "font",
    "source_id",
];

/// The role a cell of an element of `kind` has unless it is given another.
fn default_role(kind: &Value) -> &'static str {
    if kind == "table" { "data" } else { "text" }
}

/// The headers each cell of an element has unless it is given others, by
/// the roles of the element's cells: of a data cell, the column headers of
/// its column from the top, then the row headers of its row from the left;
/// of a row header, the column headers of its column; of another cell, none.
/// `cells` are the address, role and ID of each cell in order.
fn default_headers(cells: &[(String, String, String)]) -> Vec<Vec<String>> {
    // The header cells by column and by row, each list in reading order.
    let mut placed: Vec<Option<(u32, u32)>> = Vec::with_capacity(cells.len());
    let mut columns: BTreeMap<u32, Vec<(u32, usize)>> = BTreeMap::new();
    let mut rows: BTreeMap<u32, Vec<(u32, usize)>> = BTreeMap::new();
    for (at, (address, role, _)) in cells.iter().enumerate() {
        let place = crate::excel::coordinate(address).ok();
        placed.push(place);
        let Some((column, row)) = place else {
            continue;
        };
        match role.as_str() {
            "column_header" => columns.entry(column).or_default().push((row, at)),
            "row_header" => rows.entry(row).or_default().push((column, at)),
            _ => {}
        }
    }
    for list in columns.values_mut().chain(rows.values_mut()) {
        list.sort();
    }
    let ids = |list: Option<&Vec<(u32, usize)>>| -> Vec<String> {
        list.into_iter()
            .flatten()
            .map(|&(_, at)| cells[at].2.clone())
            .collect()
    };
    cells
        .iter()
        .zip(&placed)
        .map(|((_, role, _), place)| {
            let Some((column, row)) = place else {
                return vec![];
            };
            match role.as_str() {
                "data" => {
                    let mut headers = ids(columns.get(column));
                    headers.extend(ids(rows.get(row)));
                    headers
                }
                "row_header" => ids(columns.get(column)),
                _ => vec![],
            }
        })
        .collect()
}

/// The address, role and ID of each cell of `element` in order, the role and
/// ID a cell names or its defaults.
fn cell_keys(ext: &Extraction, element: &Value) -> Vec<(String, String, String)> {
    let role = default_role(&element["kind"]);
    element["cells"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|cell| {
            let address = cell["address"].as_str().unwrap_or("").to_owned();
            let id = match cell["id"].as_str() {
                Some(id) => id.to_owned(),
                None => {
                    default_cell_id(ext, &element["sheet"], &cell["address"]).unwrap_or_default()
                }
            };
            (
                address,
                cell["role"].as_str().unwrap_or(role).to_owned(),
                id,
            )
        })
        .collect()
}

/// `element` as read returns it and an edit takes it (`edit_element`): each
/// cell without the members that have their default, which most have.
pub(super) fn compact_element(ext: &Extraction, element: &mut Value) {
    let sheet = element["sheet"].clone();
    let role = default_role(&element["kind"]);
    let headers = default_headers(&cell_keys(ext, element));
    for (cell, headers) in element["cells"]
        .as_array_mut()
        .into_iter()
        .flatten()
        .zip(headers)
    {
        let default = default_cell_id(ext, &sheet, &cell["address"]);
        let Some(cell) = cell.as_object_mut() else {
            continue;
        };
        if default.is_some() && cell.get("id").and_then(Value::as_str) == default.as_deref() {
            cell.remove("id");
        }
        if cell.get("role").is_some_and(|given| given == role) {
            cell.remove("role");
        }
        if cell.get("headers") == Some(&json!(headers)) {
            cell.remove("headers");
        }
        if cell.get("text_state").is_some_and(|state| state == "read") {
            cell.remove("text_state");
        }
    }
}

/// The element [`compact_element`] was made of: each cell with every member
/// of the interpretation, and without those read returns from the original.
fn complete_element(ext: &Extraction, element: &mut Value) -> Result<()> {
    let sheet = element["sheet"].clone();
    let element_id = element["id"].clone();
    let role = default_role(&element["kind"]);
    for cell in element["cells"].as_array_mut().into_iter().flatten() {
        if let Some(cell) = cell.as_object_mut() {
            cell.retain(|name, _| !ORIGINAL.contains(&name.as_str()));
        }
        if cell.get("id").is_none() {
            let id = default_cell_id(ext, &sheet, &cell["address"]).with_context(|| {
                format!(
                    "element {element_id} names the sheet {sheet}, which the document does not have"
                )
            })?;
            cell["id"] = json!(id);
        }
        if cell.get("role").is_none() {
            cell["role"] = json!(role);
        }
        if cell.get("text_state").is_none() {
            cell["text_state"] = json!("read");
        }
    }
    // The headers left out follow from the roles, which are now all given.
    let headers = default_headers(&cell_keys(ext, element));
    for (cell, headers) in element["cells"]
        .as_array_mut()
        .into_iter()
        .flatten()
        .zip(headers)
    {
        if cell.get("headers").is_none() {
            cell["headers"] = json!(headers);
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
        let mut upserts: Vec<Value> = edits["upsert"]
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
        // A patch is the element as read returns it with the cells given in
        // place of those at their addresses.
        for patch in edits["patch"].as_array().into_iter().flatten() {
            let id = string(&patch["id"])?;
            let mut element = items
                .iter()
                .find(|element| element["id"] == id)
                .with_context(|| format!("unknown patch target: {collection}/{id}"))?
                .clone();
            compact_element(&ext, &mut element);
            let cells = element["cells"]
                .as_array_mut()
                .context("element cells missing")?;
            for given in array(&patch["cells"])? {
                let address = &given["address"];
                let cell = cells
                    .iter_mut()
                    .find(|cell| cell["address"] == *address)
                    .with_context(|| {
                        format!("element {id} has no cell at {address}; add a cell by upserting the element")
                    })?;
                *cell = given.clone();
            }
            complete_element(&ext, &mut element)?;
            upserts.push(element);
        }
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
    // Of each element that stays, the cells whose headers differ as stored.
    let mut followed: Vec<Vec<String>> = changes
        .iter()
        .map(|change| {
            if change["collection"] != "elements" {
                return vec![];
            }
            let headers = |side: &Value| -> BTreeMap<String, Value> {
                side["cells"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|cell| {
                        Some((
                            cell["address"].as_str()?.to_owned(),
                            cell["headers"].clone(),
                        ))
                    })
                    .collect()
            };
            let (before, after) = (headers(&change["before"]), headers(&change["after"]));
            array(&change["after"]["cells"])
                .into_iter()
                .flatten()
                .filter_map(|cell| cell["address"].as_str())
                .filter(|address| {
                    before
                        .get(*address)
                        .is_some_and(|earlier| Some(earlier) != after.get(*address))
                })
                .map(str::to_owned)
                .collect()
        })
        .collect();
    for (at, change) in changes.iter_mut().enumerate() {
        if change["collection"] == "elements" {
            for side in ["before", "after"] {
                if change[side].is_object() {
                    compact_element(&ext, &mut change[side]);
                }
            }
        }
        if change["before"].is_object() && change["after"].is_object() {
            let followed = std::mem::take(&mut followed[at]);
            *change = differences(change);
            // The cells whose headers changed with the roles of others,
            // beside those the edit changed itself.
            let told: BTreeSet<&str> = change["cells"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|cell| cell["address"].as_str())
                .collect();
            let followed: Vec<String> = followed
                .into_iter()
                .filter(|address| !told.contains(address.as_str()))
                .collect();
            if !followed.is_empty() {
                change["headers_followed"] = json!(followed);
            }
        }
    }
    report["changes"] = json!(changes);
    report["unresolved"] = super::unresolved_by_state(&report["unresolved"]);
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extraction() -> Extraction {
        crate::extraction::sample(json!({"assets":[],"sheets":[
            {"name":"表紙","cells":[],"drawings":[]},
            {"name":"一覧","cells":[],"drawings":[]}]}))
    }

    #[test]
    fn cells_left_without_role_and_headers_take_them_from_the_element() {
        let ext = extraction();
        // Two levels of column headers, a row header, data and a note.
        let cell = |address: &str, role: &str, headers: Value| {
            json!({"id":format!("s2-{address}"),"address":address,"role":role,
                "headers":headers,"text_state":"read"})
        };
        let stored = json!({"id":"t","kind":"table","sheet":"一覧","evidence":[],
            "reading":{"method":"parser","actor":"a","reason":"r"},"cells":[
            cell("A1", "column_header", json!([])),
            cell("B1", "column_header", json!([])),
            cell("B2", "column_header", json!([])),
            cell("A3", "row_header", json!(["s2-A1"])),
            cell("B3", "data", json!(["s2-B1", "s2-B2", "s2-A3"])),
            cell("C3", "data", json!(["s2-A3"])),
            cell("D3", "note", json!([])),
            // Headers other than the roles give are kept as given.
            cell("B4", "data", json!(["s2-B2"])),
            cell("C4", "data", json!([])),
        ]});
        let mut told = stored.clone();
        compact_element(&ext, &mut told);
        assert_eq!(
            told["cells"],
            json!([
                {"address":"A1","role":"column_header"},
                {"address":"B1","role":"column_header"},
                {"address":"B2","role":"column_header"},
                {"address":"A3","role":"row_header"},
                {"address":"B3"},
                {"address":"C3"},
                {"address":"D3","role":"note"},
                {"address":"B4","headers":["s2-B2"]},
                {"address":"C4"}
            ])
        );
        let mut back = told.clone();
        complete_element(&ext, &mut back).unwrap();
        assert_eq!(back, stored);

        // What read adds of the original is taken back unread.
        let mut read = told.clone();
        read["cells"][4]["value"] = json!("読んだ値");
        read["cells"][4]["font"] = json!({"size":11});
        complete_element(&ext, &mut read).unwrap();
        assert_eq!(read, stored);

        // The headers left out follow the roles an edit gives: a new row
        // header heads the data of its row.
        let mut edited = told.clone();
        edited["cells"][7] = json!({"address":"B4","role":"row_header"});
        complete_element(&ext, &mut edited).unwrap();
        assert_eq!(edited["cells"][7]["headers"], json!(["s2-B1", "s2-B2"]));
        assert_eq!(edited["cells"][8]["headers"], json!(["s2-B4"]));

        // A text element's cells are text unless given another role.
        let mut text = json!({"id":"x","kind":"text","sheet":"表紙","evidence":[],
            "reading":{"method":"parser","actor":"a","reason":"r"},
            "cells":[{"address":"A1"},{"address":"A2","role":"unassigned","id":"own"}]});
        complete_element(&ext, &mut text).unwrap();
        assert_eq!(
            text["cells"],
            json!([
                {"id":"s1-A1","address":"A1","role":"text","headers":[],"text_state":"read"},
                {"id":"own","address":"A2","role":"unassigned","headers":[],"text_state":"read"}
            ])
        );
        compact_element(&ext, &mut text);
        assert_eq!(
            text["cells"],
            json!([{"address":"A1"},{"address":"A2","role":"unassigned","id":"own"}])
        );
    }
}
