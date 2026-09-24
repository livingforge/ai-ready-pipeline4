use super::{Store, obj, read, s};
use crate::data::{hash, under};
use anyhow::{Context, Result, ensure};
use std::{
    fs::{self, OpenOptions},
    path::Path,
};

/// Publish only a completed run. Stage and verify the entire bundle before replacement.
/// Kept separate from binary-version checks so historical completed runs can be published.
pub fn publish(root: &Path, output: &Path) -> Result<bool> {
    let store = Store::open(root)?;
    let state = store.load()?;
    if state["status"] != "complete" {
        return Ok(false);
    }
    ensure!(
        state["version"] == 1 && state["engine"] == "arp4-rust",
        "unsupported workflow state"
    );
    let exports = obj(&state["exports"])?;
    ensure!(
        exports.contains_key("design.md"),
        "completed run has no design.md"
    );
    let parent = output.parent().context("output has no parent")?;
    fs::create_dir_all(parent)?;
    let parent = dunce::canonicalize(parent)?;
    let name = output
        .file_name()
        .and_then(|v| v.to_str())
        .context("invalid output directory")?;
    let output = under(&parent, name)?;
    ensure!(
        !output.starts_with(&store.root) && !store.root.starts_with(&output),
        "output overlaps workflow directory"
    );
    let lock_path = under(&parent, &format!(".{name}.publish.lock"))?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)?;
    lock.try_lock()
        .context("output is being published by another process")?;
    let backup = under(&parent, &format!(".{name}.previous"))?;
    ensure!(
        !backup.exists(),
        "previous publication backup exists; inspect {} before retrying",
        backup.display()
    );
    if output.exists() {
        let manifest =
            read(&under(&output, "manifest.json")?).context("output is not a managed export")?;
        for entry in fs::read_dir(&output)? {
            let entry = entry?;
            let filename = entry.file_name();
            let filename = filename.to_str().context("invalid export filename")?;
            let path = under(&output, filename)?;
            if filename != "manifest.json" {
                let digest = manifest
                    .get(filename)
                    .context("unmanaged file in output directory")?;
                ensure!(
                    hash(&fs::read(path)?) == s(digest)?,
                    "modified output: {filename}"
                );
            }
        }
    }
    let stage = tempfile::Builder::new()
        .prefix(".arp4-publish-")
        .tempdir_in(&parent)?;
    for (name, digest) in exports {
        ensure!(
            Path::new(name).file_name().and_then(|v| v.to_str()) == Some(name)
                && name != "manifest.json",
            "invalid export name"
        );
        fs::write(under(stage.path(), name)?, store.get(s(digest)?)?)?;
    }
    fs::write(
        stage.path().join("manifest.json"),
        super::store::encode(&state["exports"]),
    )?;
    let had_output = output.exists();
    if had_output {
        fs::rename(&output, &backup)?;
    }
    if let Err(error) = fs::rename(stage.path(), &output) {
        if had_output {
            fs::rename(&backup, &output)
                .context("publication failed; restore previous backup manually")?;
        }
        return Err(error.into());
    }
    if had_output {
        fs::remove_dir_all(&backup)?;
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn publication_preserves_previous_output_until_complete_and_rejects_edits() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("run");
        let output = temp.path().join("output/project");
        let set = |status: &str, text: &str| {
            let store = Store::open(&root).unwrap();
            let digest = store.put(text.as_bytes()).unwrap();
            let mut state = json!({"version":1,"engine":"arp4-rust","status":status,"exports":{"design.md":digest}});
            store.save(&mut state).unwrap();
        };
        set("complete", "first");
        assert!(publish(&root, &output).unwrap());
        set("blocked", "second");
        assert!(!publish(&root, &output).unwrap());
        assert_eq!(
            fs::read_to_string(output.join("design.md")).unwrap(),
            "first"
        );
        set("complete", "second");
        assert!(publish(&root, &output).unwrap());
        assert_eq!(
            fs::read_to_string(output.join("design.md")).unwrap(),
            "second"
        );
        fs::write(output.join("design.md"), "user edits").unwrap();
        assert!(publish(&root, &output).is_err());
        assert_eq!(
            fs::read_to_string(output.join("design.md")).unwrap(),
            "user edits"
        );
    }
}
