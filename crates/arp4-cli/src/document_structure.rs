//! Read-only interpretations of Office/PDF text and visual objects. Never used by writeback.
use crate::data::{array, encoded, hash, immutable, read, string, under, write};
use anyhow::{Context, Result, ensure};
use clap::{Subcommand, ValueEnum};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};
mod context;
mod inference;
mod policy;
mod relations;
pub use policy::{policy as requirement_policy, requirements};

pub fn schema() -> Value {
    serde_json::from_str(include_str!(
        "../../../contracts/document-structure-schema.json"
    ))
    .unwrap()
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
        #[arg(long)]
        range: String,
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
    },
}

pub fn content_hash(value: &Value) -> String {
    let mut content = value.clone();
    content.as_object_mut().unwrap().remove("review");
    hash(&encoded(&content))
}

fn validate_schema(value: &Value) -> Result<()> {
    let validator = jsonschema::validator_for(&schema())?;
    let errors: Vec<_> = validator
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

fn extraction(path: &Path) -> Result<Value> {
    let value = read(path, Some("extraction"))?;
    let extension = Path::new(string(&value["source"]["path"])?)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    ensure!(
        ["xlsx", "xlsm", "docx", "pptx", "pdf"].contains(&extension.as_str()),
        "document structure requires Excel, DOCX, PPTX or PDF extraction"
    );
    context::assets(path, &value)?;
    Ok(value)
}

pub fn initialize(root: &Path, extraction: &Value) -> Result<Value> {
    let elements = inference::elements(extraction)?;
    let value = json!({"schema_version":1,"document":extraction["document_id"],"source":{"path":extraction["source"]["path"],"sha256":extraction["source"]["sha256"]},
        "extraction_hash":hash(&encoded(extraction)),"regions":[],"elements":elements,"visuals":policy::visuals(extraction)?,"review":{"status":"pending"}});
    validate(root, extraction, &value)?;
    Ok(value)
}

/// Validity is distinct from readiness: pending/unreadable cells remain inspectable.
pub fn validate(root: &Path, extraction: &Value, value: &Value) -> Result<Value> {
    validate_schema(value)?;
    relations::validate(value)?;
    ensure!(
        value["document"] == extraction["document_id"]
            && value["source"]["path"] == extraction["source"]["path"]
            && value["source"]["sha256"] == extraction["source"]["sha256"]
            && value["extraction_hash"] == hash(&encoded(extraction)),
        "structure/extraction version mismatch"
    );
    let original = under(root, string(&value["source"]["path"])?)?;
    ensure!(
        hash(&fs::read(original)?) == value["source"]["sha256"],
        "original changed; re-import and reinterpret"
    );
    let sheets: BTreeMap<_, _> = array(&extraction["sheets"])?
        .iter()
        .map(|s| (s["name"].as_str().unwrap(), s))
        .collect();
    let mut regions = BTreeMap::new();
    for r in array(&value["regions"])? {
        let id = string(&r["id"])?;
        ensure!(regions.insert(id, r).is_none(), "duplicate region ID");
        ensure!(
            sheets.contains_key(string(&r["sheet"])?),
            "unknown region sheet"
        );
        range(string(&r["range"])?)?;
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
    let mut ids = BTreeSet::new();
    let mut owned = BTreeSet::new();
    let mut unresolved = policy::validate_visuals(extraction, value)?;
    for element in array(&value["elements"])? {
        ensure!(
            ids.insert(string(&element["id"])?),
            "duplicate element/cell ID"
        );
        let sheet_name = string(&element["sheet"])?;
        let sheet = sheets.get(sheet_name).context("unknown element sheet")?;
        let original_cells: BTreeMap<_, _> = array(&sheet["cells"])?
            .iter()
            .map(|c| (c["address"].as_str().unwrap(), c))
            .collect();
        let cells = array(&element["cells"])?;
        let local: BTreeMap<_, _> = cells
            .iter()
            .map(|c| (c["id"].as_str().unwrap(), c))
            .collect();
        let evidence: Vec<_> = array(&element["evidence"])?
            .iter()
            .map(|id| {
                let r = *regions.get(string(id)?).context("unknown image region")?;
                ensure!(r["sheet"] == sheet_name, "image region on different sheet");
                range(string(&r["range"])?)
            })
            .collect::<Result<_>>()?;
        if element["reading"]["method"] == "vision" {
            ensure!(
                !evidence.is_empty(),
                "vision reading requires image evidence"
            );
        }
        for cell in cells {
            let id = string(&cell["id"])?;
            ensure!(ids.insert(id), "duplicate element/cell ID");
            let address = string(&cell["address"])?;
            let point = crate::excel::coordinate(address)?;
            ensure!(
                owned.insert((sheet_name, address)),
                "cell belongs to multiple elements"
            );
            if element["reading"]["method"] == "vision" {
                ensure!(
                    evidence.iter().any(|r| contains(*r, point)),
                    "vision cell outside supplied image regions"
                );
            }
            let text = original_cells.get(address).map(|c| &c["value"]);
            match string(&cell["text_state"])? {
                "read" => ensure!(
                    text.is_some_and(|t| !t.is_null() && t.as_str().is_none_or(|s| !s.is_empty()))
                        || original_cells
                            .get(address)
                            .is_some_and(|c| c["formula"].is_string()),
                    "read cell has no captured text; OCR cannot become an exact source quote"
                ),
                "empty" => ensure!(
                    text.is_none_or(|t| t.is_null() || t == "")
                        && original_cells
                            .get(address)
                            .is_none_or(|c| !c["formula"].is_string()),
                    "empty cell contains original content"
                ),
                _ => unresolved
                    .push(json!({"element":element["id"],"cell":id,"state":cell["text_state"]})),
            }
            if cell["role"] == "unassigned" {
                unresolved.push(json!({"cell":id,"state":"unassigned_role"}));
            }
            for header in array(&cell["headers"])? {
                let h = local
                    .get(string(header)?)
                    .context("header must reference a cell in the same element")?;
                ensure!(
                    ["row_header", "column_header"].contains(&string(&h["role"])?),
                    "header target is not a header"
                );
            }
        }
        // Iterative traversal avoids recursive overflow for deeply nested headers.
        for id in local.keys() {
            let mut pending = vec![*id];
            let mut seen = BTreeSet::new();
            while let Some(current) = pending.pop() {
                if !seen.insert(current) {
                    continue;
                }
                for h in array(&local[current]["headers"])? {
                    let h = string(h)?;
                    ensure!(h != *id, "cyclic header relationship");
                    pending.push(h);
                }
            }
        }
    }
    // A structure edit cannot silently drop captured cells, including formulas/notes.
    for (name, sheet) in &sheets {
        for cell in array(&sheet["cells"])? {
            ensure!(
                owned.contains(&(*name, string(&cell["address"])?)),
                "structure omits original cell {}!{}",
                name,
                cell["address"]
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
                ["xlsx", "xlsm"].contains(&policy::extension(&ext).as_str()),
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
        StructureCommand::Schema { out } => {
            immutable(&out, &encoded(&schema()))?;
            Ok(json!({"schema":out}))
        }
        StructureCommand::Init {
            extraction: path,
            out,
        } => {
            let ext = extraction(&path)?;
            ensure!(!out.exists(), "structure output already exists");
            write(&out, &initialize(root, &ext)?)?;
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
            let sheet = e["sheet"].clone();
            e["evidence"].as_array_mut().unwrap().push(json!(id));
            let source_hash = value["source"]["sha256"].clone();
            value["regions"].as_array_mut().unwrap().push(json!({"id":id,"sheet":sheet,"range":cell_range,"image":image,"image_sha256":image_hash,"source_sha256":source_hash,"bbox":bbox}));
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
            value["review"] = json!({"status":if matches!(decision,Decision::Accepted) {"accepted"} else {"rejected"},"content_hash":content_hash(&value),"actor":actor,"reason":reason});
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
            let ext = read(path, Some("extraction"))?;
            if ext["document_id"] == document {
                context::assets(path, &ext)?;
                found = Some(ext);
                break;
            }
        }
        let ext = found.context("structure document missing from capture")?;
        let report = validate(root, &ext, &value)?;
        ensure!(
            report["ready"] == true,
            "structure requires current accepted review and complete reading"
        );
        input
            .revisions
            .insert(document.into(), hash(&encoded(&json!([ext, value]))));
        input.structures.insert(document.into(), value);
    }
    Ok(())
}

pub fn sheet_context(value: &Value, sheet: &str) -> Value {
    json!({"document":value["document"],"source":value["source"],"review":value["review"],
        "elements":value["elements"].as_array().unwrap().iter().filter(|e| e["sheet"] == sheet).collect::<Vec<_>>(),
        "regions":value["regions"].as_array().unwrap().iter().filter(|r| r["sheet"] == sheet).collect::<Vec<_>>(),
        "visuals":value["visuals"].as_array().unwrap().iter().filter(|v| v["sheet"].is_null() || v["sheet"] == sheet).collect::<Vec<_>>()})
}

pub fn validate_snapshot(document: &str, value: &Value) -> Result<()> {
    validate_schema(value)?;
    relations::validate(value)?;
    ensure!(
        value["document"] == document
            && value["review"]["status"] == "accepted"
            && value["review"]["content_hash"] == content_hash(value),
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
