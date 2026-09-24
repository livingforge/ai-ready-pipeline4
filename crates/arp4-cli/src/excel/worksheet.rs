use super::*;

pub(super) struct RowOutput {
    pub(super) number: u32,
    pub(super) raw: String,
    pub(super) cells: BTreeSet<u32>,
}

impl RowOutput {
    pub(super) fn append_cell(&mut self, cell: &str, column: u32) {
        let close = self.raw.rfind("</").expect("row has a closing tag");
        self.raw.insert_str(close, cell);
        self.cells.insert(column);
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
            let text = text
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

pub(super) fn render_new_cell(
    address: &str,
    value: &Value,
    template: Option<&str>,
) -> Result<String> {
    let mut opening = if let Some(template) = template {
        xml_opening(template)?.0.to_owned()
    } else {
        "<c>".to_owned()
    };
    opening = remove_xml_attribute(&opening, "t")?;
    opening = opening
        .trim_end_matches('/')
        .trim_end_matches('>')
        .to_owned();
    if opening.contains(" r=") {
        opening = replace_xml_attribute(&format!("{opening}>"), "r", address)?;
        opening = opening.trim_end_matches('>').to_owned();
    } else {
        opening.push_str(&format!(" r=\"{address}\""));
    }
    let tag = opening[1..]
        .split([' ', '\t', '\r', '\n', '/', '>'])
        .next()
        .context("invalid cell tag")?;
    let prefix = tag.strip_suffix('c').context("invalid cell tag")?;
    Ok(format!("{opening}{}</{tag}>", scalar_body(prefix, value)?))
}

pub(super) fn rewrite_formula(
    formula: &str,
    current_sheet: &str,
    operations: &[StructuralOperation],
) -> Result<String> {
    let re = regex::Regex::new(
        r#"(?:(?P<sheet>'[^']+'|[A-Za-z_][A-Za-z0-9_. ]*)!)?(?P<abs_col>\$?)(?P<col>[A-Z]{1,3})(?P<abs_row>\$?)(?P<row>[1-9][0-9]*)"#,
    )?;
    let mut error = None;
    let output = re
        .replace_all(formula, |captures: &regex::Captures<'_>| {
            let sheet = captures.name("sheet").map(|value| {
                let value = value.as_str();
                if value.starts_with('\'') && value.ends_with("!") {
                    value[1..value.len() - 2].replace("''", "'")
                } else {
                    value.trim_end_matches('!').to_owned()
                }
            });
            let sheet = sheet.as_deref().unwrap_or(current_sheet);
            let address = format!("{}{}", &captures["col"], &captures["row"]);
            let mapped = match map_coordinate(sheet, &address, operations) {
                Ok(Some(value)) => value,
                Ok(None) => return "#REF!".to_owned(),
                Err(e) => {
                    error = Some(e);
                    return captures.get(0).unwrap().as_str().to_owned();
                }
            };
            let (column, row) = coordinate(&mapped).unwrap();
            let column = column_name(column).unwrap();
            format!(
                "{}{}{}{}{}",
                captures.name("sheet").map_or("", |value| value.as_str()),
                &captures["abs_col"],
                column,
                &captures["abs_row"],
                row
            )
        })
        .into_owned();
    if let Some(error) = error {
        return Err(error);
    }
    Ok(output)
}

pub(super) struct RowTransform<'a, 'b> {
    pub(super) original: &'a str,
    pub(super) original_row: u32,
    pub(super) final_row: u32,
    pub(super) sheet: &'a str,
    pub(super) operations: &'b [StructuralOperation],
    pub(super) all_operations: &'b [StructuralOperation],
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
                        rewrite_formula(text, context.sheet, context.all_operations)?,
                    ));
                }
            }
        }
    }
    edits.sort_by_key(|(range, _)| range.start);
    for pair in edits.windows(2) {
        ensure!(
            pair[0].0.end <= pair[1].0.start,
            "overlapping worksheet edits"
        );
    }
    let mut result = raw.to_owned();
    for (range, replacement) in edits.into_iter().rev() {
        result.replace_range(range, &replacement);
    }
    Ok(RowOutput {
        number: context.final_row,
        raw: result,
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

pub(super) fn new_row(template: Option<&str>, number: u32) -> RowOutput {
    let opening = template
        .and_then(|raw| xml_opening(raw).ok().map(|(opening, _)| opening))
        .and_then(|opening| replace_xml_attribute(opening, "r", &number.to_string()).ok())
        .unwrap_or_else(|| format!("<row r=\"{number}\">"));
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

pub(super) fn rewrite_range_ref(
    reference: &str,
    sheet: &str,
    operations: &[StructuralOperation],
) -> Result<Option<String>> {
    let mut parts = reference.split(':');
    let first = parts.next().context("empty Excel range")?;
    let second = parts.next().unwrap_or(first);
    ensure!(parts.next().is_none(), "invalid Excel range");
    let Some(first) = map_coordinate(sheet, first, operations)? else {
        return Ok(None);
    };
    let Some(second) = map_coordinate(sheet, second, operations)? else {
        return Ok(None);
    };
    Ok(Some(if first == second {
        first
    } else {
        format!("{first}:{second}")
    }))
}

pub(super) fn rewrite_sheet_ranges(
    original: &str,
    sheet: &str,
    operations: &[StructuralOperation],
) -> Result<String> {
    let doc = xml(original.as_bytes())?;
    let mut edits = vec![];
    for node in doc
        .descendants()
        .filter(|node| node.has_tag_name((NS, "mergeCell")) || node.has_tag_name((NS, "dimension")))
    {
        let Some(reference) = node.attribute("ref") else {
            continue;
        };
        let opening_end = original[node.range()]
            .find('>')
            .context("invalid range element")?;
        let opening = &original[node.range().start..node.range().start + opening_end + 1];
        let Some(reference) = rewrite_range_ref(reference, sheet, operations)? else {
            if node.has_tag_name((NS, "mergeCell")) {
                edits.push((node.range(), String::new()));
            }
            continue;
        };
        let replacement = replace_xml_attribute(opening, "ref", &reference)?;
        edits.push((
            node.range().start..node.range().start + opening_end + 1,
            replacement,
        ));
    }
    edits.sort_by_key(|(range, _)| range.start);
    for pair in edits.windows(2) {
        ensure!(pair[0].0.end <= pair[1].0.start, "overlapping range edits");
    }
    let mut result = original.to_owned();
    for (range, replacement) in edits.into_iter().rev() {
        result.replace_range(range, &replacement);
    }
    Ok(result)
}
