use super::*;

pub fn import(
    input: spec::Input,
    model: spec::Model,
    ledger: Ledger,
    mut catalog: Catalog,
    project: String,
) -> Result<Registry> {
    ensure!(
        spec::assess(&input, &model)?.ready,
        "extraction model must pass validation"
    );
    ledger.verify(&model)?;
    catalog.validate_issue_documents(&input)?;
    nonempty(&catalog.actor)?;
    nonempty(&catalog.reason)?;
    let snapshot = hash(&bytes(&input)?);
    let judgments = json!({"input_hash":snapshot,"audits":model.audits,"exclusions":model.exclusions,"decisions":model.decisions});
    let mut entries = BTreeMap::new();
    for item in model.items {
        let c = catalog
            .entries
            .remove(&item.id)
            .context("missing classification")?;
        if let Some(reason) = &c.reason {
            nonempty(reason)?;
        }
        let entry = Entry {
            id: item.id,
            name: item.name.context("item needs a name")?,
            category: c.category,
            module: c.module,
            status: Status::Proposed,
            subject: item.subject,
            property: item.property,
            condition: item.condition,
            value: item.value,
            statement: item.statement,
            verification: item.verification,
            acceptance: vec![],
            requirements: c.requirements,
            related: c.related,
            evidence: item
                .evidence
                .into_iter()
                .map(|span| Evidence {
                    snapshot: snapshot.clone(),
                    span,
                })
                .collect(),
            rationale: c.reason,
            approval: None,
        };
        entries.insert(entry.id.clone(), entry);
    }
    ensure!(catalog.entries.is_empty(), "unknown classified items");
    let last_change = ChangeRecord {
        revision: 1,
        parent: None,
        actor: catalog.actor,
        reason: catalog.reason,
        changed: entries.keys().cloned().collect(),
        impacted: vec![],
        approved: vec![],
        aliases: BTreeMap::new(),
        resolved_issues: BTreeMap::new(),
        repointed: vec![],
    };
    let archives: BTreeMap<String, Value> = BTreeMap::from([("judgments".into(), judgments)]);
    let mut r = Registry {
        manifest: Manifest {
            schema_version: 1,
            project,
            revision: 1,
            parent: None,
            modules: catalog.modules,
            open_issues: catalog.open_issues,
            records: BTreeMap::new(),
            evidence: BTreeSet::from([snapshot.clone()]),
            archives: archives
                .iter()
                .map(|(k, v)| (k.clone(), hash(&encoded(v))))
                .collect(),
            issued: ledger.entries.into_keys().collect(),
            last_change,
        },
        entries,
        inputs: BTreeMap::from([(snapshot, serde_json::to_value(input)?)]),
        archives,
    };
    refresh(&mut r)?;
    validate(&r)?;
    Ok(r)
}
/// Check every known registry import boundary without creating a revision.
///
/// init deliberately fails fast because it is a write operation. This
/// read-only report is intended for operators and workflow preflight: it
/// keeps the same gates, but reports the independent gates together.
pub fn preflight(
    input: &spec::Input,
    model: &spec::Model,
    ledger: &Ledger,
    catalog: &Catalog,
    project: &str,
) -> Result<Value> {
    let mut checks = Vec::new();

    let report = match spec::assess(input, model) {
        Ok(report) => {
            let details =
                json!({"summary":report.summary,"coverage":report.coverage,"issues":report.issues});
            add_check(
                &mut checks,
                "model_validation",
                if report.ready {
                    Ok(())
                } else {
                    Err(anyhow::anyhow!("model has validation issues"))
                },
                Some(details),
            );
            Some(report)
        }
        Err(error) => {
            add_check(&mut checks, "model_validation", Err(error), None);
            None
        }
    };
    add_check(&mut checks, "identity_ledger", ledger.verify(model), None);
    add_check(&mut checks, "project", nonempty(project), None);
    add_check(&mut checks, "catalog_actor", nonempty(&catalog.actor), None);
    add_check(
        &mut checks,
        "catalog_reason",
        nonempty(&catalog.reason),
        None,
    );

    let model_ids: BTreeSet<_> = model.items.iter().map(|item| item.id.as_str()).collect();
    let catalog_ids: BTreeSet<_> = catalog.entries.keys().map(String::as_str).collect();
    let missing: BTreeSet<_> = model_ids.difference(&catalog_ids).copied().collect();
    let unknown: BTreeSet<_> = catalog_ids.difference(&model_ids).copied().collect();
    add_check(
        &mut checks,
        "classification_keys",
        ensure_result(
            missing.is_empty() && unknown.is_empty(),
            "model/catalog classification keys differ",
        ),
        Some(json!({"missing":missing,"unknown":unknown})),
    );

    if checks.iter().all(|check| check["ok"] == true) {
        let candidate = import(
            serde_json::from_value(serde_json::to_value(input)?)?,
            serde_json::from_value(serde_json::to_value(model)?)?,
            serde_json::from_value(serde_json::to_value(ledger)?)?,
            serde_json::from_value(serde_json::to_value(catalog)?)?,
            project.to_owned(),
        );
        add_check(&mut checks, "registry_import", candidate.map(|_| ()), None);
    }
    let ready = checks.iter().all(|check| check["ok"] == true);
    let passed = checks.iter().filter(|check| check["ok"] == true).count();
    let failed = checks.len() - passed;
    Ok(json!({
        "ready": ready,
        "summary": {"checks":checks.len(),"passed":passed,"failed":failed},
        "coverage": report.map(|r| r.coverage),
        "checks": checks,
    }))
}

/// Points the evidence of the records citing an earlier snapshot at one of
/// the snapshots `added` by this change that holds every source they cite
/// from the earlier one as it is, of the same version of its document: the
/// same version, structure requirements, structures and merges. The earlier
/// snapshot is then cited no more and leaves the registry, so the snapshots
/// follow the captures kept rather than the changes made. One the judgments
/// of an archive name stays, as the record of what they were made on.
/// Returns the records re-pointed, in order; they keep their approvals, as
/// what they quote is the same.
fn repoint(r: &mut Registry, added: &BTreeSet<String>) -> Result<Vec<String>> {
    if added.is_empty() {
        return Ok(vec![]);
    }
    let archived: BTreeSet<&str> = r
        .archives
        .values()
        .filter_map(|judgments| judgments["input_hash"].as_str())
        .collect();
    // The sources each earlier snapshot is cited for, by the records citing it.
    let mut wanted: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for e in r.entries.values() {
        let needed = cited(e);
        for ev in &e.evidence {
            if !added.contains(&ev.snapshot) && !archived.contains(ev.snapshot.as_str()) {
                wanted
                    .entry(ev.snapshot.as_str())
                    .or_default()
                    .extend(needed.iter().copied());
            }
        }
    }
    // A source by its ID, the first with it as validation reads it, and
    // what a snapshot holds of a document besides its sources.
    fn sources(input: &spec::Input) -> Result<BTreeMap<String, Vec<u8>>> {
        let mut sources = BTreeMap::new();
        for source in &input.sources {
            if !sources.contains_key(&source.id) {
                sources.insert(source.id.clone(), bytes(source)?);
            }
        }
        Ok(sources)
    }
    fn document(input: &spec::Input, name: &str) -> Result<Vec<u8>> {
        bytes(&json!([
            input.revisions.get(name),
            input.structure_requirements.get(name),
            input.structures.get(name),
            input.merges.get(name)
        ]))
    }
    let read = |key: &str| spec::Input::deserialize(&r.inputs[key]);
    let fresh = added
        .iter()
        .map(|key| {
            let input = read(key)?;
            let held = sources(&input)?;
            Ok((key, input, held))
        })
        .collect::<Result<Vec<_>>>()?;
    let mut moves = BTreeMap::new();
    for (old, ids) in &wanted {
        let earlier = read(old)?;
        let cited: BTreeMap<String, Vec<u8>> = sources(&earlier)?
            .into_iter()
            .filter(|(id, _)| ids.contains(id.as_str()))
            .collect();
        let documents: BTreeSet<&str> = earlier
            .sources
            .iter()
            .filter(|s| cited.contains_key(&s.id))
            .map(|s| s.document.as_str())
            .collect();
        'fresh: for (key, input, held) in &fresh {
            if cited
                .iter()
                .any(|(id, source)| held.get(id) != Some(source))
            {
                continue;
            }
            for name in &documents {
                if document(&earlier, name)? != document(input, name)? {
                    continue 'fresh;
                }
            }
            moves.insert(old.to_string(), (*key).clone());
            break;
        }
    }
    let mut repointed = vec![];
    for e in r.entries.values_mut() {
        let mut moved = false;
        for ev in &mut e.evidence {
            if let Some(new) = moves.get(&ev.snapshot) {
                ev.snapshot = new.clone();
                moved = true;
            }
        }
        if moved {
            repointed.push(e.id.clone());
        }
    }
    Ok(repointed)
}

pub(super) fn add_check(
    checks: &mut Vec<Value>,
    name: &str,
    result: Result<()>,
    details: Option<Value>,
) {
    match result {
        Ok(()) => checks.push(json!({"name":name,"ok":true})),
        Err(error) => checks.push(json!({
            "name": name,
            "ok": false,
            "message": format!("{error:#}"),
            "details": details.unwrap_or(Value::Null),
        })),
    }
}

pub(super) fn ensure_result(condition: bool, message: &str) -> Result<()> {
    ensure!(condition, message.to_owned());
    Ok(())
}

pub fn apply(r: Registry, change: Change, additional: Vec<spec::Input>) -> Result<Registry> {
    apply_with(r, change, additional, Snapshots::default())
}

/// [`apply`] to a registry whose snapshots `snapshots` read.
pub(super) fn apply_with(
    mut r: Registry,
    change: Change,
    additional: Vec<spec::Input>,
    mut snapshots: Snapshots,
) -> Result<Registry> {
    let parent = fingerprint(&r)?;
    ensure!(change.base_hash == parent, "stale base revision");
    nonempty(&change.actor)?;
    nonempty(&change.reason)?;
    let mut added = BTreeSet::new();
    for input in additional {
        let v = serde_json::to_value(input)?;
        let h = hash(&encoded(&v));
        if !r.inputs.contains_key(&h) {
            added.insert(h.clone());
        }
        r.manifest.evidence.insert(h.clone());
        r.inputs.insert(h, v);
    }
    for (slug, title) in change.modules {
        ensure!(self::slug(&slug), "invalid module slug");
        nonempty(&title)?;
        r.manifest.modules.insert(slug, title);
    }
    let mut touched = BTreeSet::new();
    let mut aliases = BTreeMap::new();
    // The last number of each prefix, found in the issued IDs once and then
    // counted on, rather than searched again for every new entry.
    let mut numbers: BTreeMap<&str, u64> = BTreeMap::new();
    for e in &change.entries {
        ensure!(touched.insert(e.id.clone()), "duplicate change ID");
        if e.id.starts_with("new:") {
            ensure!(slug(&e.id[4..]), "invalid temporary ID");
            let prefix = match e.category {
                Category::Requirement => "REQ",
                Category::Specification => "SPEC",
                Category::Observation => "OBS",
                Category::Estimate => "EST",
                Category::Reference => "REF",
            };
            let last = match numbers.get(prefix) {
                Some(&last) => last,
                None => r
                    .manifest
                    .issued
                    .iter()
                    .filter_map(|s| s.strip_prefix(&format!("{prefix}-")))
                    .filter_map(|s| s.parse::<u64>().ok())
                    .max()
                    .unwrap_or(0),
            };
            let next = last.checked_add(1).context("ID overflow")?;
            numbers.insert(prefix, next);
            let id = format!("{prefix}-{next:06}");
            r.manifest.issued.insert(id.clone());
            aliases.insert(e.id.clone(), id);
        } else {
            ensure!(
                r.entries.contains_key(&e.id),
                "unknown update ID; use new:<key>"
            );
            ensure!(
                r.entries[&e.id].status != Status::Retired,
                "cannot reactivate retired ID"
            );
        }
        ensure!(
            e.status == Status::Proposed && e.approval.is_none(),
            "submitted entries must be proposed without approval"
        );
    }
    let mut changed = BTreeSet::new();
    for mut e in change.entries {
        if let Some(id) = aliases.get(&e.id) {
            e.id = id.clone();
        }
        for target in e.requirements.iter_mut().chain(&mut e.related) {
            if let Some(id) = aliases.get(target) {
                *target = id.clone();
            }
        }
        changed.insert(e.id.clone());
        r.entries.insert(e.id.clone(), e);
    }
    for id in change.retire {
        ensure!(changed.insert(id.clone()), "duplicate retirement/update");
        let e = r.entries.get_mut(&id).context("unknown retirement ID")?;
        ensure!(e.status != Status::Retired, "already retired");
        e.status = Status::Retired;
        e.approval = None;
    }
    let mut dependents = BTreeMap::<&str, Vec<&str>>::new();
    for entry in r.entries.values() {
        for target in entry.requirements.iter().chain(&entry.related) {
            dependents
                .entry(target)
                .or_default()
                .push(entry.id.as_str());
        }
    }
    let mut affected = changed.clone();
    let mut pending: std::collections::VecDeque<_> = changed.iter().map(String::as_str).collect();
    while let Some(id) = pending.pop_front() {
        if let Some(referrers) = dependents.get(id) {
            for &referrer in referrers {
                if affected.insert(referrer.to_owned()) {
                    pending.push_back(referrer);
                }
            }
        }
    }
    if !change.add_issues.is_empty() {
        affected.extend(
            r.entries
                .values()
                .filter(|e| e.status == Status::Approved)
                .map(|e| e.id.clone()),
        );
    }
    let impacted: Vec<_> = affected.difference(&changed).cloned().collect();
    for id in &impacted {
        let e = r.entries.get_mut(id).unwrap();
        if e.status == Status::Approved {
            e.status = Status::Proposed;
            e.approval = None;
        }
    }
    for (issue, resolution) in &change.resolve_issues {
        nonempty(resolution)?;
        ensure!(
            r.manifest.open_issues.contains(issue),
            "unknown issue resolution"
        );
        r.manifest.open_issues.retain(|x| x != issue);
    }
    r.manifest.open_issues.extend(change.add_issues);
    r.manifest.revision += 1;
    r.manifest.parent = Some(parent.clone());
    let mut approved = Vec::new();
    let mut seen = BTreeSet::new();
    for approval in change.approve {
        ensure!(
            r.manifest.open_issues.is_empty(),
            "resolve open issues before approval"
        );
        nonempty(&approval.reason)?;
        let id = aliases.get(&approval.id).cloned().unwrap_or(approval.id);
        ensure!(seen.insert(id.clone()), "duplicate approval");
        let e = r.entries.get_mut(&id).context("unknown approval ID")?;
        ensure!(
            e.status == Status::Proposed,
            "only proposed entries can be approved"
        );
        e.status = Status::Approved;
        e.approval = Some(Approval {
            actor: change.actor.clone(),
            reason: approval.reason,
            revision: r.manifest.revision,
        });
        approved.push(id);
    }
    let repointed = repoint(&mut r, &added)?;
    r.manifest.last_change = ChangeRecord {
        revision: r.manifest.revision,
        parent: Some(parent),
        actor: change.actor,
        reason: change.reason,
        changed: changed.into_iter().collect(),
        impacted,
        approved,
        aliases,
        resolved_issues: change.resolve_issues,
        repointed,
    };
    refresh(&mut r)?;
    validate_with(&r, &mut snapshots)?;
    Ok(r)
}
