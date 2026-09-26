use super::*;

/// A compared file: YAML/JSON text, parsed only when the other side differs, or an
/// asset's hash. A large workbook's mappings.yml is mostly unchanged between versions.
enum DiffFile {
    Text(String),
    Hash(String),
}

impl DiffFile {
    fn value(&self, name: &str) -> Result<Value> {
        match self {
            Self::Text(text) => parse(text, name.ends_with(".json")),
            Self::Hash(hash) => Ok(json!(hash)),
        }
    }
}

fn compare(before: &Value, after: &Value, path: &str, out: &mut Vec<Value>) {
    if before == after {
        return;
    }
    if let (Some(a), Some(b)) = (before.as_object(), after.as_object()) {
        let keys: BTreeSet<_> = a.keys().chain(b.keys()).collect();
        for k in keys {
            let p = format!("{path}/{k}");
            match (a.get(k), b.get(k)) {
                (Some(x), Some(y)) => compare(x, y, &p, out),
                (x, y) => out.push(json!({"path":p,"before":x,"after":y,"kind":if x.is_none(){"added"}else{"removed"}})),
            }
        }
    } else {
        out.push(json!({"path":path,"before":before,"after":after,"kind":"changed"}));
    }
}

/// The changes between two sets of files, in file name order.
fn compare_files(
    before: &BTreeMap<String, DiffFile>,
    after: &BTreeMap<String, DiffFile>,
) -> Result<Vec<Value>> {
    let mut out = vec![];
    let names: BTreeSet<_> = before.keys().chain(after.keys()).collect();
    for name in names {
        let path = format!("/{name}");
        match (before.get(name), after.get(name)) {
            (Some(DiffFile::Text(a)), Some(DiffFile::Text(b))) if a == b => {}
            (Some(a), Some(b)) => compare(&a.value(name)?, &b.value(name)?, &path, &mut out),
            (a, b) => {
                let (x, y) = (
                    a.map(|a| a.value(name)).transpose()?,
                    b.map(|b| b.value(name)).transpose()?,
                );
                out.push(json!({"path":path,"before":x,"after":y,"kind":if x.is_none(){"added"}else{"removed"}}));
            }
        }
    }
    Ok(out)
}

impl Store {
    fn diff_files(&self, dir: Option<&Path>) -> Result<BTreeMap<String, DiffFile>> {
        let mut out = BTreeMap::new();
        let Some(dir) = dir else { return Ok(out) };
        if self.fingerprint(dir)?.is_none() {
            return Ok(out);
        }
        for (name, path) in self.logical(dir)? {
            if name.starts_with("content/") || name == "mappings.yml" || name == "document.yml" {
                out.insert(name, DiffFile::Text(fs::read_to_string(&path)?));
            } else if name.starts_with("assets/") {
                out.insert(name, DiffFile::Hash(hash(&fs::read(path)?)));
            }
        }
        Ok(out)
    }
    pub fn diff(&self, proposal: Option<&str>, document: Option<&str>) -> Result<Value> {
        let mut comparisons = vec![];
        let mut source_impact = None;
        let changed;
        let pairs = if let Some(id) = proposal {
            let p = self.proposal(id)?;
            let after = self.inspect(&p, false)?.extraction;
            let plan = read(&p.join("proposal.json"), Some("proposal"))?;
            let current = self.document(string(&plan["document_id"])?)?;
            changed = self.fingerprint(&current)? != plan["base"].as_str().map(str::to_owned);
            let native_text = |e: &Value| {
                e["parser"]
                    .as_str()
                    .is_some_and(|p| p.contains(";native-text/"))
            };
            // Source impact compares two text extractions; only then is the current one read.
            if current.exists() && native_text(&after) {
                let before = self.inspect(&current, false)?.extraction;
                if native_text(&before) {
                    let registry_path = under(&self.arp, "registry")?;
                    let registry = if registry_path.join("registry.json").exists() {
                        Some(crate::registry::load(&registry_path)?)
                    } else {
                        None
                    };
                    source_impact = Some(crate::source_impact::compare(
                        &before,
                        &after,
                        registry.as_ref(),
                    )?);
                }
            }
            vec![("current_to_proposal", Some(current), Some(p))]
        } else {
            let current = self.document(document.context("document or proposal required")?)?;
            self.inspect(&current, false)?;
            let relative = current
                .strip_prefix(&self.root)?
                .to_string_lossy()
                .replace('\\', "/");
            let git = |args: &[&str]| -> Result<Vec<u8>> {
                let output = std::process::Command::new("git")
                    .arg("-C")
                    .arg(&self.root)
                    .args(args)
                    .output()
                    .context("Git is required for document history")?;
                ensure!(
                    output.status.success(),
                    "Git baseline unavailable; commit the document before diff"
                );
                Ok(output.stdout)
            };
            let mut before = BTreeMap::new();
            let tree = git(&[
                "ls-tree",
                "-r",
                "--name-only",
                "-z",
                "HEAD",
                "--",
                &relative,
            ])?;
            for path in tree
                .split(|byte| *byte == 0)
                .filter(|path| !path.is_empty())
            {
                let path = std::str::from_utf8(path)?;
                let name = path
                    .strip_prefix(&format!("{relative}/"))
                    .context("unexpected Git path")?;
                if name.starts_with("content/") || name == "mappings.yml" || name == "document.yml"
                {
                    let raw = git(&["show", &format!("HEAD:{path}")])?;
                    before.insert(name.to_owned(), DiffFile::Text(String::from_utf8(raw)?));
                } else if name.starts_with("assets/") {
                    let raw = git(&["show", &format!("HEAD:{path}")])?;
                    before.insert(name.to_owned(), DiffFile::Hash(hash(&raw)));
                }
            }
            let changes = compare_files(&before, &self.diff_files(Some(&current))?)?;
            return Ok(
                json!({"schema_version":"1", "implementation":"rust", "comparisons":[{"kind":"git_to_current", "changes":changes}], "authority_changed":false}),
            );
        };
        for (label, left, right) in pairs {
            let changes = compare_files(
                &self.diff_files(left.as_deref())?,
                &self.diff_files(right.as_deref())?,
            )?;
            comparisons.push(json!({"kind":label,"changes":changes}));
        }
        if let Some(impact) = source_impact {
            comparisons.push(impact);
        }
        Ok(
            json!({"schema_version":"1","implementation":"rust","comparisons":comparisons,"authority_changed":changed}),
        )
    }
}
