use super::*;

pub(super) struct PlanningContext {
    pub(super) input_path: PathBuf,
    pub(super) inputs: Value,
    pub(super) typed_input: crate::specifications::Input,
    pub(super) documents: BTreeSet<String>,
    pub(super) config: Value,
    pub(super) packets: BTreeMap<String, Value>,
    pub(super) bases: BTreeMap<String, Value>,
    pub(super) partitions: BTreeMap<String, Vec<Value>>,
}

pub(super) struct ReviewPlan {
    pub(super) tasks: Vec<Task>,
    pub(super) accepted: Vec<PathBuf>,
    pub(super) unreviewed: bool,
}

impl Workflow {
    pub(super) fn prepare_extraction(
        &mut self,
        root: &Path,
    ) -> Result<(PlanningContext, Vec<Task>)> {
        let input_path = root.join("input.json");
        self.store.materialize(&self.state.input, &input_path)?;
        let inputs = read(&input_path)?;
        self.state.notices = inputs.get("warnings").cloned().unwrap_or(json!([]));
        let documents: BTreeSet<String> = arr(&inputs["sources"])?
            .iter()
            .map(|v| s(&v["document"]).map(str::to_owned))
            .collect::<Result<_>>()?;
        let config = self.store.json(&self.state.regions)?;
        ensure!(
            obj(&config)?
                .keys()
                .chain(obj(&self.state.replies)?.keys())
                .all(|d| documents.contains(d)),
            "unknown document in regions or replies"
        );
        let mut tasks = Vec::new();
        let mut packets = BTreeMap::new();
        let mut bases = BTreeMap::new();
        let mut partitions = BTreeMap::new();
        let typed_input = crate::specifications::load_input(&input_path)?;
        for doc in &documents {
            let packet = crate::semantic::packet(&typed_input, doc)?;
            partitions.insert(doc.clone(), patch::regions(&config, doc, &packet)?);
            let digest = self.store.put_json(&packet)?;
            self.state.packets[doc] = json!(digest);
            if let Some(reply) = self.state.replies.get(doc) {
                let base = self.store.json(s(reply)?)?;
                ensure!(
                    base["packet"] == packet["packet"] && base["document"] == *doc,
                    "stale extraction reply"
                );
                bases.insert(doc.clone(), base);
            } else if let Some(task) = self.incremental_task(doc, &packet)? {
                tasks.push(task);
            } else {
                let regions = extraction::partitions(
                    &packet,
                    &partitions[doc],
                    self.state.extract_max_chars,
                    self.state.extract_max_sources,
                    self.state.extract_max_bytes,
                    &self.state.modules,
                )?;
                if regions.is_empty() {
                    let text = extraction::document_task_text(&packet, &self.state.modules);
                    tasks.push(self.task(
                        Stage::Extract,
                        text.as_bytes(),
                        json!({"document":doc,"packet":digest}),
                    )?);
                } else {
                    for (index, region) in regions.iter().enumerate() {
                        let scoped =
                            extraction::scoped_packet(&packet, region, &self.state.modules)?;
                        let text = extraction::task_text(&scoped);
                        tasks.push(self.task(Stage::Extract,text.as_bytes(),json!({"document":doc,"packet":self.store.put_json(&scoped)?,"partition":index}))?);
                    }
                }
            }
            packets.insert(doc.clone(), packet);
        }
        Ok((
            PlanningContext {
                input_path,
                inputs,
                typed_input,
                documents,
                config,
                packets,
                bases,
                partitions,
            },
            tasks,
        ))
    }
    pub(super) fn assemble_bundle(
        &mut self,
        root: &Path,
        context: &PlanningContext,
    ) -> Result<Value> {
        let PlanningContext {
            input_path,
            documents,
            ..
        } = context;
        let assembled = root.join("assembled");
        let mut reply_paths = Vec::new();
        for (i, doc) in documents.iter().enumerate() {
            let path = root.join(format!("reply-{i}.json"));
            self.store
                .materialize(s(&self.state.replies[doc])?, &path)?;
            reply_paths.push(path);
        }
        crate::semantic_operations::assemble(
            input_path,
            &reply_paths,
            "semantic-workflow",
            &assembled,
        )?;
        self.state.bundle = self.collect(&assembled)?;
        let report = read(&assembled.join("check.json"))?;
        Ok(report)
    }
    pub(super) fn plan_repairs(
        &mut self,
        context: &PlanningContext,
        report: &Value,
        findings: &Value,
    ) -> Result<Vec<Task>> {
        let PlanningContext {
            bases,
            packets,
            config,
            ..
        } = context;
        let mut tasks = Vec::new();
        let declined: BTreeSet<String> = self
            .unresolved()
            .iter()
            .map(Self::unresolved_signature)
            .collect();
        let (repairs, planned_deferred) = patch::plan(
            bases,
            packets,
            config,
            arr(&report["issues"])?,
            arr(findings)?,
            &declined,
        )?;
        // Review findings for a human (original inspection, manual triage)
        // stay deferred and visible: reviewers see them as known and the
        // draft lists them. Dropping them made every round rediscover them.
        // Machine markers that only say "review pending" are not findings.
        let deferred: Vec<Value> = planned_deferred
            .into_iter()
            .filter(|d| {
                d["code"] == "review_finding"
                    || d["code"] == "cross_document_finding"
                    || (d["repair"]["action"] != "review"
                        && d["repair"]["action"] != "original_inspection")
            })
            .collect();
        if self.state.round >= self.state.max_rounds {
            self.state.deferred = json!(deferred);
            self.escalate("round_limit");
            return Ok(tasks);
        }
        // When structural work is necessary, fix the currently known field
        // defects in the same atomic candidate instead of paying for two waves.
        let mut combined = deferred.clone();
        for repair in &repairs {
            combined.push(json!({"code":"grouped_field_repairs","document":repair["document"],
                "findings":repair["scope"]["findings"],
                "repair":{"action":"field_repair","items":arr(&repair["scope"]["allowed_keys"])? .iter()
                    .map(|key| format!("{}/{}", repair["document"].as_str().unwrap(),key.as_str().unwrap())).collect::<Vec<_>>()}}));
        }
        if let Some(task) = self.restructure_task(&combined)? {
            self.state.deferred = json!(deferred);
            return Ok(vec![task]);
        }
        for repair in repairs {
            let doc = s(&repair["document"])?;
            tasks.push(self.task(Stage::Repair,s(&repair["text"] )?.as_bytes(),json!({"document":doc,"base":self.state.replies[doc],"packet":self.state.packets[doc],
                    "scope":self.store.put_json(&repair["scope"])?,"context":self.store.put_json(&repair["context"])?}))?);
        }
        self.state.deferred = json!(deferred);
        if !tasks.is_empty() {
            self.state.findings = json!(
                arr(findings)?
                    .iter()
                    .filter(|f| f["action"] == "improvement")
                    .chain(deferred.iter().flat_map(findings::original_findings))
                    .collect::<Vec<_>>()
            );
            return Ok(tasks);
        }
        if !deferred.is_empty() {
            // What repair cannot fix goes to independent review, not to a halt.
            self.state.findings = findings.clone();
            self.state.deferred = json!(deferred);
            self.escalate("unrepairable");
        }
        Ok(tasks)
    }
    pub(super) fn plan_reviews(
        &self,
        root: &Path,
        context: &PlanningContext,
        open: &[Value],
        findings: &Value,
        unresolved: bool,
    ) -> Result<ReviewPlan> {
        let PlanningContext {
            inputs,
            typed_input,
            documents,
            partitions,
            packets,
            ..
        } = context;
        let assembled = root.join("assembled");
        let mut tasks = Vec::new();
        // Reviewers see what repair could not resolve, limited to their document.
        let escalated_context = |doc: Option<&String>| -> Result<Value> {
            let mut context = json!({"escalation":{"reason":self.state.escalation["reason"],
                "guidance":ESCALATION_GUIDANCE,"concerns":self.concerns(doc.map(String::as_str), open)}});
            if !self.reference_paths().is_empty() {
                context["references"] =
                    json!({"guidance":REFERENCE_GUIDANCE,"documents":self.references()?});
            }
            Ok(context)
        };
        // Reviewing an unchanged model again cannot resolve what is still open.
        let reviewed_before = self.state.reviewed_replies == self.state.replies;
        let mut unreviewed = false;
        let mut units: Vec<(Option<String>, Option<String>, Option<Value>)> = Vec::new();
        for doc in documents {
            if !partitions[doc].is_empty() {
                units.extend(
                    partitions[doc]
                        .iter()
                        .map(|r| (Some(doc.clone()), None, Some(r.clone()))),
                );
                continue;
            }
            let automatic = incremental::review_regions(&packets[doc])?;
            if !automatic.is_empty() && self.state.review_granularity != ReviewGranularity::Document
            {
                units.extend(
                    automatic
                        .into_iter()
                        .map(|region| (Some(doc.clone()), None, Some(region))),
                );
                continue;
            }
            let source_rows: Vec<_> = arr(&inputs["sources"])?
                .iter()
                .filter(|v| v["document"] == *doc)
                .collect();
            let source_bytes: usize = source_rows
                .iter()
                .filter_map(|v| v["text"].as_str())
                .map(str::len)
                .sum();
            let granularity = self.state.review_granularity;
            let use_document = granularity == ReviewGranularity::Document
                || (granularity == ReviewGranularity::Adaptive
                    && source_rows.len() <= MAX_ADAPTIVE_DOCUMENT_REVIEW_SOURCES
                    && source_bytes <= MAX_ADAPTIVE_DOCUMENT_REVIEW_BYTES);
            if use_document {
                units.push((Some(doc.clone()), None, None));
                continue;
            }
            let sheets: BTreeSet<Option<String>> = arr(&inputs["sources"])?
                .iter()
                .filter(|v| v["document"] == *doc)
                .map(|v| v["context"]["sheet"].as_str().map(str::to_owned))
                .collect();
            if sheets.contains(&None) {
                units.push((Some(doc.clone()), None, None));
            } else {
                units.extend(
                    sheets
                        .into_iter()
                        .map(|sheet| (Some(doc.clone()), sheet, None)),
                );
            }
        }
        units.push((None, None, None));
        if let Some(plan) = &self.state.review_plan {
            plan.validate(typed_input)?;
            let mut selected = Vec::new();
            for (doc, sheet, region) in units {
                let Some(document) = doc.as_deref() else {
                    selected.push((doc, sheet, region));
                    continue;
                };
                let rows = crate::semantic::sources(typed_input, Some(document))?;
                let mut targets = Vec::new();
                let mut context: BTreeSet<String> = region
                    .as_ref()
                    .and_then(|r| r["context"].as_array())
                    .into_iter()
                    .flatten()
                    .filter_map(|s| s.as_str().map(str::to_owned))
                    .collect();
                for (i, source) in rows.iter().enumerate() {
                    let alias = format!("s{}", i + 1);
                    let in_region = region.as_ref().is_none_or(|r| {
                        r["sources"]
                            .as_array()
                            .is_some_and(|a| a.contains(&json!(alias)))
                    });
                    let in_sheet = sheet
                        .as_ref()
                        .is_none_or(|s| source.context.as_ref().is_some_and(|c| &c.sheet == s));
                    if !in_region || !in_sheet {
                        continue;
                    }
                    if plan.omitted_sources.contains_key(&source.id) {
                        context.insert(alias);
                    } else {
                        targets.push(alias);
                    }
                }
                if !targets.is_empty() {
                    selected.push((
                        doc,
                        None,
                        Some(json!({"sources":targets,"context":context})),
                    ));
                }
            }
            units = selected;
        }
        let previous = self.previous_review_model(root, &context.input_path)?;
        let mut review_paths = Vec::new();
        for (i, (doc, sheet, region)) in units.iter().enumerate() {
            let scope = region
                .as_ref()
                .map(|r| -> Result<crate::semantic::ReviewScope> {
                    Ok(crate::semantic::ReviewScope {
                        sources: serde_json::from_value(r["sources"].clone())?,
                        context: serde_json::from_value(r["context"].clone())?,
                    })
                })
                .transpose()?;
            let packet = crate::semantic::review_packet_selected(
                typed_input,
                &crate::specifications::load_model(&assembled.join("model.json"))?,
                &serde_json::from_value(read(&assembled.join("catalog.json"))?)?,
                doc.as_deref(),
                sheet.as_deref(),
                scope.as_ref(),
            )?;
            let mut review_context = escalated_context(doc.as_ref())?;
            if !self
                .state
                .reviews
                .as_object()
                .is_some_and(|r| r.contains_key(packet["packet"].as_str().unwrap()))
                && let Some((model, catalog)) = &previous
            {
                let prior = crate::semantic::review_packet_selected(
                    typed_input,
                    model,
                    catalog,
                    doc.as_deref(),
                    sheet.as_deref(),
                    scope.as_ref(),
                )?;
                if let Some(digest) = self.state.reviews.get(s(&prior["packet"])?) {
                    let review = self.store.json(s(digest)?)?;
                    review_context["confirmation"] = confirmation::plan(&prior, &packet, &review)?;
                }
            }
            let packet = crate::semantic::compact_review(&packet);
            let text = format!(
                "{}\n\n{}\n",
                crate::semantic::REVIEW_PROMPT,
                serde_json::to_string(&packet)?
            );
            let mut meta = json!({"document":doc,"packet":self.store.put_json(&packet)?});
            if unresolved
                || !self.reference_paths().is_empty()
                || review_context.get("confirmation").is_some()
            {
                meta["context"] = json!(self.store.put_json(&review_context)?);
            }
            let task = self.task(
                if doc.is_some() {
                    Stage::Review
                } else {
                    Stage::GlobalReview
                },
                text.as_bytes(),
                meta,
            )?;
            let cached = self
                .state
                .reviews
                .get(s(&packet["packet"])?)
                .map(|digest| {
                    Ok::<_, anyhow::Error>((digest.clone(), self.store.json(s(digest)?)?))
                })
                .transpose()?;
            let known: BTreeSet<String> = arr(findings)?
                .iter()
                .map(finding_signature)
                .chain(self.unresolved().iter().map(Self::unresolved_signature))
                .collect();
            match cached {
                Some((digest, review))
                    if arr(&review["findings"])?
                        .iter()
                        .all(|f| f["action"] == "improvement") =>
                {
                    self.validate(&task, &review)?;
                    let path = root.join(format!("accepted-{i}.json"));
                    self.store.materialize(s(&digest)?, &path)?;
                    review_paths.push(path);
                }
                // The packet is unchanged and every finding it produced is still
                // open: another review of it can only repeat those findings.
                Some((_, review))
                    if arr(&review["findings"])?
                        .iter()
                        .all(|f| finding_known(f, &known)) =>
                {
                    unreviewed = true;
                }
                _ if unresolved && reviewed_before => unreviewed = true,
                _ => tasks.push(task),
            }
        }
        Ok(ReviewPlan {
            tasks,
            accepted: review_paths,
            unreviewed,
        })
    }
    pub(super) fn publish_reviewed(
        &mut self,
        root: &Path,
        input_path: &Path,
        review_paths: &[PathBuf],
        unresolved: bool,
    ) -> Result<Value> {
        let assembled = root.join("assembled");
        let model = assembled.join("model.json");
        let catalog = assembled.join("catalog.json");
        let reviewed = root.join("reviewed.json");
        let review_report = crate::semantic_operations::apply_reviews(
            input_path,
            &model,
            &catalog,
            review_paths,
            "independent-semantic-review",
            &reviewed,
            self.state.review_plan.clone(),
        )?;
        if unresolved {
            return self.needs_decision(root, input_path, &reviewed, true);
        }
        ensure!(
            review_report.ready,
            "reviewed model is not ready for publication"
        );
        let finalized = root.join("finalized");
        crate::semantic_operations::finalize(
            input_path,
            &reviewed,
            &catalog,
            &assembled.join("identity-plan.json"),
            &finalized,
            None,
            None,
        )?;
        let design = root.join("design.md");
        crate::semantic_operations::render(
            input_path,
            &finalized.join("model.json"),
            &design,
            false,
        )?;
        self.state.exports = self.collect(&finalized)?;
        let mut design_text = fs::read_to_string(design)?;
        design_text.push_str("\n\n## Generation provenance\n\nSee provenance.json for provider calls, external interventions, reported cost and unresolved source warnings. Review records are not proof of semantic completeness.\n");
        self.state.exports["design.md"] = json!(self.store.put(design_text.as_bytes())?);
        let improvements: Vec<_> = self
            .findings()
            .into_iter()
            .filter(|f| f["action"] == "improvement")
            .collect();
        self.state.exports["improvements.json"] = json!(self.store.put_json(&json!({"findings":improvements,
            "reason":"Optional refinements; no demonstrated downstream error. Reconsider when intended use or relevant source conditions change."}))?);
        self.state.exports["provenance.json"] = json!(self.store.put_json(&self.provenance())?);
        self.state.status = WorkflowStatus::Complete;
        self.state.blocked = json!([]);
        self.state.tasks.clear();
        self.save()?;
        self.export()?;
        self.compact()?;
        Ok(self.status())
    }
}
