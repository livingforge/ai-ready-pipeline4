use super::*;

impl Store {
    pub fn export(&self, id: &str, output: Option<&Path>, engine: &str) -> Result<Value> {
        let dir = self.document(id)?;
        let result = self.inspect(&dir, true)?;
        ensure!(
            result.source_current,
            "source changed/missing; re-import before export"
        );
        let source = under(&self.root, string(&result.meta["source"]["path"])?)?;
        let book = Source::open(&source)?;
        ensure!(
            book.parser() != "native-text/1",
            "text documents use direct editing; edit the original and re-import with the same document ID (export/apply is not supported)"
        );
        ensure!(
            engine == "auto" || engine == book.engine(),
            "unsupported export engine for source format; use auto or {}",
            book.engine()
        );
        let operations = excel::parse_operations(
            array(&result.mappings["operations"])?,
            array(&result.extraction["sheets"])?,
        )?;
        // Report unsupported row/column edits in the plan, before any output is written.
        if let Source::Excel(workbook) = &book {
            workbook.ensure_structural_edits_supported(&operations)?;
        }
        let image_operations = excel::parse_image_operations(
            array(&result.mappings["operations"])?,
            array(&result.extraction["sheets"])?,
        )?;
        let image_assets = self.image_assets(&dir, &image_operations)?;
        let mut changes = vec![];
        let mut excluded = vec![];
        let mut pending = vec![];
        let mut deleted_cells = vec![];
        for sheet in array(&result.extraction["sheets"])? {
            let sheet_name = string(&sheet["name"])?;
            for cell in array(&sheet["cells"])? {
                let address = string(&cell["address"])?;
                if excel::map_coordinate(sheet_name, address, &operations)?.is_none() {
                    deleted_cells.push(json!({"sheet":sheet_name,"cell":address}));
                }
            }
        }
        for e in array(&result.mappings["entries"])? {
            match string(&e["writeback"])? {
                "excluded" => excluded.push(e.clone()),
                "pending" => pending.push(e.clone()),
                "cell" => {
                    let new = &result.values[&key(e)?];
                    match mapping_target(&e["target"])?.context("cell writeback requires target")? {
                        MappingTarget::Cell { sheet, cell } => {
                            let old = array(&result.extraction["sheets"])?
                                .iter()
                                .find(|s| s["name"] == sheet)
                                .and_then(|s| {
                                    s["cells"].as_array()?.iter().find(|c| c["address"] == cell)
                                })
                                .context("missing cell")?;
                            if let Some(mapped) = excel::map_coordinate(&sheet, &cell, &operations)?
                                && old["value"] != *new
                            {
                                changes.push(json!({"sheet":sheet,"cell":mapped,"source_cell":cell,"before":old["value"],"after":new,"field":e["field"]}));
                            }
                        }
                        MappingTarget::InsertionRow {
                            sheet,
                            insertion,
                            offset,
                            column,
                        } => {
                            if !new.is_null() {
                                let cell = excel::resolve_insertion(
                                    &sheet,
                                    &insertion,
                                    offset,
                                    Some(&column),
                                    None,
                                    &operations,
                                )?;
                                changes.push(json!({"sheet":sheet,"cell":cell,"before":Value::Null,"after":new,"field":e["field"],"inserted":true}));
                            }
                        }
                        MappingTarget::InsertionColumn {
                            sheet,
                            insertion,
                            offset,
                            row,
                        } => {
                            if !new.is_null() {
                                let cell = excel::resolve_insertion(
                                    &sheet,
                                    &insertion,
                                    offset,
                                    None,
                                    Some(row),
                                    &operations,
                                )?;
                                changes.push(json!({"sheet":sheet,"cell":cell,"before":Value::Null,"after":new,"field":e["field"],"inserted":true}));
                            }
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
        let mut report = json!({"schema_version":"1","document_id":id,"content":result.fingerprint,"source_sha256":result.meta["source"]["sha256"],"changes":changes,"unreflected":pending,"excluded":excluded,"omissions":result.mappings["omissions"],"operations":result.mappings["operations"],"deleted_cells":deleted_cells,"engine":book.engine(),"complete":pending.is_empty(),"written":false});
        let Some(output) = output else {
            return Ok(report);
        };
        ensure!(pending.is_empty(), "unresolved writeback mappings");
        let output = if output.is_absolute() {
            output.to_owned()
        } else {
            std::env::current_dir()?.join(output)
        };
        let relative = output
            .strip_prefix(&self.root)
            .context("output must be inside project .arp/cache/export")?
            .to_string_lossy()
            .replace('\\', "/");
        let output = under(&self.root, &relative)?;
        ensure!(
            output.starts_with(self.arp.join("cache/export"))
                && output.extension() == source.extension(),
            "output must be a new file matching the source format inside .arp/cache/export"
        );
        let report_path = output.with_extension(format!(
            "{}.report.json",
            output.extension().unwrap().to_string_lossy()
        ));
        ensure!(
            !output.exists() && !report_path.exists(),
            "output/report already exists"
        );
        fs::create_dir_all(output.parent().unwrap())?;
        let stage = tempfile::tempdir_in(output.parent().unwrap())?;
        let staged = stage.path().join(format!(
            "result.{}",
            source.extension().unwrap().to_string_lossy()
        ));
        let info = book.patch(
            &staged,
            array(&result.mappings["operations"])?,
            &changes,
            &image_assets,
        )?;
        for (k, v) in info.as_object().unwrap() {
            report[k] = v.clone()
        }
        report["written"] = json!(true);
        report["output_sha256"] = json!(hash(&fs::read(&staged)?));
        let staged_report = stage.path().join("report.json");
        write(&staged_report, &report)?;
        ensure!(
            self.fingerprint(&dir)?.as_deref() == Some(&result.fingerprint)
                && hash(&fs::read(under(
                    &self.root,
                    string(&result.meta["source"]["path"])?
                )?)?)
                    == result.meta["source"]["sha256"],
            "source/document changed during export"
        );
        fs::hard_link(&staged, &output)?;
        if let Err(error) = fs::hard_link(&staged_report, &report_path) {
            if fs::read(&output).ok() == fs::read(&staged).ok() {
                fs::remove_file(&output)?
            }
            return Err(error.into());
        }
        Ok(report)
    }
}
