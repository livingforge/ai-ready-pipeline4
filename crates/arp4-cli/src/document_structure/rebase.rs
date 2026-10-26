use super::corrections::SourceCell;
use super::grid::Axes;
use super::*;
use std::collections::HashMap;

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

fn holding_index(elements: &[Element]) -> BTreeMap<String, BTreeMap<String, (usize, usize)>> {
    let mut holding: BTreeMap<String, BTreeMap<String, (usize, usize)>> = BTreeMap::new();
    for (element_index, element) in elements.iter().enumerate() {
        let Some(sheet) = element["sheet"].as_str() else {
            continue;
        };
        for (cell_index, cell) in element.cells.iter().enumerate() {
            // The first element wins when inferred elements overlap.
            holding
                .entry(sheet.to_owned())
                .or_default()
                .entry(cell.address.to_string())
                .or_insert((element_index, cell_index));
        }
    }
    holding
}

/// Replays `journal` on the extraction `current`, whose cells it names by the
/// identities of their rows and columns in `axes`. `original` requires the
/// original to be still the one `current` was extracted from.
pub(super) fn rebase(
    root: &Path,
    journal: &Value,
    current: &Extraction,
    extraction_hash: &str,
    original: bool,
    axes: &Axes,
) -> Result<(Structure, Value)> {
    corrections::validate_journal(journal)?;
    let same_source = journal["source_sha256"] == current["source"]["sha256"];
    // Validated once below, after the corrections are applied.
    let mut result = inferred(root, current, extraction_hash)?;
    // The cells of each sheet by address, which only corrections of
    // elements, readings and cell states read: a large sheet has many.
    let corrected = ["elements", "readings", "cell_states"]
        .into_iter()
        .any(|list| {
            journal[list]
                .as_array()
                .is_some_and(|list| !list.is_empty())
        });
    let sheets = if corrected {
        corrections::sheets(current)?
    } else {
        HashMap::new()
    };
    // What the parser gives each cell, read only to replay corrections of
    // elements: a large sheet has a default per cell.
    let defaults = if array(&journal["elements"])?.is_empty() {
        corrections::Defaults::default()
    } else {
        corrections::Defaults::of(&result.elements)
    };
    let region_ids: BTreeSet<String> = array(&journal["regions"])?
        .iter()
        .map(|region| string(&region["id"]).map(str::to_owned))
        .collect::<Result<_>>()?;
    // The IDs the corrections give cells, which the context of a parent names.
    let mut ids = HashMap::new();
    for correction in array(&journal["elements"])? {
        let name = string(&correction["element"]["sheet"])?;
        for change in correction["changes"].as_array().into_iter().flatten() {
            if let (Some(id), Ok(point)) =
                (change.get("id"), axes.point(name, string(&change["cell"])?))
            {
                ids.insert((name.to_owned(), corrections::address(point)?), id.clone());
            }
        }
    }
    let mut conflicts = Vec::new();
    let mut candidates = BTreeMap::new();
    let mut claimed = BTreeSet::new();
    for correction in array(&journal["elements"])? {
        let id = string(&correction["element"]["id"])?.to_owned();
        let name = string(&correction["element"]["sheet"])?;
        let replayed = match sheets.get(name) {
            Some(sheet) => corrections::replayed(correction, sheet, &defaults, &ids, axes)?
                .map(|(element, matched)| {
                    Ok::<_, anyhow::Error>((Element::from_value(element)?, matched))
                })
                .transpose()?,
            None => None,
        };
        // A cell another correction holds is missing to this one.
        let Some((mut element, matched)) = replayed.filter(|(element, _)| {
            element
                .cells
                .iter()
                .all(|cell| !claimed.contains(&(name.to_owned(), cell.address.to_string())))
        }) else {
            conflicts.push(json!({"kind":"element","id":id,"reason":"source_cells_missing"}));
            continue;
        };
        if !same_source && !array(&element["evidence"])?.is_empty() {
            conflicts.push(json!({"kind":"element","id":id,
                "reason":"image_evidence_requires_new_source_review"}));
            continue;
        }
        let cells = &mut element.cells;
        claimed.extend(
            cells
                .iter()
                .map(|cell| (name.to_owned(), cell.address.to_string())),
        );
        // Its cells, or what the parser gives them, changed since it was
        // recorded: each cell is read again, and keeps only headers that are
        // still headers of the element.
        let mut changed = Vec::new();
        if !matched {
            let headers: BTreeSet<Box<str>> = cells
                .iter()
                .filter(|cell| cell.role.heads())
                .map(|cell| cell.id.clone())
                .collect();
            for cell in cells.iter_mut() {
                cell.text_state = TextState::NotExamined;
                cell.headers.retain(|header| headers.contains(header));
                changed.push(json!(&*cell.address));
            }
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

    if same_source {
        result["regions"] = journal["regions"].clone();
    }
    let retained: Vec<Element> = candidates
        .into_iter()
        .filter(|(id, _)| transferable.contains(id))
        .map(|(_, element)| element)
        .collect();
    // The addresses the corrected elements hold, by sheet.
    let mut owned: HashMap<String, BTreeSet<String>> = HashMap::new();
    for element in &retained {
        owned
            .entry(string(&element["sheet"])?.to_owned())
            .or_default()
            .extend(element.cells.iter().map(|cell| cell.address.to_string()));
    }
    let mut fresh = Vec::new();
    let mut renamed: BTreeMap<String, Value> = BTreeMap::new();
    let mut used_ids: BTreeSet<String> = retained
        .iter()
        .map(|element| element["id"].as_str().unwrap().to_owned())
        .collect();
    // The inferred elements are moved, not copied: there is one per table and
    // text of the extraction, each with its cells.
    for mut element in std::mem::take(&mut result.elements) {
        if let Some(held) = owned.get(string(&element["sheet"])?) {
            element.cells.retain(|cell| !held.contains(&*cell.address));
        }
        if element.cells.is_empty() {
            continue;
        }
        // Headers outside the cells left go; the cells are only read when
        // some header is.
        let outside = {
            let local: HashSet<&str> = element.cells.iter().map(|cell| &*cell.id).collect();
            element
                .cells
                .iter()
                .any(|cell| cell.headers.iter().any(|h| !local.contains(&**h)))
        };
        if outside {
            let local: HashSet<Box<str>> =
                element.cells.iter().map(|cell| cell.id.clone()).collect();
            for cell in &mut element.cells {
                cell.headers.retain(|header| local.contains(header));
            }
        }
        let inferred_id = string(&element["id"])?.to_owned();
        element["id"] = json!(fresh_id(&mut used_ids, &inferred_id));
        renamed.insert(inferred_id, element["id"].clone());
        fresh.push(element);
    }
    // A parser table nested in another parser table follows that table's new ID.
    for element in &mut fresh {
        if let Some(parent) = element.get_mut("parent")
            && let Some(id) = parent["element"].as_str().and_then(|id| renamed.get(id))
        {
            parent["element"] = id.clone();
        }
    }
    let corrected: BTreeSet<String> = retained
        .iter()
        .map(|element| element["id"].as_str().unwrap().to_owned())
        .collect();
    let carried_elements = retained.len();
    let mut elements: Vec<Element> = retained.into_iter().chain(fresh).collect();
    for id in relations::prune_parents(&mut elements) {
        if corrected.contains(&id) {
            conflicts.push(json!({"kind":"element","id":id,"reason":"parent_dependency_changed"}));
        }
    }

    // A reading of an element the parser got right goes to the parser's
    // element holding its anchor cell, while that cell is what was read.
    let holding =
        if array(&journal["readings"])?.is_empty() && array(&journal["cell_states"])?.is_empty() {
            BTreeMap::new()
        } else {
            holding_index(&elements)
        };
    // The address and source cell a cell name of the journal names now.
    let source = |sheet: &str, cell: &Value| -> Result<Option<(String, Option<SourceCell<'_>>)>> {
        let Ok(point) = axes.point(sheet, string(cell)?) else {
            return Ok(None);
        };
        let address = corrections::address(point)?;
        let now = sheets.get(sheet).and_then(|s| s.source(&address));
        Ok(Some((address, now)))
    };
    for reading in array(&journal["readings"])? {
        let sheet = string(&reading["sheet"])?;
        let (address, now) = source(sheet, &reading["anchor"])?.unwrap_or_default();
        let reason = match holding
            .get(sheet)
            .and_then(|cells| cells.get(&address))
            .map(|(index, _)| *index)
        {
            None => Some("source_cells_missing"),
            Some(index) if corrected.contains(elements[index]["id"].as_str().unwrap_or("")) => {
                Some("anchor_in_corrected_element")
            }
            Some(_) if corrections::cell_hash(now)? != reading["sha256"] => {
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
        let Some((address, now)) = source(sheet, &state["cell"])? else {
            continue;
        };
        let Some(&(index, cell_index)) = holding.get(sheet).and_then(|cells| cells.get(&address))
        else {
            continue;
        };
        let changed = corrections::cell_hash(now)? != state["sha256"];
        let cell = &mut elements[index].cells[cell_index];
        if changed {
            cell.text_state = TextState::NotExamined;
            conflicts.push(json!({"kind":"cell","id":&*cell.id,"reason":"source_cells_changed"}));
        } else {
            cell.text_state = TextState::parse(string(&state["text_state"])?)?;
        }
    }
    result.elements = elements;

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
        result.elements.sort_by_key(|element| {
            order
                .get(element["id"].as_str().unwrap_or(""))
                .copied()
                .unwrap_or(usize::MAX)
        });
        if journal["review"]["content_hash"] == content_hash(&result) {
            result["review"] = journal["review"].clone();
        }
    }
    let ready = ready_hashed(root, current, extraction_hash, &result, original)?;
    let mut report = json!({"state":if ready {"reviewed"} else {"needs_review"},
        "ready":ready,"carried_elements":carried_elements,
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
        let typed: Vec<Element> = elements
            .iter()
            .map(|element| {
                let cells = array(&element["cells"])
                    .unwrap()
                    .iter()
                    .map(|cell| {
                        let address = cell["address"].as_str().unwrap();
                        ElementCell {
                            address: address.into(),
                            headers: vec![],
                            id: address.into(),
                            role: Role::Data,
                            text_state: TextState::Read,
                        }
                    })
                    .collect();
                Element::new(json!({"sheet":"Sheet"}), cells)
            })
            .collect();
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
            let index = holding_index(&typed);
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
    current: &Extraction,
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
                &Extraction::from_value(json!({"sheets":[]})).unwrap(),
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
            let current = Extraction::from_value(json!({"anchor":"original","sheets":[]})).unwrap();
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
