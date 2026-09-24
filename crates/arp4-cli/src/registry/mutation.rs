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

pub fn apply(mut r: Registry, change: Change, additional: Vec<spec::Input>) -> Result<Registry> {
    let parent = fingerprint(&r)?;
    ensure!(change.base_hash == parent, "stale base revision");
    nonempty(&change.actor)?;
    nonempty(&change.reason)?;
    for input in additional {
        let v = serde_json::to_value(input)?;
        let h = hash(&encoded(&v));
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
            let next = r
                .manifest
                .issued
                .iter()
                .filter_map(|s| s.strip_prefix(&format!("{prefix}-")))
                .filter_map(|s| s.parse::<u64>().ok())
                .max()
                .unwrap_or(0)
                .checked_add(1)
                .context("ID overflow")?;
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
    let mut affected = changed.clone();
    loop {
        let next: Vec<_> = r
            .entries
            .values()
            .filter(|e| {
                !affected.contains(&e.id)
                    && e.requirements
                        .iter()
                        .chain(&e.related)
                        .any(|id| affected.contains(id))
            })
            .map(|e| e.id.clone())
            .collect();
        if next.is_empty() {
            break;
        }
        affected.extend(next);
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
    for approval in change.approve {
        ensure!(
            r.manifest.open_issues.is_empty(),
            "resolve open issues before approval"
        );
        nonempty(&approval.reason)?;
        let id = aliases.get(&approval.id).cloned().unwrap_or(approval.id);
        ensure!(!approved.contains(&id), "duplicate approval");
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
    };
    refresh(&mut r)?;
    validate(&r)?;
    Ok(r)
}
