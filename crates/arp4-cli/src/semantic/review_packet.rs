use super::*;
use std::collections::HashMap;

pub(super) fn compact_span<'s>(span: &mut Value, alias: &impl Fn(&str) -> Option<&'s String>) {
    if let Some(source) = span["source"].as_str().and_then(alias) {
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
    ReviewBasis::new(input, model, catalog)?.packet(document, sheet, scope)
}

/// A model and catalog checked against their input once, for the review
/// packets of every document, sheet or source scope of a review plan, with
/// what every packet looks up indexed: the sources of each document, and the
/// items and exclusions citing each source. Checking the whole model, or
/// scanning every item and source, for each packet made a plan grow with its
/// units times the model.
pub struct ReviewBasis<'a> {
    input: &'a Input,
    model: &'a Model,
    catalog: &'a Catalog,
    /// Every source by the alias it has in any packet, `document/sN`.
    all_aliases: BTreeMap<String, String>,
    /// Each document's sources in packet order with their aliases, `sN`.
    documents: BTreeMap<&'a str, (Vec<&'a Source>, BTreeMap<String, String>)>,
    /// The document of each source and its place in the input.
    source_documents: BTreeMap<&'a str, &'a str>,
    source_order: HashMap<&'a str, usize>,
    /// The items, by their place in the model, citing each source, and the
    /// place of each item.
    items_citing: HashMap<String, Vec<usize>>,
    item_order: HashMap<&'a str, usize>,
    /// The exclusions, by their place in the model, of each source.
    exclusions_of: HashMap<&'a str, Vec<usize>>,
    /// The documents the input warns about, and whether a warning names none.
    warning_documents: BTreeSet<String>,
    global_warning: bool,
}

impl<'a> ReviewBasis<'a> {
    pub fn new(input: &'a Input, model: &'a Model, catalog: &'a Catalog) -> Result<Self> {
        spec::assess(input, model)?;
        catalog.validate_issue_documents(input)?;
        ensure!(
            catalog.entries.keys().collect::<BTreeSet<_>>()
                == model.items.iter().map(|i| &i.id).collect(),
            "catalog and model keys differ"
        );
        let mut all_aliases = BTreeMap::new();
        let mut documents = BTreeMap::new();
        for doc in input
            .sources
            .iter()
            .map(|s| s.document.as_str())
            .collect::<BTreeSet<_>>()
        {
            let rows = sources(input, Some(doc))?;
            let mut aliases = BTreeMap::new();
            for (i, source) in rows.iter().enumerate() {
                all_aliases.insert(source.id.clone(), format!("{doc}/s{}", i + 1));
                aliases.insert(source.id.clone(), format!("s{}", i + 1));
            }
            documents.insert(doc, (rows, aliases));
        }
        let mut items_citing: HashMap<String, Vec<usize>> = HashMap::new();
        for (k, item) in model.items.iter().enumerate() {
            for source in item_source_ids(item) {
                items_citing.entry(source).or_default().push(k);
            }
        }
        let mut exclusions_of: HashMap<&str, Vec<usize>> = HashMap::new();
        for (k, exclusion) in model.exclusions.iter().enumerate() {
            exclusions_of
                .entry(exclusion.evidence.source.as_str())
                .or_default()
                .push(k);
        }
        let warned = |warning: &String| {
            input
                .revisions
                .keys()
                .find(|document| warning.starts_with(&format!("{document}: ")))
                .cloned()
        };
        Ok(Self {
            input,
            model,
            catalog,
            all_aliases,
            documents,
            source_documents: input
                .sources
                .iter()
                .map(|source| (source.id.as_str(), source.document.as_str()))
                .collect(),
            source_order: input
                .sources
                .iter()
                .enumerate()
                .map(|(k, source)| (source.id.as_str(), k))
                .collect(),
            items_citing,
            item_order: model
                .items
                .iter()
                .enumerate()
                .map(|(k, item)| (item.id.as_str(), k))
                .collect(),
            exclusions_of,
            warning_documents: input.warnings.iter().filter_map(warned).collect(),
            global_warning: input
                .warnings
                .iter()
                .any(|warning| warned(warning).is_none()),
        })
    }

    /// The review packet of `document` (every document for none), of one
    /// `sheet` of it or of a source `scope` of it.
    pub fn packet(
        &self,
        document: Option<&str>,
        sheet: Option<&str>,
        scope: Option<&ReviewScope>,
    ) -> Result<Value> {
        let (input, model, catalog) = (self.input, self.model, self.catalog);
        let all_aliases = &self.all_aliases;
        ensure!(
            scope.is_none() || (document.is_some() && sheet.is_none()),
            "source scope requires document and cannot combine with sheet"
        );
        ensure!(
            sheet.is_none() || document.is_some(),
            "sheet requires document"
        );
        let every;
        let (rows, document_aliases) = match document {
            Some(doc) => {
                let (rows, aliases) = self
                    .documents
                    .get(doc)
                    .context("unknown document or empty input")?;
                (rows.as_slice(), aliases)
            }
            None => {
                let rows = sources(input, None)?;
                every = (
                    rows.iter()
                        .enumerate()
                        .map(|(i, s)| (s.id.clone(), format!("s{}", i + 1)))
                        .collect::<BTreeMap<_, _>>(),
                    rows,
                );
                (every.1.as_slice(), &every.0)
            }
        };
        // A source scope names sources by alias, `sN` the N-th source of the
        // document: its rows are found by place, in document order, rather than
        // by going through every source of the document for each scope.
        let place = |alias: &String| -> Option<usize> {
            let n: usize = alias.strip_prefix('s')?.parse().ok()?;
            (n >= 1 && n <= rows.len() && format!("s{n}") == *alias).then(|| n - 1)
        };
        let in_order = |aliases: &[String]| -> Vec<&Source> {
            let mut places: Vec<usize> = aliases.iter().filter_map(place).collect();
            places.sort_unstable();
            places.into_iter().map(|k| rows[k]).collect()
        };
        let (rows, context_rows) = match scope {
            Some(scope) => {
                let selected: BTreeSet<_> = scope.sources.iter().chain(&scope.context).collect();
                ensure!(
                    !scope.sources.is_empty()
                        && selected.len() == scope.sources.len() + scope.context.len()
                        && selected.iter().all(|s| place(s).is_some()),
                    "source scope must contain unique known aliases and nonempty audit sources"
                );
                (in_order(&scope.sources), in_order(&scope.context))
            }
            None => (
                rows.iter()
                    .copied()
                    .filter(|s| {
                        sheet.is_none_or(|name| s.context.as_ref().is_some_and(|c| c.sheet == name))
                    })
                    .collect::<Vec<_>>(),
                vec![],
            ),
        };
        ensure!(!rows.is_empty(), "unknown or empty review sheet");
        let aliases: BTreeMap<_, _> = rows
            .iter()
            .map(|s| (s.id.clone(), document_aliases[&s.id].clone()))
            .collect();
        // A source shows the alias of this packet's document, or else the
        // alias with its document.
        let display = |source: &str| -> Option<&String> {
            document
                .and(aliases.get(source))
                .or_else(|| all_aliases.get(source))
        };
        let compact_item = |item: &mut Value, cells: bool| {
            for span in item["evidence"].as_array_mut().unwrap() {
                compact_span(span, &display);
            }
            if let Some(spans) = item["condition"]["evidence"].as_array_mut() {
                for span in spans {
                    compact_span(span, &display);
                }
            }
            for field in ["basis", "unit_basis", "semantics_basis"] {
                if let Some(span) = item["value"].get_mut(field) {
                    compact_span(span, &display);
                }
            }
            if cells {
                for field in ["title", "description", "cells", "notes"] {
                    if let Some(cells) = item["value"][field].as_array_mut() {
                        for cell in cells {
                            compact_span(cell, &display);
                        }
                    }
                }
            }
        };
        // The items citing a source of the packet, in model order.
        let mut cited: Vec<usize> = aliases
            .keys()
            .flat_map(|source| self.items_citing.get(source).into_iter().flatten().copied())
            .collect();
        cited.sort_unstable();
        cited.dedup();
        let mut external = BTreeSet::new();
        let mut items = Vec::new();
        for item in cited.into_iter().map(|k| &model.items[k]) {
            external.extend(
                item_source_ids(item)
                    .into_iter()
                    .filter(|s| !aliases.contains_key(s)),
            );
            let classification = serde_json::to_value(&catalog.entries[&item.id])?;
            let mut item = serde_json::to_value(item)?;
            item["classification"] = classification;
            compact_item(&mut item, true);
            items.push(item);
        }
        let mut excluded: Vec<usize> = aliases
            .keys()
            .flat_map(|source| {
                self.exclusions_of
                    .get(source.as_str())
                    .into_iter()
                    .flatten()
                    .copied()
            })
            .collect();
        excluded.sort_unstable();
        let exclusions = excluded
            .into_iter()
            .map(|k| {
                let mut value = serde_json::to_value(&model.exclusions[k]).unwrap();
                compact_span(&mut value["evidence"], &display);
                value
            })
            .collect::<Vec<_>>();
        let source_documents = &self.source_documents;
        let mut relevant_documents: BTreeSet<String> =
            rows.iter().map(|s| s.document.clone()).collect();
        relevant_documents.extend(
            external
                .iter()
                .filter_map(|id| source_documents.get(id.as_str()))
                .map(|document| (*document).to_owned()),
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
        let mut targeted: Vec<usize> = target_ids
            .iter()
            .filter_map(|id| self.item_order.get(id.as_str()).copied())
            .collect();
        targeted.sort_unstable();
        let targets: Vec<Value> = targeted
            .into_iter()
            .map(|k| &model.items[k])
            .map(|i| {
                for id in item_source_ids(i) {
                    if let Some(&document) = source_documents.get(id.as_str()) {
                        relevant_documents.insert(document.to_owned());
                        if !aliases.contains_key(&id) {
                            external.insert(id);
                        }
                    }
                }
                let mut item = serde_json::to_value(i).unwrap();
                compact_item(&mut item, false);
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
        let mut risk_summary = BTreeMap::<String, usize>::new();
        let original = |id: &str| -> Result<&spec::Item> {
            Ok(&model.items[*self.item_order.get(id).context("review item not found")?])
        };
        for item in &mut items {
            let id = item["id"].as_str().context("review item ID missing")?;
            let original = original(id)?;
            let source_ids = item_source_ids(original);
            let warning = self.global_warning
                || source_ids.iter().any(|source| {
                    source_documents
                        .get(source.as_str())
                        .is_some_and(|document| self.warning_documents.contains(*document))
                });
            let risk = review_risk(original, &catalog.entries[id], warning);
            let level = risk["level"].as_str().unwrap();
            *risk_summary.entry(level.to_owned()).or_default() += 1;
            item["review_risk"] = risk;
        }
        let mut cluster_keys = BTreeMap::<String, Vec<String>>::new();
        for item in &mut items {
            let refs: BTreeSet<String> = item_source_ids(original(item["id"].as_str().unwrap())?)
                .iter()
                .map(|source| display(source).cloned().context("unknown evidence source"))
                .collect::<Result<_>>()?;
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
        // The sources other packets show, in input order.
        let mut outside: Vec<usize> = external
            .iter()
            .filter_map(|id| self.source_order.get(id.as_str()).copied())
            .collect();
        outside.sort_unstable();
        let mut packet = json!({"version":1,"document":document,"sources":source_data,"items":items,"exclusions":exclusions,
        "warnings":warnings,"modules":modules,"open_issues":issues,"relationship_targets":targets,"contract":hash(REVIEW_PROMPT.as_bytes()),
        "external_sources":outside.into_iter().map(|k| &input.sources[k]).map(|s| {
            let mut context = json!(s.context);
            if let Some(c) = &s.context {
                // The merges of the source's sheet, which the input records once for the sheet.
                context["merges"] = json!(input.sheet_merges(&s.document, &c.sheet));
                if let Some(structure) = input.structures.get(&s.document) {
                    context["structure"] = crate::document_structure::sheet_context(structure, &c.sheet);
                }
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
}
