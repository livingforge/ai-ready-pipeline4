//! Column permutations on plain worksheets. Positional features that do not
//! have a proven permutation writer are refused before the operation is saved.
use super::*;

impl Workbook {
    pub(super) fn ensure_column_moves_supported(
        &self,
        operations: &[StructuralOperation],
    ) -> Result<()> {
        self.ensure_structural_parts_supported(operations)?;
        ensure!(
            operations
                .iter()
                .all(|o| matches!(o.kind, OperationKind::MoveColumns { .. })),
            "apply other structural operations before moving columns"
        );
        self.ensure_move_parts()?;
        for sheet in &self.sheets {
            let source = std::str::from_utf8(&self.parts[string(&sheet["part"])?])?;
            let doc = Document::parse(source)?;
            for node in doc.root_element().children().filter(Node::is_element) {
                ensure!(
                    matches!(
                        node.tag_name().name(),
                        "sheetPr"
                            | "dimension"
                            | "sheetViews"
                            | "sheetFormatPr"
                            | "cols"
                            | "sheetData"
                            | "pageMargins"
                            | "pageSetup"
                            | "printOptions"
                            | "headerFooter"
                    ),
                    "column move is not supported with worksheet feature {}; edit in Excel",
                    node.tag_name().name()
                );
            }
            for cell in doc.descendants().filter(|n| n.has_tag_name((NS, "c"))) {
                if let Some(formula) = child(cell, "f") {
                    ensure!(
                        formula.attributes().len() == 0,
                        "column move requires ordinary formulas, not shared/array formulas"
                    );
                    let text = formula.text().unwrap_or("");
                    ensure!(
                        !text.to_ascii_uppercase().contains("INDIRECT(")
                            && !text.to_ascii_uppercase().contains("OFFSET("),
                        "column move cannot prove dynamic references"
                    );
                    rewrite_references(text, Some(string(&sheet["name"])?), operations)?;
                }
            }
        }
        Ok(())
    }

    fn ensure_move_parts(&self) -> Result<()> {
        ensure!(
            !self.parts.keys().any(|p| p.starts_with("_xmlsignatures/")
                || p.starts_with("xl/calcChain")
                || p.starts_with("xl/tables/")
                || p.starts_with("xl/pivot")
                || p.starts_with("xl/drawings/")
                || p.starts_with("xl/charts/")
                || p.starts_with("xl/comments")
                || p.starts_with("xl/threadedComments/")
                || p.starts_with("xl/activeX/")
                || p.ends_with("vbaProject.bin")),
            "column move with positional objects, macros or calculation chain is unsupported; edit in Excel"
        );
        let doc = xml(&self.parts["xl/workbook.xml"])?;
        ensure!(
            !doc.descendants()
                .any(|n| n.has_tag_name((NS, "definedName"))),
            "column move with defined names is unsupported; edit in Excel"
        );
        Ok(())
    }

    pub(super) fn patch_column_moves(
        &self,
        destination: &Path,
        operations: &[StructuralOperation],
        changes: &[Value],
        formulas: &[Value],
        shapes: &[Value],
    ) -> Result<Value> {
        self.ensure_column_moves_supported(operations)?;
        ensure!(shapes.is_empty(), "column move with shapes is unsupported");
        let edits = self.formula_edits(formulas)?;
        let mut patched = BTreeMap::new();
        for sheet in &self.sheets {
            let name = string(&sheet["name"])?;
            let part = string(&sheet["part"])?;
            let source = std::str::from_utf8(&self.parts[part])?;
            let doc = Document::parse(source)?;
            let mut replacements = vec![];
            for row in doc.descendants().filter(|n| n.has_tag_name((NS, "row"))) {
                let mut cells = vec![];
                let mut row_changed = false;
                for cell in row.children().filter(Node::is_element) {
                    ensure!(
                        cell.has_tag_name((NS, "c")),
                        "unsupported row feature during column move"
                    );
                    let old = cell.attribute("r").context("cell address missing")?;
                    let address =
                        map_coordinate(name, old, operations)?.context("move deleted a cell")?;
                    row_changed |= address != old;
                    let mut raw = if let Some(change) = changes
                        .iter()
                        .find(|v| v["sheet"] == name && v["cell"] == address)
                    {
                        row_changed = true;
                        render_cell(source, cell, &address, &change["after"], &self.parts)?
                    } else if address == old {
                        source[cell.range()].to_owned()
                    } else {
                        let raw = &source[cell.range()];
                        let (opening, _) = xml_opening(raw)?;
                        format!(
                            "{}{}",
                            replace_xml_attribute(opening, "r", &address)?,
                            &raw[opening.len()..]
                        )
                    };
                    if let Some(formula) = child(cell, "f") {
                        let original = edits
                            .get(name)
                            .and_then(|v| v.get(old))
                            .map(String::as_str)
                            .unwrap_or(formula.text().unwrap_or(""));
                        let rewritten = rewrite_references(original, Some(name), operations)?;
                        row_changed |= rewritten != formula.text().unwrap_or("");
                        let raw_doc_text = format!(
                            "<root {}>{raw}</root>",
                            doc.root_element()
                                .namespaces()
                                .map(|n| match n.name() {
                                    Some(prefix) =>
                                        format!("xmlns:{prefix}=\"{}\"", xml_attr(n.uri())),
                                    None => format!("xmlns=\"{}\"", xml_attr(n.uri())),
                                })
                                .collect::<Vec<_>>()
                                .join(" ")
                        );
                        let raw_doc = Document::parse(&raw_doc_text)?;
                        let f = raw_doc
                            .descendants()
                            .find(|n| n.has_tag_name((NS, "f")))
                            .context("formula disappeared")?;
                        let prefix_length =
                            raw_doc_text.find('>').context("root opening missing")? + 1;
                        let old_xml = &raw_doc_text[f.range()];
                        let (opening, _) = xml_opening(old_xml)?;
                        let tag = f.tag_name().name();
                        let qualified = opening
                            .trim_start_matches('<')
                            .split([' ', '>'])
                            .next()
                            .unwrap_or(tag);
                        raw.replace_range(
                            f.range().start - prefix_length..f.range().end - prefix_length,
                            &format!(
                                "{opening}{}</{}>",
                                crate::document_source::xml_text(&rewritten)?,
                                qualified
                            ),
                        );
                    }
                    cells.push((coordinate(&address)?.0, raw));
                }
                cells.sort_by_key(|(column, _)| *column);
                let original = &source[row.range()];
                let (opening, _) = xml_opening(original)?;
                if row_changed && !cells.is_empty() {
                    let closing = original.rfind("</").context("row closing missing")?;
                    replacements.push((
                        row.range(),
                        format!(
                            "{}{}{}",
                            opening,
                            cells.into_iter().map(|(_, s)| s).collect::<String>(),
                            &original[closing..]
                        ),
                    ));
                }
            }
            let own = sheet_operations(name, operations);
            if !own.is_empty()
                && let Some(cols) = child(doc.root_element(), "cols")
            {
                let mut moved = vec![];
                for col in cols.children().filter(Node::is_element) {
                    let first: u32 = col
                        .attribute("min")
                        .context("column min missing")?
                        .parse()?;
                    let last: u32 = col
                        .attribute("max")
                        .context("column max missing")?
                        .parse()?;
                    ensure!(last <= 16384 && first <= last, "invalid column span");
                    for column in first..=last {
                        let address = map_coordinate(
                            name,
                            &format!("{}1", column_name(column)?),
                            operations,
                        )?
                        .context("move deleted column")?;
                        let target = coordinate(&address)?.0;
                        let raw = &source[col.range()];
                        let raw = replace_xml_attribute(raw, "min", &target.to_string())?;
                        moved.push((
                            target,
                            replace_xml_attribute(&raw, "max", &target.to_string())?,
                        ));
                    }
                }
                moved.sort_by_key(|(column, _)| *column);
                let raw = &source[cols.range()];
                let (opening, _) = xml_opening(raw)?;
                if !moved.is_empty() {
                    replacements.push((
                        cols.range(),
                        format!(
                            "{}{}{}",
                            opening,
                            moved.into_iter().map(|(_, s)| s).collect::<String>(),
                            &raw[raw.rfind("</").context("cols closing missing")?..]
                        ),
                    ));
                }
            }
            if let Some(dimension) = child(doc.root_element(), "dimension") {
                let mut bounds: Option<(u32, u32, u32, u32)> = None;
                for cell in doc.descendants().filter(|n| n.has_tag_name((NS, "c"))) {
                    let address = map_coordinate(
                        name,
                        cell.attribute("r").context("cell address missing")?,
                        operations,
                    )?
                    .context("move deleted cell")?;
                    let (column, row) = coordinate(&address)?;
                    bounds = Some(match bounds {
                        Some((c1, r1, c2, r2)) => {
                            (c1.min(column), r1.min(row), c2.max(column), r2.max(row))
                        }
                        None => (column, row, column, row),
                    });
                }
                if let Some((c1, r1, c2, r2)) = bounds {
                    let from = format!("{}{r1}", column_name(c1)?);
                    let to = format!("{}{r2}", column_name(c2)?);
                    let range = if from == to {
                        from
                    } else {
                        format!("{from}:{to}")
                    };
                    if dimension.attribute("ref") != Some(range.as_str()) {
                        replacements.push((
                            dimension.range(),
                            replace_xml_attribute(&source[dimension.range()], "ref", &range)?,
                        ));
                    }
                }
            }
            if !replacements.is_empty() {
                patched.insert(
                    part.to_owned(),
                    splice(source, replacements, "column move")?.into_bytes(),
                );
            }
        }
        patched.insert(
            "xl/workbook.xml".to_owned(),
            super::writeback::request_full_calculation(std::str::from_utf8(
                &self.parts["xl/workbook.xml"],
            )?)?
            .into_bytes(),
        );
        write_archive(&self.raw, destination, &patched)?;
        let reread = Workbook::open(destination)?;
        ensure!(
            self.parts.len() == reread.parts.len(),
            "column move changed package part count"
        );
        for (part, before) in &self.parts {
            ensure!(
                reread.parts.get(part) == Some(patched.get(part).unwrap_or(before)),
                "column move package preservation failed: {part}"
            );
        }
        for (sheet_index, sheet) in self.sheets.iter().enumerate() {
            let name = string(&sheet["name"])?;
            ensure!(
                self.cells[sheet_index].len() == reread.cells[sheet_index].len(),
                "column move cell count changed"
            );
            for cell in &self.cells[sheet_index] {
                let address = map_coordinate(name, &cell.address, operations)?
                    .context("move deleted cell")?;
                let written = reread.cells[sheet_index]
                    .iter()
                    .find(|c| c.address == address)
                    .context("moved cell missing")?;
                if let Some(formula) = &cell.formula {
                    let original = edits
                        .get(name)
                        .and_then(|v| v.get(&cell.address))
                        .unwrap_or(formula);
                    ensure!(
                        written.formula.as_deref()
                            == Some(rewrite_references(original, Some(name), operations)?.as_str()),
                        "moved formula mismatch"
                    );
                } else {
                    let expected = changes
                        .iter()
                        .find(|c| c["sheet"] == name && c["cell"] == address)
                        .map(|c| &c["after"])
                        .unwrap_or(&cell.value);
                    ensure!(written.value == *expected, "moved cell value mismatch");
                }
            }
        }
        Ok(
            json!({"changed_parts":patched.keys().collect::<Vec<_>>(),"layout_review_required":true,"requires_excel_recalculation":true,"identity_writeback_verified":true}),
        )
    }
}
