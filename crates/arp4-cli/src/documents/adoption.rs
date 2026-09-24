use super::*;

impl Store {
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
    pub fn adopt(&self, id: &str, reviewer: &str) -> Result<Value> {
        nonempty(reviewer)?;
        let proposal = self.proposal(id)?;
        let plan = read(&proposal.join("proposal.json"), Some("proposal"))?;
        let result = self.inspect(&proposal, false)?;
        let (f, key) = self.formation(&proposal)?;
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
        let dest = current.clone();
        fs::create_dir_all(dest.parent().unwrap())?;
        let work = under(&self.arp, "work")?;
        fs::create_dir_all(&work)?;
        let stage = tempfile::tempdir_in(&work)?;
        let ready = stage.path().join("ready");
        fs::create_dir(&ready)?;
        for (name, path) in self.logical(&proposal)? {
            if ["proposal.json", "review.json"].contains(&name.as_str()) {
                continue;
            }
            immutable(&under(&ready, &name)?, &fs::read(path)?)?;
        }
        write(
            &ready.join("review.json"),
            &json!({"schema_version":"1","content":result.fingerprint,"reviewer":reviewer,"formation":key}),
        )?;
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
        let before = fs::read(&source)?;
        let cache = under(&self.arp, "cache/export")?;
        fs::create_dir_all(&cache)?;
        let stage = tempfile::tempdir_in(&cache)?;
        let output = stage
            .path()
            .join(source.file_name().context("source filename missing")?);
        let report = self.export(id, Some(&output), "auto")?;
        ensure!(fs::read(&source)? == before, "source changed during apply");
        let updated = fs::read(&output)?;
        replace(&source, &updated)?;
        let proposal = match self.import(&source, id) {
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
        Ok(
            json!({"state":"needs_record", "source":inspected.meta["source"]["path"], "proposal_id":proposal["proposal_id"], "proposal":proposal["proposal"], "report":report}),
        )
    }
}
