use super::*;

pub(super) fn fresh_id(used: &mut BTreeSet<String>, preferred: &str) -> String {
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

fn holding_index(elements: &[Value]) -> BTreeMap<String, BTreeMap<String, (usize, usize)>> {
    let mut holding: BTreeMap<String, BTreeMap<String, (usize, usize)>> = BTreeMap::new();
    for (element_index, element) in elements.iter().enumerate() {
        let Some(sheet) = element["sheet"].as_str() else {
            continue;
        };
        if let Ok(cells) = array(&element["cells"]) {
            for (cell_index, cell) in cells.iter().enumerate() {
                if let Some(address) = cell["address"].as_str() {
                    // The first element wins when inferred elements overlap.
                    holding
                        .entry(sheet.to_owned())
                        .or_default()
                        .entry(address.to_owned())
                        .or_insert((element_index, cell_index));
                }
            }
        }
    }
    holding
}

/// Replays `journal` on the extraction `current`. `original` requires the
/// original to be still the one `current` was extracted from.
pub(super) fn rebase(
    root: &Path,
    journal: &Value,
    current: &Value,
    extraction_hash: &str,
    original: bool,
) -> Result<(Value, Value)> {
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

    // Validated once below, after the corrections are applied.
    let mut result = inferred(current, extraction_hash)?;
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
    let corrected: BTreeSet<&str> = retained
        .iter()
        .map(|element| element["id"].as_str().unwrap())
        .collect();
    let mut elements: Vec<Value> = retained.iter().chain(fresh.iter()).cloned().collect();

    // A reading of an element the parser got right goes to the parser's
    // element holding its anchor cell, while that cell is what was read.
    let holding = holding_index(&elements);
    for reading in array(&journal["readings"])? {
        let sheet = string(&reading["sheet"])?;
        let address = string(&reading["anchor"]["address"])?;
        let now = source_cells
            .get(&(sheet.to_owned(), address.to_owned()))
            .copied();
        let reason = match holding
            .get(sheet)
            .and_then(|cells| cells.get(address))
            .map(|(index, _)| *index)
        {
            None => Some("source_cells_missing"),
            Some(index) if corrected.contains(elements[index]["id"].as_str().unwrap_or("")) => {
                Some("anchor_in_corrected_element")
            }
            Some(_) if corrections::cell_hash(now) != reading["anchor"]["sha256"] => {
                Some("source_cells_changed")
            }
            Some(index) => {
                elements[index]["reading"] = reading["reading"].clone();
                None
            }
        };
        if let Some(reason) = reason {
            conflicts.push(json!({"kind":"reading","id":reading["element"],"reason":reason}));
        }
    }

    // A text_state a reader gave holds while the cell is as it was; a changed
    // cell is to be examined again.
    for state in array(&journal["cell_states"])? {
        let sheet = string(&state["sheet"])?;
        let address = string(&state["address"])?;
        let now = source_cells
            .get(&(sheet.to_owned(), address.to_owned()))
            .copied();
        let Some(&(index, cell_index)) = holding.get(sheet).and_then(|cells| cells.get(address))
        else {
            continue;
        };
        let changed = corrections::cell_hash(now) != state["sha256"];
        let cell = &mut elements[index]["cells"][cell_index];
        if changed {
            cell["text_state"] = json!("not_examined");
            conflicts.push(json!({"kind":"cell","id":cell["id"],"reason":"source_cells_changed"}));
        } else {
            cell["text_state"] = state["text_state"].clone();
        }
    }
    result["elements"] = json!(elements);

    let mut visuals = result["visuals"].as_array().unwrap().clone();
    let carried_visuals = rebase_visuals(
        &mut visuals,
        array(&journal["visuals"])?,
        current,
        same_source,
        &region_ids,
        &mut conflicts,
    )?;
    result["visuals"] = json!(visuals);
    result["review"] = json!({"status":"pending"});
    if same_source && journal["baseline_extraction_hash"] == extraction_hash {
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
    let validation = validate_hashed(root, current, extraction_hash, &result, original)?;
    let mut report = json!({"state":if validation["ready"] == true {"reviewed"} else {"needs_review"},
        "ready":validation["ready"],"carried_elements":retained.len(),
        "carried_visuals":carried_visuals,"conflicts":conflicts});
    // How apply carried this version, while it is the version replayed.
    if same_source
        && journal["baseline_extraction_hash"] == extraction_hash
        && let Some(carried) = journal.get("carried")
    {
        report["carried"] = carried.clone();
    }
    Ok((result, report))
}

#[cfg(test)]
mod performance_tests {
    use super::*;
    use std::{hint::black_box, time::Instant};

    #[test]
    #[ignore = "manual performance measurement"]
    fn benchmark_holding_lookup() {
        let elements: Vec<_> = (0..300)
            .map(|element| {
                json!({"sheet":"Sheet","cells":(0..10)
                .map(|cell| json!({"address":format!("C{}", element * 10 + cell)}))
                .collect::<Vec<_>>() })
            })
            .collect();
        let addresses: Vec<_> = (0..3000).map(|i| format!("C{i}")).collect();
        let mut old = vec![];
        let mut new = vec![];
        for _ in 0..5 {
            let started = Instant::now();
            let found: Vec<_> = addresses
                .iter()
                .map(|address| {
                    elements.iter().position(|element| {
                        element["sheet"] == "Sheet"
                            && array(&element["cells"]).is_ok_and(|cells| {
                                cells.iter().any(|cell| cell["address"] == *address)
                            })
                    })
                })
                .collect();
            old.push(started.elapsed().as_secs_f64() * 1000.0);
            let started = Instant::now();
            let index = holding_index(&elements);
            let indexed: Vec<_> = addresses
                .iter()
                .map(|address| {
                    index
                        .get("Sheet")
                        .and_then(|cells| cells.get(address))
                        .map(|(element, _)| *element)
                })
                .collect();
            new.push(started.elapsed().as_secs_f64() * 1000.0);
            assert_eq!(black_box(found), black_box(indexed));
        }
        old.sort_by(f64::total_cmp);
        new.sort_by(f64::total_cmp);
        eprintln!(
            "holding_lookup_old_ms={:.2} indexed_ms={:.2}",
            old[2], new[2]
        );
    }
}

fn rebase_visuals(
    visuals: &mut [Value],
    corrections: &[Value],
    current: &Value,
    same_source: bool,
    region_ids: &BTreeSet<String>,
    conflicts: &mut Vec<Value>,
) -> Result<usize> {
    if corrections.is_empty() {
        return Ok(0);
    }
    // Serialized keys preserve exact JSON equality, including source order.
    let key = |visual: &Value| {
        serde_json::to_vec(&(&visual["id"], &visual["sources"]))
            .expect("JSON values can be serialized")
    };
    let mut indices = BTreeMap::new();
    for (i, visual) in visuals.iter().enumerate() {
        indices.entry(key(visual)).or_insert(i);
    }
    let mut carried_visuals = 0;
    for correction in corrections {
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
            && (same_source || !uses_region(visual, region_ids)?)
            && let Some(&index) = indices.get(&key(visual))
        {
            visuals[index] = visual.clone();
            carried_visuals += 1;
        } else {
            conflicts.push(json!({"kind":"visual","id":visual["id"],
                "reason":"source_or_image_evidence_changed"}));
        }
    }
    Ok(carried_visuals)
}

#[cfg(test)]
mod visual_measurement {
    use super::*;

    #[test]
    fn visual_index_preserves_first_match_and_source_order() {
        let first = json!({"id":"v","sources":["/a","/b"],"label":"first"});
        let reversed = json!({"id":"v","sources":["/b","/a"],"label":"reversed"});
        let mut visuals = vec![first.clone(), first.clone(), reversed.clone()];
        let mut updated = first.clone();
        updated["label"] = json!("updated");
        let corrections = vec![
            json!({"visual":updated,"sources":[]}),
            json!({"visual":{"id":"missing","sources":[]},"sources":[]}),
            json!({"visual":reversed,"sources":[{"pointer":"/absent","sha256":"stale"}]}),
        ];
        let mut conflicts = vec![];
        assert_eq!(
            rebase_visuals(
                &mut visuals,
                &corrections,
                &json!({}),
                true,
                &BTreeSet::new(),
                &mut conflicts
            )
            .unwrap(),
            1
        );
        assert_eq!(visuals, vec![updated, first, reversed]);
        assert_eq!(conflicts.len(), 2);
        assert_eq!(conflicts[0]["id"], "missing");
        assert_eq!(conflicts[1]["id"], "v");
    }

    #[test]
    #[ignore = "manual visual correction performance measurement"]
    fn measure_visual_corrections() {
        for count in [500, 2_000] {
            let visuals: Vec<_> = (0..count).map(|i| json!({"id":format!("v{i}"),"sources":[format!("/drawing/{i}")],"evidence":[]})).collect();
            let current = json!({"anchor":"original"});
            let corrections: Vec<_> = visuals.iter().map(|visual| json!({"visual":visual,"sources":[{"pointer":"/anchor","sha256":hash(&encoded(&current["anchor"]))}]})).collect();
            let mut times = vec![];
            for _ in 0..5 {
                let mut actual = visuals.clone();
                let mut conflicts = vec![];
                let start = std::time::Instant::now();
                let carried = rebase_visuals(
                    &mut actual,
                    &corrections,
                    &current,
                    true,
                    &BTreeSet::new(),
                    &mut conflicts,
                )
                .unwrap();
                times.push(start.elapsed().as_secs_f64() * 1000.0);
                assert_eq!(carried, count);
                assert_eq!(actual, visuals);
                assert!(conflicts.is_empty());
            }
            times.sort_by(f64::total_cmp);
            eprintln!("visual_corrections count={count} median_ms={:.3}", times[2]);
        }
    }
}
