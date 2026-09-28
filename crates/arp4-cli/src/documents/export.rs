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
        // Inserted slides are written like pages of the original they copy.
        let operated = operated_extraction(&result.extraction, &result.mappings["operations"])?;
        let operations = excel::parse_operations(
            array(&result.mappings["operations"])?,
            array(&operated["sheets"])?,
        )?;
        let mut operations_by_sheet: BTreeMap<&str, Vec<excel::StructuralOperation>> =
            BTreeMap::new();
        for operation in &operations {
            operations_by_sheet
                .entry(&operation.sheet)
                .or_default()
                .push(operation.clone());
        }
        let sheet_operations = |sheet: &str| {
            operations_by_sheet
                .get(sheet)
                .map(Vec::as_slice)
                .unwrap_or(&[])
        };
        // Report unsupported row/column and slide edits in the plan, before any
        // output is written.
        book.ensure_row_edits_supported(&operations, array(&result.mappings["operations"])?)?;
        book.ensure_slide_edits_supported(array(&result.mappings["operations"])?)?;
        let image_operations = excel::parse_image_operations(
            array(&result.mappings["operations"])?,
            array(&operated["sheets"])?,
        )?;
        let image_assets = self.image_assets(&dir, &image_operations)?;
        let mut changes = vec![];
        let mut formulas = vec![];
        let mut excluded = vec![];
        let mut pending = vec![];
        let mut deleted_cells = vec![];
        // Each cell by sheet and address, so that an entry does not scan its sheet.
        let mut cells = BTreeMap::new();
        for sheet in array(&operated["sheets"])? {
            let sheet_name = string(&sheet["name"])?;
            let own_operations = sheet_operations(sheet_name);
            let has_deletions = own_operations.iter().any(|operation| {
                matches!(
                    operation.kind,
                    excel::OperationKind::DeleteRows | excel::OperationKind::DeleteColumns
                )
            });
            let mut deleted_rows = BTreeMap::new();
            let mut deleted_columns = BTreeMap::new();
            for cell in array(&sheet["cells"])? {
                let address = string(&cell["address"])?;
                cells.entry((sheet_name, address)).or_insert(cell);
                if !has_deletions {
                    continue;
                }
                let (column, row) = excel::coordinate(address)?;
                let row_deleted = match deleted_rows.get(&row) {
                    Some(deleted) => *deleted,
                    None => {
                        let deleted = excel::axis_deleted(row, own_operations, true)?;
                        deleted_rows.insert(row, deleted);
                        deleted
                    }
                };
                let column_deleted = match deleted_columns.get(&column) {
                    Some(deleted) => *deleted,
                    None => {
                        let deleted = excel::axis_deleted(column, own_operations, false)?;
                        deleted_columns.insert(column, deleted);
                        deleted
                    }
                };
                if row_deleted || column_deleted {
                    deleted_cells.push(json!({"sheet":sheet_name,"cell":address}));
                }
            }
        }
        // The text of each shape by sheet and extraction ID.
        let mut shapes = BTreeMap::new();
        for sheet in array(&result.extraction["sheets"])? {
            for drawing in sheet["drawings"].as_array().into_iter().flatten() {
                shapes.insert(
                    (string(&sheet["name"])?, string(&drawing["id"])?),
                    &drawing["text"],
                );
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
                            let old = cells.get(&(sheet, cell)).context("missing cell")?;
                            if old["value"] != *new
                                && let Some(mapped) =
                                    excel::map_coordinate(sheet, cell, sheet_operations(sheet))?
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
                                    sheet,
                                    insertion,
                                    offset,
                                    Some(column),
                                    None,
                                    &operations,
                                )?;
                                changes.push(json!({"sheet":sheet,"cell":cell,"before":Value::Null,"after":new,"field":e["field"],"inserted":true,"insertion":insertion,"offset":offset,"column":column}));
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
                                    sheet,
                                    insertion,
                                    offset,
                                    None,
                                    Some(row),
                                    &operations,
                                )?;
                                changes.push(json!({"sheet":sheet,"cell":cell,"before":Value::Null,"after":new,"field":e["field"],"inserted":true}));
                            }
                        }
                        MappingTarget::Shape { .. } => {
                            bail!("the text of a shape is written back with shape writeback")
                        }
                    }
                }
                "shape" => {
                    let new = &result.values[&key(e)?];
                    let Some(MappingTarget::Shape { sheet, shape }) = mapping_target(&e["target"])?
                    else {
                        bail!("shape writeback requires a shape target");
                    };
                    let old = shapes
                        .get(&(sheet, shape))
                        .with_context(|| format!("shape {shape} on {sheet} is missing"))?;
                    if **old != *new {
                        changes.push(json!({"sheet":sheet,"shape":shape,"before":old,"after":new,"field":e["field"]}));
                    }
                }
                "operation" => {
                    bail!("operation writeback entries are not supported; use operations")
                }
                "formula" => {
                    let new = &result.values[&key(e)?];
                    let Some(MappingTarget::Cell { sheet, cell }) = mapping_target(&e["target"])?
                    else {
                        bail!("formula writeback requires a cell target");
                    };
                    let old = cells.get(&(sheet, cell)).context("missing cell")?;
                    let before = format!("={}", string(&old["formula"])?);
                    if *new != before {
                        let mapped = excel::map_coordinate(sheet, cell, sheet_operations(sheet))?;
                        formulas.push(json!({"sheet":sheet,"cell":cell,"final_cell":mapped,"before":before,"after":new,"field":e["field"]}));
                    }
                }
                other => bail!("unsupported writeback: {other}"),
            }
        }
        let mut report = json!({"schema_version":"1","document_id":id,"content":result.fingerprint,"source_sha256":result.meta["source"]["sha256"],"changes":changes,"formula_changes":formulas,"unreflected":pending,"excluded":excluded,"omissions":result.mappings["omissions"],"operations":result.mappings["operations"],"deleted_cells":deleted_cells,"engine":book.engine(),"complete":pending.is_empty(),"written":false});
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
        let stage = Stage::new_in(output.parent().unwrap(), &self.arp)?;
        let staged = stage.path().join(format!(
            "result.{}",
            source.extension().unwrap().to_string_lossy()
        ));
        let info = book.patch(
            &staged,
            array(&result.mappings["operations"])?,
            &changes,
            &formulas,
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

#[cfg(test)]
mod efficiency_tests {
    use super::*;

    #[test]
    #[ignore = "manual performance measurement"]
    fn benchmark_deleted_cells() {
        use std::time::Instant;

        let operations: Vec<_> = (0..24)
            .map(|index| excel::StructuralOperation {
                id: format!("op-{index}"),
                sheet: "Sheet1".into(),
                kind: if index % 3 == 0 {
                    excel::OperationKind::DeleteRows
                } else {
                    excel::OperationKind::InsertRows
                },
                at: 400 + index * 4,
                count: 1,
                style_from: None,
            })
            .collect();
        let addresses: Vec<_> = (1..=1000)
            .flat_map(|row| {
                (1..=50)
                    .map(move |column| format!("{}{}", excel::column_name(column).unwrap(), row))
            })
            .collect();

        let start = Instant::now();
        let old: Vec<_> = addresses
            .iter()
            .filter(|address| {
                excel::map_coordinate("Sheet1", address, &operations)
                    .unwrap()
                    .is_none()
            })
            .collect();
        let old_ms = start.elapsed().as_millis();

        let start = Instant::now();
        let mut rows = BTreeMap::new();
        let mut columns = BTreeMap::new();
        let new: Vec<_> = addresses
            .iter()
            .filter(|address| {
                let (column, row) = excel::coordinate(address).unwrap();
                let row_deleted = *rows
                    .entry(row)
                    .or_insert_with(|| excel::axis_deleted(row, &operations, true).unwrap());
                let column_deleted = *columns
                    .entry(column)
                    .or_insert_with(|| excel::axis_deleted(column, &operations, false).unwrap());
                row_deleted || column_deleted
            })
            .collect();
        let new_ms = start.elapsed().as_millis();
        assert_eq!(old, new);
        eprintln!(
            "deleted_cells old_ms={old_ms} new_ms={new_ms} cells={} operations={}",
            addresses.len(),
            operations.len()
        );
    }
}
