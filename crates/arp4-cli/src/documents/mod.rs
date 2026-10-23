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
mod result_cache;
pub mod search;
mod sheet_edit;
mod status_cache;
pub use sheet_edit::{Axis, EditKind, Position, SheetEdit};
mod elements;
pub(crate) use elements::CONTENT_VERSION;
mod export_confirmation;
mod identity;
mod image_edit;
mod layout;
pub(crate) use layout::{
    Layout, LayoutSheet, Meta, MetaCell, MetaRow, MetaText, metadata_from, metadata_value,
};
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
    extraction::{Cell, CellKind, Extraction, Sheet},
};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet, HashMap},
    fs,
    path::{Path, PathBuf},
    sync::Arc,
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
    pub extraction: Extraction,
    pub mappings: Value,
    /// The complete mapping entries; `mappings` keeps none.
    entries: Vec<Entry>,
    /// Content values and their fonts by page, block and field: one map, as
    /// a large sheet has a value for each cell, and a key for each in two
    /// maps took as much as the values.
    pub content: HashMap<(String, String, String), Field>,
    /// Content file name by validated page ID.
    pub page_files: BTreeMap<String, String>,
    pub fingerprint: String,
    pub reviewed: bool,
    pub source_current: bool,
    pub source_missing: bool,
    /// The identities of the rows and columns of the extraction, which the
    /// correction journal names cells by.
    pub axes: crate::document_structure::Axes,
    /// The digest of the stored rows and columns `axes` is read from.
    axes_digest: String,
    /// The digest of the structure interpretation the layout's element
    /// metadata was classified from (`elements::interpretation_digest`).
    pub structure_sha256: Option<String>,
    /// The mapping entries still to be resolved before export.
    pub pending: usize,
    /// Whether the content differs from the extraction: an operation is
    /// pending or a value written back to a cell changed.
    pub content_edited: bool,
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
        self.content = HashMap::new();
        self.entries = Vec::new();
    }
}

/// A content value and its font, if the field has one (see `crate::fonts`).
#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub value: Value,
    pub font: Option<Arc<Value>>,
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
    pub fn structure(
        &self,
        id: &str,
        proposal: bool,
    ) -> Result<(crate::document_structure::Structure, Value)> {
        let dir = self.structure_dir(id, proposal)?;
        let (inspected, _) = self.inspect_records(&dir, false, false)?;
        self.interpretation(&inspected)?
            .context("document has no structure interpretation")
    }

    /// The element metadata of every page of the adopted document, or of its
    /// proposal, with the fonts it refers to resolved, the roles and headers
    /// of its table cells from the structure interpretation, and the visuals
    /// its references point to: what is read beside the body, as the
    /// `document-elements` contract.
    pub fn elements(&self, id: &str, proposal: bool) -> Result<Value> {
        let (layout, interpretation) = self.interpreted_layout(id, proposal)?;
        let out = elements_view(id, proposal, &layout, &interpretation)?.into_value()?;
        validate("document-elements", &out)?;
        Ok(out)
    }

    /// Writes [`Store::elements`] to `out`, made as it is written and checked
    /// a part at a time; returns the number of its pages.
    pub fn write_elements(&self, id: &str, proposal: bool, out: &Path) -> Result<usize> {
        let (layout, interpretation) = self.interpreted_layout(id, proposal)?;
        check_elements(id, proposal, &layout, &interpretation, |part| {
            validate("document-elements", part)
        })?;
        write_lazy(out, elements_view(id, proposal, &layout, &interpretation)?)?;
        Ok(layout.sheets.len())
    }

    /// The layout of the adopted document, or of its proposal, with the roles
    /// and headers the structure interpretation gives its table cells. The
    /// extraction and the interpretation are let go once they are in it.
    fn interpreted_layout(
        &self,
        id: &str,
        proposal: bool,
    ) -> Result<(Layout, elements::Interpretation)> {
        let dir = self.structure_dir(id, proposal)?;
        let (inspected, layout) = self.inspect_records(&dir, false, true)?;
        let layout = layout.context("layout kept")?;
        // The interpretation of the stored extraction, as import classifies
        // the layout from, whether or not the original has changed since.
        let (structure, _) = crate::document_structure::replay_verified_corrections(
            &self.root,
            &inspected.mappings["interpretation"],
            &inspected.extraction,
            string(&inspected.meta["extraction"])?,
            &inspected.axes,
        )?;
        let interpretation = elements::interpret(&layout, &structure.elements)?;
        Ok((layout, interpretation))
    }

    /// Writes the extraction of the adopted document, or of its proposal, to
    /// `out` with every cell's font, style and cached result in place, as the
    /// `extraction-view` contract: made as it is written and checked a part
    /// at a time.
    pub fn write_extraction_view(&self, id: &str, proposal: bool, out: &Path) -> Result<()> {
        let dir = self.structure_dir(id, proposal)?;
        let extraction = self.inspect_records(&dir, false, false)?.0.extraction;
        crate::extraction::check_view(&extraction, |part| validate("extraction-view", part))?;
        write_lazy(out, crate::extraction::view_lazy(&extraction))
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
            reconstructed.to_value() == *structure,
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
            let mut classified =
                elements::classify(&mut layout, &inspected.extraction, &reconstructed.elements)?;
            for name in inspected.page_files.values() {
                let page = self.read_content(&dir, name, &inspected.extraction)?;
                let groups = classified.remove(string(&page["page_id"])?);
                let body = elements::encode(page, &mut layout, groups)?;
                planned.insert(name.clone(), Some(elements::content_bytes(&body, &layout)?));
            }
            planned.insert(
                "layout.yml".into(),
                Some(identity::stored(layout, &inspected.extraction)?),
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
    Ok(identity::load(dir, &extraction, &Planned::new())?.to_value())
}

/// Writes a layout as [`read_layout`] gives it to `layout.yml` at `path`, in
/// its stored form.
pub fn write_layout(path: &Path, layout: Value) -> Result<()> {
    let dir = path.parent().context("layout has no document directory")?;
    let extraction = crate::extraction::read_record(&dir.join("extraction.json"))?;
    replace(
        path,
        &identity::stored(Layout::from_value(layout)?, &extraction)?,
    )
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

/// The `document-elements` read of `layout`, the interpreted layout of the
/// document `id` or of its proposal, made as it is written: the fonts of
/// every table cell resolved held several times the memory of the layout.
fn elements_view<'a>(
    id: &'a str,
    proposal: bool,
    layout: &'a Layout,
    interpretation: &'a elements::Interpretation,
) -> Result<Lazy<'a>> {
    let mut pages = vec![];
    for (page_id, sheet) in &layout.sheets {
        let found = &interpretation[page_id];
        let metadata =
            Lazy::Items(Box::new(sheet.metadata.iter().map(move |element| {
                resolved_element(&sheet.fields, element, found)
            })));
        pages.push((
            page_id.as_str(),
            Lazy::Object(vec![
                ("name", Lazy::Borrowed(&sheet["name"])),
                ("visuals", Lazy::Borrowed(&sheet["visuals"])),
                ("element_metadata", metadata),
            ]),
        ));
    }
    Ok(Lazy::Object(vec![
        ("schema_version", Lazy::Value(json!("1"))),
        ("document_id", Lazy::Value(json!(id))),
        ("proposal", Lazy::Value(json!(proposal))),
        ("pages", Lazy::Object(pages)),
    ]))
}

/// The element metadata `element` of the layout `sheet` with its fonts
/// resolved: those of the cells of each row of a table, made row by row.
fn resolved_element<'a>(
    sheet: &'a Value,
    element: &'a Meta,
    found: &'a HashMap<String, (Value, Vec<String>)>,
) -> Result<Lazy<'a>> {
    let Some(rows) = element.records() else {
        let mut element = element.to_value();
        resolve_font(sheet, &mut element)?;
        return Ok(Lazy::Value(element));
    };
    // The cells of the table, which a cell's headers are among.
    let present: std::collections::HashSet<&str> = rows
        .iter()
        .flat_map(|row| &row.cells)
        .map(|cell| string(&cell["id"]))
        .collect::<Result<_>>()?;
    let mut members = Lazy::members(
        element.fields().context("table metadata missing")?,
        &["rows"],
    );
    members.push((
        "rows",
        Lazy::Items(Box::new(rows.iter().map(move |row| {
            let mut row = row.to_value();
            for cell in row["cells"].as_array_mut().context("table cells missing")? {
                elements::interpret_cell(cell, found, &present)?;
                resolve_font(sheet, cell)?;
            }
            Ok(Lazy::Value(row))
        }))),
    ));
    Ok(Lazy::Object(members))
}

/// The elements or table rows [`check_elements`] checks together.
const ELEMENTS_PART: usize = 2048;

/// Checks [`elements_view`] of `layout` with `check` a part at a time: each
/// page with its visuals and no elements, then with at most
/// [`ELEMENTS_PART`] of its elements, and each table with at most as many of
/// its rows, as no part of the contract relates the items of a list.
fn check_elements(
    id: &str,
    proposal: bool,
    layout: &Layout,
    interpretation: &elements::Interpretation,
    check: impl Fn(&Value) -> Result<()>,
) -> Result<()> {
    let read = |pages: serde_json::Map<String, Value>| {
        let mut out = json!({"schema_version":"1","document_id":id,"proposal":proposal});
        out["pages"] = Value::Object(pages);
        out
    };
    check(&read(serde_json::Map::new()))?;
    for (page_id, typed) in &layout.sheets {
        let sheet = &typed.fields;
        let found = &interpretation[page_id];
        let page = |metadata: Vec<Value>, visuals: Value| {
            let mut page = json!({"name":sheet["name"],"visuals":visuals});
            page["element_metadata"] = Value::Array(metadata);
            read([(page_id.clone(), page)].into_iter().collect())
        };
        check(&page(vec![], sheet["visuals"].clone()))?;
        let mut batch = vec![];
        for element in &typed.metadata {
            let members = match resolved_element(sheet, element, found)? {
                Lazy::Object(members) => members,
                other => {
                    batch.push(other.into_value()?);
                    if batch.len() == ELEMENTS_PART {
                        check(&page(std::mem::take(&mut batch), json!({})))?;
                    }
                    continue;
                }
            };
            let mut head = serde_json::Map::new();
            let mut rows = None;
            for (key, member) in members {
                match member {
                    Lazy::Items(items) if key == "rows" => rows = Some(items),
                    member => {
                        head.insert(key.to_owned(), member.into_value()?);
                    }
                }
            }
            let mut rows = rows.context("table rows missing")?.peekable();
            let mut first = true;
            while first || rows.peek().is_some() {
                first = false;
                let chunk = rows
                    .by_ref()
                    .take(ELEMENTS_PART)
                    .map(|row| row?.into_value())
                    .collect::<Result<Vec<_>>>()?;
                let mut table = Value::Object(head.clone());
                table["rows"] = Value::Array(chunk);
                check(&page(vec![table], json!({})))?;
            }
        }
        if !batch.is_empty() {
            check(&page(batch, json!({})))?;
        }
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
