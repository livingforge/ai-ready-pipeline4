use super::*;

impl Workbook {
    /// Row/column edits move cells and every reference ARP can rewrite
    /// (formulas on all sheets, defined names, sheet ranges, tables, pivots,
    /// charts and DrawingML anchors). Reject workbooks whose other parts hold
    /// cell positions that would silently point at the wrong cells, and
    /// changes Excel itself refuses (cutting through tables or pivot tables).
    pub fn ensure_structural_edits_supported(
        &self,
        operations: &[StructuralOperation],
    ) -> Result<()> {
        if operations.is_empty() {
            return Ok(());
        }
        let edited: BTreeSet<&str> = operations.iter().map(|o| o.sheet.as_str()).collect();
        let mut scratch = BTreeMap::new();
        let moves = Moves::new(
            operations,
            removed_table_columns(&self.parts, &self.sheets, operations)?,
        );
        relocate_tables(
            &self.parts,
            &self.sheets,
            &moves,
            &mut BTreeMap::new(),
            &mut BTreeMap::new(),
            &mut scratch,
        )?;
        relocate_pivots(&self.parts, &self.sheets, operations, &mut scratch)?;
        let mut reasons = vec![];
        if self
            .parts
            .keys()
            .any(|name| name.to_ascii_lowercase().ends_with("vbaproject.bin"))
        {
            reasons.push("VBA project (cell addresses in macro code cannot be updated)".to_owned());
        }
        for sheet in &self.skipped_sheets {
            let kind = string(&sheet["kind"])?;
            if matches!(kind, "macrosheet" | "dialogsheet") {
                reasons.push(format!("{kind} '{}'", string(&sheet["name"])?));
            }
        }
        for sheet in self
            .sheets
            .iter()
            .filter(|s| s["name"].as_str().is_some_and(|name| edited.contains(name)))
        {
            let doc = xml(&self.parts[string(&sheet["part"])?])?;
            let objects: Vec<_> = ["legacyDrawing", "controls", "oleObjects"]
                .into_iter()
                .filter(|tag| child(doc.root_element(), tag).is_some())
                .collect();
            if !objects.is_empty() {
                reasons.push(format!(
                    "form controls, comments or embedded objects on '{}' ({})",
                    string(&sheet["name"])?,
                    objects.join(", ")
                ));
            }
        }
        ensure!(
            reasons.is_empty(),
            "row/column insert/delete is not supported for this workbook because ARP cannot move cell positions held by: {}. Make row/column changes in Excel and re-import; cell value edits remain supported",
            reasons.join("; ")
        );
        Ok(())
    }

    pub fn patch(&self, destination: &Path, changes: &[Value]) -> Result<Value> {
        self.patch_with_operations(destination, &[], changes)
    }

    fn patch_scalar(&self, destination: &Path, changes: &[Value]) -> Result<Value> {
        ensure!(
            !self
                .parts
                .keys()
                .any(|n| n.to_lowercase().starts_with("_xmlsignatures/")),
            "signed Excel cannot be modified"
        );
        let mut updates: BTreeMap<String, BTreeMap<String, Value>> = BTreeMap::new();
        for change in changes {
            let sheet = self
                .sheets
                .iter()
                .find(|s| s["name"] == change["sheet"])
                .context("missing writeback sheet")?;
            let cell = string(&change["cell"])?;
            let found = array(&sheet["cells"])?
                .iter()
                .find(|c| c["address"] == cell)
                .context("missing writeback cell")?;
            ensure!(
                found["type"] != "formula" && found["type"] != "error",
                "formula/error cells cannot be overwritten"
            );
            ensure!(
                kind(&change["after"]) != "object",
                "scalar writeback required"
            );
            ensure!(
                change["after"].is_null() || found["type"] == kind(&change["after"]),
                "Excel target type mismatch"
            );
            let (col, row) = coordinate(cell)?;
            for merge in array(&sheet["merges"])? {
                let (a, b) = string(merge)?.split_once(':').unwrap();
                let (c1, r1) = coordinate(a)?;
                let (c2, r2) = coordinate(b)?;
                ensure!(
                    !(c1 <= col && col <= c2 && r1 <= row && row <= r2) || (col, row) == (c1, r1),
                    "merged cell is not top-left"
                );
            }
            ensure!(
                updates
                    .entry(string(&sheet["part"])?.into())
                    .or_default()
                    .insert(cell.into(), change["after"].clone())
                    .is_none(),
                "duplicate writeback target"
            );
        }
        let recalc = !changes.is_empty()
            && self.sheets.iter().any(|s| {
                s["cells"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|c| c["type"] == "formula")
            });
        let mut patched = BTreeMap::new();
        let type_attribute = regex::Regex::new(r#"\s+t\s*=\s*(?:"[^"]*"|'[^']*')"#)?;
        for s in &self.sheets {
            let part = string(&s["part"])?;
            let original = std::str::from_utf8(&self.parts[part])?;
            let doc = Document::parse(original)?;
            let mut edits = vec![];
            let mut seen = BTreeSet::new();
            for cell in doc.descendants().filter(|n| {
                n.has_tag_name((NS, "c")) && n.parent().is_some_and(|p| p.has_tag_name((NS, "row")))
            }) {
                let address = cell.attribute("r").context("missing address")?;
                if let Some(value) = updates.get(part).and_then(|u| u.get(address)) {
                    seen.insert(address.to_owned());
                    let range = cell.range();
                    let raw = &original[range.clone()];
                    let end = raw.find('>').context("invalid cell")?;
                    let tag = raw[1..]
                        .split([' ', '\t', '\r', '\n', '/', '>'])
                        .next()
                        .unwrap();
                    let prefix = tag.strip_suffix('c').unwrap();
                    let opening = type_attribute
                        .replace_all(&raw[..end], "")
                        .trim_end_matches('/')
                        .to_owned();
                    let body = match value {
                        Value::Null => ">".to_owned(),
                        Value::String(text) => {
                            ensure!(
                                text.encode_utf16().count() <= 32767
                                    && !text
                                        .chars()
                                        .any(|c| c < ' ' && !matches!(c, '\t' | '\n' | '\r')),
                                "unsupported Excel string"
                            );
                            let text = text
                                .replace('&', "&amp;")
                                .replace('<', "&lt;")
                                .replace('>', "&gt;")
                                .replace('\r', "&#13;");
                            format!(
                                " t=\"inlineStr\"><{prefix}is><{prefix}t xml:space=\"preserve\">{text}</{prefix}t></{prefix}is>"
                            )
                        }
                        Value::Bool(b) => {
                            format!(" t=\"b\"><{prefix}v>{}</{prefix}v>", if *b { 1 } else { 0 })
                        }
                        Value::Number(n) => format!(" t=\"n\"><{prefix}v>{n}</{prefix}v>"),
                        _ => bail!("scalar required"),
                    };
                    edits.push((range, format!("{opening}{body}</{tag}>")));
                } else if recalc
                    && child(cell, "f").is_some()
                    && let Some(cache) = child(cell, "v")
                {
                    edits.push((cache.range(), String::new()));
                }
            }
            if let Some(wanted) = updates.get(part) {
                ensure!(
                    seen == wanted.keys().cloned().collect(),
                    "writeback cell missing"
                );
            }
            if !edits.is_empty() {
                edits.sort_by_key(|(range, _)| range.start);
                let mut result = original.to_owned();
                for (range, replacement) in edits.into_iter().rev() {
                    result.replace_range(range, &replacement)
                }
                patched.insert(part.to_owned(), result.into_bytes());
            }
        }
        if recalc {
            let original = std::str::from_utf8(&self.parts["xl/workbook.xml"])?;
            let doc = Document::parse(original)?;
            let root = doc.root_element();
            let start = &original[root.range().start + 1..];
            let tag = start.split([' ', '>', '\n', '\r', '\t']).next().unwrap();
            let prefix = tag.strip_suffix("workbook").unwrap();
            let calc = format!(
                "<{prefix}calcPr calcId=\"0\" fullCalcOnLoad=\"1\" forceFullCalc=\"1\" calcMode=\"auto\"/>"
            );
            let mut result = original.to_owned();
            if let Some(old) = child(root, "calcPr") {
                result.replace_range(old.range(), &calc)
            } else {
                let pos = root
                    .children()
                    .find(|n| {
                        n.is_element()
                            && [
                                "oleSize",
                                "customWorkbookViews",
                                "pivotCaches",
                                "smartTagPr",
                                "smartTagTypes",
                                "webPublishing",
                                "fileRecoveryPr",
                                "webPublishObjects",
                                "extLst",
                            ]
                            .contains(&n.tag_name().name())
                    })
                    .map(|n| n.range().start)
                    .unwrap_or_else(|| original.rfind("</").unwrap());
                result.insert_str(pos, &calc)
            }
            patched.insert("xl/workbook.xml".into(), result.into_bytes());
        }
        let mut source = ZipArchive::new(Cursor::new(&self.raw))?;
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(destination)?;
        let mut output = ZipWriter::new(file);
        output.set_raw_comment(source.comment().to_vec().into())?;
        for i in 0..source.len() {
            let entry = source.by_index(i)?;
            if let Some(bytes) = patched.get(entry.name()) {
                let mut options =
                    SimpleFileOptions::default().compression_method(entry.compression());
                if let Some(time) = entry.last_modified() {
                    options = options.last_modified_time(time)
                }
                if let Some(mode) = entry.unix_mode() {
                    options = options.unix_permissions(mode)
                }
                output.start_file(entry.name(), options)?;
                output.write_all(bytes)?;
            } else {
                output.raw_copy_file(entry)?;
            }
        }
        output.finish()?.sync_all()?;
        let reread = Self::open(destination)?;
        for change in changes {
            let found = reread
                .sheets
                .iter()
                .find(|s| s["name"] == change["sheet"])
                .and_then(|s| {
                    s["cells"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|c| c["address"] == change["cell"])
                })
                .map(|c| &c["value"])
                .unwrap_or(&Value::Null);
            ensure!(
                found == &change["after"]
                    || (found.is_number()
                        && change["after"].is_number()
                        && found.as_f64() == change["after"].as_f64()),
                "Excel read-back failed for {}!{}: expected {}, found {}",
                change["sheet"],
                change["cell"],
                change["after"],
                found
            );
        }
        Ok(
            json!({"changed_parts":patched.keys().collect::<Vec<_>>(),"requires_excel_recalculation":recalc}),
        )
    }

    pub fn patch_with_operations(
        &self,
        destination: &Path,
        operations: &[Value],
        changes: &[Value],
    ) -> Result<Value> {
        self.patch_with_operations_and_assets(destination, operations, changes, &BTreeMap::new())
    }

    pub fn patch_with_operations_and_assets(
        &self,
        destination: &Path,
        operation_values: &[Value],
        changes: &[Value],
        assets: &BTreeMap<String, Vec<u8>>,
    ) -> Result<Value> {
        if operation_values.is_empty() && assets.is_empty() {
            return self.patch_scalar(destination, changes);
        }
        let operations = parse_operations(operation_values, &self.sheets)?;
        self.ensure_structural_edits_supported(&operations)?;
        let image_operations = parse_image_operations(operation_values, &self.sheets)?;
        ensure!(
            image_operations.is_empty() || !assets.is_empty(),
            "image operations require image assets"
        );
        ensure!(
            !self
                .parts
                .keys()
                .any(|n| n.to_lowercase().starts_with("_xmlsignatures/")),
            "signed Excel cannot be modified"
        );
        let mut change_map: BTreeMap<(String, u32, u32), Value> = BTreeMap::new();
        for change in changes {
            let sheet = string(&change["sheet"])?;
            let (column, row) = coordinate(string(&change["cell"])?)?;
            ensure!(
                self.sheets
                    .iter()
                    .any(|candidate| candidate["name"] == sheet),
                "missing writeback sheet"
            );
            ensure!(
                change_map
                    .insert((sheet.to_owned(), row, column), change["after"].clone())
                    .is_none(),
                "duplicate writeback target"
            );
        }
        let structural = !operations.is_empty();
        let recalc = structural
            || !changes.is_empty()
                && self.sheets.iter().any(|sheet| {
                    sheet["cells"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|cell| cell["type"] == "formula")
                });
        let mut patched = BTreeMap::new();
        let mut removed = BTreeSet::new();
        let mut table_formulas = BTreeMap::new();
        let moves = Moves::new(
            &operations,
            if structural {
                removed_table_columns(&self.parts, &self.sheets, &operations)?
            } else {
                BTreeMap::new()
            },
        );
        if structural {
            relocate_tables(
                &self.parts,
                &self.sheets,
                &moves,
                &mut change_map,
                &mut table_formulas,
                &mut patched,
            )?;
        }
        for sheet in &self.sheets {
            let name = string(&sheet["name"])?;
            let sheet_operations: Vec<_> = operations
                .iter()
                .filter(|operation| operation.sheet == name)
                .cloned()
                .collect();
            let sheet_changes: BTreeMap<(u32, u32), Value> = change_map
                .iter()
                .filter(|((sheet_name, _, _), _)| sheet_name == name)
                .map(|((_, row, column), value)| ((*row, *column), value.clone()))
                .collect();
            let part = string(&sheet["part"])?;
            let source = std::str::from_utf8(&self.parts[part])?;
            let unshared = if structural {
                unshare_formulas(source, name, &moves, !sheet_operations.is_empty())?
            } else {
                None
            };
            let original = unshared.as_deref().unwrap_or(source);
            if sheet_operations.is_empty() && sheet_changes.is_empty() {
                if structural {
                    let rewritten = rewrite_sheet_references(original, name, &moves, false)?;
                    if rewritten != source {
                        patched.insert(part.to_owned(), rewritten.into_bytes());
                    }
                }
                continue;
            }
            let doc = xml(original.as_bytes())?;
            let sheet_data = child(doc.root_element(), "sheetData").context("missing sheetData")?;
            let mut rows = vec![];
            for row in sheet_data
                .children()
                .filter(|node| node.has_tag_name((NS, "row")))
            {
                let original_row: u32 =
                    row.attribute("r").context("missing row number")?.parse()?;
                let Some(final_row) = transform_index(original_row, &sheet_operations, true) else {
                    continue;
                };
                let raw = &original[row.range()];
                rows.push(transform_row(
                    row,
                    raw,
                    &RowTransform {
                        original,
                        original_row,
                        final_row,
                        sheet: name,
                        operations: &sheet_operations,
                        moves: &moves,
                        changes: &sheet_changes,
                        recalc,
                    },
                )?);
            }
            for operation in &sheet_operations {
                if !matches!(operation.kind, OperationKind::InsertRows) {
                    continue;
                }
                for offset in 0..operation.count {
                    let address = resolve_insertion(
                        name,
                        &operation.id,
                        offset,
                        Some("A"),
                        None,
                        &operations,
                    )?;
                    let (_, row) = coordinate(&address)?;
                    let template = operation
                        .style_from
                        .and_then(|number| find_row_raw(sheet_data, number, original));
                    rows.push(new_row(template, row));
                }
            }
            let mut seen_rows: BTreeSet<u32> = rows.iter().map(|row| row.number).collect();
            for (row, _column) in sheet_changes.keys() {
                if !seen_rows.contains(row) {
                    rows.push(new_row(None, *row));
                    seen_rows.insert(*row);
                }
            }
            for row in &mut rows {
                let mut existing = row.cells.clone();
                for ((change_row, change_column), value) in sheet_changes.iter() {
                    if *change_row != row.number || existing.contains(change_column) {
                        continue;
                    }
                    let address = format!("{}{}", column_name(*change_column)?, row.number);
                    let template = None;
                    row.insert_cell(&render_new_cell(&address, value, template)?, *change_column)?;
                    existing.insert(*change_column);
                }
                for ((formula_row, formula_column), formula) in table_formulas
                    .range(
                        (name.to_owned(), row.number, 0)..=(name.to_owned(), row.number, u32::MAX),
                    )
                    .map(|((_, r, c), f)| ((*r, *c), f))
                {
                    if formula_row != row.number || existing.contains(&formula_column) {
                        continue;
                    }
                    let prefix = row.prefix().to_owned();
                    let cell = format!(
                        r#"<{prefix}c r="{}{}"><{prefix}f>{}</{prefix}f></{prefix}c>"#,
                        column_name(formula_column)?,
                        row.number,
                        xml_attr(formula)
                    );
                    row.insert_cell(&cell, formula_column)?;
                    existing.insert(formula_column);
                }
            }
            rows.sort_by_key(|row| row.number);
            for pair in rows.windows(2) {
                ensure!(
                    pair[0].number != pair[1].number,
                    "structural operation row collision"
                );
            }
            let inner_start = original[sheet_data.range().start..]
                .find('>')
                .map(|offset| sheet_data.range().start + offset + 1)
                .context("invalid sheetData")?;
            let inner_end = original[..sheet_data.range().end]
                .rfind("</")
                .context("invalid sheetData")?;
            let mut result = original.to_owned();
            result.replace_range(
                inner_start..inner_end,
                &rows
                    .iter()
                    .map(|row| row.raw.as_str())
                    .collect::<Vec<_>>()
                    .join(""),
            );
            if structural {
                result = rewrite_sheet_references(&result, name, &moves, true)?;
            }
            patched.insert(part.to_owned(), result.into_bytes());
        }
        apply_image_operations(
            &self.parts,
            &mut patched,
            &self.sheets,
            &operations,
            &image_operations,
            assets,
        )?;
        if structural {
            relocate_pivots(&self.parts, &self.sheets, &operations, &mut patched)?;
            relocate_charts(&self.parts, &moves, &mut patched)?;
            let workbook = std::str::from_utf8(&self.parts["xl/workbook.xml"])?;
            if let Some(updated) = relocate_defined_names(workbook, &moves)? {
                patched.insert("xl/workbook.xml".into(), updated.into_bytes());
            }
            drop_calc_chain(&self.parts, &mut patched, &mut removed)?;
        }
        if recalc {
            let original = std::str::from_utf8(
                patched
                    .get("xl/workbook.xml")
                    .unwrap_or(&self.parts["xl/workbook.xml"]),
            )?
            .to_owned();
            let original = original.as_str();
            let doc = xml(original.as_bytes())?;
            let root = doc.root_element();
            let start = &original[root.range().start + 1..];
            let tag = start.split([' ', '>', '\n', '\r', '\t']).next().unwrap();
            let prefix = tag.strip_suffix("workbook").unwrap();
            let calc = format!(
                "<{prefix}calcPr calcId=\"0\" fullCalcOnLoad=\"1\" forceFullCalc=\"1\" calcMode=\"auto\"/>"
            );
            let mut result = original.to_owned();
            if let Some(old) = child(root, "calcPr") {
                result.replace_range(old.range(), &calc)
            } else {
                let pos = root
                    .children()
                    .find(|node| {
                        node.is_element()
                            && [
                                "oleSize",
                                "customWorkbookViews",
                                "pivotCaches",
                                "smartTagPr",
                                "smartTagTypes",
                                "webPublishing",
                                "fileRecoveryPr",
                                "webPublishObjects",
                                "extLst",
                            ]
                            .contains(&node.tag_name().name())
                    })
                    .map(|node| node.range().start)
                    .unwrap_or_else(|| original.rfind("</").unwrap());
                result.insert_str(pos, &calc)
            }
            patched.insert("xl/workbook.xml".into(), result.into_bytes());
        }
        write_archive_without(&self.raw, destination, &patched, &removed)?;
        let reread = Self::open(destination)?;
        for change in changes {
            let found = reread
                .sheets
                .iter()
                .find(|sheet| sheet["name"] == change["sheet"])
                .and_then(|sheet| {
                    sheet["cells"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|cell| cell["address"] == change["cell"])
                })
                .map(|cell| &cell["value"])
                .unwrap_or(&Value::Null);
            ensure!(
                found == &change["after"]
                    || (found.is_number()
                        && change["after"].is_number()
                        && found.as_f64() == change["after"].as_f64()),
                "Excel read-back failed for {}!{}: expected {}, found {}",
                change["sheet"],
                change["cell"],
                change["after"],
                found
            );
        }
        Ok(json!({
            "changed_parts": patched.keys().collect::<Vec<_>>(),
            "requires_excel_recalculation": recalc,
        }))
    }
}
