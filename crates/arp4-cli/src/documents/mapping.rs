use super::*;

pub(super) fn key(entry: &Value) -> Result<(String, String, String)> {
    Ok((
        string(&entry["page"])?.into(),
        string(&entry["block"])?.into(),
        entry["field"].as_str().unwrap_or("").into(),
    ))
}
pub(super) fn target(value: &Value) -> Result<(&str, &str)> {
    Ok((string(&value["sheet"])?, string(&value["cell"])?))
}
pub(super) enum MappingTarget<'a> {
    Cell {
        sheet: &'a str,
        cell: &'a str,
    },
    InsertionRow {
        sheet: &'a str,
        insertion: &'a str,
        offset: u32,
        column: &'a str,
    },
    InsertionColumn {
        sheet: &'a str,
        insertion: &'a str,
        offset: u32,
        row: u32,
    },
}
pub(super) fn mapping_target(value: &Value) -> Result<Option<MappingTarget<'_>>> {
    if value.is_null() {
        return Ok(None);
    }
    if value.get("cell").is_some() {
        return Ok(Some(MappingTarget::Cell {
            sheet: string(&value["sheet"])?,
            cell: string(&value["cell"])?,
        }));
    }
    let sheet = string(&value["sheet"])?;
    let insertion = string(&value["insertion"])?;
    let offset = u32::try_from(
        value["offset"]
            .as_u64()
            .context("target offset must be integer")?,
    )?;
    if value.get("column").is_some() {
        return Ok(Some(MappingTarget::InsertionRow {
            sheet,
            insertion,
            offset,
            column: string(&value["column"])?,
        }));
    }
    Ok(Some(MappingTarget::InsertionColumn {
        sheet,
        insertion,
        offset,
        row: u32::try_from(
            value["row"]
                .as_u64()
                .context("target row must be integer")?,
        )?,
    }))
}
pub(super) fn page_sheet(extraction: &Value, page: &str) -> Result<String> {
    let index: usize = page
        .strip_prefix("sheet-")
        .context("structural content page must be a worksheet page")?
        .parse::<usize>()?
        .checked_sub(1)
        .context("invalid worksheet page")?;
    string(
        &array(&extraction["sheets"])?
            .get(index)
            .context("content page has no worksheet")?["name"],
    )
    .map(str::to_owned)
}
/// The insertion a row or column key of new table content names: `<operation
/// ID>-<n>` is the n-th row or column that operation inserts. Original rows
/// keep `r<number>` and original columns their letters, so a new position
/// never takes the key of an original one.
fn inserted_position(
    operations: &[excel::StructuralOperation],
    sheet: &str,
    key: &str,
    row: bool,
) -> Result<Option<(String, u32)>> {
    let Some((id, number)) = key.rsplit_once('-') else {
        return Ok(None);
    };
    let Some(operation) = operations
        .iter()
        .find(|operation| operation.sheet == sheet && operation.id == id)
    else {
        return Ok(None);
    };
    let Ok(number) = number.parse::<u32>() else {
        return Ok(None);
    };
    let axis = if row { "row" } else { "column" };
    ensure!(
        operation.insertion() && operation.row_operation() == row,
        "{axis} {key} names operation {id}, which does not insert {axis}s"
    );
    ensure!(
        (1..=operation.count).contains(&number),
        "{axis} {key} is outside the {} {axis}(s) operation {id} inserts",
        operation.count
    );
    Ok(Some((id.to_owned(), number - 1)))
}
/// A (page, block) or (row, column) pair borrowed from the content pages.
type Pair<'a> = (&'a str, &'a str);

/// Keeps the entries whose content still exists and maps new table cells. Takes
/// `mappings` by value: a large document has an entry for every cell, and copying
/// them, or keying them by owned strings, cost more than the checks themselves.
pub(super) fn regenerate_mappings(
    mut regenerated: Value,
    extraction: &Value,
    pages: &[Value],
    operations: &[excel::StructuralOperation],
) -> Result<Value> {
    let mut positions: BTreeMap<Pair, BTreeMap<Pair, &Value>> = BTreeMap::new();
    let mut fields: BTreeMap<Pair, BTreeSet<&str>> = BTreeMap::new();
    let mut blocks = BTreeSet::new();
    for page in pages {
        let page_id = string(&page["page_id"])?;
        for (block, body) in page["blocks"]
            .as_object()
            .context("content blocks required")?
        {
            let block_key = (page_id, block.as_str());
            blocks.insert(block_key);
            if let Some(values) = body["fields"].as_object() {
                fields
                    .entry(block_key)
                    .or_default()
                    .extend(values.keys().map(String::as_str));
            }
            if let Some(rows) = body["rows"].as_object() {
                let table = positions.entry(block_key).or_default();
                for (row, values) in rows {
                    for (column, value) in values.as_object().context("invalid table row")? {
                        table.insert((row.as_str(), column.as_str()), value);
                    }
                }
            }
        }
    }
    let mut keep = vec![];
    let mut used_fields = BTreeSet::new();
    let mut mapped_positions = BTreeSet::new();
    for entry in array(&regenerated["entries"])? {
        let page = string(&entry["page"])?;
        let block = string(&entry["block"])?;
        let kept = if let Some(position) = entry["position"].as_object() {
            let cell = (
                string(&position["row"]).unwrap_or(""),
                string(&position["column"]).unwrap_or(""),
            );
            let exists = positions
                .get(&(page, block))
                .is_some_and(|values| values.contains_key(&cell));
            if exists {
                string(&position["row"])?;
                string(&position["column"])?;
                mapped_positions.insert((page, block, cell.0, cell.1));
            }
            exists
        } else if entry["field"].is_null() {
            blocks.contains(&(page, block))
        } else {
            let field = string(&entry["field"])?;
            fields
                .get(&(page, block))
                .is_some_and(|values| values.contains(field))
        };
        if kept && !entry["field"].is_null() {
            used_fields.insert(string(&entry["field"])?);
        }
        keep.push(kept);
    }
    let mut generated = vec![];
    let mut generated_fields = BTreeSet::new();
    for ((page, block), values) in positions {
        let sheet = page_sheet(extraction, page)?;
        for ((row, column), value) in values {
            if mapped_positions.contains(&(page, block, row, column)) {
                continue;
            }
            ensure!(
                value.is_null() || kind(value) != "object",
                "new table references require explicit mapping"
            );
            let target = match (
                inserted_position(operations, &sheet, row, true)?,
                inserted_position(operations, &sheet, column, false)?,
            ) {
                (Some((insertion, offset)), None) => {
                    excel::column_number(column).with_context(|| {
                        format!("{row}/{column}: an inserted row takes values in original columns (A, B, ...)")
                    })?;
                    json!({"sheet":sheet,"insertion":insertion,"offset":offset,"column":column})
                }
                (None, Some((insertion, offset))) => {
                    let row_number = row
                        .strip_prefix('r')
                        .and_then(|number| number.parse::<u32>().ok())
                        .filter(|number| *number > 0)
                        .with_context(|| {
                            format!("{row}/{column}: an inserted column takes values in original rows (r1, r2, ...)")
                        })?;
                    json!({"sheet":sheet,"insertion":insertion,"offset":offset,"row":row_number})
                }
                (Some(_), Some(_)) => bail!(
                    "{row}/{column}: a cell in both an inserted row and an inserted column cannot be written"
                ),
                (None, None) => bail!(
                    "{row}/{column} is not a cell of the original: edit only cells that hold a value, and name new rows and columns <operation ID>-<n> after the insert_rows or insert_columns operation that adds them"
                ),
            };
            let field_prefix = format!("generated-{row}-{column}");
            let mut field = field_prefix.clone();
            let mut suffix = 2;
            while used_fields.contains(field.as_str()) || generated_fields.contains(&field) {
                field = format!("{field_prefix}-{suffix}");
                suffix += 1;
            }
            generated_fields.insert(field.clone());
            generated.push(json!({
                "page":page,
                "block":block,
                "field":field,
                "origins":[],
                "reason":"structural content generated mapping",
                "target":target,
                "writeback":"cell",
                "position":{"row":row,"column":column,"type":kind(value)}
            }));
        }
    }
    let Value::Array(entries) = regenerated["entries"].take() else {
        bail!("expected array");
    };
    let mut kept: Vec<Value> = entries
        .into_iter()
        .zip(keep)
        .filter_map(|(entry, keep)| keep.then_some(entry))
        .collect();
    kept.extend(generated);
    regenerated["entries"] = Value::Array(kept);
    if let Some(tables) = regenerated["tables"].as_array_mut() {
        for table in tables {
            let page = string(&table["page"])?;
            let block = string(&table["block"])?;
            let Some(page_value) = pages.iter().find(|value| value["page_id"] == page) else {
                continue;
            };
            let Some(rows) = page_value["blocks"][block]["rows"].as_object() else {
                continue;
            };
            let columns = table["columns"]
                .as_array_mut()
                .context("table columns required")?;
            for row in rows.values() {
                for column in row.as_object().context("invalid table row")?.keys() {
                    if !columns.iter().any(|value| value == column) {
                        columns.push(json!(column));
                    }
                }
            }
        }
    }
    Ok(regenerated)
}
