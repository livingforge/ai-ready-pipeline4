use super::*;

fn fresh_id(used: &mut BTreeSet<String>, preferred: &str) -> String {
    if used.insert(preferred.to_owned()) {
        return preferred.to_owned();
    }
    let mut number = 1;
    loop {
        let candidate = format!("{preferred}-fresh-{number}");
        if used.insert(candidate.clone()) {
            return candidate;
        }
        number += 1;
    }
}

fn uses_region(visual: &Value, region_ids: &BTreeSet<String>) -> Result<bool> {
    let mut references = Vec::new();
    references.extend(array(&visual["evidence"])?.iter());
    if let Some(graph) = visual.get("graph") {
        for kind in ["nodes", "edges"] {
            for item in array(&graph[kind])? {
                references.extend(array(&item["sources"])?.iter());
            }
        }
    }
    Ok(references
        .into_iter()
        .any(|reference| reference.as_str().is_some_and(|id| region_ids.contains(id))))
}

pub(super) fn rebase(root: &Path, journal: &Value, current: &Value) -> Result<(Value, Value)> {
    corrections::validate_journal(journal)?;
    ensure!(
        journal["document"] == current["document_id"]
            && journal["source_path"] == current["source"]["path"],
        "corrections belong to another document or source path"
    );
    let same_source = journal["source_sha256"] == current["source"]["sha256"];
    let mut source_cells = BTreeMap::new();
    for sheet in array(&current["sheets"])? {
        let name = string(&sheet["name"])?;
        for cell in array(&sheet["cells"])? {
            let key = (name.to_owned(), string(&cell["address"])?.to_owned());
            ensure!(
                source_cells.insert(key, cell).is_none(),
                "duplicate source cell"
            );
        }
    }
    let region_ids: BTreeSet<String> = array(&journal["regions"])?
        .iter()
        .map(|region| string(&region["id"]).map(str::to_owned))
        .collect::<Result<_>>()?;
    let mut conflicts = Vec::new();
    let mut candidates = BTreeMap::new();
    for correction in array(&journal["elements"])? {
        let mut element = correction["element"].clone();
        let id = string(&element["id"])?.to_owned();
        let sheet = string(&element["sheet"])?.to_owned();
        let anchors = array(&correction["sources"])?;
        let cells = element["cells"].as_array_mut().unwrap();
        ensure!(
            anchors.len() == cells.len(),
            "correction cell anchor count mismatch"
        );
        let mut missing = false;
        let mut changed = Vec::new();
        for (cell, anchor) in cells.iter_mut().zip(anchors) {
            let address = string(&cell["address"])?.to_owned();
            ensure!(
                anchor["address"] == address,
                "correction cell anchor mismatch"
            );
            let key = (sheet.clone(), address.clone());
            if let Some(source) = source_cells.get(&key) {
                if anchor["sha256"] != hash(&encoded(source)) {
                    cell["text_state"] = json!("not_examined");
                    changed.push(address);
                }
            } else {
                missing = true;
            }
        }
        if missing || (!same_source && !array(&element["evidence"])?.is_empty()) {
            conflicts.push(json!({"kind":"element","id":id,
                "reason":if missing {"source_cells_missing"} else {"image_evidence_requires_new_source_review"}}));
            continue;
        }
        ensure!(
            candidates.insert(id.clone(), element).is_none(),
            "duplicate correction element"
        );
        if !changed.is_empty() {
            conflicts.push(json!({"kind":"element","id":id,
                "reason":"source_cells_changed","cells":changed}));
        }
    }
    let mut transferable: BTreeSet<String> = candidates.keys().cloned().collect();
    loop {
        let removed: Vec<_> = candidates
            .iter()
            .filter(|(id, element)| {
                transferable.contains(*id)
                    && element.get("descriptions").is_some_and(|descriptions| {
                        descriptions.as_array().is_some_and(|references| {
                            references.iter().any(|reference| {
                                !transferable.contains(reference.as_str().unwrap_or(""))
                            })
                        })
                    })
            })
            .map(|(id, _)| id.clone())
            .collect();
        if removed.is_empty() {
            break;
        }
        for id in removed {
            transferable.remove(&id);
            conflicts
                .push(json!({"kind":"element","id":id,"reason":"description_dependency_changed"}));
        }
    }

    let mut result = initialize(root, current)?;
    if same_source {
        result["regions"] = journal["regions"].clone();
    }
    let retained: Vec<Value> = candidates
        .into_iter()
        .filter(|(id, _)| transferable.contains(id))
        .map(|(_, element)| element)
        .collect();
    let owned: BTreeSet<(String, String)> = retained
        .iter()
        .flat_map(|element| {
            let sheet = element["sheet"].as_str().unwrap().to_owned();
            element["cells"]
                .as_array()
                .unwrap()
                .iter()
                .map(move |cell| (sheet.clone(), cell["address"].as_str().unwrap().to_owned()))
        })
        .collect();
    let mut fresh = Vec::new();
    let mut used_ids: BTreeSet<String> = retained
        .iter()
        .map(|element| element["id"].as_str().unwrap().to_owned())
        .collect();
    for mut element in result["elements"].as_array().unwrap().iter().cloned() {
        let sheet = string(&element["sheet"])?.to_owned();
        element["cells"].as_array_mut().unwrap().retain(|cell| {
            !owned.contains(&(sheet.clone(), cell["address"].as_str().unwrap().to_owned()))
        });
        if element["cells"].as_array().unwrap().is_empty() {
            continue;
        }
        let local: BTreeSet<String> = element["cells"]
            .as_array()
            .unwrap()
            .iter()
            .map(|cell| cell["id"].as_str().unwrap().to_owned())
            .collect();
        for cell in element["cells"].as_array_mut().unwrap() {
            cell["headers"]
                .as_array_mut()
                .unwrap()
                .retain(|header| local.contains(header.as_str().unwrap_or("")));
        }
        element["id"] = json!(fresh_id(&mut used_ids, string(&element["id"])?));
        fresh.push(element);
    }
    result["elements"] = json!(retained.iter().chain(fresh.iter()).collect::<Vec<_>>());

    let mut visuals = result["visuals"].as_array().unwrap().clone();
    let mut carried_visuals = 0;
    for correction in array(&journal["visuals"])? {
        let visual = &correction["visual"];
        let anchored = array(&correction["sources"])?.iter().all(|anchor| {
            let Some(pointer) = anchor["pointer"].as_str() else {
                return false;
            };
            current
                .pointer(pointer)
                .is_some_and(|source| anchor["sha256"] == hash(&encoded(source)))
        });
        if anchored
            && (same_source || !uses_region(visual, &region_ids)?)
            && let Some(index) = visuals.iter().position(|candidate| {
                candidate["id"] == visual["id"] && candidate["sources"] == visual["sources"]
            })
        {
            visuals[index] = visual.clone();
            carried_visuals += 1;
        } else {
            conflicts.push(json!({"kind":"visual","id":visual["id"],
                "reason":"source_or_image_evidence_changed"}));
        }
    }
    result["visuals"] = json!(visuals);
    result["review"] = json!({"status":"pending"});
    if same_source && journal["baseline_extraction_hash"] == hash(&encoded(current)) {
        let order: BTreeMap<String, usize> = array(&journal["element_order"])?
            .iter()
            .enumerate()
            .map(|(index, id)| Ok((string(id)?.to_owned(), index)))
            .collect::<Result<_>>()?;
        result["elements"]
            .as_array_mut()
            .unwrap()
            .sort_by_key(|element| {
                order
                    .get(element["id"].as_str().unwrap_or(""))
                    .copied()
                    .unwrap_or(usize::MAX)
            });
        if journal["review"]["content_hash"] == content_hash(&result) {
            result["review"] = journal["review"].clone();
        }
    }
    let validation = validate(root, current, &result)?;
    Ok((
        result,
        json!({"state":if validation["ready"] == true {"reviewed"} else {"needs_review"},
        "ready":validation["ready"],"carried_elements":retained.len(),
        "carried_visuals":carried_visuals,"conflicts":conflicts}),
    ))
}
