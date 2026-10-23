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
/// The manifest of the registry at `root`, which names its files.
fn manifest(root: &Path) -> Result<Manifest> {
    let manifest: Manifest = serde_json::from_value(read(&root.join("registry.json"), None)?)?;
    ensure!(
        manifest.records.keys().all(|s| valid_id(s))
            && manifest.evidence.iter().all(|s| digest(s))
            && manifest.archives.keys().all(|s| slug(s)),
        "unsafe manifest paths"
    );
    Ok(manifest)
}
pub fn load(root: &Path) -> Result<Registry> {
    Ok(load_checked(root)?.0)
}
/// [`load`] with the snapshots it read.
fn load_checked(root: &Path) -> Result<(Registry, Snapshots)> {
    let manifest = manifest(root)?;
    // The files are read on several threads: a registry holds a record per
    // entry and a snapshot per capture its records cite.
    let ids: Vec<&String> = manifest.records.keys().collect();
    let entries = crate::data::in_order(&ids, |id| -> Result<Entry> {
        Ok(serde_json::from_value(read(
            &root.join("records").join(format!("{id}.json")),
            None,
        )?)?)
    })
    .into_iter()
    .zip(&ids)
    .map(|(entry, id)| Ok(((*id).clone(), entry?)))
    .collect::<Result<_>>()?;
    let ids: Vec<&String> = manifest.evidence.iter().collect();
    let inputs = crate::data::in_order(&ids, |id| {
        read(&root.join("evidence").join(format!("{id}.json")), None)
    })
    .into_iter()
    .zip(&ids)
    .map(|(input, id)| Ok(((*id).clone(), input?)))
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
    let mut snapshots = Snapshots::default();
    validate_with(&r, &mut snapshots)?;
    Ok((r, snapshots))
}
/// Writes a file of the staged registry. One the current registry already holds
/// with the same bytes is linked instead: Windows virus scanning reads every newly
/// written file again when it is next opened, so rewriting all records for one
/// change made the next command wait about a minute for 6,000 records.
fn stage_file(current: Option<&Path>, out: &Path, relative: &str, bytes: &[u8]) -> Result<()> {
    let path = out.join(relative);
    if let Some(current) = current {
        let existing = current.join(relative);
        // A file system without hard links gets a written copy.
        if fs::read(&existing).is_ok_and(|old| old == bytes)
            && fs::hard_link(&existing, &path).is_ok()
        {
            return Ok(());
        }
    }
    fs::write(path, bytes)?;
    Ok(())
}
pub fn save(r: &Registry, out: &Path) -> Result<()> {
    validate(r)?;
    write(r, out)
}
/// Applies `change` to the registry at `root` and saves it. [`apply`]
/// validates what it gives, so it is written without validating it again:
/// each validation reads every evidence snapshot a record cites, and those
/// grow with the changes. The snapshots read when loading are not read
/// again when validating the change.
pub fn update(root: &Path, change: Change, additional: Vec<spec::Input>) -> Result<Registry> {
    let (r, snapshots) = load_checked(root)?;
    let r = apply_with(r, change, additional, snapshots)?;
    write(&r, root)?;
    Ok(r)
}
/// Writes the validated registry `r` over the one at `out`, which must be the
/// one it was changed from.
fn write(r: &Registry, out: &Path) -> Result<()> {
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
        // The manifest names the registry and its files; the registry was
        // validated when it was read to be changed.
        let current = manifest(out)?;
        ensure!(
            r.manifest.parent.as_deref() == Some(&hash(&bytes(&current)?)),
            "registry changed or already initialized"
        );
        let mut owned = BTreeSet::from(["registry.json".to_owned()]);
        owned.extend(
            current
                .records
                .keys()
                .map(|id| format!("records/{id}.json")),
        );
        owned.extend(
            current
                .evidence
                .iter()
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
    // The registry is .arp/registry: stage it in the untracked .arp/work so a
    // failed save leaves nothing beside the tracked registry.
    let stage = crate::data::Stage::new_in(&parent.join("work"), parent)?;
    let ready = stage.path().join("ready");
    fs::create_dir(&ready)?;
    let out = ready.as_path();
    for folder in ["records", "evidence", "archive"] {
        fs::create_dir(out.join(folder))?;
    }
    let current = destination.exists().then_some(destination);
    for (id, e) in &r.entries {
        stage_file(current, out, &format!("records/{id}.json"), &bytes(e)?)?;
    }
    for (id, v) in &r.inputs {
        stage_file(current, out, &format!("evidence/{id}.json"), &encoded(v))?;
    }
    for (id, v) in &r.archives {
        stage_file(current, out, &format!("archive/{id}.json"), &encoded(v))?;
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
                "registry update failed: {error}; rollback failed: {restore}; recovery data: {}",
                recovery.display()
            );
        }
        return Err(error.into());
    }
    Ok(())
}
