//! Read-only interpretations of Office/PDF text and visual objects. Never used by writeback.
use crate::data::{array, encoded, hash, immutable, read, string, under, write};
use crate::extraction::{Cell, CellKind, Extraction, Sheet};
use anyhow::{Context, Result, bail, ensure};
use clap::{Subcommand, ValueEnum};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};
mod align;
mod carry;
mod context;
mod corrections;
mod edit;
mod grid;
mod inference;
mod model;
mod policy;
mod rebase;
mod relations;
pub use grid::Axes;
pub use model::{Element, ElementCell, Role, Structure, TextState};
pub use policy::{policy as requirement_policy, requirements};

pub fn schema() -> Value {
    serde_json::from_str(include_str!(
        "../../../contracts/document-structure-schema.json"
    ))
    .unwrap()
}

pub fn empty_corrections(extraction: &Extraction, extraction_hash: &str) -> Value {
    corrections::empty(extraction, extraction_hash)
}

pub fn validate_corrections(value: &Value) -> Result<()> {
    corrections::validate_journal(value)
}

/// Replays the correction `journal` on `extraction`, whose cells it names by
/// the identities of their rows and columns in `axes`.
pub fn replay_corrections(
    root: &Path,
    journal: &Value,
    extraction: &Extraction,
    axes: &Axes,
) -> Result<(Structure, Value)> {
    rebase::rebase(
        root,
        journal,
        extraction,
        &crate::extraction::hash(extraction)?,
        true,
        axes,
    )
}

/// [`replay_corrections`] for an extraction whose canonical hash the caller has
/// verified, such as the key of an inspected document.
pub fn replay_verified_corrections(
    root: &Path,
    journal: &Value,
    extraction: &Extraction,
    extraction_hash: &str,
    axes: &Axes,
) -> Result<(Structure, Value)> {
    rebase::rebase(root, journal, extraction, extraction_hash, true, axes)
}

/// [`replay_verified_corrections`] on the extraction of an earlier version of
/// the original, which has changed since: the interpretation the new version's
/// import carries from.
pub fn replay_earlier_corrections(
    root: &Path,
    journal: &Value,
    extraction: &Extraction,
    extraction_hash: &str,
    axes: &Axes,
) -> Result<(Structure, Value)> {
    rebase::rebase(root, journal, extraction, extraction_hash, false, axes)
}

/// How the row and column operations between two versions of an original
/// are known when an interpretation is carried from one to the other.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Carrier {
    /// `documents apply` wrote them, so their positions and counts are exact.
    Apply,
    /// The original was edited outside ARP; they were found by aligning the
    /// rows and columns of the two extractions by their values.
    Alignment,
}

impl Carrier {
    pub fn name(self) -> &'static str {
        match self {
            Self::Apply => "apply",
            Self::Alignment => "alignment",
        }
    }
}

/// The row and column operations that turn the sheets of the extraction
/// `before` into those of `after`, found by aligning their values, for an
/// original edited outside ARP.
pub fn align_operations(
    before: &Extraction,
    after: &Extraction,
) -> Result<Vec<crate::excel::StructuralOperation>> {
    align::operations(before, after)
}

/// Carries `structure`, the interpretation of the extraction `before`, onto
/// `after`, the extraction of the original changed by `operations`. Returns
/// the correction journal for `after`, naming its cells by `axes`, and the
/// record of what moved, was added, removed or changed, and whether the review
/// was kept.
pub fn carry_corrections(
    root: &Path,
    structure: &Structure,
    before: &Extraction,
    after: &Extraction,
    operations: &[crate::excel::StructuralOperation],
    carrier: Carrier,
    axes: &Axes,
) -> Result<(Value, Value)> {
    carry::carry(root, structure, before, after, operations, carrier, axes)
}

/// The correction journal of `structure` on `extraction`, which names cells by
/// the identities of their rows and columns in `axes`.
pub fn record_corrections(
    root: &Path,
    extraction: &Extraction,
    structure: &Value,
    axes: &Axes,
) -> Result<Value> {
    corrections::record(root, extraction, structure, axes)
}

#[derive(Clone, ValueEnum)]
pub enum Decision {
    Accepted,
    Rejected,
}

#[derive(Subcommand)]
pub enum StructureCommand {
    /// Render an element or visual's Excel range to PNG and register it as evidence.
    /// Requires Windows and desktop Excel; does not save changes to the workbook.
    Render {
        #[arg(long)]
        extraction: PathBuf,
        #[arg(long)]
        structure: PathBuf,
        #[arg(long)]
        element: String,
        #[arg(long)]
        id: String,
        /// New repository-relative PNG path outside disposable cache/work.
        #[arg(long)]
        image: String,
        /// A1 range; omitted uses the visual's cell anchors, or the element sheet's UsedRange.
        #[arg(long)]
        range: Option<String>,
        #[arg(long, default_value_t = 120, value_parser = clap::value_parser!(u64).range(1..=600))]
        timeout: u64,
    },
    /// Create a read-only interpretation YAML from Excel, DOCX, PPTX or PDF extraction.
    Init {
        #[arg(long)]
        extraction: PathBuf,
        #[arg(long)]
        out: PathBuf,
    },
    /// Validate source/image versions, cell references and review freshness.
    Check {
        #[arg(long)]
        extraction: PathBuf,
        #[arg(long)]
        structure: PathBuf,
    },
    /// Read the interpretation, original cells, compact drawings and verified image paths.
    Read {
        #[arg(long)]
        extraction: PathBuf,
        #[arg(long)]
        structure: PathBuf,
    },
    /// Batch upsert/remove elements and visuals against the revision returned by read.
    /// Validate the whole interpretation and reset review only when content changes.
    Edit {
        #[arg(long)]
        extraction: PathBuf,
        #[arg(long)]
        structure: PathBuf,
        /// JSON or YAML request matching structure schema --edit.
        #[arg(long)]
        input: PathBuf,
        /// Return validated before/after differences without writing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Register an existing PNG region on an element or visual; does not render Excel.
    Region {
        #[arg(long)]
        extraction: PathBuf,
        #[arg(long)]
        structure: PathBuf,
        #[arg(long)]
        element: String,
        #[arg(long)]
        id: String,
        /// Repository-relative PNG path; keep evidence outside disposable work/cache directories.
        #[arg(long)]
        image: String,
        /// Cell range the image shows. Required for an element or a visual on a
        /// sheet; omitted for a visual not on a sheet, such as a Word picture.
        #[arg(long)]
        range: Option<String>,
        /// Normalized x,y,width,height in the supplied full image.
        #[arg(long, value_delimiter = ',', num_args = 4, default_value = "0,0,1,1")]
        bbox: Vec<f64>,
    },
    /// Record an actual review of this exact interpretation, including image hashes.
    Review {
        #[arg(long)]
        extraction: PathBuf,
        #[arg(long)]
        structure: PathBuf,
        #[arg(long, value_enum)]
        decision: Decision,
        #[arg(long)]
        actor: String,
        #[arg(long)]
        reason: String,
    },
    /// Save the authoritative interpretation schema.
    Schema {
        #[arg(long)]
        out: PathBuf,
        /// Save the batch editing request schema instead of the interpretation schema.
        #[arg(long)]
        edit: bool,
    },
}

/// The hash a review binds: of the structure without its review, as
/// encoded, a cell at a time.
pub fn content_hash(structure: &Structure) -> String {
    crate::data::hash_encoded_lazy(structure.lazy(Some("review")))
        .expect("typed cells always encode")
}

/// [`content_hash`] of a structure given as JSON.
pub fn json_content_hash(value: &Value) -> String {
    // Hashed as encoded, without copying the structure to leave the review out.
    crate::data::hash_encoded_without(value, "review")
}

fn validate_schema(value: &Value) -> Result<()> {
    // Compiled once: every replay validates the interpretation.
    static VALIDATOR: std::sync::LazyLock<jsonschema::Validator> = std::sync::LazyLock::new(|| {
        jsonschema::validator_for(&schema()).expect("embedded structure schema compiles")
    });
    let errors: Vec<_> = VALIDATOR
        .iter_errors(value)
        .map(|e| e.to_string())
        .collect();
    ensure!(
        errors.is_empty(),
        "invalid document structure: {}",
        errors.join("; ")
    );
    Ok(())
}

fn range(value: &str) -> Result<((u32, u32), (u32, u32))> {
    let (a, b) = value.split_once(':').unwrap_or((value, value));
    let a = crate::excel::coordinate(a)?;
    let b = crate::excel::coordinate(b)?;
    ensure!(a.0 <= b.0 && a.1 <= b.1, "reversed cell range");
    Ok((a, b))
}

fn contains(range: ((u32, u32), (u32, u32)), point: (u32, u32)) -> bool {
    (range.0.0..=range.1.0).contains(&point.0) && (range.0.1..=range.1.1).contains(&point.1)
}

fn image_path(root: &Path, image: &str) -> Result<PathBuf> {
    let path = under(root, image)?;
    let relative = path
        .strip_prefix(root)?
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase();
    ensure!(
        !relative.starts_with(".arp/cache/") && !relative.starts_with(".arp/work/"),
        "visual evidence must not be stored in disposable cache/work directories"
    );
    Ok(path)
}

fn png(root: &Path, image: &str) -> Result<Vec<u8>> {
    let path = image_path(root, image)?;
    ensure!(
        image.to_ascii_lowercase().ends_with(".png"),
        "visual evidence requires a PNG path"
    );
    ensure!(
        fs::metadata(&path)?.len() <= 64 * 1024 * 1024,
        "PNG exceeds size budget"
    );
    let bytes = fs::read(path)?;
    crate::excel::validate_image_asset("assets/evidence.png", &bytes)?;
    ensure!(
        bytes.len() >= 33
            && &bytes[12..16] == b"IHDR"
            && u32::from_be_bytes(bytes[16..20].try_into()?) > 0
            && u32::from_be_bytes(bytes[20..24].try_into()?) > 0,
        "PNG dimensions missing"
    );
    Ok(bytes)
}

fn extraction(path: &Path) -> Result<Extraction> {
    let value = crate::extraction::read(path)?;
    let extension = Path::new(string(&value["source"]["path"])?)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    ensure!(
        crate::document_source::structure_formats().contains(&extension.as_str()),
        "document structure requires Excel, Word, PPTX or PDF extraction"
    );
    context::assets(path, &value)?;
    Ok(value)
}

pub fn initialize(root: &Path, extraction: &Extraction) -> Result<Structure> {
    let extraction_hash = crate::extraction::hash(extraction)?;
    let value = inferred(root, extraction, &extraction_hash)?;
    ready_hashed(root, extraction, &extraction_hash, &value, true)?;
    Ok(value)
}

/// The unreviewed interpretation inferred from `extraction`, whose hash the caller supplies.
/// [`infer`] saved in the project's cache: every status, check, edit and
/// search refresh replays the corrections on the inferred interpretation, and
/// inferring the tables of a large sheet takes seconds. It depends on nothing
/// but the extraction, which `extraction_hash` names with the document and
/// original it records apart, and the program.
fn inferred(root: &Path, extraction: &Extraction, extraction_hash: &str) -> Result<Structure> {
    let arp = root.join(".arp");
    if !arp.is_dir() {
        return infer(extraction, extraction_hash);
    }
    let directory = crate::data::under(&arp, "cache/structure")?;
    let key = crate::data::hash(&crate::data::encoded(&json!({
        "program":crate::data::program()?,"extraction":extraction_hash,
        "document":extraction["document_id"],"source":extraction["source"]})));
    let path = directory.join(format!("{key}.json"));
    if let Some(saved) = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
    {
        return Ok(saved);
    }
    let value = infer(extraction, extraction_hash)?;
    // An interpretation that cannot be saved is inferred again next time.
    // Written as it is encoded: a large sheet's took as much as the
    // interpretation itself.
    if crate::data::replace_cache_streamed(&path, |out| Ok(serde_json::to_writer(out, &value)?))
        .is_ok()
    {
        crate::data::keep_newest(&directory, 16);
    }
    Ok(value)
}

fn infer(extraction: &Extraction, extraction_hash: &str) -> Result<Structure> {
    let fields = json!({"schema_version":1,"document":extraction["document_id"],"source":{"path":extraction["source"]["path"],"sha256":extraction["source"]["sha256"]},
        "extraction_hash":extraction_hash,"regions":[],"visuals":policy::visuals(extraction)?,"review":{"status":"pending"}});
    Ok(Structure::new(fields, inference::elements(extraction)?))
}

/// Validity is distinct from readiness: pending/unreadable cells remain inspectable.
pub fn validate(root: &Path, extraction: &Extraction, value: &Value) -> Result<Value> {
    validate_schema(value)?;
    validate_structure(root, extraction, &Structure::from_value(value.clone())?)
}

/// [`validate`] of a structure held typed.
pub fn validate_structure(
    root: &Path,
    extraction: &Extraction,
    structure: &Structure,
) -> Result<Value> {
    validate_hashed(
        root,
        extraction,
        &crate::extraction::hash(extraction)?,
        structure,
        true,
    )
}

/// [`validate`] against the already computed hash of `extraction`. `current`
/// requires the original to be still the one extracted; an earlier version's
/// interpretation, read to carry it onto the new one, is validated without.
fn validate_hashed(
    root: &Path,
    extraction: &Extraction,
    extraction_hash: &str,
    value: &Structure,
    current: bool,
) -> Result<Value> {
    let mut unresolved = Unresolved {
        listed: true,
        ..Unresolved::default()
    };
    let (reviewed, fingerprint) = verify(
        root,
        extraction,
        extraction_hash,
        value,
        current,
        &mut unresolved,
    )?;
    Ok(
        json!({"valid":true,"ready":reviewed && unresolved.count == 0,"review_current":value["review"]["content_hash"] == fingerprint,
        "content_hash":fingerprint,"unresolved":unresolved.entries}),
    )
}

/// Whether `value` is ready, validated as [`validate_hashed`] validates it
/// but without listing what is unresolved: replay reads only this, and a new
/// interpretation of a large document lists each of its cells.
fn ready_hashed(
    root: &Path,
    extraction: &Extraction,
    extraction_hash: &str,
    value: &Structure,
    current: bool,
) -> Result<bool> {
    let mut unresolved = Unresolved::default();
    let (reviewed, _) = verify(
        root,
        extraction,
        extraction_hash,
        value,
        current,
        &mut unresolved,
    )?;
    Ok(reviewed && unresolved.count == 0)
}

/// What validation finds unresolved: counted, and listed when `listed`.
#[derive(Default)]
struct Unresolved {
    listed: bool,
    count: usize,
    entries: Vec<Value>,
}

impl Unresolved {
    fn push(&mut self, entry: impl FnOnce() -> Value) {
        self.count += 1;
        if self.listed {
            self.entries.push(entry());
        }
    }
}

/// The checks of [`validate_hashed`], noting in `unresolved` what is left to
/// resolve. Returns whether the review is accepted for the content, and the
/// content's hash.
fn verify(
    root: &Path,
    extraction: &Extraction,
    extraction_hash: &str,
    value: &Structure,
    current: bool,
    unresolved: &mut Unresolved,
) -> Result<(bool, String)> {
    value.check(validate_schema)?;
    relations::validate_elements(&value.elements, &value.fields)?;
    ensure!(
        value["document"] == extraction["document_id"]
            && value["source"]["path"] == extraction["source"]["path"]
            && value["source"]["sha256"] == extraction["source"]["sha256"]
            && value["extraction_hash"] == extraction_hash,
        "structure/extraction version mismatch"
    );
    if current {
        let original = under(root, string(&value["source"]["path"])?)?;
        ensure!(
            hash(&fs::read(original)?) == value["source"]["sha256"],
            "original changed; re-import and reinterpret"
        );
    }
    let sheets: BTreeMap<_, _> = extraction
        .sheets
        .iter()
        .map(|s| (s["name"].as_str().unwrap(), s))
        .collect();
    let mut regions = BTreeMap::new();
    for r in array(&value["regions"])? {
        let id = string(&r["id"])?;
        ensure!(regions.insert(id, r).is_none(), "duplicate region ID");
        // A region without a sheet is an image of a visual not on a sheet; the
        // visual checks bind it, and no element can use it.
        if let Some(sheet) = r.get("sheet") {
            ensure!(sheets.contains_key(string(sheet)?), "unknown region sheet");
            range(string(&r["range"])?)?;
        }
        ensure!(
            r["source_sha256"] == value["source"]["sha256"],
            "image region belongs to another original version"
        );
        ensure!(
            hash(&png(root, string(&r["image"])?)?) == r["image_sha256"],
            "image hash mismatch"
        );
        let b: Vec<_> = array(&r["bbox"])?
            .iter()
            .map(|n| n.as_f64().unwrap())
            .collect();
        ensure!(
            b[2] > 0.0 && b[3] > 0.0 && b[0] + b[2] <= 1.0 && b[1] + b[3] <= 1.0,
            "image bbox outside image"
        );
    }
    // Every ID is looked up in this once; hashed, as its order is never read,
    // and made as large as it becomes, it takes a fraction of the time an
    // ordered one took.
    let elements = &value.elements;
    let cell_count: usize = elements.iter().map(|e| e.cells.len()).sum();
    let mut ids = HashSet::with_capacity(elements.len() + cell_count);
    for entry in policy::validate_visuals(extraction, &value.fields)? {
        unresolved.push(|| entry);
    }
    // Built once per sheet: a sheet holds many elements, each looking up its cells.
    let mut sheet_cells: HashMap<&str, SheetCells<'_>> = HashMap::new();
    for element in elements {
        ensure!(
            ids.insert(string(&element["id"])?),
            "duplicate element/cell ID"
        );
        let sheet_name = string(&element["sheet"])?;
        let sheet = sheets.get(sheet_name).context("unknown element sheet")?;
        let original_cells = sheet_cells
            .entry(sheet_name)
            .or_insert_with(|| SheetCells::new(sheet));
        let cells = &element.cells;
        // The cells by ID, the last of an ID: their places in the order of
        // their IDs, as an element of a large table has a cell for each of its
        // values.
        let mut by_id: Vec<u32> = (0..cells.len() as u32).collect();
        by_id.sort_unstable_by(|&a, &b| {
            cells[a as usize]
                .id
                .cmp(&cells[b as usize].id)
                .then(b.cmp(&a))
        });
        by_id.dedup_by(|a, b| cells[*a as usize].id == cells[*b as usize].id);
        let local = |id: &str| {
            by_id
                .binary_search_by(|&at| (*cells[at as usize].id).cmp(id))
                .ok()
                .map(|at| by_id[at] as usize)
        };
        let evidence: Vec<_> = array(&element["evidence"])?
            .iter()
            .map(|id| {
                let r = *regions.get(string(id)?).context("unknown image region")?;
                ensure!(r["sheet"] == sheet_name, "image region on different sheet");
                range(string(&r["range"])?)
            })
            .collect::<Result<_>>()?;
        let vision = element["reading"]["method"] == "vision";
        if vision {
            ensure!(
                !evidence.is_empty(),
                "vision reading requires image evidence"
            );
        }
        for cell in cells {
            let id = &*cell.id;
            ensure!(ids.insert(id), "duplicate element/cell ID");
            let address = &*cell.address;
            let point = crate::excel::coordinate(address)?;
            let found = original_cells.find(address, point);
            ensure!(
                original_cells.own(address, found),
                "cell belongs to multiple elements"
            );
            if vision {
                ensure!(
                    evidence.iter().any(|r| contains(*r, point)),
                    "vision cell outside supplied image regions"
                );
            }
            let original = found.map(|at| original_cells.cell(at));
            let text = original.map(|c| c.value());
            match cell.text_state {
                TextState::Read => ensure!(
                    text.is_some_and(|t| !t.is_null() && t.as_str().is_none_or(|s| !s.is_empty()))
                        || original.is_some_and(|c| c.formula().is_some()),
                    "read cell has no captured text; OCR cannot become an exact source quote"
                ),
                TextState::Empty => ensure!(
                    text.is_none_or(|t| t.is_null() || t == "")
                        && original.is_none_or(|c| c.formula().is_none()),
                    "empty cell contains original content"
                ),
                _ => unresolved.push(
                    || json!({"element":element["id"],"cell":id,"state":cell.text_state.as_str()}),
                ),
            }
            if cell.role == Role::Unassigned {
                unresolved.push(|| json!({"cell":id,"state":"unassigned_role"}));
            }
            for header in &cell.headers {
                let h =
                    local(header).context("header must reference a cell in the same element")?;
                ensure!(cells[h].role.heads(), "header target is not a header");
            }
        }
        // Iterative traversal avoids recursive overflow for deeply nested
        // headers. A cell is finished once all its headers are, and is not
        // followed again, so a header repeated down a long table is read once
        // rather than from every record; a header still on the path is a cycle.
        // Whether there is a cycle does not depend on the order cells are visited.
        // Cells are visited in their order, which reads them where they lie in
        // memory, and the path keeps the headers of each cell on it.
        // Each cell by its place: not yet visited, on the path, or finished.
        let (on_path, finished) = (1u8, 2u8);
        let mut visited = vec![0u8; cells.len()];
        let mut path: Vec<(usize, usize)> = vec![];
        for start in 0..cells.len() {
            if visited[start] != 0 {
                continue;
            }
            visited[start] = on_path;
            path.push((start, 0));
            while let Some(top) = path.last_mut() {
                let (current, next) = *top;
                let Some(h) = cells[current].headers.get(next) else {
                    visited[current] = finished;
                    path.pop();
                    continue;
                };
                top.1 += 1;
                let h = local(h).expect("the headers of the cells are checked above");
                ensure!(visited[h] != on_path, "cyclic header relationship");
                if visited[h] == 0 {
                    visited[h] = on_path;
                    path.push((h, 0));
                }
            }
        }
    }
    // A structure edit cannot silently drop captured cells, including formulas/notes.
    for (name, sheet) in &sheets {
        for (at, cell) in sheet.cells.iter().enumerate() {
            ensure!(
                sheet_cells
                    .get(name)
                    .is_some_and(|cells| cells.owns(at as u32)),
                "structure omits original cell {}!{}",
                name,
                json!(cell.address())
            );
        }
    }
    let fingerprint = content_hash(value);
    let reviewed =
        value["review"]["status"] == "accepted" && value["review"]["content_hash"] == fingerprint;
    Ok((reviewed, fingerprint))
}

/// The cells of a sheet by address, the last of an address, with whether an
/// element owns each: found by halves among them in order, as a large sheet
/// has a cell for each of its values, and a map of them took several times
/// the room.
struct SheetCells<'a> {
    sheet: &'a Sheet,
    /// The cells by the row and column their addresses name, when each names
    /// one no other cell's does: compared as numbers rather than as text.
    places: Option<Vec<(u64, u32)>>,
    /// The cells in the order of their addresses otherwise.
    addresses: Vec<u32>,
    /// Whether an element owns each cell, by its number.
    owned: Vec<bool>,
    /// The addresses owned that no cell of the sheet has.
    others: HashSet<&'a str>,
}

/// The row and column of a cell, as one number in their order.
fn place_key((column, row): (u32, u32)) -> u64 {
    (u64::from(row) << 32) | u64::from(column)
}

impl<'a> SheetCells<'a> {
    fn new(sheet: &'a Sheet) -> Self {
        let cells = &sheet.cells;
        let places = cells
            .iter()
            .enumerate()
            .map(|(at, cell)| {
                Some((
                    place_key(crate::excel::coordinate(cell.address()).ok()?),
                    at as u32,
                ))
            })
            .collect::<Option<Vec<_>>>()
            .map(|mut places| {
                if !places.is_sorted() {
                    places.sort_unstable();
                }
                places
            })
            .filter(|places| places.windows(2).all(|pair| pair[0].0 != pair[1].0));
        let addresses = if places.is_some() {
            vec![]
        } else {
            let address = |at: u32| cells.get(at as usize).expect("a cell").address();
            let mut order: Vec<u32> = (0..cells.len() as u32).collect();
            order.sort_unstable_by(|&a, &b| address(a).cmp(address(b)).then(b.cmp(&a)));
            order.dedup_by(|a, b| address(*a) == address(*b));
            order
        };
        Self {
            sheet,
            places,
            addresses,
            owned: vec![false; cells.len()],
            others: HashSet::new(),
        }
    }

    fn cell(&self, at: u32) -> Cell<'a> {
        self.sheet
            .cells
            .get(at as usize)
            .expect("a cell of the sheet")
    }

    /// The number of the cell at `address`, the row and column `point`.
    fn find(&self, address: &str, point: (u32, u32)) -> Option<u32> {
        match &self.places {
            Some(places) => {
                let at = places
                    .binary_search_by_key(&place_key(point), |&(key, _)| key)
                    .ok()?;
                let at = places[at].1;
                (self.cell(at).address() == address).then_some(at)
            }
            None => self.find_address(address),
        }
    }

    fn find_address(&self, address: &str) -> Option<u32> {
        let at = self
            .addresses
            .binary_search_by(|&at| self.cell(at).address().cmp(address))
            .ok()?;
        Some(self.addresses[at])
    }

    /// Notes that an element owns `address`, the cell `found` if any: whether
    /// none did.
    fn own(&mut self, address: &'a str, found: Option<u32>) -> bool {
        match found {
            Some(at) => !std::mem::replace(&mut self.owned[at as usize], true),
            None => self.others.insert(address),
        }
    }

    /// Whether an element owns the address of the cell numbered `at`.
    fn owns(&self, at: u32) -> bool {
        let found = match self.places {
            Some(_) => Some(at),
            None => self.find_address(self.cell(at).address()),
        };
        found.is_some_and(|at| self.owned[at as usize])
    }
}

pub fn execute(root: &Path, command: StructureCommand) -> Result<Value> {
    let include_context = matches!(&command, StructureCommand::Read { .. });
    match command {
        StructureCommand::Edit {
            extraction,
            structure,
            input,
            dry_run,
        } => edit::execute(root, &extraction, &structure, &input, dry_run),
        StructureCommand::Render {
            extraction: path,
            structure,
            element,
            id,
            image,
            range: cell_range,
            timeout,
        } => {
            ensure!(
                !id.trim().is_empty() && id == id.trim(),
                "nonempty, trimmed region ID required"
            );
            let ext = extraction(&path)?;
            ensure!(
                crate::document_source::is_excel(&policy::extension(&ext)),
                "automatic rendering requires Excel; register externally rendered PNGs with structure region"
            );
            let before = read(&structure, None)?;
            validate(root, &ext, &before)?;
            ensure!(
                !array(&before["regions"])?.iter().any(|r| r["id"] == id),
                "region ID already exists"
            );
            let collection = target_collection(&before, &element)?;
            let e = array(&before[collection])?
                .iter()
                .find(|e| e["id"] == element)
                .context("unknown element")?;
            let cell_range = if cell_range.is_none() && collection == "visuals" {
                Some(context::visual_range(&ext, e)?)
            } else {
                cell_range
            };
            if let Some(r) = &cell_range {
                range(r)?;
            }
            let image_file = image_path(root, &image)?;
            ensure!(
                image.to_ascii_lowercase().ends_with(".png") && !image_file.exists(),
                "render requires a new PNG path"
            );
            let original = under(root, string(&before["source"]["path"])?)?;
            let (bytes, rendering) = crate::excel::render(
                &original,
                string(&e["sheet"])?,
                cell_range.as_deref(),
                timeout,
                false,
            )?;
            let actual_range = string(&rendering["range"])?;
            range(actual_range)?;
            ensure!(
                read(&structure, None)? == before,
                "structure changed during rendering; output discarded"
            );
            validate(root, &ext, &before)?;
            immutable(&image_file, &bytes)?;
            // Bind to the sheet actually rendered, not to an element reloaded
            // after another editor might have moved it to a different sheet.
            let mut adopted = before.clone();
            adopted[collection]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|e| e["id"] == element)
                .unwrap()["evidence"]
                .as_array_mut()
                .unwrap()
                .push(json!(id));
            adopted["regions"].as_array_mut().unwrap().push(json!({
                "id":id,"sheet":rendering["sheet"],"range":actual_range,"image":image,
                "image_sha256":hash(&bytes),"source_sha256":before["source"]["sha256"],"bbox":[0.0,0.0,1.0,1.0]
            }));
            adopted["review"] = json!({"status":"pending"});
            let mut result = validate(root, &ext, &adopted)?;
            ensure!(
                read(&structure, None)? == before,
                "structure changed before image registration; PNG was saved but not registered"
            );
            write(&structure, &adopted)?;
            result["rendering"] = rendering;
            result["image_path"] = json!(image_file);
            result["image_sha256"] = json!(hash(&bytes));
            Ok(result)
        }
        StructureCommand::Schema { out, edit } => {
            let schema = if edit { edit::schema() } else { schema() };
            immutable(&out, &encoded(&schema))?;
            Ok(json!({"schema":out}))
        }
        StructureCommand::Init {
            extraction: path,
            out,
        } => {
            let ext = extraction(&path)?;
            ensure!(!out.exists(), "structure output already exists");
            let structure = initialize(root, &ext)?;
            crate::data::write_lazy(&out, structure.lazy(None))?;
            Ok(json!({"structure":out,"state":"needs_review"}))
        }
        StructureCommand::Check {
            extraction: path,
            structure,
        }
        | StructureCommand::Read {
            extraction: path,
            structure,
        } => {
            let ext = extraction(&path)?;
            let value = read(&structure, None)?;
            let mut report = validate(root, &ext, &value)?;
            report["revision"] = json!(hash(&encoded(&value)));
            if !include_context {
                return Ok(report);
            }
            report["structure"] = value;
            context::read(root, &path, &ext, &mut report)?;
            Ok(report)
        }
        StructureCommand::Region {
            extraction: path,
            structure,
            element,
            id,
            image,
            range: cell_range,
            bbox,
        } => {
            let ext = extraction(&path)?;
            let mut value = read(&structure, None)?;
            validate(root, &ext, &value)?;
            ensure!(
                !array(&value["regions"])?.iter().any(|r| r["id"] == id),
                "region ID already exists"
            );
            let image_hash = hash(&png(root, &image)?);
            let collection = target_collection(&value, &element)?;
            let e = value[collection]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|e| e["id"] == element)
                .context("unknown element")?;
            // A visual not on a sheet (a Word picture) has no range to show:
            // its region is an image of the picture itself.
            let place = match (&e["sheet"], cell_range) {
                (Value::Null, None) => None,
                (Value::Null, Some(_)) => bail!("{element} is not on a sheet; omit --range"),
                (_, None) => bail!("--range is required for {element}, which is on a sheet"),
                (sheet, Some(cell_range)) => Some((sheet.clone(), cell_range)),
            };
            e["evidence"].as_array_mut().unwrap().push(json!(id));
            let mut region = json!({"id":id,"image":image,"image_sha256":image_hash,
                "source_sha256":value["source"]["sha256"],"bbox":bbox});
            if let Some((sheet, cell_range)) = place {
                region["sheet"] = sheet;
                region["range"] = json!(cell_range);
            }
            value["regions"].as_array_mut().unwrap().push(region);
            value["review"] = json!({"status":"pending"});
            let report = validate(root, &ext, &value)?;
            write(&structure, &value)?;
            Ok(report)
        }
        StructureCommand::Review {
            extraction: path,
            structure,
            decision,
            actor,
            reason,
        } => {
            let ext = extraction(&path)?;
            let mut value = read(&structure, None)?;
            let report = validate(root, &ext, &value)?;
            if matches!(decision, Decision::Accepted) {
                ensure!(
                    array(&report["unresolved"])?.is_empty(),
                    "unexamined/unreadable cells, unassigned roles, or unresolved visual/OCR readings remain"
                );
            }
            value["review"] = json!({"status":if matches!(decision,Decision::Accepted) {"accepted"} else {"rejected"},"content_hash":json_content_hash(&value),"actor":actor,"reason":reason});
            let report = validate(root, &ext, &value)?;
            write(&structure, &value)?;
            Ok(report)
        }
    }
}

fn target_collection(value: &Value, id: &str) -> Result<&'static str> {
    let matches: Vec<_> = ["elements", "visuals"]
        .into_iter()
        .filter(|key| {
            value[*key]
                .as_array()
                .is_some_and(|items| items.iter().any(|e| e["id"] == id))
        })
        .collect();
    ensure!(matches.len() == 1, "unknown or ambiguous element/visual ID");
    Ok(matches[0])
}

/// Attach reviewed interpretations as context without replacing any original source text.
pub fn apply(
    root: &Path,
    paths: &[PathBuf],
    extractions: &[PathBuf],
    input: &mut crate::specifications::Input,
) -> Result<()> {
    let mut seen = BTreeSet::new();
    // The extractions read so far, by the first of each document: each is
    // read in order once while looking for one, rather than again for each
    // structure, and only the one found is kept.
    let mut documents: HashMap<String, usize> = HashMap::new();
    let mut read_up_to = 0;
    for path in paths {
        let value = read(path, None)?;
        validate_schema(&value)?;
        let document = string(&value["document"])?;
        ensure!(
            seen.insert(document.to_owned()),
            "duplicate document structure"
        );
        let mut found = None;
        if let Some(&at) = documents.get(document) {
            found = Some((at, crate::extraction::read(&extractions[at])?));
        }
        while found.is_none() && read_up_to < extractions.len() {
            let at = read_up_to;
            let ext = crate::extraction::read(&extractions[at])?;
            read_up_to += 1;
            if let Some(id) = ext["document_id"].as_str() {
                documents.entry(id.to_owned()).or_insert(at);
            }
            if ext["document_id"] == document {
                found = Some((at, ext));
            }
        }
        let (at, ext) = found.context("structure document missing from capture")?;
        context::assets(&extractions[at], &ext)?;
        attach(root, &ext, &value, input)?;
    }
    Ok(())
}

pub fn attach(
    root: &Path,
    extraction: &Extraction,
    structure: &Value,
    input: &mut crate::specifications::Input,
) -> Result<()> {
    let document = string(&structure["document"])?;
    ensure!(
        extraction["document_id"] == document,
        "structure document missing from capture"
    );
    let report = validate(root, extraction, structure)?;
    ensure!(
        report["ready"] == true,
        "structure requires current accepted review and complete reading"
    );
    input.revisions.insert(
        document.into(),
        hash(&encoded(&Value::Array(vec![
            extraction.to_value(),
            structure.clone(),
        ]))),
    );
    input.structures.insert(document.into(), structure.clone());
    Ok(())
}

pub fn sheet_context(value: &Value, sheet: &str) -> Value {
    json!({"document":value["document"],"source":value["source"],"review":value["review"],
        "elements":value["elements"].as_array().unwrap().iter().filter(|e| e["sheet"] == sheet).collect::<Vec<_>>(),
        "regions":value["regions"].as_array().unwrap().iter().filter(|r| r["sheet"].is_null() || r["sheet"] == sheet).collect::<Vec<_>>(),
        "visuals":value["visuals"].as_array().unwrap().iter().filter(|v| v["sheet"].is_null() || v["sheet"] == sheet).collect::<Vec<_>>()})
}

pub fn validate_snapshot(document: &str, value: &Value) -> Result<()> {
    validate_schema(value)?;
    relations::validate_elements(array(&value["elements"])?, value)?;
    ensure!(
        value["document"] == document
            && value["review"]["status"] == "accepted"
            && value["review"]["content_hash"] == json_content_hash(value),
        "invalid or stale structure snapshot"
    );
    ensure!(
        array(&value["visuals"])?.iter().all(policy::visual_ready),
        "unresolved visual/OCR snapshot"
    );
    for element in array(&value["elements"])? {
        for cell in array(&element["cells"])? {
            ensure!(
                cell["role"] != "unassigned"
                    && ["read", "empty"].contains(&string(&cell["text_state"])?),
                "unresolved structure snapshot"
            );
        }
    }
    Ok(())
}
