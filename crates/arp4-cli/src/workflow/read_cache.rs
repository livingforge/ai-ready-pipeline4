//! Laid-out reads kept between the commands of one reader. An agent reads a
//! large task in dozens of pages, each a separate command; building the task and
//! its fragments again for every page made reading grow with the square of the
//! task size. A layout is keyed by everything the task view is built from, so a
//! changed task is laid out again.
use super::paging::{Layout, View};
use super::*;
use std::io::{Read, Seek, SeekFrom};

/// The stored view and the byte offset of each fragment line in the data file.
#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct Index {
    key: String,
    pub view: View,
    offsets: Vec<u64>,
}

// File names use digest prefixes: full digests of the task and the key would
// exceed Windows' 260-character path limit below a deep working directory. The
// full key is kept in the index and checked on load.
const TASK_PREFIX: usize = 16;
const KEY_PREFIX: usize = 32;

pub(super) struct ReadCache {
    directory: PathBuf,
    key: String,
}

impl ReadCache {
    pub(super) fn new(root: &Path, task: &str, key: String) -> Self {
        Self {
            directory: root
                .join("read-cache")
                .join(&task[..task.len().min(TASK_PREFIX)]),
            key,
        }
    }

    fn index_path(&self) -> PathBuf {
        self.directory
            .join(format!("{}.index.json", &self.key[..KEY_PREFIX]))
    }

    fn data_path(&self) -> PathBuf {
        self.directory
            .join(format!("{}.jsonl", &self.key[..KEY_PREFIX]))
    }

    /// The stored index, or `None` when this read has not been laid out.
    pub(super) fn load(&self) -> Option<Index> {
        let index: Index = serde_json::from_slice(&fs::read(self.index_path()).ok()?).ok()?;
        (index.key == self.key && index.offsets.len() == index.view.sizes.len() + 1)
            .then_some(index)
    }

    pub(super) fn fetch(&self, index: &Index, range: std::ops::Range<usize>) -> Result<Vec<Value>> {
        let (start, end) = (index.offsets[range.start], index.offsets[range.end]);
        let mut file = fs::File::open(self.data_path())?;
        file.seek(SeekFrom::Start(start))?;
        let mut bytes = vec![0; usize::try_from(end - start)?];
        file.read_exact(&mut bytes)?;
        bytes
            .split(|b| *b == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| Ok(serde_json::from_slice(line)?))
            .collect()
    }

    /// Writes the data before the index, each atomically, so a reader that finds
    /// an index also finds its data.
    pub(super) fn store(&self, layout: &Layout) -> Result<()> {
        let mut data = Vec::new();
        let mut offsets = vec![0];
        for fragment in &layout.fragments {
            serde_json::to_writer(&mut data, fragment)?;
            data.push(b'\n');
            offsets.push(data.len() as u64);
        }
        fs::create_dir_all(&self.directory)?;
        super::store::atomic(&self.data_path(), &data)?;
        let index = Index {
            key: self.key.clone(),
            view: View {
                revision: layout.view.revision.clone(),
                content_bytes: layout.view.content_bytes,
                array: layout.view.array,
                sizes: layout.view.sizes.clone(),
            },
            offsets,
        };
        super::store::atomic(&self.index_path(), &serde_json::to_vec(&index)?)
    }
}

/// Removes the layouts of tasks that no longer take replies.
pub(super) fn prune(root: &Path, open: &BTreeSet<&str>) -> Result<()> {
    let open: BTreeSet<_> = open
        .iter()
        .map(|task| &task[..task.len().min(TASK_PREFIX)])
        .collect();
    let directory = root.join("read-cache");
    if !directory.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if !entry
            .file_name()
            .to_str()
            .is_some_and(|task| open.contains(task))
        {
            fs::remove_dir_all(entry.path())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::paging::{ReadOptions, layout, select};
    use super::*;
    use crate::agent_format::AgentFormat;

    #[test]
    fn stored_layouts_serve_the_same_pages() {
        let rows: Vec<_> = (0..400)
            .map(|i| {
                json!([
                    format!("s{i}"),
                    0,
                    format!("A{i}"),
                    "value",
                    "原文\n\"条件\"".repeat(i % 7 + 1)
                ])
            })
            .collect();
        let data = json!({"packet":{"sources":{"rows":rows},"note":"長い説明".repeat(3000)},"scope":{"sources":["s1"]}});
        let dir = tempfile::tempdir().unwrap();
        for (format, pointer, limit, max_bytes) in [
            (AgentFormat::Json, "", 10000, 4096),
            (AgentFormat::Toon, "", 7, 12000),
            (AgentFormat::Json, "/packet/sources/rows", 3, 8192),
            (AgentFormat::Json, "/scope", 1, 4096),
        ] {
            let options = |offset| ReadOptions {
                format,
                pointer,
                offset,
                limit,
                max_bytes,
                revision: None,
            };
            let laid = layout(&data, &["packet", "scope"], &options(0)).unwrap();
            let key = crate::data::hash(format!("{pointer}{max_bytes}{format:?}").as_bytes());
            let cache = ReadCache::new(dir.path(), "task", key.clone());
            assert!(cache.load().is_none());
            cache.store(&laid).unwrap();
            let index = cache.load().unwrap();
            let other = crate::data::hash(b"other");
            let stale = ReadCache::new(
                dir.path(),
                "task",
                format!("{}{}", &key[..32], &other[32..]),
            );
            assert!(
                stale.load().is_none(),
                "a key sharing the file prefix is not served"
            );
            for offset in 0..laid.view.sizes.len() {
                let direct = select(
                    "t",
                    &laid.view,
                    |r| Ok(laid.fragments[r].to_vec()),
                    &options(offset),
                );
                let stored = select(
                    "t",
                    &index.view,
                    |r| cache.fetch(&index, r),
                    &options(offset),
                );
                assert_eq!(direct.unwrap(), stored.unwrap(), "{pointer} at {offset}");
            }
        }
        prune(dir.path(), &BTreeSet::from(["other"])).unwrap();
        assert!(!dir.path().join("read-cache").join("task").exists());
    }
}
