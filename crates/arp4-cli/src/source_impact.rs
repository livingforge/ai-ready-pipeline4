//! Conservative correspondence and review impact; never rewrites evidence or approvals.
use crate::{
    data::{array, encoded, hash, string},
    registry::Registry,
};
use anyhow::Result;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

struct Unit {
    id: String,
    text: String,
    scope: String,
    kind: String,
    position: Value,
}

fn units(extraction: &Value) -> Result<Vec<Unit>> {
    let mut units = vec![];
    for (si, sheet) in array(&extraction["sheets"])?.iter().enumerate() {
        for (ci, cell) in array(&sheet["cells"])?.iter().enumerate() {
            let text = string(&cell["value"])?;
            if text.trim().is_empty() {
                continue;
            }
            let position = &cell["position"];
            let kind = string(&position["kind"])?;
            units.push(Unit {
                id: hash(&encoded(&json!([
                    extraction["document_id"],
                    format!("/sheets/{si}/cells/{ci}/value")
                ]))),
                text: text.into(),
                kind: kind.into(),
                scope: serde_json::to_string(&json!([
                    kind,
                    position["headings"],
                    position["columns"],
                    cell["address"]
                        .as_str()
                        .unwrap_or("")
                        .trim_end_matches(|c: char| c.is_ascii_digit())
                ]))?,
                position: position.clone(),
            });
        }
    }
    Ok(units)
}

fn describe(unit: &Unit) -> Value {
    json!({"source":unit.id,"text":unit.text,"position":unit.position})
}

fn change(kind: &str, old: &[&Unit], new: &[&Unit], confirmed: bool) -> Value {
    json!({"path":format!("/sources/{}",old.first().or(new.first()).unwrap().id),
        "kind":kind,"before":old.iter().map(|u| describe(u)).collect::<Vec<_>>(),
        "after":new.iter().map(|u| describe(u)).collect::<Vec<_>>(),
        "correspondence":if confirmed {"confirmed"} else {"requires_confirmation"},
        "review_required":kind != "moved","affected_entries":[]})
}

pub fn compare(before: &Value, after: &Value, registry: Option<&Registry>) -> Result<Value> {
    let old = units(before)?;
    let new = units(after)?;
    let mut matched_before = BTreeSet::new();
    let mut matched_after = BTreeSet::new();
    let mut pairs = vec![];
    let mut changes = vec![];
    if before["source"]["sha256"] != after["source"]["sha256"] {
        // Each stage narrows ambiguity; changed text is only a proposed correspondence.
        for stage in 0..3 {
            let key = |u: &Unit| match stage {
                0 => json!([u.scope, u.text]).to_string(),
                1 => json!([u.kind, u.text]).to_string(),
                _ => u.scope.clone(),
            };
            let mut left: BTreeMap<String, Vec<usize>> = BTreeMap::new();
            let mut right: BTreeMap<String, Vec<usize>> = BTreeMap::new();
            for (i, u) in old
                .iter()
                .enumerate()
                .filter(|(i, _)| !matched_before.contains(i))
            {
                left.entry(key(u)).or_default().push(i);
            }
            for (i, u) in new
                .iter()
                .enumerate()
                .filter(|(i, _)| !matched_after.contains(i))
            {
                right.entry(key(u)).or_default().push(i);
            }
            for (key, a) in left {
                let Some(b) = right.get(&key) else {
                    continue;
                };
                matched_before.extend(a.iter().copied());
                matched_after.extend(b.iter().copied());
                if a.len() == 1 && b.len() == 1 {
                    if stage == 0 {
                        pairs.push((a[0], b[0]));
                    } else {
                        changes.push(change(
                            if stage == 1 {
                                "context_changed"
                            } else {
                                "modified_candidate"
                            },
                            &[&old[a[0]]],
                            &[&new[b[0]]],
                            stage == 1,
                        ));
                    }
                } else {
                    changes.push(change(
                        "ambiguous",
                        &a.iter().map(|i| &old[*i]).collect::<Vec<_>>(),
                        &b.iter().map(|i| &new[*i]).collect::<Vec<_>>(),
                        false,
                    ));
                }
            }
        }
        pairs.sort();
        let mut order = pairs.clone();
        order.sort_by_key(|(_, j)| *j);
        for (rank, (i, j)) in pairs.iter().enumerate() {
            let reordered = order[rank] != (*i, *j);
            if reordered || old[*i].id != new[*j].id || old[*i].position != new[*j].position {
                changes.push(change(
                    if reordered { "reordered" } else { "moved" },
                    &[&old[*i]],
                    &[&new[*j]],
                    true,
                ));
            }
        }
        for (_, u) in old
            .iter()
            .enumerate()
            .filter(|(i, _)| !matched_before.contains(i))
        {
            changes.push(change("removed", &[u], &[], true));
        }
        for (_, u) in new
            .iter()
            .enumerate()
            .filter(|(i, _)| !matched_after.contains(i))
        {
            changes.push(change("added", &[], &[u], true));
        }
    }
    if let Some(registry) = registry {
        attach_entries(&mut changes, &old, before, registry)?;
    }
    Ok(json!({"kind":"source_impact","changes":changes,
        "policy":"Correspondence is a review aid. Evidence IDs, snapshots and approvals are never rewritten or transferred automatically."}))
}

fn attach_entries(
    changes: &mut Vec<Value>,
    old: &[Unit],
    before: &Value,
    registry: &Registry,
) -> Result<()> {
    let revision = hash(&encoded(before));
    let document = string(&before["document_id"])?;
    let mut references: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut stale = BTreeSet::new();
    for (id, entry) in &registry.entries {
        if matches!(entry.status, crate::registry::Status::Retired) {
            continue;
        }
        for evidence in &entry.evidence {
            let input = &registry.inputs[&evidence.snapshot];
            if array(&input["sources"])?
                .iter()
                .any(|s| s["id"] == evidence.span.source && s["document"] == document)
            {
                if input["revisions"][document] == revision {
                    references
                        .entry(evidence.span.source.clone())
                        .or_default()
                        .insert(id.clone());
                } else {
                    stale.insert(id.clone());
                }
            }
        }
    }
    if !stale.is_empty() {
        changes.push(json!({"path":"/registry/evidence","kind":"stale_evidence","before":[],"after":[],
            "review_required":true,"correspondence":"requires_confirmation","affected_entries":stale}));
    }
    for change in changes {
        let mut affected: BTreeSet<String> = if change["kind"] == "stale_evidence" {
            array(&change["affected_entries"])?
                .iter()
                .map(|v| Ok(string(v)?.to_owned()))
                .collect::<Result<_>>()?
        } else {
            BTreeSet::new()
        };
        let mut ids: BTreeSet<String> = array(&change["before"])?
            .iter()
            .map(|u| Ok(string(&u["source"])?.to_owned()))
            .collect::<Result<_>>()?;
        if change["review_required"] == true {
            // New or changed content also calls for review of claims in the same section.
            for unit in old {
                if array(&change["after"])?
                    .iter()
                    .chain(array(&change["before"])?)
                    .any(|c| {
                        unit.position["headings"].as_array().is_some_and(|h| {
                            c["position"]["headings"]
                                .as_array()
                                .is_some_and(|scope| h.starts_with(scope))
                        })
                    })
                {
                    ids.insert(unit.id.clone());
                }
            }
        }
        for id in ids {
            if let Some(entries) = references.get(&id) {
                affected.extend(entries.iter().cloned());
            }
        }
        let direct = affected.clone();
        loop {
            let size = affected.len();
            for (id, entry) in &registry.entries {
                if !matches!(entry.status, crate::registry::Status::Retired)
                    && entry
                        .requirements
                        .iter()
                        .chain(&entry.related)
                        .any(|r| affected.contains(r))
                {
                    affected.insert(id.clone());
                }
            }
            if size == affected.len() {
                break;
            }
        }
        change["affected_entries"] = json!(affected);
        change["direct_entries"] = json!(direct);
    }
    Ok(())
}
