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
/// Parsed artifacts are kept up to this many source bytes per process.
const PARSED_CACHE_BYTES: usize = 256 << 20;
/// How long a command waits for another process to release the workflow.
const LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

pub struct Store {
    pub root: PathBuf,
    _lock: File,
    preview: Option<std::cell::RefCell<std::collections::BTreeMap<String, Vec<u8>>>>,
    /// Artifacts already parsed in this process with their source sizes. A digest
    /// names its content, so the value is the one a new parse would return; one
    /// command otherwise parses the same input, packet and replies many times.
    parsed: std::cell::RefCell<(usize, std::collections::HashMap<String, Value>)>,
    /// The names in `archive.zip`, read once in this process instead of for
    /// every artifact put; compaction replaces them.
    archived: std::cell::RefCell<Option<std::collections::BTreeSet<String>>>,
    /// `archive.zip` opened once in this process for the objects read from
    /// it, rather than reading its directory again for each; compaction
    /// closes it before replacing the file.
    archive: std::cell::RefCell<Option<zip::ZipArchive<File>>>,
}

impl Store {
    pub fn open(root: &Path) -> Result<Self> {
        Self::open_mode(root, false)
    }
    pub fn open_mode(root: &Path, shared: bool) -> Result<Self> {
        Self::open_waiting(root, shared, LOCK_WAIT)
    }
    fn open_waiting(root: &Path, shared: bool, wait: std::time::Duration) -> Result<Self> {
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
                        started.elapsed() < wait,
                        "workflow is in use by another process after waiting {} seconds; check the active process before retrying; do not delete the lock file",
                        wait.as_secs()
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
            parsed: Default::default(),
            archived: Default::default(),
            archive: Default::default(),
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
            let mut archive = self.archive.borrow_mut();
            if archive.is_none() {
                *archive = Some(zip::ZipArchive::new(File::open(
                    self.managed("archive.zip")?,
                )?)?);
            }
            let mut data = Vec::new();
            archive
                .as_mut()
                .unwrap()
                .by_name(digest)?
                .read_to_end(&mut data)?;
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
        if path.exists() || self.is_archived(&digest)? {
            ensure!(self.get(&digest)? == bytes, "artifact collision");
        } else {
            atomic(&path, bytes)?;
        }
        Ok(digest)
    }
    fn is_archived(&self, digest: &str) -> Result<bool> {
        let mut archived = self.archived.borrow_mut();
        if archived.is_none() {
            let archive = self.managed("archive.zip")?;
            *archived = Some(if archive.exists() {
                zip::ZipArchive::new(File::open(archive)?)?
                    .file_names()
                    .map(str::to_owned)
                    .collect()
            } else {
                Default::default()
            });
        }
        Ok(archived.as_ref().unwrap().contains(digest))
    }
    pub fn put_json(&self, v: &Value) -> Result<String> {
        self.put(&encode(v))
    }
    pub fn json(&self, digest: &str) -> Result<Value> {
        if let Some(value) = self.parsed.borrow().1.get(digest) {
            return Ok(value.clone());
        }
        let bytes = self.get(digest)?;
        let value = super::parse(&bytes)?;
        let mut parsed = self.parsed.borrow_mut();
        if parsed.0 + bytes.len() > PARSED_CACHE_BYTES {
            *parsed = Default::default();
        }
        parsed.0 += bytes.len();
        parsed.1.insert(digest.to_owned(), value.clone());
        Ok(value)
    }
    pub fn materialize(&self, digest: &str, path: &Path) -> Result<()> {
        atomic(path, &self.get(digest)?)
    }
    pub fn load(&self) -> Result<Value> {
        super::read(&self.managed("state.json")?)
    }
    /// Writes `state` over the current state. Only the current state is kept:
    /// a copy of each earlier one grew with the state times its saves.
    pub fn save(&self, state: &Value) -> Result<()> {
        let path = self.managed("state.json")?;
        let bytes = encode(state);
        if path.exists() && fs::read(&path)? == bytes {
            return Ok(());
        }
        atomic(&path, &bytes)
    }
    pub fn compact(&self, live: &BTreeSet<String>) -> Result<Value> {
        let objects = self.managed("objects")?;
        let mut candidates = Vec::new();
        if objects.exists() {
            for entry in fs::read_dir(&objects)? {
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
        // The archive is replaced below; a later read opens the new one.
        *self.archive.borrow_mut() = None;
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
        *self.archived.borrow_mut() = Some(known);
        for (digest, bytes) in &candidates {
            let path = self.path(digest)?;
            ensure!(
                fs::read(&path)? == *bytes,
                "object changed during compaction"
            );
            fs::remove_file(path)?;
        }
        crate::data::remove_empty_directories(&objects, &self.root);
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
    fn writer_gives_up_while_another_process_holds_the_lock() {
        let dir = tempfile::tempdir().unwrap();
        let reader = Store::open_mode(dir.path(), true).unwrap();
        let error = Store::open_waiting(dir.path(), false, std::time::Duration::from_millis(200))
            .err()
            .unwrap();
        assert!(
            error
                .to_string()
                .contains("workflow is in use by another process")
        );
        drop(reader);
        Store::open_waiting(dir.path(), false, std::time::Duration::from_millis(200)).unwrap();
    }
    #[test]
    fn deduplicates_archives_and_keeps_only_the_current_state() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let one = store.put(&b"old source".repeat(100)).unwrap();
        let two = store.put(b"current").unwrap();
        assert_eq!(store.put(&b"old source".repeat(100)).unwrap(), one);
        store.save(&json!({"input":one})).unwrap();
        let state = json!({"input":two});
        store.save(&state).unwrap();
        // Saving stores no copy of the state it replaces.
        assert_eq!(store.load().unwrap(), state);
        let objects = || fs::read_dir(dir.path().join("objects")).map_or(0, |d| d.count());
        assert_eq!(objects(), 2);
        let live = BTreeSet::from([two.clone()]);
        let report = store.compact(&live).unwrap();
        assert_eq!(report["archived_objects"], 1);
        assert_eq!(store.get(&one).unwrap(), b"old source".repeat(100));
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
