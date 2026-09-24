use super::*;

pub(super) fn conflicts(r: &Registry, approved_only: bool) -> Vec<Vec<String>> {
    let normalize = |s: &str| {
        s.to_lowercase()
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
    };
    let mut groups = BTreeMap::<_, Vec<&Entry>>::new();
    for e in r.entries.values().filter(|e| {
        matches!(e.category, Category::Requirement | Category::Specification)
            && if approved_only {
                e.status == Status::Approved
            } else {
                e.status != Status::Retired
            }
    }) {
        groups
            .entry((
                normalize(&e.subject),
                normalize(&e.property),
                normalize(e.condition.text()),
            ))
            .or_default()
            .push(e);
    }
    groups
        .values()
        .filter(|g| {
            g.iter()
                .map(|e| e.value.key())
                .collect::<BTreeSet<_>>()
                .len()
                > 1
        })
        .map(|g| g.iter().map(|e| e.id.clone()).collect())
        .collect()
}
pub fn fingerprint(r: &Registry) -> Result<String> {
    Ok(hash(&bytes(&r.manifest)?))
}

pub fn validate(r: &Registry) -> Result<()> {
    let m = &r.manifest;
    let inputs = r
        .inputs
        .iter()
        .map(|(k, v)| Ok((k.clone(), serde_json::from_value::<spec::Input>(v.clone())?)))
        .collect::<Result<BTreeMap<_, _>>>()?;
    ensure!(
        m.schema_version == 1 && m.revision > 0,
        "unsupported registry version"
    );
    nonempty(&m.project)?;
    ensure!(
        m.modules
            .iter()
            .all(|(k, v)| slug(k) && !v.trim().is_empty()),
        "invalid module"
    );
    ensure!(m.issued.iter().all(|id| valid_id(id)), "invalid issued ID");
    ensure!(
        m.evidence.iter().all(|s| digest(s)),
        "invalid evidence hash"
    );
    ensure!(
        m.evidence == r.inputs.keys().cloned().collect(),
        "evidence set mismatch"
    );
    ensure!(
        m.archives.keys().eq(r.archives.keys()),
        "archive set mismatch"
    );
    for (name, value) in &r.archives {
        ensure!(
            slug(name) && m.archives[name] == hash(&encoded(value)),
            "invalid archive or fingerprint"
        );
    }
    ensure!(m.records.keys().eq(r.entries.keys()), "record set mismatch");
    for (key, input) in &r.inputs {
        ensure!(
            hash(&encoded(input)) == *key,
            "evidence fingerprint mismatch"
        );
    }
    for (id, e) in &r.entries {
        ensure!(
            id == &e.id && valid_id(id) && m.issued.contains(id),
            "invalid record ID"
        );
        ensure!(
            m.records[id] == hash(&bytes(e)?),
            "record fingerprint mismatch: {id}"
        );
        ensure!(
            m.modules.contains_key(&e.module),
            "unknown module: {}",
            e.module
        );
        for t in [&e.name, &e.subject, &e.property, &e.statement] {
            nonempty(t)?;
        }
        for text in [&e.verification, &e.rationale].into_iter().flatten() {
            nonempty(text)?;
        }
        if matches!(e.value, AtomicValue::Table { .. }) {
            ensure!(
                spec::table_category_allowed(&e.category),
                "table category is not allowed by the specification contract"
            );
        }
        for t in &e.acceptance {
            nonempty(t)?;
        }
        if e.evidence.is_empty() {
            nonempty(
                e.rationale
                    .as_deref()
                    .context("authored judgment without evidence needs a rationale")?,
            )?;
            match &e.condition {
                Condition::Stated { .. } | Condition::Composed { .. } => {
                    anyhow::bail!("stated condition requires archived evidence: {id}")
                }
                Condition::Assumed { text, reason } => {
                    nonempty(text)?;
                    nonempty(reason)?;
                }
                Condition::Unspecified => {}
            }
            match &e.value {
                AtomicValue::Text { text } => nonempty(text)?,
                AtomicValue::Table { .. } => {
                    anyhow::bail!("table requires archived evidence: {id}")
                }
                AtomicValue::Quantity {
                    unit,
                    basis,
                    unit_basis,
                    semantics_basis,
                    ..
                } => {
                    nonempty(unit)?;
                    ensure!(
                        basis.is_none() && unit_basis.is_none() && semantics_basis.is_none(),
                        "quantity citations require archived evidence: {id}"
                    );
                }
                AtomicValue::QuantityExpression { .. } => {
                    anyhow::bail!(
                        "unresolved quantity requires interpretation before registry adoption: {id}"
                    )
                }
            }
        }
        ensure!(
            e.category == Category::Specification || e.requirements.is_empty(),
            "only specifications implement requirements: {id}"
        );
        let mut links = BTreeSet::new();
        for target in e.requirements.iter().chain(&e.related) {
            ensure!(
                target != id && links.insert(target),
                "self or duplicate link: {id}"
            );
            let other = r.entries.get(target).context("dangling relationship")?;
            if e.requirements.contains(target) {
                ensure!(
                    other.category == Category::Requirement,
                    "requirement target has wrong category"
                );
            }
            if e.status == Status::Approved {
                ensure!(
                    other.status == Status::Approved,
                    "approved entry depends on unapproved/retired entry: {id}"
                );
            }
        }
        if e.status == Status::Approved {
            ensure!(
                !matches!(e.condition, Condition::Assumed { .. }),
                "assumed condition cannot be approved: {id}"
            );
            let a = e
                .approval
                .as_ref()
                .context("approved entry has no approval")?;
            nonempty(&a.actor)?;
            nonempty(&a.reason)?;
            ensure!(
                a.revision > 0 && a.revision <= m.revision,
                "invalid approval revision"
            );
            if matches!(e.category, Category::Requirement | Category::Specification) {
                ensure!(
                    !e.acceptance.is_empty(),
                    "approved requirement/specification needs acceptance criteria: {id}"
                );
            }
        } else {
            ensure!(
                e.approval.is_none(),
                "nonapproved entry carries approval: {id}"
            );
        }
        for ev in &e.evidence {
            let input = inputs
                .get(&ev.snapshot)
                .context("unknown evidence snapshot")?;
            let src = input
                .sources
                .iter()
                .find(|s| s.id == ev.span.source)
                .context("unknown evidence source")?;
            let chars: Vec<_> = src.text.chars().collect();
            ensure!(
                ev.span.start < ev.span.end && ev.span.end <= chars.len(),
                "invalid evidence range"
            );
            ensure!(
                chars[ev.span.start..ev.span.end].iter().collect::<String>() == ev.span.quote,
                "evidence quote mismatch"
            );
        }
        // Preserve the same structural/quantity checks as extraction, even for authored records.
        // Newly authored decisions without source evidence are explicitly distinguished by rationale.
        if !e.evidence.is_empty() {
            let mut sources = BTreeMap::new();
            let mut needed: BTreeSet<_> = e
                .evidence
                .iter()
                .map(|ev| ev.span.source.as_str())
                .collect();
            if let Some(evidence) = e.condition.evidence() {
                needed.extend(evidence.iter().map(|s| s.source.as_str()));
            }
            for snapshot in e
                .evidence
                .iter()
                .map(|ev| &ev.snapshot)
                .collect::<BTreeSet<_>>()
            {
                let input = &inputs[snapshot];
                for s in input
                    .sources
                    .iter()
                    .filter(|s| needed.contains(s.id.as_str()))
                {
                    if let Some(existing) = sources.get(&s.id) {
                        ensure!(
                            bytes(existing)? == bytes(s)?,
                            "conflicting revisions of one source in a record"
                        );
                    }
                    sources.insert(
                        s.id.clone(),
                        serde_json::from_value::<spec::Source>(serde_json::to_value(s)?)?,
                    );
                }
            }
            let input = spec::Input {
                schema_version: 1,
                revisions: BTreeMap::new(),
                sources: sources.into_values().collect(),
                warnings: vec![],
                structures: BTreeMap::new(),
                structure_requirements: BTreeMap::new(),
            };
            let model: spec::Model = serde_json::from_value(
                json!({"schema_version":1,"input_hash":hash(&bytes(&input)?),"items":[{
                "id":id,"name":e.name,"kind":e.category,"subject":e.subject,"property":e.property,
                "condition":e.condition,"value":e.value,"statement":e.statement,"verification":e.verification,
                "requirements":[],"evidence":e.evidence.iter().map(|x|&x.span).collect::<Vec<_>>()
            }],"exclusions":[],"audits":[],"decisions":[]}),
            )?;
            let report = spec::assess(&input, &model)?;
            ensure!(
                report.issues.iter().all(|x| matches!(
                    x["code"].as_str(),
                    Some("uncovered_text" | "missing_semantic_audit")
                ) || (x["code"] == "assumed_condition"
                    && e.status == Status::Proposed)),
                "invalid source-bound claim: {id}: {:?}",
                report.summary
            );
        }
    }
    for issue in &m.open_issues {
        nonempty(issue)?;
    }
    ensure!(conflicts(r, true).is_empty(), "conflicting approved claims");
    ensure!(
        m.last_change.revision == m.revision && m.last_change.parent == m.parent,
        "change identity mismatch"
    );
    nonempty(&m.last_change.actor)?;
    nonempty(&m.last_change.reason)?;
    Ok(())
}
