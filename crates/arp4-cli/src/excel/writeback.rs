use super::*;

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
        self.ensure_structural_parts_supported(operations)
    }

    /// Package features unsupported by structural writeback. Table and pivot
    /// ranges are checked by relocation itself when the writeback runs.
    fn ensure_structural_parts_supported(&self, operations: &[StructuralOperation]) -> Result<()> {
        if operations.is_empty() {
            return Ok(());
        }
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

    /// Whether any cell holds a formula, whose cached result a change can make stale.
    fn has_formulas(&self) -> bool {
        self.cells
            .iter()
            .flatten()
            .any(|cell| cell.kind == "formula")
    }

    pub fn patch(&self, destination: &Path, changes: &[Value]) -> Result<Value> {
        self.patch_with_operations(destination, &[], changes)
    }

    /// The formula edits (`sheet`, original `cell`, new formula `after` with
    /// its `=`) grouped by sheet, as address to formula text without `=`.
    fn formula_edits(
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
                found.kind == "formula",
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
                found.kind != "formula" && found.kind != "error",
                "formula/error cells cannot be overwritten"
            );
            ensure!(
                kind(&change["after"]) != "object",
                "scalar writeback required"
            );
            ensure!(
                change["after"].is_null() || found.kind == kind(&change["after"]),
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
        let type_attribute = attribute_pattern("t")?;
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
                    let body = scalar_body(prefix, value)?;
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
        write_archive(&self.raw, destination, &patched)?;
        ensure_read_back(destination, changes, formulas, shapes, &[])?;
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
        // A table's header and totals labels are written with its definition.
        if operation_values.is_empty() && assets.is_empty() && !self.writes_table_labels(changes)? {
            return self.patch_scalar(destination, changes, formulas, &shapes);
        }
        let operations = parse_operations(operation_values, &self.sheets)?;
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
            ensure!(
                change_map
                    .insert((sheet.to_owned(), row, column), change["after"].clone())
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
        // Renamed table columns rename structured references in every formula.
        let rewriting = structural || !labels.renamed.is_empty();
        let recalc =
            structural || (!changes.is_empty() || !formulas.is_empty()) && self.has_formulas();
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
        )
        .with_renamed_columns(labels.renamed.clone());
        if structural || !labels.parts.is_empty() {
            let mut labeled;
            let parts = if labels.parts.is_empty() {
                &self.parts
            } else {
                labeled = self.parts.clone();
                for (part, text) in &labels.parts {
                    labeled.insert(part.clone(), text.clone().into_bytes());
                }
                &labeled
            };
            relocate_tables(
                parts,
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
            let mut rows = vec![];
            let indexed_styles = sheet_operations
                .iter()
                .any(|operation| operation.style_from.is_some());
            let mut original_rows = BTreeMap::new();
            for row in sheet_data
                .children()
                .filter(|node| node.has_tag_name((NS, "row")))
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
                        .and_then(|number| original_rows.get(&number).copied());
                    if let Some(style_from) = operation.style_from {
                        formats.template(row, style_from);
                    }
                    rows.push(new_row(template, row, &prefix));
                }
            }
            let mut seen_rows: BTreeSet<u32> = rows.iter().map(|row| row.number).collect();
            for (row, _column) in sheet_changes.keys() {
                if !seen_rows.contains(row) {
                    rows.push(new_row(None, *row, &prefix));
                    seen_rows.insert(*row);
                }
            }
            for row in &mut rows {
                let mut existing = row.cells.clone();
                let mut additions = Vec::new();
                for ((_, change_column), value) in
                    sheet_changes.range((row.number, 0)..=(row.number, u32::MAX))
                {
                    if existing.contains(change_column) {
                        continue;
                    }
                    let address = format!("{}{}", column_name(*change_column)?, row.number);
                    let style = formats.style(row, *change_column);
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
                    let cell = format!(
                        r#"<{prefix}c r="{}{}"><{prefix}f>{}</{prefix}f></{prefix}c>"#,
                        column_name(formula_column)?,
                        row.number,
                        xml_attr(formula)
                    );
                    additions.push((formula_column, cell));
                    existing.insert(formula_column);
                }
                for (column, style) in formats.template_cells(row.number) {
                    let Some(column) = transform_index(column, &sheet_operations, false) else {
                        continue;
                    };
                    if existing.contains(&column) {
                        continue;
                    }
                    let prefix = row.prefix().to_owned();
                    let cell = format!(
                        r#"<{prefix}c r="{}{}" s="{}"/>"#,
                        column_name(column)?,
                        row.number,
                        xml_attr(style)
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
            if let Some(used) = used {
                result = cover_dimension(&result, used)?;
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
        write_archive_without(&self.raw, destination, &patched, &removed)?;
        ensure_read_back(destination, changes, formulas, &shapes, &operations)?;
        Ok(json!({
            "changed_parts": patched.keys().collect::<Vec<_>>(),
            "requires_excel_recalculation": recalc,
        }))
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
            cells.entry((name, cell.address.as_str())).or_insert(cell);
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
        let found = change["sheet"]
            .as_str()
            .zip(change["cell"].as_str())
            .and_then(|key| cells.get(&key))
            .map(|cell| &cell.value)
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
    Ok(())
}

/// Asks Excel to recalculate every formula, since the cached results ARP
/// removed or left behind are stale. The workbook's own calculation settings
/// (iteration for intended circular references, precision as displayed, manual
/// mode, R1C1 display) stay as they are.
fn request_full_calculation(original: &str) -> Result<String> {
    let doc = xml(original.as_bytes())?;
    let root = doc.root_element();
    let mut result = original.to_owned();
    if let Some(old) = child(root, "calcPr") {
        let (opening, _) = xml_opening(&original[old.range()])?;
        let replacement = set_xml_attribute(
            &set_xml_attribute(opening, "fullCalcOnLoad", "1")?,
            "forceFullCalc",
            "1",
        )?;
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
        result.insert_str(
            pos,
            &format!("<{prefix}calcPr fullCalcOnLoad=\"1\" forceFullCalc=\"1\"/>"),
        );
    }
    Ok(result)
}

/// The formatting Excel gives a cell ARP creates: an inserted row's cell is
/// formatted like the cell above it and an inserted column's like the cell to its
/// left; otherwise the row's format applies, then the column's.
pub(super) struct CellFormats<'a> {
    cells: BTreeMap<(u32, u32), String>,
    columns: Vec<(u32, u32, String)>,
    operations: Vec<&'a StructuralOperation>,
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
        Ok(Self {
            cells,
            columns,
            operations: operations.iter().collect(),
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

    /// The original position a final row or column is, or takes its format from.
    fn source(&self, position: u32, row: bool) -> Option<u32> {
        original_position(position, &self.operations, row).or_else(|| {
            inherited_from(position, &self.operations, row)
                .and_then(|from| original_position(from, &self.operations, row))
        })
    }

    pub(super) fn style(&self, row: &RowOutput, column: u32) -> Option<String> {
        let source_column = self.source(column, false);
        let source_row = self
            .templates
            .get(&row.number)
            .copied()
            .or_else(|| self.source(row.number, true));
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
