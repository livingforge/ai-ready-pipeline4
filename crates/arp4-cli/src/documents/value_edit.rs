use super::*;
use crate::fonts::FontEdit;
use std::collections::HashMap;

impl Store {
    /// Edit existing cells or text runs, including Excel formulas and table
    /// labels, and the fonts of cells and shape text.
    pub fn edit_values(&self, id: &str, request: &Value, dry_run: bool) -> Result<Value> {
        validate("value-edits", request)?;
        let dir = self.document(id)?;
        let inspected = self.inspect(&dir, false)?;
        ensure!(
            inspected.source_current,
            "source changed; re-import before editing values"
        );
        ensure!(
            request["base"] == inspected.fingerprint
                && request["source_sha256"] == inspected.meta["source"]["sha256"],
            "value edit base changed"
        );
        ensure!(
            array(&inspected.mappings["operations"])?.is_empty(),
            "value edits require a document without structural operations"
        );
        let source = under(&self.root, string(&inspected.meta["source"]["path"])?)?;
        let book = Source::open(&source)?;
        ensure!(
            book.parser() != "native-text/1",
            "text documents use direct editing and re-import"
        );
        let mut pages = BTreeMap::new();
        let mut targets = BTreeSet::new();
        for edit in array(&request["edits"])? {
            let sheet = string(&edit["sheet"])?;
            let (target, entry) = match edit["shape"].as_str() {
                Some(shape) => (
                    shape,
                    array(&inspected.mappings["entries"])?
                        .iter()
                        .find(|entry| {
                            entry["target"]["sheet"] == sheet
                                && entry["target"]["shape"] == shape
                                && entry["writeback"] == "shape"
                        })
                        .with_context(|| {
                            format!("{sheet} has no editable text of shape {shape}")
                        })?,
                ),
                None => {
                    let cell = string(&edit["cell"])?;
                    (
                        cell,
                        array(&inspected.mappings["entries"])?
                            .iter()
                            .find(|entry| {
                                entry["target"]["sheet"] == sheet
                                    && entry["target"]["cell"] == cell
                                    && matches!(
                                        entry["writeback"].as_str(),
                                        Some("cell" | "formula")
                                    )
                            })
                            .context("target is not an editable value or formula cell")?,
                    )
                }
            };
            ensure!(
                targets.insert((sheet, target)),
                "duplicate value edit target"
            );
            let page_id = string(&entry["page"])?;
            let filename = inspected
                .page_files
                .get(page_id)
                .context("missing content page")?;
            if !pages.contains_key(filename) {
                pages.insert(
                    filename.clone(),
                    self.read_content(&dir, filename, &inspected.extraction)?,
                );
            }
            let page = pages.get_mut(filename).unwrap();
            let block = string(&entry["block"])?;
            if edit.get("before").is_some() {
                let current = inspected
                    .values
                    .get(&key(entry)?)
                    .context("missing current value")?;
                ensure!(
                    *current == edit["before"],
                    "value edit expected old value differs at {sheet}!{target}"
                );
                let field = if entry["position"].is_object() {
                    &mut page["blocks"][block]["rows"][string(&entry["position"]["row"])?]
                        [string(&entry["position"]["column"])?]
                } else {
                    &mut page["blocks"][block]["fields"][string(&entry["field"])?]
                };
                *field = edit["after"].clone();
            }
            if edit.get("font").is_some() {
                ensure!(
                    book.font_edits_supported(),
                    "font edits are supported for Excel, Word and PowerPoint documents; edit the fonts of this document in Office"
                );
                let current = inspected
                    .fonts
                    .get(&key(entry)?)
                    .with_context(|| format!("{sheet}!{target} has no font to edit"))?;
                let before = edit["font"]["before"].as_object().context("font before")?;
                let after = edit["font"]["after"].as_object().context("font after")?;
                ensure!(
                    before.keys().eq(after.keys()),
                    "font before must give the current value of each property after changes at {sheet}!{target}"
                );
                for (property, value) in before {
                    ensure!(
                        crate::fonts::same(&current[property.as_str()], value),
                        "font edit expected old {property} differs at {sheet}!{target}: the font has {}",
                        current[property.as_str()]
                    );
                }
                let change = FontEdit::parse(&edit["font"]["after"])
                    .with_context(|| format!("font edit at {sheet}!{target}"))?;
                if matches!(book, Source::Excel(_)) && edit.get("shape").is_none() {
                    crate::fonts::excel_name(&change)?;
                }
                let position = &entry["position"];
                ensure!(position.is_object(), "{sheet}!{target} has no font to edit");
                page["fonts"][block][string(&position["row"])?][string(&position["column"])?] =
                    change.applied(current);
            }
        }
        let mut layout = identity::load(&dir, &inspected.extraction, &Planned::new())?;
        let mut planned: Planned = pages
            .iter()
            .map(|(name, page)| {
                Ok((
                    name.clone(),
                    Some(serialized(
                        &dir.join(name),
                        &elements::encode(page, &mut layout)?,
                    )?),
                ))
            })
            .collect::<Result<_>>()?;
        planned.insert(
            "layout.yml".into(),
            Some(serialized(Path::new("layout.yml"), &layout)?),
        );
        let checked = self.inspect_with(&dir, false, &planned)?;
        // Preview all pending cell edits, not just this request: earlier renames
        // and formula edits must take part in the same writer validation.
        let cells: HashMap<_, _> = array(&checked.extraction["sheets"])?
            .iter()
            .flat_map(|s| {
                s["cells"].as_array().into_iter().flatten().map(move |c| {
                    (
                        (s["name"].as_str().unwrap(), c["address"].as_str().unwrap()),
                        c,
                    )
                })
            })
            .collect();
        let shapes = shape_drawings(&checked.extraction)?;
        let mut changes = vec![];
        let mut formulas = vec![];
        for entry in array(&checked.mappings["entries"])? {
            if !matches!(
                entry["writeback"].as_str(),
                Some("cell" | "formula" | "shape")
            ) {
                continue;
            }
            let after = &checked.values[&key(entry)?];
            match mapping_target(&entry["target"])? {
                Some(MappingTarget::Cell { sheet, cell }) => {
                    let source_cell = cells[&(sheet, cell)];
                    match entry["writeback"].as_str() {
                        Some("cell") if *after != source_cell["value"] => {
                            changes.push(json!({"sheet":sheet,"cell":cell,"before":source_cell["value"],"after":after}));
                        }
                        Some("formula") => {
                            let before = format!("={}", string(&source_cell["formula"])?);
                            if *after != before {
                                formulas.push(
                                    json!({"sheet":sheet,"cell":cell,"before":before,"after":after}),
                                );
                            }
                        }
                        _ => {}
                    }
                }
                Some(MappingTarget::Shape { sheet, shape }) => {
                    let before = &shapes
                        .get(&(sheet, shape))
                        .with_context(|| format!("shape {shape} on {sheet} is missing"))?["text"];
                    if after != before {
                        changes.push(
                            json!({"sheet":sheet,"shape":shape,"before":before,"after":after}),
                        );
                    }
                }
                _ => {}
            }
        }
        let fonts = font_changes(&checked.extraction, &checked.mappings, &checked.fonts)?;
        // Exercise the format's writer protections before any managed files change.
        let stage = Stage::new_in(&under(&self.arp, "cache/export")?, &self.arp)?;
        let preview = stage
            .path()
            .join(source.file_name().context("missing filename")?);
        let verification = book.patch(
            &preview,
            &crate::document_source::Edits {
                operations: &[],
                changes: &changes,
                formulas: &formulas,
                fonts: &fonts,
                assets: &BTreeMap::new(),
            },
        )?;
        ensure!(
            hash(&fs::read(&source)?) == request["source_sha256"],
            "source changed during value edit"
        );
        if !dry_run {
            self.commit(&dir, &planned, &inspected.fingerprint)?;
        }
        Ok(
            json!({"document_id":id,"state":if dry_run {"planned"} else {"written"},"content":checked.fingerprint,"edits":array(&request["edits"])?.len(),"actor":request["actor"],"reason":request["reason"],"verification":verification,"layout_status":"unconfirmed"}),
        )
    }
}

/// The shapes of each sheet by sheet name and extraction ID.
pub(super) fn shape_drawings(extraction: &Value) -> Result<BTreeMap<(&str, &str), &Value>> {
    let mut shapes = BTreeMap::new();
    for sheet in array(&extraction["sheets"])? {
        for drawing in sheet["drawings"].as_array().into_iter().flatten() {
            shapes.insert((string(&sheet["name"])?, string(&drawing["id"])?), drawing);
        }
    }
    Ok(shapes)
}

/// The font changes to write back: each original cell or shape whose current
/// font (`fonts`, by mapping entry key) differs from the font extracted from the
/// original, with the properties that changed.
pub(super) fn font_changes(
    extraction: &Value,
    mappings: &Value,
    fonts: &HashMap<(String, String, String), Value>,
) -> Result<Vec<Value>> {
    if fonts.is_empty() {
        return Ok(vec![]);
    }
    let mut cells = BTreeMap::new();
    for sheet in array(&extraction["sheets"])? {
        for cell in array(&sheet["cells"])? {
            cells.insert((string(&sheet["name"])?, string(&cell["address"])?), cell);
        }
    }
    let shapes = shape_drawings(extraction)?;
    let originals: BTreeSet<&str> = array(&extraction["sheets"])?
        .iter()
        .filter_map(|sheet| sheet["name"].as_str())
        .collect();
    let mut changes = vec![];
    for entry in array(&mappings["entries"])? {
        if !matches!(
            entry["writeback"].as_str(),
            Some("cell" | "formula" | "shape")
        ) {
            continue;
        }
        let Some(current) = fonts.get(&key(entry)?) else {
            continue;
        };
        // A slide copy has no original page; it takes the fonts of the page it
        // copies, which are written before the copy is made.
        if entry["target"]["sheet"]
            .as_str()
            .is_some_and(|sheet| !originals.contains(sheet))
        {
            continue;
        }
        let (target, original) = match mapping_target(&entry["target"])? {
            Some(MappingTarget::Cell { sheet, cell }) => (
                json!({"sheet":sheet,"cell":cell}),
                &cells.get(&(sheet, cell)).context("missing cell")?["font"],
            ),
            Some(MappingTarget::Shape { sheet, shape }) => (
                json!({"sheet":sheet,"shape":shape}),
                &shapes
                    .get(&(sheet, shape))
                    .with_context(|| format!("shape {shape} on {sheet} is missing"))?["font"],
            ),
            _ => continue,
        };
        ensure!(
            !original.is_null(),
            "{} has a font the original does not record; re-import the original",
            entry["field"]
        );
        let edit = FontEdit::between(original, current)?;
        if !edit.is_empty() {
            let mut change = target;
            change["before"] = original.clone();
            change["after"] = edit.json();
            change["field"] = entry["field"].clone();
            changes.push(change);
        }
    }
    Ok(changes)
}
