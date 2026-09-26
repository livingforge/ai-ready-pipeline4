use super::*;

pub(super) struct RowOutput {
    pub(super) number: u32,
    pub(super) raw: String,
    pub(super) cells: BTreeSet<u32>,
}

impl RowOutput {
    /// Inserts a cell before the first cell of a later column; Excel requires
    /// cells in column order.
    pub(super) fn insert_cell(&mut self, cell: &str, column: u32) -> Result<()> {
        static CELL: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
            regex::Regex::new(r#"<(?:[A-Za-z_][\w.-]*:)?c\s[^>]*?\br\s*=\s*["']([A-Z]+)[0-9]+["']"#)
                .unwrap()
        });
        if self.raw.ends_with("/>") {
            // Excel writes a formatted row without cells as `<row .../>`.
            let tag = element_tag(&self.raw)?.to_owned();
            self.raw = format!("{}></{tag}>", self.raw.trim_end_matches("/>").trim_end());
        }
        let mut position = self.raw.rfind("</").context("row has no closing tag")?;
        for captures in CELL.captures_iter(&self.raw) {
            if column_number(&captures[1])? > column {
                position = captures.get(0).unwrap().start();
                break;
            }
        }
        self.raw.insert_str(position, cell);
        self.cells.insert(column);
        Ok(())
    }

    /// The row's own style (`s` with `customFormat`), which Excel applies to
    /// cells of the row that have no format of their own.
    pub(super) fn custom_style(&self) -> Option<String> {
        let (opening, _) = xml_opening(&self.raw).ok()?;
        let attribute = |name: &str| xml_attribute_value(opening, name).ok().flatten();
        matches!(attribute("customFormat"), Some("1" | "true"))
            .then(|| attribute("s").map(str::to_owned))
            .flatten()
    }

    /// The namespace prefix used by this row's elements (e.g. `x:`).
    pub(super) fn prefix(&self) -> &str {
        let tag = self.raw[1..]
            .split([' ', '\t', '\r', '\n', '/', '>'])
            .next()
            .unwrap_or("row");
        tag.strip_suffix("row").unwrap_or("")
    }
}

pub(super) fn scalar_body(prefix: &str, value: &Value) -> Result<String> {
    Ok(match value {
        Value::Null => ">".to_owned(),
        Value::String(text) => {
            ensure!(
                text.encode_utf16().count() <= 32767
                    && !text
                        .chars()
                        .any(|c| c < ' ' && !matches!(c, '\t' | '\n' | '\r')),
                "unsupported Excel string"
            );
            let text = encode_xstring(text)
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
                .replace('\r', "&#13;");
            format!(
                " t=\"inlineStr\"><{prefix}is><{prefix}t xml:space=\"preserve\">{text}</{prefix}t></{prefix}is>"
            )
        }
        Value::Bool(value) => format!(
            " t=\"b\"><{prefix}v>{}</{prefix}v>",
            if *value { 1 } else { 0 }
        ),
        Value::Number(number) => format!(" t=\"n\"><{prefix}v>{number}</{prefix}v>"),
        _ => bail!("scalar required"),
    })
}

pub(super) fn render_cell(raw: &str, address: &str, value: &Value) -> Result<String> {
    let (opening, _) = xml_opening(raw)?;
    let tag = opening[1..]
        .split([' ', '\t', '\r', '\n', '/', '>'])
        .next()
        .context("invalid cell tag")?;
    let prefix = tag.strip_suffix('c').context("invalid cell tag")?;
    let mut opening = remove_xml_attribute(opening, "t")?;
    opening = opening
        .trim_end_matches('/')
        .trim_end_matches('>')
        .to_owned();
    opening = replace_xml_attribute(&format!("{opening}>"), "r", address)?;
    opening = opening.trim_end_matches('>').to_owned();
    Ok(format!("{opening}{}</{tag}>", scalar_body(prefix, value)?))
}

/// A cell ARP creates, in the sheet's namespace `prefix`, with `style` (`s`).
pub(super) fn render_new_cell(
    address: &str,
    value: &Value,
    prefix: &str,
    style: Option<&str>,
) -> Result<String> {
    let style = style
        .map(|s| format!(" s=\"{}\"", xml_attr(s)))
        .unwrap_or_default();
    Ok(format!(
        "<{prefix}c r=\"{address}\"{style}{}</{prefix}c>",
        scalar_body(prefix, value)?
    ))
}

pub(super) struct RowTransform<'a, 'b> {
    pub(super) original: &'a str,
    pub(super) original_row: u32,
    pub(super) final_row: u32,
    pub(super) sheet: &'a str,
    pub(super) operations: &'b [StructuralOperation],
    pub(super) moves: &'b Moves<'b>,
    pub(super) changes: &'b BTreeMap<(u32, u32), Value>,
    pub(super) recalc: bool,
}

pub(super) fn transform_row(
    row: Node<'_, '_>,
    raw: &str,
    context: &RowTransform<'_, '_>,
) -> Result<RowOutput> {
    let mut edits: Vec<(std::ops::Range<usize>, String)> = vec![];
    let row_start = row.range().start;
    let row_open_end = raw.find('>').context("invalid row")? + 1;
    let mut row_opening = raw[..row_open_end].to_owned();
    if context.original_row != context.final_row {
        row_opening = replace_xml_attribute(&row_opening, "r", &context.final_row.to_string())?;
        edits.push((0..row_open_end, row_opening));
    }
    let mut cells = BTreeSet::new();
    for cell in row.children().filter(|node| node.has_tag_name((NS, "c"))) {
        let address = cell.attribute("r").context("missing cell address")?;
        let (column, row_number) = coordinate(address)?;
        ensure!(row_number == context.original_row, "cell/row mismatch");
        let cell_range = cell.range();
        let local_range = cell_range.start - row_start..cell_range.end - row_start;
        let Some(final_column) = transform_index(column, context.operations, false) else {
            edits.push((local_range, String::new()));
            continue;
        };
        ensure!(
            final_column <= MAX_COLUMN,
            "inserting columns in {} would push column {} past the last column of the sheet (XFD); delete columns at the right first",
            context.sheet,
            column_name(column)?
        );
        let final_address = format!("{}{}", column_name(final_column)?, context.final_row);
        cells.insert(final_column);
        if let Some(value) = context.changes.get(&(context.final_row, final_column)) {
            edits.push((
                local_range,
                render_cell(&context.original[cell_range], &final_address, value)?,
            ));
        } else {
            if final_address != address {
                let cell_raw = &context.original[cell_range.clone()];
                let (opening, _) = xml_opening(cell_raw)?;
                let new_opening = replace_xml_attribute(opening, "r", &final_address)?;
                edits.push((
                    cell_range.start - row_start..cell_range.start - row_start + opening.len(),
                    new_opening,
                ));
            }
            if context.recalc && child(cell, "f").is_some() {
                if let Some(cache) = child(cell, "v") {
                    edits.push((
                        cache.range().start - row_start..cache.range().end - row_start,
                        String::new(),
                    ));
                }
                if let Some(formula) = child(cell, "f")
                    && let Some(text) = formula.text()
                    && let Some(text_node) = formula.children().find(Node::is_text)
                {
                    edits.push((
                        text_node.range().start - row_start..text_node.range().end - row_start,
                        xml_attr(&context.moves.rewrite(text, Some(context.sheet))?),
                    ));
                }
                if let Some(formula) = child(cell, "f")
                    && formula.attribute("t") == Some("array")
                    && let Some(reference) = formula.attribute("ref")
                {
                    let area = Area::parse(reference)?;
                    let own = sheet_operations(context.sheet, context.moves.operations);
                    let mapped = map_area(area, &own)?
                        .filter(|mapped| same_size(area, *mapped))
                        .with_context(|| {
                            format!(
                                "row/column changes cannot cut through array formula {}!{reference}; change it in Excel",
                                context.sheet
                            )
                        })?;
                    let (opening, _) = xml_opening(&context.original[formula.range()])?;
                    edits.push((
                        formula.range().start - row_start
                            ..formula.range().start - row_start + opening.len(),
                        replace_xml_attribute(opening, "ref", &mapped.render()?)?,
                    ));
                }
                // A What-If data table keeps its range and input cells as
                // attributes of the formula in its top-left cell.
                if let Some(formula) = child(cell, "f")
                    && formula.attribute("t") == Some("dataTable")
                {
                    let (opening, _) = xml_opening(&context.original[formula.range()])?;
                    let mut rewritten = opening.to_owned();
                    if let Some(reference) = formula.attribute("ref") {
                        let area = Area::parse(reference)?;
                        let own = sheet_operations(context.sheet, context.moves.operations);
                        let mapped = map_area(area, &own)?
                            .filter(|mapped| same_size(area, *mapped))
                            .with_context(|| {
                                format!(
                                    "row/column changes cannot cut through data table {}!{reference}; change it in Excel",
                                    context.sheet
                                )
                            })?;
                        rewritten = replace_xml_attribute(&rewritten, "ref", &mapped.render()?)?;
                    }
                    for input in ["r1", "r2"] {
                        if let Some(address) = formula.attribute(input) {
                            let mapped = map_coordinate(
                                context.sheet,
                                address,
                                context.moves.operations,
                            )?
                            .with_context(|| {
                                format!(
                                    "row/column changes would delete input cell {}!{address} of a data table; change it in Excel",
                                    context.sheet
                                )
                            })?;
                            rewritten = replace_xml_attribute(&rewritten, input, &mapped)?;
                        }
                    }
                    if rewritten != opening {
                        edits.push((
                            formula.range().start - row_start
                                ..formula.range().start - row_start + opening.len(),
                            rewritten,
                        ));
                    }
                }
            }
        }
    }
    Ok(RowOutput {
        number: context.final_row,
        raw: splice(raw, edits, "worksheet")?,
        cells,
    })
}

pub(super) fn find_row_raw<'a>(
    sheet_data: Node<'_, 'a>,
    number: u32,
    original: &'a str,
) -> Option<&'a str> {
    sheet_data
        .children()
        .find(|node| {
            node.has_tag_name((NS, "row"))
                && node
                    .attribute("r")
                    .and_then(|value| value.parse::<u32>().ok())
                    == Some(number)
        })
        .map(|node| &original[node.range()])
}

/// An inserted row, formatted like `template` (the `style_from` row) when given.
/// It is shown even when the template row is hidden or collapsed, since a new
/// row that could not be seen would hide the values written to it.
pub(super) fn new_row(template: Option<&str>, number: u32, prefix: &str) -> RowOutput {
    let opening = template
        .and_then(|raw| xml_opening(raw).ok().map(|(opening, _)| opening))
        .and_then(|opening| replace_xml_attribute(opening, "r", &number.to_string()).ok())
        .and_then(|opening| remove_xml_attribute(&opening, "hidden").ok())
        .and_then(|opening| remove_xml_attribute(&opening, "collapsed").ok())
        .map(|opening| {
            // A template row without cells is written `<row .../>`.
            let open = opening
                .trim_end_matches('>')
                .trim_end_matches('/')
                .trim_end();
            format!("{open}>")
        })
        .unwrap_or_else(|| format!("<{prefix}row r=\"{number}\">"));
    let tag = opening[1..]
        .split([' ', '\t', '\r', '\n', '/', '>'])
        .next()
        .unwrap_or("row");
    RowOutput {
        number,
        raw: format!("{opening}</{tag}>"),
        cells: BTreeSet::new(),
    }
}

/// Whether a mapped area only moved, keeping its height and width.
pub(super) fn same_size(before: Area, after: Area) -> bool {
    let length = |span: Option<(u32, u32)>| span.map(|(a, b)| b - a);
    length(before.columns) == length(after.columns) && length(before.rows) == length(after.rows)
}

pub(super) fn element_tag(opening: &str) -> Result<&str> {
    opening[1..]
        .split([' ', '\t', '\r', '\n', '/', '>'])
        .next()
        .context("invalid XML element")
}

/// Expands shared formulas into per-cell formulas where row/column operations
/// would change them. Excel derives each member from the master cell, which is
/// no longer valid once cells between them move. On edited sheets every group
/// is expanded because the cells themselves move.
pub(super) fn unshare_formulas(
    original: &str,
    sheet: &str,
    moves: &Moves<'_>,
    edited: bool,
) -> Result<Option<String>> {
    let doc = xml(original.as_bytes())?;
    let mut masters = BTreeMap::new();
    let mut groups: BTreeMap<&str, Vec<(Node<'_, '_>, u32, u32)>> = BTreeMap::new();
    for cell in doc.descendants().filter(|n| n.has_tag_name((NS, "c"))) {
        let Some(formula) = child(cell, "f") else {
            continue;
        };
        if formula.attribute("t") != Some("shared") {
            continue;
        }
        let group = formula
            .attribute("si")
            .context("shared formula without si")?;
        let (column, row) = coordinate(cell.attribute("r").context("missing cell address")?)?;
        if formula.attribute("ref").is_some()
            && let Some(text) = formula.text()
        {
            masters.insert(group, (column, row, text));
        }
        groups
            .entry(group)
            .or_default()
            .push((formula, column, row));
    }
    let mut edits = vec![];
    for (group, members) in &groups {
        let Some((master_column, master_row, text)) = masters.get(group) else {
            continue;
        };
        let texts = members
            .iter()
            .map(|(_, column, row)| {
                shift_relative(
                    text,
                    i64::from(*row) - i64::from(*master_row),
                    i64::from(*column) - i64::from(*master_column),
                )
            })
            .collect::<Result<Vec<_>>>()?;
        let mut changes = edited;
        for text in &texts {
            changes = changes || moves.rewrite(text, Some(sheet))? != *text;
        }
        if !changes {
            continue;
        }
        for ((formula, _, _), text) in members.iter().zip(texts) {
            let (opening, _) = xml_opening(&original[formula.range()])?;
            let tag = element_tag(opening)?;
            let mut opening = opening.to_owned();
            for attribute in ["t", "ref", "si"] {
                opening = remove_xml_attribute(&opening, attribute)?;
            }
            let opening = opening
                .trim_end_matches('>')
                .trim_end_matches('/')
                .trim_end();
            edits.push((
                formula.range(),
                format!("{opening}>{}</{tag}>", xml_attr(&text)),
            ));
        }
    }
    if edits.is_empty() {
        return Ok(None);
    }
    splice(original, edits, "shared formula").map(Some)
}

/// The sqref a conditional-format or validation formula is relative to.
fn rule_sqref(node: Node<'_, '_>) -> Option<String> {
    let owner = node.ancestors().find(|n| {
        matches!(n.tag_name().namespace(), Some(NS) | Some(X14))
            && matches!(
                n.tag_name().name(),
                "conditionalFormatting" | "dataValidation"
            )
    })?;
    owner.attribute("sqref").map(str::to_owned).or_else(|| {
        owner
            .children()
            .find(|n| n.has_tag_name((XM, "sqref")))
            .and_then(|n| n.text())
            .map(str::to_owned)
    })
}

/// Offset from the old top-left cell of `sqref` to the old cell that becomes
/// the new top-left. Rule formulas are written relative to that cell, so they
/// are re-expressed from it before references move.
fn anchor_shift(sqref: &str, operations: &[&StructuralOperation]) -> Result<(i64, i64)> {
    let areas = sqref
        .split_whitespace()
        .map(Area::parse)
        .collect::<Result<Vec<_>>>()?;
    let Some(first) = areas.first() else {
        return Ok((0, 0));
    };
    let survivor = |span: Option<(u32, u32)>, row: bool| -> Result<Option<u32>> {
        let Some((start, end)) = span else {
            return Ok(Some(1));
        };
        for index in start..=end {
            if map_span(index, index, operations, row)?.is_some() {
                return Ok(Some(index));
            }
        }
        Ok(None)
    };
    for area in &areas {
        if let (Some(row), Some(column)) =
            (survivor(area.rows, true)?, survivor(area.columns, false)?)
        {
            return Ok((
                i64::from(row) - i64::from(first.first_row()),
                i64::from(column) - i64::from(first.first_column()),
            ));
        }
    }
    Ok((0, 0))
}

pub(super) fn opening_range(original: &str, node: Node<'_, '_>) -> Result<std::ops::Range<usize>> {
    let end = original[node.range()]
        .find('>')
        .context("invalid XML element")?;
    Ok(node.range().start..node.range().start + end + 1)
}

/// Attribute changes collected per element, so that several attributes of
/// one opening tag can change together.
#[derive(Default)]
pub(super) struct AttributeEdits(BTreeMap<usize, (std::ops::Range<usize>, String)>);

impl AttributeEdits {
    pub(super) fn set(
        &mut self,
        original: &str,
        node: Node<'_, '_>,
        name: &str,
        value: &str,
    ) -> Result<()> {
        let range = opening_range(original, node)?;
        let (_, opening) = self
            .0
            .entry(range.start)
            .or_insert_with(|| (range.clone(), original[range].to_owned()));
        *opening = replace_xml_attribute(opening, name, &xml_attr(value))?;
        Ok(())
    }

    pub(super) fn into_edits(self) -> impl Iterator<Item = (std::ops::Range<usize>, String)> {
        self.0.into_values()
    }
}

/// Applies non-overlapping edits after dropping those inside removed ranges.
pub(super) fn apply_edits(
    original: &str,
    mut edits: Vec<(std::ops::Range<usize>, String)>,
    mut removals: Vec<std::ops::Range<usize>>,
) -> Result<String> {
    removals.sort_by_key(|range| (range.start, std::cmp::Reverse(range.end)));
    removals.dedup_by(|inner, outer| outer.start <= inner.start && inner.end <= outer.end);
    // XML ranges nest or are disjoint, so the removals left are disjoint and
    // only the last one starting at or before an edit can contain it.
    edits.retain(|(range, _)| {
        let before = removals.partition_point(|removed| removed.start <= range.start);
        !before
            .checked_sub(1)
            .is_some_and(|last| range.end <= removals[last].end)
    });
    edits.extend(removals.into_iter().map(|range| (range, String::new())));
    splice(original, edits, "reference")
}

/// Containers that become invalid when their last item is removed, with the
/// number of ancestor levels removed along with them.
const CONTAINERS: [(&str, &str, usize); 12] = [
    (NS, "mergeCells", 0),
    (NS, "rowBreaks", 0),
    (NS, "colBreaks", 0),
    (NS, "dataValidations", 0),
    (NS, "hyperlinks", 0),
    (NS, "cols", 0),
    (NS, "protectedRanges", 0),
    (NS, "ignoredErrors", 0),
    (X14, "conditionalFormattings", 0),
    (X14, "dataValidations", 0),
    (X14, "sparklines", 1),
    (X14, "sparklineGroups", 0),
];

/// Removes emptied containers (then emptied extensions) and refreshes counts.
fn tidy_containers(mut text: String) -> Result<String> {
    loop {
        let doc = xml(text.as_bytes())?;
        let empty = doc.descendants().find_map(|node| {
            if node.children().any(|c| c.is_element()) {
                return None;
            }
            if let Some((_, _, level)) = CONTAINERS
                .iter()
                .find(|(ns, name, _)| node.has_tag_name((*ns, *name)))
            {
                return node.ancestors().nth(*level).map(|n| n.range());
            }
            (node.has_tag_name((NS, "ext")) || node.has_tag_name((NS, "extLst")))
                .then(|| node.range())
        });
        let Some(range) = empty else {
            break;
        };
        text.replace_range(range, "");
    }
    let doc = xml(text.as_bytes())?;
    let mut attributes = AttributeEdits::default();
    for node in doc.descendants().filter(|node| {
        node.attribute("count").is_some()
            && (node.has_tag_name((NS, "mergeCells"))
                || node.has_tag_name((NS, "dataValidations"))
                || node.has_tag_name((X14, "dataValidations"))
                || node.has_tag_name((NS, "rowBreaks"))
                || node.has_tag_name((NS, "colBreaks")))
    }) {
        let count = node.children().filter(Node::is_element).count().to_string();
        if node.attribute("count") != Some(count.as_str()) {
            attributes.set(&text, node, "count", &count)?;
        }
        if let Some(manual) = node.attribute("manualBreakCount") {
            let breaks = node
                .children()
                .filter(|brk| brk.is_element() && brk.attribute("man") == Some("1"))
                .count()
                .to_string();
            if manual != breaks {
                attributes.set(&text, node, "manualBreakCount", &breaks)?;
            }
        }
    }
    apply_edits(&text, attributes.into_edits().collect(), vec![])
}

/// Rewrites references held outside cell values. Every sheet updates formulas
/// that point at edited sheets (cell formulas only when `cells_transformed` is
/// false, because row transformation already rewrote them); edited sheets also
/// move their own ranges: merges, conditional formats, validations, hyperlink
/// cells, filters, sorting, protected ranges, column widths and extension ranges.
pub(super) fn rewrite_sheet_references(
    original: &str,
    sheet: &str,
    moves: &Moves<'_>,
    cells_transformed: bool,
) -> Result<String> {
    let own = sheet_operations(sheet, moves.operations);
    let edited = !own.is_empty();
    let columns_changed = own.iter().any(|o| !o.row_operation());
    let doc = xml(original.as_bytes())?;
    let mut removals = vec![];
    let mut edits = vec![];
    let mut attributes = AttributeEdits::default();
    let rewrite_text = |node: Node<'_, '_>, shift: (i64, i64), edits: &mut Vec<_>| -> Result<()> {
        let (Some(text), Some(text_node)) = (node.text(), node.children().find(Node::is_text))
        else {
            return Ok(());
        };
        let shifted = if shift == (0, 0) {
            text.to_owned()
        } else {
            shift_relative(text, shift.0, shift.1)?
        };
        let rewritten = moves.rewrite(&shifted, Some(sheet))?;
        if rewritten != text {
            edits.push((text_node.range(), xml_attr(&rewritten)));
        }
        Ok(())
    };
    let rule_shift = |node: Node<'_, '_>| -> Result<(i64, i64)> {
        match rule_sqref(node) {
            Some(sqref) if edited => anchor_shift(&sqref, &own),
            _ => Ok((0, 0)),
        }
    };
    for node in doc.descendants().filter(Node::is_element) {
        let name = node.tag_name().name();
        match (node.tag_name().namespace(), name) {
            (Some(NS), "f") if !cells_transformed => rewrite_text(node, (0, 0), &mut edits)?,
            (Some(NS), "formula" | "formula1" | "formula2") | (Some(XM), "f") => {
                rewrite_text(node, rule_shift(node)?, &mut edits)?
            }
            (Some(XM), "sqref") if edited => {
                let text = node.text().unwrap_or("");
                let formatting = node.parent_element().is_some_and(|owner| {
                    matches!(
                        owner.tag_name().name(),
                        "conditionalFormatting" | "dataValidation"
                    )
                });
                match map_sqref(text, &own, formatting)? {
                    Some(mapped) if mapped != text => {
                        let text_node =
                            node.children().find(Node::is_text).context("empty sqref")?;
                        edits.push((text_node.range(), mapped));
                    }
                    Some(_) => {}
                    None => removals.push(
                        node.parent_element()
                            .context("sqref without owner")?
                            .range(),
                    ),
                }
            }
            (Some(NS), _) => {
                // Hyperlink `location` targets stay as written; Excel does not
                // update them when rows or columns move.
                if !edited {
                    continue;
                }
                if matches!(
                    name,
                    "mergeCell"
                        | "dimension"
                        | "hyperlink"
                        | "autoFilter"
                        | "sortState"
                        | "sortCondition"
                ) && let Some(reference) = node.attribute("ref")
                {
                    let area = Area::parse(reference)?;
                    let mapped = if name == "mergeCell" {
                        map_merge(area, &own)?
                    } else {
                        map_area(area, &own)?
                    };
                    match mapped {
                        Some(mapped) if mapped != area => {
                            attributes.set(original, node, "ref", &mapped.render()?)?
                        }
                        Some(_) => {}
                        None if name == "dimension" => {
                            attributes.set(original, node, "ref", "A1")?
                        }
                        None => removals.push(node.range()),
                    }
                }
                if matches!(
                    name,
                    "conditionalFormatting" | "dataValidation" | "protectedRange" | "ignoredError"
                ) && let Some(sqref) = node.attribute("sqref")
                {
                    let formatting = matches!(name, "conditionalFormatting" | "dataValidation");
                    match map_sqref(sqref, &own, formatting)? {
                        Some(mapped) if mapped != sqref => {
                            attributes.set(original, node, "sqref", &mapped)?
                        }
                        Some(_) => {}
                        None => removals.push(node.range()),
                    }
                }
                if name == "col" {
                    let min: u32 = node.attribute("min").context("col without min")?.parse()?;
                    let max: u32 = node.attribute("max").context("col without max")?.parse()?;
                    // A column inserted after a hidden one is shown, as in Excel;
                    // other widths and formats carry over to it.
                    let hidden = matches!(node.attribute("hidden"), Some("1" | "true"));
                    let mapped = if hidden {
                        map_span(min, max, &own, false)?
                    } else {
                        map_format_span(min, max, &own, false)?
                    };
                    match mapped {
                        Some((a, b)) if (a, b) != (min, max) => {
                            attributes.set(original, node, "min", &a.to_string())?;
                            attributes.set(original, node, "max", &b.to_string())?;
                        }
                        Some(_) => {}
                        None => removals.push(node.range()),
                    }
                }
                // Color scale, data bar and icon set thresholds may be formulas
                // such as `$B$1`; plain numbers stay as written.
                if name == "cfvo"
                    && let Some(value) = node.attribute("val")
                    && value.parse::<f64>().is_err()
                {
                    let shift = rule_shift(node)?;
                    let shifted = if shift == (0, 0) {
                        value.to_owned()
                    } else {
                        shift_relative(value, shift.0, shift.1)?
                    };
                    let rewritten = moves.rewrite(&shifted, Some(sheet))?;
                    if rewritten != value {
                        attributes.set(original, node, "val", &rewritten)?;
                    }
                }
                // A break at `id` ends a page after that row or column, so it
                // follows the row or column that starts the next page.
                if name == "brk"
                    && let Some(axis) = node
                        .parent_element()
                        .map(|breaks| breaks.tag_name().name())
                        .filter(|breaks| matches!(*breaks, "rowBreaks" | "colBreaks"))
                {
                    let id: u32 = node
                        .attribute("id")
                        .context("page break without id")?
                        .parse()?;
                    match map_span(id + 1, id + 1, &own, axis == "rowBreaks")? {
                        Some((next, _)) if next != id + 1 => {
                            attributes.set(original, node, "id", &(next - 1).to_string())?
                        }
                        Some(_) => {}
                        None => removals.push(node.range()),
                    }
                }
                if name == "filterColumn" && columns_changed {
                    filter_column_edit(original, node, &own, &mut removals, &mut attributes)?;
                }
            }
            _ => {}
        }
    }
    edits.extend(attributes.into_edits());
    let result = if edits.is_empty() && removals.is_empty() {
        original.to_owned()
    } else {
        tidy_containers(apply_edits(original, edits, removals)?)?
    };
    inherit_sparklines(result, &own)
}

/// The pre-operation position of a final position, or `None` when it was
/// inserted.
pub(super) fn original_position(
    mut position: u32,
    operations: &[&StructuralOperation],
    row: bool,
) -> Option<u32> {
    for operation in operations.iter().rev().filter(|o| o.row_operation() == row) {
        let end = operation.at + operation.count;
        if operation.insertion() {
            if (operation.at..end).contains(&position) {
                return None;
            }
            if position >= end {
                position -= operation.count;
            }
        } else if position >= operation.at {
            position += operation.count;
        }
    }
    Some(position)
}

/// For an inserted position, the final position it takes its formatting from:
/// Excel formats inserted rows like the row above and inserted columns like
/// the column to the left.
pub(super) fn inherited_from(
    position: u32,
    operations: &[&StructuralOperation],
    row: bool,
) -> Option<u32> {
    if original_position(position, operations, row).is_some() {
        return None;
    }
    (1..position)
        .rev()
        .find(|candidate| original_position(*candidate, operations, row).is_some())
}

/// Adds sparklines to rows/columns inserted after a sparkline cell, copying
/// it with its data range moved like a copied formula, as Excel does.
fn inherit_sparklines(text: String, operations: &[&StructuralOperation]) -> Result<String> {
    if !operations.iter().any(|o| o.insertion()) {
        return Ok(text);
    }
    let doc = xml(text.as_bytes())?;
    let mut edits = vec![];
    for sparkline in doc
        .descendants()
        .filter(|n| n.has_tag_name((X14, "sparkline")))
    {
        let (Some(formula), Some(location)) = (
            child_ns(sparkline, XM, "f"),
            child_ns(sparkline, XM, "sqref"),
        ) else {
            continue;
        };
        let (Some(source), Some(cell)) = (formula.text(), location.text()) else {
            continue;
        };
        let Ok((column, row)) = coordinate(cell.trim()) else {
            continue;
        };
        let (Some(source_text), Some(location_text)) = (
            formula.children().find(Node::is_text),
            location.children().find(Node::is_text),
        ) else {
            continue;
        };
        let start = sparkline.range().start;
        let raw = &text[sparkline.range()];
        let mut added = String::new();
        for (is_row, origin) in [(true, row), (false, column)] {
            let mut position = origin + 1;
            while inherited_from(position, operations, is_row) == Some(origin) {
                let delta = i64::from(position - origin);
                let (new_source, new_cell) = if is_row {
                    (
                        shift_relative(source, delta, 0)?,
                        format!("{}{position}", column_name(column)?),
                    )
                } else {
                    (
                        shift_relative(source, 0, delta)?,
                        format!("{}{row}", column_name(position)?),
                    )
                };
                let local = |range: std::ops::Range<usize>| range.start - start..range.end - start;
                added.push_str(&apply_edits(
                    raw,
                    vec![
                        (local(source_text.range()), xml_attr(&new_source)),
                        (local(location_text.range()), new_cell),
                    ],
                    vec![],
                )?);
                position += 1;
            }
        }
        if !added.is_empty() {
            let end = sparkline.range().end;
            edits.push((end..end, added));
        }
    }
    apply_edits(&text, edits, vec![])
}

/// Re-bases an autoFilter column after column operations. `colId` counts from
/// the filter's first column, which may itself move.
pub(super) fn filter_column_edit(
    original: &str,
    node: Node<'_, '_>,
    operations: &[&StructuralOperation],
    removals: &mut Vec<std::ops::Range<usize>>,
    attributes: &mut AttributeEdits,
) -> Result<()> {
    let filter = node
        .parent_element()
        .context("filterColumn without autoFilter")?;
    let area = Area::parse(filter.attribute("ref").context("autoFilter without ref")?)?;
    let Some(mapped) = map_area(area, operations)? else {
        return Ok(());
    };
    let id: u32 = node
        .attribute("colId")
        .context("filterColumn without colId")?
        .parse()?;
    let column = area.first_column() + id;
    match map_span(column, column, operations, false)? {
        None => removals.push(node.range()),
        Some((new_column, _)) => {
            let new_id = new_column - mapped.first_column();
            if new_id != id {
                attributes.set(original, node, "colId", &new_id.to_string())?;
            }
        }
    }
    Ok(())
}
