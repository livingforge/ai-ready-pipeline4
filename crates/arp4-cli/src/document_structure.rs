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
    validate_hashed(root, extraction, &extraction_hash, &value, true)?;
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
    if let Ok(bytes) = serde_json::to_vec(&value)
        && crate::data::replace_cache(&path, &bytes).is_ok()
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
    // Every cell is looked up in these a few times; hashed, as their order is
    // never read, and made as large as they become, they take a fraction of
    // the time ordered ones took.
    let elements = &value.elements;
    let cell_count: usize = elements.iter().map(|e| e.cells.len()).sum();
    let mut ids = HashSet::with_capacity(elements.len() + cell_count);
    let mut owned = HashSet::with_capacity(cell_count);
    let mut unresolved = policy::validate_visuals(extraction, &value.fields)?;
    // Built once per sheet: a sheet holds many elements, each looking up its cells.
    let mut sheet_cells: HashMap<&str, HashMap<&str, &Cell>> = HashMap::new();
    for element in elements {
        ensure!(
            ids.insert(string(&element["id"])?),
            "duplicate element/cell ID"
        );
        let sheet_name = string(&element["sheet"])?;
        let sheet = sheets.get(sheet_name).context("unknown element sheet")?;
        let original_cells = match sheet_cells.entry(sheet_name) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(sheet.cells.iter().map(|c| (&*c.address, c)).collect())
            }
        };
        let cells = &element.cells;
        let local: HashMap<&str, &ElementCell> = cells.iter().map(|c| (&*c.id, c)).collect();
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
            ensure!(
                owned.insert((sheet_name, address)),
                "cell belongs to multiple elements"
            );
            if vision {
                ensure!(
                    evidence.iter().any(|r| contains(*r, point)),
                    "vision cell outside supplied image regions"
                );
            }
            let original = original_cells.get(address);
            let text = original.map(|c| &c.value);
            match cell.text_state {
                TextState::Read => ensure!(
                    text.is_some_and(|t| !t.is_null() && t.as_str().is_none_or(|s| !s.is_empty()))
                        || original.is_some_and(|c| c.formula.is_some()),
                    "read cell has no captured text; OCR cannot become an exact source quote"
                ),
                TextState::Empty => ensure!(
                    text.is_none_or(|t| t.is_null() || t == "")
                        && original.is_none_or(|c| c.formula.is_none()),
                    "empty cell contains original content"
                ),
                _ => unresolved.push(
                    json!({"element":element["id"],"cell":id,"state":cell.text_state.as_str()}),
                ),
            }
            if cell.role == Role::Unassigned {
                unresolved.push(json!({"cell":id,"state":"unassigned_role"}));
            }
            for header in &cell.headers {
                let h = local
                    .get(&**header)
                    .context("header must reference a cell in the same element")?;
                ensure!(h.role.heads(), "header target is not a header");
            }
        }
        // Iterative traversal avoids recursive overflow for deeply nested
        // headers. A cell is finished once all its headers are, and is not
        // followed again, so a header repeated down a long table is read once
        // rather than from every record; a header still on the path is a cycle.
        // Whether there is a cycle does not depend on the order cells are visited.
        // Cells are visited in their order, which reads them where they lie in
        // memory, and the path keeps the headers of each cell on it.
        let mut finished: HashMap<&str, bool> = HashMap::with_capacity(local.len());
        let mut path: Vec<(&str, &[Box<str>], usize)> = vec![];
        for cell in cells {
            let id = &*cell.id;
            if finished.contains_key(id) {
                continue;
            }
            finished.insert(id, false);
            path.push((id, &cell.headers, 0));
            while let Some(top) = path.last_mut() {
                let (current, headers, next) = *top;
                let Some(h) = headers.get(next) else {
                    finished.insert(current, true);
                    path.pop();
                    continue;
                };
                top.2 += 1;
                let h = &**h;
                ensure!(
                    finished.get(h) != Some(&false),
                    "cyclic header relationship"
                );
                if !finished.contains_key(h) {
                    finished.insert(h, false);
                    path.push((h, &local[h].headers, 0));
                }
            }
        }
    }
    // A structure edit cannot silently drop captured cells, including formulas/notes.
    for (name, sheet) in &sheets {
        for cell in &sheet.cells {
            ensure!(
                owned.contains(&(*name, &*cell.address)),
                "structure omits original cell {}!{}",
                name,
                json!(&*cell.address)
            );
        }
    }
    let fingerprint = content_hash(value);
    let reviewed =
        value["review"]["status"] == "accepted" && value["review"]["content_hash"] == fingerprint;
    Ok(
        json!({"valid":true,"ready":reviewed && unresolved.is_empty(),"review_current":value["review"]["content_hash"] == fingerprint,
        "content_hash":fingerprint,"unresolved":unresolved}),
    )
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
    for path in paths {
        let value = read(path, None)?;
        validate_schema(&value)?;
        let document = string(&value["document"])?;
        ensure!(
            seen.insert(document.to_owned()),
            "duplicate document structure"
        );
        let mut found = None;
        for path in extractions {
            let ext = crate::extraction::read(path)?;
            if ext["document_id"] == document {
                context::assets(path, &ext)?;
                found = Some(ext);
                break;
            }
        }
        let ext = found.context("structure document missing from capture")?;
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
