//! Repair confirmation with explicit reuse of unchanged source audits.
use super::*;

impl Workflow {
    pub(super) fn previous_review_model(
        &self,
        root: &Path,
        input: &Path,
    ) -> Result<Option<(crate::specifications::Model, crate::registry::Catalog)>> {
        let Some(replies) = self
            .state
            .reviewed_replies
            .as_object()
            .filter(|r| !r.is_empty())
        else {
            return Ok(None);
        };
        let mut paths = Vec::new();
        for (i, digest) in replies.values().enumerate() {
            let path = root.join(format!("previous-reply-{i}.json"));
            self.store.materialize(s(digest)?, &path)?;
            paths.push(path);
        }
        let output = root.join("previous-assembled");
        crate::semantic_operations::assemble(input, &paths, "previous-review", &output)?;
        Ok(Some((
            crate::specifications::load_model(&output.join("model.json"))?,
            serde_json::from_value(read(&output.join("catalog.json"))?)?,
        )))
    }

    pub(super) fn confirmation_context(&self, task: &Task) -> Result<Value> {
        if !task.stage.is_review() || task.context.is_none() {
            return Ok(Value::Null);
        }
        Ok(self.artifact(&task.context)?["confirmation"].clone())
    }

    /// The provider only declares fresh audits. Carried audits are attached by
    /// the CLI after validation and retain their original rationale/provenance.
    pub(super) fn confirmed_review(&self, task: &Task, mut reply: Value) -> Result<Value> {
        let confirmation = self.confirmation_context(task)?;
        if let Some(carried) = confirmation["carried_audits"].as_array() {
            reply["audits"]
                .as_array_mut()
                .context("audits missing")?
                .extend(carried.clone());
        }
        Ok(reply)
    }
}

fn mentions(value: &Value, id: &str) -> bool {
    match value {
        Value::String(v) => v == id,
        Value::Array(a) => a.iter().any(|v| mentions(v, id)),
        Value::Object(o) => o.values().any(|v| mentions(v, id)),
        _ => false,
    }
}

pub(super) fn plan(previous: &Value, current: &Value, review: &Value) -> Result<Value> {
    let index = |packet: &Value| -> Result<BTreeMap<String, Value>> {
        arr(&packet["items"])?
            .iter()
            .map(|item| Ok((s(&item["id"])?.to_owned(), item.clone())))
            .collect()
    };
    let before = index(previous)?;
    let after = index(current)?;
    let ids: BTreeSet<_> = before.keys().chain(after.keys()).cloned().collect();
    let changed: BTreeSet<_> = ids
        .iter()
        .filter(|id| before.get(*id) != after.get(*id))
        .cloned()
        .collect();
    let mut affected = changed.clone();
    for finding in arr(&review["findings"])? {
        if finding["action"] != "improvement" {
            affected.extend(
                arr(&finding["items"])?
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned),
            );
        }
    }
    loop {
        let prior = affected.clone();
        for (id, item) in before.iter().chain(after.iter()) {
            for other in &ids {
                if (prior.contains(id) || prior.contains(other)) && mentions(item, other) {
                    affected.insert(id.clone());
                    affected.insert(other.clone());
                }
            }
        }
        if prior == affected {
            break;
        }
    }
    let metadata = |packet: &Value| {
        let mut metadata = packet.clone();
        for key in ["packet", "items", "risk_summary", "evidence_clusters"] {
            metadata.as_object_mut().unwrap().remove(key);
        }
        metadata
    };
    let shared_context_changed = metadata(previous) != metadata(current);
    let shared_changes: Vec<_> = obj(current)?
        .keys()
        .chain(obj(previous)?.keys())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|key| {
            !["packet", "items", "risk_summary", "evidence_clusters"].contains(&key.as_str())
                && previous[*key] != current[*key]
        })
        .map(|key| json!({"field":key,"before":previous[key],"after":current[key]}))
        .collect();
    let all: BTreeSet<String> = current["sources"]["rows"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| r[0].as_str().map(str::to_owned))
        .collect();
    let mut required = BTreeSet::new();
    // Shared metadata includes originals, headings, exclusions, external targets,
    // modules and issues. Any change conservatively invalidates all source audits.
    let full_review = shared_context_changed
        || arr(&review["findings"])?.iter().any(|f| {
            f["action"] != "improvement" && f["items"].as_array().is_none_or(Vec::is_empty)
        });
    if full_review {
        required = all.clone();
    } else {
        for packet in [previous, current] {
            for cluster in arr(&packet["evidence_clusters"])? {
                if arr(&cluster["items"])?
                    .iter()
                    .any(|id| id.as_str().is_some_and(|id| affected.contains(id)))
                {
                    required.extend(
                        arr(&cluster["sources"])?
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned),
                    );
                }
            }
        }
        required = required.intersection(&all).cloned().collect();
    }
    let mut carried = Vec::new();
    let mut covered = BTreeSet::new();
    for audit in arr(&review["audits"])? {
        let sources: Vec<_> = arr(&audit["sources"])?
            .iter()
            .filter_map(Value::as_str)
            .filter(|source| all.contains(*source) && !required.contains(*source))
            .map(str::to_owned)
            .collect();
        if !sources.is_empty() {
            covered.extend(sources.clone());
            carried.push(json!({"sources":sources,"rationale":format!("Carried from review {}: {}", s(&review["packet"])?, s(&audit["rationale"])?) }));
        }
    }
    required.extend(all.difference(&covered).cloned());
    Ok(
        json!({"mode":"repair_confirmation","previous_packet":review["packet"],
        "required_sources":required,"carried_audits":carried,"previous_findings":review["findings"],"full_review":full_review,
        "changed_items":changed.iter().map(|id| json!({"id":id,"before":before.get(id),"after":after.get(id)})).collect::<Vec<_>>(),
        "affected_items":affected,"shared_changes":shared_changes}),
    )
}

/// Reduce delivery only. The stored packet and its identity remain authoritative
/// for validation. Shared changes and unscoped defects require the full packet.
pub(super) fn delivery(packet: &Value, confirmation: &Value) -> Result<Value> {
    if confirmation.is_null() || confirmation["full_review"] == true {
        return Ok(packet.clone());
    }
    fn expand(value: &Value, spans: &[Value]) -> Value {
        match value {
            Value::Object(o) if o.len() == 1 && o.contains_key("span_ref") => {
                let row = &spans[value["span_ref"].as_u64().unwrap() as usize];
                json!({"source":row[0],"start":row[1],"end":row[2],"quote":row[3]})
            }
            Value::Object(o) => Value::Object(
                o.iter()
                    .map(|(k, v)| (k.clone(), expand(v, spans)))
                    .collect(),
            ),
            Value::Array(a) => json!(a.iter().map(|v| expand(v, spans)).collect::<Vec<_>>()),
            _ => value.clone(),
        }
    }
    let mut view = expand(packet, arr(&packet["span_table"]["rows"])?);
    view.as_object_mut().unwrap().remove("span_table");
    view.as_object_mut().unwrap().remove("encoding");
    if let Some(columns) = view["items"]["columns"].as_array() {
        view["items"] = json!(
            arr(&view["items"]["rows"])?
                .iter()
                .map(|row| {
                    Value::Object(
                        columns
                            .iter()
                            .zip(row.as_array().unwrap())
                            .map(|(k, v)| (k.as_str().unwrap().to_owned(), v.clone()))
                            .collect(),
                    )
                })
                .collect::<Vec<_>>()
        );
    }
    let mut ids: BTreeSet<String> = arr(&confirmation["affected_items"])?
        .iter()
        .map(|v| s(v).map(str::to_owned))
        .collect::<Result<_>>()?;
    let mut sources: BTreeSet<String> = arr(&confirmation["required_sources"])?
        .iter()
        .map(|v| s(v).map(str::to_owned))
        .collect::<Result<_>>()?;
    let owned: BTreeSet<&str> = arr(&view["evidence_clusters"])?
        .iter()
        .flat_map(|cluster| cluster["sources"].as_array().into_iter().flatten())
        .filter_map(Value::as_str)
        .collect();
    // Unassigned originals can be headings, units or exclusions. Retain these
    // shared context rows instead of mistaking absence of a claim for irrelevance.
    for row in view["sources"]["rows"].as_array().into_iter().flatten() {
        if let Some(source) = row[0].as_str().filter(|source| !owned.contains(source)) {
            sources.insert(source.to_owned());
        }
    }
    // Include all claims sharing a required original, even if their own fields
    // did not change: omissions, duplication and conflicting interpretations matter.
    for cluster in arr(&view["evidence_clusters"])? {
        if arr(&cluster["sources"])?
            .iter()
            .any(|v| v.as_str().is_some_and(|v| sources.contains(v)))
        {
            ids.extend(
                arr(&cluster["items"])?
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned),
            );
        }
    }
    view["items"]
        .as_array_mut()
        .unwrap()
        .retain(|item| item["id"].as_str().is_some_and(|id| ids.contains(id)));
    view["evidence_clusters"]
        .as_array_mut()
        .unwrap()
        .retain(|cluster| {
            cluster["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|id| id.as_str().is_some_and(|id| ids.contains(id)))
        });
    fn source_refs(value: &Value, sources: &mut BTreeSet<String>) {
        match value {
            Value::Object(o) => {
                if let Some(source) = o.get("source").and_then(Value::as_str) {
                    sources.insert(source.to_owned());
                }
                for v in o.values() {
                    source_refs(v, sources);
                }
            }
            Value::Array(a) => {
                for v in a {
                    source_refs(v, sources);
                }
            }
            _ => {}
        }
    }
    source_refs(&view["items"], &mut sources);
    // Keep incoming and outgoing relationships of delivered claims (including
    // deleted predecessors), not the targets of unrelated claims in the packet.
    let items = view["items"].clone();
    if let Some(targets) = view
        .get_mut("relationship_targets")
        .and_then(Value::as_array_mut)
    {
        targets.retain(|target| {
            ids.iter().any(|id| mentions(target, id))
                || target["item"]["id"]
                    .as_str()
                    .is_some_and(|id| mentions(&items, id))
        });
    }
    source_refs(&view["relationship_targets"], &mut sources);
    if let Some(exclusions) = view.get_mut("exclusions").and_then(Value::as_array_mut) {
        exclusions.retain(|entry| {
            let mut refs = BTreeSet::new();
            source_refs(entry, &mut refs);
            !refs.is_disjoint(&sources)
        });
    }
    if let Some(rows) = view
        .pointer_mut("/sources/rows")
        .and_then(Value::as_array_mut)
    {
        rows.retain(|row| {
            row[0]
                .as_str()
                .is_some_and(|source| sources.contains(source))
        });
    }
    if let Some(external) = view
        .get_mut("external_sources")
        .and_then(Value::as_array_mut)
    {
        external.retain(|source| {
            source["ref"]
                .as_str()
                .is_some_and(|source| sources.contains(source))
        });
    }
    view.as_object_mut().unwrap().remove("risk_summary");
    Ok(crate::semantic::compact_review(&view))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet() -> Value {
        json!({"packet":"before", "document":"doc", "sources":{"rows":[["s1"],["s2"],["s3"]]},
            "items":[{"id":"doc/a", "name":"A"}, {"id":"doc/b", "name":"B"}, {"id":"doc/c", "name":"C"}],
            "evidence_clusters":[{"items":["doc/a"],"sources":["s1"]}, {"items":["doc/b"],"sources":["s2"]}, {"items":["doc/c"],"sources":["s3"]}]})
    }
    fn review() -> Value {
        json!({"packet":"before", "findings":[], "audits":[{"sources":["s1","s2","s3"],"rationale":"Checked originals"}]})
    }

    #[test]
    fn delivery_removes_unaffected_text_and_rebuilds_span_references() {
        let mut before = packet();
        before["sources"]["rows"] = json!([
            ["s1", "Changed original"],
            ["s2", "Adjacent original"],
            ["s3", "Unused".repeat(2000)],
            ["s4", "Shared heading"]
        ]);
        for (i, quote) in ["Changed original", "Adjacent original", "Unused"]
            .iter()
            .enumerate()
        {
            before["items"][i]["evidence"] =
                json!([{"source":format!("s{}",i+1),"start":0,"end":quote.len(),"quote":quote}]);
        }
        before["items"][1]["related"] = json!(["doc/a"]);
        // Different field sets exercise the non-tabular item encoding too.
        let mut after = before.clone();
        after["items"][0]["name"] = json!("Corrected");
        let mut prior = review();
        prior["audits"][0]["sources"] = json!(["s1", "s2", "s3", "s4"]);
        let confirmation = plan(&before, &after, &prior).unwrap();
        let full = crate::semantic::compact_review(&after);
        let reduced = delivery(&full, &confirmation).unwrap();
        assert_eq!(reduced["packet"], full["packet"]);
        assert!(encode(&reduced).len() < encode(&full).len() / 2);
        assert_eq!(
            reduced["sources"]["rows"],
            json!([
                ["s1", "Changed original"],
                ["s2", "Adjacent original"],
                ["s4", "Shared heading"]
            ])
        );
        assert!(!reduced.to_string().contains("Unused"));
        assert_eq!(reduced["span_table"]["rows"].as_array().unwrap().len(), 2);
        for item in reduced["items"].as_array().unwrap() {
            let span = item["evidence"][0]["span_ref"].as_u64().unwrap() as usize;
            assert!(
                reduced["span_table"]["rows"][span][3]
                    .as_str()
                    .unwrap()
                    .ends_with("original")
            );
        }
        assert_eq!(delivery(&full, &Value::Null).unwrap(), full);
        let mut shared = confirmation.clone();
        shared["full_review"] = json!(true);
        assert_eq!(delivery(&full, &shared).unwrap(), full);
    }

    #[test]
    fn global_confirmation_keeps_only_affected_claims_and_deletion_context() {
        let mut before = packet();
        before.as_object_mut().unwrap().remove("sources");
        before["document"] = Value::Null;
        let mut after = before.clone();
        after["items"].as_array_mut().unwrap().remove(0);
        after["evidence_clusters"].as_array_mut().unwrap().remove(0);
        after["items"][0]["name"] = json!("Corrected B");
        let confirmation = plan(&before, &after, &review()).unwrap();
        let reduced = delivery(&crate::semantic::compact_review(&after), &confirmation).unwrap();
        assert_eq!(reduced["items"]["rows"].as_array().unwrap().len(), 1);
        assert!(reduced["items"].to_string().contains("doc/b"));
        assert!(!reduced["items"].to_string().contains("doc/c"));
        assert!(reduced.get("sources").is_none());
        assert!(reduced.get("exclusions").is_none());
        assert_eq!(confirmation["changed_items"][0]["before"]["id"], "doc/a");
        assert!(confirmation["changed_items"][0]["after"].is_null());
    }

    #[test]
    fn delivery_preserves_adjacent_external_evidence_but_omits_unrelated_targets() {
        let mut before = packet();
        before["relationship_targets"] = json!([
            {"item":{"id":"other/incoming","related":["doc/a"],"evidence":[{"source":"other/s1","start":0,"end":8,"quote":"incoming"}]}},
            {"item":{"id":"other/unrelated","related":["doc/c"],"evidence":[{"source":"other/s2","start":0,"end":9,"quote":"unrelated"}]}}
        ]);
        before["external_sources"] =
            json!([{"ref":"other/s1","text":"incoming"},{"ref":"other/s2","text":"unrelated"}]);
        let mut after = before.clone();
        after["items"][0]["name"] = json!("Corrected");
        let confirmation = plan(&before, &after, &review()).unwrap();
        let delivered = delivery(&crate::semantic::compact_review(&after), &confirmation).unwrap();
        assert_eq!(
            delivered["relationship_targets"].as_array().unwrap().len(),
            1
        );
        assert_eq!(
            delivered["external_sources"],
            json!([{"ref":"other/s1","text":"incoming"}])
        );
        assert_eq!(
            delivered["span_table"]["rows"][0],
            json!(["other/s1", 0, 8, "incoming"])
        );
    }

    #[test]
    fn changed_claims_invalidate_both_directions_of_relationships() {
        let mut before = packet();
        before["items"][1]["related"] = json!(["doc/a"]);
        let mut after = before.clone();
        after["items"][0]["name"] = json!("corrected");
        let result = plan(&before, &after, &review()).unwrap();
        assert_eq!(result["required_sources"], json!(["s1", "s2"]));
        assert_eq!(result["carried_audits"][0]["sources"], json!(["s3"]));
    }

    #[test]
    fn missing_audits_shared_context_and_unscoped_findings_are_not_reused() {
        let before = packet();
        let mut after = before.clone();
        after["items"][0]["name"] = json!("corrected");
        let mut partial = review();
        partial["audits"][0]["sources"] = json!(["s1"]);
        assert_eq!(
            plan(&before, &after, &partial).unwrap()["required_sources"],
            json!(["s1", "s2", "s3"])
        );
        after["modules"] = json!({"m":"changed shared name"});
        assert_eq!(
            plan(&before, &after, &review()).unwrap()["carried_audits"],
            json!([])
        );
        let mut unscoped = review();
        unscoped["findings"] =
            json!([{"action":"scoped_restructure","items":[],"message":"missing claim"}]);
        assert_eq!(
            plan(&before, &before, &unscoped).unwrap()["required_sources"],
            json!(["s1", "s2", "s3"])
        );
    }

    #[test]
    fn deletion_and_previous_findings_require_source_confirmation() {
        let before = packet();
        let mut after = before.clone();
        after["items"].as_array_mut().unwrap().remove(0);
        after["evidence_clusters"].as_array_mut().unwrap().remove(0);
        let mut prior = review();
        prior["findings"] =
            json!([{"action":"field_repair","items":["doc/b"],"message":"wrong condition"}]);
        assert_eq!(
            plan(&before, &after, &prior).unwrap()["required_sources"],
            json!(["s1", "s2"])
        );
        prior["findings"][0]["action"] = json!("improvement");
        assert_eq!(
            plan(&before, &after, &prior).unwrap()["required_sources"],
            json!(["s1"])
        );
    }
}
