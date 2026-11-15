//! Originals moved to other document IDs, and the records citing them.
//!
//! A source's ID is the hash of its document's ID and its place in the
//! extraction, so a document moved to another ID has other source IDs. The
//! snapshots holding it are made again as a capture of the same extraction
//! at the new ID would make them, and the records citing them follow with
//! their approvals: the document, its version and every text they quote are
//! the same.
use super::*;

/// Where a document moved: its new ID and its original's project path.
pub struct Moved {
    pub to: String,
    pub path: String,
}

/// The registry `r` after the documents `moved`, by their earlier IDs, have
/// moved, as one revision by `actor` for `reason`: each snapshot holding one
/// of them is replaced by the same capture of it at its new ID, and the
/// records citing the snapshot cite the new one, the sources they quote by
/// their new IDs. A snapshot the judgments of an archive name stays as well,
/// as the record of what they were made on. None when no snapshot holds one
/// of the documents.
pub fn relocate(
    mut r: Registry,
    moved: &BTreeMap<String, Moved>,
    actor: &str,
    reason: &str,
) -> Result<Option<Registry>> {
    nonempty(actor)?;
    nonempty(reason)?;
    let parent = fingerprint(&r)?;
    // Each replaced snapshot by its key, the new one's key and contents.
    let mut replaced: BTreeMap<String, String> = BTreeMap::new();
    let mut added: BTreeMap<String, Value> = BTreeMap::new();
    // Each source of a moved document by its earlier ID: its new ID.
    let mut ids: BTreeMap<String, String> = BTreeMap::new();
    let mut documents: BTreeMap<String, String> = BTreeMap::new();
    for (key, value) in &r.inputs {
        let mut input = spec::Input::deserialize(value)?;
        let held: BTreeSet<&str> = input
            .revisions
            .keys()
            .map(String::as_str)
            .chain(input.sources.iter().map(|s| s.document.as_str()))
            .filter(|document| moved.contains_key(*document))
            .collect();
        if held.is_empty() {
            continue;
        }
        for target in moved.values() {
            ensure!(
                !input.revisions.contains_key(&target.to)
                    && input.sources.iter().all(|s| s.document != target.to),
                "the registry snapshot {key} already holds a document at {}; retire the records citing it before moving another document there",
                target.to
            );
        }
        for document in held {
            documents.insert(document.to_owned(), moved[document].to.clone());
        }
        relocate_input(&mut input, moved, key, &mut ids)?;
        let value = serde_json::to_value(&input)?;
        let new = hash(&encoded(&value));
        replaced.insert(key.clone(), new.clone());
        added.insert(new, value);
    }
    if replaced.is_empty() {
        return Ok(None);
    }
    let mut repointed = vec![];
    for (id, entry) in r.entries.iter_mut() {
        if !entry
            .evidence
            .iter()
            .any(|ev| replaced.contains_key(&ev.snapshot))
        {
            continue;
        }
        let mut value = serde_json::to_value(&*entry)?;
        for ev in value["evidence"].as_array_mut().into_iter().flatten() {
            if let Some(new) = ev["snapshot"].as_str().and_then(|key| replaced.get(key)) {
                ev["snapshot"] = json!(new);
            }
        }
        rename_spans(&mut value, &ids);
        *entry = serde_json::from_value(value)?;
        repointed.push(id.clone());
    }
    for (key, value) in added {
        r.manifest.evidence.insert(key.clone());
        r.inputs.insert(key, value);
    }
    r.manifest.revision += 1;
    r.manifest.parent = Some(parent.clone());
    r.manifest.last_change = ChangeRecord {
        revision: r.manifest.revision,
        parent: Some(parent),
        actor: actor.into(),
        reason: reason.into(),
        changed: vec![],
        impacted: vec![],
        approved: vec![],
        aliases: BTreeMap::new(),
        resolved_issues: BTreeMap::new(),
        repointed,
        moved_documents: documents,
    };
    refresh(&mut r)?;
    validate(&r)?;
    Ok(Some(r))
}

/// Makes the snapshot `input`, filed under `key`, the capture of its moved
/// documents at their new IDs, adding the new ID of each of their sources to
/// `ids`. Its sources stay in their order within each document, and the
/// documents in the order of their IDs, as capture gives them.
fn relocate_input(
    input: &mut spec::Input,
    moved: &BTreeMap<String, Moved>,
    key: &str,
    ids: &mut BTreeMap<String, String>,
) -> Result<()> {
    fn rename<T>(map: &mut BTreeMap<String, T>, moved: &BTreeMap<String, Moved>) {
        let held: Vec<String> = map
            .keys()
            .filter(|document| moved.contains_key(*document))
            .cloned()
            .collect();
        for document in held {
            let value = map.remove(&document).unwrap();
            map.insert(moved[&document].to.clone(), value);
        }
    }
    rename(&mut input.revisions, moved);
    rename(&mut input.structure_requirements, moved);
    rename(&mut input.merges, moved);
    rename(&mut input.structures, moved);
    for (document, structure) in input.structures.iter_mut() {
        if let Some(target) = moved.values().find(|target| &target.to == document) {
            crate::document_structure::relocate(structure, &target.to, &target.path);
        }
    }
    for warning in &mut input.warnings {
        for (document, target) in moved {
            if let Some(rest) = warning.strip_prefix(&format!("{document}: ")) {
                *warning = format!("{}: {rest}", target.to);
                break;
            }
        }
    }
    for source in &mut input.sources {
        let Some(target) = moved.get(&source.document) else {
            continue;
        };
        ensure!(
            source.id == hash(&encoded(&json!([source.document, source.location]))),
            "source {} of the registry snapshot {key} is not named by its document and place, so it cannot follow the move",
            source.id
        );
        let id = hash(&encoded(&json!([target.to, source.location])));
        ids.insert(source.id.clone(), id.clone());
        source.document = target.to.clone();
        source.id = id;
    }
    input.sources.sort_by(|a, b| a.document.cmp(&b.document));
    Ok(())
}

/// Names each source the spans in `value` quote by its new ID in `ids`. A
/// span is an object with the `source` it quotes and the `quote`.
fn rename_spans(value: &mut Value, ids: &BTreeMap<String, String>) {
    match value {
        Value::Object(members) => {
            if members.contains_key("quote")
                && let Some(new) = members
                    .get("source")
                    .and_then(Value::as_str)
                    .and_then(|source| ids.get(source))
            {
                members.insert("source".into(), json!(new));
            }
            for member in members.values_mut() {
                rename_spans(member, ids);
            }
        }
        Value::Array(items) => {
            for item in items {
                rename_spans(item, ids);
            }
        }
        _ => {}
    }
}

/// The records not retired that quote a source of one of `documents`, by
/// ID, with the documents each quotes.
pub fn citing(
    r: &Registry,
    documents: &BTreeSet<String>,
) -> Result<BTreeMap<String, BTreeSet<String>>> {
    // The sources of the documents in each snapshot, by ID.
    let mut snapshots: BTreeMap<&str, BTreeMap<String, String>> = BTreeMap::new();
    let mut citing: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (id, entry) in &r.entries {
        if entry.status == Status::Retired {
            continue;
        }
        let quoted = cited(entry);
        for ev in &entry.evidence {
            let sources = match snapshots.entry(ev.snapshot.as_str()) {
                std::collections::btree_map::Entry::Occupied(held) => held.into_mut(),
                std::collections::btree_map::Entry::Vacant(slot) => {
                    let input = r
                        .inputs
                        .get(&ev.snapshot)
                        .context("unknown evidence snapshot")?;
                    let mut held = BTreeMap::new();
                    for source in input["sources"].as_array().into_iter().flatten() {
                        if let (Some(source_id), Some(document)) =
                            (source["id"].as_str(), source["document"].as_str())
                            && documents.contains(document)
                        {
                            held.insert(source_id.to_owned(), document.to_owned());
                        }
                    }
                    slot.insert(held)
                }
            };
            for source in &quoted {
                if let Some(document) = sources.get(*source) {
                    citing
                        .entry(id.clone())
                        .or_default()
                        .insert(document.clone());
                }
            }
        }
    }
    Ok(citing)
}
