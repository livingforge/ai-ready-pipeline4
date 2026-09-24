mod mapping;
use mapping::*;
mod adoption;
mod diff;
mod export;
mod import;
mod inspection;

use crate::{data::*, document_source::Source, excel};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
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
    pub values: BTreeMap<(String, String, String), Value>,
    pub fingerprint: String,
    pub reviewed: bool,
    pub source_current: bool,
}
fn nonempty(value: &str) -> Result<()> {
    ensure!(
        !value.trim().is_empty(),
        "nonempty actor/model/reviewer required"
    );
    Ok(())
}

impl Store {
    pub fn init(root: &Path, sources: &str) -> Result<Self> {
        let root = crate::project::init(root, sources)?;
        Self::open(&root)
    }
    pub fn open(root: &Path) -> Result<Self> {
        let root = crate::project::open(root)?;
        let arp = under(&root, ".arp")?;
        Ok(Self { root, arp })
    }
    fn index(&self) -> Result<Value> {
        let mut index = json!({});
        for (name, path) in files(&under(&self.arp, "documents")?)? {
            if name.ends_with("/document.yml") && name.split('/').count() == 2 {
                let meta = read(&path, Some("document"))?;
                index[string(&meta["document_id"])?] = meta["source"]["path"].clone();
            }
        }
        Ok(index)
    }
    pub fn document(&self, id: &str) -> Result<PathBuf> {
        identifier(id)?;
        under(&self.arp.join("documents"), id)
    }
    pub fn proposal(&self, id: &str) -> Result<PathBuf> {
        identifier(id)?;
        let mut matches = vec![];
        for (name, path) in files(&under(&self.arp, "changes")?)? {
            if name.ends_with("/proposal.json")
                && path
                    .parent()
                    .and_then(Path::file_name)
                    .is_some_and(|s| s == id)
            {
                matches.push(path.parent().unwrap().to_path_buf());
            }
        }
        ensure!(matches.len() == 1, "missing or ambiguous proposal: {id}");
        Ok(matches.remove(0))
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
        if !self.management(dir)?.join("document.yml").exists() {
            return Ok(None);
        }
        let mut map = serde_json::Map::new();
        for (name, path) in self.logical(dir)? {
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
            let mut raw = fs::read(&path)?;
            if ["md", "yml", "yaml", "json"]
                .contains(&path.extension().and_then(|s| s.to_str()).unwrap_or(""))
            {
                raw = String::from_utf8(raw)?.replace("\r\n", "\n").into_bytes()
            };
            map.insert(name, json!(hash(&raw)));
        }
        Ok(Some(hash(&encoded(&Value::Object(map)))))
    }
}
