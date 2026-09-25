//! Workbook parts that hold cell positions of edited sheets: Excel tables,
//! pivot sources and locations, defined names, chart series and the
//! calculation chain. Row/column operations update them together with cells.
use super::*;

type Changes = BTreeMap<(String, u32, u32), Value>;
/// Formulas for new cells, keyed like changes: (sheet, row, column).
pub(super) type NewFormulas = BTreeMap<(String, u32, u32), String>;

/// Parts related to `base` by a relationship type ending with `kind`.
fn related_parts(parts: &BTreeMap<String, Vec<u8>>, base: &str, kind: &str) -> Result<Vec<String>> {
    let rels = relationships_part(base)?;
    let Some(bytes) = parts.get(&rels) else {
        return Ok(vec![]);
    };
    let doc = xml(bytes)?;
    let mut related = vec![];
    for rel in doc
        .descendants()
        .filter(|n| n.has_tag_name((PKG_REL, "Relationship")))
    {
        if rel.attribute("TargetMode") == Some("External")
            || !rel.attribute("Type").is_some_and(|t| t.ends_with(kind))
        {
            continue;
        }
        let target = relationship_target(base, rel.attribute("Target").context("missing target")?)?;
        ensure!(
            parts.contains_key(&target),
            "missing related part: {target}"
        );
        related.push(target);
    }
    Ok(related)
}

fn text_of(bytes: &[u8]) -> Result<&str> {
    Ok(std::str::from_utf8(bytes)?)
}

/// Rewrites a table column's formulas, returning its XML and the rewritten
/// calculated-column formula. `data_shift` re-bases that formula (written for
/// the first data row) when the first data rows are deleted.
fn rewrite_table_column(
    original: &str,
    node: Node<'_, '_>,
    sheet: &str,
    moves: &Moves<'_>,
    data_shift: i64,
) -> Result<(String, Option<String>)> {
    let base = node.range().start;
    let mut edits = vec![];
    let mut calculated = None;
    for formula in node.descendants().filter(|n| {
        n.is_element()
            && matches!(
                n.tag_name().name(),
                "calculatedColumnFormula" | "totalsRowFormula"
            )
    }) {
        let (Some(text), Some(text_node)) =
            (formula.text(), formula.children().find(Node::is_text))
        else {
            continue;
        };
        let is_calculated = formula.tag_name().name() == "calculatedColumnFormula";
        let shifted = if is_calculated && data_shift != 0 {
            shift_relative(text, data_shift, 0)?
        } else {
            text.to_owned()
        };
        let rewritten = moves.rewrite(&shifted, Some(sheet))?;
        if is_calculated && formula.attribute("array") != Some("1") {
            calculated = Some(rewritten.clone());
        }
        if rewritten != text {
            let range = text_node.range();
            edits.push((range.start - base..range.end - base, xml_attr(&rewritten)));
        }
    }
    Ok((
        apply_edits(&original[node.range()], edits, vec![])?,
        calculated,
    ))
}

/// Columns deleted from Excel tables on edited sheets, by lowercase table name.
pub(super) fn removed_table_columns(
    parts: &BTreeMap<String, Vec<u8>>,
    sheets: &[Value],
    operations: &[StructuralOperation],
) -> Result<BTreeMap<String, BTreeSet<String>>> {
    let mut removed: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for sheet in sheets {
        let own = sheet_operations(string(&sheet["name"])?, operations);
        if !own.iter().any(|o| !o.row_operation() && !o.insertion()) {
            continue;
        }
        for part in related_parts(parts, string(&sheet["part"])?, "/table")? {
            let doc = xml(&parts[&part])?;
            let root = doc.root_element();
            let Some(name) = root
                .attribute("displayName")
                .or_else(|| root.attribute("name"))
            else {
                continue;
            };
            let area = Area::parse(root.attribute("ref").context("table without ref")?)?;
            let first = area.first_column();
            let columns = child(root, "tableColumns").context("table without tableColumns")?;
            for (offset, column) in columns.children().filter(Node::is_element).enumerate() {
                let index = first + offset as u32;
                if map_span(index, index, &own, false)?.is_none()
                    && let Some(column_name) = column.attribute("name")
                {
                    removed
                        .entry(name.to_lowercase())
                        .or_default()
                        .insert(column_name.to_lowercase());
                }
            }
        }
    }
    Ok(removed)
}

/// Moves Excel tables on edited sheets. Columns inserted inside a table become
/// new table columns whose header cells are written (from the user's value when
/// present); deleted columns are removed. Rows inserted inside a table receive
/// its calculated-column formulas, as Excel fills them. Deleting a header or
/// totals row, the last data row or the whole table is rejected, as Excel would.
pub(super) fn relocate_tables(
    parts: &BTreeMap<String, Vec<u8>>,
    sheets: &[Value],
    moves: &Moves<'_>,
    changes: &mut Changes,
    formulas: &mut NewFormulas,
    patched: &mut BTreeMap<String, Vec<u8>>,
) -> Result<()> {
    for sheet in sheets {
        let sheet_name = string(&sheet["name"])?;
        let own = sheet_operations(sheet_name, moves.operations);
        if own.is_empty() {
            continue;
        }
        for part in related_parts(parts, string(&sheet["part"])?, "/table")? {
            let updated = relocate_table(
                text_of(&parts[&part])?,
                sheet_name,
                moves,
                &own,
                changes,
                formulas,
            )?;
            patched.insert(part, updated.into_bytes());
        }
    }
    Ok(())
}

fn relocate_table(
    original: &str,
    sheet: &str,
    moves: &Moves<'_>,
    own: &[&StructuralOperation],
    changes: &mut Changes,
    formulas: &mut NewFormulas,
) -> Result<String> {
    let doc = xml(original.as_bytes())?;
    let root = doc.root_element();
    let name = root
        .attribute("displayName")
        .or_else(|| root.attribute("name"))
        .unwrap_or("table");
    let area = Area::parse(root.attribute("ref").context("table without ref")?)?;
    let (Some((first_column, last_column)), Some((first_row, last_row))) =
        (area.columns, area.rows)
    else {
        bail!("table {name} must cover a cell range");
    };
    let header_rows: u32 = root.attribute("headerRowCount").unwrap_or("1").parse()?;
    let totals_rows: u32 = root.attribute("totalsRowCount").unwrap_or("0").parse()?;
    let mapped = map_area(area, own)?.with_context(|| {
        format!("row/column changes would delete table {name}; delete it in Excel")
    })?;
    if header_rows > 0 {
        ensure!(
            map_span(first_row, first_row, own, true)?.is_some(),
            "row/column changes cannot delete the header row of table {name}"
        );
    }
    if totals_rows > 0 {
        ensure!(
            map_span(last_row, last_row, own, true)?.is_some(),
            "row/column changes cannot delete the totals row of table {name}"
        );
    }
    let (new_first_row, new_last_row) = mapped.rows.context("table rows")?;
    ensure!(
        new_last_row - new_first_row + 1 > header_rows + totals_rows,
        "row/column changes would leave table {name} without data rows"
    );
    let (new_first_column, new_last_column) = mapped.columns.context("table columns")?;
    let columns = child(root, "tableColumns").context("table without tableColumns")?;
    let old: Vec<_> = columns.children().filter(Node::is_element).collect();
    ensure!(
        old.len() as u32 == last_column - first_column + 1,
        "table {name} column count does not match its range"
    );
    let positions = (first_column..=last_column)
        .map(|column| Ok(map_span(column, column, own, false)?.map(|(p, _)| p)))
        .collect::<Result<Vec<_>>>()?;
    let mut names: BTreeSet<String> = old
        .iter()
        .zip(&positions)
        .filter(|(_, position)| position.is_some())
        .filter_map(|(column, _)| column.attribute("name"))
        .map(str::to_lowercase)
        .collect();
    let mut next_id = old
        .iter()
        .filter_map(|c| c.attribute("id")?.parse::<u32>().ok())
        .max()
        .unwrap_or(0)
        + 1;
    let prefix = element_prefix(&original[old[0].range()])?;
    // Calculated-column formulas are written for the first data row; re-express
    // them from the old row that becomes the new first data row.
    let old_first_data = first_row + header_rows;
    let mut surviving_first_data = old_first_data;
    while map_span(surviving_first_data, surviving_first_data, own, true)?.is_none() {
        surviving_first_data += 1;
    }
    let data_shift = i64::from(surviving_first_data - old_first_data);
    let new_first_data = new_first_row + header_rows;
    let new_last_data = new_last_row - totals_rows;
    let old_rows: BTreeSet<u32> = (first_row..=last_row)
        .map(|row| Ok(map_span(row, row, own, true)?.map(|(r, _)| r)))
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect();
    let inserted_rows: Vec<u32> = (new_first_data..=new_last_data)
        .filter(|row| !old_rows.contains(row))
        .collect();
    let mut inner = String::new();
    for column in new_first_column..=new_last_column {
        if let Some(index) = positions.iter().position(|p| *p == Some(column)) {
            let (xml, calculated) =
                rewrite_table_column(original, old[index], sheet, moves, data_shift)?;
            inner.push_str(&xml);
            if let Some(calculated) = calculated {
                for row in &inserted_rows {
                    let formula = shift_relative(&calculated, i64::from(row - new_first_data), 0)?;
                    formulas.insert((sheet.to_owned(), *row, column), formula);
                }
            }
            continue;
        }
        let header = (sheet.to_owned(), new_first_row, column);
        let column_name = match (header_rows > 0).then(|| changes.get(&header)).flatten() {
            Some(Value::String(text)) => text.clone(),
            Some(_) => bail!("the header of a new column in table {name} must be text"),
            None => (1..)
                .map(|n| format!("Column{n}"))
                .find(|candidate| !names.contains(&candidate.to_lowercase()))
                .unwrap(),
        };
        ensure!(
            names.insert(column_name.to_lowercase()),
            "table {name} would have duplicate column name {column_name}"
        );
        if header_rows > 0 {
            changes.entry(header).or_insert_with(|| json!(column_name));
        }
        inner.push_str(&format!(
            r#"<{prefix}tableColumn id="{next_id}" name="{}"/>"#,
            xml_attr(&column_name)
        ));
        next_id += 1;
    }
    let mut attributes = AttributeEdits::default();
    attributes.set(original, root, "ref", &mapped.render()?)?;
    let mut edits = vec![];
    let columns_open = opening_range(original, columns)?;
    let columns_close = original[..columns.range().end]
        .rfind("</")
        .context("tableColumns has no closing tag")?;
    let count = (new_last_column - new_first_column + 1).to_string();
    let opening = &original[columns_open.clone()];
    let opening = if columns.attribute("count").is_some() {
        replace_xml_attribute(opening, "count", &count)?
    } else {
        opening.to_owned()
    };
    edits.push((
        columns_open.start..columns_close,
        format!("{opening}{inner}"),
    ));
    let mut removals = vec![];
    for node in root
        .descendants()
        .filter(|n| n.is_element() && !n.ancestors().any(|a| a == columns))
    {
        match node.tag_name().name() {
            "autoFilter" | "sortState" | "sortCondition" => {
                if let Some(reference) = node.attribute("ref") {
                    let area = Area::parse(reference)?;
                    match map_area(area, own)? {
                        Some(new) if new != area => {
                            attributes.set(original, node, "ref", &new.render()?)?
                        }
                        Some(_) => {}
                        None => removals.push(node.range()),
                    }
                }
            }
            "filterColumn" => {
                filter_column_edit(original, node, own, &mut removals, &mut attributes)?
            }
            _ => {}
        }
    }
    edits.extend(attributes.into_edits());
    apply_edits(original, edits, removals)
}

fn element_prefix(raw: &str) -> Result<String> {
    let tag = raw[1..]
        .split([' ', '\t', '\r', '\n', '/', '>'])
        .next()
        .context("invalid XML element")?;
    Ok(tag
        .rsplit_once(':')
        .map(|(p, _)| format!("{p}:"))
        .unwrap_or_default())
}

/// Moves pivot cache source ranges and pivot tables placed on edited sheets,
/// as Excel does: the source range follows the cells and the cache is not
/// refreshed. Deleting the whole source or cutting through a pivot table is
/// rejected.
pub(super) fn relocate_pivots(
    parts: &BTreeMap<String, Vec<u8>>,
    sheets: &[Value],
    operations: &[StructuralOperation],
    patched: &mut BTreeMap<String, Vec<u8>>,
) -> Result<()> {
    for (part, bytes) in parts {
        if !(part.starts_with("xl/pivotCache/pivotCacheDefinition") && part.ends_with(".xml")) {
            continue;
        }
        let original = text_of(bytes)?;
        let doc = xml(bytes)?;
        let mut attributes = AttributeEdits::default();
        for source in doc
            .descendants()
            .filter(|n| n.has_tag_name((NS, "worksheetSource")))
        {
            let (Some(sheet), Some(reference)) =
                (source.attribute("sheet"), source.attribute("ref"))
            else {
                continue;
            };
            let own = sheet_operations(sheet, operations);
            if own.is_empty() {
                continue;
            }
            let area = Area::parse(reference)?;
            let mapped = map_area(area, &own)?.with_context(|| {
                format!("row/column changes would delete the source of pivot cache {part}")
            })?;
            if mapped != area {
                attributes.set(original, source, "ref", &mapped.render()?)?;
            }
        }
        let edits: Vec<_> = attributes.into_edits().collect();
        if !edits.is_empty() {
            patched.insert(
                part.clone(),
                apply_edits(original, edits, vec![])?.into_bytes(),
            );
        }
    }
    for sheet in sheets {
        let name = string(&sheet["name"])?;
        let own = sheet_operations(name, operations);
        if own.is_empty() {
            continue;
        }
        for part in related_parts(parts, string(&sheet["part"])?, "/pivotTable")? {
            let original = text_of(&parts[&part])?;
            let doc = xml(original.as_bytes())?;
            let root = doc.root_element();
            let location = child(root, "location").context("pivot table without location")?;
            let area = Area::parse(
                location
                    .attribute("ref")
                    .context("pivot location without ref")?,
            )?;
            let pivot = root.attribute("name").unwrap_or(&part);
            let mapped = map_area(area, &own)?
                .filter(|mapped| same_size(area, *mapped))
                .with_context(|| {
                    format!("row/column changes cannot cut through pivot table {pivot} on {name}; change it in Excel")
                })?;
            if mapped != area {
                let mut attributes = AttributeEdits::default();
                attributes.set(original, location, "ref", &mapped.render()?)?;
                let updated = apply_edits(original, attributes.into_edits().collect(), vec![])?;
                patched.insert(part, updated.into_bytes());
            }
        }
    }
    Ok(())
}

/// Rewrites defined names (including print areas and print titles).
pub(super) fn relocate_defined_names(workbook: &str, moves: &Moves<'_>) -> Result<Option<String>> {
    let doc = xml(workbook.as_bytes())?;
    let mut edits = vec![];
    for name in doc
        .descendants()
        .filter(|n| n.has_tag_name((NS, "definedName")))
    {
        if let (Some(text), Some(text_node)) = (name.text(), name.children().find(Node::is_text)) {
            let rewritten = moves.rewrite(text, None)?;
            if rewritten != text {
                edits.push((text_node.range(), xml_attr(&rewritten)));
            }
        }
    }
    (!edits.is_empty())
        .then(|| apply_edits(workbook, edits, vec![]))
        .transpose()
}

/// Rewrites chart series and label references (`c:f`, chartex `cx:f`).
/// Cached values stay; Excel refreshes them from the cells on open.
pub(super) fn relocate_charts(
    parts: &BTreeMap<String, Vec<u8>>,
    moves: &Moves<'_>,
    patched: &mut BTreeMap<String, Vec<u8>>,
) -> Result<()> {
    for (part, bytes) in parts {
        if !(part.starts_with("xl/charts/") && part.ends_with(".xml")) || part[10..].contains('/') {
            continue;
        }
        let original = text_of(bytes)?;
        let doc = xml(bytes)?;
        let mut edits = vec![];
        for formula in doc
            .descendants()
            .filter(|n| n.is_element() && n.tag_name().name() == "f")
        {
            if let (Some(text), Some(text_node)) =
                (formula.text(), formula.children().find(Node::is_text))
            {
                let rewritten = moves.rewrite(text, None)?;
                if rewritten != text {
                    edits.push((text_node.range(), xml_attr(&rewritten)));
                }
            }
        }
        if !edits.is_empty() {
            patched.insert(
                part.clone(),
                apply_edits(original, edits, vec![])?.into_bytes(),
            );
        }
    }
    Ok(())
}

/// The calculation chain lists formula cells by address; after cells move it
/// no longer matches and Excel reports the file as damaged. Excel rebuilds it.
pub(super) fn drop_calc_chain(
    parts: &BTreeMap<String, Vec<u8>>,
    patched: &mut BTreeMap<String, Vec<u8>>,
    removed: &mut BTreeSet<String>,
) -> Result<()> {
    const PART: &str = "xl/calcChain.xml";
    if !parts.contains_key(PART) {
        return Ok(());
    }
    removed.insert(PART.to_owned());
    for (name, element, matches) in [
        (
            "xl/_rels/workbook.xml.rels",
            (PKG_REL, "Relationship"),
            (|n: Node<'_, '_>| {
                n.attribute("Type")
                    .is_some_and(|t| t.ends_with("/calcChain"))
            }) as fn(Node<'_, '_>) -> bool,
        ),
        (
            "[Content_Types].xml",
            (CONTENT_TYPES, "Override"),
            |n: Node<'_, '_>| n.attribute("PartName") == Some("/xl/calcChain.xml"),
        ),
    ] {
        let current = match patched.get(name).or_else(|| parts.get(name)) {
            Some(bytes) => text_of(bytes)?.to_owned(),
            None => continue,
        };
        let doc = xml(current.as_bytes())?;
        let removals: Vec<_> = doc
            .descendants()
            .filter(|n| n.has_tag_name(element) && matches(*n))
            .map(|n| n.range())
            .collect();
        if !removals.is_empty() {
            let updated = apply_edits(&current, vec![], removals)?;
            patched.insert(name.to_owned(), updated.into_bytes());
        }
    }
    Ok(())
}
