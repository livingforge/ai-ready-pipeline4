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
    } else if let (Some(a), Some(b)) = (before.as_array(), after.as_array()) {
        if a.iter().chain(b).all(|v| v["id"].is_string()) {
            let index = |rows: &[Value]| -> Value {
                Value::Object(
                    rows.iter()
                        .map(|v| (v["id"].as_str().unwrap().to_owned(), v.clone()))
                        .collect(),
                )
            };
            compare(&index(a), &index(b), path, out);
            let ids = |rows: &[Value]| -> Value {
                json!(rows.iter().map(|v| &v["id"]).collect::<Vec<_>>())
            };
            if ids(a) != ids(b) {
                out.push(json!({"path":format!("{path}/order"),"before":ids(a),"after":ids(b),"kind":"changed"}));
            }
        } else if path.starts_with("/content/") && a.len() == b.len() {
            // Array bodies bind values by slot. Keep scalar edits readable;
            // structural identity moves are reported separately from layout.
            for (index, (x, y)) in a.iter().zip(b).enumerate() {
                compare(x, y, &format!("{path}/{index}"), out);
            }
        } else {
            out.push(json!({"path":path,"before":before,"after":after,"kind":"changed"}));
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

fn semantic_changes(
    before: &BTreeMap<String, DiffFile>,
    after: &BTreeMap<String, DiffFile>,
) -> Result<Vec<Value>> {
    let value = |files: &BTreeMap<String, DiffFile>, name: &str| -> Result<Value> {
        files
            .get(name)
            .map(|file| file.value(name))
            .transpose()
            .map(|v| v.unwrap_or(Value::Null))
    };
    let (a, b) = (value(before, "layout.yml")?, value(after, "layout.yml")?);
    if a.is_null() || b.is_null() {
        return Ok(vec![]);
    }
    ensure!(
        a["schema_version"] == "4" && b["schema_version"] == "4",
        "semantic diff requires document element bindings"
    );
    let mut out = vec![];
    let bm = value(before, "mappings.yml")?;
    let am = value(after, "mappings.yml")?;
    let history: Vec<_> = array(&b["history"])?
        .iter()
        .filter(|o| {
            !array(&a["history"])
                .is_ok_and(|history| history.iter().any(|old| old["id"] == o["id"]))
                || (a["source_sha256"] != b["source_sha256"]
                    && bm["operations"].as_array().is_some_and(|pending| {
                        pending.iter().any(|old| {
                            ["id", "kind", "sheet", "at", "count", "to", "style_from"]
                                .iter()
                                .all(|key| old[*key] == o[*key])
                                && !am["operations"].as_array().is_some_and(|remaining| {
                                    remaining.iter().any(|entry| entry["id"] == old["id"])
                                })
                        })
                    }))
        })
        .cloned()
        .collect();
    let history = json!(history);
    let op_values = if history.as_array().is_some_and(|o| !o.is_empty()) {
        &history
    } else {
        // Newly recorded operations prove a transition. Applied pending operations
        // are selected from history above; unchanged pending work proves no move.
        &Value::Array(
            am["operations"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|operation| {
                    !bm["operations"]
                        .as_array()
                        .is_some_and(|old| old.contains(operation))
                })
                .cloned()
                .collect(),
        )
    };
    let sheets = b["sheets"]
        .as_object()
        .context("layout sheets missing")?
        .values()
        .map(|s| json!({"name":s["name"]}))
        .collect::<Vec<_>>();
    let operations = excel::parse_operations(
        op_values.as_array().map(Vec::as_slice).unwrap_or(&[]),
        &sheets,
    )?;
    for operation in &operations {
        if matches!(operation.kind, excel::OperationKind::MoveColumns { .. }) {
            out.push(json!({"kind":"column_moved","sheet":operation.sheet,"operation_id":operation.id,"from":operation.at,"count":operation.count,"to":match operation.kind{excel::OperationKind::MoveColumns{to}=>to,_=>unreachable!()}}));
        }
    }
    for (page, sheet) in b["sheets"].as_object().context("layout sheets missing")? {
        let previous = &a["sheets"][page];
        if previous.is_null() {
            out.push(json!({"kind":"ambiguous_correspondence","page":page,"reason":"sheet identities differ"}));
            continue;
        }
        for (axis, added, removed) in [
            ("columns", "column_added", "column_removed"),
            ("rows", "row_added", "row_removed"),
        ] {
            let ac = previous[axis].as_object().context("layout axis missing")?;
            let bc = sheet[axis].as_object().context("layout axis missing")?;
            for id in ac.keys().chain(bc.keys()).collect::<BTreeSet<_>>() {
                match (ac.get(id),bc.get(id)) {
                    (None,Some(target)) => out.push(json!({"kind":added,"page":page,"identity":id,"position":target["position"]})),
                    (Some(target),None) => out.push(json!({"kind":removed,"page":page,"identity":id,"position":target["position"]})),
                    (Some(x),Some(y)) if x["position"] != y["position"] => out.push(json!({"kind":"reference_position_changed","page":page,"axis":axis,"identity":id,"before":x["position"],"after":y["position"]})),
                    _=>{}
                }
            }
        }
        for name in before
            .keys()
            .chain(after.keys())
            .filter(|n| n.starts_with("content/"))
            .collect::<BTreeSet<_>>()
        {
            let (x, y) = (value(before, name)?, value(after, name)?);
            if x["page_id"] != *page || y["page_id"] != *page {
                continue;
            }
            let x = elements::decode(&x, &a)?;
            let y = elements::decode(&y, &b)?;
            let (ar, br) = (
                &x["blocks"]["table-1"]["rows"],
                &y["blocks"]["table-1"]["rows"],
            );
            let mut changes = vec![];
            compare(ar, br, "", &mut changes);
            for mut change in changes {
                change["kind"] = json!("value_changed");
                change["page"] = json!(page);
                out.push(change);
            }
            let (af, bf) = (
                &x["blocks"]["formulas"]["rows"],
                &y["blocks"]["formulas"]["rows"],
            );
            let mut changes = vec![];
            compare(af, bf, "", &mut changes);
            for mut change in changes {
                let equivalent = change["before"]
                    .as_str()
                    .zip(change["after"].as_str())
                    .is_some_and(|(before, after)| {
                        excel::formula_reference_equivalent(
                            before.trim_start_matches('='),
                            after.trim_start_matches('='),
                            sheet["name"].as_str().unwrap_or(""),
                            &operations,
                        )
                    });
                change["kind"] = json!(if equivalent {
                    "formula_reference_changed"
                } else {
                    "formula_changed"
                });
                change["equivalence_proven"] = json!(equivalent);
                change["page"] = json!(page);
                out.push(change);
            }
        }
    }
    Ok(out)
}

impl Store {
    fn diff_files(&self, dir: Option<&Path>) -> Result<BTreeMap<String, DiffFile>> {
        let mut out = BTreeMap::new();
        let Some(dir) = dir else { return Ok(out) };
        if !self.management(dir)?.join("document.yml").exists() {
            return Ok(out);
        }
        for (name, path) in self.logical(dir)? {
            if name.starts_with("content/")
                || name == "mappings.yml"
                || name == "document.yml"
                || name == "layout.yml"
            {
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
                if name.starts_with("content/")
                    || name == "mappings.yml"
                    || name == "document.yml"
                    || name == "layout.yml"
                {
                    let raw = git(&["show", &format!("HEAD:{path}")])?;
                    before.insert(name.to_owned(), DiffFile::Text(String::from_utf8(raw)?));
                } else if name.starts_with("assets/") {
                    let raw = git(&["show", &format!("HEAD:{path}")])?;
                    before.insert(name.to_owned(), DiffFile::Hash(hash(&raw)));
                }
            }
            let after = self.diff_files(Some(&current))?;
            let changes = compare_files(&before, &after)?;
            let semantic = semantic_changes(&before, &after)?;
            return Ok(
                json!({"schema_version":"1", "implementation":"rust", "comparisons":[{"kind":"git_to_current", "changes":changes, "semantic_changes":semantic}], "authority_changed":false}),
            );
        };
        for (label, left, right) in pairs {
            let before = self.diff_files(left.as_deref())?;
            let after = self.diff_files(right.as_deref())?;
            let changes = compare_files(&before, &after)?;
            comparisons.push(json!({"kind":label,"changes":changes,"semantic_changes":semantic_changes(&before,&after)?}));
        }
        if let Some(impact) = source_impact {
            comparisons.push(impact);
        }
        Ok(
            json!({"schema_version":"1","implementation":"rust","comparisons":comparisons,"authority_changed":changed}),
        )
    }
}

#[cfg(test)]
mod performance_tests {
    use super::*;

    #[test]
    fn identical_pending_moves_are_not_semantic_changes() {
        let files = BTreeMap::from([
            (
                "layout.yml".into(),
                DiffFile::Text(
                    json!({
                        "schema_version":"4", "history":[],
                        "sheets":{"sheet-1":{"name":"Sheet", "rows":{}, "columns":{}}}
                    })
                    .to_string(),
                ),
            ),
            (
                "mappings.yml".into(),
                DiffFile::Text(
                    json!({"operations":[{
                        "id":"move", "kind":"move_columns", "sheet":"Sheet",
                        "at":3, "count":1, "to":1, "reason":"fixture"
                    }]})
                    .to_string(),
                ),
            ),
        ]);
        assert!(semantic_changes(&files, &files).unwrap().is_empty());
        let equivalent = files
            .iter()
            .map(|(name, file)| {
                let value = file.value(name).unwrap();
                (
                    name.clone(),
                    DiffFile::Text(serde_json::to_string_pretty(&value).unwrap()),
                )
            })
            .collect();
        assert!(semantic_changes(&files, &equivalent).unwrap().is_empty());
    }

    #[test]
    #[ignore]
    fn measure_diff_files() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::init(temp.path(), "sources").unwrap();
        let dir = store.arp.join("documents/bench");
        fs::create_dir_all(dir.join("assets")).unwrap();
        fs::write(dir.join("document.yml"), "benchmark").unwrap();
        let bytes = vec![42u8; 256 * 1024];
        for i in 0..48 {
            fs::write(dir.join(format!("assets/{i:03}.bin")), &bytes).unwrap();
        }
        let start = std::time::Instant::now();
        let files = store.diff_files(Some(&dir)).unwrap();
        assert_eq!(files.len(), 49);
        eprintln!(
            "diff_files_ms={:.3} files={}",
            start.elapsed().as_secs_f64() * 1000.0,
            files.len()
        );
    }
}
