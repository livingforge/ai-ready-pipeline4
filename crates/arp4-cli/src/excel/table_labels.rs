//! Header and totals labels of the columns of Excel tables. The table
//! definition repeats them as column names and totals labels, and structured
//! references name the columns, so an edit to the cell changes all of them.
use super::*;

/// Excel's limit on the length of a table column name.
const MAX_COLUMN_NAME: usize = 255;

/// The table whose header (`true`) or totals row (`false`) holds the cell at
/// `column`, `row` of `sheet`, as the extraction records its tables.
pub fn table_label(sheet: &Value, column: u32, row: u32) -> Result<Option<(&str, bool)>> {
    for table in sheet["tables"].as_array().into_iter().flatten() {
        let area = Area::parse(string(&table["range"])?)?;
        let (Some((first_column, last_column)), Some((first_row, last_row))) =
            (area.columns, area.rows)
        else {
            continue;
        };
        if !(first_column..=last_column).contains(&column) {
            continue;
        }
        let header_rows = u32::try_from(table["header_rows"].as_u64().unwrap_or(1))?;
        let totals_rows = u32::try_from(table["totals_rows"].as_u64().unwrap_or(0))?;
        let name = string(&table["name"])?;
        if (first_row..first_row + header_rows).contains(&row) {
            return Ok(Some((name, true)));
        }
        if totals_rows > 0 && (last_row + 1 - totals_rows..=last_row).contains(&row) {
            return Ok(Some((name, false)));
        }
    }
    Ok(None)
}

/// Checks a value written to a table's header (a column name) or totals row
/// (a totals label).
fn check_label(value: &Value, header: bool) -> Result<&str> {
    let what = if header {
        "a table column name"
    } else {
        "a table totals label"
    };
    let text = value
        .as_str()
        .with_context(|| format!("{what} must be text"))?;
    if header {
        ensure!(!text.trim().is_empty(), "{what} cannot be empty");
        ensure!(
            text.chars().count() <= MAX_COLUMN_NAME,
            "{what} has at most {MAX_COLUMN_NAME} characters"
        );
    }
    ensure!(
        !text.chars().any(char::is_control),
        "{what} cannot contain line breaks, tabs or other control characters; edit it in Excel"
    );
    Ok(text)
}

/// Checks `value` written to the cell at `column`, `row` of `sheet` when the
/// cell is a header or totals label of one of its tables.
pub fn ensure_table_label_edit(sheet: &Value, column: u32, row: u32, value: &Value) -> Result<()> {
    if let Some((table, header)) = table_label(sheet, column, row)? {
        check_label(value, header).with_context(|| {
            format!(
                "{}!{}{row} is a {} cell of table {table}",
                string(&sheet["name"]).unwrap_or(""),
                column_name(column).unwrap_or_default(),
                if header { "header" } else { "totals" }
            )
        })?;
    }
    Ok(())
}

/// Rejects two columns of one of `sheet`'s tables with the same name, as
/// Excel compares them (ignoring case), where `value_at` gives the value of
/// each header cell by address.
pub fn ensure_unique_table_headers(
    sheet: &Value,
    value_at: impl Fn(&str) -> Option<Value>,
) -> Result<()> {
    for table in sheet["tables"].as_array().into_iter().flatten() {
        let area = Area::parse(string(&table["range"])?)?;
        let (Some((first_column, last_column)), Some((first_row, _))) = (area.columns, area.rows)
        else {
            continue;
        };
        if table["header_rows"].as_u64().unwrap_or(1) == 0 {
            continue;
        }
        let mut names = BTreeSet::new();
        for column in first_column..=last_column {
            let name = match value_at(&format!("{}{first_row}", column_name(column)?)) {
                Some(Value::String(text)) => text,
                Some(Value::Null) | None => continue,
                Some(other) => other.to_string(),
            };
            ensure!(
                names.insert(name.to_lowercase()),
                "table {} would have two columns named {name}; Excel requires distinct column names",
                string(&table["name"])?
            );
        }
    }
    Ok(())
}

/// The cell of the original `row`, `column` of `sheet` after `own` operations
/// and the value `changes` write there, if any.
fn written<'c>(
    changes: &'c BTreeMap<(String, u32, u32), Value>,
    own: &[&StructuralOperation],
    sheet: &str,
    row: u32,
    column: u32,
) -> Result<Option<(String, &'c Value)>> {
    let (Some((row, _)), Some((column, _))) = (
        map_span(row, row, own, true)?,
        map_span(column, column, own, false)?,
    ) else {
        return Ok(None);
    };
    let Some(value) = changes.get(&(sheet.to_owned(), row, column)) else {
        return Ok(None);
    };
    Ok(Some((
        format!("{sheet}!{}{row}", column_name(column)?),
        value,
    )))
}

/// The table definitions with the new labels that `changes` (keyed by sheet,
/// final row and final column) write over existing columns, and the renamed
/// columns for structured references.
#[derive(Default)]
pub(super) struct LabelEdits {
    /// Table part -> its XML with the new names and labels.
    pub(super) parts: BTreeMap<String, String>,
    /// Lowercase table name -> lowercase old column name -> new column name.
    pub(super) renamed: BTreeMap<String, BTreeMap<String, String>>,
}

impl LabelEdits {
    pub(super) fn new(
        parts: &BTreeMap<String, Vec<u8>>,
        sheets: &[Value],
        operations: &[StructuralOperation],
        changes: &BTreeMap<(String, u32, u32), Value>,
    ) -> Result<Self> {
        let mut edits = Self::default();
        for sheet in sheets {
            let sheet_name = string(&sheet["name"])?;
            let own = sheet_operations(sheet_name, operations);
            for part in related_parts(parts, string(&sheet["part"])?, "/table")? {
                let original = std::str::from_utf8(&parts[&part])?;
                let doc = xml(original.as_bytes())?;
                let root = doc.root_element();
                let table = root
                    .attribute("displayName")
                    .or_else(|| root.attribute("name"))
                    .context("table without name")?;
                let area = Area::parse(root.attribute("ref").context("table without ref")?)?;
                let (Some((first_column, _)), Some((first_row, last_row))) =
                    (area.columns, area.rows)
                else {
                    continue;
                };
                let header_rows: u32 = root.attribute("headerRowCount").unwrap_or("1").parse()?;
                let totals_rows: u32 = root.attribute("totalsRowCount").unwrap_or("0").parse()?;
                let columns = child(root, "tableColumns").context("table without tableColumns")?;
                let written = |row, column| written(changes, &own, sheet_name, row, column);
                let mut xml_edits = vec![];
                let mut names = BTreeSet::new();
                for (offset, node) in columns.children().filter(Node::is_element).enumerate() {
                    let column = first_column + offset as u32;
                    let old = node
                        .attribute("name")
                        .context("table column without name")?;
                    let mut name = old;
                    let mut opening = original[opening_range(original, node)?].to_owned();
                    if header_rows > 0
                        && let Some((cell, value)) = written(first_row, column)?
                    {
                        let new = check_label(value, true).with_context(|| {
                            format!("{cell} is the header of column {old} in table {table}")
                        })?;
                        if new != old {
                            opening = set_xml_attribute(
                                &opening,
                                "name",
                                &xml_attr(&encode_xstring(new)),
                            )?;
                            edits
                                .renamed
                                .entry(table.to_lowercase())
                                .or_default()
                                .insert(old.to_lowercase(), new.to_owned());
                            name = new;
                        }
                    }
                    if totals_rows > 0
                        && let Some((cell, value)) = written(last_row, column)?
                    {
                        ensure!(
                            node.attribute("totalsRowFunction")
                                .is_none_or(|function| function == "none"),
                            "{cell} shows the totals function of column {old} in table {table}; change it in Excel"
                        );
                        let label = check_label(value, false).with_context(|| {
                            format!("{cell} is the totals label of column {old} in table {table}")
                        })?;
                        if node.attribute("totalsRowLabel") != Some(label) {
                            opening = set_xml_attribute(
                                &opening,
                                "totalsRowLabel",
                                &xml_attr(&encode_xstring(label)),
                            )?;
                        }
                    }
                    ensure!(
                        names.insert(name.to_lowercase()),
                        "table {table} would have two columns named {name}; Excel requires distinct column names"
                    );
                    let range = opening_range(original, node)?;
                    if opening != original[range.clone()] {
                        xml_edits.push((range, opening));
                    }
                }
                if !xml_edits.is_empty() {
                    edits
                        .parts
                        .insert(part, apply_edits(original, xml_edits, vec![])?);
                }
            }
        }
        Ok(edits)
    }
}
