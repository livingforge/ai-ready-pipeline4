use super::*;
use std::collections::{HashMap, HashSet};

impl Store {
    pub fn inspect(&self, dir: &Path, require_reviewed: bool) -> Result<Inspection> {
        self.inspect_with(dir, require_reviewed, &BTreeMap::new())
    }
    /// Inspects the document as it would be with the files in `planned` (by
    /// name, like `mappings.yml`) written or removed, so an edit is validated
    /// before anything is written.
    pub(super) fn inspect_with(
        &self,
        dir: &Path,
        require_reviewed: bool,
        planned: &Planned,
    ) -> Result<Inspection> {
        Ok(self
            .inspect_layout(dir, require_reviewed, planned, false)?
            .0)
    }
    /// [`Store::inspect_with`] together with the layout the inspection read,
    /// when `keep_layout` asks for it. Otherwise the layout is dropped as soon
    /// as the pages are read: it is as large as the extraction.
    pub(super) fn inspect_layout(
        &self,
        dir: &Path,
        require_reviewed: bool,
        planned: &Planned,
        keep_layout: bool,
    ) -> Result<(Inspection, Option<Value>)> {
        let files = self
            .file_hashes_with(dir, planned)?
            .context("document not found")?;
        let fp = fingerprint_of(&files);
        let metadata = self.management(dir)?;
        let meta = read(&metadata.join("document.yml"), Some("document"))?;
        let id = string(&meta["document_id"])?;
        let entry = if dir.join("proposal.json").is_file() {
            self.entry("changes", id)?
        } else {
            self.document(id)?
        };
        ensure!(entry == dir, "document path identity mismatch");
        ensure!(
            meta["source"]["path"] == self.source_path(id)?,
            "document source does not match its ID; re-import the original"
        );
        let ext_key = string(&meta["extraction"])?;
        // The file hash is the hash of the canonical encoding ARP wrote, which the key names.
        ensure!(
            files["extraction.json"] == ext_key,
            "extraction integrity failure"
        );
        let extraction = crate::extraction::read_record(&metadata.join("extraction.json"))?;
        let axes = identity::stored_axes(dir, planned)?;
        // The extraction takes its document and original from this document.yml
        // when it is read; the extraction key above ties it to the file.
        let layout = identity::load(dir, &extraction, planned)?;
        let original = self.original(string(&meta["source"]["path"])?)?;
        let source_missing = original.is_none();
        let source_current = original
            .map(|original| {
                Ok::<_, anyhow::Error>(hash(&fs::read(original)?) == meta["source"]["sha256"])
            })
            .transpose()?
            .unwrap_or(false);
        // Every file besides the records and assets is a content page, read once here.
        let mut content_pages = vec![];
        let mut page_files = BTreeMap::new();
        for (name, path) in self.planned_files(dir, planned)? {
            if name.starts_with("assets/")
                || RECORDS.contains(&name.as_str())
                || name == "proposal.json"
            {
                continue;
            }
            ensure!(
                name.starts_with("content/") && (name.ends_with(".yml") || name.ends_with(".yaml")),
                "unmanaged document file: {name}"
            );
            let page = read_planned(&name, &path, planned, "content")?;
            let page = identity::runtime(&page, &layout, &extraction)?;
            page_files.insert(string(&page["page_id"])?.to_owned(), name);
            content_pages.push(page);
        }
        let structure_sha256 = layout["structure_sha256"].as_str().map(str::to_owned);
        let layout = keep_layout.then_some(layout);
        let mut stored = complete_mappings(read_planned(
            "mappings.yml",
            &metadata.join("mappings.yml"),
            planned,
            "mappings",
        )?);
        // A journal recording nothing is not stored: it is the one of this
        // extraction.
        if stored.get("interpretation").is_none() {
            stored["interpretation"] =
                crate::document_structure::empty_corrections(&extraction, ext_key);
        }
        // Checked like the original, with the pages slide operations delete or add.
        let operated = operated_extraction(&extraction, &stored["operations"])?;
        let (mappings, entries) = with_default_entries(stored, &operated)?;
        let journal = &mappings["interpretation"];
        crate::document_structure::validate_corrections(journal)?;
        if !structure_format(&meta)? {
            ensure!(
                journal["elements"].as_array().is_some_and(Vec::is_empty)
                    && journal["readings"].as_array().is_some_and(Vec::is_empty)
                    && journal["cell_states"].as_array().is_some_and(Vec::is_empty)
                    && journal["visuals"].as_array().is_some_and(Vec::is_empty)
                    && journal["regions"].as_array().is_some_and(Vec::is_empty),
                "text document cannot contain structure corrections"
            );
        }
        let native_text = extraction["parser"]
            .as_str()
            .is_some_and(|p| p.contains(";native-text/"));
        let text_format = extraction["parser"]
            .as_str()
            .is_some_and(|p| !p.contains(";cells/"));
        // Word and PowerPoint lay paragraphs and table rows out as rows, which
        // row operations insert and delete, and PowerPoint slides are copied
        // and deleted; other text formats keep their structure.
        let laid_out = extraction["parser"]
            .as_str()
            .is_some_and(|p| p.contains(";word-blocks/") || p.contains(";slide-blocks/"));
        let presentation = Path::new(string(&meta["source"]["path"])?)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("pptx"));
        ensure!(
            array(&mappings["operations"])?.iter().all(|o| {
                if crate::document_source::is_presentation_operation(o) {
                    presentation
                } else {
                    !text_format
                        || laid_out
                            && matches!(o["kind"].as_str(), Some("insert_rows" | "delete_rows"))
                }
            }),
            "row operations apply to Excel, Word and PowerPoint documents, column and image operations to Excel only, and slide and shape operations to PowerPoint only"
        );
        if let Cow::Owned(operated) = &operated {
            ensure_slide_pages(&extraction, operated, &content_pages)?;
        }
        let operations =
            excel::parse_operations(array(&mappings["operations"])?, array(&operated["sheets"])?)?;
        let image_operations = excel::parse_image_operations(
            array(&mappings["operations"])?,
            array(&operated["sheets"])?,
        )?;
        let shape_operations =
            crate::document_source::parse_shape_operations(array(&mappings["operations"])?)?;
        let assets: Vec<String> = image_operations
            .iter()
            .map(|o| o.asset.clone())
            .chain(
                shape_operations
                    .iter()
                    .filter_map(|o| o.asset().map(str::to_owned)),
            )
            .collect();
        self.image_assets(dir, &assets)?;
        let (mappings, entries) =
            regenerate_mappings(mappings, entries, &operated, &content_pages, &operations)?;
        // The pages give their values and fonts up; nothing reads them after.
        let content = Self::validate_content(&entries, content_pages)?;
        validate_coverage(
            &operated,
            &mappings,
            &entries,
            &content,
            &operations,
            native_text,
            text_format,
        )?;
        let values = content.values;
        let fonts = content.fonts;
        for asset in array(&extraction["assets"])? {
            ensure!(
                hash(&fs::read(under(
                    dir,
                    &format!("assets/{}", string(&asset["path"])?)
                )?)?)
                    == asset["sha256"],
                "asset evidence changed"
            );
        }
        let mut reviewed = false;
        let review = metadata.join("review.json");
        if review.exists() {
            let r = read(&review, Some("review"))?;
            let (_, key) = self.formation(dir)?;
            ensure!(r["formation"] == key, "review/formation identity mismatch");
            reviewed = r["content"] == fp;
        }
        ensure!(
            !require_reviewed || reviewed,
            "content changed or not reviewed; run documents review"
        );
        Ok((
            Inspection {
                meta,
                extraction,
                mappings,
                entries,
                values,
                fonts,
                page_files,
                fingerprint: fp,
                reviewed,
                source_current,
                source_missing,
                axes,
                structure_sha256,
            },
            layout,
        ))
    }
}
impl Store {
    /// The structure interpretation and its replay report: the extraction with the
    /// document's corrections replayed. None for text formats and a changed original.
    /// Replaying infers every table of the extraction, so only commands reading the
    /// structure ask for it.
    pub fn interpretation(&self, inspected: &Inspection) -> Result<Option<(Value, Value)>> {
        if !inspected.source_current || !structure_format(&inspected.meta)? {
            return Ok(None);
        }
        // Inspection verified the extraction against this key.
        crate::document_structure::replay_verified_corrections(
            &self.root,
            &inspected.mappings["interpretation"],
            &inspected.extraction,
            string(&inspected.meta["extraction"])?,
            &inspected.axes,
        )
        .map(Some)
    }
    fn validate_content(entries: &[Entry], mut pages: Vec<Value>) -> Result<ValidatedContent> {
        // Positioned entries by page and block, so that each table block does not
        // scan every entry of a document with many sheets, slides or pages.
        let mut positioned: BTreeMap<(&str, &str), Vec<(&Entry, EntryPosition)>> = BTreeMap::new();
        for e in entries {
            if let Some(position) = e.position()? {
                positioned
                    .entry((e.page()?, e.block()?))
                    .or_default()
                    .push((e, position));
            }
        }
        let mut values = HashMap::new();
        let mut fonts = HashMap::new();
        let mut expected = BTreeSet::new();
        let mut page_ids = BTreeSet::new();
        for page in &mut pages {
            let page_id = string(&page["page_id"])?.to_owned();
            ensure!(page_ids.insert(page_id.clone()), "duplicate page ID");
            // Values and fonts are moved into the maps rather than copied.
            let mut page_fonts = page["fonts"].take();
            for (block, body) in page["blocks"].as_object_mut().unwrap() {
                let base = (page_id.clone(), block.clone());
                expected.insert((base.0.clone(), base.1.clone(), String::new()));
                // Local links require full link/asset semantics; reject unsupported links instead of silently accepting them.
                if let Some(links) = body["links"].as_array() {
                    ensure!(
                        links
                            .iter()
                            .all(|v| v.as_str().is_some_and(|s| s.starts_with("https://")
                                || s.starts_with("http://")
                                || s.starts_with("mailto:"))),
                        "local document links are not yet supported by Rust"
                    );
                }
                // Looked up with get_mut: indexing a missing member mutably adds it.
                if let Some(fields) = body.get_mut("fields").and_then(Value::as_object_mut) {
                    for (field, value) in fields {
                        values.insert(
                            (base.0.clone(), base.1.clone(), field.clone()),
                            value.take(),
                        );
                    }
                }
                let has_fields = body.get("fields").is_some();
                if let Some(rows) = body.get_mut("rows").and_then(Value::as_object_mut) {
                    ensure!(!has_fields, "table cannot contain fields");
                    let mut consumed = HashSet::new();
                    for (e, pos) in positioned
                        .get(&(page_id.as_str(), block.as_str()))
                        .into_iter()
                        .flatten()
                    {
                        let (row, col) = (pos.row, pos.column);
                        let value = rows
                            .get_mut(row)
                            .and_then(|r| r.get_mut(col))
                            .context("missing table position")?;
                        ensure!(consumed.insert((row, col)), "duplicate table position");
                        ensure!(
                            kind(value) != "object" && (value.is_null() || kind(value) == pos.kind),
                            "table type/value mismatch"
                        );
                        ensure!(!e.field_is_null(), "table field required");
                        if let Some(font) = page_fonts
                            .get_mut(block.as_str())
                            .and_then(|fonts| fonts.get_mut(row))
                            .and_then(|fonts| fonts.get_mut(col))
                            .filter(|font| !font.is_null())
                        {
                            fonts.insert(e.key()?, font.take());
                        }
                        ensure!(
                            values.insert(e.key()?, value.take()).is_none(),
                            "duplicate table field"
                        );
                    }
                    let mut count = 0;
                    for row in rows.values() {
                        let obj = row.as_object().context("invalid table row")?;
                        ensure!(!obj.is_empty(), "empty table row");
                        count += obj.len();
                    }
                    ensure!(count == consumed.len(), "unmapped table value");
                }
            }
        }
        ensure!(!page_ids.is_empty(), "content is empty");
        Ok(ValidatedContent {
            values,
            fonts,
            expected,
        })
    }
    pub fn status(
        &self,
        id: Option<&str>,
        proposal: Option<&str>,
        require_reviewed: bool,
    ) -> Result<Value> {
        let mut dirs = vec![];
        if let Some(p) = proposal {
            dirs.push((p.to_owned(), self.proposal(p)?))
        } else if let Some(id) = id {
            dirs.push((id.to_owned(), self.document(id)?))
        } else {
            for id in self.ids("documents", None)? {
                dirs.push((id.clone(), self.document(&id)?));
            }
            for id in self.ids("changes", None)? {
                dirs.push((id.clone(), self.proposal(&id)?));
            }
        }
        Ok(json!(
            dirs.into_iter()
                .map(|(id, dir)| self.state(&id, &dir, require_reviewed))
                .collect::<Vec<_>>()
        ))
    }

    /// The status row of the document or proposal in `dir`: its state, and the
    /// blockers or error that keep it from the next step.
    pub(super) fn state(&self, id: &str, dir: &Path, require_reviewed: bool) -> Value {
        let is_proposal = dir.join("proposal.json").is_file();
        let inspected = (|| -> Result<Value> {
            let mut r = self.inspect(dir, require_reviewed)?;
            let pending = r
                .entries
                .iter()
                .filter(|e| e.writeback().is_ok_and(|w| w == "pending"))
                .count();
            r.release_content();
            let interpretation = self.interpretation(&r)?;
            // The tables of the element metadata are those of the
            // interpretation the layout was classified from; the replay infers
            // tables with this version of ARP.
            let structure_outdated = interpretation
                .as_ref()
                .map(|(structure, _)| {
                    Ok::<_, anyhow::Error>(
                        r.structure_sha256.as_deref()
                            != Some(
                                elements::interpretation_digest(array(&structure["elements"])?)?
                                    .as_str(),
                            ),
                    )
                })
                .transpose()?;
            let report = interpretation.map(|(_, report)| report);
            let formation = self.management(dir)?.join("formation.json");
            let (recorded, by_import) = if formation.exists() {
                let (f, _) = self.formation(dir)?;
                (
                    f["content"] == r.fingerprint,
                    adoption::formed_by_import(&f),
                )
            } else {
                ensure!(is_proposal, "formation missing from adopted document");
                (false, false)
            };
            let mut blockers = vec![];
            if r.source_missing {
                blockers.push("source_missing");
            } else if !r.source_current {
                blockers.push("source_changed");
            }
            if pending > 0 {
                blockers.push("unresolved_mappings");
            }
            if is_proposal {
                let plan = read(&dir.join("proposal.json"), Some("proposal"))?;
                ensure!(
                    plan["document_id"] == r.meta["document_id"],
                    "proposal identity mismatch"
                );
                let current = self.document(string(&r.meta["document_id"])?)?;
                if self.fingerprint(&current)? != plan["base"].as_str().map(str::to_owned) {
                    blockers.push("authority_changed");
                }
            }
            let state = if !blockers.is_empty() {
                "blocked"
            } else if is_proposal {
                if recorded {
                    "ready_to_adopt"
                } else {
                    "needs_record"
                }
            } else if r.reviewed {
                "reviewed"
            } else if by_import && recorded {
                // Adopted as import extracted it, with no edits to review.
                "imported"
            } else {
                "needs_review"
            };
            let no_changes = if is_proposal {
                let current = self.document(id)?;
                self.fingerprint(&current)?.as_deref() == Some(&r.fingerprint)
            } else {
                false
            };
            Ok(
                json!({"document_id":r.meta["document_id"],"directory":dir,"reviewed":r.reviewed,
                "source_current":r.source_current,"content":r.fingerprint,"source_sha256":r.meta["source"]["sha256"],"pending":pending,
                "structure_ready":report.as_ref().is_some_and(|report| report["ready"] == true),
                "structure_conflicts":report.as_ref().map(|report| report["conflicts"].as_array().map(Vec::len).unwrap_or(0)),
                "structure_outdated":structure_outdated,
                "state":state,"blockers":blockers,"no_changes":no_changes}),
            )
        })();
        let mut row = match inspected {
            Ok(row) => row,
            Err(e) => {
                json!({"document_id":id,"directory":dir,"state":"invalid","error":format!("{e:#}")})
            }
        };
        if is_proposal {
            row["proposal"] = json!(true);
        }
        row
    }
}

struct ValidatedContent {
    values: HashMap<(String, String, String), Value>,
    fonts: HashMap<(String, String, String), Value>,
    /// The page blocks, keyed with an empty field. Every value is expected too.
    expected: BTreeSet<(String, String, String)>,
}

fn validate_coverage(
    extraction: &Value,
    mappings: &Value,
    entries: &[Entry],
    content: &ValidatedContent,
    operations: &[excel::StructuralOperation],
    native_text: bool,
    text_format: bool,
) -> Result<()> {
    let values = &content.values;
    let expected = &content.expected;
    // Keys borrow from the extraction and mappings: a large workbook has an entry per cell.
    // Hashed: every entry looks up its key, target and destination, and a large
    // workbook has an entry per cell.
    let mut cells = HashMap::new();
    for sheet in array(&extraction["sheets"])? {
        for c in array(&sheet["cells"])? {
            ensure!(
                cells
                    .insert((string(&sheet["name"])?, string(&c["address"])?), c)
                    .is_none(),
                "duplicate extraction cell"
            );
        }
    }
    let sheet_names: HashSet<&str> = array(&extraction["sheets"])?
        .iter()
        .map(|sheet| string(&sheet["name"]))
        .collect::<Result<_>>()?;
    let mut actual = HashSet::new();
    // New table column names by sheet and cell, checked for duplicates below.
    let mut header_edits = HashMap::new();
    let mut covered_cells = HashSet::new();
    let mut destinations = HashSet::new();
    // Written positions are checked against the merges as they stand after
    // the operations: an insertion inside a merge grows it over the new cells.
    let mut merges = BTreeMap::new();
    let mut computed = BTreeMap::new();
    for sheet in array(&extraction["sheets"])? {
        merges.insert(
            string(&sheet["name"])?.to_owned(),
            excel::merges_after(sheet, operations)?,
        );
        computed.insert(string(&sheet["name"])?, excel::ComputedRanges::new(sheet)?);
    }
    let merges = merges
        .iter()
        .map(|(name, merges)| Ok((name.as_str(), excel::Merges::new(merges)?)))
        .collect::<Result<BTreeMap<_, _>>>()?;
    for e in entries {
        let k = e.key()?;
        ensure!(!e.reason()?.trim().is_empty(), "new text requires reason");
        let mapped_target = e.target()?;
        if let Some(MappingTarget::Cell { sheet, cell }) = mapped_target {
            let pair = (sheet, cell);
            if cells.contains_key(&pair) {
                if native_text && let Some(value) = values.get(&k) {
                    ensure!(
                        *value == cells[&pair]["value"],
                        "text content is a read-only derived view; edit the original and re-import with the same document ID"
                    );
                }
                covered_cells.insert(pair);
            } else {
                // A workbook cell holding nothing in the original takes a value.
                ensure!(
                    !text_format
                        && e.writeback()? == "cell"
                        && sheet_names.contains(sheet)
                        && excel::coordinate(cell).is_ok(),
                    "missing Excel target"
                );
            }
        }
        match e.writeback()? {
            "excluded" => {
                let reason = e.reason()?;
                ensure!(!reason.trim().is_empty(), "exclusion requires reason");
                // An excluded cell is never written, so an edit would be lost.
                if let (Some(MappingTarget::Cell { sheet, cell }), Some(value)) =
                    (&mapped_target, values.get(&k))
                {
                    let source = &cells[&(*sheet, *cell)];
                    let original = match source["formula"].as_str() {
                        Some(formula) if e.position()?.is_some_and(|p| p.column == "formula") => {
                            json!(format!("={formula}"))
                        }
                        _ => source["value"].clone(),
                    };
                    ensure!(
                        *value == original
                            || (value.is_number() && value.as_f64() == original.as_f64()),
                        "{sheet}!{cell} is not written back ({reason}), so its value must stay {original}"
                    );
                }
            }
            "pending" => {}
            "cell" => {
                let value = values.get(&k).context("writeback requires field")?;
                ensure!(
                    !text_format || value.is_string(),
                    "text replacement must be a string; use an empty string to clear text"
                );
                let destination = match mapped_target.context("cell writeback requires target")? {
                    MappingTarget::Cell { sheet, cell } => {
                        // A cell the original leaves empty holds no value or type.
                        let source = cells.get(&(sheet, cell));
                        if let Some(source) = source {
                            ensure!(
                                source["type"] != "formula" && source["type"] != "error",
                                "formula/error cell cannot be overwritten"
                            );
                            ensure!(
                                value.is_null()
                                    || source["type"] == kind(value)
                                    || source["type"] == "null",
                                "Excel target type mismatch"
                            );
                        }
                        let original = source.map_or(&Value::Null, |source| &source["value"]);
                        if !text_format && value != original {
                            let (column, row) = excel::coordinate(cell)?;
                            let book_sheet = array(&extraction["sheets"])?
                                .iter()
                                .find(|s| s["name"] == sheet)
                                .context("missing Excel target sheet")?;
                            excel::ensure_table_label_edit(book_sheet, column, row, value)?;
                            if excel::table_label(book_sheet, column, row)?
                                .is_some_and(|(_, header)| header)
                            {
                                header_edits.insert((sheet, cell), value);
                            }
                            computed
                                .get(sheet)
                                .context("missing Excel target sheet")?
                                .ensure_not_computed(cell)?;
                        }
                        // A cell an operation deletes is removed from the content with
                        // it; one left behind would be an edit silently dropped.
                        let mapped = excel::map_coordinate(sheet, cell, operations)?
                            .with_context(|| {
                                format!(
                                    "{sheet}!{cell} is deleted by a delete_rows/delete_columns operation; remove it from the content too"
                                )
                            })?;
                        Some((sheet, mapped))
                    }
                    MappingTarget::InsertionRow {
                        sheet,
                        insertion,
                        offset,
                        column,
                    } => {
                        let mapped = excel::resolve_insertion(
                            sheet,
                            insertion,
                            offset,
                            Some(column),
                            None,
                            operations,
                        )?;
                        Some((sheet, mapped))
                    }
                    MappingTarget::InsertionColumn {
                        sheet,
                        insertion,
                        offset,
                        row,
                    } => {
                        let mapped = excel::resolve_insertion(
                            sheet,
                            insertion,
                            offset,
                            None,
                            Some(row),
                            operations,
                        )?;
                        Some((sheet, mapped))
                    }
                    MappingTarget::Shape { .. } => {
                        bail!("the text of a shape is written back with shape writeback")
                    }
                };
                if let Some((sheet, cell)) = destination {
                    merges
                        .get(sheet)
                        .context("missing Excel target sheet")?
                        .ensure_not_hidden(sheet, &cell)?;
                    ensure!(
                        destinations.insert((sheet, cell)),
                        "duplicate writeback target"
                    );
                }
            }
            "operation" => {
                bail!("operation writeback entries are not supported; use operations")
            }
            "shape" => {
                let value = values.get(&k).context("writeback requires field")?;
                let Some(MappingTarget::Shape { sheet, shape }) = mapped_target else {
                    bail!("shape writeback requires a shape target");
                };
                let old = array(&extraction["sheets"])?
                    .iter()
                    .filter(|s| s["name"] == sheet)
                    .flat_map(|s| s["drawings"].as_array().into_iter().flatten())
                    .find(|d| d["id"] == shape)
                    .with_context(|| format!("shape {shape} on {sheet} is missing"))?["text"]
                    .as_str()
                    .unwrap_or("");
                let new = value.as_str().with_context(|| {
                    format!("the text of shape {shape} on {sheet} must be text")
                })?;
                if new != old {
                    let lines = old.matches('\n').count();
                    ensure!(
                        !new.contains('\r') && new.matches('\n').count() == lines,
                        "the text of shape {shape} on {sheet} must keep its {lines} line break(s); lines and paragraphs of a shape cannot be added or removed, so edit them in Excel"
                    );
                }
            }
            "formula" => {
                let value = values.get(&k).context("writeback requires field")?;
                let Some(MappingTarget::Cell { sheet, cell }) = mapped_target else {
                    bail!("formula writeback requires a cell target");
                };
                let source = &cells[&(sheet, cell)];
                ensure!(
                    source["type"] == "formula",
                    "{sheet}!{cell} holds no formula; formula writeback replaces an existing formula"
                );
                if value.as_str() != Some(&format!("={}", string(&source["formula"])?)) {
                    let formula = value
                        .as_str()
                        .and_then(|v| v.strip_prefix('='))
                        .with_context(|| {
                            format!("the formula of {sheet}!{cell} must be text starting with =")
                        })?;
                    excel::check_formula(formula)
                        .with_context(|| format!("formula of {sheet}!{cell}"))?;
                    computed
                        .get(sheet)
                        .context("missing Excel target sheet")?
                        .ensure_not_computed(cell)?;
                    ensure!(
                        excel::map_coordinate(sheet, cell, operations)?.is_some(),
                        "{sheet}!{cell} is deleted by a delete_rows/delete_columns operation; restore its formula"
                    );
                }
            }
            other => bail!("unsupported writeback: {other}"),
        }
        ensure!(actual.insert(k), "duplicate mapping");
    }
    if !header_edits.is_empty() {
        for sheet in array(&extraction["sheets"])? {
            let name = string(&sheet["name"])?;
            excel::ensure_unique_table_headers(sheet, |cell| {
                header_edits
                    .get(&(name, cell))
                    .map(|value| (*value).clone())
                    .or_else(|| cells.get(&(name, cell)).map(|c| c["value"].clone()))
            })?;
        }
    }
    let expected_count = expected.len() + values.keys().filter(|k| !expected.contains(*k)).count();
    ensure!(
        actual.len() == expected_count
            && actual
                .iter()
                .all(|k| values.contains_key(k) || expected.contains(k)),
        "mapping coverage mismatch"
    );
    for o in array(&mappings["omissions"])? {
        ensure!(!string(&o["reason"])?.trim().is_empty(), "invalid omission");
        let pair = target(&o["target"])?;
        ensure!(
            cells.contains_key(&pair) && covered_cells.insert(pair),
            "invalid/duplicate omission target"
        );
    }
    for &(sheet, cell) in cells.keys() {
        if excel::map_coordinate(sheet, cell, operations)?.is_none() {
            covered_cells.insert((sheet, cell));
        }
    }
    // Only existing cells are ever covered, so equal counts mean every cell is.
    ensure!(
        covered_cells.len() == cells.len(),
        "source coverage incomplete"
    );
    Ok(())
}

/// Requires a content page for each page the slide operations leave (the
/// `operated` extraction) and none for a page they delete.
fn ensure_slide_pages(extraction: &Value, operated: &Value, pages: &[Value]) -> Result<()> {
    let kept: BTreeMap<&str, &str> = array(&operated["sheets"])?
        .iter()
        .map(|sheet| Ok((string(&sheet["page"])?, string(&sheet["name"])?)))
        .collect::<Result<_>>()?;
    let present: BTreeSet<&str> = pages
        .iter()
        .map(|page| string(&page["page_id"]))
        .collect::<Result<_>>()?;
    for (index, sheet) in array(&extraction["sheets"])?.iter().enumerate() {
        let page = format!("sheet-{}", index + 1);
        ensure!(
            kept.contains_key(page.as_str()) || !present.contains(page.as_str()),
            "{} is deleted by a delete_slide operation; remove its content page {page} too",
            string(&sheet["name"])?
        );
    }
    for (page, name) in kept {
        ensure!(
            present.contains(page),
            "{name} has no content page {page}; add it, as documents slides insert does, or remove the operation that inserts {name}"
        );
    }
    Ok(())
}

pub(super) fn structure_format(meta: &Value) -> Result<bool> {
    let extension = Path::new(string(&meta["source"]["path"])?)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    Ok(crate::document_source::structure_formats().contains(&extension.as_str()))
}
