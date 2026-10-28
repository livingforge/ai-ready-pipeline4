use super::*;
use crate::extraction::CellKind;

impl Workbook {
    /// Row/column edits move cells and every reference ARP can rewrite
    /// (formulas on all sheets, defined names, sheet ranges, tables, pivots,
    /// charts, drawing anchors, notes, comments and form controls). Reject
    /// workbooks whose other parts hold cell positions that would silently
    /// point at the wrong cells, and changes Excel itself refuses (cutting
    /// through tables or pivot tables).
    pub fn ensure_structural_edits_supported(
        &self,
        operations: &[StructuralOperation],
    ) -> Result<()> {
        if operations.is_empty() {
            return Ok(());
        }
        if operations
            .iter()
            .any(|o| matches!(o.kind, OperationKind::MoveColumns { .. }))
        {
            return self.ensure_column_moves_supported(operations);
        }
        let mut scratch = BTreeMap::new();
        let moves = Moves::new(
            operations,
            removed_table_columns(&self.parts, &self.sheets, operations)?,
        );
        relocate_tables(
            &self.parts,
            &BTreeMap::new(),
            &self.sheets,
            &moves,
            &mut BTreeMap::new(),
            &mut BTreeMap::new(),
            &mut scratch,
        )?;
        relocate_pivots(&self.parts, &self.sheets, operations, &mut scratch)?;
        self.ensure_structural_parts_supported(operations)
    }

    /// Package features unsupported by structural writeback. Table and pivot
    /// ranges are checked by relocation itself when the writeback runs. A VBA
    /// project keeps the addresses in its code, so operations on a workbook
    /// with one need `vba_unchanged`; without it they fail with
    /// [`VbaNotUpdated`] once nothing else refuses them.
    pub(super) fn ensure_structural_parts_supported(
        &self,
        operations: &[StructuralOperation],
    ) -> Result<()> {
        if operations.is_empty() {
            return Ok(());
        }
        let mut reasons = vec![];
        let unaccepted: Vec<String> = if self.has_vba_project() {
            operations
                .iter()
                .filter(|o| !o.vba_unchanged)
                .map(|o| o.id.clone())
                .collect()
        } else {
            vec![]
        };
        if !unaccepted.is_empty() {
            reasons.push(
                "VBA project (cell addresses in macro code are not updated; accepted with --vba-unchanged)"
                    .to_owned(),
            );
        }
        for sheet in &self.skipped_sheets {
            let kind = string(&sheet["kind"])?;
            if matches!(kind, "macrosheet" | "dialogsheet") {
                reasons.push(format!("{kind} '{}'", string(&sheet["name"])?));
            }
        }
        // ActiveX controls may keep their linked cells in binary parts.
        if self
            .parts
            .keys()
            .any(|name| name.to_ascii_lowercase().starts_with("xl/activex/"))
        {
            reasons.push(
                "ActiveX controls (their linked cells can be held in binary parts)".to_owned(),
            );
        }
        if reasons.len() == 1 && !unaccepted.is_empty() {
            return Err(VbaNotUpdated {
                operations: unaccepted,
            }
            .into());
        }
        ensure!(
            reasons.is_empty(),
            "row/column insert/delete is not supported for this workbook because ARP cannot move cell positions held by: {}. Make row/column changes in Excel and re-import; cell value edits remain supported",
            reasons.join("; ")
        );
        Ok(())
    }

    /// Whether one of `changes` writes a header or totals label of a table.
    fn writes_table_labels(&self, changes: &[Value]) -> Result<bool> {
        for change in changes {
            let Some(sheet) = self.sheets.iter().find(|s| s["name"] == change["sheet"]) else {
                continue;
            };
            let (column, row) = coordinate(string(&change["cell"])?)?;
            if table_label(sheet, column, row)?.is_some() {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Whether one of `changes` gives a value to a cell holding none, which
    /// the worksheet may not have: one whose `before` is null names a cell
    /// the workbook holds no value in, created as Excel creates it.
    fn writes_new_cells(&self, changes: &[Value]) -> Result<bool> {
        let cells = cells_by_address(self);
        for change in changes {
            if change.get("before").is_some_and(Value::is_null)
                && !cells.contains_key(&(string(&change["sheet"])?, string(&change["cell"])?))
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Whether any cell holds a formula, whose cached result a change can make stale.
    fn has_formulas(&self) -> bool {
        self.cells
            .iter()
            .flatten()
            .any(|cell| cell.kind == CellKind::Formula)
    }

    pub fn patch(&self, destination: &Path, changes: &[Value]) -> Result<Value> {
        self.patch_with_operations(destination, &[], changes)
    }

    /// The formula edits (`sheet`, original `cell`, new formula `after` with
    /// its `=`) grouped by sheet, as address to formula text without `=`.
    pub(super) fn formula_edits(
        &self,
        formulas: &[Value],
    ) -> Result<BTreeMap<String, BTreeMap<String, String>>> {
        let cells = cells_by_address(self);
        let mut edits: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
        for edit in formulas {
            let sheet = string(&edit["sheet"])?;
            let cell = string(&edit["cell"])?;
            let found = cells
                .get(&(sheet, cell))
                .with_context(|| format!("formula edit target {sheet}!{cell} is missing"))?;
            ensure!(
                found.kind == CellKind::Formula,
                "{sheet}!{cell} holds no formula; only existing formulas can be replaced"
            );
            let text = string(&edit["after"])?
                .strip_prefix('=')
                .with_context(|| format!("the formula of {sheet}!{cell} must start with ="))?;
            check_formula(text).with_context(|| format!("formula of {sheet}!{cell}"))?;
            ensure!(
                edits
                    .entry(sheet.to_owned())
                    .or_default()
                    .insert(cell.to_owned(), text.to_owned())
                    .is_none(),
                "duplicate formula edit of {sheet}!{cell}"
            );
        }
        Ok(edits)
    }

    fn patch_scalar(
        &self,
        destination: &Path,
        changes: &[Value],
        formulas: &[Value],
        shapes: &[Value],
    ) -> Result<Value> {
        ensure!(
            !self
                .parts
                .keys()
                .any(|n| n.to_lowercase().starts_with("_xmlsignatures/")),
            "signed Excel cannot be modified"
        );
        let formula_edits = self.formula_edits(formulas)?;
        let mut updates: BTreeMap<String, BTreeMap<String, Value>> = BTreeMap::new();
        let cells = cells_by_address(self);
        let checks = SheetChecks::new(&self.sheets)?;
        for change in changes {
            let sheet = self
                .sheets
                .iter()
                .find(|s| s["name"] == change["sheet"])
                .context("missing writeback sheet")?;
            let name = string(&sheet["name"])?;
            let cell = string(&change["cell"])?;
            let found = cells.get(&(name, cell)).context("missing writeback cell")?;
            ensure!(
                found.kind != CellKind::Formula && found.kind != CellKind::Error,
                "formula/error cells cannot be overwritten"
            );
            ensure!(
                kind(&change["after"]) != "object",
                "scalar writeback required"
            );
            ensure!(
                change["after"].is_null() || found.kind.as_str() == kind(&change["after"]),
                "Excel target type mismatch"
            );
            checks.merges[name].ensure_not_hidden(name, cell)?;
            checks.computed[name].ensure_not_computed(cell)?;
            ensure!(
                updates
                    .entry(string(&sheet["part"])?.into())
                    .or_default()
                    .insert(cell.into(), change["after"].clone())
                    .is_none(),
                "duplicate writeback target"
            );
        }
        let recalc = (!changes.is_empty() || !formulas.is_empty()) && self.has_formulas();
        let mut patched = BTreeMap::new();
        let shared = SharedStrings::new(&self.parts);
        for s in &self.sheets {
            let part = string(&s["part"])?;
            let name = string(&s["name"])?;
            if !recalc && !updates.contains_key(part) && !formula_edits.contains_key(name) {
                continue;
            }
            let source = std::str::from_utf8(&self.parts[part])?;
            let edited = formula_edits
                .get(name)
                .map(|edits| edit_formulas(source, name, edits))
                .transpose()?;
            let original = edited.as_deref().unwrap_or(source);
            let doc = Document::parse(original)?;
            let mut edits = vec![];
            let mut seen = BTreeSet::new();
            for cell in doc.descendants().filter(|n| {
                n.has_tag_name((NS, "c")) && n.parent().is_some_and(|p| p.has_tag_name((NS, "row")))
            }) {
                let address = cell.attribute("r").context("missing address")?;
                if let Some(value) = updates.get(part).and_then(|u| u.get(address)) {
                    seen.insert(address.to_owned());
                    edits.push((
                        cell.range(),
                        render_cell(original, cell, address, value, &shared)?,
                    ));
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
                patched.insert(
                    part.to_owned(),
                    splice(original, edits, "cell")?.into_bytes(),
                );
            } else if edited.is_some() {
                patched.insert(part.to_owned(), original.as_bytes().to_vec());
            }
        }
        if recalc {
            let original = std::str::from_utf8(&self.parts["xl/workbook.xml"])?;
            patched.insert(
                "xl/workbook.xml".into(),
                request_full_calculation(original)?.into_bytes(),
            );
        }
        apply_shape_texts(&self.parts, &mut patched, shapes)?;
        for sheet in &self.sheets {
            let part = string(&sheet["part"])?;
            if let Some(after) = patched.get(part) {
                let values: BTreeSet<_> = updates
                    .get(part)
                    .into_iter()
                    .flat_map(|v| v.keys().map(String::as_str))
                    .collect();
                let formulas: BTreeSet<_> = formula_edits
                    .get(string(&sheet["name"])?)
                    .into_iter()
                    .flat_map(|v| v.keys().map(String::as_str))
                    .collect();
                ensure!(
                    worksheet_invariants(
                        std::str::from_utf8(&self.parts[part])?,
                        &values,
                        &formulas,
                        recalc
                    )? == worksheet_invariants(
                        std::str::from_utf8(after)?,
                        &values,
                        &formulas,
                        recalc
                    )?,
                    "non-edited worksheet content changed: {part}"
                );
                self.ensure_runs_kept(part, after, &values, &shared)?;
            }
        }
        write_archive(&self.raw, destination, &patched)?;
        self.ensure_package_kept(destination, &patched)?;
        ensure_read_back(destination, changes, formulas, shapes, &[])?;
        Ok(
            json!({"changed_parts":patched.keys().collect::<Vec<_>>(),"requires_excel_recalculation":recalc,"non_edit_preservation_verified":true,"layout_review_required":true}),
        )
    }

    pub fn patch_with_operations(
        &self,
        destination: &Path,
        operations: &[Value],
        changes: &[Value],
    ) -> Result<Value> {
        self.patch_with_operations_and_assets(
            destination,
            operations,
            changes,
            &[],
            &BTreeMap::new(),
        )
    }

    /// Writes `changes` (values of cells on the final sheets, or with `shape`
    /// the text of a shape), `formulas`
    /// (new formulas of original cells, applied before rows and columns move so
    /// that their references move too), the row/column and image operations
    /// and their image assets.
    pub fn patch_with_operations_and_assets(
        &self,
        destination: &Path,
        operation_values: &[Value],
        changes: &[Value],
        formulas: &[Value],
        assets: &BTreeMap<String, Vec<u8>>,
    ) -> Result<Value> {
        // Shape text is written into the drawings, apart from the cells.
        let (shapes, changes): (Vec<Value>, Vec<Value>) = changes
            .iter()
            .cloned()
            .partition(|change| change.get("shape").is_some());
        let changes = changes.as_slice();
        // A table's header and totals labels are written with its definition,
        // and a new cell into its row.
        if operation_values.is_empty()
            && assets.is_empty()
            && !self.writes_table_labels(changes)?
            && !self.writes_new_cells(changes)?
        {
            return self.patch_scalar(destination, changes, formulas, &shapes);
        }
        let operations = parse_operations(operation_values, &self.sheets)?;
        if operations
            .iter()
            .any(|o| matches!(o.kind, OperationKind::MoveColumns { .. }))
        {
            ensure!(
                assets.is_empty(),
                "apply image operations before moving columns"
            );
            return self.patch_column_moves(destination, &operations, changes, formulas, &shapes);
        }
        self.ensure_structural_parts_supported(&operations)?;
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
        let mut merges = BTreeMap::new();
        for sheet in &self.sheets {
            merges.insert(string(&sheet["name"])?, merges_after(sheet, &operations)?);
        }
        let final_merges = merges
            .iter()
            .map(|(name, merges)| Ok((*name, Merges::new(merges)?)))
            .collect::<Result<BTreeMap<_, _>>>()?;
        let checks = SheetChecks::new(&self.sheets)?;
        let mut change_map: BTreeMap<(String, u32, u32), Value> = BTreeMap::new();
        for change in changes {
            let sheet = string(&change["sheet"])?;
            let cell = string(&change["cell"])?;
            let (column, row) = coordinate(cell)?;
            final_merges
                .get(sheet)
                .context("missing writeback sheet")?
                .ensure_not_hidden(sheet, cell)?;
            // Computed ranges on a sheet with row/column changes are checked as
            // they move; table labels are checked with their table.
            if sheet_operations(sheet, &operations).is_empty() {
                checks
                    .computed
                    .get(sheet)
                    .context("missing writeback sheet")?
                    .ensure_not_computed(cell)?;
            }
            // A formula given to a cell holding none is written as one.
            let after = if change["formula"] == true {
                json!({"formula":change["after"],"written_after":change["written_after"]})
            } else {
                change["after"].clone()
            };
            ensure!(
                change_map
                    .insert((sheet.to_owned(), row, column), after)
                    .is_none(),
                "duplicate writeback target"
            );
        }
        let formula_edits = self.formula_edits(formulas)?;
        for (sheet, edits) in &formula_edits {
            for cell in edits.keys() {
                ensure!(
                    map_coordinate(sheet, cell, &operations)?.is_some(),
                    "{sheet}!{cell} is deleted by a delete_rows/delete_columns operation, so its formula cannot be edited"
                );
            }
        }
        let labels = LabelEdits::new(&self.parts, &self.sheets, &operations, &change_map)?;
        let structural = !operations.is_empty();
        // Without operations, labels or images only new cells are written; like
        // scalar writes, they are proved to leave everything else as it was.
        let preserving = !structural
            && labels.parts.is_empty()
            && labels.renamed.is_empty()
            && image_operations.is_empty();
        // Renamed table columns rename structured references in every formula.
        let rewriting = structural || !labels.renamed.is_empty();
        let new_formulas = changes.iter().any(|change| change["formula"] == true);
        let recalc = structural
            || new_formulas
            || (!changes.is_empty() || !formulas.is_empty()) && self.has_formulas();
        let mut patched = BTreeMap::new();
        let mut removed = BTreeSet::new();
        let mut table_formulas = BTreeMap::new();
        let shared = SharedStrings::new(&self.parts);
        let moves = Moves::new(
            &operations,
            if structural {
                removed_table_columns(&self.parts, &self.sheets, &operations)?
            } else {
                BTreeMap::new()
            },
        )
        .with_renamed_columns(labels.renamed.clone());
        // A formula given to a cell holding none is written as the original's
        // places give it, and moves with the operations as the others do;
        // one written after an operation, as the cells stand after it, moves
        // with the operations recorded after that one.
        for ((sheet, _, _), value) in change_map.iter_mut() {
            if let Some(formula) = value["formula"].as_str().and_then(|f| f.strip_prefix('=')) {
                let moved = match value["written_after"].as_str() {
                    Some(frame) => {
                        let at = operations
                            .iter()
                            .position(|operation| operation.id == frame)
                            .with_context(|| {
                                format!("a formula of {sheet} is written after the operation {frame}, which is not recorded")
                            })?;
                        rewrite_references(formula, Some(sheet), &operations[at + 1..])?
                    }
                    None => moves.rewrite(formula, Some(sheet))?,
                };
                *value = json!({"formula":format!("={moved}")});
            }
        }
        if structural || !labels.parts.is_empty() {
            relocate_tables(
                &self.parts,
                &labels.parts,
                &self.sheets,
                &moves,
                &mut change_map,
                &mut table_formulas,
                &mut patched,
            )?;
            for (part, text) in labels.parts {
                patched.entry(part).or_insert_with(|| text.into_bytes());
            }
        }
        let mut operations_by_sheet: BTreeMap<&str, Vec<StructuralOperation>> = BTreeMap::new();
        for operation in &operations {
            operations_by_sheet
                .entry(&operation.sheet)
                .or_default()
                .push(operation.clone());
        }
        let mut changes_by_sheet: BTreeMap<String, BTreeMap<(u32, u32), Value>> = BTreeMap::new();
        for ((sheet, row, column), value) in change_map {
            changes_by_sheet
                .entry(sheet)
                .or_default()
                .insert((row, column), value);
        }
        let mut borders = SharedBorders::new(
            self.parts
                .get("xl/styles.xml")
                .map(|bytes| std::str::from_utf8(bytes))
                .transpose()?,
        );
        for sheet in &self.sheets {
            let name = string(&sheet["name"])?;
            let sheet_operations = operations_by_sheet.remove(name).unwrap_or_default();
            let sheet_changes = changes_by_sheet.remove(name).unwrap_or_default();
            let part = string(&sheet["part"])?;
            let part_text = std::str::from_utf8(&self.parts[part])?;
            let edited = formula_edits
                .get(name)
                .map(|edits| edit_formulas(part_text, name, edits))
                .transpose()?;
            let source = edited.as_deref().unwrap_or(part_text);
            let unshared = if rewriting {
                unshare_formulas(source, name, &moves, !sheet_operations.is_empty())?
            } else {
                None
            };
            let original = unshared.as_deref().unwrap_or(source);
            if sheet_operations.is_empty() && sheet_changes.is_empty() {
                let rewritten = if rewriting {
                    rewrite_sheet_references(original, name, &moves, false)?
                } else {
                    original.to_owned()
                };
                if rewritten != part_text {
                    patched.insert(part.to_owned(), rewritten.into_bytes());
                }
                continue;
            }
            let doc = xml(original.as_bytes())?;
            let sheet_data = child(doc.root_element(), "sheetData").context("missing sheetData")?;
            let prefix = element_tag(&original[sheet_data.range()])?
                .strip_suffix("sheetData")
                .context("invalid sheetData tag")?
                .to_owned();
            let mut formats = CellFormats::read(doc.root_element(), &sheet_operations)?;
            // Cells shifted within a band leave their rows: the sheet is laid
            // out cell by cell.
            let banded = sheet_operations.iter().any(|operation| !operation.whole());
            let mut rows = if banded {
                let formulas: BTreeMap<(u32, u32), &str> = table_formulas
                    .range((name.to_owned(), 0, 0)..=(name.to_owned(), u32::MAX, u32::MAX))
                    .map(|((_, row, column), formula)| ((*row, *column), formula.as_str()))
                    .collect();
                super::shifted::shifted_rows(
                    sheet_data,
                    &prefix,
                    &RowTransform {
                        shared: &shared,
                        original,
                        original_row: 0,
                        final_row: 0,
                        sheet: name,
                        operations: &sheet_operations,
                        moves: &moves,
                        changes: &sheet_changes,
                        recalc,
                    },
                    &formats,
                    &formulas,
                    &mut borders,
                )?
            } else {
                vec![]
            };
            let indexed_styles = !banded
                && sheet_operations
                    .iter()
                    .any(|operation| operation.style_from.is_some());
            let mut original_rows = BTreeMap::new();
            for row in sheet_data
                .children()
                .filter(|node| !banded && node.has_tag_name((NS, "row")))
            {
                let original_row: u32 =
                    row.attribute("r").context("missing row number")?.parse()?;
                if indexed_styles {
                    original_rows
                        .entry(original_row)
                        .or_insert(&original[row.range()]);
                }
                let Some(final_row) = transform_index(original_row, &sheet_operations, true) else {
                    continue;
                };
                ensure!(
                    final_row <= MAX_ROW,
                    "inserting rows in {name} would push row {original_row} past the last row of the sheet ({MAX_ROW}); delete rows at the bottom first"
                );
                let raw = &original[row.range()];
                rows.push(transform_row(
                    row,
                    raw,
                    &RowTransform {
                        shared: &shared,
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
            for operation in sheet_operations.iter().filter(|_| !banded) {
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
                    // The style rows are taken in turn.
                    let style_from = operation
                        .style_from
                        .map(|number| number + offset % operation.style_rows.max(1));
                    let template =
                        style_from.and_then(|number| original_rows.get(&number).copied());
                    if let Some(style_from) = style_from {
                        formats.template(row, style_from);
                    }
                    rows.push(new_row(template, row, &prefix));
                }
            }
            let mut seen_rows: BTreeSet<u32> = rows.iter().map(|row| row.number).collect();
            for (row, _column) in sheet_changes.keys().filter(|_| !banded) {
                if !seen_rows.contains(row) {
                    rows.push(new_row(None, *row, &prefix));
                    seen_rows.insert(*row);
                }
            }
            for row in rows.iter_mut().filter(|_| !banded) {
                let mut existing = row.cells.clone();
                let mut additions = Vec::new();
                for ((_, change_column), value) in
                    sheet_changes.range((row.number, 0)..=(row.number, u32::MAX))
                {
                    if existing.contains(change_column) {
                        continue;
                    }
                    let address = format!("{}{}", column_name(*change_column)?, row.number);
                    let style = formats
                        .style(row, *change_column)
                        .map(|style| {
                            match formats.after_inserted_column(row.number, *change_column) {
                                Some(next) => borders.between(&style, Some(next)),
                                None => Ok(style),
                            }
                        })
                        .transpose()?;
                    additions.push((
                        *change_column,
                        render_new_cell(&address, value, &prefix, style.as_deref())?,
                    ));
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
                    // Excel formats the formula like the other cells it fills in.
                    let style = formats
                        .style(row, formula_column)
                        .map(|style| {
                            match formats.after_inserted_column(row.number, formula_column) {
                                Some(next) => borders.between(&style, Some(next)),
                                None => Ok(style),
                            }
                        })
                        .transpose()?
                        .map(|s| format!(r#" s="{}""#, xml_attr(&s)))
                        .unwrap_or_default();
                    let cell = format!(
                        r#"<{prefix}c r="{}{}"{style}><{prefix}f>{}</{prefix}f></{prefix}c>"#,
                        column_name(formula_column)?,
                        row.number,
                        xml_attr(formula)
                    );
                    additions.push((formula_column, cell));
                    existing.insert(formula_column);
                }
                let template =
                    formats
                        .template_cells(row.number)
                        .into_iter()
                        .filter_map(|(column, style)| {
                            Some((transform_index(column, &sheet_operations, false)?, style))
                        });
                for (column, style) in template.chain(formats.inserted_column_cells(row.number)) {
                    if existing.contains(&column) {
                        continue;
                    }
                    let style = match formats.after_inserted_column(row.number, column) {
                        Some(next) => borders.between(style, Some(next))?,
                        None => style.to_owned(),
                    };
                    let prefix = row.prefix().to_owned();
                    let cell = format!(
                        r#"<{prefix}c r="{}{}" s="{}"/>"#,
                        column_name(column)?,
                        row.number,
                        xml_attr(&style)
                    );
                    additions.push((column, cell));
                    existing.insert(column);
                }
                row.insert_cells(additions)?;
            }
            let used = used_range(&rows);
            rows.sort_by_key(|row| row.number);
            for pair in rows.windows(2) {
                ensure!(
                    pair[0].number != pair[1].number,
                    "structural operation row collision"
                );
            }
            let joined = rows
                .iter()
                .map(|row| row.raw.as_str())
                .collect::<Vec<_>>()
                .join("");
            let raw_data = &original[sheet_data.range()];
            let mut result = original.to_owned();
            if raw_data.ends_with("/>") {
                // An empty sheet writes `<sheetData/>`, which has no content to replace.
                let tag = element_tag(raw_data)?;
                result.replace_range(sheet_data.range(), &format!("<{tag}>{joined}</{tag}>"));
            } else {
                let inner_start =
                    sheet_data.range().start + raw_data.find('>').context("invalid sheetData")? + 1;
                let inner_end =
                    sheet_data.range().start + raw_data.rfind("</").context("invalid sheetData")?;
                result.replace_range(inner_start..inner_end, &joined);
            }
            if rewriting {
                result = rewrite_sheet_references(&result, name, &moves, true)?;
            }
            let copies = copied_merges(sheet, &operations)?;
            if !copies.is_empty() {
                result = add_merges(&result, &copies)?;
            }
            if let Some(used) = used {
                result = cover_dimension(&result, used)?;
            }
            patched.insert(part.to_owned(), result.into_bytes());
        }
        if let Some(styles) = borders.finish()? {
            patched.insert("xl/styles.xml".into(), styles.into_bytes());
        }
        let placements = super::parse_place_operations(operation_values, &self.sheets)?;
        apply_image_operations(
            &self.parts,
            &mut patched,
            &self.sheets,
            &operations,
            &image_operations,
            &placements,
            assets,
        )?;
        if structural {
            relocate_pivots(&self.parts, &self.sheets, &operations, &mut patched)?;
            relocate_charts(&self.parts, &moves, &mut patched)?;
            relocate_sheet_objects(&self.parts, &self.sheets, &operations, &moves, &mut patched)?;
            for sheet in &self.sheets {
                let part = string(&sheet["part"])?;
                // Only a drawing of the original can hold links; one add_image
                // created has none.
                let worksheet = std::str::from_utf8(&self.parts[part])?;
                let Some((drawing_part, _)) = worksheet_drawing(&self.parts, part, worksheet)?
                else {
                    continue;
                };
                let drawing = std::str::from_utf8(
                    patched
                        .get(&drawing_part)
                        .unwrap_or(&self.parts[&drawing_part]),
                )?;
                let linked = rewrite_text_links(drawing, string(&sheet["name"])?, &moves)?;
                if linked != drawing {
                    patched.insert(drawing_part, linked.into_bytes());
                }
            }
            drop_calc_chain(&self.parts, &mut patched, &mut removed)?;
        }
        if rewriting {
            let workbook = std::str::from_utf8(&self.parts["xl/workbook.xml"])?;
            if let Some(updated) = relocate_defined_names(workbook, &moves)? {
                patched.insert("xl/workbook.xml".into(), updated.into_bytes());
            }
        }
        if recalc {
            let original = std::str::from_utf8(
                patched
                    .get("xl/workbook.xml")
                    .unwrap_or(&self.parts["xl/workbook.xml"]),
            )?;
            let result = request_full_calculation(original)?;
            patched.insert("xl/workbook.xml".into(), result.into_bytes());
        }
        apply_shape_texts(&self.parts, &mut patched, &shapes)?;
        if preserving {
            self.ensure_new_cells_only(&patched, changes, &formula_edits, recalc)?;
        }
        write_archive_without(&self.raw, destination, &patched, &removed)?;
        if preserving {
            self.ensure_package_kept(destination, &patched)?;
        }
        ensure_read_back(destination, changes, formulas, &shapes, &operations)?;
        let mut report = json!({
            "changed_parts": patched.keys().collect::<Vec<_>>(),
            "requires_excel_recalculation": recalc,
            "layout_review_required": true,
        });
        if preserving {
            report["non_edit_preservation_verified"] = json!(true);
        }
        Ok(report)
    }

    /// Each patched worksheet differs from the original only in the cells
    /// `changes` write, the rows created for new cells and the used range.
    fn ensure_new_cells_only(
        &self,
        patched: &BTreeMap<String, Vec<u8>>,
        changes: &[Value],
        formula_edits: &BTreeMap<String, BTreeMap<String, String>>,
        recalc: bool,
    ) -> Result<()> {
        let cells = cells_by_address(self);
        let shared = SharedStrings::new(&self.parts);
        for sheet in &self.sheets {
            let name = string(&sheet["name"])?;
            let part = string(&sheet["part"])?;
            let Some(after) = patched.get(part) else {
                continue;
            };
            let mut values = BTreeSet::new();
            let mut created = BTreeSet::new();
            for change in changes.iter().filter(|change| change["sheet"] == name) {
                let cell = string(&change["cell"])?;
                if cells.contains_key(&(name, cell)) {
                    values.insert(cell);
                } else {
                    created.insert(cell);
                }
            }
            let formulas: BTreeSet<_> = formula_edits
                .get(name)
                .into_iter()
                .flat_map(|v| v.keys().map(String::as_str))
                .collect();
            let before = std::str::from_utf8(&self.parts[part])?;
            let edits = WorksheetEdits {
                values: &values,
                formulas: &formulas,
                created: &created,
                rows: &row_numbers(before)?,
                recalc,
            };
            ensure!(
                edits.preserved(before)? == edits.preserved(std::str::from_utf8(after)?)?,
                "non-edited worksheet content changed: {part}"
            );
            self.ensure_runs_kept(part, after, &values, &shared)?;
        }
        Ok(())
    }

    /// The run formats of each edited cell in `values` are those of the original.
    fn ensure_runs_kept(
        &self,
        part: &str,
        after: &[u8],
        values: &BTreeSet<&str>,
        shared: &SharedStrings<'_>,
    ) -> Result<()> {
        let before_text = std::str::from_utf8(&self.parts[part])?;
        let after_text = std::str::from_utf8(after)?;
        let before_doc = xml(before_text.as_bytes())?;
        let after_doc = xml(after_text.as_bytes())?;
        let after_cells: BTreeMap<_, _> = after_doc
            .descendants()
            .filter(|node| node.has_tag_name((NS, "c")))
            .map(|cell| (cell.attribute("r").unwrap_or(""), cell))
            .collect();
        for cell in before_doc.descendants().filter(|node| {
            node.has_tag_name((NS, "c")) && values.contains(node.attribute("r").unwrap_or(""))
        }) {
            let address = cell.attribute("r").context("missing cell address")?;
            let written = *after_cells
                .get(address)
                .context("edited cell disappeared")?;
            ensure!(
                super::text_edit::run_properties(cell, before_text, shared)?
                    == super::text_edit::run_properties(written, after_text, shared)?,
                "rich text run properties changed: {part}!{address}"
            );
        }
        Ok(())
    }

    /// The package written at `destination` holds the same parts, each with
    /// the original bytes unless `patched` replaced it.
    fn ensure_package_kept(
        &self,
        destination: &Path,
        patched: &BTreeMap<String, Vec<u8>>,
    ) -> Result<()> {
        let reread = Workbook::open(destination)?;
        ensure!(
            reread.parts.len() == self.parts.len(),
            "Excel package parts changed unexpectedly"
        );
        for (part, before) in &self.parts {
            ensure!(
                reread.parts.get(part) == Some(patched.get(part).unwrap_or(before)),
                "Excel package preservation failed: {part}"
            );
        }
        Ok(())
    }
}

/// Compare everything except explicitly edited values/formulas and stale caches.
/// Cell attributes and non-value children, row heights, column widths, merges,
/// print settings and all unedited cell markup remain part of the comparison.
fn worksheet_invariants(
    original: &str,
    values: &BTreeSet<&str>,
    formulas: &BTreeSet<&str>,
    recalc: bool,
) -> Result<String> {
    let doc = xml(original.as_bytes())?;
    let mut edits = vec![];
    for cell in doc.descendants().filter(|n| n.has_tag_name((NS, "c"))) {
        let address = cell.attribute("r").context("missing cell address")?;
        if values.contains(address) || formulas.contains(address) {
            let attributes: BTreeMap<_, _> = cell
                .attributes()
                .filter(|a| {
                    !(values.contains(address) && a.namespace().is_none() && a.name() == "t")
                })
                .map(|a| ((a.namespace().unwrap_or(""), a.name()), a.value()))
                .collect();
            let retained: String = cell
                .children()
                .filter(Node::is_element)
                .filter(|node| {
                    !(node.has_tag_name((NS, "v"))
                        || (values.contains(address) && node.has_tag_name((NS, "is")))
                        || (formulas.contains(address) && node.has_tag_name((NS, "f"))))
                })
                .map(|node| &original[node.range()])
                .collect();
            edits.push((cell.range(), format!("{attributes:?}{retained}")));
        } else if recalc
            && child(cell, "f").is_some()
            && let Some(cache) = child(cell, "v")
        {
            edits.push((cache.range(), String::new()));
        }
    }
    splice(original, edits, "worksheet invariants")
}

/// The numbers of the rows the worksheet `text` holds.
fn row_numbers(text: &str) -> Result<BTreeSet<u32>> {
    xml(text.as_bytes())?
        .descendants()
        .filter(|node| {
            node.has_tag_name((NS, "row"))
                && node
                    .parent()
                    .is_some_and(|p| p.has_tag_name((NS, "sheetData")))
        })
        .map(|row| Ok(row.attribute("r").context("missing row number")?.parse()?))
        .collect()
}

/// What writing cells without operations may change in a worksheet: the
/// values of the original cells in `values`, the formulas in `formulas`, the
/// cells in `created`, rows that are not among the original `rows`, the
/// recorded used range and, when `recalc`, formula caches.
struct WorksheetEdits<'a> {
    values: &'a BTreeSet<&'a str>,
    formulas: &'a BTreeSet<&'a str>,
    created: &'a BTreeSet<&'a str>,
    rows: &'a BTreeSet<u32>,
    recalc: bool,
}

impl WorksheetEdits<'_> {
    /// The worksheet `text` as a tree without what the edits may change. A
    /// row written `<row/>` compares equal to the same row with an end tag,
    /// which it takes when a new cell joins it.
    fn preserved(&self, text: &str) -> Result<String> {
        let doc = xml(text.as_bytes())?;
        let mut out = String::new();
        self.render(doc.root_element(), &mut out)?;
        Ok(out)
    }

    fn render(&self, node: Node<'_, '_>, out: &mut String) -> Result<()> {
        use std::fmt::Write as _;
        if node.is_text() {
            write!(out, "{:?}", node.text().unwrap_or(""))?;
            return Ok(());
        }
        if !node.is_element() || self.skipped(node)? {
            return Ok(());
        }
        let address = node.attribute("r").unwrap_or("");
        let attributes: BTreeMap<_, _> = node
            .attributes()
            .filter(|a| {
                !(a.namespace().is_none()
                    && (node.has_tag_name((NS, "dimension")) && a.name() == "ref"
                        || node.has_tag_name((NS, "c"))
                            && self.values.contains(address)
                            && a.name() == "t"))
            })
            .map(|a| ((a.namespace().unwrap_or(""), a.name()), a.value()))
            .collect();
        let name = node.tag_name();
        write!(
            out,
            "<{}|{}{attributes:?}>",
            name.namespace().unwrap_or(""),
            name.name()
        )?;
        for child in node.children() {
            self.render(child, out)?;
        }
        out.push_str("</>");
        Ok(())
    }

    fn skipped(&self, node: Node<'_, '_>) -> Result<bool> {
        if node.has_tag_name((NS, "c")) {
            return Ok(self.created.contains(node.attribute("r").unwrap_or("")));
        }
        if node.has_tag_name((NS, "row"))
            && node
                .parent()
                .is_some_and(|p| p.has_tag_name((NS, "sheetData")))
        {
            let number = node.attribute("r").context("missing row number")?.parse()?;
            return Ok(!self.rows.contains(&number));
        }
        let Some(cell) = node.parent().filter(|p| p.has_tag_name((NS, "c"))) else {
            return Ok(false);
        };
        let address = cell.attribute("r").unwrap_or("");
        let value = self.values.contains(address);
        let formula = self.formulas.contains(address);
        Ok(node.has_tag_name((NS, "v"))
            && (value || formula || self.recalc && child(cell, "f").is_some())
            || value && node.has_tag_name((NS, "is"))
            || formula && node.has_tag_name((NS, "f")))
    }
}

/// Each cell of `book` by sheet name and address, so that checking many
/// changes does not scan the sheet's cells for each one.
fn cells_by_address(book: &Workbook) -> BTreeMap<(&str, &str), &Cell> {
    let mut cells = BTreeMap::new();
    for (sheet, list) in book.sheets.iter().zip(&book.cells) {
        let Some(name) = sheet["name"].as_str() else {
            continue;
        };
        for cell in list {
            cells.entry((name, &*cell.address)).or_insert(cell);
        }
    }
    cells
}

#[cfg(test)]
mod performance_tests {
    use super::*;
    use std::{hint::black_box, time::Instant};

    #[test]
    #[ignore = "manual sheet change partition performance measurement"]
    fn measure_sheet_change_partition() {
        let names: Vec<_> = (0..200).map(|sheet| format!("S{sheet}")).collect();
        let changes: BTreeMap<_, _> = names
            .iter()
            .flat_map(|sheet| (1..=100).map(move |row| ((sheet.clone(), row, 1), json!(row))))
            .collect();
        let mut original = Vec::new();
        let mut optimized = Vec::new();
        for _ in 0..5 {
            let start = Instant::now();
            let mut old = BTreeMap::new();
            for name in &names {
                let sheet_changes: BTreeMap<(u32, u32), Value> = changes
                    .iter()
                    .filter(|((sheet_name, _, _), _)| sheet_name == name)
                    .map(|((_, row, column), value)| ((*row, *column), value.clone()))
                    .collect();
                old.insert(name.clone(), sheet_changes);
            }
            original.push(start.elapsed().as_secs_f64() * 1000.0);
            let owned = changes.clone();
            let start = Instant::now();
            let mut new: BTreeMap<String, BTreeMap<(u32, u32), Value>> = BTreeMap::new();
            for ((sheet, row, column), value) in owned {
                new.entry(sheet).or_default().insert((row, column), value);
            }
            optimized.push(start.elapsed().as_secs_f64() * 1000.0);
            assert_eq!(black_box(old), black_box(new));
        }
        original.sort_by(f64::total_cmp);
        optimized.sort_by(f64::total_cmp);
        eprintln!(
            "sheet_partition_original_ms={:.3} optimized_ms={:.3}",
            original[2], optimized[2]
        );
    }
}

/// Merged and computed ranges of each sheet, parsed once for all changes.
struct SheetChecks<'a> {
    merges: BTreeMap<&'a str, Merges<'a>>,
    computed: BTreeMap<&'a str, ComputedRanges<'a>>,
}

impl<'a> SheetChecks<'a> {
    fn new(sheets: &'a [Value]) -> Result<Self> {
        let mut checks = Self {
            merges: BTreeMap::new(),
            computed: BTreeMap::new(),
        };
        for sheet in sheets {
            let name = string(&sheet["name"])?;
            checks.merges.insert(name, Merges::new(&sheet["merges"])?);
            checks.computed.insert(name, ComputedRanges::new(sheet)?);
        }
        Ok(checks)
    }
}

/// Writes the new text of the shapes of `shapes` (`shape` is the extraction ID
/// `<drawing part>#<id>`) into their drawings as `patched` leaves them.
fn apply_shape_texts(
    parts: &BTreeMap<String, Vec<u8>>,
    patched: &mut BTreeMap<String, Vec<u8>>,
    shapes: &[Value],
) -> Result<()> {
    let mut by_part: BTreeMap<&str, BTreeMap<String, (String, String)>> = BTreeMap::new();
    for change in shapes {
        let (part, id) = string(&change["shape"])?
            .rsplit_once('#')
            .context("shape ID without drawing part")?;
        let text = |key: &str| string(&change[key]).map(str::to_owned);
        ensure!(
            by_part
                .entry(part)
                .or_default()
                .insert(id.to_owned(), (text("before")?, text("after")?))
                .is_none(),
            "duplicate shape text edit of {part}#{id}"
        );
    }
    for (part, texts) in by_part {
        let drawing = patched
            .get(part)
            .or_else(|| parts.get(part))
            .with_context(|| format!("drawing part {part} is missing"))?;
        let edited =
            crate::document_source::edit_shape_texts(std::str::from_utf8(drawing)?, &texts)?;
        patched.insert(part.to_owned(), edited.into_bytes());
    }
    Ok(())
}

/// Re-opens the written workbook and checks that every change reads back, and
/// every formula edit as a formula where `operations` moved its cell. References
/// move with rows and columns, so only formulas no operation moved read back
/// verbatim.
fn ensure_read_back(
    destination: &Path,
    changes: &[Value],
    formulas: &[Value],
    shapes: &[Value],
    operations: &[StructuralOperation],
) -> Result<()> {
    let reread = Workbook::open(destination)?;
    let cells = cells_by_address(&reread);
    for change in shapes {
        let found = reread
            .sheets
            .iter()
            .filter(|s| s["name"] == change["sheet"])
            .flat_map(|s| s["drawings"].as_array().into_iter().flatten())
            .find(|d| d["id"] == change["shape"])
            .map(|d| &d["text"]);
        ensure!(
            found == Some(&change["after"]),
            "Excel read-back failed for shape {}: expected {}, found {found:?}",
            change["shape"],
            change["after"]
        );
    }
    for edit in formulas {
        let sheet = string(&edit["sheet"])?;
        let address = map_coordinate(sheet, string(&edit["cell"])?, operations)?
            .context("edited formula cell deleted")?;
        let found = cells
            .get(&(sheet, address.as_str()))
            .and_then(|cell| cell.formula.as_deref());
        let expected = string(&edit["after"])?.strip_prefix('=');
        ensure!(
            found.is_some() && (!operations.is_empty() || found == expected),
            "Excel formula read-back failed for {sheet}!{address}: expected {}, found {found:?}",
            edit["after"]
        );
    }
    for change in changes {
        let cell = change["sheet"]
            .as_str()
            .zip(change["cell"].as_str())
            .and_then(|key| cells.get(&key));
        if change["formula"] == true {
            let found = cell.and_then(|cell| cell.formula.as_deref());
            ensure!(
                found.is_some()
                    && (!operations.is_empty()
                        || found == string(&change["after"])?.strip_prefix('=')),
                "Excel formula read-back failed for {}!{}: expected {}, found {found:?}",
                change["sheet"],
                change["cell"],
                change["after"]
            );
            continue;
        }
        let found = cell.map(|cell| &cell.value).unwrap_or(&Value::Null);
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
    Ok(())
}

/// Asks Excel to recalculate every formula, since the cached results ARP
/// removed or left behind are stale. `fullCalcOnLoad` asks for one full
/// calculation when Excel opens the book; `forceFullCalc` would make every
/// later calculation full as well, so it is not set. The workbook's own
/// calculation settings (iteration for intended circular references,
/// precision as displayed, manual mode, R1C1 display, its own forceFullCalc)
/// stay as they are.
pub(super) fn request_full_calculation(original: &str) -> Result<String> {
    let doc = xml(original.as_bytes())?;
    let root = doc.root_element();
    let mut result = original.to_owned();
    if let Some(old) = child(root, "calcPr") {
        let (opening, _) = xml_opening(&original[old.range()])?;
        let replacement = set_xml_attribute(opening, "fullCalcOnLoad", "1")?;
        let start = old.range().start;
        result.replace_range(start..start + opening.len(), &replacement);
    } else {
        let start = &original[root.range().start + 1..];
        let tag = start.split([' ', '>', '\n', '\r', '\t']).next().unwrap();
        let prefix = tag
            .strip_suffix("workbook")
            .context("invalid workbook tag")?;
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
            .or_else(|| original.rfind("</"))
            .context("invalid workbook")?;
        result.insert_str(pos, &format!("<{prefix}calcPr fullCalcOnLoad=\"1\"/>"));
    }
    Ok(result)
}

/// The formats of cells inserted between two cells across columns, made as
/// the workbook is written: Excel formats such a cell like the cell before
/// it but keeps only the borders the cells on both sides draw alike, so a
/// column inserted beside a bordered block or merge draws no stray lines.
pub(super) struct SharedBorders<'s> {
    source: Option<&'s str>,
    styles: Option<super::fonts::StyleFormats<'s>>,
    made: BTreeMap<(String, String), String>,
}

impl<'s> SharedBorders<'s> {
    pub(super) fn new(source: Option<&'s str>) -> Self {
        Self {
            source,
            styles: None,
            made: BTreeMap::new(),
        }
    }

    /// The format of a cell inserted after one of format `style`, before one
    /// of format `next` (none: the default format).
    pub(super) fn between(&mut self, style: &str, next: Option<&str>) -> Result<String> {
        let next = next.unwrap_or("0");
        if style == next {
            return Ok(style.to_owned());
        }
        let key = (style.to_owned(), next.to_owned());
        if let Some(made) = self.made.get(&key) {
            return Ok(made.clone());
        }
        let Some(source) = self.source else {
            return Ok(style.to_owned());
        };
        let styles = match &mut self.styles {
            Some(styles) => styles,
            None => match super::fonts::StyleFormats::new(source) {
                Ok(styles) => self.styles.insert(styles),
                // A styles part without the lists of formats tells no
                // borders to keep apart: the cell keeps the format before it.
                Err(_) => {
                    self.source = None;
                    return Ok(style.to_owned());
                }
            },
        };
        let made = styles
            .with_shared_borders(style.parse()?, next.parse()?)?
            .to_string();
        self.made.insert(key, made.clone());
        Ok(made)
    }

    /// The styles part with the formats made, if any was.
    pub(super) fn finish(self) -> Result<Option<String>> {
        match (self.styles, self.source) {
            (Some(styles), Some(source)) => {
                let finished = styles.finish()?;
                Ok((finished != source).then_some(finished))
            }
            _ => Ok(None),
        }
    }
}

/// The formatting Excel gives a cell ARP creates: an inserted row's cell is
/// formatted like the cell above it and an inserted column's like the cell to its
/// left; otherwise the row's format applies, then the column's.
pub(super) struct CellFormats<'a> {
    cells: BTreeMap<(u32, u32), String>,
    columns: Vec<(u32, u32, String)>,
    operations: Vec<&'a StructuralOperation>,
    inserted_rows: InsertedIntervals,
    inserted_columns: InsertedIntervals,
    /// The original `style_from` row of each inserted row that names one.
    templates: BTreeMap<u32, u32>,
}

impl<'a> CellFormats<'a> {
    pub(super) fn read(
        worksheet: Node<'_, '_>,
        operations: &'a [StructuralOperation],
    ) -> Result<Self> {
        let mut cells = BTreeMap::new();
        if let Some(data) = child(worksheet, "sheetData") {
            for cell in data.descendants().filter(|n| n.has_tag_name((NS, "c"))) {
                if let (Some(address), Some(style)) = (cell.attribute("r"), cell.attribute("s")) {
                    let (column, row) = coordinate(address)?;
                    cells.insert((row, column), style.to_owned());
                }
            }
        }
        let mut columns = vec![];
        if let Some(cols) = child(worksheet, "cols") {
            for col in cols.children().filter(|n| n.has_tag_name((NS, "col"))) {
                if let (Some(min), Some(max), Some(style)) = (
                    col.attribute("min"),
                    col.attribute("max"),
                    col.attribute("style"),
                ) {
                    columns.push((min.parse()?, max.parse()?, style.to_owned()));
                }
            }
        }
        let own: Vec<_> = operations.iter().collect();
        Ok(Self {
            cells,
            columns,
            inserted_rows: InsertedIntervals::new(&own, true),
            inserted_columns: InsertedIntervals::new(&own, false),
            operations: own,
            templates: BTreeMap::new(),
        })
    }

    /// Formats the inserted final `row` like the original row `style_from`.
    pub(super) fn template(&mut self, row: u32, style_from: u32) {
        self.templates.insert(row, style_from);
    }

    /// The original columns and formats of the cells of the `style_from` row
    /// of the inserted final `row`. Excel formats every cell of an inserted
    /// row, including the empty ones that draw borders and fills.
    pub(super) fn template_cells(&self, row: u32) -> Vec<(u32, &str)> {
        let Some(&from) = self.templates.get(&row) else {
            return vec![];
        };
        self.cells
            .range((from, 0)..=(from, u32::MAX))
            .filter(|(_, style)| style.as_str() != "0")
            .map(|((_, column), style)| (*column, style.as_str()))
            .collect()
    }

    /// The final columns and formats of the cells inserted columns get in the
    /// final `row`. Excel formats each like the cell to its left, so borders
    /// and fills run on through the new columns.
    pub(super) fn inserted_column_cells(&self, row: u32) -> Vec<(u32, &str)> {
        let Some(source_row) = self.source_row(row) else {
            return vec![];
        };
        self.inserted_columns
            .inheriting()
            .filter_map(|column| {
                let style = self.cells.get(&(source_row, self.source(column, false)?))?;
                (style != "0").then_some((column, style.as_str()))
            })
            .collect()
    }

    /// For a cell of a column inserted next to an original one in the final
    /// `row`, the format of the cell after the inserted columns: Excel keeps
    /// only the borders the cells on both sides draw alike.
    pub(super) fn after_inserted_column(&self, row: u32, column: u32) -> Option<&str> {
        self.inserted_columns.inherited_from(column)?;
        let next = self.inserted_columns.end_of(column)?;
        let source_row = self.source_row(row)?;
        let next = original_position(next, &self.operations, false)?;
        Some(
            self.cells
                .get(&(source_row, next))
                .map(String::as_str)
                .or_else(|| {
                    self.columns
                        .iter()
                        .find(|(min, max, _)| (*min..=*max).contains(&next))
                        .map(|(_, _, style)| style.as_str())
                })
                .unwrap_or("0"),
        )
    }

    /// The original row a final row is, or takes its format from.
    fn source_row(&self, row: u32) -> Option<u32> {
        self.templates
            .get(&row)
            .copied()
            .or_else(|| self.source(row, true))
    }

    /// The original position a final row or column is, or takes its format from.
    fn source(&self, position: u32, row: bool) -> Option<u32> {
        let inserted = if row {
            &self.inserted_rows
        } else {
            &self.inserted_columns
        };
        original_position(position, &self.operations, row).or_else(|| {
            inserted
                .inherited_from(position)
                .and_then(|from| original_position(from, &self.operations, row))
        })
    }

    pub(super) fn style(&self, row: &RowOutput, column: u32) -> Option<String> {
        let source_column = self.source(column, false);
        let source_row = self.source_row(row.number);
        if let (Some(source_row), Some(source_column)) = (source_row, source_column)
            && let Some(style) = self.cells.get(&(source_row, source_column))
        {
            return Some(style.clone());
        }
        if let Some(style) = row.custom_style() {
            return Some(style);
        }
        let source_column = source_column?;
        self.columns
            .iter()
            .find(|(min, max, _)| (*min..=*max).contains(&source_column))
            .map(|(_, _, style)| style.clone())
    }
}

#[cfg(test)]
mod preservation_tests {
    use super::*;

    #[test]
    fn worksheet_projection_detects_layout_and_unedited_value_changes() {
        let before = format!(
            r#"<worksheet xmlns="{NS}"><cols><col min="1" max="1" width="20"/></cols><sheetData><row r="1" ht="30"><c r="A1" s="0" t="inlineStr"><is><t>old</t></is><extLst/></c><c r="B1"><v>5</v></c><c r="C1"><f>B1*2</f><v>10</v></c></row></sheetData><mergeCells><mergeCell ref="A2:B2"/></mergeCells></worksheet>"#
        );
        let values = BTreeSet::from(["A1"]);
        let baseline = worksheet_invariants(&before, &values, &BTreeSet::new(), true).unwrap();
        let permitted = before.replace("old", "new").replace("<v>10</v>", "");
        assert_eq!(
            baseline,
            worksheet_invariants(&permitted, &values, &BTreeSet::new(), true).unwrap()
        );
        for (from, to) in [
            ("width=\"20\"", "width=\"21\""),
            ("ht=\"30\"", "ht=\"31\""),
            ("s=\"0\"", "s=\"1\""),
            ("<v>5</v>", "<v>6</v>"),
            ("B1*2", "B1*3"),
            ("A2:B2", "A2:C2"),
            ("<extLst/>", ""),
        ] {
            assert_ne!(
                baseline,
                worksheet_invariants(&before.replace(from, to), &values, &BTreeSet::new(), true)
                    .unwrap(),
                "{from}"
            );
        }
    }

    #[test]
    fn new_cell_projection_ignores_only_written_cells_rows_and_used_range() {
        let before = format!(
            r#"<worksheet xmlns="{NS}"><dimension ref="A1:B2"/><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>old</t></is></c><c r="B1"><v>5</v></c></row><row r="2" ht="30"/></sheetData></worksheet>"#
        );
        let values = BTreeSet::from(["A1"]);
        let created = BTreeSet::from(["C2", "A3"]);
        let rows = row_numbers(&before).unwrap();
        let edits = WorksheetEdits {
            values: &values,
            formulas: &BTreeSet::new(),
            created: &created,
            rows: &rows,
            recalc: false,
        };
        let baseline = edits.preserved(&before).unwrap();
        let written = before
            .replace("A1:B2", "A1:C3")
            .replace("<is><t>old</t></is>", "<v>1</v>")
            .replace(
                r#"<row r="2" ht="30"/>"#,
                r#"<row r="2" ht="30"><c r="C2"><v>2</v></c></row><row r="3"><c r="A3"><v>3</v></c></row>"#,
            );
        assert_eq!(baseline, edits.preserved(&written).unwrap());
        for (from, to) in [
            ("<v>5</v>", "<v>6</v>"),
            (r#"ht="30""#, r#"ht="31""#),
            (r#"<row r="2" ht="30"><c r="C2"><v>2</v></c></row>"#, ""),
            (
                r#"<c r="C2"><v>2</v></c>"#,
                r#"<c r="B2"><v>2</v></c><c r="C2"><v>2</v></c>"#,
            ),
        ] {
            assert_ne!(
                baseline,
                edits.preserved(&written.replace(from, to)).unwrap(),
                "{from}"
            );
        }
    }
}
