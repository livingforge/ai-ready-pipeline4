//! Validated ID-based interpretation edits; extraction and evidence stay CLI-owned.
use super::*;

pub(super) fn schema() -> Value {
    let schema = super::schema();
    json!({"$schema":schema["$schema"],"$ref":"#/$defs/edit","$defs":schema["$defs"]})
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
        for item in edits["upsert"].as_array().into_iter().flatten() {
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
    report["changes"] = json!(changes);
    Ok(report)
}
