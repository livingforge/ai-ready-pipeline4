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
        if formed_by_arp(&f) {
            ensure!(
                !dir.join("prompt.txt").exists(),
                "prompt.txt beside a formation by {}",
                f["by"].as_str().unwrap_or_default()
            );
        } else {
            ensure!(
                hash(&fs::read(dir.join("prompt.txt"))?) == f["prompt_sha256"],
                "formation prompt changed"
            );
        }
        Ok((f, key))
    }
    /// Why the adopted document of `id` keeps what it holds against a new
    /// import: `reviewed` when it has a review, `edited` when its content is not
    /// what import or apply extracted from its original. None when there is no
    /// adopted document, or it holds what was extracted, so adopting a new
    /// import loses nothing.
    pub(super) fn kept_from_import(&self, id: &str) -> Result<Option<&'static str>> {
        let dir = self.document(id)?;
        if !dir.join("document.yml").is_file() {
            return Ok(None);
        }
        if dir.join("review.json").exists() {
            return Ok(Some("reviewed"));
        }
        let (f, _) = self.formation(&dir)?;
        Ok(
            if formed_by_arp(&f) && self.fingerprint(&dir)?.as_deref() == f["content"].as_str() {
                None
            } else {
                Some("edited")
            },
        )
    }
    /// Adopts the proposal import just made of `id` as it is, recorded as formed
    /// by import rather than by an actor, model and prompt. The adopted document
    /// is then `imported` until its content is edited. A proposal is left for
    /// record and adopt, with the reason, when the adopted document keeps edits
    /// or a review (see [`Self::kept_from_import`]).
    pub fn adopt_import(&self, id: &str) -> Result<Value> {
        if let Some(reason) = self.kept_from_import(id)? {
            return Ok(json!({"document_id":id,"state":"needs_record","not_adopted":reason}));
        }
        let dir = self.proposal(id)?;
        ensure!(
            !dir.join("formation.json").exists(),
            "proposal already recorded; adopt it"
        );
        // Only what adoption reads of the inspection is kept, and the proposal
        // is adopted on it rather than inspected again: the formation written
        // here is no part of what inspection reads of a proposal.
        let (document_id, fingerprint, source_current) = {
            let (inspected, _) = self.inspect_records(&dir, false, false)?;
            (
                inspected.meta["document_id"].clone(),
                inspected.fingerprint,
                inspected.source_current,
            )
        };
        let f = json!({"schema_version":"1","content":fingerprint,"by":"import"});
        write(&dir.join("formation.json"), &f)?;
        let plan = read(&dir.join("proposal.json"), Some("proposal"))?;
        let mut adopted =
            self.adopt_inspected(&dir, &plan, &document_id, &fingerprint, source_current)?;
        if adopted.get("state").is_none() {
            adopted["state"] = json!("imported");
        }
        Ok(adopted)
    }
    pub fn record(&self, id: &str, model: &str, actor: &str, prompt: &Path) -> Result<Value> {
        nonempty(model)?;
        nonempty(actor)?;
        let dir = self.proposal(id)?;
        let (result, _) = self.inspect_records(&dir, false, false)?;
        let raw = fs::read(prompt)?;
        let prompt_hash = hash(&raw);
        let f = json!({"schema_version":"1","content":result.fingerprint,"actor":actor,"model":model,"prompt_sha256":prompt_hash});
        replace(&dir.join("prompt.txt"), &raw)?;
        write(&dir.join("formation.json"), &f)?;
        Ok(f)
    }
    pub fn review(&self, id: &str, reviewer: &str) -> Result<Value> {
        nonempty(reviewer)?;
        let dir = self.document(id)?;
        let (result, _) = self.inspect_records(&dir, false, false)?;
        let (_, key) = self.formation(&dir)?;
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
        let (result, _) = self.inspect_records(&proposal, false, false)?;
        self.adopt_inspected(
            &proposal,
            &plan,
            &result.meta["document_id"],
            &result.fingerprint,
            result.source_current,
        )
    }
    /// Adopts `proposal`, whose `plan` is its proposal.json, on what its
    /// inspection found: the `document_id` it records, the `fingerprint` of its
    /// content and whether its original is the one extracted.
    fn adopt_inspected(
        &self,
        proposal: &Path,
        plan: &Value,
        document_id: &Value,
        fingerprint: &str,
        source_current: bool,
    ) -> Result<Value> {
        let (f, _) = self.formation(proposal)?;
        ensure!(
            plan["document_id"] == *document_id && f["content"] == fingerprint,
            "proposal changed after record; record again"
        );
        let doc_id = string(&plan["document_id"])?;
        let current = self.document(doc_id)?;
        ensure!(
            self.fingerprint(&current)? == plan["base"].as_str().map(str::to_owned),
            "authority changed since import"
        );
        ensure!(source_current, "source changed; re-import before adoption");
        if self.fingerprint(&current)?.as_deref() == Some(fingerprint) {
            remove_folder(
                proposal,
                "remove the proposal holding what the document holds",
            )?;
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
        for (name, path) in self.logical(proposal)? {
            if ["proposal.json", "review.json"].contains(&name.as_str()) {
                continue;
            }
            immutable(&under(&ready, &name)?, &fs::read(&path)?).with_context(|| {
                format!(
                    "copy {} of the proposal to adopt it in {}",
                    path.display(),
                    ready.display()
                )
            })?;
        }
        ensure!(
            self.fingerprint(&current)? == plan["base"].as_str().map(str::to_owned)
                && self.fingerprint(proposal)?.as_deref() == Some(fingerprint),
            "document changed during adoption"
        );
        let backup = stage.path().join("previous");
        if dest.exists() {
            move_path(&dest, &backup, "set the adopted document aside")?;
        }
        if let Err(error) = move_path(&ready, &dest, "put the proposal in place of the document") {
            if backup.exists()
                && let Err(restore) = move_path(&backup, &dest, "put the adopted document back")
            {
                let recovery = stage.keep();
                anyhow::bail!(
                    "adoption failed: {error:#}; rollback failed: {restore:#}; recovery data: {}",
                    recovery.display()
                );
            }
            return Err(error);
        }
        // Only the adopted current document survives. Git owns its prior versions.
        remove_folder(proposal, "remove the proposal").with_context(|| {
            format!("{doc_id} was adopted, but what remains of its proposal is to be discarded")
        })?;
        remove_empty_directories(proposal.parent().unwrap(), &self.arp);
        Ok(json!({"document_id":doc_id,"document":dest}))
    }
}
/// Whether formation `f` records content adopted as import extracted it.
pub(super) fn formed_by_import(f: &Value) -> bool {
    f["by"] == "import"
}

/// Whether formation `f` records content ARP extracted, by import or by
/// apply, rather than formed by an actor with a model and prompt.
fn formed_by_arp(f: &Value) -> bool {
    f["by"].is_string()
}

impl Store {
    /// Apply reviewed edits to the referenced original, then create a fresh
    /// extraction proposal, recorded as formed by apply.
    /// When the original written is the one there, the document is `unchanged`.
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
                    &inspected.extraction.sheets,
                )?,
                extraction: inspected.extraction.clone(),
                carrier: crate::document_structure::Carrier::Apply,
            }),
            _ => None,
        };
        let before = fs::read(&source)?;
        let (updated, report) = self.confirmed_export(id, &inspected)?;
        ensure!(fs::read(&source)? == before, "source changed during apply");
        // An original written back as it was is the version adopted, so
        // nothing is written or extracted again, as an import of it would not.
        if updated == before {
            return Ok(
                json!({"state":"unchanged", "document_id":id, "source":inspected.meta["source"]["path"], "report":report}),
            );
        }
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
        // Nobody formed the new extraction: it is recorded as formed by apply,
        // ready to adopt, unless its content is changed before.
        let dir = self.proposal(id)?;
        let (formed, _) = self.inspect_records(&dir, false, false)?;
        write(
            &dir.join("formation.json"),
            &json!({"schema_version":"1","content":formed.fingerprint,"by":"apply"}),
        )?;
        let mut result = json!({"state":"ready_to_adopt", "document_id":id, "source":inspected.meta["source"]["path"], "proposal":proposal["proposal"], "report":report});
        if let Some(structure) = proposal.get("structure") {
            result["structure"] = structure.clone();
        }
        Ok(result)
    }
}
