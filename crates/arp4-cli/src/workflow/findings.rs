use super::*;

pub(super) fn original_findings(entry: &Value) -> Vec<&Value> {
    entry
        .get("finding")
        .into_iter()
        .chain(
            entry["findings"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|finding| finding.get("finding")),
        )
        .collect()
}

impl Workflow {
    /// One view of open concerns for status, reviewers and decision drafts.
    /// Storage provenance stays separate; being absent from `unresolved` never
    /// makes a reviewer's manual-triage finding disappear from this view.
    pub(super) fn concerns(&self, document: Option<&str>, diagnostics: &[Value]) -> Vec<Value> {
        fn add(
            rows: &mut BTreeMap<String, Value>,
            value: &Value,
            finding: bool,
            decision: bool,
            reason: Option<&str>,
        ) {
            if value["code"] == "missing_semantic_audit" {
                return;
            }
            let originals = original_findings(value);
            if !originals.is_empty() {
                for original in originals {
                    add(rows, original, true, decision, reason);
                }
                return;
            }
            if let Some(diagnostic) = value.get("diagnostic") {
                let mut diagnostic = diagnostic.clone();
                for key in ["item", "document"] {
                    if diagnostic.get(key).is_none()
                        && let Some(v) = value.get(key)
                    {
                        diagnostic[key] = v.clone();
                    }
                }
                add(rows, &diagnostic, false, decision, reason);
                return;
            }
            let action = if finding {
                &value["action"]
            } else {
                &value["repair"]["action"]
            };
            let status = if action == "improvement" {
                "improvement"
            } else if decision
                || ["manual_triage", "original_inspection", "validator_support"]
                    .iter()
                    .any(|a| action == *a)
            {
                "needs_decision"
            } else {
                "open"
            };
            let key = if finding {
                finding_signature(value)
            } else {
                if let Some(item) = value["item"].as_str() {
                    diagnostic_signature(&value["code"], item)
                } else {
                    json!([
                        value["code"],
                        value["repair"]["items"],
                        value["source"],
                        value["document"]
                    ])
                    .to_string()
                }
            };
            let field = if finding { "finding" } else { "diagnostic" };
            let row = rows.entry(key.clone()).or_insert_with(
                || json!({"id":hash(key.as_bytes()),"status":status,field:value,"reasons":[]}),
            );
            if status == "needs_decision" {
                row["status"] = json!(status);
            }
            if let Some(reason) = reason.filter(|s| !s.is_empty()) {
                let reasons = row["reasons"].as_array_mut().unwrap();
                if !reasons.contains(&json!(reason)) {
                    reasons.push(json!(reason));
                }
            }
        }
        let mut rows = BTreeMap::new();
        let terminal = self.state.status == WorkflowStatus::NeedsDecision
            || self.state.escalation["reason"] == "round_limit";
        for finding in self.findings() {
            add(&mut rows, &finding, true, terminal, None);
        }
        for diagnostic in diagnostics {
            add(&mut rows, diagnostic, false, terminal, None);
        }
        for deferred in self.state.deferred.as_array().into_iter().flatten() {
            let action = &deferred["repair"]["action"];
            // A scoped restructuring is still executable even if its wrapper
            // originated as a non-field-repair review finding.
            let structural = deferred["finding"]["action"] == "scoped_restructure";
            let decision =
                !structural && action != "scoped_restructure" && action != "field_repair";
            add(
                &mut rows,
                deferred,
                false,
                terminal || decision,
                deferred["reason"].as_str(),
            );
        }
        for unresolved in self.unresolved() {
            add(
                &mut rows,
                &unresolved,
                false,
                true,
                unresolved["reason"].as_str(),
            );
        }
        rows.into_values()
            .filter(|row| {
                let Some(document) = document else {
                    return true;
                };
                let value = row.get("finding").unwrap_or(&row["diagnostic"]);
                let mut docs = BTreeSet::new();
                if let Some(doc) = value["document"].as_str() {
                    docs.insert(doc.to_owned());
                }
                for id in value["items"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .chain(value["repair"]["items"].as_array().into_iter().flatten())
                    .chain(value.get("item"))
                {
                    if let Some((doc, _)) = id.as_str().and_then(|id| id.split_once('/')) {
                        docs.insert(doc.to_owned());
                    }
                }
                for doc in value["repair"]["documents"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                {
                    docs.insert(doc.to_owned());
                }
                docs.is_empty() || docs.contains(document)
            })
            .collect()
    }

    pub(super) fn findings(&self) -> Vec<Value> {
        self.state.findings.as_array().cloned().unwrap_or_default()
    }
    /// Findings and diagnostics a repair returned without changes, with the
    /// submitter's reason. They are not reissued as repair tasks and stay listed
    /// until the affected items change or a human corrects the extraction.
    pub(super) fn unresolved(&self) -> Vec<Value> {
        self.state
            .unresolved
            .as_array()
            .cloned()
            .unwrap_or_default()
    }
    /// Identity of an entry in `unresolved`: a review finding by its signature,
    /// a machine diagnostic by code and item.
    pub(super) fn unresolved_signature(entry: &Value) -> String {
        if let Some(finding) = entry.get("finding") {
            finding_signature(finding)
        } else {
            diagnostic_signature(&entry["code"], entry["item"].as_str().unwrap_or(""))
        }
    }
    pub(super) fn note_unresolved(&mut self, entry: Value) {
        let signature = Self::unresolved_signature(&entry);
        let mut list = self.unresolved();
        if let Some(existing) = list
            .iter_mut()
            .find(|e| Self::unresolved_signature(e) == signature)
        {
            existing["reports"] = json!(existing["reports"].as_u64().unwrap_or(1) + 1);
            existing["reason"] = entry["reason"].clone();
        } else {
            list.push(entry);
        }
        self.state.unresolved = json!(list);
    }
    /// A declined record is void once any of its items changed: the next review
    /// judges the new content, and repair may be asked again.
    pub(super) fn prune_unresolved(&mut self, bases: &BTreeMap<String, Value>) -> Result<()> {
        let mut fingerprints = BTreeMap::new();
        for (doc, base) in bases {
            for item in arr(&base["items"])? {
                fingerprints.insert(
                    format!("{doc}/{}", s(&item["key"])?),
                    patch::fingerprint(item),
                );
            }
        }
        let before = self.unresolved();
        let kept: Vec<Value> = before
            .iter()
            .filter(|entry| {
                if let Some(recorded) = entry["bases"].as_object() {
                    return recorded
                        .iter()
                        .all(|(doc, digest)| self.state.replies[doc] == *digest);
                }
                entry["fingerprints"]
                    .as_object()
                    .into_iter()
                    .flatten()
                    .all(|(id, digest)| fingerprints.get(id).is_some_and(|d| *d == *digest))
            })
            .cloned()
            .collect();
        if kept.len() != before.len() {
            self.state.unresolved = json!(kept);
        }
        Ok(())
    }
    pub(super) fn write_unresolved(&self) -> Result<()> {
        let path = self.store.managed("unresolved.json")?;
        let unresolved = self.unresolved();
        if unresolved.is_empty() {
            if path.exists() {
                fs::remove_file(path)?;
            }
            return Ok(());
        }
        store::atomic(
            &path,
            &encode(&json!({"unresolved":unresolved,
            "meaning":"Repair or restructure deferred these findings or diagnostics with a reason. They remain unresolved and are not reissued while their recorded dependencies stay unchanged. Supply the missing information or correct the extraction with update --reply; deferral is not publication approval."})),
        )
    }
    /// Record that repair cannot resolve the current diagnostics for this model.
    /// A changed model or finding set voids the escalation and repair resumes.
    pub(super) fn escalate(&mut self, reason: &str) {
        self.state.escalation =
            json!({"reason":reason,"replies":self.state.replies,"findings":self.findings()});
    }
    pub(super) fn escalated(&self) -> bool {
        let escalation = &self.state.escalation;
        escalation.is_object()
            && escalation["replies"] == self.state.replies
            && escalation["findings"] == json!(self.findings())
    }
}
