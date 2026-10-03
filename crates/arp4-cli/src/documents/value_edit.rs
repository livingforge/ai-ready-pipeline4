use super::*;

impl Store {
    /// Edit existing cells or text runs, including Excel formulas and table labels.
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
            let cell = string(&edit["cell"])?;
            ensure!(targets.insert((sheet, cell)), "duplicate value edit target");
            let entry = array(&inspected.mappings["entries"])?
                .iter()
                .find(|entry| {
                    entry["target"]["sheet"] == sheet
                        && entry["target"]["cell"] == cell
                        && matches!(entry["writeback"].as_str(), Some("cell" | "formula"))
                })
                .context("target is not an editable value or formula cell")?;
            let current = inspected
                .values
                .get(&key(entry)?)
                .context("missing current value")?;
            ensure!(
                *current == edit["before"],
                "value edit expected old value differs at {sheet}!{cell}"
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
            let field = if entry["position"].is_object() {
                &mut page["blocks"][block]["rows"][string(&entry["position"]["row"])?]
                    [string(&entry["position"]["column"])?]
            } else {
                &mut page["blocks"][block]["fields"][string(&entry["field"])?]
            };
            *field = edit["after"].clone();
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
        let mut changes = vec![];
        let mut formulas = vec![];
        for entry in array(&checked.mappings["entries"])? {
            if !matches!(entry["writeback"].as_str(), Some("cell" | "formula")) {
                continue;
            }
            let Some(MappingTarget::Cell { sheet, cell }) = mapping_target(&entry["target"])?
            else {
                continue;
            };
            let after = &checked.values[&key(entry)?];
            let source_cell = cells[&(sheet, cell)];
            match entry["writeback"].as_str() {
                Some("cell") if *after != source_cell["value"] => {
                    changes.push(json!({"sheet":sheet,"cell":cell,"before":source_cell["value"],"after":after}));
                }
                Some("formula") => {
                    let before = format!("={}", string(&source_cell["formula"])?);
                    if *after != before {
                        formulas
                            .push(json!({"sheet":sheet,"cell":cell,"before":before,"after":after}));
                    }
                }
                _ => {}
            }
        }
        // Exercise the format's writer protections before any managed files change.
        let stage = Stage::new_in(&under(&self.arp, "cache/export")?, &self.arp)?;
        let preview = stage
            .path()
            .join(source.file_name().context("missing filename")?);
        let verification = book.patch(&preview, &[], &changes, &formulas, &BTreeMap::new())?;
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
