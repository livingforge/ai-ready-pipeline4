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
            .inspect_layout(dir, require_reviewed, planned, &Keep::default())?
            .0)
    }
    /// [`Store::inspect_with`] together with what `keep` asks for of what the
    /// inspection read. Otherwise the layout is dropped as soon as the pages
    /// are read: it is as large as the extraction.
    pub(super) fn inspect_layout(
        &self,
        dir: &Path,
        require_reviewed: bool,
        planned: &Planned,
        keep: &Keep<'_>,
    ) -> Result<(Inspection, Kept)> {
        let files = self
            .file_hashes_with(dir, planned)?
            .context("document not found")?;
        self.inspect_files(dir, &files, require_reviewed, planned, keep, None)
    }
    /// [`Store::inspect`] for a command that reads none of the content values,
    /// fonts and mapping entries, which are left empty, with the layout when
    /// `layout` asks for it. Files an inspection by this program found valid
    /// before, by the hash of every one of them, are not validated again, nor
    /// are their content pages read: validating a large workbook takes seconds.
    /// What is not in the files is checked as every inspection checks it: the
    /// document's place and ID, its original and its review.
    pub(super) fn inspect_records(
        &self,
        dir: &Path,
        require_reviewed: bool,
        layout: bool,
    ) -> Result<(Inspection, Option<Layout>)> {
        let files = self.file_hashes(dir)?.context("document not found")?;
        let known = self.known_inspection(&fingerprint_of(&files));
        let keep = Keep {
            layout,
            ..Default::default()
        };
        let (mut inspected, kept) =
            self.inspect_files(dir, &files, require_reviewed, &Planned::new(), &keep, known)?;
        inspected.release_content();
        Ok((inspected, kept.layout))
    }
    /// The inspection of the files whose hashes are `files`; with `known`, what
    /// an earlier inspection found of the same files, which are then not
    /// validated again.
    fn inspect_files(
        &self,
        dir: &Path,
        files: &serde_json::Map<String, Value>,
        require_reviewed: bool,
        planned: &Planned,
        keep: &Keep<'_>,
        known: Option<Known>,
    ) -> Result<(Inspection, Kept)> {
        let fp = fingerprint_of(files);
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
        let extraction_path = metadata.join("extraction.json");
        let extraction = if known.is_some() {
            crate::extraction::read_known_record(&extraction_path)?
        } else {
            crate::extraction::read_record(&extraction_path)?
        };
        let (axes, axes_digest) = identity::stored_axes(dir, planned)?;
        // The extraction takes its document and original from this document.yml
        // when it is read; the extraction key above ties it to the file.
        let layout = if known.is_none() || keep.layout {
            Some(identity::load(dir, &extraction, planned)?)
        } else {
            None
        };
        let original = self.original(string(&meta["source"]["path"])?)?;
        let source_missing = original.is_none();
        let source_current = original
            .map(|original| {
                Ok::<_, anyhow::Error>(hash(&fs::read(original)?) == meta["source"]["sha256"])
            })
            .transpose()?
            .unwrap_or(false);
        if let Some(known) = known {
            debug_assert!(keep.pages.is_empty(), "known files read no content page");
            let mut mappings = complete_mappings(read_planned(
                "mappings.yml",
                &metadata.join("mappings.yml"),
                planned,
                "mappings",
            )?);
            if mappings.get("interpretation").is_none() {
                mappings["interpretation"] =
                    crate::document_structure::empty_corrections(&extraction, ext_key);
            }
            // As with_default_entries leaves them: the entries are its own.
            mappings["entries"] = json!([]);
            let reviewed = self.reviewed(&metadata, dir, &fp)?;
            ensure!(
                !require_reviewed || reviewed,
                "content changed or not reviewed; run documents review"
            );
            return Ok((
                Inspection {
                    meta,
                    extraction,
                    mappings,
                    operated: None,
                    entries: Entries::default(),
                    content: ContentFields::default(),
                    page_files: known.page_files,
                    fingerprint: fp,
                    reviewed,
                    source_current,
                    source_missing,
                    axes,
                    axes_digest,
                    structure_sha256: known.structure_sha256,
                    pending: known.pending,
                    content_edited: known.content_edited,
                },
                Kept {
                    layout,
                    pages: BTreeMap::new(),
                },
            ));
        }
        let layout = layout.expect("loaded for an inspection validating the files");
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
            // Held typed: as JSON, the rows of a page took several times its values.
            let page = match planned.get(&name) {
                Some(Some(bytes)) => {
                    identity::runtime_grid(std::str::from_utf8(bytes)?, &layout, &extraction)?
                }
                _ => identity::runtime_grid(
                    &fs::read_to_string(&path)
                        .with_context(|| format!("read {}", path.display()))?,
                    &layout,
                    &extraction,
                )?,
            };
            page_files.insert(string(&page.fields["page_id"])?.to_owned(), name);
            content_pages.push(page);
        }
        let structure_sha256 = layout["structure_sha256"].as_str().map(str::to_owned);
        let layout = keep.layout.then_some(layout);
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
            excel::parse_operations(array(&mappings["operations"])?, &operated.sheets)?;
        let image_operations =
            excel::parse_image_operations(array(&mappings["operations"])?, &operated.sheets)?;
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
        // Validating the content takes the values out of the pages but those kept.
        let mut content = Self::validate_content(&entries, &operated, content_pages, keep.pages)?;
        let kept_pages = std::mem::take(&mut content.kept);
        validate_coverage(
            &operated,
            &mappings,
            &entries,
            &content,
            &operations,
            native_text,
            text_format,
        )?;
        let fields = content.fields;
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
        let reviewed = self.reviewed(&metadata, dir, &fp)?;
        ensure!(
            !require_reviewed || reviewed,
            "content changed or not reviewed; run documents review"
        );
        let pending = entries
            .iter(&operated)
            .filter(|e| e.writeback().is_ok_and(|w| w == "pending"))
            .count();
        let content_edited = content_edited(&extraction, &mappings, &entries, &operated, &fields)?;
        // The defaults are read from the extraction the operations leave.
        let operated = match operated {
            Cow::Owned(operated) => Some(operated),
            Cow::Borrowed(_) => None,
        };
        self.save_known_inspection(
            &fp,
            &Known {
                pending,
                content_edited,
                page_files: page_files.clone(),
                structure_sha256: structure_sha256.clone(),
            },
        );
        Ok((
            Inspection {
                meta,
                extraction,
                mappings,
                operated,
                entries,
                content: fields,
                page_files,
                fingerprint: fp,
                reviewed,
                source_current,
                source_missing,
                axes,
                axes_digest,
                structure_sha256,
                pending,
                content_edited,
            },
            Kept {
                layout,
                pages: kept_pages,
            },
        ))
    }
    /// Whether the review of the document in `dir`, whose management folder
    /// is `metadata`, is of its content as `fingerprint` names it.
    fn reviewed(&self, metadata: &Path, dir: &Path, fingerprint: &str) -> Result<bool> {
        let review = metadata.join("review.json");
        if !review.exists() {
            return Ok(false);
        }
        let r = read(&review, Some("review"))?;
        let (_, key) = self.formation(dir)?;
        ensure!(r["formation"] == key, "review/formation identity mismatch");
        Ok(r["content"] == fingerprint)
    }
    /// Where what an inspection found of the files `fingerprint` names is kept:
    /// valid for this program and the shared settings it read them with.
    fn known_path(&self, fingerprint: &str) -> Result<PathBuf> {
        let config = hash(&fs::read(under(&self.arp, "config.yml")?)?);
        let key = hash(&encoded(
            &json!({"program":crate::data::program()?,"config":config,"files":fingerprint}),
        ));
        under(&self.arp, &format!("cache/inspection/{key}.json"))
    }
    /// What an inspection by this program found of the files `fingerprint`
    /// names, when one found them valid; none when nothing readable is kept.
    fn known_inspection(&self, fingerprint: &str) -> Option<Known> {
        let bytes = fs::read(self.known_path(fingerprint).ok()?).ok()?;
        serde_json::from_slice(&bytes).ok()
    }
    /// Keeps `known` for the valid files `fingerprint` names, with the newest
    /// others. An inspection that cannot keep it still answers.
    fn save_known_inspection(&self, fingerprint: &str, known: &Known) {
        let Ok(path) = self.known_path(fingerprint) else {
            return;
        };
        if let Ok(bytes) = serde_json::to_vec(known)
            && crate::data::replace_cache(&path, &bytes).is_ok()
            && let Some(directory) = path.parent()
        {
            crate::data::keep_newest(directory, KNOWN_KEPT);
        }
    }
}

/// The inspections kept; saving another removes the oldest.
const KNOWN_KEPT: usize = 64;

/// What an inspection found of valid files besides what it reads of them
/// again: what [`Store::inspect_records`] gives without reading the content.
#[derive(serde::Serialize, serde::Deserialize)]
struct Known {
    pending: usize,
    content_edited: bool,
    page_files: BTreeMap<String, String>,
    structure_sha256: Option<String>,
}

/// The cells of an extraction by sheet name and address: the cells of the
/// sheets of each name in the order of their addresses, found by halves. A
/// large workbook has a cell for each of its values, and a map of them took
/// six times the room of this order.
struct CellIndex<'a> {
    extraction: &'a Extraction,
    /// The sheet and cell numbers of the cells of the sheets of each name.
    names: HashMap<&'a str, Vec<(u32, u32)>>,
}

impl<'a> CellIndex<'a> {
    fn new(extraction: &'a Extraction) -> Result<Self> {
        let mut names: HashMap<&str, Vec<(u32, u32)>> = HashMap::new();
        for (si, sheet) in extraction.sheets.iter().enumerate() {
            let cells = names.entry(string(&sheet["name"])?).or_default();
            cells.extend((0..sheet.cells.len() as u32).map(|ci| (si as u32, ci)));
        }
        let address = |at| indexed(extraction, at).address();
        for cells in names.values_mut() {
            cells.sort_unstable_by(|&a, &b| address(a).cmp(address(b)));
            ensure!(
                cells
                    .windows(2)
                    .all(|pair| address(pair[0]) != address(pair[1])),
                "duplicate extraction cell"
            );
        }
        Ok(Self { extraction, names })
    }

    fn cell(&self, at: (u32, u32)) -> crate::extraction::Cell<'a> {
        indexed(self.extraction, at)
    }

    fn address(&self, at: (u32, u32)) -> &'a str {
        self.cell(at).address()
    }

    /// Where the cell `address` of the sheet `sheet` is in the order of the
    /// cells of the sheets of its name.
    fn place(&self, sheet: &str, address: &str) -> Option<usize> {
        let cells = self.names.get(sheet)?;
        cells
            .binary_search_by(|&at| self.address(at).cmp(address))
            .ok()
    }

    fn get(&self, sheet: &str, address: &str) -> Option<crate::extraction::Cell<'a>> {
        let place = self.place(sheet, address)?;
        Some(self.cell(self.names[sheet][place]))
    }

    fn len(&self) -> usize {
        self.names.values().map(Vec::len).sum()
    }
}

/// The cell numbered `ci` of the sheet numbered `si` of `extraction`.
fn indexed(extraction: &Extraction, (si, ci): (u32, u32)) -> crate::extraction::Cell<'_> {
    extraction.sheets[si as usize]
        .cells
        .get(ci as usize)
        .expect("an indexed cell is a cell of its sheet")
}

/// Whether the content differs from the extraction: a row or column
/// operation is pending, or a value written back to a cell is not the one
/// the extraction holds there.
fn content_edited(
    extraction: &Extraction,
    mappings: &Value,
    entries: &Entries,
    operated: &Extraction,
    fields: &ContentFields,
) -> Result<bool> {
    if !array(&mappings["operations"])?.is_empty() {
        return Ok(true);
    }
    let originals = CellIndex::new(extraction)?;
    for (index, entry) in entries.iter(operated).enumerate() {
        if entry.writeback().is_ok_and(|w| w == "cell")
            && let Some(MappingTarget::Cell { sheet, cell }) = entry.target()?
            && let Some(Field { value, .. }) = fields.get(index)
            && originals
                .get(sheet, cell)
                .is_some_and(|old| old.value() != *value)
        {
            return Ok(true);
        }
    }
    Ok(false)
}
impl Store {
    /// The structure interpretation and its replay report: the extraction with the
    /// document's corrections replayed. None for text formats and a changed original.
    /// Replaying infers every table of the extraction, so only commands reading the
    /// structure ask for it.
    pub fn interpretation(
        &self,
        inspected: &Inspection,
    ) -> Result<Option<(crate::document_structure::Structure, Value)>> {
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
    /// What a status row reads of [`Store::interpretation`]: whether it is
    /// ready, the number of conflicts of its replay and the digest of its
    /// elements. Replaying infers and checks every table, so the summary is
    /// saved under what the replay reads, as the correction journal, the
    /// extraction, the stored rows and columns, the original and the program,
    /// and reused while they are the same, as after an edit of values or rows.
    /// A journal with image regions also reads their files and is replayed
    /// every time.
    fn structure_summary(&self, inspected: &Inspection) -> Result<Option<Value>> {
        let compute = || -> Result<Value> {
            let Some((structure, report)) = self.interpretation(inspected)? else {
                return Ok(Value::Null);
            };
            Ok(json!({"ready":report["ready"] == true,
                "conflicts":report["conflicts"].as_array().map(Vec::len).unwrap_or(0),
                "digest":elements::interpretation_digest(&structure.elements)?}))
        };
        let journal = &inspected.mappings["interpretation"];
        let summary = if !inspected.source_current
            || journal["regions"].as_array().is_some_and(|r| !r.is_empty())
        {
            compute()?
        } else {
            let inputs = json!({"document":inspected.meta["document_id"],
                "source":inspected.meta["source"],"extraction":inspected.meta["extraction"],
                "journal":journal,"axes":inspected.axes_digest});
            self.cached_result("structure-status", &inputs, 64, compute)?
        };
        Ok((!summary.is_null()).then_some(summary))
    }
    fn validate_content(
        entries: &Entries,
        extraction: &Extraction,
        pages: Vec<elements::GridPage>,
        keep: &[&str],
    ) -> Result<ValidatedContent> {
        // Positioned entries by page and block, so that each table block does not
        // scan every entry of a document with many sheets, slides or pages.
        // The entries by number: their positions are read again where they
        // are used, as a large sheet has an entry for each cell.
        let mut positioned: BTreeMap<(&str, &str), Vec<u32>> = BTreeMap::new();
        for (index, e) in entries.iter(extraction).enumerate() {
            if e.position()?.is_some() {
                positioned
                    .entry((e.page()?, e.block()?))
                    .or_default()
                    .push(u32::try_from(index)?);
            }
        }
        let mut fields = ContentFields::new(entries.len());
        // The page, block and field of each entry given a value that may
        // repeat another's.
        let mut keyed = HashSet::new();
        let mut kept_pages = BTreeMap::new();
        let mut expected = BTreeMap::new();
        let mut page_ids = BTreeSet::new();
        for mut page in pages {
            let page_id = string(&page.fields["page_id"])?.to_owned();
            ensure!(page_ids.insert(page_id.clone()), "duplicate page ID");
            // Values are moved into the fields rather than copied, but those of a
            // page kept, which the page keeps too, and each font of the page is
            // shared by the fields that have it.
            let kept = keep.contains(&page.fields["title"].as_str().unwrap_or(""));
            let font_table = match kept {
                true => page.font_table.clone(),
                false => std::mem::take(&mut page.font_table),
            };
            let font_table: Vec<Arc<Value>> = match font_table {
                Value::Array(fonts) => fonts.into_iter().map(Arc::new).collect(),
                _ => vec![],
            };
            // The notes, and the blocks of rows: those titled and those of values.
            let mut blocks: BTreeSet<String> = page.titles.keys().cloned().collect();
            blocks.extend(page.values.iter().map(|v| v.block.to_string()));
            let mut page_blocks = blocks.clone();
            page_blocks.insert("extraction-notes".into());
            expected.insert(page_id.clone(), page_blocks);
            for block in blocks {
                let start = page.values.partition_point(|v| *v.block < *block);
                let rows = &mut page.values[start..];
                let end = rows.partition_point(|v| *v.block == *block);
                let rows = &mut rows[..end];
                // Whether each value is an entry's, by its place among the
                // values in order, which is their position's.
                let mut consumed = vec![false; rows.len()];
                let block_entries = positioned
                    .get(&(page_id.as_str(), block.as_str()))
                    .map_or(&[][..], Vec::as_slice);
                for &index in block_entries {
                    let index = index as usize;
                    let e = entries.get(index, extraction);
                    let pos = e.position()?.context("table entry position missing")?;
                    let (row, col) = (pos.row, pos.column);
                    let at = rows
                        .binary_search_by(|v| (&*v.row, &*v.column).cmp(&(row, col)))
                        .ok()
                        .context("missing table position")?;
                    ensure!(
                        !std::mem::replace(&mut consumed[at], true),
                        "duplicate table position"
                    );
                    let value = &mut rows[at];
                    ensure!(
                        kind(&value.value) != "object"
                            && (value.value.is_null() || kind(&value.value) == pos.kind),
                        "table type/value mismatch"
                    );
                    ensure!(!e.field_is_null(), "table field required");
                    let font = match value.font {
                        None => None,
                        Some(index) => Some(Arc::clone(
                            font_table
                                .get(index as usize)
                                .with_context(|| format!("unknown font index {index}"))?,
                        )),
                    };
                    let field = Field {
                        value: match kept {
                            true => value.value.clone(),
                            false => value.value.take(),
                        },
                        font,
                    };
                    ensure!(
                        e.unique()
                            || keyed.insert((e.page()?, e.block()?, e.field().unwrap_or(""))),
                        "duplicate table field"
                    );
                    fields.set(index, field);
                }
                ensure!(consumed.iter().all(|&c| c), "unmapped table value");
            }
            if kept {
                kept_pages.insert(page_id, page);
            }
        }
        ensure!(!page_ids.is_empty(), "content is empty");
        Ok(ValidatedContent {
            fields,
            expected,
            kept: kept_pages,
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
        Ok(json!(self.states(&dirs, require_reviewed)?))
    }

    /// The status row of the document or proposal in `dir`: its state, and the
    /// blockers or error that keep it from the next step.
    pub(super) fn state(&self, id: &str, dir: &Path, require_reviewed: bool) -> Value {
        let inspected = self
            .inspect_records(dir, require_reviewed, false)
            .map(|(inspected, _)| inspected);
        self.state_of(id, dir, inspected)
    }

    /// [`Store::state`] from an inspection of the document or proposal in
    /// `dir` already made, as an edit makes of the content it writes.
    pub(super) fn state_of(&self, id: &str, dir: &Path, inspected: Result<Inspection>) -> Value {
        let is_proposal = dir.join("proposal.json").is_file();
        let inspected = (|| -> Result<Value> {
            let mut r = inspected?;
            let pending = r.pending;
            r.release_content();
            let summary = self.structure_summary(&r)?;
            // The tables of the element metadata are those of the
            // interpretation the layout was classified from; the replay infers
            // tables with this version of ARP.
            let structure_outdated = summary
                .as_ref()
                .map(|summary| r.structure_sha256.as_deref() != summary["digest"].as_str());
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
                "structure_ready":summary.as_ref().is_some_and(|summary| summary["ready"] == true),
                "structure_conflicts":summary.as_ref().map(|summary| summary["conflicts"].clone()),
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

/// What an inspection keeps of what it read besides its result.
#[derive(Default)]
pub(super) struct Keep<'a> {
    /// The layout, placed under the pending operations.
    pub(super) layout: bool,
    /// The content pages with these titles, the names of their sheets.
    pub(super) pages: &'a [&'a str],
}

/// What an inspection kept, as [`Keep`] asked.
pub(super) struct Kept {
    pub(super) layout: Option<Layout>,
    /// The content pages by page ID, as the writer reads them.
    pub(super) pages: BTreeMap<String, elements::GridPage>,
}

struct ValidatedContent {
    fields: ContentFields,
    /// The blocks of each page, by page ID, each the field of an entry
    /// without a field. Every value is expected too.
    expected: BTreeMap<String, BTreeSet<String>>,
    /// The pages kept, whole, by page ID.
    kept: BTreeMap<String, elements::GridPage>,
}

fn validate_coverage(
    extraction: &Extraction,
    mappings: &Value,
    entries: &Entries,
    content: &ValidatedContent,
    operations: &[excel::StructuralOperation],
    native_text: bool,
    text_format: bool,
) -> Result<()> {
    let fields = &content.fields;
    let expected = |page: &str, block: &str| {
        content
            .expected
            .get(page)
            .is_some_and(|blocks| blocks.contains(block))
    };
    // Every cell is placed through the pending operations; each row and
    // column is mapped once.
    let coordinates = excel::CoordinateMap::new(operations);
    // Every entry looks up its target, and a large workbook has an entry per cell.
    let cells = CellIndex::new(extraction)?;
    let sheet_names: HashSet<&str> = extraction
        .sheets
        .iter()
        .map(|sheet| string(&sheet["name"]))
        .collect::<Result<_>>()?;
    // The entries with a value, those of them whose field is no expected
    // block's, the expected blocks entries name, and the entries that name
    // neither, which fail below. Only the page, block and field of an entry
    // that may repeat another's are kept: a large workbook has an entry per
    // cell.
    let mut named = 0;
    let mut named_fields = 0;
    let mut actual = HashSet::new();
    let mut strays = HashSet::new();
    let mut keyed = HashSet::new();
    // New table column names by sheet and cell, checked for duplicates below.
    let mut header_edits = HashMap::new();
    // Whether each cell is covered, by its place in `cells`.
    let mut covered: HashMap<&str, Vec<bool>> = cells
        .names
        .iter()
        .map(|(name, list)| (*name, vec![false; list.len()]))
        .collect();
    let mut cover = |sheet: &str, place: usize| {
        let bit = &mut covered
            .get_mut(sheet)
            .expect("a covered cell's sheet is indexed")[place];
        !std::mem::replace(bit, true)
    };
    let mut destinations = HashSet::new();
    // Written positions are checked against the merges as they stand after
    // the operations: an insertion inside a merge grows it over the new cells.
    let mut merges = BTreeMap::new();
    let mut computed = BTreeMap::new();
    for sheet in &extraction.sheets {
        merges.insert(
            string(&sheet["name"])?.to_owned(),
            excel::merges_after(&sheet.fields, operations)?,
        );
        computed.insert(
            string(&sheet["name"])?,
            excel::ComputedRanges::new(&sheet.fields)?,
        );
    }
    let merges = merges
        .iter()
        .map(|(name, merges)| Ok((name.as_str(), excel::Merges::new(merges)?)))
        .collect::<Result<BTreeMap<_, _>>>()?;
    for (index, e) in entries.iter(extraction).enumerate() {
        let (page, block, field) = (e.page()?, e.block()?, e.field().unwrap_or(""));
        let value = fields.get(index).map(|field| &field.value);
        ensure!(!e.reason()?.trim().is_empty(), "new text requires reason");
        let mapped_target = e.target()?;
        if let Some(MappingTarget::Cell { sheet, cell }) = mapped_target {
            if let Some(place) = cells.place(sheet, cell) {
                if native_text && let Some(value) = value {
                    ensure!(
                        *value == cells.get(sheet, cell).expect("a placed cell").value(),
                        "text content is a read-only derived view; edit the original and re-import with the same document ID"
                    );
                }
                cover(sheet, place);
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
                    (&mapped_target, value)
                {
                    let source = cells.get(sheet, cell).context("missing Excel target")?;
                    let original = match source.formula() {
                        Some(formula) if e.position()?.is_some_and(|p| p.column == "formula") => {
                            json!(format!("={formula}"))
                        }
                        _ => source.value().to_json(),
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
                let value = value.context("writeback requires field")?;
                ensure!(
                    !text_format || value.is_string(),
                    "text replacement must be a string; use an empty string to clear text"
                );
                let destination = match mapped_target.context("cell writeback requires target")? {
                    MappingTarget::Cell { sheet, cell } => {
                        // A cell the original leaves empty holds no value or type.
                        let source = cells.get(sheet, cell);
                        if let Some(source) = source {
                            ensure!(
                                !matches!(source.kind(), CellKind::Formula | CellKind::Error),
                                "formula/error cell cannot be overwritten"
                            );
                            ensure!(
                                value.is_null()
                                    || source.kind().as_str() == kind(value)
                                    || source.kind() == CellKind::Null,
                                "Excel target type mismatch"
                            );
                        }
                        let original = source.map_or(CellValue::Null, |source| source.value());
                        if !text_format && *value != original {
                            let (column, row) = excel::coordinate(cell)?;
                            let book_sheet = &extraction
                                .sheets
                                .iter()
                                .find(|s| s["name"] == sheet)
                                .context("missing Excel target sheet")?
                                .fields;
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
                        let mapped = coordinates.map_point(sheet, cell)?
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
                        Some((sheet, excel::coordinate(&mapped)?))
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
                        Some((sheet, excel::coordinate(&mapped)?))
                    }
                    MappingTarget::Shape { .. } => {
                        bail!("the text of a shape is written back with shape writeback")
                    }
                };
                if let Some((sheet, cell)) = destination {
                    merges
                        .get(sheet)
                        .context("missing Excel target sheet")?
                        .ensure_not_hidden_at(sheet, cell)?;
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
                let value = value.context("writeback requires field")?;
                let Some(MappingTarget::Shape { sheet, shape }) = mapped_target else {
                    bail!("shape writeback requires a shape target");
                };
                let old = extraction
                    .sheets
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
                let value = value.context("writeback requires field")?;
                let Some(MappingTarget::Cell { sheet, cell }) = mapped_target else {
                    bail!("formula writeback requires a cell target");
                };
                let source = cells.get(sheet, cell).context("missing Excel target")?;
                let formula = source
                    .formula()
                    .filter(|_| source.kind() == CellKind::Formula);
                let Some(formula) = formula else {
                    bail!(
                        "{sheet}!{cell} holds no formula; formula writeback replaces an existing formula"
                    );
                };
                if value.as_str() != Some(&format!("={formula}")) {
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
                        coordinates.map(sheet, cell)?.is_some(),
                        "{sheet}!{cell} is deleted by a delete_rows/delete_columns operation; restore its formula"
                    );
                }
            }
            other => bail!("unsupported writeback: {other}"),
        }
        let block_field = field.is_empty() && expected(page, block);
        let unique = (e.unique() || keyed.insert((page, block, field)))
            && if value.is_some() {
                named += 1;
                named_fields += usize::from(!block_field);
                true
            } else if block_field {
                actual.insert((page, block))
            } else {
                strays.insert((page, block, field))
            };
        ensure!(unique, "duplicate mapping");
    }
    if !header_edits.is_empty() {
        for sheet in &extraction.sheets {
            let name = string(&sheet["name"])?;
            excel::ensure_unique_table_headers(&sheet.fields, |cell| {
                header_edits
                    .get(&(name, cell))
                    .map(|value| (*value).clone())
                    .or_else(|| cells.get(name, cell).map(|c| c.value().to_json()))
            })?;
        }
    }
    // A field that is expected too is named as a field.
    let expected_count = content.expected.values().map(BTreeSet::len).sum::<usize>() + named_fields;
    ensure!(
        strays.is_empty() && named + actual.len() == expected_count,
        "mapping coverage mismatch"
    );
    for o in array(&mappings["omissions"])? {
        ensure!(!string(&o["reason"])?.trim().is_empty(), "invalid omission");
        let (sheet, cell) = target(&o["target"])?;
        ensure!(
            cells
                .place(sheet, cell)
                .is_some_and(|place| cover(sheet, place)),
            "invalid/duplicate omission target"
        );
    }
    for (sheet, list) in &cells.names {
        for (place, &at) in list.iter().enumerate() {
            if coordinates.map(sheet, cells.address(at))?.is_none() {
                cover(sheet, place);
            }
        }
    }
    ensure!(
        covered.values().flatten().all(|&covered| covered),
        "source coverage incomplete"
    );
    debug_assert_eq!(covered.values().map(Vec::len).sum::<usize>(), cells.len());
    Ok(())
}

/// Requires a content page for each page the slide operations leave (the
/// `operated` extraction) and none for a page they delete.
fn ensure_slide_pages(
    extraction: &Extraction,
    operated: &Extraction,
    pages: &[elements::GridPage],
) -> Result<()> {
    let kept: BTreeMap<&str, &str> = operated
        .sheets
        .iter()
        .map(|sheet| Ok((string(&sheet["page"])?, string(&sheet["name"])?)))
        .collect::<Result<_>>()?;
    let present: BTreeSet<&str> = pages
        .iter()
        .map(|page| string(&page.fields["page_id"]))
        .collect::<Result<_>>()?;
    for (index, sheet) in extraction.sheets.iter().enumerate() {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A cell is found by its sheet's name and its address, the cells of
    /// sheets of one name together, and a cell twice under one name fails.
    #[test]
    fn the_cell_index_finds_cells_by_sheet_and_address() {
        let extraction = crate::extraction::sample(json!({"sheets":[
            {"name":"A","cells":[{"address":"B2","value":1},{"address":"A10","value":2}]},
            {"name":"B","cells":[{"address":"A1","value":3}]},
            {"name":"A","cells":[{"address":"C1","value":4}]}]}));
        let index = CellIndex::new(&extraction).unwrap();
        assert_eq!(index.len(), 4);
        let value = |sheet, address| index.get(sheet, address).map(|c| c.value().to_json());
        assert_eq!(value("A", "B2"), Some(json!(1)));
        assert_eq!(value("A", "A10"), Some(json!(2)));
        assert_eq!(value("A", "C1"), Some(json!(4)));
        assert_eq!(value("B", "A1"), Some(json!(3)));
        assert_eq!((value("B", "B2"), value("C", "A1")), (None, None));
        let twice = crate::extraction::sample(json!({"sheets":[
            {"name":"A","cells":[{"address":"A1"}]},{"name":"A","cells":[{"address":"A1"}]}]}));
        assert!(
            CellIndex::new(&twice)
                .err()
                .is_some_and(|e| e.to_string() == "duplicate extraction cell")
        );
    }
}
