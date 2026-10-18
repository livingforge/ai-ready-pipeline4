mod mapping;
use mapping::*;
mod adoption;
pub use adoption::Batch;
mod diff;
mod export;
mod import;
use import::Carry;
pub use import::Unprotection;
mod inspection;
pub mod search;
mod sheet_edit;
pub use sheet_edit::{Axis, EditKind, Position, SheetEdit};
mod elements;
pub(crate) use elements::CONTENT_VERSION;
mod export_confirmation;
mod identity;
mod image_edit;
mod operation_edit;
mod render;
mod shape_edit;
mod slide_edit;
mod value_edit;
mod visual_elements;
pub use image_edit::ImageRequest;
pub use render::RenderPlace;
pub use shape_edit::ShapeRequest;
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

const RECORDS: [&str; 7] = [
    "document.yml",
    "mappings.yml",
    "formation.json",
    "review.json",
    "extraction.json",
    "prompt.txt",
    "layout.yml",
];

/// The first positive suffix not already used by this operation prefix.
fn next_operation_id(recorded: &[Value], prefix: &str) -> String {
    let used: BTreeSet<_> = recorded.iter().filter_map(|o| o["id"].as_str()).collect();
    (1..)
        .map(|n| format!("{prefix}-{n}"))
        .find(|id| !used.contains(id.as_str()))
        .unwrap()
}

pub struct Store {
    pub root: PathBuf,
    pub arp: PathBuf,
}
pub struct Inspection {
    pub meta: Value,
    pub extraction: Value,
    pub mappings: Value,
    /// The complete mapping entries; `mappings` keeps none.
    entries: Vec<Entry>,
    /// Content values by page, block and field.
    pub values: HashMap<(String, String, String), Value>,
    /// The fonts of content values by page, block and field (see `crate::fonts`).
    pub fonts: HashMap<(String, String, String), Value>,
    /// Content file name by validated page ID.
    pub page_files: BTreeMap<String, String>,
    pub fingerprint: String,
    pub reviewed: bool,
    pub source_current: bool,
    pub source_missing: bool,
    /// The identities of the rows and columns of the extraction, which the
    /// correction journal names cells by.
    pub axes: crate::document_structure::Axes,
    /// The digest of the structure interpretation the layout's element
    /// metadata was classified from (`elements::interpretation_digest`).
    pub structure_sha256: Option<String>,
}
impl Inspection {
    /// The complete mapping entries, as mappings.yml writes them.
    pub fn entries_json(&self) -> Vec<Value> {
        self.entries.iter().map(|e| e.json().into_owned()).collect()
    }

    /// Lets go of the content values and fonts and the mapping entries, the
    /// largest parts besides the extraction, before a step that reads none of
    /// them, such as replaying the structure.
    pub(super) fn release_content(&mut self) {
        self.values = HashMap::new();
        self.fonts = HashMap::new();
        self.entries = Vec::new();
    }
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
        let mut inspected = self.inspect(&dir, false)?;
        inspected.release_content();
        self.interpretation(&inspected)?
            .context("document has no structure interpretation")
    }

    /// The element metadata of every page of the adopted document, or of its
    /// proposal, with the fonts it refers to resolved, the roles and headers
    /// of its table cells from the structure interpretation, and the visuals
    /// its references point to: what is read beside the body, as the
    /// `document-elements` contract.
    pub fn elements(&self, id: &str, proposal: bool) -> Result<Value> {
        let dir = self.structure_dir(id, proposal)?;
        let (mut inspected, layout) = self.inspect_layout(&dir, false, &Planned::new(), true)?;
        let mut layout = layout.context("layout kept")?;
        inspected.release_content();
        // The interpretation of the stored extraction, as import classifies
        // the layout from, whether or not the original has changed since.
        let (structure, _) = crate::document_structure::replay_verified_corrections(
            &self.root,
            &inspected.mappings["interpretation"],
            &inspected.extraction,
            string(&inspected.meta["extraction"])?,
            &inspected.axes,
        )?;
        elements::interpret(&mut layout, array(&structure["elements"])?)?;
        let mut pages = serde_json::Map::new();
        for (page_id, sheet) in layout["sheets"]
            .as_object()
            .context("layout sheets missing")?
        {
            let mut metadata = sheet["element_metadata"].clone();
            for element in metadata
                .as_array_mut()
                .context("element metadata missing")?
            {
                if let Some(rows) = element.get_mut("rows") {
                    for row in rows.as_array_mut().context("table rows missing")? {
                        for cell in row["cells"].as_array_mut().context("table cells missing")? {
                            resolve_font(sheet, cell)?;
                        }
                    }
                } else {
                    resolve_font(sheet, element)?;
                }
            }
            pages.insert(
                page_id.clone(),
                json!({"name":sheet["name"],"element_metadata":metadata,"visuals":sheet["visuals"]}),
            );
        }
        let out = json!({"schema_version":"1","document_id":id,"proposal":proposal,"pages":pages});
        validate("document-elements", &out)?;
        Ok(out)
    }

    /// The extraction of the adopted document, or of its proposal, with every
    /// cell's font, style and cached result in place, as the `extraction-view`
    /// contract.
    pub fn extraction_view(&self, id: &str, proposal: bool) -> Result<Value> {
        let dir = self.structure_dir(id, proposal)?;
        let extraction = crate::extraction::view(&self.inspect(&dir, false)?.extraction)?;
        validate("extraction-view", &extraction)?;
        Ok(extraction)
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
            &inspected.axes,
        )?;
        let (reconstructed, report) = crate::document_structure::replay_corrections(
            &self.root,
            &journal,
            &inspected.extraction,
            &inspected.axes,
        )?;
        ensure!(
            reconstructed == *structure,
            "structure cannot be reproduced from corrections"
        );
        let path = dir.join("mappings.yml");
        let mut mappings = read_mappings(&path)?;
        mappings["interpretation"] = journal;
        ensure!(
            self.fingerprint(&dir)?.as_deref() == Some(&inspected.fingerprint),
            "document changed while saving structure"
        );
        let mut planned = Planned::new();
        {
            let mut layout = identity::load(&dir, &inspected.extraction, &Planned::new())?;
            ensure!(
                array(&mappings["operations"])?.is_empty(),
                "apply row/column operations before changing element structure"
            );
            elements::classify(&mut layout, array(&structure["elements"])?)?;
            for name in inspected.page_files.values() {
                let page = self.read_content(&dir, name, &inspected.extraction)?;
                let body = elements::encode(&page, &mut layout)?;
                planned.insert(name.clone(), Some(elements::page_bytes(&body, &layout)?));
            }
            planned.insert(
                "layout.yml".into(),
                Some(identity::stored(&mut layout, &inspected.extraction)?),
            );
        }
        planned.insert(
            "mappings.yml".into(),
            Some(stored_mappings(&path, &mappings)?),
        );
        self.inspect_with(&dir, false, &planned)?;
        self.commit(&dir, &planned, &inspected.fingerprint)?;
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
    /// The bytes of the image assets (`assets/…` paths) operations add.
    fn image_assets(&self, dir: &Path, assets: &[String]) -> Result<BTreeMap<String, Vec<u8>>> {
        let logical = self.logical(dir)?;
        let mut images = BTreeMap::new();
        for asset in assets {
            let path = logical
                .get(asset)
                .with_context(|| format!("image asset missing: {asset}"))?;
            let bytes = fs::read(path)?;
            excel::validate_image_asset(asset, &bytes)?;
            images.insert(asset.clone(), bytes);
        }
        Ok(images)
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

/// Reads and validates the stored `layout.yml` at `path` of a document or
/// proposal, as its documents read it: its rows and columns placed under
/// the pending operations and its element metadata complete, with what the
/// stored form leaves out in place.
pub fn read_layout(path: &Path) -> Result<Value> {
    let dir = path.parent().context("layout has no document directory")?;
    let extraction = crate::extraction::read_record(&dir.join("extraction.json"))?;
    identity::load(dir, &extraction, &Planned::new())
}

/// Writes a layout as [`read_layout`] gives it to `layout.yml` at `path`, in
/// its stored form.
pub fn write_layout(path: &Path, layout: &mut Value) -> Result<()> {
    let dir = path.parent().context("layout has no document directory")?;
    let extraction = crate::extraction::read_record(&dir.join("extraction.json"))?;
    replace(path, &identity::stored(layout, &extraction)?)
}

/// The block, row and column of the grid of a layout `sheet` that the element
/// `id` writes to.
pub fn element_binding(sheet: &Value, id: &str) -> Result<(String, String, String)> {
    elements::Bindings::of(sheet)?.get(id)
}

/// The ID of the element of a layout `sheet` at `row` and `column` of `block`.
pub fn element_id(sheet: &Value, block: &str, row: &str, column: &str) -> Result<String> {
    elements::Bindings::id(sheet, block, row, column)
}

/// Replaces the font index of an element or table cell with the font.
fn resolve_font(sheet: &Value, element: &mut Value) -> Result<()> {
    let font = elements::font(sheet, element)?;
    if !font.is_null() {
        element["font"] = font;
    }
    Ok(())
}

fn fingerprint_of(files: &serde_json::Map<String, Value>) -> String {
    hash(&encoded(&Value::Object(files.clone())))
}

#[cfg(test)]
mod operation_id_tests {
    use super::*;

    #[test]
    fn operation_id_uses_first_gap_and_exact_prefix() {
        let recorded = vec![
            json!({"id":"add-rows-1"}),
            json!({"id":"add-rows-3"}),
            json!({"id":"add-rows-02"}),
            json!({"id":"add-columns-2"}),
        ];
        assert_eq!(next_operation_id(&recorded, "add-rows"), "add-rows-2");
        assert_eq!(next_operation_id(&[], "add-slides"), "add-slides-1");
    }

    #[test]
    #[ignore = "manual operation ID performance measurement"]
    fn measure_operation_ids() {
        for count in [1_000, 4_000] {
            let recorded: Vec<_> = (1..=count)
                .map(|n| json!({"id":format!("add-rows-{n}")}))
                .collect();
            let mut times = vec![];
            for _ in 0..5 {
                let start = std::time::Instant::now();
                let id = next_operation_id(&recorded, "add-rows");
                times.push(start.elapsed().as_secs_f64() * 1000.0);
                assert_eq!(id, format!("add-rows-{}", count + 1));
            }
            times.sort_by(f64::total_cmp);
            eprintln!("operation_ids count={count} median_ms={:.3}", times[2]);
        }
    }
}
