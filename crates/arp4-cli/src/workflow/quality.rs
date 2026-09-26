//! Validation and progress tracking before adopting agent output.
use super::*;

pub fn diagnostic(error: &anyhow::Error) -> Value {
    if let Some(payload) = crate::data::rejection(error) {
        return payload.clone();
    }
    serde_json::from_str(&error.to_string()).unwrap_or_else(
        |_| json!({"code":"validation_or_provider_failure","message":format!("{error:#}")}),
    )
}

pub(super) fn validate_usage(value: &Value) -> Result<()> {
    if value.is_null() {
        return Ok(());
    }
    let errors = crate::semantic_contract::errors(
        &crate::semantic_contract::workflow_report_schemas()["usage"],
        value,
    )?;
    ensure!(errors.is_empty(), "invalid measured usage: {errors:?}");
    Ok(())
}

impl Workflow {
    pub(super) fn record_usage(
        &mut self,
        task: &str,
        submission: usize,
        measured: &Value,
    ) -> Result<()> {
        validate_usage(measured)?;
        ensure!(submission > 0, "submission must be one-based");
        ensure!(
            measured["reported_cost_usd"].is_number()
                || measured["usage"].as_object().is_some_and(|o| !o.is_empty()),
            "measured usage or cost required"
        );
        let id = self.recorded_task_id(task)?;
        let index = self
            .state
            .submissions
            .iter()
            .enumerate()
            .filter(|(_, e)| e["task"] == id)
            .nth(submission - 1)
            .map(|(i, _)| i)
            .context("unknown task submission")?;
        let mut entry = self.state.submissions[index].clone();
        ensure!(
            matches!(
                entry["origin"].as_str(),
                Some("external" | "interactive-agent")
            ),
            "provider usage is recorded by its run, not an external submission"
        );
        // Fill missing measurements only. Retrying the command is idempotent;
        // corrections or cumulative values must not silently double count usage.
        if let Some(cost) = measured.get("reported_cost_usd").filter(|v| !v.is_null()) {
            ensure!(
                entry["reported_cost_usd"].is_null() || entry["reported_cost_usd"] == *cost,
                "conflicting cost already recorded"
            );
            entry["reported_cost_usd"] = cost.clone();
        }
        if let Some(usage) = measured["usage"].as_object() {
            if entry["usage"].is_null() {
                entry["usage"] = json!({});
            }
            for (key, count) in usage {
                ensure!(
                    entry["usage"][key].is_null() || entry["usage"][key] == *count,
                    "conflicting token count already recorded: {key}"
                );
                entry["usage"][key] = count.clone();
            }
        }
        self.state.submissions[index] = entry;
        self.save()
    }

    pub(super) fn preview_quality(&self, task: &Task, reply: &Value) -> Result<Value> {
        let input = serde_json::from_value(self.store.json(&self.state.input)?)?;
        let (candidate, original) = match task.stage {
            Stage::Extract if reply.get("request_tables").is_none() => (reply.clone(), None),
            Stage::Repair if reply.get("request_tables").is_none() => {
                let base = self.artifact(&task.base)?;
                (
                    patch::apply(&base, &self.artifact(&task.scope)?, reply)?,
                    Some(base),
                )
            }
            _ => {
                return Ok(
                    json!({"checked":false,"reason":"No document field candidate in this reply"}),
                );
            }
        };
        let report = crate::semantic_contract::reply_report(&input, &candidate)?;
        let packet = self.artifact(&task.packet)?;
        let source_ids: BTreeSet<String> =
            crate::semantic::sources(&input, Some(s(&candidate["document"])?))?
                .iter()
                .enumerate()
                .filter(|(index, _)| {
                    packet["scope"]["sources"]
                        .as_array()
                        .is_none_or(|sources| sources.contains(&json!(format!("s{}", index + 1))))
                })
                .map(|(_, source)| source.id.clone())
                .collect();
        let summarize = |report: &crate::specifications::Report| -> Result<Value> {
            let rows: Vec<_> = arr(&report.coverage["matrix"])?
                .iter()
                .filter(|row| {
                    row["source"]
                        .as_str()
                        .is_some_and(|id| source_ids.contains(id))
                })
                .collect();
            Ok(
                json!({"sources":rows.len(),"characters":rows.iter().filter_map(|r|r["characters"].as_u64()).sum::<u64>(),
                "covered_characters":rows.iter().filter_map(|r|r["covered_characters"].as_u64()).sum::<u64>(),
                "uncovered":rows.iter().filter(|r|r["uncovered_characters"].as_u64().unwrap_or(0)>0).map(|r|json!({"source":r["source"],"ranges":r["uncovered_ranges"]})).collect::<Vec<_>>()}),
            )
        };
        let after = summarize(&report)?;
        let before = original
            .as_ref()
            .map(|base| -> Result<Value> {
                summarize(&crate::semantic_contract::reply_report(&input, base)?)
            })
            .transpose()?;
        let quantity_diagnostics: Vec<_> = report
            .issues
            .iter()
            .filter(|i| {
                i["code"]
                    .as_str()
                    .is_some_and(|code| code.contains("quantity"))
            })
            .collect();
        Ok(
            json!({"checked":true,"coverage":{"before":before,"after":after},
            "quantity":{"passed":quantity_diagnostics.is_empty(),"diagnostics":quantity_diagnostics},
            "independent_semantic_review":"required","meaning":"Coverage checks quoted non-whitespace characters, not completeness or correctness of claims."}),
        )
    }

    /// The `modules` a reply artifact declares, read once per artifact.
    fn reply_modules(&self, digest: &str) -> Result<Value> {
        if let Some(modules) = self.state.reply_modules.borrow().get(digest) {
            return Ok(modules.clone());
        }
        let modules = self.store.json(digest)?["modules"].clone();
        self.state
            .reply_modules
            .borrow_mut()
            .insert(digest.to_owned(), modules.clone());
        Ok(modules)
    }

    /// Index the modules of every live reply and forget retired ones, so the
    /// saved index covers exactly the replies the vocabulary reads.
    pub(super) fn index_reply_modules(&self) -> Result<()> {
        let mut live = BTreeSet::new();
        for digest in obj(&self.state.replies)?.values() {
            live.insert(s(digest)?);
        }
        for task in &self.state.tasks {
            if task.stage == Stage::Extract
                && task.state == TaskState::Complete
                && let Some(digest) = &task.reply
            {
                live.insert(digest);
            }
        }
        self.state
            .reply_modules
            .borrow_mut()
            .retain(|digest, _| live.contains(digest.as_str()));
        for digest in live {
            self.reply_modules(digest)?;
        }
        Ok(())
    }

    pub(super) fn module_vocabulary(&self) -> Result<Value> {
        let mut modules = self.state.modules.clone();
        for digest in obj(&self.state.replies)?.values() {
            for (slug, name) in obj(&self.reply_modules(s(digest)?)?)? {
                modules
                    .as_object_mut()
                    .unwrap()
                    .entry(slug.clone())
                    .or_insert(name.clone());
            }
        }
        for task in &self.state.tasks {
            if task.stage == Stage::Extract && task.state == TaskState::Complete {
                let reply =
                    self.reply_modules(task.reply.as_deref().context("task artifact required")?)?;
                if let Some(values) = reply.as_object() {
                    for (slug, name) in values {
                        modules
                            .as_object_mut()
                            .unwrap()
                            .entry(slug.clone())
                            .or_insert(name.clone());
                    }
                }
            }
        }
        Ok(modules)
    }

    pub(super) fn reply_schema(&self, task: &Task) -> Value {
        match task.stage {
            Stage::Extract => extraction::reply_schema(),
            Stage::Repair => patch::reply_schema(),
            Stage::Restructure => restructure::schema(),
            Stage::Link => linking::schema(),
            Stage::Review | Stage::GlobalReview => crate::semantic_contract::review_schema(),
        }
    }

    pub(super) fn validate_reply(&self, reply: &Value) -> Result<()> {
        let vocabulary = self.module_vocabulary()?;
        let key = hash(&encode(&json!([
            "reply",
            self.state.input,
            vocabulary,
            reply
        ])));
        if self.validation_cache.borrow().contains_key(&key) {
            return Ok(());
        }
        let input = serde_json::from_value(self.store.json(&self.state.input)?)?;
        let mut errors = crate::semantic_contract::diagnostics(&input, reply)?;
        if let (Some(modules), Some(reply_modules)) =
            (vocabulary.as_object(), reply["modules"].as_object())
        {
            for (slug, name) in reply_modules {
                if modules.get(slug).is_some_and(|known| known != name) {
                    errors.push(json!({"code":"module_vocabulary_mismatch","path":format!("/modules/{slug}"),"expected":modules[slug],"actual":name,
                        "next_action":"Read module_vocabulary again; another task may have added this slug. If the meaning matches, use the expected name; otherwise choose a distinct slug and update its item references. Do not mechanically remap unrelated modules."}));
                }
            }
        }
        if !errors.is_empty() {
            return Err(crate::data::Rejection(
                json!({"code":"reply_validation_failed","document":reply["document"],"diagnostics":errors,"next_action":"Correct the reported document and fields. For a pending/failed task, submit the corrected reply to that task. For an already saved extraction, use update --reply <corrected-extraction.json>. Do not reinitialize or edit state.json."}),
            )
            .into());
        }
        self.validation_cache.borrow_mut().insert(key, Value::Null);
        Ok(())
    }

    pub(super) fn validate_replies(&self, replies: &Value) -> Result<Value> {
        let key = hash(&encode(&json!([
            "replies",
            self.state.input,
            self.module_vocabulary()?,
            replies
        ])));
        if let Some(issues) = self.validation_cache.borrow().get(&key) {
            return Ok(issues.clone());
        }
        let input: crate::specifications::Input =
            serde_json::from_value(self.store.json(&self.state.input)?)?;
        let documents: BTreeSet<_> = input.sources.iter().map(|s| s.document.as_str()).collect();
        let mut values = Vec::new();
        for digest in obj(replies)?.values() {
            let reply = self.store.json(s(digest)?)?;
            self.validate_reply(&reply)?;
            values.push(serde_json::from_value(reply)?);
        }
        if values.len() != documents.len() {
            return Ok(json!([]));
        }
        let (model, _, _) = crate::semantic::assemble(&input, values, "workflow-validation")?;
        let issues =
            serde_json::to_value(crate::specifications::assess(&input, &model)?)?["issues"].clone();
        self.validation_cache
            .borrow_mut()
            .insert(key, issues.clone());
        Ok(issues)
    }

    pub(super) fn record_submission(
        &mut self,
        task: &Value,
        reply: &Value,
        attribution: &Value,
        accepted: bool,
        error: Option<String>,
    ) -> Result<()> {
        let mut entry = json!({"task":task["id"],"stage":task["stage"],"document":task["document"],"base":task["base"],
            "reply":self.store.put_json(reply)?,"actor":attribution["actor"],"model":attribution["model"],"reason":attribution["reason"],"origin":attribution["origin"],"usage":attribution["usage"],"reported_cost_usd":attribution["reported_cost_usd"],"accepted":accepted,"error":error});
        if let Some(correction) = attribution.get("correction") {
            entry["correction"] = correction.clone();
        }
        self.state.submissions.push(entry);
        Ok(())
    }

    /// Record diagnostic progress; true when the same diagnostics survived three waves.
    pub(super) fn quality_progress(&mut self, issues: &Value) -> Result<bool> {
        let mut signature: Vec<String> = arr(issues)?
            .iter()
            .filter(|i| i["code"] != "missing_semantic_audit")
            .map(|i| json!([i["code"], i["item"], i["items"], i["source"]]).to_string())
            .collect();
        if let Some(findings) = self.state.findings.as_array() {
            signature.extend(
                findings
                    .iter()
                    .map(|f| json!(["review", f["action"], f["items"]]).to_string()),
            );
        }
        signature.sort();
        let digest = hash(&encode(&json!(signature)));
        let previous = self.state.quality.clone();
        let round = self.state.round;
        let mut fingerprints = previous.get("fingerprints").cloned().unwrap_or(json!({}));
        let prior = fingerprints[&digest].clone();
        let count = if signature.is_empty() {
            0
        } else if prior["round"].as_u64() == Some(round) {
            prior["count"].as_u64().unwrap_or(1)
        } else {
            prior["count"].as_u64().unwrap_or(0) + 1
        };
        if !signature.is_empty() {
            fingerprints[&digest] = json!({"round":round,"count":count});
        }
        let repeated = count.saturating_sub(1);
        self.state.quality = json!({"fingerprints":fingerprints,"fingerprint":digest,"round":round,"issue_count":signature.len(),"unchanged_rounds":repeated,
            "previous_issue_count":previous["issue_count"],"diagnostics":self.store.put_json(issues)?});
        // One unchanged wave can be necessary for a multi-step correction. Three is a loop.
        Ok(repeated >= 3)
    }

    /// The quality record with its diagnostics, which are kept as an object: every
    /// command reads the workflow state, and they list every open issue.
    pub(super) fn quality_view(&self) -> Result<Value> {
        let mut quality = self.state.quality.clone();
        if let Some(digest) = quality["diagnostics"].as_str() {
            quality["diagnostics"] = self.store.json(digest)?;
        }
        Ok(quality)
    }

    pub(super) fn provenance(&self) -> Result<Value> {
        let mut provenance = self.provenance_counters();
        provenance["quality"] = self.quality_view()?;
        Ok(provenance)
    }

    fn provenance_counters(&self) -> Value {
        let runs = self.state.runs.clone();
        // fold from +0.0: an empty f64 sum would print as -0.0.
        let external_usage: Vec<_> = self
            .state
            .submissions
            .iter()
            .filter(|entry| {
                matches!(
                    entry["origin"].as_str(),
                    Some("external" | "interactive-agent")
                )
            })
            .collect();
        let records: Vec<_> = runs.iter().chain(external_usage.iter().copied()).collect();
        let measured = records
            .iter()
            .filter(|r| r["reported_cost_usd"].is_number())
            .count();
        let token_measured = records
            .iter()
            .filter(|r| r["usage"]["input_tokens"].is_u64() && r["usage"]["output_tokens"].is_u64())
            .count();
        let cost = records
            .iter()
            .filter_map(|r| r["reported_cost_usd"].as_f64())
            .fold(0.0, |total, cost| total + cost);
        let mut first = BTreeMap::new();
        for run in &runs {
            if let Some(task) = run["task"].as_str() {
                first.entry(task).or_insert(run["accepted"] == true);
            }
        }
        let first_accepted = first.values().filter(|accepted| **accepted).count();
        let external: Vec<_> = self
            .state
            .submissions
            .iter()
            .filter(|s| s["origin"] == "external" && s["accepted"] == true)
            .collect();
        json!({"first_response_acceptance":{"accepted":first_accepted,"tasks":first.len(),"rate":if first.is_empty(){None}else{Some(first_accepted as f64 / first.len() as f64)}},"failed_provider_calls":runs.iter().filter(|r|r["accepted"] == false).count(),"external_interventions":external.len(),"external_scopes":external.iter().map(|s|json!({"stage":s["stage"],"document":s["document"],"task":s["task"]})).collect::<Vec<_>>(),"provider_calls":runs.len(),"binary_hash":self.state.binary_hash,"input":self.state.input,"modules":self.state.modules,
            "interactive_agent_submissions":self.state.submissions.iter().filter(|s|s["origin"] == "interactive-agent" && s["accepted"] == true).count(),
            "submissions":self.state.submissions.clone(),"runs":runs,
            "reported_cost_usd":if measured == 0 {Value::Null} else {json!(cost)},
            "cost_complete":!records.is_empty() && measured == records.len(),
            "token_usage_complete":!records.is_empty() && token_measured == records.len(),
            "unmeasured_token_calls":records.len()-token_measured,
            "unmeasured_calls":records.len()-measured,
            "repair_rounds":self.state.repair_rounds,"deferred":self.state.deferred,"notices":self.state.notices,
            "evaluation":"External submissions are interventions, not evidence of an unattended provider-only run. Source audits record review activity, not proof of semantic completeness."})
    }

    /// Agent-facing history: provenance counters plus compact submission and run rows.
    /// Object digests stay in state.json; rejection text is shown as diagnostics.
    pub(super) fn history(&self) -> Result<Value> {
        let mut history = self.provenance()?;
        history["notices"] = self.notices();
        let mut numbers = BTreeMap::<String, usize>::new();
        let submissions: Vec<_> = self
            .state
            .submissions
            .iter()
            .map(|entry| {
                let mut row = json!({});
                if let Some(id) = entry["task"].as_str() {
                    row["task_ref"] = json!(self.short_ref(id));
                    let number = numbers.entry(id.to_owned()).or_default();
                    *number += 1;
                    row["submission"] = json!(*number);
                }
                for key in [
                    "stage",
                    "document",
                    "actor",
                    "model",
                    "reason",
                    "origin",
                    "accepted",
                    "usage",
                    "reported_cost_usd",
                ] {
                    if let Some(value) = entry.get(key).filter(|v| !v.is_null()) {
                        row[key] = value.clone();
                    }
                }
                if let Some(error) = entry["error"].as_str() {
                    row["diagnostics"] =
                        serde_json::from_str(error).unwrap_or_else(|_| json!({"message":error}));
                }
                row
            })
            .collect();
        let runs: Vec<_> = self
            .state
            .runs
            .iter()
            .map(|run| {
                let mut row =
                    json!({"task_ref":self.short_ref(run["task"].as_str().unwrap_or(""))});
                for key in [
                    "provider",
                    "model",
                    "accepted",
                    "failure_class",
                    "usage",
                    "reported_cost_usd",
                    "validation_elapsed_seconds",
                ] {
                    if let Some(value) = run.get(key).filter(|v| !v.is_null()) {
                        row[key] = value.clone();
                    }
                }
                row
            })
            .collect();
        history["submissions"] = json!(submissions);
        history["runs"] = json!(runs);
        for key in ["binary_hash", "input"] {
            history.as_object_mut().unwrap().remove(key);
        }
        Ok(history)
    }

    /// Counters only; hashes, module vocabulary and evaluation text belong to history.
    pub(super) fn provenance_summary(&self) -> Value {
        let full = self.provenance_counters();
        let mut summary = json!({"details_command":"history"});
        for key in [
            "provider_calls",
            "failed_provider_calls",
            "external_interventions",
            "interactive_agent_submissions",
            "reported_cost_usd",
            "cost_complete",
            "token_usage_complete",
            "unmeasured_token_calls",
            "unmeasured_calls",
        ] {
            summary[key] = full[key].clone();
        }
        summary
    }

    /// One entry per distinct action; tasks sharing it are listed by task_ref (first
    /// 20, with `count` for the rest). Diagnostics live on the task rows, not here.
    pub(super) fn next_actions(&self) -> Value {
        const LISTED_TASKS: usize = 20;
        let mut actions: Vec<(String, Vec<String>)> = Vec::new();
        let tasks = &self.state.tasks;
        let all_complete = tasks.iter().all(|task| task.state == TaskState::Complete);
        for task in tasks {
            let action = match task.state {
                TaskState::Running => "recover --task <task_ref>",
                TaskState::Pending | TaskState::Failed if task.draft.is_some() => {
                    "read --task <task_ref> --pointer draft; read --task <task_ref> --pointer previous_error; submit --task <task_ref> --reply <correction.json> (only changed fields; CLI merges and revalidates the whole reply)"
                }
                TaskState::Failed => {
                    "read --task <task_ref> --max-bytes 12000; validate-reply then submit --task <task_ref> --reply <corrected.json>, or retry --task <task_ref> --reason <reason> then run"
                }
                TaskState::Pending => {
                    "read --task <task_ref> --max-bytes 12000; validate-reply then submit --task <task_ref> --reply <reply.json> (or run for a configured provider)"
                }
                TaskState::Complete if all_complete => {
                    "advance (apply the saved reply locally; no provider call)"
                }
                TaskState::Complete if task.stage == Stage::Extract => {
                    "Saved; waiting for remaining tasks. To correct this extraction, use update --reply <corrected-extraction.json>."
                }
                TaskState::Complete => {
                    "Saved; waiting for remaining tasks before advance can apply this reply."
                }
            };
            let task_ref = self.short_id(task);
            match actions.iter_mut().find(|(known, _)| known == action) {
                Some((_, refs)) => refs.push(task_ref),
                None => actions.push((action.to_owned(), vec![task_ref])),
            }
        }
        let mut actions: Vec<Value> = actions
            .into_iter()
            .map(|(action, refs)| {
                let count = refs.len();
                json!({"action":action,"tasks":refs.into_iter().take(LISTED_TASKS).collect::<Vec<_>>(),"count":count})
            })
            .collect();
        if self.state.status == WorkflowStatus::NeedsDecision {
            actions.push(json!({"action":"Open issues remain after repair and independent review. Read draft/open-issues.json and draft/design.draft.md in the workflow root. To correct the extraction use update --reply <corrected-extraction.json> on the same root; otherwise the draft stands and formal export stays unavailable. Do not edit state.json."}));
        }
        if self.state.status == WorkflowStatus::Blocked && actions.is_empty() {
            actions.push(json!({"action":"Inspect blocked diagnostics and concerns; update --reply <corrected-extraction.json> or update --regions <regions.json>. Keep the same root. Do not edit state.json."}));
        }
        json!(actions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_validation_rejects_changed_input_reply_and_vocabulary() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(temp.path()).unwrap();
        let mut input = json!({"schema_version":1,"structure_requirements":{"doc":[]},"revisions":{"doc":"a".repeat(64)},"warnings":[],"sources":[{
            "id":hash(b"source"),"document":"doc","location":"/value","text":"enabled"}]});
        let input_hash = store.put_json(&input).unwrap();
        let typed = serde_json::from_value(input.clone()).unwrap();
        let reply = json!({"packet":crate::semantic::packet(&typed,"doc").unwrap()["packet"],"document":"doc","modules":{"app":"Application"},"items":[{
            "key":"a","name":"a","section":"process","subject":"a","property":"state","condition":{"basis":"unspecified"},"value":{"kind":"text","text":"enabled"},"statement":"enabled","evidence":["s1"],"classification":{"category":"specification","module":"app","requirements":[],"related":[]}}],"exclusions":[],"open_issues":[]});
        let replies = json!({"doc":store.put_json(&reply).unwrap()});
        let mut flow = Workflow {
            store,
            state: WorkflowState {
                input: input_hash,
                ..WorkflowState::test_state()
            },
            validation_cache: Default::default(),
        };
        let findings = flow.validate_replies(&replies).unwrap();
        assert_eq!(flow.validate_replies(&replies).unwrap(), findings);
        // Revalidate saved replies and identify their document, even when
        // validation was triggered by a different task's submission.
        let mut overlap = reply.clone();
        overlap["exclusions"] = json!([{"evidence":"s1","reason":"excluded"}]);
        let saved = json!({"doc":flow.store.put_json(&overlap).unwrap()});
        let error = flow.validate_replies(&saved).unwrap_err();
        let diagnostic = diagnostic(&error);
        assert_eq!(
            diagnostic["diagnostics"][0]["code"],
            "excluded_span_overlap"
        );
        assert_eq!(diagnostic["diagnostics"][0]["document"], "doc");
        let mut invalid = reply.clone();
        invalid["items"][0]["condition"] =
            json!({"basis":"stated","text":"invented","evidence":["s1"]});
        let invalid_replies = json!({"doc":flow.store.put_json(&invalid).unwrap()});
        assert!(flow.validate_replies(&invalid_replies).is_err());
        flow.state.modules = json!({"app":"Another application"});
        assert!(flow.validate_replies(&replies).is_err());
        flow.state.modules = json!({});
        input["sources"][0]["text"] = json!("changed");
        flow.state.input = flow.store.put_json(&input).unwrap();
        assert!(flow.validate_replies(&replies).is_err());
    }

    #[test]
    fn module_vocabulary_reads_each_live_reply_once() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(temp.path()).unwrap();
        let document = store
            .put_json(&json!({"modules":{"app":"Application"}}))
            .unwrap();
        let partition = store
            .put_json(&json!({"modules":{"db":"Database","app":"Other"}}))
            .unwrap();
        let retired = store.put_json(&json!({"modules":{"old":"Old"}})).unwrap();
        let task = |id: &str, reply: &str| json!({"task":"unused","id":id,"stage":"extract","state":"complete","reply":reply});
        let flow = Workflow {
            store,
            state: WorkflowState {
                replies: json!({"doc":document}),
                tasks: serde_json::from_value(json!([task("a", &partition), task("b", &retired)]))
                    .unwrap(),
                ..WorkflowState::test_state()
            },
            validation_cache: Default::default(),
        };
        let vocabulary = json!({"app":"Application","db":"Database","old":"Old"});
        assert_eq!(flow.module_vocabulary().unwrap(), vocabulary);
        let mut state = flow.state.clone();
        drop(flow);
        state.tasks.truncate(1);
        let flow = Workflow {
            store: Store::open(temp.path()).unwrap(),
            state,
            validation_cache: Default::default(),
        };
        flow.index_reply_modules().unwrap();
        let indexed: Vec<_> = flow.state.reply_modules.borrow().keys().cloned().collect();
        let mut live = vec![document.clone(), partition.clone()];
        live.sort();
        assert_eq!(indexed, live, "retired replies leave the index");
        // A later command answers from the saved index without reading the replies.
        for digest in &live {
            fs::remove_file(temp.path().join(format!("objects/{digest}.json"))).unwrap();
        }
        let saved = serde_json::to_value(&flow.state).unwrap();
        drop(flow);
        let flow = Workflow {
            store: Store::open(temp.path()).unwrap(),
            state: WorkflowState::decode(saved).unwrap(),
            validation_cache: Default::default(),
        };
        assert_eq!(
            flow.module_vocabulary().unwrap(),
            json!({"app":"Application","db":"Database"})
        );
    }

    #[test]
    fn advance_is_only_suggested_when_all_tasks_are_complete() {
        let temp = tempfile::tempdir().unwrap();
        let mut flow = Workflow {
            store: Store::open(temp.path()).unwrap(),
            state: WorkflowState {
                tasks: serde_json::from_value(json!([
                    {"task":"unused","id":"saved","stage":"extract","state":"complete"},
                    {"task":"unused","id":"remains","stage":"extract","state":"pending"}
                ]))
                .unwrap(),
                ..WorkflowState::test_state()
            },
            validation_cache: Default::default(),
        };
        let actions = flow.next_actions();
        assert!(actions[0]["action"].as_str().unwrap().contains("waiting"));
        assert!(!actions[0]["action"].as_str().unwrap().contains("advance"));
        assert_eq!(actions.as_array().unwrap().len(), 2);
        assert_eq!(actions[0]["tasks"], json!(["saved"]));
        assert_eq!(actions[1]["tasks"], json!(["remains"]));
        assert!(
            actions[1]["action"]
                .as_str()
                .unwrap()
                .contains("<task_ref>")
        );
        assert!(!flow.consume().unwrap());
        flow.state.tasks[1].state = TaskState::Complete;
        let actions = flow.next_actions();
        // Tasks sharing an action are grouped into one entry.
        assert_eq!(actions.as_array().unwrap().len(), 1);
        assert!(
            actions[0]["action"]
                .as_str()
                .unwrap()
                .starts_with("advance")
        );
        assert_eq!(actions[0]["tasks"], json!(["saved", "remains"]));
        assert_eq!(actions[0]["count"], 2);
        assert!(actions[0].get("error").is_none());
    }

    #[test]
    fn partitions_defer_links_while_repairs_and_restructure_allow_them() {
        let full = crate::semantic_contract::reply_schema();
        let repair = patch::reply_schema();
        for schema in [repair.clone(), restructure::schema()] {
            assert_eq!(
                schema["$defs"]["classification"],
                full["$defs"]["classification"]
            );
            jsonschema::validator_for(&schema).unwrap();
        }
        let extraction = extraction::reply_schema();
        for field in ["requirements", "related"] {
            let schema = &extraction["$defs"]["classification"]["properties"][field];
            assert_eq!(schema["maxItems"], 0);
        }
        jsonschema::validator_for(&extraction).unwrap();
        for field in ["requirements", "related"] {
            let path = format!("/classification/{field}");
            let mut change = json!({"base":"base","changes":[{"key":"a","set":{}}]});
            change["changes"][0]["set"][&path] = json!([]);
            assert!(
                crate::semantic_contract::errors(&repair, &change)
                    .unwrap()
                    .is_empty()
            );
            change["changes"][0]["set"][&path] = Value::Null;
            assert!(
                !crate::semantic_contract::errors(&repair, &change)
                    .unwrap()
                    .is_empty()
            );
        }
    }
}
