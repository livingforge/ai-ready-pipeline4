use crate::data::hash;
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

pub fn encode(value: &Value) -> Vec<u8> {
    let mut bytes = serde_json::to_vec(value).expect("JSON value");
    bytes.push(b'\n');
    bytes
}
pub fn atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("missing parent")?;
    fs::create_dir_all(parent)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.write_all(bytes)?;
    temp.as_file().sync_all()?;
    temp.persist(path)?;
    Ok(())
}
pub struct Store {
    pub root: PathBuf,
    _lock: File,
    preview: Option<std::cell::RefCell<std::collections::BTreeMap<String, Vec<u8>>>>,
}

impl Store {
    pub fn open(root: &Path) -> Result<Self> {
        Self::open_mode(root, false)
    }
    pub fn open_mode(root: &Path, shared: bool) -> Result<Self> {
        fs::create_dir_all(root)?;
        let root = dunce::canonicalize(root)?;
        let lock_path = crate::data::under(&root, ".rust-workflow.lock")?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)?;
        let started = std::time::Instant::now();
        let mut delay = 25;
        loop {
            let result = if shared {
                lock.try_lock_shared()
            } else {
                lock.try_lock()
            };
            match result {
                Ok(()) => break,
                Err(std::fs::TryLockError::WouldBlock) => {
                    ensure!(
                        started.elapsed() < std::time::Duration::from_secs(30),
                        "workflow is in use by another process after waiting 30 seconds; check the active process before retrying; do not delete the lock file"
                    );
                    std::thread::sleep(std::time::Duration::from_millis(delay));
                    delay = (delay * 2).min(250);
                }
                Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
            }
        }
        Ok(Self {
            root,
            _lock: lock,
            preview: None,
        })
    }
    pub fn preview(&mut self) {
        self.preview = Some(Default::default());
    }
    pub fn managed(&self, relative: &str) -> Result<PathBuf> {
        crate::data::under(&self.root, relative)
    }
    fn path(&self, digest: &str) -> Result<PathBuf> {
        ensure!(
            digest.len() == 64
                && digest
                    .bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)),
            "invalid artifact hash"
        );
        self.managed(&format!("objects/{digest}.json"))
    }
    pub fn get(&self, digest: &str) -> Result<Vec<u8>> {
        if let Some(bytes) = self
            .preview
            .as_ref()
            .and_then(|p| p.borrow().get(digest).cloned())
        {
            return Ok(bytes);
        }
        let path = self.path(digest)?;
        let data = if path.exists() {
            fs::read(path)?
        } else {
            let mut zip = zip::ZipArchive::new(File::open(self.managed("archive.zip")?)?)?;
            let mut data = Vec::new();
            zip.by_name(digest)?.read_to_end(&mut data)?;
            data
        };
        ensure!(hash(&data) == digest, "corrupted artifact: {digest}");
        Ok(data)
    }
    pub fn put(&self, bytes: &[u8]) -> Result<String> {
        let digest = hash(bytes);
        if let Some(preview) = &self.preview {
            preview.borrow_mut().insert(digest.clone(), bytes.to_vec());
            return Ok(digest);
        }
        let path = self.path(&digest)?;
        let archive = self.managed("archive.zip")?;
        let archived = if archive.exists() {
            let mut zip = zip::ZipArchive::new(File::open(archive)?)?;
            match zip.by_name(&digest) {
                Ok(_) => true,
                Err(zip::result::ZipError::FileNotFound) => false,
                Err(e) => return Err(e.into()),
            }
        } else {
            false
        };
        if path.exists() || archived {
            ensure!(self.get(&digest)? == bytes, "artifact collision");
        } else {
            atomic(&path, bytes)?;
        }
        Ok(digest)
    }
    pub fn put_json(&self, v: &Value) -> Result<String> {
        self.put(&encode(v))
    }
    pub fn json(&self, digest: &str) -> Result<Value> {
        super::parse(&self.get(digest)?)
    }
    pub fn materialize(&self, digest: &str, path: &Path) -> Result<()> {
        atomic(path, &self.get(digest)?)
    }
    pub fn load(&self) -> Result<Value> {
        super::read(&self.managed("state.json")?)
    }
    pub fn save(&self, state: &mut Value) -> Result<()> {
        let path = self.managed("state.json")?;
        if path.exists() {
            let previous = fs::read(&path)?;
            if previous == encode(state) {
                return Ok(());
            }
            state["previous"] = json!(self.put(&previous)?);
        }
        atomic(&path, &encode(state))
    }
    pub fn compact(&self, live: &BTreeSet<String>) -> Result<Value> {
        let objects = self.managed("objects")?;
        let mut candidates = Vec::new();
        if objects.exists() {
            for entry in fs::read_dir(objects)? {
                let entry = entry?;
                let path = entry.path();
                let digest = path
                    .file_stem()
                    .and_then(|x| x.to_str())
                    .context("invalid object name")?
                    .to_owned();
                ensure!(path == self.path(&digest)?, "unmanaged object path");
                if !live.contains(&digest) {
                    candidates.push((digest.clone(), self.get(&digest)?));
                }
            }
        }
        if candidates.is_empty() {
            return Ok(json!({"archived_objects":0}));
        }
        let path = self.managed("archive.zip")?;
        let temp = tempfile::NamedTempFile::new_in(&self.root)?;
        let mut writer = zip::ZipWriter::new(temp.reopen()?);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        let mut known = BTreeSet::new();
        if path.exists() {
            let mut old = zip::ZipArchive::new(File::open(&path)?)?;
            for i in 0..old.len() {
                let mut entry = old.by_index(i)?;
                let digest = entry.name().to_owned();
                self.path(&digest)?;
                let mut bytes = Vec::new();
                entry.read_to_end(&mut bytes)?;
                ensure!(
                    hash(&bytes) == digest && known.insert(digest.clone()),
                    "corrupted archive"
                );
                writer.start_file(digest, options)?;
                writer.write_all(&bytes)?;
            }
        }
        for (digest, bytes) in &candidates {
            if known.insert(digest.clone()) {
                writer.start_file(digest, options)?;
                writer.write_all(bytes)?;
            }
        }
        writer.finish()?.sync_all()?;
        {
            let mut zip = zip::ZipArchive::new(temp.reopen()?)?;
            for (digest, bytes) in &candidates {
                let mut checked = Vec::new();
                zip.by_name(digest)?.read_to_end(&mut checked)?;
                ensure!(&checked == bytes, "archive verification failed");
            }
        }
        temp.persist(&path)?;
        for (digest, bytes) in &candidates {
            let path = self.path(digest)?;
            ensure!(
                fs::read(&path)? == *bytes,
                "object changed during compaction"
            );
            fs::remove_file(path)?;
        }
        Ok(json!({"archived_objects":candidates.len(), "archive_bytes":fs::metadata(path)?.len()}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn readers_share_lock_and_writer_waits_for_release() {
        let dir = tempfile::tempdir().unwrap();
        let first = Store::open_mode(dir.path(), true).unwrap();
        let second = Store::open_mode(dir.path(), true).unwrap();
        let probe = OpenOptions::new()
            .read(true)
            .write(true)
            .open(dir.path().join(".rust-workflow.lock"))
            .unwrap();
        assert!(matches!(
            probe.try_lock(),
            Err(std::fs::TryLockError::WouldBlock)
        ));
        let path = dir.path().to_owned();
        let (tx, rx) = std::sync::mpsc::channel();
        let writer = std::thread::spawn(move || {
            let _store = Store::open(&path).unwrap();
            tx.send(()).unwrap();
        });
        assert!(
            rx.recv_timeout(std::time::Duration::from_millis(100))
                .is_err()
        );
        drop(first);
        assert!(
            rx.recv_timeout(std::time::Duration::from_millis(100))
                .is_err()
        );
        drop(second);
        rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        writer.join().unwrap();
    }
    #[test]
    fn deduplicates_archives_and_preserves_history() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let one = store.put(&b"old source".repeat(100)).unwrap();
        let two = store.put(b"current").unwrap();
        assert_eq!(store.put(&b"old source".repeat(100)).unwrap(), one);
        store.save(&mut json!({"input":one})).unwrap();
        let mut state = json!({"input":two});
        store.save(&mut state).unwrap();
        let previous = state["previous"].as_str().unwrap();
        let live = BTreeSet::from([two.clone()]);
        let report = store.compact(&live).unwrap();
        assert!(report["archived_objects"].as_u64().unwrap() >= 2);
        assert_eq!(store.get(&one).unwrap(), b"old source".repeat(100));
        assert_eq!(store.json(previous).unwrap(), json!({"input":one}));
        assert_eq!(store.put(&b"old source".repeat(100)).unwrap(), one);
        assert!(!store.path(&one).unwrap().exists());
        assert!(store.path(&two).unwrap().exists());
        assert_eq!(store.compact(&live).unwrap()["archived_objects"], 0);
    }
    #[test]
    fn corrupted_objects_and_paths_are_never_removed() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let digest = store.put(b"original").unwrap();
        let path = store.path(&digest).unwrap();
        fs::write(&path, b"corrupt").unwrap();
        assert!(store.get(&digest).is_err());
        assert!(store.compact(&BTreeSet::new()).is_err());
        assert!(path.exists());
        assert!(store.path("../outside").is_err());
        assert!(store.managed("latest/../outside").is_err());
    }
}
