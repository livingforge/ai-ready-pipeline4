use super::*;

impl Workflow {
    pub(super) fn collect(&self, path: &Path) -> Result<Value> {
        let mut result = Map::new();
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            ensure!(
                !crate::data::is_link(&fs::symlink_metadata(entry.path())?),
                "linked artifact path"
            );
            if entry.file_type()?.is_file() {
                result.insert(
                    entry
                        .file_name()
                        .to_str()
                        .context("invalid filename")?
                        .to_owned(),
                    json!(self.store.put(&fs::read(entry.path())?)?),
                );
            }
        }
        Ok(Value::Object(result))
    }
    pub(super) fn export(&self) -> Result<Value> {
        ensure!(
            self.state.status == WorkflowStatus::Complete,
            "formal export requires complete reviewed generation; a needs_decision run keeps its draft under draft/"
        );
        let out = self.store.managed("latest")?;
        for (name, digest) in obj(&self.state.exports)? {
            ensure!(
                Path::new(name).file_name().and_then(|v| v.to_str()) == Some(name),
                "invalid export name"
            );
            let path = self.store.managed(&format!("latest/{name}"))?;
            if path.exists() {
                ensure!(
                    hash(&fs::read(path)?) == s(digest)?,
                    "modified export: {name}"
                );
            }
        }
        for (name, digest) in obj(&self.state.exports)? {
            self.store
                .materialize(s(digest)?, &self.store.managed(&format!("latest/{name}"))?)?;
        }
        store::atomic(
            &self.store.managed("latest/manifest.json")?,
            &encode(&self.state.exports),
        )?;
        Ok(json!({"export":out}))
    }
    pub(super) fn compact(&self) -> Result<Value> {
        let mut live = BTreeSet::new();
        for digest in [&self.state.input, &self.state.regions] {
            live.insert(digest.clone());
        }
        for digest in [&self.state.unresolved, &self.state.quality["diagnostics"]] {
            if let Some(digest) = digest.as_str() {
                live.insert(digest.to_owned());
            }
        }
        for values in [
            &self.state.replies,
            &self.state.exports,
            &self.state.packets,
            &self.state.bundle,
            &self.state.references,
        ] {
            if values.is_null() {
                continue;
            }
            for digest in obj(values)?.values() {
                live.insert(s(digest)?.to_owned());
            }
        }
        for digest in self
            .state
            .drafts
            .as_object()
            .into_iter()
            .flat_map(|d| d.values())
        {
            live.insert(s(digest)?.to_owned());
        }
        for task in &self.state.tasks {
            live.insert(task.task.clone());
            for digest in [
                &task.context,
                &task.scope,
                &task.packet,
                &task.base,
                &task.reply,
            ]
            .into_iter()
            .flatten()
            {
                live.insert(digest.clone());
            }
        }
        if let Some(plans) = self.state.incremental.as_object() {
            for plan in plans.values() {
                live.insert(s(&plan["base"])?.to_owned());
            }
        }
        self.store.compact(&live)
    }
    /// Terminal state for open issues: publish a draft for a human decision
    /// instead of halting. Formal export stays limited to complete runs.
    pub(super) fn needs_decision(
        &mut self,
        root: &Path,
        input: &Path,
        model: &Path,
        reviewed: bool,
    ) -> Result<Value> {
        let design = root.join("design.draft.md");
        crate::semantic_operations::render(input, model, &design, true)?;
        let report = read(&root.join("design.draft.md.report.json"))?;
        self.state.status = WorkflowStatus::NeedsDecision;
        let open = json!({"reason":self.state.escalation["reason"],"reviewed":reviewed,
            "concerns":self.concerns(None, report["validation"]["issues"].as_array().map(Vec::as_slice).unwrap_or_default())?,
            "references":self.reference_paths(),
            "next_action":"Decide each open issue. Correct the extraction with update --reply <corrected-extraction.json> on the same root, or keep the draft. Formal export requires a complete run."});
        let mut drafts = Map::new();
        drafts.insert(
            "design.draft.md".into(),
            json!(self.store.put(&fs::read(&design)?)?),
        );
        drafts.insert(
            "design.draft.report.json".into(),
            json!(self.store.put_json(&report)?),
        );
        drafts.insert(
            "open-issues.json".into(),
            json!(self.store.put_json(&open)?),
        );
        for (name, digest) in &drafts {
            self.store
                .materialize(s(digest)?, &self.store.managed(&format!("draft/{name}"))?)?;
        }
        self.state.drafts = Value::Object(drafts);
        self.state.status = WorkflowStatus::NeedsDecision;
        self.state.blocked = json!([]);
        self.state.tasks.clear();
        self.save()?;
        self.status()
    }
}
