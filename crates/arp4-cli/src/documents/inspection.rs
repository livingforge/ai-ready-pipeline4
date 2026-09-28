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
        let extraction = read(&metadata.join("extraction.json"), Some("extraction"))?;
        ensure!(
            meta["source"] == extraction["source"]
                && meta["document_id"] == extraction["document_id"],
            "document/extraction mismatch"
        );
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
            page_files.insert(string(&page["page_id"])?.to_owned(), name);
            content_pages.push(page);
        }
        let stored = read_planned(
            "mappings.yml",
            &metadata.join("mappings.yml"),
            planned,
            "mappings",
        )?;
        // Checked like the original, with the pages slide operations delete or add.
        let operated = operated_extraction(&extraction, &stored["operations"])?;
        let mut mappings = with_default_entries(stored, &operated)?;
        let journal = &mappings["interpretation"];
        crate::document_structure::validate_corrections(journal)?;
        ensure!(
            journal["document"] == meta["document_id"]
                && journal["source_path"] == meta["source"]["path"],
            "document interpretation belongs to another source"
        );
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
                if is_slide_operation(o) {
                    presentation
                } else {
                    !text_format
                        || laid_out
                            && matches!(o["kind"].as_str(), Some("insert_rows" | "delete_rows"))
                }
            }),
            "row operations apply to Excel, Word and PowerPoint documents, column and image operations to Excel only, and slide operations to PowerPoint only"
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
        self.image_assets(dir, &image_operations)?;
        mappings = regenerate_mappings(mappings, &operated, &content_pages, &operations)?;
        let content = Self::validate_content(&meta, &mappings, &content_pages)?;
        validate_coverage(
            &operated,
            &mappings,
            &content,
            &operations,
            native_text,
            text_format,
        )?;
        let values = content.values;
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
            let (f, key) = self.formation(dir)?;
            ensure!(
                r["formation"] == key
                    && f["document_id"] == meta["document_id"]
                    && f["extraction"] == meta["extraction"],
                "review/formation identity mismatch"
            );
            reviewed = r["content"] == fp;
        }
        ensure!(
            !require_reviewed || reviewed,
            "content changed or not reviewed; run documents review"
        );
        Ok(Inspection {
            meta,
            extraction,
            mappings,
            values,
            page_files,
            fingerprint: fp,
            reviewed,
            source_current,
            source_missing,
        })
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
        )
        .map(Some)
    }
    fn validate_content(
        meta: &Value,
        mappings: &Value,
        pages: &[Value],
    ) -> Result<ValidatedContent> {
        let entries = array(&mappings["entries"])?;
        let tables = mappings["tables"].as_array().cloned().unwrap_or_default();
        let mut table_keys = BTreeSet::new();
        let mut table_definitions = BTreeMap::new();
        for t in &tables {
            ensure!(
                table_keys.insert((
                    string(&t["page"])?.to_owned(),
                    string(&t["block"])?.to_owned()
                )),
                "duplicate table definition"
            );
            table_definitions.insert((string(&t["page"])?, string(&t["block"])?), t);
        }
        // Positioned entries by page and block, so that each table block does not
        // scan every entry of a document with many sheets, slides or pages.
        let mut positioned: BTreeMap<(&str, &str), Vec<&Value>> = BTreeMap::new();
        for e in entries.iter().filter(|e| e.get("position").is_some()) {
            if let (Some(page), Some(block)) = (e["page"].as_str(), e["block"].as_str()) {
                positioned.entry((page, block)).or_default().push(e);
            }
        }
        let mut values = HashMap::new();
        let mut expected = BTreeSet::new();
        let mut page_ids = BTreeSet::new();
        let mut references = vec![];
        for page in pages {
            ensure!(
                page["document_id"] == meta["document_id"]
                    && page["source_path"] == meta["source"]["path"],
                "page identity mismatch"
            );
            let page_id = string(&page["page_id"])?.to_owned();
            ensure!(page_ids.insert(page_id.clone()), "duplicate page ID");
            for (block, body) in page["blocks"].as_object().unwrap() {
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
                if let Some(fields) = body["fields"].as_object() {
                    for (field, value) in fields {
                        values.insert(
                            (base.0.clone(), base.1.clone(), field.clone()),
                            value.clone(),
                        );
                    }
                }
                if let Some(rows) = body["rows"].as_object() {
                    let table = table_definitions
                        .get(&(page_id.as_str(), block.as_str()))
                        .context("rows require table definition")?;
                    ensure!(body.get("fields").is_none(), "table cannot contain fields");
                    let columns = array(&table["columns"])?;
                    let known_columns: HashSet<&str> =
                        columns.iter().filter_map(Value::as_str).collect();
                    let known = |column: &str| known_columns.contains(column);
                    let mut consumed = HashSet::new();
                    for e in positioned
                        .get(&(page_id.as_str(), block.as_str()))
                        .into_iter()
                        .flatten()
                    {
                        let pos = &e["position"];
                        let row = string(&pos["row"])?;
                        let col = string(&pos["column"])?;
                        let value = rows
                            .get(row)
                            .and_then(|r| r.get(col))
                            .context("missing table position")?;
                        ensure!(consumed.insert((row, col)), "duplicate table position");
                        ensure!(
                            kind(value) != "object"
                                && (value.is_null() || kind(value) == pos["type"]),
                            "table type/value mismatch"
                        );
                        ensure!(known(col), "unknown column");
                        ensure!(!e["field"].is_null(), "table field required");
                        ensure!(
                            values.insert(key(e)?, value.clone()).is_none(),
                            "duplicate table field"
                        );
                    }
                    for r in array(&table["references"])? {
                        let row = string(&r["row"])?;
                        let col = string(&r["column"])?;
                        ensure!(
                            consumed.insert((row, col))
                                && rows.get(row).and_then(|v| v.get(col))
                                    == Some(
                                        &json!({"ref":{"block":r["block"],"field":r["field"]}})
                                    ),
                            "invalid table reference"
                        );
                        ensure!(known(col), "unknown reference column");
                        references.push((
                            page_id.clone(),
                            string(&r["block"])?.into(),
                            string(&r["field"])?.into(),
                        ));
                    }
                    let mut count = 0;
                    for row in rows.values() {
                        let obj = row.as_object().context("invalid table row")?;
                        ensure!(!obj.is_empty(), "empty table row");
                        count += obj.len();
                    }
                    ensure!(count == consumed.len(), "unmapped table value");
                } else {
                    ensure!(!table_keys.contains(&base), "mapped table has no rows");
                }
            }
        }
        ensure!(!page_ids.is_empty(), "content is empty");
        for r in references {
            ensure!(values.contains_key(&r), "missing table reference target");
        }
        for (p, b) in table_keys {
            ensure!(
                expected.contains(&(p, b, String::new())),
                "orphaned table definition"
            );
        }
        Ok(ValidatedContent { values, expected })
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
            let r = self.inspect(dir, require_reviewed)?;
            let report = self.interpretation(&r)?.map(|(_, report)| report);
            let formation = self.management(dir)?.join("formation.json");
            let recorded = if formation.exists() {
                let (f, _) = self.formation(dir)?;
                ensure!(
                    f["document_id"] == r.meta["document_id"]
                        && f["extraction"] == r.meta["extraction"],
                    "formation identity mismatch"
                );
                f["content"] == r.fingerprint
            } else {
                ensure!(is_proposal, "formation missing from adopted document");
                false
            };
            let pending = array(&r.mappings["entries"])?
                .iter()
                .filter(|e| e["writeback"] == "pending")
                .count();
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
                "source_current":r.source_current,"content":r.fingerprint,"pending":pending,
                "structure_ready":report.as_ref().is_some_and(|report| report["ready"] == true),
                "structure_conflicts":report.as_ref().map(|report| report["conflicts"].as_array().map(Vec::len).unwrap_or(0)),
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
    /// The page blocks, keyed with an empty field. Every value is expected too.
    expected: BTreeSet<(String, String, String)>,
}

fn validate_coverage(
    extraction: &Value,
    mappings: &Value,
    content: &ValidatedContent,
    operations: &[excel::StructuralOperation],
    native_text: bool,
    text_format: bool,
) -> Result<()> {
    let entries = array(&mappings["entries"])?;
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
        let k = key(e)?;
        ensure!(
            !string(&e["reason"])?.trim().is_empty(),
            "new text requires reason"
        );
        let mapped_target = mapping_target(&e["target"])?;
        if let Some(MappingTarget::Cell { sheet, cell }) = mapped_target {
            let pair = (sheet, cell);
            ensure!(cells.contains_key(&pair), "missing Excel target");
            if native_text && let Some(value) = values.get(&k) {
                ensure!(
                    *value == cells[&pair]["value"],
                    "text content is a read-only derived view; edit the original and re-import with the same document ID"
                );
            }
            covered_cells.insert(pair);
        }
        match string(&e["writeback"])? {
            "excluded" => {
                let reason = string(&e["reason"])?;
                ensure!(!reason.trim().is_empty(), "exclusion requires reason");
                // An excluded cell is never written, so an edit would be lost.
                if let (Some(MappingTarget::Cell { sheet, cell }), Some(value)) =
                    (&mapped_target, values.get(&k))
                {
                    let source = &cells[&(*sheet, *cell)];
                    let original = match source["formula"].as_str() {
                        Some(formula) if e["position"]["column"] == "formula" => {
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
                        let source = cells.get(&(sheet, cell)).context("missing target")?;
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
                        if !text_format && *value != source["value"] {
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
