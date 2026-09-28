use super::*;

/// A step taken for every document of a scope that awaits it.
pub enum Batch<'a> {
    Record {
        model: &'a str,
        actor: &'a str,
        prompt: &'a Path,
    },
    Adopt,
    Review {
        reviewer: &'a str,
    },
}
impl Batch<'_> {
    /// The command, as a plan names it.
    pub fn action(&self) -> &'static str {
        match self {
            Batch::Record { .. } => "record",
            Batch::Adopt => "adopt",
            Batch::Review { .. } => "review",
        }
    }
    /// The key that lists the documents the step was taken for.
    pub fn done(&self) -> &'static str {
        match self {
            Batch::Record { .. } => "recorded",
            Batch::Adopt => "adopted",
            Batch::Review { .. } => "reviewed",
        }
    }
    /// The state a document is left in, as the command on one document reports it.
    pub fn state(&self) -> &'static str {
        match self {
            Batch::Record { .. } => "recorded",
            Batch::Adopt => "needs_review",
            Batch::Review { .. } => "reviewed",
        }
    }
    fn awaits(&self) -> &'static str {
        match self {
            Batch::Record { .. } => "needs_record",
            Batch::Adopt => "ready_to_adopt",
            Batch::Review { .. } => "needs_review",
        }
    }
    fn area(&self) -> &'static str {
        match self {
            Batch::Review { .. } => "documents",
            _ => "changes",
        }
    }
}

impl Store {
    /// Whether `id` names a folder of documents rather than one document.
    pub fn is_folder(&self, id: &str) -> Result<bool> {
        document_id(id)?;
        let mut ids = self.ids("documents", Some(id))?;
        ids.extend(self.ids("changes", Some(id))?);
        Ok(if ids.is_empty() {
            under(&self.sources()?, id)?.is_dir()
        } else {
            ids.iter().any(|found| found != id)
        })
    }

    /// Takes `batch` for every document at or below `scope`, or for every document,
    /// whose state awaits it; the others are listed as skipped with their state.
    /// With `expect`, a plan saved by a dry run, only the documents it lists are
    /// taken, and only while their content is the one it lists. A document that
    /// fails is listed with the reason and the others go on.
    pub fn batch(
        &self,
        batch: &Batch,
        scope: Option<&str>,
        expect: Option<&Value>,
        dry_run: bool,
    ) -> Result<Value> {
        match batch {
            Batch::Record {
                model,
                actor,
                prompt,
            } => {
                nonempty(model)?;
                nonempty(actor)?;
                fs::read(prompt).with_context(|| format!("read {}", prompt.display()))?;
            }
            Batch::Adopt => {}
            Batch::Review { reviewer } => nonempty(reviewer)?,
        }
        if let Some(scope) = scope {
            document_id(scope)?;
            let mut ids = self.ids("documents", Some(scope))?;
            ids.extend(self.ids("changes", Some(scope))?);
            ensure!(!ids.is_empty(), "no document at or below {scope}");
        }
        let expected = match expect {
            Some(plan) => {
                ensure!(
                    plan["action"] == batch.action(),
                    "the plan is for {}, not {}",
                    plan["action"],
                    batch.action()
                );
                let mut documents = BTreeMap::new();
                for document in array(&plan["documents"])? {
                    documents.insert(
                        string(&document["document_id"])?,
                        string(&document["content"])?,
                    );
                }
                Some(documents)
            }
            None => None,
        };
        let mut planned = vec![];
        let mut skipped = vec![];
        for id in self.ids(batch.area(), scope)? {
            let dir = under(&under(&self.arp, batch.area())?, &id)?;
            let row = self.state(&id, &dir, false);
            let mut skip = json!({"document_id":id,"state":row["state"]});
            if row["state"] != batch.awaits() {
                if row["blockers"].as_array().is_some_and(|b| !b.is_empty()) {
                    skip["blockers"] = row["blockers"].clone();
                }
                if let Some(error) = row.get("error") {
                    skip["error"] = error.clone();
                }
                skipped.push(skip);
                continue;
            }
            if !matches!(batch, Batch::Review { .. }) && row["no_changes"] == true {
                skip["reason"] = json!("no_changes");
                skipped.push(skip);
                continue;
            }
            if let Some(expected) = &expected {
                let reason = match expected.get(id.as_str()) {
                    None => Some("not_planned"),
                    Some(content) if row["content"] != *content => Some("changed_since_plan"),
                    Some(_) => None,
                };
                if let Some(reason) = reason {
                    skip["reason"] = json!(reason);
                    skipped.push(skip);
                    continue;
                }
            }
            planned.push(json!({"document_id":id,"content":row["content"]}));
        }
        if dry_run {
            return Ok(json!({"planned":planned,"skipped":skipped}));
        }
        let mut done = vec![];
        let mut failed = vec![];
        for document in planned {
            let id = string(&document["document_id"])?;
            let taken = match batch {
                Batch::Record {
                    model,
                    actor,
                    prompt,
                } => self.record(id, model, actor, prompt),
                Batch::Adopt => self.adopt(id),
                Batch::Review { reviewer } => self.review(id, reviewer),
            };
            match taken {
                Ok(_) => done.push(json!(id)),
                Err(error) => {
                    failed.push(json!({"document_id":id,"reason":format!("{error:#}")}));
                }
            }
        }
        let mut result = json!({"skipped":skipped,"failed":failed});
        result[batch.done()] = json!(done);
        Ok(result)
    }

    pub(super) fn formation(&self, dir: &Path) -> Result<(Value, String)> {
        let f = read(
            &self.management(dir)?.join("formation.json"),
            Some("formation"),
        )?;
        let key = hash(&encoded(&f));
        ensure!(
            hash(&fs::read(dir.join("prompt.txt"))?) == f["prompt_sha256"],
            "formation prompt changed"
        );
        Ok((f, key))
    }
    pub fn record(&self, id: &str, model: &str, actor: &str, prompt: &Path) -> Result<Value> {
        nonempty(model)?;
        nonempty(actor)?;
        let dir = self.proposal(id)?;
        let result = self.inspect(&dir, false)?;
        let raw = fs::read(prompt)?;
        let prompt_hash = hash(&raw);
        let f = json!({"schema_version":"1","document_id":result.meta["document_id"],"extraction":result.meta["extraction"],"content":result.fingerprint,"actor":actor,"model":model,"prompt_sha256":prompt_hash});
        replace(&dir.join("prompt.txt"), &raw)?;
        write(&dir.join("formation.json"), &f)?;
        Ok(f)
    }
    pub fn review(&self, id: &str, reviewer: &str) -> Result<Value> {
        nonempty(reviewer)?;
        let dir = self.document(id)?;
        let result = self.inspect(&dir, false)?;
        let (f, key) = self.formation(&dir)?;
        ensure!(
            f["document_id"] == id && f["extraction"] == result.meta["extraction"],
            "formation identity mismatch"
        );
        let r = json!({"schema_version":"1","content":result.fingerprint,"reviewer":reviewer,"formation":key});
        ensure!(
            self.fingerprint(&dir)?.as_deref() == Some(&result.fingerprint),
            "document changed during review"
        );
        write(&self.management(&dir)?.join("review.json"), &r)?;
        Ok(r)
    }
    pub fn adopt(&self, id: &str) -> Result<Value> {
        let proposal = self.proposal(id)?;
        let plan = read(&proposal.join("proposal.json"), Some("proposal"))?;
        let result = self.inspect(&proposal, false)?;
        let (f, _) = self.formation(&proposal)?;
        ensure!(
            f["document_id"] == plan["document_id"]
                && plan["document_id"] == result.meta["document_id"]
                && f["extraction"] == result.meta["extraction"]
                && f["content"] == result.fingerprint,
            "proposal changed after record; record again"
        );
        let doc_id = string(&plan["document_id"])?;
        let current = self.document(doc_id)?;
        ensure!(
            self.fingerprint(&current)? == plan["base"].as_str().map(str::to_owned),
            "authority changed since import"
        );
        ensure!(
            result.source_current,
            "source changed; re-import before adoption"
        );
        if self.fingerprint(&current)?.as_deref() == Some(&result.fingerprint) {
            fs::remove_dir_all(&proposal)?;
            remove_empty_directories(proposal.parent().unwrap(), &self.arp);
            return Ok(
                json!({"state":"skipped","reason":"no_changes","document_id":doc_id,"document":current}),
            );
        }
        let dest = current.clone();
        fs::create_dir_all(dest.parent().unwrap())?;
        let stage = Stage::new_in(&under(&self.arp, "work")?, &self.arp)?;
        let ready = stage.path().join("ready");
        fs::create_dir(&ready)?;
        for (name, path) in self.logical(&proposal)? {
            if ["proposal.json", "review.json"].contains(&name.as_str()) {
                continue;
            }
            immutable(&under(&ready, &name)?, &fs::read(path)?)?;
        }
        ensure!(
            self.fingerprint(&current)? == plan["base"].as_str().map(str::to_owned)
                && self.fingerprint(&proposal)?.as_deref() == Some(&result.fingerprint),
            "document changed during adoption"
        );
        let backup = stage.path().join("previous");
        if dest.exists() {
            fs::rename(&dest, &backup)?;
        }
        if let Err(error) = fs::rename(&ready, &dest) {
            if backup.exists()
                && let Err(restore) = fs::rename(&backup, &dest)
            {
                let recovery = stage.keep();
                anyhow::bail!(
                    "adoption failed: {error}; rollback failed: {restore}; recovery data: {}",
                    recovery.display()
                );
            }
            return Err(error.into());
        }
        // Only the adopted current document survives. Git owns its prior versions.
        fs::remove_dir_all(&proposal)?;
        remove_empty_directories(proposal.parent().unwrap(), &self.arp);
        Ok(json!({"document_id":doc_id,"document":dest}))
    }
}
impl Store {
    /// Apply reviewed edits to the referenced original, then create a fresh extraction proposal.
    pub fn apply(&self, id: &str) -> Result<Value> {
        let inspected = self.inspect(&self.document(id)?, true)?;
        ensure!(
            inspected.source_current,
            "source changed; re-import before apply"
        );
        let source = under(&self.root, string(&inspected.meta["source"]["path"])?)?;
        // The interpretation of the original as it is now, carried onto the one
        // written below. Its replay checks the original, so it is taken first.
        let excel = inspected.extraction["parser"]
            .as_str()
            .is_some_and(|parser| parser.contains(";cells/"));
        let carry = match self.interpretation(&inspected)? {
            Some((structure, _)) if excel => Some(Carry {
                structure,
                operations: excel::parse_operations(
                    array(&inspected.mappings["operations"])?,
                    array(&inspected.extraction["sheets"])?,
                )?,
                extraction: inspected.extraction.clone(),
                carrier: crate::document_structure::Carrier::Apply,
            }),
            _ => None,
        };
        let before = fs::read(&source)?;
        let stage = Stage::new_in(&under(&self.arp, "cache/export")?, &self.arp)?;
        let output = stage
            .path()
            .join(source.file_name().context("source filename missing")?);
        let report = self.export(id, Some(&output), "auto")?;
        ensure!(fs::read(&source)? == before, "source changed during apply");
        let updated = fs::read(&output)?;
        replace(&source, &updated)?;
        let proposal = match self.import_original(&source, id, carry.as_ref()) {
            Ok(proposal) => proposal,
            Err(error) => {
                ensure!(
                    fs::read(&source)? == updated,
                    "apply failed and source changed; restore using Git"
                );
                replace(&source, &before)?;
                return Err(error);
            }
        };
        let mut result = json!({"state":"needs_record", "document_id":id, "source":inspected.meta["source"]["path"], "proposal":proposal["proposal"], "report":report});
        if let Some(structure) = proposal.get("structure") {
            result["structure"] = structure.clone();
        }
        Ok(result)
    }
}
