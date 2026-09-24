use super::*;

impl Workflow {
    pub(super) fn validate(&self, task: &Task, value: &Value) -> Result<()> {
        obj(value)?;
        let schema = if task.stage == Stage::Extract && value.get("request_tables").is_none() {
            crate::semantic_contract::reply_schema()
        } else if task.stage == Stage::Repair {
            let full = self.reply_schema(task);
            let mut branch =
                full["oneOf"][usize::from(value.get("request_tables").is_some())].clone();
            branch["$defs"] = full["$defs"].clone();
            branch
        } else {
            self.reply_schema(task)
        };
        let errors = crate::semantic_contract::errors(&schema, value)?;
        if !errors.is_empty() {
            return Err(crate::data::Rejection(
                json!({"code":"reply_validation_failed","diagnostics":errors}),
            )
            .into());
        }
        if task.stage == Stage::Extract && value.get("request_tables").is_some() {
            self.expand_extraction(task, value)?;
            return Ok(());
        }
        if task.stage == Stage::Restructure {
            self.restructure_candidate(task, value)?;
            return Ok(());
        }
        if task.stage == Stage::Link {
            self.linked_candidate(task, value)?;
            return Ok(());
        }
        if task.stage == Stage::Repair {
            let base = self.artifact(&task.base)?;
            let scope = self.artifact(&task.scope)?;
            if value.get("request_tables").is_some() {
                patch::expand(
                    &base,
                    &scope,
                    &self.artifact(&task.packet)?,
                    &self.artifact(&task.context)?,
                    value,
                )?;
            } else {
                let candidate = patch::apply(&base, &scope, value)?;
                self.validate_reply(&candidate)?;
                let mut next = self.state.replies.clone();
                next[s(&candidate["document"])?] = json!(self.store.put_json(&candidate)?);
                self.validate_replies(&next)?;
            }
            return Ok(());
        }
        let packet = self.artifact(&task.packet)?;
        ensure!(
            value["packet"] == packet["packet"] && value["document"] == packet["document"],
            "reply packet/document mismatch"
        );
        if task.stage == Stage::Extract {
            extraction::validate_scope(value, &packet)?;
            let mut full = value.clone();
            if let Some(parent) = packet.get("parent_packet") {
                full["packet"] = parent.clone();
            }
            self.validate_reply(&full)?;
            let mut pending = self.state.tasks.clone();
            for candidate in &mut pending {
                if candidate.id == task.id {
                    candidate.state = TaskState::Complete;
                    candidate.reply = Some(self.store.put_json(value)?);
                }
            }
            if pending.iter().all(|t| t.state == TaskState::Complete)
                && pending.iter().all(|t| {
                    self.artifact(&t.reply)
                        .is_ok_and(|r| r.get("request_tables").is_none())
                })
            {
                self.extraction_candidate(&pending)?;
            }
        }
        if task.stage == Stage::Review || task.stage == Stage::GlobalReview {
            ensure!(
                obj(value)?.keys().all(|k| [
                    "packet", "document", "sheet", "scope", "audits", "findings"
                ]
                .contains(&k.as_str()))
                    && value["sheet"] == packet["sheet"]
                    && value["scope"] == packet["scope"],
                "invalid review shape or scope"
            );
            let findings = arr(&value["findings"])?;
            let mut expected = BTreeSet::new();
            if let Some(rows) = packet["sources"]["rows"].as_array() {
                for r in rows {
                    expected.insert(s(&r[0])?.to_owned());
                }
            }
            let confirmation = self.confirmation_context(task)?;
            if let Some(required) = confirmation["required_sources"].as_array() {
                expected = required
                    .iter()
                    .map(|v| s(v).map(str::to_owned))
                    .collect::<Result<_>>()?;
            }
            let mut seen = BTreeSet::new();
            for group in arr(&value["audits"])? {
                ensure!(
                    obj(group)?.len() == 2
                        && !s(&group["rationale"])?.trim().is_empty()
                        && !arr(&group["sources"])?.is_empty(),
                    "invalid source audit"
                );
                for source in arr(&group["sources"])? {
                    let source = s(source)?.to_owned();
                    ensure!(
                        expected.contains(&source) && seen.insert(source),
                        "duplicate or out-of-scope audit"
                    );
                }
            }
            ensure!(
                findings.iter().any(|f| f["action"] != "improvement") || seen == expected,
                "successful review must audit every source"
            );
        }
        Ok(())
    }
    pub(super) fn submit(&mut self, id: &str, value: &Value, attribution: &Value) -> Result<()> {
        let i = self.index(id)?;
        ensure!(
            matches!(
                self.state.tasks[i].state,
                TaskState::Pending | TaskState::Failed
            ),
            "task already complete or running; recover running tasks first"
        );
        let task = self.state.tasks[i].clone();
        let submitted = value;
        let value = &self.complete_reply(&task, value.clone())?;
        self.check_draft_identity(&task, value)?;
        let mut attribution = attribution.clone();
        if submitted.get("draft").is_some() {
            attribution["correction"] = json!(self.store.put_json(submitted)?);
        }
        let validation = self.validate(&task, value);
        self.record_submission(
            &json!(task),
            value,
            &attribution,
            validation.is_ok(),
            validation.as_ref().err().map(|e| format!("{e:#}")),
        )?;
        if let Err(error) = validation {
            self.state.tasks[i].rejected_reply = Some(self.store.put_json(value)?);
            self.retain_draft(i, value)?;
            self.register_transport_refs()?;
            let mut diagnostics = quality::diagnostic(&error);
            diagnostics["draft"] = self.draft_info(&self.state.tasks[i])?["revision"].clone();
            diagnostics["next_action"] = json!(
                "Read draft and previous_error; submit only changed fields with the current draft revision, or submit a complete corrected reply. The draft is not accepted; the entire merged reply will be checked."
            );
            let error: anyhow::Error = crate::data::Rejection(diagnostics.clone()).into();
            self.state.tasks[i].error = Some(format!("{error:#}"));
            self.state.tasks[i].diagnostics = diagnostics;
            self.save()?;
            return Err(error);
        }
        self.state.tasks[i].reply = Some(self.store.put_json(value)?);
        self.state.tasks[i].state = TaskState::Complete;
        self.clear_task_failure(i);
        if task.stage == Stage::Extract && value.get("request_tables").is_none() {
            self.state.extraction_cache[id] = json!(self.state.tasks[i].reply);
        }
        self.save()
    }
}
