use super::*;

pub(super) fn refresh(r: &mut Registry) -> Result<()> {
    let mut needed: BTreeSet<_> = r
        .entries
        .values()
        .flat_map(|entry| entry.evidence.iter().map(|e| e.snapshot.clone()))
        .collect();
    for judgment in r.archives.values() {
        if let Some(input) = judgment["input_hash"].as_str() {
            needed.insert(input.to_owned());
        }
    }
    r.inputs.retain(|key, _| needed.contains(key));
    r.manifest.evidence = needed;
    r.manifest.records = r
        .entries
        .iter()
        .map(|(id, e)| Ok((id.clone(), hash(&bytes(e)?))))
        .collect::<Result<_>>()?;
    Ok(())
}
pub fn load(root: &Path) -> Result<Registry> {
    let manifest: Manifest = serde_json::from_value(read(&root.join("registry.json"), None)?)?;
    ensure!(
        manifest.records.keys().all(|s| valid_id(s))
            && manifest.evidence.iter().all(|s| digest(s))
            && manifest.archives.keys().all(|s| slug(s)),
        "unsafe manifest paths"
    );
    let entries = manifest
        .records
        .keys()
        .map(|id| {
            Ok((
                id.clone(),
                serde_json::from_value(read(
                    &root.join("records").join(format!("{id}.json")),
                    None,
                )?)?,
            ))
        })
        .collect::<Result<_>>()?;
    let inputs = manifest
        .evidence
        .iter()
        .map(|id| {
            Ok((
                id.clone(),
                read(&root.join("evidence").join(format!("{id}.json")), None)?,
            ))
        })
        .collect::<Result<_>>()?;
    let mut archives = BTreeMap::new();
    for (name, expected) in &manifest.archives {
        let v = read(&root.join("archive").join(format!("{name}.json")), None)?;
        ensure!(hash(&encoded(&v)) == *expected, "archive mismatch");
        archives.insert(name.clone(), v);
    }
    let r = Registry {
        manifest,
        entries,
        inputs,
        archives,
    };
    validate(&r)?;
    Ok(r)
}
pub fn save(r: &Registry, out: &Path) -> Result<()> {
    validate(r)?;
    let parent = out.parent().context("registry has no parent")?;
    fs::create_dir_all(parent)?;
    let lock_path = parent.join("registry.lock");
    let lock = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_path)
        .context("registry update already running")?;
    struct Lock(std::path::PathBuf, Option<fs::File>);
    impl Drop for Lock {
        fn drop(&mut self) {
            drop(self.1.take());
            let _ = fs::remove_file(&self.0);
        }
    }
    let _lock = Lock(lock_path, Some(lock));
    if out.exists() {
        let current = load(out)?;
        ensure!(
            r.manifest.parent.as_deref() == Some(&fingerprint(&current)?),
            "registry changed or already initialized"
        );
        let mut owned = BTreeSet::from(["registry.json".to_owned()]);
        owned.extend(
            current
                .entries
                .keys()
                .map(|id| format!("records/{id}.json")),
        );
        owned.extend(
            current
                .inputs
                .keys()
                .map(|id| format!("evidence/{id}.json")),
        );
        owned.extend(
            current
                .archives
                .keys()
                .map(|id| format!("archive/{id}.json")),
        );
        ensure!(
            crate::data::files(out)?
                .keys()
                .cloned()
                .collect::<BTreeSet<_>>()
                == owned,
            "unmanaged files in registry"
        );
    }
    let destination = out;
    let stage = tempfile::tempdir_in(parent)?;
    let ready = stage.path().join("ready");
    fs::create_dir(&ready)?;
    let out = ready.as_path();
    for folder in ["records", "evidence", "archive"] {
        fs::create_dir(out.join(folder))?;
    }
    for (id, e) in &r.entries {
        fs::write(out.join("records").join(format!("{id}.json")), bytes(e)?)?;
    }
    for (id, v) in &r.inputs {
        fs::write(out.join("evidence").join(format!("{id}.json")), encoded(v))?;
    }
    for (id, v) in &r.archives {
        fs::write(out.join("archive").join(format!("{id}.json")), encoded(v))?;
    }
    fs::write(out.join("registry.json"), bytes(&r.manifest)?)?;
    let backup = stage.path().join("previous");
    if destination.exists() {
        fs::rename(destination, &backup)?;
    }
    if let Err(error) = fs::rename(out, destination) {
        if backup.exists()
            && let Err(restore) = fs::rename(&backup, destination)
        {
            let recovery = stage.keep();
            anyhow::bail!(
                "installation failed: {error}; rollback failed: {restore}; recovery data: {}",
                recovery.display()
            );
        }
        return Err(error.into());
    }
    Ok(())
}
