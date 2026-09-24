use super::*;

impl Store {
    pub fn inspect(&self, dir: &Path, require_reviewed: bool) -> Result<Inspection> {
        let fp = self.fingerprint(dir)?.context("document not found")?;
        let metadata = self.management(dir)?;
        let meta = read(&metadata.join("document.yml"), Some("document"))?;
        let id = string(&meta["document_id"])?;
        identifier(id)?;
        if dir.starts_with(self.arp.join("documents")) {
            ensure!(self.document(id)? == dir, "document path identity mismatch");
        }
        let ext_key = string(&meta["extraction"])?;
        let extraction = read(&metadata.join("extraction.json"), Some("extraction"))?;
        ensure!(
            hash(&encoded(&extraction)) == ext_key,
            "extraction integrity failure"
        );
        ensure!(
            meta["source"] == extraction["source"]
                && meta["document_id"] == extraction["document_id"],
            "document/extraction mismatch"
        );
        let content_pages = self
            .logical(dir)?
            .into_iter()
            .filter(|(name, _)| {
                name.starts_with("content/") && (name.ends_with(".yml") || name.ends_with(".yaml"))
            })
            .map(|(_, path)| read(&path, Some("content")))
            .collect::<Result<Vec<_>>>()?;
        let mut mappings = read(&metadata.join("mappings.yml"), Some("mappings"))?;
        let native_text = extraction["parser"]
            .as_str()
            .is_some_and(|p| p.contains(";native-text/"));
        let text_format = native_text
            || extraction["parser"]
                .as_str()
                .is_some_and(|p| p.contains(";text-runs/"));
        ensure!(
            !text_format || array(&mappings["operations"])?.is_empty(),
            "text formats support existing text edits only; structural/image operations are Excel-only"
        );
        let operations = excel::parse_operations(
            array(&mappings["operations"])?,
            array(&extraction["sheets"])?,
        )?;
        let image_operations = excel::parse_image_operations(
            array(&mappings["operations"])?,
            array(&extraction["sheets"])?,
        )?;
        self.image_assets(dir, &image_operations)?;
        mappings = regenerate_mappings(&mappings, &extraction, &content_pages, &operations)?;
        let content = self.validate_content(dir, &meta, &mappings)?;
        validate_coverage(
            &extraction,
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
        let original = under(&self.root, string(&meta["source"]["path"])?)?;
        let source_current =
            original.is_file() && hash(&fs::read(original)?) == meta["source"]["sha256"];
        Ok(Inspection {
            meta,
            extraction,
            mappings,
            values,
            fingerprint: fp,
            reviewed,
            source_current,
        })
    }
}
impl Store {
    fn validate_content(
        &self,
        dir: &Path,
        meta: &Value,
        mappings: &Value,
    ) -> Result<ValidatedContent> {
        let entries = array(&mappings["entries"])?;
        let tables = mappings["tables"].as_array().cloned().unwrap_or_default();
        let mut table_keys = BTreeSet::new();
        for t in &tables {
            ensure!(
                table_keys.insert((
                    string(&t["page"])?.to_owned(),
                    string(&t["block"])?.to_owned()
                )),
                "duplicate table definition"
            );
        }
        let mut values = BTreeMap::new();
        let mut expected = BTreeSet::new();
        let mut page_ids = BTreeSet::new();
        let mut references = vec![];
        for (name, path) in self.logical(dir)? {
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
            let page = read(&path, Some("content"))?;
            ensure!(
                page["document_id"] == meta["document_id"]
                    && page["source_path"] == meta["source"]["source_path"],
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
                    let table = tables
                        .iter()
                        .find(|t| t["page"] == page_id && t["block"] == *block)
                        .context("rows require table definition")?;
                    ensure!(body.get("fields").is_none(), "table cannot contain fields");
                    let mut consumed = BTreeSet::new();
                    for e in entries.iter().filter(|e| {
                        e["page"] == page_id && e["block"] == *block && e.get("position").is_some()
                    }) {
                        let pos = &e["position"];
                        let row = string(&pos["row"])?;
                        let col = string(&pos["column"])?;
                        let value = rows
                            .get(row)
                            .and_then(|r| r.get(col))
                            .context("missing table position")?;
                        ensure!(
                            consumed.insert((row.to_owned(), col.to_owned())),
                            "duplicate table position"
                        );
                        ensure!(
                            kind(value) != "object"
                                && (value.is_null() || kind(value) == pos["type"]),
                            "table type/value mismatch"
                        );
                        ensure!(
                            array(&table["columns"])?.contains(&json!(col)),
                            "unknown column"
                        );
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
                            consumed.insert((row.into(), col.into()))
                                && rows.get(row).and_then(|v| v.get(col))
                                    == Some(
                                        &json!({"ref":{"block":r["block"],"field":r["field"]}})
                                    ),
                            "invalid table reference"
                        );
                        ensure!(
                            array(&table["columns"])?.contains(&json!(col)),
                            "unknown reference column"
                        );
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
        expected.extend(values.keys().cloned());
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
            dirs.push(self.proposal(p)?)
        } else if let Some(id) = id {
            dirs.push(self.document(id)?)
        } else {
            for id in self.index()?.as_object().unwrap().keys() {
                let d = self.document(id)?;
                if self.management(&d)?.join("document.yml").exists() {
                    dirs.push(d)
                }
            }
            for (name, path) in files(&under(&self.arp, "changes")?)? {
                if name.ends_with("/proposal.json") {
                    dirs.push(path.parent().unwrap().into());
                }
            }
        }
        let mut out = vec![];
        for dir in dirs {
            let is_proposal = dir.join("proposal.json").is_file();
            let inspected = (|| -> Result<Value> {
                let r = self.inspect(&dir, require_reviewed)?;
                let formation = self.management(&dir)?.join("formation.json");
                let recorded = if formation.exists() {
                    let (f, _) = self.formation(&dir)?;
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
                if !r.source_current {
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
                Ok(
                    json!({"document_id":r.meta["document_id"],"directory":dir,"reviewed":r.reviewed,
                    "source_current":r.source_current,"content":r.fingerprint,"pending":pending,"state":state,"blockers":blockers}),
                )
            })();
            let mut row = match inspected {
                Ok(row) => row,
                Err(e) => json!({"directory":dir,"state":"invalid","error":format!("{e:#}")}),
            };
            if is_proposal {
                row["proposal_id"] = json!(dir.file_name().unwrap().to_string_lossy());
            }
            out.push(row);
        }
        Ok(json!(out))
    }
}

struct ValidatedContent {
    values: BTreeMap<(String, String, String), Value>,
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
    let mut cells = BTreeMap::new();
    for sheet in array(&extraction["sheets"])? {
        for c in array(&sheet["cells"])? {
            ensure!(
                cells
                    .insert(
                        (
                            string(&sheet["name"])?.to_owned(),
                            string(&c["address"])?.to_owned()
                        ),
                        c
                    )
                    .is_none(),
                "duplicate extraction cell"
            );
        }
    }
    let mut origins = BTreeSet::new();
    for p in array(&extraction["pages"])? {
        for c in array(&p["chunks"])? {
            ensure!(
                origins.insert((string(&p["id"])?.to_owned(), string(&c["id"])?.to_owned())),
                "duplicate extraction origin"
            );
        }
    }
    let mut actual = BTreeSet::new();
    let mut covered_cells = BTreeSet::new();
    let mut covered_origins = BTreeSet::new();
    let mut destinations = BTreeSet::new();
    for e in entries {
        let k = key(e)?;
        ensure!(actual.insert(k.clone()), "duplicate mapping");
        ensure!(
            !array(&e["origins"])?.is_empty() || !string(&e["reason"])?.trim().is_empty(),
            "new text requires reason"
        );
        for origin in array(&e["origins"])? {
            let pair = (
                string(&origin["page"])?.to_owned(),
                string(&origin["block"])?.to_owned(),
            );
            ensure!(origins.contains(&pair), "missing origin");
            covered_origins.insert(pair);
        }
        let mapped_target = mapping_target(&e["target"])?;
        if let Some(MappingTarget::Cell { sheet, cell }) = &mapped_target {
            let pair = (sheet.clone(), cell.clone());
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
            "excluded" => ensure!(
                !string(&e["reason"])?.trim().is_empty(),
                "exclusion requires reason"
            ),
            "pending" => {}
            "cell" => {
                let value = values.get(&k).context("writeback requires field")?;
                ensure!(
                    !text_format || value.is_string(),
                    "text replacement must be a string; use an empty string to clear text"
                );
                match mapped_target.context("cell writeback requires target")? {
                    MappingTarget::Cell { sheet, cell } => {
                        let pair = (sheet.clone(), cell.clone());
                        let source = cells.get(&pair).context("missing target")?;
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
                        if let Some(mapped) = excel::map_coordinate(&sheet, &cell, operations)? {
                            ensure!(
                                destinations.insert((sheet.clone(), mapped.clone())),
                                "duplicate writeback target"
                            );
                        }
                        let (col, row) = excel::coordinate(&cell)?;
                        for sheet in array(&extraction["sheets"])?
                            .iter()
                            .filter(|s| s["name"] == pair.0)
                        {
                            for merge in array(&sheet["merges"])? {
                                let (a, b) =
                                    string(merge)?.split_once(':').context("invalid merge")?;
                                let (c1, r1) = excel::coordinate(a)?;
                                let (c2, r2) = excel::coordinate(b)?;
                                ensure!(
                                    !(c1 <= col && col <= c2 && r1 <= row && row <= r2)
                                        || (col, row) == (c1, r1),
                                    "merged cell is not top-left"
                                );
                            }
                        }
                    }
                    MappingTarget::InsertionRow {
                        sheet,
                        insertion,
                        offset,
                        column,
                    } => {
                        let mapped = excel::resolve_insertion(
                            &sheet,
                            &insertion,
                            offset,
                            Some(&column),
                            None,
                            operations,
                        )?;
                        ensure!(
                            destinations.insert((sheet, mapped)),
                            "duplicate writeback target"
                        );
                    }
                    MappingTarget::InsertionColumn {
                        sheet,
                        insertion,
                        offset,
                        row,
                    } => {
                        let mapped = excel::resolve_insertion(
                            &sheet,
                            &insertion,
                            offset,
                            None,
                            Some(row),
                            operations,
                        )?;
                        ensure!(
                            destinations.insert((sheet, mapped)),
                            "duplicate writeback target"
                        );
                    }
                }
            }
            "operation" => {
                bail!("operation writeback entries are not supported; use operations")
            }
            "formula" => bail!("formula writeback is not supported by Rust"),
            other => bail!("unsupported writeback: {other}"),
        }
    }
    ensure!(*expected == actual, "mapping coverage mismatch");
    for o in array(&mappings["omissions"])? {
        ensure!(
            !string(&o["reason"])?.trim().is_empty()
                && (!o["origin"].is_null() || !o["target"].is_null()),
            "invalid omission"
        );
        if !o["origin"].is_null() {
            let pair = (
                string(&o["origin"]["page"])?.to_owned(),
                string(&o["origin"]["block"])?.to_owned(),
            );
            ensure!(
                origins.contains(&pair) && covered_origins.insert(pair),
                "invalid/duplicate omission origin"
            );
        }
        if !o["target"].is_null() {
            let pair = target(&o["target"])?;
            ensure!(
                cells.contains_key(&pair) && covered_cells.insert(pair),
                "invalid/duplicate omission target"
            );
        }
    }
    for (sheet, cell) in cells.keys() {
        if excel::map_coordinate(sheet, cell, operations)?.is_none() {
            covered_cells.insert((sheet.clone(), cell.clone()));
        }
    }
    ensure!(
        covered_cells == cells.keys().cloned().collect() && covered_origins == origins,
        "source coverage incomplete"
    );
    Ok(())
}
