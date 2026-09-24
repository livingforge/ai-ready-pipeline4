use super::*;

impl Workflow {
    pub(super) fn update(
        &mut self,
        input: Option<PathBuf>,
        replies: Vec<PathBuf>,
        regions: Option<PathBuf>,
        references: Vec<PathBuf>,
    ) -> Result<()> {
        ensure!(
            self.state.status != WorkflowStatus::Complete,
            "completed initial generation is immutable; use registry for canonical updates"
        );
        ensure!(
            !self
                .state
                .tasks
                .iter()
                .any(|t| t.state == TaskState::Running),
            "recover interrupted runs before update"
        );
        let before = self.state.clone();
        let declared = regions
            .as_ref()
            .map(|path| read(path))
            .transpose()?
            .unwrap_or(self.store.json(&self.state.regions)?);
        if let Some(input) = input {
            let input = dunce::canonicalize(input)?;
            let captured = read(&input)?;
            let current = crate::specifications::load_input(&input)?;
            let previous: crate::specifications::Input =
                serde_json::from_value(self.store.json(&self.state.input)?)?;
            self.state.incremental = json!({});
            self.state.input = self.store.put_json(&captured)?;
            let docs: Vec<String> = obj(&self.state.replies)?.keys().cloned().collect();
            for doc in &docs {
                let keep = if arr(&captured["sources"])?
                    .iter()
                    .any(|s| s["document"] == *doc)
                {
                    let packet = crate::semantic::packet(&current, doc)?;
                    let base = self.store.json(s(&self.state.replies[doc])?)?;
                    let external_changed =
                        incremental::external_changed(&base, &previous, &current)?;
                    let same = base["packet"] == packet["packet"] && !external_changed;
                    if !same && !external_changed && declared.get(doc).is_none() {
                        let old = crate::semantic::packet(&previous, doc)?;
                        if let Some(region) = incremental::plan(&old, &packet, &base)? {
                            self.state.incremental[doc] =
                                json!({"base":self.state.replies[doc],"region":region});
                        }
                    }
                    same
                } else {
                    false
                };
                if !keep {
                    self.state.replies.as_object_mut().unwrap().remove(doc);
                }
            }
        }
        let mut docs = BTreeSet::new();
        for path in replies {
            let reply = read(&path)?;
            let input = serde_json::from_value(self.store.json(&self.state.input)?)?;
            let packet = crate::semantic::packet(&input, s(&reply["document"])?)?;
            let reply =
                self.expand_payload(reply, &BTreeSet::from([s(&packet["packet"])?.to_owned()]))?;
            let doc = s(&reply["document"])?;
            ensure!(docs.insert(doc.to_owned()), "duplicate reply");
            self.state.replies[doc] = json!(self.store.put_json(&reply)?);
            if let Some(plans) = self.state.incremental.as_object_mut() {
                plans.remove(doc);
            }
        }
        if let Some(path) = regions {
            self.state.regions = self.store.put_json(&read(&path)?)?;
        }
        if !references.is_empty() {
            self.state.references = self.load_references(&references)?;
        }
        ensure!(
            before.input != self.state.input
                || before.replies != self.state.replies
                || before.regions != self.state.regions
                || before.references != self.state.references,
            "no input changes; failed calls require explicit retry or corrected reply"
        );
        if let Err(error) = self.validate_replies(&self.state.replies) {
            self.state = before;
            return Err(error);
        }
        for (doc, digest) in obj(&self.state.replies)?.clone() {
            if before.replies[&doc] != digest {
                let reply = self.store.json(s(&digest)?)?;
                self.record_submission(
                    &json!({"stage":"update","document":doc,"base":before.replies[&doc]}),
                    &reply,
                    &json!({"actor":"external","reason":"Externally updated extraction","origin":"external"}),
                    true,
                    None,
                )?;
            }
        }
        // An update can interrupt a review batch. Preserve completed clean reviews
        // even when another task in that batch has not been submitted yet.
        for task in &before.tasks {
            if task.state == TaskState::Complete
                && (task.stage == Stage::Review || task.stage == Stage::GlobalReview)
            {
                let reply = self.artifact(&task.reply)?;
                if arr(&reply["findings"])?.is_empty() {
                    self.state.reviews[s(&reply["packet"])?] = json!(task.reply);
                }
            }
        }
        self.state.tasks.clear();
        self.state.status = WorkflowStatus::New;
        self.state.blocked = json!([]);
        self.state.repair_rounds.clear();
        self.state.round = 0;
        self.state.quality = Value::Null;
        self.state.bundle = json!({});
        self.state.exports = json!({});
        self.state.findings = Value::Null;
        self.state.escalation = Value::Null;
        self.state.drafts = Value::Null;
        self.state.reviewed_replies = Value::Null;
        self.state.review_findings = Value::Null;
        self.state.unresolved = Value::Null;
        self.save()
    }
}
