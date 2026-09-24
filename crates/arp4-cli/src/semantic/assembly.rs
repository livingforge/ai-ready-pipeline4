use super::*;

pub(super) fn span_in(input: &Input, value: &Value, rows: &[&Source]) -> Result<Value> {
    let alias = value
        .as_str()
        .or_else(|| value["source"].as_str())
        .context("evidence selector required")?;
    if let Some((document, reference)) = alias.split_once('/') {
        ensure!(
            value.is_object(),
            "cross-document evidence requires an exact quote"
        );
        let mut value = value.clone();
        value["source"] = json!(reference);
        span(&value, &sources(input, Some(document))?)
    } else {
        span(value, rows)
    }
}

pub(crate) fn expand_item(input: &Input, item: &mut Value, rows: &[&Source]) -> Result<()> {
    let mut selectors = item["evidence"]
        .as_array()
        .context("item evidence array required")?
        .clone();
    for field in ["basis", "unit_basis", "semantics_basis"] {
        if let Some(selector) = item["value"].get(field) {
            selectors.push(selector.clone());
        }
    }
    if let Some(evidence) = item["condition"]["evidence"].as_array() {
        selectors.extend(evidence.iter().cloned());
    }
    if item["value"]["kind"] == "table" {
        for field in ["title", "description", "cells", "notes"] {
            let selected = item["value"][field]
                .as_array()
                .context("table part array required")?
                .clone();
            selectors.extend(selected.iter().cloned());
            item["value"][field] = json!(
                selected
                    .iter()
                    .map(|e| span_in(input, e, rows))
                    .collect::<Result<Vec<_>>>()?
            );
        }
    }
    let mut evidence = Vec::new();
    for span in selectors
        .iter()
        .map(|e| span_in(input, e, rows))
        .collect::<Result<Vec<_>>>()?
    {
        if !evidence.contains(&span) {
            evidence.push(span);
        }
    }
    ensure!(!evidence.is_empty(), "item requires source evidence");
    item["evidence"] = json!(evidence);
    if matches!(
        item["condition"]["basis"].as_str(),
        Some("stated" | "composed")
    ) {
        let expanded = item["condition"]["evidence"]
            .as_array()
            .context("condition evidence required")?
            .iter()
            .map(|e| span_in(input, e, rows))
            .collect::<Result<Vec<_>>>()?;
        item["condition"]["evidence"] = json!(expanded);
    }
    for field in ["basis", "unit_basis", "semantics_basis"] {
        if let Some(selection) = item["value"].get(field) {
            item["value"][field] = span_in(input, selection, rows)?;
        }
    }
    if item.get("statement").is_none() {
        // Mechanical display only. Never invent a rationale or an omitted role/detail.
        let value: crate::specifications::AtomicValue =
            serde_json::from_value(item["value"].clone())?;
        let condition: crate::specifications::Condition =
            serde_json::from_value(item["condition"].clone())?;
        item["statement"] = json!(format!(
            "{} / {} / {} / {}",
            item["subject"].as_str().unwrap_or(""),
            item["property"].as_str().unwrap_or(""),
            condition.text(),
            value.display()
        ));
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reply {
    pub packet: String,
    pub document: String,
    pub modules: BTreeMap<String, String>,
    pub items: Vec<Value>,
    pub exclusions: Vec<Value>,
    pub open_issues: Vec<String>,
}

/// Creates temporary, namespaced item keys. Persistent IDs remain the assign-ids/registry job.
/// No audits, exclusions, conditions, classifications or acceptance tests are invented.
pub fn assemble(
    input: &Input,
    replies: Vec<Reply>,
    actor: &str,
) -> Result<(Model, Catalog, Value)> {
    ensure!(!actor.trim().is_empty(), "actor required");
    let mut documents = BTreeSet::new();
    let mut model = json!({"schema_version":1,"input_hash":hash(&encoded(&serde_json::to_value(input)?)),"items":[],"exclusions":[],"audits":[],"decisions":[]});
    let mut catalog = Catalog {
        actor: actor.into(),
        reason: "Classification supplied with semantic extraction".into(),
        modules: BTreeMap::new(),
        open_issues: vec![],
        open_issue_documents: BTreeMap::new(),
        entries: BTreeMap::new(),
    };
    let mut plan = json!({"reviewer":actor,"items":{},"retire":{}});
    let mut replies = replies;
    replies.sort_by(|a, b| a.document.cmp(&b.document));
    for reply in replies {
        ensure!(
            documents.insert(reply.document.clone()),
            "duplicate document reply"
        );
        ensure!(
            packet(input, &reply.document)?["packet"] == reply.packet,
            "stale or mismatched semantic packet"
        );
        let rows = sources(input, Some(&reply.document))?;
        for (module, name) in reply.modules {
            ensure!(
                !module.is_empty()
                    && module
                        .bytes()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
                    && !name.trim().is_empty(),
                "invalid module"
            );
            if let Some(old) = catalog.modules.insert(module, name.clone()) {
                ensure!(old == name, "inconsistent module names");
            }
        }
        for issue in reply.open_issues {
            catalog
                .open_issue_documents
                .entry(issue.clone())
                .or_default()
                .insert(reply.document.clone());
            if !catalog.open_issues.contains(&issue) {
                catalog.open_issues.push(issue);
            }
        }
        for mut item in reply.items {
            let object = item
                .as_object_mut()
                .context("semantic item must be object")?;
            let key = object.remove("key").context("item key required")?;
            let key = key.as_str().context("key must be string")?;
            ensure!(
                !key.is_empty()
                    && key
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_'),
                "invalid local item key"
            );
            let id = format!("{}/{key}", reply.document);
            let classification = object
                .remove("classification")
                .context("classification required")?;
            let mut classification: Classification = serde_json::from_value(classification)?;
            ensure!(
                catalog.modules.contains_key(&classification.module)
                    && classification
                        .reason
                        .as_ref()
                        .is_none_or(|r| !r.trim().is_empty()),
                "known classification module and nonempty reason (when supplied) required"
            );
            // Cross-document relationships are deferred to the global semantic review.
            let qualify = |keys: &mut Vec<String>| {
                for key in keys {
                    if !key.contains('/') {
                        *key = format!("{}/{key}", reply.document);
                    }
                }
            };
            qualify(&mut classification.requirements);
            qualify(&mut classification.related);
            ensure!(
                !object.contains_key("id")
                    && !object.contains_key("kind")
                    && !object.contains_key("requirements"),
                "ID/kind/requirements are generated from key/classification"
            );
            object.insert("id".into(), json!(id));
            object.insert(
                "kind".into(),
                serde_json::to_value(&classification.category)?,
            );
            object.insert("requirements".into(), json!(classification.requirements));
            ensure!(
                catalog.entries.insert(id.clone(), classification).is_none(),
                "duplicate item key"
            );
            plan["items"][&id] = json!({"action":"new","reason":"Initial semantic extraction; no prior identity asserted"});
            expand_item(input, &mut item, &rows)?;
            model["items"].as_array_mut().unwrap().push(item);
        }
        for mut exclusion in reply.exclusions {
            exclusion["evidence"] = span(&exclusion["evidence"], &rows)?;
            model["exclusions"].as_array_mut().unwrap().push(exclusion);
        }
    }
    ensure!(
        documents == input.sources.iter().map(|s| s.document.clone()).collect(),
        "one reply per input document required; missing replies are not exclusions"
    );
    for classification in catalog.entries.values() {
        for id in classification
            .requirements
            .iter()
            .chain(&classification.related)
        {
            ensure!(
                catalog.entries.contains_key(id),
                "unknown item relationship: {id}"
            );
        }
        for id in &classification.requirements {
            ensure!(
                catalog.entries[id].category == crate::registry::Category::Requirement,
                "requirements must reference requirement category"
            );
        }
    }
    let model: Model = serde_json::from_value(model)?;
    spec::assess(input, &model)?; // Structural validity only; missing review/coverage stay blocked.
    Ok((model, catalog, plan))
}
