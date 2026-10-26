use super::*;
use std::{
    cell::OnceCell,
    collections::{HashMap, HashSet},
};

pub(super) fn nonempty(value: &str) -> Result<()> {
    ensure!(
        !value.trim().is_empty() && value == value.trim(),
        "empty or untrimmed text"
    );
    Ok(())
}
pub(super) fn normalized(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

pub fn assess(input: &Input, model: &Model) -> Result<Report> {
    InputIndex::new(input)?.of(input).assess(model)
}

/// Positions in the input by document.
type Positions = BTreeMap<String, Vec<usize>>;

/// What assessing a model reads from its input besides the sources: the
/// input's hash and where each source is, by ID and by document. A command
/// checks the replies of every document against one input; building this
/// once spares hashing and indexing the whole input again for each reply.
pub struct InputIndex {
    hash: String,
    /// The position of the last source with each ID, as a map of the sources keeps it.
    by_id: HashMap<String, usize>,
    /// The positions of each document's sources in packet order, made when first read.
    documents: OnceCell<Positions>,
    /// The warnings naming no document and those of each document, made when first read.
    warnings: OnceCell<(Vec<usize>, Positions)>,
}

impl InputIndex {
    pub fn new(input: &Input) -> Result<Self> {
        Ok(Self {
            hash: hash(&encoded(&serde_json::to_value(input)?)),
            by_id: input
                .sources
                .iter()
                .enumerate()
                .map(|(position, source)| (source.id.clone(), position))
                .collect(),
            documents: OnceCell::new(),
            warnings: OnceCell::new(),
        })
    }
    /// The input with this index, which must be the one it was built from.
    pub fn of<'a>(&'a self, input: &'a Input) -> Indexed<'a> {
        Indexed { input, index: self }
    }
}

/// An input with its [`InputIndex`].
#[derive(Clone, Copy)]
pub struct Indexed<'a> {
    pub input: &'a Input,
    index: &'a InputIndex,
}

/// The claims of a model with what checking it has marked.
struct Checked<'a, 'm> {
    /// Marks of the characters of each cited source: 1 cited, 2 excluded.
    coverage: BTreeMap<&'a str, Vec<u8>>,
    items: BTreeMap<&'m str, &'m Item>,
    groups: BTreeMap<(String, String, String), Vec<&'m Item>>,
}

impl<'a> Indexed<'a> {
    pub fn hash(&self) -> &'a str {
        &self.index.hash
    }
    pub fn source(&self, id: &str) -> Option<&'a Source> {
        let input = self.input;
        self.index.by_id.get(id).map(|&i| &input.sources[i])
    }
    /// The sources of `document` in the order its extraction packet lists them.
    pub fn rows(&self, document: &str) -> Result<Vec<&'a Source>> {
        let input = self.input;
        Ok(self
            .positions(document)?
            .iter()
            .map(|&i| &input.sources[i])
            .collect())
    }
    /// The source at each position of [`Self::rows`], without collecting them.
    pub(crate) fn row_at(
        &self,
        document: &str,
    ) -> Result<impl Fn(usize) -> Option<&'a Source> + use<'a>> {
        let input = self.input;
        let positions = self.positions(document)?;
        Ok(move |i: usize| positions.get(i).map(|&p| &input.sources[p]))
    }
    fn positions(&self, document: &str) -> Result<&'a [usize]> {
        let input = self.input;
        let index = self.index;
        let documents = index.documents.get_or_init(|| {
            let mut documents = BTreeMap::<String, Vec<usize>>::new();
            for (position, source) in input.sources.iter().enumerate() {
                documents
                    .entry(source.document.clone())
                    .or_default()
                    .push(position);
            }
            for positions in documents.values_mut() {
                positions.sort_by_key(|&i| input.sources[i].packet_order());
            }
            documents
        });
        let positions = documents.get(document).map_or(&[][..], Vec::as_slice);
        ensure!(!positions.is_empty(), "unknown document or empty input");
        Ok(positions)
    }
    /// The warnings an extraction packet of `document` lists, in input order:
    /// those naming no document of the input and those naming `document`.
    pub fn warnings(&self, document: &str) -> Vec<&'a String> {
        let input = self.input;
        let (shared, named) = self.index.warnings.get_or_init(|| {
            let mut shared = Vec::new();
            let mut named = BTreeMap::<String, Vec<usize>>::new();
            for (position, warning) in input.warnings.iter().enumerate() {
                match warning_document(input, warning) {
                    Some(document) => named.entry(document.clone()).or_default().push(position),
                    None => shared.push(position),
                }
            }
            (shared, named)
        });
        let mut positions: Vec<usize> = shared
            .iter()
            .chain(named.get(document).into_iter().flatten())
            .copied()
            .collect();
        positions.sort_unstable();
        positions.into_iter().map(|i| &input.warnings[i]).collect()
    }
    pub fn assess(&self, model: &Model) -> Result<Report> {
        let Checked {
            mut coverage,
            items,
            groups,
        } = self.checked(model)?;
        let input = self.input;
        let sources: BTreeMap<_, _> = input.sources.iter().map(|s| (s.id.as_str(), s)).collect();
        self.report(model, &sources, &mut coverage, &items, &groups)
    }
    /// Whether `model` is well formed against the input, as [`assess`] checks
    /// it, without assessing its coverage. Decisions, audits and a review
    /// record are checked only by [`assess`].
    pub(crate) fn check(&self, model: &Model) -> Result<()> {
        self.checked(model).map(drop)
    }
    fn checked<'m>(&self, model: &'m Model) -> Result<Checked<'a, 'm>> {
        let value = serde_json::to_value(model)?;
        static VALIDATOR: std::sync::LazyLock<jsonschema::Validator> =
            std::sync::LazyLock::new(|| {
                jsonschema::validator_for(&schema()).expect("embedded model schema")
            });
        let validator = &*VALIDATOR;
        if let Err(error) = validator.validate(&value) {
            anyhow::bail!("model {}: {}", error.instance_path(), error);
        }
        ensure!(
            model.schema_version == 1,
            "unsupported model version; schema_version 1 required"
        );
        ensure!(
            model.input_hash == self.index.hash,
            "input hash mismatch; rebuild/review against current input"
        );
        // Only the cited sources are marked; the others stay uncovered.
        let mut coverage = BTreeMap::<&str, Vec<u8>>::new();
        let mut source_chars = BTreeMap::<&str, Vec<char>>::new();
        let mut span = |s: &Span, excluded: bool| -> Result<()> {
            let source = self
                .source(s.source.as_str())
                .context("unknown evidence source")?;
            let chars = source_chars
                .entry(source.id.as_str())
                .or_insert_with(|| source.text.chars().collect());
            ensure!(
                s.start < s.end && s.end <= chars.len(),
                "invalid Unicode character span"
            );
            ensure!(
                chars[s.start..s.end].iter().copied().eq(s.quote.chars()),
                "evidence quote mismatch"
            );
            let marks = coverage
                .entry(source.id.as_str())
                .or_insert_with(|| vec![0; chars.len()]);
            for mark in &mut marks[s.start..s.end] {
                ensure!(
                    *mark != 2 && (!excluded || *mark == 0),
                    "excluded span overlaps another disposition: document={}, source={}, location={}, range={}..{}, quote={:?}",
                    source.document,
                    s.source,
                    source.location,
                    s.start,
                    s.end,
                    s.quote
                );
                *mark = if excluded { 2 } else { 1 };
            }
            Ok(())
        };
        let mut items = BTreeMap::new();
        let mut groups: BTreeMap<_, Vec<&Item>> = BTreeMap::new();
        for item in &model.items {
            if let Some(name) = &item.name {
                nonempty(name)?;
            }
            for text in [
                &item.id,
                &item.subject,
                &item.property,
                item.condition.text(),
                &item.statement,
            ] {
                nonempty(text)?;
            }
            if let Some(verification) = &item.verification {
                nonempty(verification)?;
            }
            ensure!(
                items.insert(item.id.as_str(), item).is_none(),
                "duplicate item ID"
            );
            ensure!(!item.evidence.is_empty(), "item needs evidence");
            for evidence in &item.evidence {
                span(evidence, false)?;
            }
            if let AtomicValue::Table {
                title,
                description,
                cells,
                notes,
            } = &item.value
            {
                ensure!(
                    table_category_allowed(&item.kind),
                    "table category is not allowed by the specification contract"
                );
                ensure!(!cells.is_empty(), "table body must contain original cells");
                let mut seen = BTreeSet::new();
                let mut sheet = None;
                // The item's evidence spans, looked up for each cell rather than scanned.
                let evidence: HashSet<_> = item
                    .evidence
                    .iter()
                    .map(|e| (e.source.as_str(), e.start, e.end))
                    .collect();
                for cell in title.iter().chain(description).chain(cells).chain(notes) {
                    span(cell, false)?;
                    let source = self
                        .source(cell.source.as_str())
                        .context("unknown evidence source")?;
                    let context = source
                        .context
                        .as_ref()
                        .context("table cell needs original grid context")?;
                    let identity = (&source.document, &context.sheet);
                    ensure!(
                        sheet.is_none_or(|old| old == identity),
                        "table cells must belong to one document/sheet"
                    );
                    sheet = Some(identity);
                    ensure!(
                        seen.insert((&cell.source, cell.start, cell.end)),
                        "duplicate table cell across or within parts"
                    );
                    ensure!(
                        cell.start == 0
                            && cell.end == source.text.chars().count()
                            && cell.quote == source.text,
                        "table cells must preserve the complete original text"
                    );
                    ensure!(
                        evidence.contains(&(cell.source.as_str(), cell.start, cell.end)),
                        "table cell must be contained in item evidence"
                    );
                }
            }
            if let AtomicValue::Quantity {
                basis: Some(basis), ..
            } = &item.value
            {
                span(basis, false)?;
                ensure!(
                    item.evidence.iter().any(|e| e.source == basis.source
                        && e.start <= basis.start
                        && e.end >= basis.end),
                    "quantity basis must be contained in item evidence: {}",
                    item.id
                );
            }
            if let AtomicValue::QuantityExpression { basis } = &item.value {
                span(basis, false)?;
                ensure!(
                    item.evidence.iter().any(|e| e.source == basis.source
                        && e.start <= basis.start
                        && e.end >= basis.end),
                    "quantity expression basis must be contained in item evidence: {}",
                    item.id
                );
            }
            if let AtomicValue::Quantity {
                unit_basis: Some(basis),
                unit,
                interpretation,
                ..
            } = &item.value
            {
                span(basis, false)?;
                ensure!(
                    basis.quote == *unit
                        && item.evidence.iter().any(|e| e.source == basis.source
                            && e.start <= basis.start
                            && e.end >= basis.end),
                    "unit basis must quote the unit and be contained in item evidence: {}",
                    item.id
                );
                if *interpretation == QuantityInterpretation::OpaqueUnit {
                    ensure!(
                        !basis.quote.is_empty(),
                        "opaque quantity needs a non-empty unit basis: {}",
                        item.id
                    );
                }
            } else if let AtomicValue::Quantity {
                interpretation: QuantityInterpretation::OpaqueUnit,
                ..
            } = &item.value
            {
                ensure!(false, "opaque quantity needs unit_basis: {}", item.id);
            }
            if let AtomicValue::Quantity {
                semantics_basis: Some(basis),
                ..
            } = &item.value
            {
                span(basis, false)?;
                ensure!(
                    item.evidence.iter().any(|e| e.source == basis.source
                        && e.start <= basis.start
                        && e.end >= basis.end),
                    "semantics basis must be contained in item evidence: {}",
                    item.id
                );
            }
            if let Condition::Stated { text, evidence } = &item.condition {
                ensure!(
                    !evidence.is_empty(),
                    "stated condition needs evidence: {}",
                    item.id
                );
                for evidence in evidence {
                    span(evidence, false)?;
                }
                let compact =
                    |s: &str| s.chars().filter(|c| !c.is_whitespace()).collect::<String>();
                ensure!(
                    evidence.iter().any(|s| compact(&s.quote) == compact(text)),
                    "condition must match an exact source quote (whitespace may differ): {}",
                    item.id
                );
            }
            if let Condition::Composed {
                evidence, reason, ..
            } = &item.condition
            {
                nonempty(reason)?;
                ensure!(
                    evidence.len() >= 2,
                    "composed condition needs at least two source fragments"
                );
                for fragment in evidence {
                    span(fragment, false)?;
                    ensure!(
                        item.evidence.iter().any(|e| e.source == fragment.source
                            && e.start <= fragment.start
                            && e.end >= fragment.end),
                        "composed condition fragment must be contained in item evidence: {}",
                        item.id
                    );
                }
            }
            // Raw tables do not assert one scalar property; global review checks overlapping tables.
            if matches!(item.value, AtomicValue::Table { .. }) {
                continue;
            }
            groups
                .entry(claim_key(&item.subject, &item.property, &item.condition))
                .or_default()
                .push(item);
        }
        for exclusion in &model.exclusions {
            nonempty(&exclusion.reason)?;
            span(&exclusion.evidence, true)?;
        }
        for item in &model.items {
            let mut refs = BTreeSet::new();
            ensure!(
                item.kind == Kind::Specification || item.requirements.is_empty(),
                "only specifications can implement requirements"
            );
            for target in &item.requirements {
                ensure!(
                    refs.insert(target)
                        && items
                            .get(target.as_str())
                            .is_some_and(|t| t.kind == Kind::Requirement && t.id != item.id),
                    "invalid requirement reference"
                );
            }
        }
        Ok(Checked {
            coverage,
            items,
            groups,
        })
    }
    fn report<'m>(
        &self,
        model: &'m Model,
        sources: &BTreeMap<&'a str, &'a Source>,
        coverage: &mut BTreeMap<&'a str, Vec<u8>>,
        items: &BTreeMap<&'m str, &'m Item>,
        groups: &BTreeMap<(String, String, String), Vec<&'m Item>>,
    ) -> Result<Report> {
        let input = self.input;
        let mut issues = vec![];
        let mut quantity_exemptions = vec![];
        for item in &model.items {
            if let Some(issue) = quantity_diagnostic(item, sources) {
                issues.push(issue);
            }
            quantity_exemptions.extend(quantities::quantity_exemptions(item));
            if let Condition::Assumed { .. } = &item.condition {
                issues.push(json!({"code":"assumed_condition", "item":item.id}));
            }
        }
        let mut decided = BTreeSet::new();
        for decision in &model.decisions {
            nonempty(&decision.reviewer)?;
            nonempty(&decision.rationale)?;
            let candidates: BTreeSet<_> = decision.candidates.iter().map(String::as_str).collect();
            ensure!(
                candidates.len() >= 2
                    && candidates.len() == decision.candidates.len()
                    && candidates.contains(decision.selected.as_str()),
                "invalid conflict decision"
            );
            let group = items
                .get(decision.selected.as_str())
                .and_then(|item| {
                    groups.get(&claim_key(&item.subject, &item.property, &item.condition))
                })
                .filter(|group| {
                    group.len() == candidates.len()
                        && group
                            .iter()
                            .all(|item| candidates.contains(item.id.as_str()))
                })
                .context("decision must cover exactly one complete conflict group")?;
            ensure!(
                group
                    .iter()
                    .map(|i| i.value.key())
                    .collect::<BTreeSet<_>>()
                    .len()
                    > 1,
                "decision does not describe conflicting values"
            );
            ensure!(decided.insert(candidates), "duplicate conflict decision");
        }
        for group in groups.values().filter(|g| g.len() > 1) {
            let candidates: BTreeSet<_> = group.iter().map(|i| i.id.as_str()).collect();
            let values: BTreeSet<_> = group.iter().map(|i| i.value.key()).collect();
            if values.len() < group.len() {
                issues.push(json!({"code":"duplicate_claim", "items":candidates}));
            }
            if values.len() > 1 && !decided.contains(&candidates) {
                issues.push(json!({"code":"conflict", "items":candidates}));
            }
        }
        let rejected: BTreeSet<_> = model
            .decisions
            .iter()
            .flat_map(|d| d.candidates.iter().filter(|id| *id != &d.selected))
            .collect();
        for item in &model.items {
            if !rejected.contains(&item.id) {
                for requirement in &item.requirements {
                    if rejected.contains(requirement) {
                        issues.push(json!({"code":"rejected_requirement", "item":item.id,"requirement":requirement}));
                    }
                }
            }
        }
        let mut audited = BTreeSet::new();
        for audit in &model.audits {
            nonempty(&audit.reviewer)?;
            nonempty(&audit.rationale)?;
            ensure!(
                sources.contains_key(audit.source.as_str())
                    && audited.insert(audit.source.as_str()),
                "invalid or duplicate source audit"
            );
        }
        let mut total_characters = 0usize;
        let mut omitted = BTreeMap::new();
        if let Some(review) = &model.review {
            review.plan.validate(input)?;
            if review.content_hash != review_content_hash(model) {
                issues.push(json!({"code":"stale_semantic_review"}));
            }
            if !review.global {
                issues.push(json!({"code":"missing_global_review"}));
            }
            for finding in review
                .findings
                .iter()
                .filter(|f| f["action"] != "improvement")
            {
                issues.push(json!({"code":"semantic_review_finding","finding":finding}));
            }
            omitted = review.plan.omitted_sources.clone();
        }
        let mut covered_characters = 0usize;
        let mut complete_sources = 0usize;
        let mut coverage_matrix = Vec::new();
        for source in &input.sources {
            let marks = &*coverage
                .entry(source.id.as_str())
                .or_insert_with(|| vec![0; sources[source.id.as_str()].text.chars().count()]);
            let missing: Vec<_> = source
                .text
                .chars()
                .enumerate()
                .filter(|(i, c)| !c.is_whitespace() && marks[*i] == 0)
                .map(|(i, _)| i)
                .collect();
            let total = source.text.chars().filter(|c| !c.is_whitespace()).count();
            let uncovered_ranges = {
                let mut ranges: Vec<[usize; 2]> = vec![];
                for position in &missing {
                    if let Some(last) = ranges.last_mut().filter(|last| last[1] == *position) {
                        last[1] += 1;
                    } else {
                        ranges.push([*position, *position + 1]);
                    }
                }
                ranges
            };
            total_characters += total;
            covered_characters += total - missing.len();
            if missing.is_empty() {
                complete_sources += 1;
            }
            if !missing.is_empty() {
                issues.push(json!({"code":"uncovered_text", "source":source.id,"document":source.document,"location":source.location,"ranges":uncovered_ranges,"count":missing.len()}));
            }
            let audited_source = audited.contains(source.id.as_str());
            if !audited_source && !omitted.contains_key(&source.id) {
                issues.push(json!({"code":"missing_semantic_audit", "source":source.id}));
            }
            let status = match (missing.is_empty(), audited_source) {
                (true, true) => "covered_and_audited",
                (true, false) => "covered_not_audited",
                (false, true) => "partially_covered_and_audited",
                (false, false) => "partially_covered_not_audited",
            };
            coverage_matrix.push(json!({
                "source": source.id,
                "document": source.document,
                "location": source.location,
                "characters": total,
                "covered_characters": total - missing.len(),
                "uncovered_characters": missing.len(),
                "uncovered_ranges": uncovered_ranges,
                "audited": audited_source,
                "review_required": !omitted.contains_key(&source.id),
                "review_omission_reason": omitted.get(&source.id),
                "status": status,
            }));
        }
        if model.items.is_empty() {
            issues.push(json!({"code":"no_items"}));
        }
        // Routing metadata is deterministic. It does not decide the semantic correction.
        for issue in &mut issues {
            let code = issue["code"].as_str().unwrap();
            let (action, fields): (&str, &[&str]) = match code {
                "missing_semantic_audit"
                | "missing_global_review"
                | "stale_semantic_review"
                | "semantic_review_finding" => ("review", &[]),
                "unsupported_quantity_expression" | "unsupported_quantity_unit" => {
                    ("validator_support", &[])
                }
                "quantity_interpretation_required" => ("manual_triage", &["value", "evidence"]),
                "uncovered_text" | "no_items" | "duplicate_claim" | "conflict" => {
                    ("scoped_restructure", &[])
                }
                "assumed_condition" => ("field_repair", &["condition"]),
                "rejected_requirement" => ("field_repair", &["classification"]),
                "quantity_semantics_in_name" => ("field_repair", &["name", "value"]),
                "quantity_semantics_in_property" => ("field_repair", &["property", "value"]),
                "quantity_in_text"
                | "quantity_basis_mismatch"
                | "ambiguous_quantity_basis"
                | "missing_quantity_basis"
                | "invalid_quantity_semantics_basis"
                | "missing_quantity_unit_basis"
                | "quantity_source_mismatch" => ("field_repair", &["value", "evidence"]),
                _ => ("manual_triage", &[]),
            };
            let mut ids: BTreeSet<String> = issue["items"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect();
            if let Some(id) = issue["item"].as_str() {
                ids.insert(id.into());
            }
            let mut source_ids: BTreeSet<String> = ids
                .iter()
                .filter_map(|id| items.get(id.as_str()))
                .flat_map(|item| item.evidence.iter().map(|s| s.source.clone()))
                .collect();
            if let Some(id) = issue["source"].as_str() {
                source_ids.insert(id.into());
            }
            let documents: BTreeSet<_> = source_ids
                .iter()
                .filter_map(|id| sources.get(id.as_str()))
                .map(|s| s.document.clone())
                .collect();
            issue["repair"] = json!({"action":action,"fields":fields,"items":ids,
            "sources":source_ids,"documents":documents});
        }
        let mut summary = BTreeMap::new();
        for issue in &issues {
            *summary
                .entry(issue["code"].as_str().unwrap().to_owned())
                .or_insert(0) += 1;
        }
        Ok(Report {
            ready: issues.is_empty(),
            assurance: json!({"quantity_source_checks": true, "semantic_completeness_proven":false,"original_document_completeness_proven":false,"audit_contents_verified":false}),
            summary,
            coverage: json!({"sources":input.sources.len(), "fully_covered_sources":complete_sources,"audited_sources":audited.len(),"required_review_sources":input.sources.len()-omitted.len(),"omitted_review_sources":omitted.len(),"global_reviewed":model.review.as_ref().is_some_and(|r| r.global),"characters":total_characters,"covered_characters":covered_characters,"matrix":coverage_matrix}),
            issues,
            quantity_exemptions,
        })
    }
}

/// The document a warning names: the shortest `<document>: ` it starts with,
/// which is the first such document in the order of the input's revisions.
pub(crate) fn warning_document<'a>(input: &'a Input, warning: &str) -> Option<&'a String> {
    warning
        .match_indices(": ")
        .find_map(|(end, _)| input.revisions.get_key_value(&warning[..end]))
        .map(|(document, _)| document)
}
