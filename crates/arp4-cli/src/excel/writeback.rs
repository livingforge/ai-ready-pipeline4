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
        // A form control on another sheet keeps its linked cell and input range in
        // ctrlProp and VML parts, which ARP does not rewrite.
        for (name, bytes) in &self.parts {
            let lower = name.to_ascii_lowercase();
            if !(lower.starts_with("xl/ctrlprops/") || lower.ends_with(".vml")) {
                continue;
            }
            let text = String::from_utf8_lossy(bytes)
                .replace("&apos;", "'")
                .replace("&amp;", "&");
            for sheet in &edited {
                if names_sheet(&text, sheet) {
                    reasons.push(format!("form control links to '{sheet}' in {name}"));
                }
            }
        }
        ensure!(
            reasons.is_empty(),
            "row/column insert/delete is not supported for this workbook because ARP cannot move cell positions held by: {}. Make row/column changes in Excel and re-import; cell value edits remain supported",
            reasons.join("; ")
        );
        Ok(())
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

    fn patch_scalar(&self, destination: &Path, changes: &[Value]) -> Result<Value> {
        ensure!(
            !self
                .parts
                .keys()
                .any(|n| n.to_lowercase().starts_with("_xmlsignatures/")),
            "signed Excel cannot be modified"
        );
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
            let (column, row) = coordinate(cell)?;
            ensure_not_table_label(sheet, column, row)?;
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
        let recalc = !changes.is_empty() && self.has_formulas();
        let mut patched = BTreeMap::new();
        let type_attribute = attribute_pattern("t")?;
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
            }
        }
        if recalc {
            let original = std::str::from_utf8(&self.parts["xl/workbook.xml"])?;
            patched.insert(
                "xl/workbook.xml".into(),
                request_full_calculation(original)?.into_bytes(),
            );
        }
        write_archive(&self.raw, destination, &patched)?;
        ensure_read_back(destination, changes)?;
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
            // Tables on a sheet with row/column changes are checked as they move.
            if sheet_operations(sheet, &operations).is_empty() {
                let source = self
                    .sheets
                    .iter()
                    .find(|s| s["name"] == sheet)
                    .context("missing writeback sheet")?;
                ensure_not_table_label(source, column, row)?;
                checks.computed[sheet].ensure_not_computed(cell)?;
            }
            ensure!(
                change_map
                    .insert((sheet.to_owned(), row, column), change["after"].clone())
                    .is_none(),
                "duplicate writeback target"
            );
        }
        let structural = !operations.is_empty();
        let recalc = structural || !changes.is_empty() && self.has_formulas();
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
            let prefix = element_tag(&original[sheet_data.range()])?
                .strip_suffix("sheetData")
                .context("invalid sheetData tag")?
                .to_owned();
            let formats = CellFormats::read(doc.root_element(), &sheet_operations)?;
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
                        .and_then(|number| find_row_raw(sheet_data, number, original));
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
                for ((change_row, change_column), value) in sheet_changes.iter() {
                    if *change_row != row.number || existing.contains(change_column) {
                        continue;
                    }
                    let address = format!("{}{}", column_name(*change_column)?, row.number);
                    let style = formats.style(row, *change_column);
                    row.insert_cell(
                        &render_new_cell(&address, value, &prefix, style.as_deref())?,
                        *change_column,
                    )?;
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
            )?;
            let result = request_full_calculation(original)?;
            patched.insert("xl/workbook.xml".into(), result.into_bytes());
        }
        write_archive_without(&self.raw, destination, &patched, &removed)?;
        ensure_read_back(destination, changes)?;
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

/// Re-opens the written workbook and checks that every change reads back.
fn ensure_read_back(destination: &Path, changes: &[Value]) -> Result<()> {
    let reread = Workbook::open(destination)?;
    let cells = cells_by_address(&reread);
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

/// The header and totals cells of an Excel table are repeated in `tableN.xml` as
/// column names and totals labels (and header names in structured references), so
/// an edit to the cell alone makes Excel repair the table.
pub fn ensure_not_table_label(sheet: &Value, column: u32, row: u32) -> Result<()> {
    for table in sheet["tables"].as_array().into_iter().flatten() {
        let area = Area::parse(string(&table["range"])?)?;
        let (Some((first_column, last_column)), Some((first_row, last_row))) =
            (area.columns, area.rows)
        else {
            continue;
        };
        let header_rows = u32::try_from(table["header_rows"].as_u64().unwrap_or(1))?;
        let totals_rows = u32::try_from(table["totals_rows"].as_u64().unwrap_or(0))?;
        ensure!(
            !(first_column..=last_column).contains(&column)
                || (first_row + header_rows..=last_row.saturating_sub(totals_rows)).contains(&row)
                || !(first_row..=last_row).contains(&row),
            "{}!{}{row} is a header or totals cell of table {}; Excel keeps those names in the table definition, so rename columns and totals labels in Excel",
            string(&sheet["name"])?,
            column_name(column)?,
            string(&table["name"])?
        );
    }
    Ok(())
}

/// Whether `text` holds a reference qualified with `sheet` (`Data!` or `'My Data'!`).
fn names_sheet(text: &str, sheet: &str) -> bool {
    let quoted = format!("'{}'!", sheet.replace('\'', "''"));
    let plain = format!("{sheet}!");
    text.contains(&quoted)
        || text.match_indices(&plain).any(|(at, _)| {
            !text[..at]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | '\''))
        })
}

/// The formatting Excel gives a cell ARP creates: an inserted row's cell is
/// formatted like the cell above it and an inserted column's like the cell to its
/// left; otherwise the row's format applies, then the column's.
pub(super) struct CellFormats<'a> {
    cells: BTreeMap<(u32, u32), String>,
    columns: Vec<(u32, u32, String)>,
    operations: Vec<&'a StructuralOperation>,
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
        })
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
        if let (Some(source_row), Some(source_column)) =
            (self.source(row.number, true), source_column)
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
