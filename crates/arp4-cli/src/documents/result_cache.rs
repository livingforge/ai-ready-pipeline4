//! Results of the read-only commands a caller pages through, kept for their
//! later pages. A page of a diff or an export plan otherwise compares or
//! writes the whole document again, so reading every page took time in
//! proportion to the pages times the document. The summary of a structure
//! replay a status row shows is kept likewise, for the status after an edit
//! that leaves what the replay reads as it was. A result is saved under the
//! hash of every byte it is made from and reused while they are the same;
//! the newest few are kept.
use super::*;

/// The results of a paged command kept of each kind; saving another removes
/// the oldest.
const KEPT: usize = 8;

impl Store {
    /// The result `compute` gives for `inputs`, the stamps of what it is made
    /// from, saved in `.arp/cache/<kind>/` with the newest `kept` others.
    pub(super) fn cached_result(
        &self,
        kind: &str,
        inputs: &Value,
        kept: usize,
        compute: impl FnOnce() -> Result<Value>,
    ) -> Result<Value> {
        let directory = under(&self.arp, &format!("cache/{kind}"))?;
        let key = hash(&encoded(
            &json!({"program":crate::data::program()?,"inputs":inputs}),
        ));
        let path = directory.join(format!("{key}.json"));
        if let Some(saved) = fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        {
            return Ok(saved);
        }
        let result = compute()?;
        // A command that cannot save its result still answers; the next page
        // computes it again.
        if crate::data::replace_cache(&path, &serde_json::to_vec(&result)?).is_ok() {
            crate::data::keep_newest(&directory, kept);
        }
        Ok(result)
    }

    /// The hash of every file the document or proposal in `dir` is made from.
    fn stamp_of(&self, dir: &Path) -> Result<String> {
        let config = hash(&fs::read(under(&self.arp, "config.yml")?)?);
        self.document_stamp(dir, &config, &mut vec![0; search::READ_BUFFER_SIZE])
    }

    /// The changes of [`Store::diff`] without its semantic changes, for a
    /// page of them: saved while the proposal, the adopted document and the
    /// registry, or the document and the Git commit it is compared with, are
    /// the same.
    pub fn diff_changes(&self, proposal: Option<&str>, document: Option<&str>) -> Result<Value> {
        let compute = || self.compare(proposal, document, false);
        let inputs = if let Some(id) = proposal {
            let dir = self.proposal(id)?;
            let plan = read(&dir.join("proposal.json"), Some("proposal"))?;
            let current = self.document(string(&plan["document_id"])?)?;
            let registry = under(&self.arp, "registry")?;
            let mut files = BTreeMap::new();
            if registry.is_dir() {
                for (name, path) in crate::data::files(&registry)? {
                    files.insert(name, hash(&fs::read(path)?));
                }
            }
            json!({"proposal":id,"own":self.stamp_of(&dir)?,
                "current":current.exists().then(|| self.stamp_of(&current)).transpose()?,
                "registry":files})
        } else {
            let id = document.context("document or proposal required")?;
            let dir = self.document(id)?;
            let head = std::process::Command::new("git")
                .arg("-C")
                .arg(&self.root)
                .args(["rev-parse", "HEAD"])
                .output()
                .ok()
                .filter(|output| output.status.success());
            // Without a commit the comparison fails as it would uncached.
            let Some(head) = head else {
                return compute();
            };
            json!({"document":id,"own":self.stamp_of(&dir)?,
                "head":String::from_utf8_lossy(&head.stdout).trim()})
        };
        self.cached_result("diff", &inputs, KEPT, compute)
    }

    /// The plan [`Store::export`] gives without an output, for a page of it:
    /// saved while the document, its original and the configuration are the
    /// same.
    pub fn export_plan(&self, id: &str) -> Result<Value> {
        let inputs = json!({"document":id,"own":self.stamp_of(&self.document(id)?)?});
        self.cached_result("export", &inputs, KEPT, || self.export(id, None, false))
    }
}
