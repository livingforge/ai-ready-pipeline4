use super::*;

impl Workflow {
    pub(super) fn consume(&mut self) -> Result<bool> {
        let tasks = self.state.tasks.clone();
        if tasks.iter().any(|t| t.state != TaskState::Complete) {
            return Ok(false);
        }
        if tasks.is_empty() {
            return Ok(true);
        }
        eprintln!(
            "workflow: applying completed {:?} tasks (local validation; no provider call)",
            tasks[0].stage
        );
        ensure!(
            tasks.iter().all(|task| task.stage == tasks[0].stage
                || (task.stage.is_review() && tasks[0].stage.is_review())),
            "mixed task stages"
        );
        let consumed = match tasks[0].stage {
            Stage::Link => self.consume_link(&tasks)?,
            Stage::Extract => self.consume_extraction(&tasks)?,
            Stage::Repair => self.consume_repairs(&tasks)?,
            Stage::Restructure => self.consume_restructure(&tasks)?,
            Stage::Review | Stage::GlobalReview => self.consume_reviews(&tasks)?,
        };
        if consumed {
            self.state.tasks.clear();
            self.save()?;
        }
        Ok(consumed)
    }
    pub(super) fn consume_link(&mut self, tasks: &[Task]) -> Result<bool> {
        ensure!(tasks.len() == 1, "one atomic relationship task required");
        self.state.replies = self.linked_candidate(&tasks[0], &self.artifact(&tasks[0].reply)?)?;
        Ok(true)
    }
    pub(super) fn consume_extraction(&mut self, tasks: &[Task]) -> Result<bool> {
        let mut expanded = Vec::new();
        let mut needs_context = false;
        for task in tasks {
            let reply = self.artifact(&task.reply)?;
            if reply.get("request_tables").is_some() {
                expanded.push(self.expand_extraction(task, &reply)?);
                needs_context = true;
            } else {
                expanded.push(task.clone());
            }
        }
        if needs_context {
            self.queue(expanded)?;
            return Ok(false);
        }
        self.state.replies = self.extraction_candidate(tasks)?;
        self.state.incremental = Value::Null;
        Ok(true)
    }
    pub(super) fn consume_restructure(&mut self, tasks: &[Task]) -> Result<bool> {
        ensure!(tasks.len() == 1, "one atomic restructuring task required");
        let value = self.artifact(&tasks[0].reply)?;
        let next = self.restructure_candidate(&tasks[0], &value)?;
        if let Some(defer) = value.get("defer") {
            let context = self.artifact(&tasks[0].context)?;
            for finding in arr(&context["findings"])? {
                self.note_unresolved(json!({"code":"restructure_deferred",
                    "item":patch::fingerprint(finding),"diagnostic":finding,
                    "reason":defer["reason"],"kind":defer["kind"],
                    "task":tasks[0].id,"bases":tasks[0].bases,
                    "repair":{"action":"manual_triage"}}));
            }
            self.escalate("restructure_deferred");
            return Ok(true);
        }
        let limit = self.state.max_rounds;
        let changed: Vec<String> = obj(&next)?
            .keys()
            .filter(|doc| next[*doc] != self.state.replies[*doc])
            .cloned()
            .collect();
        if self.state.round >= limit {
            self.escalate("round_limit");
            self.state.tasks.clear();
            self.save()?;
            return Ok(true);
        }
        for doc in changed {
            let rounds = self.state.repair_rounds.get(&doc).copied().unwrap_or(0);
            self.state.repair_rounds.insert(doc.clone(), rounds + 1);
        }
        self.state.replies = next;
        self.state.round += 1;
        let context = self.artifact(&tasks[0].context)?;
        let handled = arr(&context["findings"])?;
        self.state.findings = json!(
            self.state
                .findings
                .as_array()
                .into_iter()
                .flatten()
                .filter(|finding| !handled
                    .iter()
                    .flat_map(findings::original_findings)
                    .any(|h| h == *finding))
                .cloned()
                .collect::<Vec<_>>()
        );
        Ok(true)
    }
    pub(super) fn consume_repairs(&mut self, tasks: &[Task]) -> Result<bool> {
        let mut expanded = Vec::new();
        let mut needs_context = false;
        for task in tasks {
            let value = self.artifact(&task.reply)?;
            if value.get("request_tables").is_some() {
                let base = self.artifact(&task.base)?;
                let scope = self.artifact(&task.scope)?;
                let packet = self.artifact(&task.packet)?;
                let tables = patch::expand(
                    &base,
                    &scope,
                    &packet,
                    &self.artifact(&task.context)?,
                    &value,
                )?;
                let ctx = patch::context(&base, &scope, &packet, &tables)?;
                expanded.push(self.task(Stage::Repair,patch::task_text(&scope).as_bytes(),json!({"document":task.document,"base":task.base,"scope":task.scope,"packet":task.packet,"context":self.store.put_json(&ctx)?}))?);
                needs_context = true;
            } else {
                expanded.push(task.clone());
            }
        }
        if needs_context {
            self.queue(expanded)?;
            return Ok(false);
        }
        let mut changes: BTreeMap<String, Vec<Value>> = BTreeMap::new();
        let mut seen = BTreeSet::new();
        for task in tasks {
            let doc = task.document.as_deref().context("task document required")?;
            ensure!(
                self.state.replies[doc].as_str() == task.base.as_deref(),
                "stale repair base"
            );
            let value = self.artifact(&task.reply)?;
            patch::apply(
                &self.artifact(&task.base)?,
                &self.artifact(&task.scope)?,
                &value,
            )?;
            for edit in arr(&value["changes"])? {
                ensure!(
                    seen.insert((doc.to_owned(), s(&edit["key"])?.to_owned())),
                    "overlapping repair tasks"
                );
                changes
                    .entry(doc.to_owned())
                    .or_default()
                    .push(edit.clone());
            }
        }
        // A repair that changes nothing states, with its reason, that the
        // findings in its scope cannot be fixed faithfully. Record them so
        // they are not reissued while the items stay as they are.
        for task in tasks {
            let value = self.artifact(&task.reply)?;
            if !arr(&value["changes"])?.is_empty() {
                continue;
            }
            let scope = self.artifact(&task.scope)?;
            let doc = task.document.as_deref().context("task document required")?;
            let reason = self
                .state
                .submissions
                .iter()
                .rev()
                .find(|entry| entry["task"] == task.id && entry["accepted"] == true)
                .and_then(|entry| entry["reason"].as_str())
                .unwrap_or("")
                .to_owned();
            let fingerprints: Map<String, Value> = arr(&scope["items"])?
                .iter()
                .map(|item| {
                    Ok((
                        format!("{doc}/{}", s(&item["key"])?),
                        json!(patch::fingerprint(item)),
                    ))
                })
                .collect::<Result<_>>()?;
            for finding in arr(&scope["findings"])? {
                let id = format!("{doc}/{}", s(&finding["key"])?);
                let mut entry = json!({"code":"repair_declined","document":doc,"reason":reason,"task":task.id,
                    "fingerprints":{&id:fingerprints[&id]},"repair":{"action":"manual_triage"}});
                if finding["code"] == "review_finding" {
                    entry["finding"] =
                        json!({"message":finding["message"],"items":[id],"action":"field_repair"});
                } else {
                    entry["diagnostic"] = json!({"code":finding["code"],"message":finding["message"],"fields":finding["fields"]});
                    entry["code"] = finding["code"].clone();
                    entry["item"] = json!(id);
                }
                self.note_unresolved(entry);
            }
        }
        let mut next = self.state.replies.clone();
        let mut next_rounds = self.state.repair_rounds.clone();
        for (doc, changes) in changes {
            let rounds = self.state.repair_rounds.get(&doc).copied().unwrap_or(0);
            ensure!(
                self.state.round < self.state.max_rounds,
                "repair round limit reached for this run"
            );
            let base = self.store.json(s(&next[&doc])?)?;
            let scope = json!({"base":patch::fingerprint(&base),"allowed_keys":arr(&base["items"] )?.iter().map(|i| i["key"].clone()).collect::<Vec<_>>()});
            let value = patch::apply(
                &base,
                &scope,
                &json!({"base":scope["base"],"changes":changes}),
            )?;
            next[&doc] = json!(self.store.put_json(&value)?);
            next_rounds.insert(doc.clone(), rounds + 1);
        }
        if next == self.state.replies {
            // An unchanged repair states that the rest cannot be fixed faithfully.
            self.escalate("repair_no_progress");
        } else {
            self.validate_replies(&next)?;
            self.state.replies = next;
            self.state.repair_rounds = next_rounds;
            self.state.round += 1;
        }
        Ok(true)
    }
    pub(super) fn consume_reviews(&mut self, tasks: &[Task]) -> Result<bool> {
        // Escalated findings stay open until a repair or a human resolves them.
        // Every accepted review is kept by packet: an unchanged packet whose
        // findings are all still open is not reviewed again.
        let mut findings = self.findings();
        let mut reported = Vec::new();
        let mut known: BTreeSet<String> = findings
            .iter()
            .map(finding_signature)
            .chain(self.unresolved().iter().map(Self::unresolved_signature))
            .collect();
        for task in tasks {
            let value = self.artifact(&task.reply)?;
            let value = self.confirmed_review(task, value)?;
            self.state.reviews[s(&value["packet"])?] = json!(self.store.put_json(&value)?);
            reported.extend(arr(&value["findings"])?.clone());
        }
        // A finding reported again with the same action and items is the
        // same finding worded by another reviewer: fold, do not duplicate.
        let mut folded = 0;
        for finding in &reported {
            if finding_known(finding, &known) {
                folded += 1;
            } else {
                known.insert(finding_signature(finding));
                findings.push(finding.clone());
            }
        }
        if folded > 0 {
            eprintln!("workflow: {folded} repeated review findings folded into existing records");
        }
        self.state.findings = json!(findings);
        self.state.review_findings = json!(reported);
        self.state.reviewed_replies = self.state.replies.clone();
        Ok(true)
    }
}
