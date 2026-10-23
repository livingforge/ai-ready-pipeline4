use super::*;
use crate::document_source::encryption::{self, Protection};
use crate::document_structure::Carrier;
use crate::rights_management::Unprotected;
use std::collections::HashSet;

/// The longest project-relative path of a document record. Windows limits a full path
/// to 259 characters unless Git and every application enable long paths, so this
/// leaves room for the folder a project is checked out in.
const MAX_RECORD_PATH: usize = 200;
/// Records written after import, which the proposal does not contain yet.
const LATER_RECORDS: [&str; 3] = ["formation.json", "review.json", "prompt.txt"];

/// How import removes the encryption of an Office original, which a project
/// keeps unencrypted: a password-encrypted original is decrypted with the
/// first of `passwords` that opens it, and rights management protection (IRM
/// and sensitivity labels) is removed by desktop Office.
pub struct Unprotection {
    passwords: Vec<String>,
    rights_management: fn(&Path, &Path) -> Result<Unprotected>,
}

impl Default for Unprotection {
    fn default() -> Self {
        Self::new(vec![])
    }
}

impl Unprotection {
    pub fn new(passwords: Vec<String>) -> Self {
        Self::with_rights_management(passwords, crate::rights_management::remove)
    }

    /// Removes rights management protection with `remove`, which is given the
    /// original and a folder for its files, in place of desktop Office.
    pub fn with_rights_management(
        passwords: Vec<String>,
        remove: fn(&Path, &Path) -> Result<Unprotected>,
    ) -> Self {
        Self {
            passwords,
            rights_management: remove,
        }
    }
}

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
    pub fn import_folder(
        &self,
        source: &Path,
        force: bool,
        unprotection: &Unprotection,
    ) -> Result<Value> {
        let (dir, scope) = self.resolve(source)?;
        ensure!(dir.is_dir(), "import a folder or use a file: {scope}");
        let dir = dir.as_path();
        let formats = crate::document_source::input_formats();
        let mut imported = vec![];
        let mut unchanged = vec![];
        let mut pending_proposals = vec![];
        let mut skipped = vec![];
        let mut failed = vec![];
        let mut unprotected = vec![];
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
            match self.import_changed(&path, &id, force, unprotection) {
                Ok(Some(result)) => {
                    if let Some(record) = result.get("unprotected") {
                        let mut record = record.clone();
                        record["document_id"] = json!(id);
                        unprotected.push(record);
                    }
                    imported.push(json!(id));
                }
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
            json!({"imported":imported,"unchanged":unchanged,"pending_proposals":pending_proposals,"skipped":skipped,"failed":failed,"unprotected":unprotected,"missing":self.missing(Some(scope.as_str()).filter(|s| !s.is_empty()))?}),
        )
    }

    /// Import an original unless its proposal or adopted document already has its hash.
    /// Returns whether it was imported.
    fn proposal_path(&self, id: &str) -> Result<PathBuf> {
        self.entry("changes", id)
    }

    /// An encrypted original is saved over without its encryption first; the
    /// result then records the protection removed under `unprotected`.
    fn import_changed(
        &self,
        path: &Path,
        id: &str,
        force: bool,
        unprotection: &Unprotection,
    ) -> Result<Option<Value>> {
        let raw = fs::read(path)?;
        let (raw, unprotected) = match encryption::protection(&raw) {
            Some(protection) => {
                let (plain, record) = self.unprotect(path, &raw, protection, unprotection)?;
                (plain, Some(record))
            }
            None => (raw, None),
        };
        let sha = hash(&raw);
        let proposal = self.proposal_path(id)?.join("document.yml");
        let adopted = self.document(id)?.join("document.yml");
        let recorded = if proposal.is_file() {
            proposal
        } else {
            adopted
        };
        // Records an earlier version of ARP wrote are not read: with --force the
        // document is made anew from the original, without them. Only --force
        // reads every record to find out.
        let earlier = if force {
            self.unreadable_records(id)?
        } else {
            None
        };
        if !force
            && unprotected.is_none()
            && recorded.is_file()
            && read(&recorded, Some("document"))?["source"]["sha256"] == sha
        {
            return Ok(None);
        }
        let Some(record) = unprotected else {
            let result = self.import_original_with_bytes(path, id, None, Some(raw), earlier)?;
            if self.reproduces_adopted(id)? {
                self.discard(id)?;
                return Ok(None);
            }
            return Ok(Some(result));
        };
        let mut result = self
            .import_original_with_bytes(path, id, None, Some(raw), earlier)
            .context("the encryption was removed from the original, but its import failed")?;
        result["unprotected"] = record;
        Ok(Some(result))
    }

    /// Why the records of the adopted document or the proposal of `id` cannot
    /// be read by this version, when they cannot: an earlier version wrote them.
    fn unreadable_records(&self, id: &str) -> Result<Option<String>> {
        for dir in [self.document(id)?, self.proposal_path(id)?] {
            if !dir.join("document.yml").is_file() {
                continue;
            }
            let readable = || -> Result<()> {
                read(&dir.join("document.yml"), Some("document"))?;
                read(&dir.join("layout.yml"), Some("document-layout"))?;
                read_mappings(&dir.join("mappings.yml"))?;
                crate::extraction::read_record(&dir.join("extraction.json"))?;
                if dir.join("content").is_dir() {
                    for page in fs::read_dir(dir.join("content"))? {
                        read(&page?.path(), Some("content"))?;
                    }
                }
                Ok(())
            };
            if let Err(error) = readable() {
                return Ok(Some(format!("{error:#}")));
            }
        }
        Ok(None)
    }

    /// Whether the proposal of `id` holds what its adopted document holds, so
    /// there is nothing to record or adopt.
    fn reproduces_adopted(&self, id: &str) -> Result<bool> {
        let current = self.document(id)?;
        Ok(current.join("document.yml").is_file()
            && self.fingerprint(&current)? == self.fingerprint(&self.proposal_path(id)?)?)
    }

    /// Removes the encryption of the original at `path`, whose bytes are `raw`,
    /// and saves the decrypted package over it. Returns the package and the
    /// record of the protection removed.
    fn unprotect(
        &self,
        path: &Path,
        raw: &[u8],
        protection: Protection,
        unprotection: &Unprotection,
    ) -> Result<(Vec<u8>, Value)> {
        let (plain, mut record) = match protection {
            Protection::Password => (
                encryption::decrypt(raw, &unprotection.passwords)?,
                json!({}),
            ),
            Protection::RightsManagement => {
                let stage = Stage::new_in(&under(&self.arp, "work")?, &self.arp)?;
                let unprotected = (unprotection.rights_management)(path, stage.path())?;
                (unprotected.bytes, json!({"removed":unprotected.removed}))
            }
        };
        record["protection"] = json!(protection.name());
        // Only a package ARP opens replaces the original.
        crate::document_source::office_package(&plain)
            .context("the decrypted original is not an Open XML package")?;
        ensure!(
            fs::read(path)? == raw,
            "source changed while its encryption was removed"
        );
        replace(path, &plain)?;
        Ok((plain, record))
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
        self.import_with(source, force, &Unprotection::default())
    }

    /// Imports one original as `import_with_force` does, removing its
    /// encryption with `unprotection`.
    pub fn import_with(
        &self,
        source: &Path,
        force: bool,
        unprotection: &Unprotection,
    ) -> Result<Value> {
        let (source, id) = self.resolve(source)?;
        ensure!(source.is_file(), "import a file or use a folder: {id}");
        match self.import_changed(&source, &id, force, unprotection)? {
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
        self.import_original_with_bytes(source, id, carry, None, None)
    }

    /// [`Store::import_original`] of the original's bytes `raw`, when read
    /// already. `earlier` says why the records of the document are not read,
    /// when an earlier version wrote them: the document is then made anew.
    fn import_original_with_bytes(
        &self,
        source: &Path,
        id: &str,
        carry: Option<&Carry>,
        raw: Option<Vec<u8>>,
        earlier: Option<String>,
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
        let mut extraction = json!({"schema_version":"2","document_id":id,"source":info,"parser":format!("arp4-rust/{};{};ocr=auto-images",env!("CARGO_PKG_VERSION"),book.parser()),"findings":[{"level":"warning","code":"R001","message":&note}],"assets":asset_info});
        if matches!(&book, Source::Excel(workbook) if workbook.date1904) {
            extraction["date1904"] = json!(true);
        }
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
        let layouts = book.slide_layouts();
        if !layouts.is_empty() {
            extraction["slide_layouts"] = json!(layouts);
        }
        let opaque_parts = book.opaque_parts();
        if !opaque_parts.is_empty() {
            extraction["opaque_parts"] = json!(opaque_parts);
        }
        // The parsed original is not needed past its sheets; only its bytes
        // are compared again at the end.
        let (sheets, raw) = book.into_extraction()?;
        let extraction = Extraction {
            fields: extraction,
            sheets,
        };
        let stage = Stage::new_in(&under(&self.arp, "work")?, &self.arp)?;
        let ready = stage.path().join("ready");
        fs::create_dir(&ready)?;
        // Encoded once, in the stored form: the record is these bytes and its
        // key is their hash. Written at once, as they are as large as the
        // extraction and nothing reads them again.
        let extraction_hash = {
            let bytes = crate::extraction::stored(&extraction)?;
            replace(&ready.join("extraction.json"), &bytes)?;
            hash(&bytes)
        };
        let mut layout = if identity::is_excel(&extraction) && earlier.is_none() {
            let identity_base = if carry.is_none()
                && destination.join("document.yml").is_file()
                && read(&destination.join("document.yml"), Some("document"))?["source"]["sha256"]
                    == sha
                && current.join("document.yml").is_file()
                && read(&current.join("document.yml"), Some("document"))?["source"]["sha256"] != sha
            {
                &destination
            } else {
                &current
            };
            let mut layout = identity::reimport(identity_base, &extraction, carry.is_some())?;
            // The element metadata of the version before is let go: classify
            // gives the groups and the pages encoded below give the metadata
            // again, and it has an entry per cell.
            for sheet in layout.sheets.values_mut() {
                sheet.metadata = vec![];
            }
            layout
        } else {
            identity::initial(&extraction)?
        };
        let same_extraction = earlier.is_none()
            && current.join("document.yml").exists()
            && read(&current.join("document.yml"), Some("document"))?["extraction"]
                == extraction_hash;
        let previous = if earlier.is_none() && current.join("mappings.yml").exists() {
            let previous = read_mappings(&current.join("mappings.yml"))?;
            // A journal recording nothing is not stored.
            let journal = previous["interpretation"].clone();
            if journal.is_null() {
                None
            } else {
                crate::document_structure::validate_corrections(&journal)?;
                Some(journal)
            }
        } else {
            None
        };
        // The interpretation of the version before is carried onto this one
        // with the operations apply wrote or, for an original edited outside
        // ARP, those aligning the two versions. An original saved again with
        // the same content aligns with no operation: its interpretation is
        // carried to it as to one apply wrote with no cell changed, rather
        // than left naming the original it was made for. When carrying fails,
        // the corrections are kept as recorded and replay matches them by
        // address.
        let aligned = match (carry, &previous) {
            (None, Some(journal))
                if (!same_extraction && interpreted(journal))
                    || (same_extraction
                        && !records_nothing(journal)
                        && journal["source_sha256"] != info["sha256"]) =>
            {
                Some(self.aligned(&current, journal, &extraction))
            }
            _ => None,
        };
        // A journal holding nothing is made again for this version, so it does
        // not keep naming the original and extraction it was first made for.
        let interpretation = match previous {
            Some(journal) if !records_nothing(&journal) => journal,
            _ => crate::document_structure::empty_corrections(&extraction, &extraction_hash),
        };
        let attempt = match (carry, &aligned) {
            (Some(carry), _) => Some(Ok(carry)),
            (None, Some(aligned)) => Some(aligned.as_ref()),
            (None, None) => None,
        };
        // The journal names cells by the row and column identities of this
        // version.
        let axes = identity::axes(&layout)?;
        let (interpretation, carried) = match attempt {
            Some(Ok(carry)) => match crate::document_structure::carry_corrections(
                &self.root,
                &carry.structure,
                &carry.extraction,
                &extraction,
                &carry.operations,
                carry.carrier,
                &axes,
            ) {
                Ok((journal, record)) => (journal, record),
                Err(error) => (interpretation, not_carried(carry.carrier, &error)),
            },
            Some(Err(error)) => (interpretation, not_carried(Carrier::Alignment, error)),
            None => (interpretation, Value::Null),
        };
        let mut classified = {
            let (structure, _) = crate::document_structure::replay_verified_corrections(
                &self.root,
                &interpretation,
                &extraction,
                &extraction_hash,
                &axes,
            )?;
            let classified = elements::classify(&mut layout, &extraction, &structure.elements)?;
            elements::geometry(&mut layout, &extraction, &[])?;
            classified
        };
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
        for (i, sheet) in extraction.sheets.iter().enumerate() {
            let page = format!("sheet-{}", i + 1);
            let note = json!(note);
            let grid = initial_grid(sheet, &page, &note, &layout)?;
            debug_assert_eq!(
                grid,
                elements::GridPage::from_value(identity::grid(
                    initial_page(sheet, &page, &note)?,
                    &layout,
                    false
                )?)?,
                "the grid import makes of {page} is the one of its initial page"
            );
            let body = elements::encode_grid(grid, &mut layout, classified.remove(&page))?;
            elements::write_page(
                &ready
                    .join("content")
                    .join(excel::filename(string(&sheet["name"])?)),
                &body,
                &layout,
            )?;
        }
        replace(
            &ready.join("layout.yml"),
            &identity::stored(layout, &extraction)?,
        )?;
        // The per-cell entries follow from the extraction; inspection derives them.
        let mappings_path = ready.join("mappings.yml");
        replace(
            &mappings_path,
            &stored_mappings(
                &mappings_path,
                &json!({"schema_version":"1","interpretation":interpretation}),
            )?,
        )?;
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
        ensure!(fs::read(source)? == raw, "source changed during import");
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
        if let Some(reason) = earlier {
            result["discarded_records"] = json!(reason);
        }
        Ok(result)
    }
}

/// The grid page of `page` that import makes of the extraction `sheet`: its
/// values, formulas, shape texts and fonts, with the extraction's `note`.
pub(super) fn initial_page(sheet: &Sheet, page: &str, note: &Value) -> Result<Value> {
    let mut rows = json!({});
    let mut formulas = json!({});
    let mut fonts = json!({});
    let mut table = elements::Fonts::default();
    for c in &sheet.cells {
        let address = &*c.address;
        let (_, row) = excel::coordinate(address)?;
        let column = address.trim_end_matches(|c: char| c.is_ascii_digit());
        rows[&format!("r{row}")][column] = c.value.clone();
        let font = crate::extraction::font(sheet, c)?;
        if c.kind == CellKind::Formula {
            let f = &*c.id;
            let formula = c
                .formula
                .as_deref()
                .context("formula cell without formula")?;
            formulas[f]["formula"] = json!(format!("={formula}"));
            if !font.is_null() {
                fonts["formulas"][f]["formula"] = table.add(font);
            }
        } else if !font.is_null() {
            fonts["table-1"][&format!("r{row}")][column] = table.add(font);
        }
    }
    let mut shapes = json!({});
    for (key, drawing) in shape_texts(&sheet.fields)? {
        shapes[&key]["text"] = drawing["text"].clone();
        if let Some(font) = drawing.get("font") {
            fonts["shapes"][&key]["text"] = table.add(font);
        }
    }
    let mut blocks = json!({"extraction-notes":{"title":"未抽出・注意事項","text":note}});
    for (block, title, body) in [
        ("table-1", "本文", rows),
        ("formulas", "数式原文", formulas),
        ("shapes", "図形の文字", shapes),
    ] {
        if body.as_object().is_some_and(|o| !o.is_empty()) {
            // Moved in: `json!` would copy the values of every cell.
            blocks[block] = json!({"title":title});
            blocks[block]["rows"] = body;
        }
    }
    let mut page =
        json!({"schema_version":identity::GRID_VERSION,"page_id":page,"title":sheet["name"]});
    page["blocks"] = blocks;
    page["fonts"] = fonts;
    page[elements::FONT_TABLE] = Value::Array(table.list);
    Ok(page)
}

/// [`initial_page`] as [`identity::grid`] places it in `layout` for
/// [`elements::encode_grid`], made without the page as JSON, which held an
/// object for each row of values and of fonts.
pub(super) fn initial_grid(
    sheet: &Sheet,
    page: &str,
    note: &Value,
    layout: &Layout,
) -> Result<elements::GridPage> {
    let places = identity::grid_places(layout, page, &sheet["name"], false)?;
    let name = places.sheet["name"].as_str().unwrap_or("sheet");
    // Formula caches are extraction metadata, not editable body values.
    let formula_cells: HashSet<(&str, &str)> = places.sheet["formulas"]
        .as_object()
        .context("layout formulas missing")?
        .values()
        .map(|target| Ok((string(&target["row"])?, string(&target["column"])?)))
        .collect::<Result<_>>()?;
    let (body, formulas, shapes): (Arc<str>, Arc<str>, Arc<str>) =
        ("table-1".into(), "formulas".into(), "shapes".into());
    let mut table = elements::Fonts::default();
    let mut font = |font: &Value| {
        if font.is_null() {
            Value::Null
        } else {
            table.add(font)
        }
    };
    let (mut values, mut formula_values) = (vec![], vec![]);
    for c in &sheet.cells {
        let address = &*c.address;
        let (_, row) = excel::coordinate(address)?;
        let column = address.trim_end_matches(|c: char| c.is_ascii_digit());
        let key = format!("r{row}");
        let target_row = places.rows.get(&key).with_context(|| {
            format!("unknown row identity: deleted by operations or unmapped ({name} {key})")
        })?;
        let target_column = places
            .columns
            .get(column)
            .context("unknown column identity")?;
        let cell_font = crate::extraction::font(sheet, c)?;
        let mut body_font = Value::Null;
        if c.kind == CellKind::Formula {
            let formula = c
                .formula
                .as_deref()
                .context("formula cell without formula")?;
            let field = places.formulas.get(&*c.id).with_context(|| {
                format!(
                    "{name}!{} is deleted or has unknown formula identity",
                    c.id.rsplit('-').next().unwrap_or(&c.id)
                )
            })?;
            formula_values.push(elements::GridValue {
                block: formulas.clone(),
                row: field.clone(),
                column: "formula".into(),
                value: json!(format!("={formula}")),
                font: font(cell_font),
            });
        } else {
            body_font = font(cell_font);
        }
        if !formula_cells.contains(&(target_row.as_str(), target_column.as_str())) {
            values.push(elements::GridValue {
                block: body.clone(),
                row: target_row.clone(),
                column: target_column.clone(),
                value: c.value.clone(),
                font: body_font,
            });
        }
    }
    let shape_values: Vec<_> = shape_texts(&sheet.fields)?
        .into_iter()
        .map(|(key, drawing)| elements::GridValue {
            block: shapes.clone(),
            row: key,
            column: "text".into(),
            value: drawing["text"].clone(),
            font: drawing
                .get("font")
                .map_or(Value::Null, |font| table.add(font)),
        })
        .collect();
    let mut titles = serde_json::Map::new();
    for (block, title, present) in [
        ("table-1", "本文", !sheet.cells.is_empty()),
        ("formulas", "数式原文", !formula_values.is_empty()),
        ("shapes", "図形の文字", !shape_values.is_empty()),
    ] {
        if present {
            titles.insert(block.into(), json!(title));
        }
    }
    // In the order of their block, row and column, as a page lists them.
    let mut ordered = vec![];
    for mut block in [formula_values, shape_values, values] {
        block.sort_by(|a, b| (&a.row, &a.column).cmp(&(&b.row, &b.column)));
        ordered.extend(block);
    }
    Ok(elements::GridPage {
        fields: json!({"schema_version":identity::GRID_VERSION,"page_id":page,"title":sheet["name"]}),
        notes: json!({"title":"未抽出・注意事項","text":note}),
        titles,
        values: ordered,
        font_table: Value::Array(table.list),
    })
}

/// What the import of a new version of an original carries the interpretation
/// from: the interpretation and extraction of the version before and the
/// row/column operations between the two, which `apply` wrote or alignment found.
pub(super) struct Carry {
    pub(super) structure: crate::document_structure::Structure,
    pub(super) extraction: Extraction,
    pub(super) operations: Vec<excel::StructuralOperation>,
    pub(super) carrier: Carrier,
}

impl Store {
    /// The carry of `journal`, the interpretation of the document in `current`,
    /// onto `extraction`, the extraction of its original as edited outside ARP.
    fn aligned(&self, current: &Path, journal: &Value, extraction: &Extraction) -> Result<Carry> {
        let meta = read(&current.join("document.yml"), Some("document"))?;
        let key = string(&meta["extraction"])?;
        let before = crate::extraction::read_record(&current.join("extraction.json"))?;
        ensure!(
            crate::extraction::hash(&before)? == key,
            "extraction integrity failure"
        );
        let (structure, _) = crate::document_structure::replay_earlier_corrections(
            &self.root,
            journal,
            &before,
            key,
            &identity::stored_axes(current, &Planned::new())?.0,
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

/// Whether a journal records nothing a reader decided, not even an order.
pub(super) fn records_nothing(journal: &Value) -> bool {
    !interpreted(journal)
        && journal["element_order"]
            .as_array()
            .is_some_and(Vec::is_empty)
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
