use super::*;

impl Store {
    pub(super) fn diff_values(&self, dir: Option<&Path>) -> Result<Value> {
        let Some(dir) = dir else { return Ok(json!({})) };
        if self.fingerprint(dir)?.is_none() {
            return Ok(json!({}));
        }
        let mut out = json!({});
        for (name, path) in self.logical(dir)? {
            if name.starts_with("content/") || name == "mappings.yml" || name == "document.yml" {
                out[&name] = read(&path, None)?
            } else if name.starts_with("assets/") {
                out[&name] = json!(hash(&fs::read(path)?));
            }
        }
        Ok(out)
    }
    pub fn diff(&self, proposal: Option<&str>, document: Option<&str>) -> Result<Value> {
        fn compare(before: &Value, after: &Value, path: &str, out: &mut Vec<Value>) {
            if before == after {
                return;
            }
            if let (Some(a), Some(b)) = (before.as_object(), after.as_object()) {
                let keys: BTreeSet<_> = a.keys().chain(b.keys()).collect();
                for k in keys {
                    let p = format!("{path}/{k}");
                    match(a.get(k),b.get(k)){
                    (Some(x),Some(y))=>compare(x,y,&p,out),
                    (x,y)=>out.push(json!({"path":p,"before":x,"after":y,"kind":if x.is_none(){"added"}else{"removed"}}))
                }
                }
            } else {
                out.push(json!({"path":path,"before":before,"after":after,"kind":"changed"}));
            }
        }
        let mut comparisons = vec![];
        let mut source_impact = None;
        let changed;
        let pairs = if let Some(id) = proposal {
            let p = self.proposal(id)?;
            self.inspect(&p, false)?;
            let plan = read(&p.join("proposal.json"), Some("proposal"))?;
            let current = self.document(string(&plan["document_id"])?)?;
            changed = self.fingerprint(&current)? != plan["base"].as_str().map(str::to_owned);
            if current.exists() {
                let before = self.inspect(&current, false)?.extraction;
                let after = self.inspect(&p, false)?.extraction;
                if [&before, &after].iter().all(|e| {
                    e["parser"]
                        .as_str()
                        .is_some_and(|p| p.contains(";native-text/"))
                }) {
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
            let mut before = json!({});
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
                    before[name] = parse(std::str::from_utf8(&raw)?, name.ends_with(".json"))?;
                } else if name.starts_with("assets/") {
                    before[name] = json!(hash(&git(&["show", &format!("HEAD:{path}")])?));
                }
            }
            let mut changes = vec![];
            compare(
                &before,
                &self.diff_values(Some(&current))?,
                "",
                &mut changes,
            );
            return Ok(
                json!({"schema_version":"1", "implementation":"rust", "comparisons":[{"kind":"git_to_current", "changes":changes}], "authority_changed":false}),
            );
        };
        for (label, left, right) in pairs {
            let mut changes = vec![];
            compare(
                &self.diff_values(left.as_deref())?,
                &self.diff_values(right.as_deref())?,
                "",
                &mut changes,
            );
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
