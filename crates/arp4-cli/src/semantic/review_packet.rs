use super::*;

pub(super) fn compact_span(span: &mut Value, aliases: &BTreeMap<String, String>) {
    if let Some(source) = span["source"].as_str().and_then(|s| aliases.get(s)) {
        // Review displays offsets, since repeated quotes must remain distinguishable.
        span["source"] = json!(source);
    }
}

pub(super) fn item_source_ids(item: &spec::Item) -> BTreeSet<String> {
    let mut ids: BTreeSet<_> = item.evidence.iter().map(|s| s.source.clone()).collect();
    if let Some(evidence) = item.condition.evidence() {
        ids.extend(evidence.iter().map(|s| s.source.clone()));
    }
    if let spec::AtomicValue::Quantity {
        basis,
        unit_basis,
        semantics_basis,
        ..
    } = &item.value
    {
        ids.extend(basis.iter().chain(unit_basis).map(|s| s.source.clone()));
        ids.extend(semantics_basis.iter().map(|s| s.source.clone()));
    } else if let spec::AtomicValue::QuantityExpression { basis } = &item.value {
        ids.insert(basis.source.clone());
    }
    ids
}

/// Deterministic triage metadata. This is a review hint, never an approval
/// decision: the source coverage and semantic findings remain authoritative.
pub(super) fn review_risk(
    item: &spec::Item,
    classification: &Classification,
    warning: bool,
) -> Value {
    let mut score = 0u8;
    let mut signals = Vec::new();
    if item.kind == spec::Kind::Requirement {
        score += 3;
        signals.push("requirement");
    }
    if matches!(
        item.value,
        spec::AtomicValue::Quantity { .. } | spec::AtomicValue::QuantityExpression { .. }
    ) {
        score += 3;
        signals.push("quantity");
    }
    if matches!(item.condition, spec::Condition::Assumed { .. }) {
        score += 2;
        signals.push("assumed_condition");
    }
    if matches!(item.condition, spec::Condition::Composed { .. }) {
        score += 3;
        signals.push("composed_condition_requires_interpretation_review");
    }
    if item_source_ids(item).len() > 1 {
        score += 2;
        signals.push("multiple_sources");
    }
    if !classification.requirements.is_empty() || !classification.related.is_empty() {
        score += 2;
        signals.push("relationship");
    }
    if warning {
        score += 2;
        signals.push("source_warning");
    }
    let level = if score >= 5 {
        "high"
    } else if score >= 2 {
        "medium"
    } else {
        "low"
    };
    json!({"level":level,"score":score,"signals":signals,"purpose":"triage_hint_not_approval"})
}

pub fn review_packet(
    input: &Input,
    model: &Model,
    catalog: &Catalog,
    document: Option<&str>,
) -> Result<Value> {
    review_packet_scoped(input, model, catalog, document, None)
}

/// A sheet retains all of its original text; global consistency is still reviewed separately.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewScope {
    pub sources: Vec<String>,
    pub context: Vec<String>,
}

pub fn review_packet_scoped(
    input: &Input,
    model: &Model,
    catalog: &Catalog,
    document: Option<&str>,
    sheet: Option<&str>,
) -> Result<Value> {
    review_packet_selected(input, model, catalog, document, sheet, None)
}

pub fn review_packet_selected(
    input: &Input,
    model: &Model,
    catalog: &Catalog,
    document: Option<&str>,
    sheet: Option<&str>,
    scope: Option<&ReviewScope>,
) -> Result<Value> {
    ensure!(
        scope.is_none() || (document.is_some() && sheet.is_none()),
        "source scope requires document and cannot combine with sheet"
    );
    ensure!(
        sheet.is_none() || document.is_some(),
        "sheet requires document"
    );
    spec::assess(input, model)?;
    catalog.validate_issue_documents(input)?;
    ensure!(
        catalog.entries.keys().collect::<BTreeSet<_>>()
            == model.items.iter().map(|i| &i.id).collect(),
        "catalog and model keys differ"
    );
    let rows = sources(input, document)?;
    let document_aliases: BTreeMap<_, _> = rows
        .iter()
        .enumerate()
        .map(|(i, s)| (s.id.clone(), format!("s{}", i + 1)))
        .collect();
    if let Some(scope) = scope {
        let selected: BTreeSet<_> = scope.sources.iter().chain(&scope.context).collect();
        ensure!(
            !scope.sources.is_empty()
                && selected.len() == scope.sources.len() + scope.context.len()
                && selected
                    .iter()
                    .all(|s| document_aliases.values().any(|alias| alias == *s)),
            "source scope must contain unique known aliases and nonempty audit sources"
        );
    }
    let context_rows: Vec<_> = rows
        .iter()
        .copied()
        .filter(|s| scope.is_some_and(|scope| scope.context.contains(&document_aliases[&s.id])))
        .collect();
    let rows: Vec<_> = rows
        .into_iter()
        .filter(|s| sheet.is_none_or(|name| s.context.as_ref().is_some_and(|c| c.sheet == name)))
        .filter(|s| scope.is_none_or(|scope| scope.sources.contains(&document_aliases[&s.id])))
        .collect();
    ensure!(!rows.is_empty(), "unknown or empty review sheet");
    let aliases: BTreeMap<_, _> = rows
        .iter()
        .map(|s| (s.id.clone(), document_aliases[&s.id].clone()))
        .collect();
    let mut all_aliases = BTreeMap::new();
    for doc in input
        .sources
        .iter()
        .map(|s| s.document.as_str())
        .collect::<BTreeSet<_>>()
    {
        for (i, source) in sources(input, Some(doc))?.iter().enumerate() {
            all_aliases.insert(source.id.clone(), format!("{doc}/s{}", i + 1));
        }
    }
    let mut display_aliases = all_aliases.clone();
    if document.is_some() {
        display_aliases.extend(aliases.clone());
    }
    let mut external = BTreeSet::new();
    let mut items = Vec::new();
    for item in &model.items {
        let item_sources = item_source_ids(item);
        if !item_sources.iter().any(|s| aliases.contains_key(s)) {
            continue;
        }
        external.extend(
            item_sources
                .into_iter()
                .filter(|s| !aliases.contains_key(s)),
        );
        let classification = serde_json::to_value(&catalog.entries[&item.id])?;
        let mut item = serde_json::to_value(item)?;
        item["classification"] = classification;
        for span in item["evidence"].as_array_mut().unwrap() {
            compact_span(span, &display_aliases);
        }
        if let Some(spans) = item["condition"]["evidence"].as_array_mut() {
            for span in spans {
                compact_span(span, &display_aliases);
            }
        }
        for field in ["basis", "unit_basis", "semantics_basis"] {
            if let Some(span) = item["value"].get_mut(field) {
                compact_span(span, &display_aliases);
            }
        }
        for field in ["title", "description", "cells", "notes"] {
            if let Some(cells) = item["value"][field].as_array_mut() {
                for cell in cells {
                    compact_span(cell, &display_aliases);
                }
            }
        }
        items.push(item);
    }
    let exclusions = model
        .exclusions
        .iter()
        .filter(|e| aliases.contains_key(&e.evidence.source))
        .map(|e| {
            let mut value = serde_json::to_value(e).unwrap();
            compact_span(&mut value["evidence"], &display_aliases);
            value
        })
        .collect::<Vec<_>>();
    let mut relevant_documents: BTreeSet<String> =
        rows.iter().map(|s| s.document.clone()).collect();
    relevant_documents.extend(
        input
            .sources
            .iter()
            .filter(|s| external.contains(&s.id))
            .map(|s| s.document.clone()),
    );
    let local_ids: BTreeSet<String> = items
        .iter()
        .filter_map(|i| i["id"].as_str().map(str::to_owned))
        .collect();
    let target_ids: BTreeSet<String> = local_ids
        .iter()
        .flat_map(|id| {
            let c = &catalog.entries[id];
            c.requirements.iter().chain(&c.related)
        })
        .filter(|id| !local_ids.contains(*id))
        .cloned()
        .collect();
    ensure!(
        target_ids.iter().all(|id| catalog.entries.contains_key(id)),
        "unknown relationship target"
    );
    // Referenced claims are real dependencies, even without cross-document source citations.
    let targets: Vec<Value> = model
        .items
        .iter()
        .filter(|i| target_ids.contains(&i.id))
        .map(|i| {
            for id in item_source_ids(i) {
                if let Some(s) = input.sources.iter().find(|s| s.id == id) {
                    relevant_documents.insert(s.document.clone());
                    if !aliases.contains_key(&s.id) {
                        external.insert(s.id.clone());
                    }
                }
            }
            let mut item = serde_json::to_value(i).unwrap();
            for span in item["evidence"].as_array_mut().unwrap() {
                compact_span(span, &display_aliases);
            }
            if let Some(spans) = item["condition"]["evidence"].as_array_mut() {
                for span in spans {
                    compact_span(span, &display_aliases);
                }
            }
            for field in ["basis", "unit_basis", "semantics_basis"] {
                if let Some(span) = item["value"].get_mut(field) {
                    compact_span(span, &display_aliases);
                }
            }
            json!({"item":item,"classification":catalog.entries[&i.id]})
        })
        .collect();
    let used_modules: BTreeSet<&str> = local_ids
        .iter()
        .chain(&target_ids)
        .map(|id| catalog.entries[id].module.as_str())
        .collect();
    let modules: BTreeMap<_, _> = catalog
        .modules
        .iter()
        .filter(|(id, _)| document.is_none() || used_modules.contains(id.as_str()))
        .collect();
    let issues: Vec<_> = catalog
        .open_issues
        .iter()
        .filter(|issue| {
            document.is_none()
                || catalog
                    .open_issue_documents
                    .get(*issue)
                    .is_some_and(|owners| {
                        owners.is_empty() || !owners.is_disjoint(&relevant_documents)
                    })
        })
        .collect();
    let warnings = relevant_warnings(input, &relevant_documents);
    let source_documents: BTreeMap<_, _> = input
        .sources
        .iter()
        .map(|source| (source.id.as_str(), source.document.as_str()))
        .collect();
    let warning_documents: BTreeSet<_> = input
        .warnings
        .iter()
        .filter_map(|warning| {
            input
                .revisions
                .keys()
                .find(|document| warning.starts_with(&format!("{document}: ")))
                .cloned()
        })
        .collect();
    let global_warning = input.warnings.iter().any(|warning| {
        !input
            .revisions
            .keys()
            .any(|document| warning.starts_with(&format!("{document}: ")))
    });
    let mut risk_summary = BTreeMap::<String, usize>::new();
    let originals: BTreeMap<_, _> = model
        .items
        .iter()
        .map(|item| (item.id.as_str(), item))
        .collect();
    for item in &mut items {
        let id = item["id"].as_str().context("review item ID missing")?;
        let original = originals.get(id).context("review item not found")?;
        let source_ids = item_source_ids(original);
        let warning = global_warning
            || source_ids.iter().any(|source| {
                source_documents
                    .get(source.as_str())
                    .is_some_and(|document| warning_documents.contains(*document))
            });
        let risk = review_risk(original, &catalog.entries[id], warning);
        let level = risk["level"].as_str().unwrap();
        *risk_summary.entry(level.to_owned()).or_default() += 1;
        item["review_risk"] = risk;
    }
    let mut cluster_keys = BTreeMap::<String, Vec<String>>::new();
    for item in &mut items {
        let refs: BTreeSet<String> = item_source_ids(originals[item["id"].as_str().unwrap()])
            .iter()
            .map(|source| display_aliases[source].clone())
            .collect();
        let key = serde_json::to_string(&refs)?;
        item["evidence_cluster"] = json!(format!("evidence-{}", &hash(key.as_bytes())[..12]));
        cluster_keys
            .entry(key)
            .or_default()
            .push(item["id"].as_str().unwrap().to_owned());
    }
    let mut evidence_clusters = Vec::new();
    for (key, ids) in cluster_keys {
        let cluster_id = format!("evidence-{}", &hash(key.as_bytes())[..12]);
        let sources = serde_json::from_str::<BTreeSet<String>>(&key)?;
        evidence_clusters.push(json!({"id":cluster_id,"items":ids,"sources":sources}));
    }
    let mut source_data = source_rows(input, &rows);
    for (row, source) in source_data["rows"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .zip(&rows)
    {
        row[0] = json!(aliases[&source.id]);
    }
    let mut packet = json!({"version":1,"document":document,"sources":source_data,"items":items,"exclusions":exclusions,
        "warnings":warnings,"modules":modules,"open_issues":issues,"relationship_targets":targets,"contract":hash(REVIEW_PROMPT.as_bytes()),
        "external_sources":input.sources.iter().filter(|s| external.contains(&s.id)).map(|s| {
            let mut context = json!(s.context);
            if let Some(c) = &s.context && let Some(structure) = input.structures.get(&s.document) {
                context["structure"] = crate::document_structure::sheet_context(structure, &c.sheet);
            }
            json!({"ref":all_aliases[&s.id],"text":s.text,"document":s.document,"location":s.location,"context":context})
        }).collect::<Vec<_>>(),
        "risk_summary":risk_summary,
        "evidence_clusters":evidence_clusters});
    if let Some(sheet) = sheet {
        packet["sheet"] = json!(sheet);
    }
    if let Some(scope) = scope {
        packet["scope"] = serde_json::to_value(scope)?;
        let mut context = source_rows(input, &context_rows);
        for (row, source) in context["rows"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .zip(&context_rows)
        {
            row[0] = json!(document_aliases[&source.id]);
        }
        packet["context_sources"] = context;
    }
    if document.is_none() {
        if !input.structures.is_empty() {
            packet["structures"] = serde_json::to_value(&input.structures)?;
        }
        // Global review compares claims and their quoted evidence. Full source review is per document.
        packet.as_object_mut().unwrap().remove("sources");
        packet.as_object_mut().unwrap().remove("exclusions");
    }
    packet["packet"] = json!(packet_fingerprint(&packet));
    Ok(packet)
}
