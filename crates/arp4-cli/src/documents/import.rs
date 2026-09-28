use super::*;
use crate::document_structure::Carrier;

/// The longest project-relative path of a document record. Windows limits a full path
/// to 259 characters unless Git and every application enable long paths, so this
/// leaves room for the folder a project is checked out in.
const MAX_RECORD_PATH: usize = 200;
/// Records written after import, which the proposal does not contain yet.
const LATER_RECORDS: [&str; 3] = ["formation.json", "review.json", "prompt.txt"];

impl Store {
    /// The original named on the command line and its document ID: an absolute path,
    /// a path from the project root, or a path from the sources folder.
    pub fn resolve(&self, source: &Path) -> Result<(PathBuf, String)> {
        let sources = self.sources()?;
        let path = if source.is_absolute() {
            source.to_owned()
        } else {
            let direct = self.root.join(source);
            if direct.exists() {
                direct
            } else {
                sources.join(source)
            }
        };
        let path = dunce::canonicalize(&path)
            .with_context(|| format!("original not found: {}", source.display()))?;
        let sources = dunce::canonicalize(&sources).context("sources folder missing")?;
        let id = path
            .strip_prefix(&sources)
            .with_context(|| {
                format!(
                    "place the original under the sources folder {}",
                    sources.display()
                )
            })?
            .to_string_lossy()
            .replace('\\', "/");
        Ok((path, id))
    }
    /// The project-relative path of the original a document ID names.
    pub fn source_path(&self, id: &str) -> Result<String> {
        let config = read(
            &under(&self.root, ".arp/config.yml")?,
            Some("project-config"),
        )?;
        Ok(format!("{}/{id}", string(&config["sources"])?))
    }

    /// Import every original below a folder of the sources. Originals whose proposal or
    /// adopted document already has their hash are left alone, and documents whose
    /// original is gone are listed for `documents remove`. An original that cannot be
    /// imported is listed under `failed` and the others are still imported.
    pub fn import_folder(&self, source: &Path, force: bool) -> Result<Value> {
        let (dir, scope) = self.resolve(source)?;
        ensure!(dir.is_dir(), "import a folder or use a file: {scope}");
        let dir = dir.as_path();
        let formats = crate::document_source::input_formats();
        let mut imported = vec![];
        let mut unchanged = vec![];
        let mut pending_proposals = vec![];
        let mut skipped = vec![];
        let mut failed = vec![];
        let mut pending = vec![dir.to_owned()];
        let mut originals = vec![];
        while let Some(dir) = pending.pop() {
            for entry in fs::read_dir(&dir)? {
                let path = entry?.path();
                let name = path.file_name().unwrap().to_string_lossy();
                // Hidden files and Office lock files are not documents.
                if name.starts_with('.') || name.starts_with("~$") {
                    continue;
                }
                let meta = fs::symlink_metadata(&path)?;
                ensure!(!is_link(&meta), "links are not allowed: {}", path.display());
                if meta.is_dir() {
                    pending.push(path);
                } else if meta.is_file() {
                    originals.push(path);
                }
            }
        }
        originals.sort();
        for path in originals {
            let (path, id) = self.resolve(&path)?;
            let ext = extension(&path);
            if let Some(replacement) = crate::document_source::binary_office_replacement(&ext) {
                skipped.push(json!({"document_id":id,"reason":format!("binary Office format; save as {replacement}")}));
                continue;
            }
            if !formats.contains(&ext.as_str()) {
                skipped.push(json!({"document_id":id,"reason":"unsupported format"}));
                continue;
            }
            match self.import_changed(&path, &id, force) {
                Ok(Some(_)) => imported.push(json!(id)),
                Ok(None) => {
                    if self.proposal_path(&id)?.exists() {
                        pending_proposals.push(json!(id));
                    }
                    unchanged.push(json!(id));
                }
                Err(error) => {
                    failed.push(json!({"document_id":id,"reason":format!("{error:#}")}));
                }
            }
        }
        Ok(
            json!({"imported":imported,"unchanged":unchanged,"pending_proposals":pending_proposals,"skipped":skipped,"failed":failed,"missing":self.missing(Some(scope.as_str()).filter(|s| !s.is_empty()))?}),
        )
    }

    /// Import an original unless its proposal or adopted document already has its hash.
    /// Returns whether it was imported.
    fn proposal_path(&self, id: &str) -> Result<PathBuf> {
        self.entry("changes", id)
    }

    fn import_changed(&self, path: &Path, id: &str, force: bool) -> Result<Option<Value>> {
        let raw = fs::read(path)?;
        let sha = hash(&raw);
        let proposal = self.proposal_path(id)?.join("document.yml");
        let adopted = self.document(id)?.join("document.yml");
        let recorded = if proposal.is_file() {
            proposal
        } else {
            adopted
        };
        if !force
            && recorded.is_file()
            && read(&recorded, Some("document"))?["source"]["sha256"] == sha
        {
            return Ok(None);
        }
        self.import_original_with_bytes(path, id, None, Some(raw))
            .map(Some)
    }

    /// Documents and proposals at or below `scope` whose original no longer exists.
    pub fn missing(&self, scope: Option<&str>) -> Result<Vec<String>> {
        let mut ids: BTreeSet<String> = self.ids("documents", scope)?.into_iter().collect();
        ids.extend(self.ids("changes", scope)?);
        let mut missing = vec![];
        for id in ids {
            if self.original(&self.source_path(&id)?)?.is_none() {
                missing.push(id);
            }
        }
        Ok(missing)
    }

    /// Remove the documents and proposals at or below `scope` whose originals were
    /// moved, renamed or deleted. A moved original is imported again as a new document.
    pub fn remove(&self, scope: &str) -> Result<Value> {
        document_id(scope)?;
        let mut ids: BTreeSet<String> = self.ids("documents", Some(scope))?.into_iter().collect();
        ids.extend(self.ids("changes", Some(scope))?);
        ensure!(!ids.is_empty(), "no document at or below {scope}");
        let missing: BTreeSet<_> = self.missing(Some(scope))?.into_iter().collect();
        let present: Vec<_> = ids.iter().filter(|id| !missing.contains(*id)).collect();
        ensure!(
            present.is_empty(),
            "originals still exist; move or delete them first: {}",
            present
                .iter()
                .map(|id| id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
        for area in ["documents", "changes"] {
            let base = under(&self.arp, area)?;
            for id in &ids {
                let dir = under(&base, id)?;
                if dir.exists() {
                    fs::remove_dir_all(&dir)?;
                    remove_empty_directories(dir.parent().unwrap(), &self.arp);
                }
            }
        }
        Ok(json!({"removed":ids}))
    }

    /// Discard one pending proposal without changing its adopted document or original.
    pub fn discard(&self, id: &str) -> Result<Value> {
        let proposal = self.proposal(id)?;
        fs::remove_dir_all(&proposal)?;
        remove_empty_directories(proposal.parent().unwrap(), &self.arp);
        Ok(json!({"state":"discarded","document_id":id}))
    }

    /// Import one original as the proposal for its document ID, replacing an earlier
    /// proposal of the same document.
    pub fn import(&self, source: &Path) -> Result<Value> {
        self.import_with_force(source, false)
    }

    pub fn import_with_force(&self, source: &Path, force: bool) -> Result<Value> {
        let (source, id) = self.resolve(source)?;
        ensure!(source.is_file(), "import a file or use a folder: {id}");
        match self.import_changed(&source, &id, force)? {
            Some(result) => Ok(result),
            None => Ok(
                json!({"document_id":id,"state":"unchanged","pending_proposal":self.proposal_path(&id)?.exists()}),
            ),
        }
    }

    /// Imports `source` as the proposal of `id`. `carry`, from `apply`, moves the
    /// document's interpretation onto the original it wrote; without it, the
    /// interpretation moves along the rows and columns aligned between the
    /// adopted version and this one.
    pub(super) fn import_original(
        &self,
        source: &Path,
        id: &str,
        carry: Option<&Carry>,
    ) -> Result<Value> {
        self.import_original_with_bytes(source, id, carry, None)
    }

    fn import_original_with_bytes(
        &self,
        source: &Path,
        id: &str,
        carry: Option<&Carry>,
        raw: Option<Vec<u8>>,
    ) -> Result<Value> {
        document_id(id)?;
        let ext = extension(source);
        if let Some(replacement) = crate::document_source::binary_office_replacement(&ext) {
            bail!(
                ".{ext} is a binary Office format; save it in Office as {replacement} and import that copy"
            );
        }
        let formats = crate::document_source::input_formats();
        ensure!(
            formats.contains(&ext.as_str()),
            "import supports {}",
            formats
                .iter()
                .map(|f| format!(".{f}"))
                .collect::<Vec<_>>()
                .join("/")
        );
        let current = self.document(id)?;
        let destination = self.entry("changes", id)?;
        let source_path = self.source_path(id)?;
        ensure!(
            self.original(&source_path)?.as_deref() == Some(source),
            "original does not match document ID {id}"
        );
        let book = match raw {
            Some(raw) => Source::from_bytes(source, raw)?,
            None => Source::open(source)?,
        };
        let sha = hash(book.raw());
        let info = json!({"path":source_path,"sha256":sha});
        let note = book.note();
        let mut assets = BTreeMap::new();
        if let Source::Excel(workbook) = &book {
            for sheet in &workbook.sheets {
                for drawing in array(&sheet["drawings"])? {
                    if let Some(part) = drawing["image"]["part"].as_str() {
                        assets.insert(
                            string(&drawing["image"]["asset"])?.to_owned(),
                            workbook.parts[part].clone(),
                        );
                    }
                }
            }
        }
        assets.extend(book.word_images()?);
        let asset_info: Vec<_> = assets
            .iter()
            .map(|(name, bytes)| json!({"path":name,"sha256":hash(bytes),"ocr":crate::ocr::recognize(name, bytes)}))
            .collect();
        let mut extraction = json!({"schema_version":"1","document_id":id,"source":info,"parser":format!("arp4-rust/{};{};ocr=auto-images",env!("CARGO_PKG_VERSION"),book.parser()),"sheets":[],"findings":[{"level":"warning","code":"R001","message":&note}],"assets":asset_info});
        let field_codes = book.word_field_codes()?;
        if !field_codes.is_empty() {
            extraction["field_codes"] = json!(field_codes);
        }
        let print_settings = book.print_settings()?;
        if !print_settings.is_empty() {
            extraction["print_settings"] = json!(print_settings);
        }
        let chart_parts = book.chart_parts()?;
        if !chart_parts.is_empty() {
            extraction["chart_parts"] = json!(chart_parts);
        }
        let opaque_parts = book.opaque_parts();
        if !opaque_parts.is_empty() {
            extraction["opaque_parts"] = json!(opaque_parts);
        }
        // Moved in rather than serialized again: the sheets hold every cell.
        extraction["sheets"] = Value::Array(book.sheets().into_owned());
        validate("extraction", &extraction)?;
        // Encoded once: the record is these bytes and its key is their hash.
        let extraction_bytes = encoded(&extraction);
        let extraction_hash = hash(&extraction_bytes);
        let same_extraction = current.join("document.yml").exists()
            && read(&current.join("document.yml"), Some("document"))?["extraction"]
                == extraction_hash;
        let previous = if current.join("mappings.yml").exists() {
            let previous = read(&current.join("mappings.yml"), Some("mappings"))?;
            let journal = previous["interpretation"].clone();
            crate::document_structure::validate_corrections(&journal)?;
            (journal["document"] == id && journal["source_path"] == source_path).then_some(journal)
        } else {
            None
        };
        // The interpretation of the version before is carried onto this one
        // with the operations apply wrote or, for an original edited outside
        // ARP, those aligning the two versions. When carrying fails, the
        // corrections are kept as recorded and replay matches them by address.
        let aligned = match (carry, &previous) {
            (None, Some(journal)) if !same_extraction && interpreted(journal) => {
                Some(self.aligned(&current, journal, &extraction))
            }
            _ => None,
        };
        let interpretation =
            previous.unwrap_or_else(|| crate::document_structure::empty_corrections(&extraction));
        let attempt = match (carry, &aligned) {
            (Some(carry), _) => Some(Ok(carry)),
            (None, Some(aligned)) => Some(aligned.as_ref()),
            (None, None) => None,
        };
        let (interpretation, carried) = match attempt {
            Some(Ok(carry)) => match crate::document_structure::carry_corrections(
                &self.root,
                &carry.structure,
                &carry.extraction,
                &extraction,
                &carry.operations,
                carry.carrier,
            ) {
                Ok((journal, record)) => (journal, record),
                Err(error) => (interpretation, not_carried(carry.carrier, &error)),
            },
            Some(Err(error)) => (interpretation, not_carried(Carrier::Alignment, error)),
            None => (interpretation, Value::Null),
        };
        let stage = Stage::new_in(&under(&self.arp, "work")?, &self.arp)?;
        let ready = stage.path().join("ready");
        fs::create_dir(&ready)?;
        if !assets.is_empty() {
            fs::create_dir(ready.join("assets"))?;
            for (name, bytes) in &assets {
                fs::write(ready.join("assets").join(name), bytes)?;
            }
        }
        let base = self.fingerprint(&current)?;
        write(
            &ready.join("document.yml"),
            &json!({"schema_version":"1","document_id":id,"source":info,"extraction":extraction_hash}),
        )?;
        write(
            &ready.join("proposal.json"),
            &json!({"schema_version":"1","document_id":id,"base":base}),
        )?;
        let mut tables = vec![];
        for (i, sheet) in array(&extraction["sheets"])?.iter().enumerate() {
            let page = format!("sheet-{}", i + 1);
            let mut rows = json!({});
            let mut formulas = json!({});
            let mut columns = BTreeSet::new();
            for c in array(&sheet["cells"])? {
                let address = string(&c["address"])?;
                let (_, row) = excel::coordinate(address)?;
                let column = address.trim_end_matches(|c: char| c.is_ascii_digit());
                columns.insert(column.to_owned());
                rows[&format!("r{row}")][column] = c["value"].clone();
                if c["type"] == "formula" {
                    let f = string(&c["id"])?;
                    formulas[f]["formula"] = json!(format!("={}", string(&c["formula"])?));
                }
            }
            let mut shapes = json!({});
            for (key, drawing) in shape_texts(sheet)? {
                shapes[key]["text"] = drawing["text"].clone();
            }
            let mut blocks = json!({"extraction-notes":{"title":"未抽出・注意事項","text":&note}});
            for (block, title, body, cols) in [
                (
                    "table-1",
                    "本文",
                    rows,
                    columns.into_iter().collect::<Vec<_>>(),
                ),
                ("formulas", "数式原文", formulas, vec!["formula".into()]),
                ("shapes", "図形の文字", shapes, vec!["text".into()]),
            ] {
                if body.as_object().is_some_and(|o| !o.is_empty()) {
                    blocks[block] = json!({"title":title,"rows":body});
                    tables.push(json!({"page":page,"block":block,"columns":cols,"references":[]}));
                }
            }
            write(
                &ready
                    .join("content")
                    .join(excel::filename(string(&sheet["name"])?)),
                &json!({"schema_version":"1","document_id":id,"page_id":page,"source_path":source_path,"title":sheet["name"],"blocks":blocks}),
            )?;
        }
        // The per-cell entries follow from the extraction; inspection derives them.
        write(
            &ready.join("mappings.yml"),
            &json!({"schema_version":"1","entries":[],"tables":tables,"omissions":[],"operations":[],"interpretation":interpretation}),
        )?;
        replace(&ready.join("extraction.json"), &extraction_bytes)?;
        let names = files(&ready)?
            .into_keys()
            .chain(LATER_RECORDS.map(String::from));
        for name in names {
            let record = format!(".arp/documents/{id}/{name}");
            ensure!(
                record.encode_utf16().count() <= MAX_RECORD_PATH,
                "document path too long for Windows checkouts ({} > {MAX_RECORD_PATH} characters): {record}; shorten the folder or file names of the original",
                record.encode_utf16().count()
            );
        }
        ensure!(
            fs::read(source)? == book.raw(),
            "source changed during import"
        );
        // Not inspected again here: record inspects the proposal before it can be adopted.
        fs::create_dir_all(destination.parent().unwrap())?;
        let previous = stage.path().join("previous");
        if destination.exists() {
            fs::rename(&destination, &previous)?;
        }
        if let Err(error) = fs::rename(&ready, &destination) {
            if previous.exists() {
                fs::rename(&previous, &destination)?;
            } else {
                remove_empty_directories(destination.parent().unwrap(), &self.arp);
            }
            return Err(error.into());
        }
        let mut result =
            json!({"document_id":id,"proposal":destination,"findings":extraction["findings"]});
        if !carried.is_null() {
            result["structure"] = carried;
        }
        Ok(result)
    }
}

/// What the import of a new version of an original carries the interpretation
/// from: the interpretation and extraction of the version before and the
/// row/column operations between the two, which `apply` wrote or alignment found.
pub(super) struct Carry {
    pub(super) structure: Value,
    pub(super) extraction: Value,
    pub(super) operations: Vec<excel::StructuralOperation>,
    pub(super) carrier: Carrier,
}

impl Store {
    /// The carry of `journal`, the interpretation of the document in `current`,
    /// onto `extraction`, the extraction of its original as edited outside ARP.
    fn aligned(&self, current: &Path, journal: &Value, extraction: &Value) -> Result<Carry> {
        let meta = read(&current.join("document.yml"), Some("document"))?;
        let key = string(&meta["extraction"])?;
        let before = read(&current.join("extraction.json"), Some("extraction"))?;
        ensure!(
            hash(&encoded(&before)) == key,
            "extraction integrity failure"
        );
        let (structure, _) = crate::document_structure::replay_earlier_corrections(
            &self.root, journal, &before, key,
        )?;
        let operations = crate::document_structure::align_operations(&before, extraction)?;
        Ok(Carry {
            structure,
            extraction: before,
            operations,
            carrier: Carrier::Alignment,
        })
    }
}

/// Whether a correction journal holds anything to carry: a correction, a
/// reading, a cell state, an image region or a review.
fn interpreted(journal: &Value) -> bool {
    ["elements", "readings", "cell_states", "visuals", "regions"]
        .iter()
        .any(|key| {
            journal[*key]
                .as_array()
                .is_some_and(|items| !items.is_empty())
        })
        || journal["review"]["status"] != "pending"
}

fn not_carried(carrier: Carrier, error: &anyhow::Error) -> Value {
    json!({"by":carrier.name(),"review":"pending","reasons":[format!("not carried: {error:#}")]})
}

fn extension(path: &Path) -> String {
    path.extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase()
}

#[cfg(test)]
mod efficiency_tests {
    use super::*;

    #[test]
    #[ignore = "manual performance measurement"]
    fn benchmark_missing_membership() {
        use std::time::Instant;

        let ids: Vec<_> = (0..5000).map(|i| format!("document-{i:05}")).collect();
        let missing: Vec<_> = ids.iter().step_by(2).cloned().collect();
        let start = Instant::now();
        let old = ids.iter().filter(|id| !missing.contains(id)).count();
        let old_ms = start.elapsed().as_millis();
        let start = Instant::now();
        let missing: BTreeSet<_> = missing.into_iter().collect();
        let new = ids.iter().filter(|id| !missing.contains(*id)).count();
        let new_ms = start.elapsed().as_millis();
        assert_eq!(old, new);
        eprintln!(
            "missing_membership old_ms={old_ms} new_ms={new_ms} ids={}",
            ids.len()
        );
    }
}
