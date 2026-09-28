mod mapping;
use mapping::*;
mod adoption;
pub use adoption::Batch;
mod diff;
mod export;
mod import;
use import::Carry;
mod inspection;
pub mod search;
mod sheet_edit;
pub use sheet_edit::{Axis, EditKind, Position, SheetEdit};
mod slide_edit;
pub use slide_edit::{SlideEdit, SlideEditKind};

use crate::{
    data::*,
    document_source::{Source, is_slide_operation, parse_slide_operations, slide_view},
    excel,
};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet, HashMap},
    fs,
    path::{Path, PathBuf},
};

const RECORDS: [&str; 6] = [
    "document.yml",
    "mappings.yml",
    "formation.json",
    "review.json",
    "extraction.json",
    "prompt.txt",
];
pub struct Store {
    pub root: PathBuf,
    pub arp: PathBuf,
}
pub struct Inspection {
    pub meta: Value,
    pub extraction: Value,
    pub mappings: Value,
    /// Content values by page, block and field.
    pub values: HashMap<(String, String, String), Value>,
    pub fingerprint: String,
    pub reviewed: bool,
    pub source_current: bool,
    pub source_missing: bool,
}
fn nonempty(value: &str) -> Result<()> {
    ensure!(
        !value.trim().is_empty(),
        "nonempty actor/model/reviewer required"
    );
    Ok(())
}

impl Store {
    /// The interpretation of the adopted document, or of its proposal, and the
    /// report of its replay: a view of the extraction plus the corrections in
    /// mappings.yml.
    pub fn structure(&self, id: &str, proposal: bool) -> Result<(Value, Value)> {
        let dir = self.structure_dir(id, proposal)?;
        let inspected = self.inspect(&dir, false)?;
        self.interpretation(&inspected)?
            .context("document has no structure interpretation")
    }

    /// Saves `structure` as the corrections of the adopted document, or of its
    /// proposal, before it is recorded and adopted.
    pub fn save_structure(&self, id: &str, structure: &Value, proposal: bool) -> Result<Value> {
        let dir = self.structure_dir(id, proposal)?;
        let inspected = self.inspect(&dir, false)?;
        ensure!(
            inspected.source_current,
            "source changed; re-import before saving structure"
        );
        ensure!(
            self.interpretation(&inspected)?.is_some(),
            "document has no structure interpretation"
        );
        let journal = crate::document_structure::record_corrections(
            &self.root,
            &inspected.extraction,
            structure,
        )?;
        let (reconstructed, report) = crate::document_structure::replay_corrections(
            &self.root,
            &journal,
            &inspected.extraction,
        )?;
        ensure!(
            reconstructed == *structure,
            "structure cannot be reproduced from corrections"
        );
        let path = dir.join("mappings.yml");
        let mut mappings = read(&path, Some("mappings"))?;
        mappings["interpretation"] = journal;
        ensure!(
            self.fingerprint(&dir)?.as_deref() == Some(&inspected.fingerprint),
            "document changed while saving structure"
        );
        write(&path, &mappings)?;
        Ok(report)
    }

    fn structure_dir(&self, id: &str, proposal: bool) -> Result<PathBuf> {
        if proposal {
            self.proposal(id)
        } else {
            self.document(id)
        }
    }

    pub fn init(root: &Path, sources: &str) -> Result<Self> {
        let root = crate::project::init(root, sources)?;
        Self::open(&root)
    }
    pub fn open(root: &Path) -> Result<Self> {
        let root = crate::project::open(root)?;
        let arp = under(&root, ".arp")?;
        Ok(Self { root, arp })
    }
    /// The directory holding the originals; document IDs are paths below it.
    pub fn sources(&self) -> Result<PathBuf> {
        let config = read(
            &under(&self.root, ".arp/config.yml")?,
            Some("project-config"),
        )?;
        under(&self.root, string(&config["sources"])?)
    }
    /// The original named by a document ID, if it exists under exactly that name.
    /// Windows and macOS would also open a file whose name differs in letter case.
    pub fn original(&self, relative: &str) -> Result<Option<PathBuf>> {
        let path = under(&self.root, relative)?;
        if !path.is_file() {
            return Ok(None);
        }
        let actual = dunce::canonicalize(&path)?;
        let exact = actual
            .strip_prefix(&self.root)
            .is_ok_and(|p| p.to_string_lossy().replace('\\', "/") == relative);
        Ok(exact.then_some(path))
    }
    /// The IDs of the documents (`documents`) or proposals (`changes`) at or below `scope`.
    /// A directory holding the area's record is an entry; other directories are folders.
    pub fn ids(&self, area: &str, scope: Option<&str>) -> Result<Vec<String>> {
        let (marker, base) = match area {
            "documents" => ("document.yml", under(&self.arp, "documents")?),
            "changes" => ("proposal.json", under(&self.arp, "changes")?),
            _ => bail!("unknown document area: {area}"),
        };
        let mut out = vec![];
        let start = match scope {
            Some(scope) => under(&base, scope)?,
            None => base.clone(),
        };
        let mut pending = vec![start];
        while let Some(dir) = pending.pop() {
            if !dir.is_dir() {
                continue;
            }
            if dir != base && dir.join(marker).is_file() {
                out.push(
                    dir.strip_prefix(&base)?
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
                continue;
            }
            for entry in fs::read_dir(&dir)? {
                let entry = entry?;
                let path = entry.path();
                let meta = entry.metadata()?;
                ensure!(!is_link(&meta), "links are not allowed: {}", path.display());
                if meta.is_dir() {
                    pending.push(path);
                }
            }
        }
        out.sort();
        Ok(out)
    }
    pub fn document(&self, id: &str) -> Result<PathBuf> {
        self.entry("documents", id)
    }
    pub fn proposal(&self, id: &str) -> Result<PathBuf> {
        let dir = self.entry("changes", id)?;
        ensure!(
            dir.join("proposal.json").is_file(),
            "missing proposal: {id}"
        );
        Ok(dir)
    }
    fn entry(&self, area: &str, id: &str) -> Result<PathBuf> {
        document_id(id)?;
        let base = under(&self.arp, area)?;
        // Windows and macOS file names ignore case, so `spec` would open, and adopting
        // it would replace, the directory of an existing `Spec`.
        let mut dir = base.clone();
        for segment in id.split('/') {
            if !dir.is_dir() {
                break;
            }
            for entry in fs::read_dir(&dir)? {
                let name = entry?.file_name();
                let name = name.to_string_lossy();
                ensure!(
                    name == segment || name.to_lowercase() != segment.to_lowercase(),
                    "document ID {id} differs from existing {name} only in letter case"
                );
            }
            dir.push(segment);
        }
        under(&base, id)
    }
    pub fn management(&self, dir: &Path) -> Result<PathBuf> {
        Ok(dir.to_owned())
    }
    fn logical(&self, dir: &Path) -> Result<BTreeMap<String, PathBuf>> {
        files(dir)
    }
    fn image_assets(
        &self,
        dir: &Path,
        operations: &[excel::ImageOperation],
    ) -> Result<BTreeMap<String, Vec<u8>>> {
        let logical = self.logical(dir)?;
        let mut assets = BTreeMap::new();
        for operation in operations {
            let path = logical
                .get(&operation.asset)
                .with_context(|| format!("image asset missing: {}", operation.asset))?;
            let bytes = fs::read(path)?;
            excel::validate_image_asset(&operation.asset, &bytes)?;
            assets.insert(operation.asset.clone(), bytes);
        }
        Ok(assets)
    }
    pub fn fingerprint(&self, dir: &Path) -> Result<Option<String>> {
        Ok(self.file_hashes(dir)?.map(|files| fingerprint_of(&files)))
    }
    /// The hash of each fingerprinted file, with line endings normalized in text files.
    /// Records written by ARP are canonical, so a record's hash is also the hash of
    /// its canonical encoding.
    fn file_hashes(&self, dir: &Path) -> Result<Option<serde_json::Map<String, Value>>> {
        self.file_hashes_with(dir, &BTreeMap::new())
    }
    /// The document's files as an edit leaves them: the files of `dir` with
    /// the files `planned` adds, less those it removes.
    fn planned_files(&self, dir: &Path, planned: &Planned) -> Result<BTreeMap<String, PathBuf>> {
        let mut files = self.logical(dir)?;
        for (name, bytes) in planned {
            if bytes.is_some() {
                if !files.contains_key(name) {
                    files.insert(name.clone(), under(dir, name)?);
                }
            } else {
                files.remove(name);
            }
        }
        Ok(files)
    }
    /// Like [`Self::file_hashes`], with the files in `planned` holding the given
    /// bytes, added or removed.
    fn file_hashes_with(
        &self,
        dir: &Path,
        planned: &Planned,
    ) -> Result<Option<serde_json::Map<String, Value>>> {
        if !self.management(dir)?.join("document.yml").exists() {
            return Ok(None);
        }
        let mut map = serde_json::Map::new();
        for (name, path) in self.planned_files(dir, planned)? {
            if [
                "review.json",
                "formation.json",
                "proposal.json",
                "prompt.txt",
            ]
            .contains(&name.as_str())
            {
                continue;
            }
            let mut raw = match planned.get(&name) {
                Some(Some(bytes)) => bytes.clone(),
                _ => fs::read(&path)?,
            };
            if ["md", "yml", "yaml", "json"]
                .contains(&path.extension().and_then(|s| s.to_str()).unwrap_or(""))
            {
                raw = String::from_utf8(raw)?.replace("\r\n", "\n").into_bytes()
            };
            map.insert(name, json!(hash(&raw)));
        }
        Ok(Some(map))
    }
}

/// Files an edit writes (Some) or removes (None), by name like `mappings.yml`.
type Planned = BTreeMap<String, Option<Vec<u8>>>;

/// The record `name` at `path`, or its planned bytes when an edit writes it.
fn read_planned(name: &str, path: &Path, planned: &Planned, schema: &str) -> Result<Value> {
    let Some(Some(bytes)) = planned.get(name) else {
        return read(path, Some(schema));
    };
    let value = parse(std::str::from_utf8(bytes)?, name.ends_with(".json"))?;
    validate(schema, &value)?;
    Ok(value)
}

fn fingerprint_of(files: &serde_json::Map<String, Value>) -> String {
    hash(&encoded(&Value::Object(files.clone())))
}
