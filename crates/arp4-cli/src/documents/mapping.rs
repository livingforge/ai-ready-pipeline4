use super::*;

pub(super) fn key(entry: &Value) -> Result<(String, String, String)> {
    Ok((
        string(&entry["page"])?.into(),
        string(&entry["block"])?.into(),
        entry["field"].as_str().unwrap_or("").into(),
    ))
}
pub(super) fn target(value: &Value) -> Result<(String, String)> {
    Ok((
        string(&value["sheet"])?.into(),
        string(&value["cell"])?.into(),
    ))
}
pub(super) enum MappingTarget {
    Cell {
        sheet: String,
        cell: String,
    },
    InsertionRow {
        sheet: String,
        insertion: String,
        offset: u32,
        column: String,
    },
    InsertionColumn {
        sheet: String,
        insertion: String,
        offset: u32,
        row: u32,
    },
}
pub(super) fn mapping_target(value: &Value) -> Result<Option<MappingTarget>> {
    if value.is_null() {
        return Ok(None);
    }
    if value.get("cell").is_some() {
        return Ok(Some(MappingTarget::Cell {
            sheet: string(&value["sheet"])?.into(),
            cell: string(&value["cell"])?.into(),
        }));
    }
    let sheet = string(&value["sheet"])?.to_owned();
    let insertion = string(&value["insertion"])?.to_owned();
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
            column: string(&value["column"])?.to_owned(),
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
pub(super) fn regenerate_mappings(
    mappings: &Value,
    extraction: &Value,
    pages: &[Value],
    operations: &[excel::StructuralOperation],
) -> Result<Value> {
    let mut regenerated = mappings.clone();
    let mut positions: BTreeMap<(String, String), BTreeMap<(String, String), Value>> =
        BTreeMap::new();
    let mut fields: BTreeMap<(String, String), BTreeSet<String>> = BTreeMap::new();
    let mut blocks = BTreeSet::new();
    for page in pages {
        let page_id = string(&page["page_id"])?;
        for (block, body) in page["blocks"]
            .as_object()
            .context("content blocks required")?
        {
            let block_key = (page_id.to_owned(), block.clone());
            blocks.insert(block_key.clone());
            if let Some(values) = body["fields"].as_object() {
                fields
                    .entry(block_key.clone())
                    .or_default()
                    .extend(values.keys().cloned());
            }
            if let Some(rows) = body["rows"].as_object() {
                let table = positions.entry(block_key).or_default();
                for (row, values) in rows {
                    for (column, value) in values.as_object().context("invalid table row")? {
                        table.insert((row.clone(), column.clone()), value.clone());
                    }
                }
            }
        }
    }
    let original_entries = array(&mappings["entries"])?;
    let mut used_fields = BTreeSet::new();
    let mut kept = vec![];
    let mut mapped_positions = BTreeSet::new();
    for entry in original_entries {
        let page = string(&entry["page"])?;
        let block = string(&entry["block"])?;
        let block_key = (page.to_owned(), block.to_owned());
        let keep = if let Some(position) = entry["position"].as_object() {
            positions.get(&block_key).is_some_and(|values| {
                values.contains_key(&(
                    string(&position["row"]).unwrap_or("").to_owned(),
                    string(&position["column"]).unwrap_or("").to_owned(),
                ))
            })
        } else if entry["field"].is_null() {
            blocks.contains(&block_key)
        } else {
            let field = string(&entry["field"])?;
            fields
                .get(&block_key)
                .is_some_and(|values| values.contains(field))
        };
        if keep {
            if let Some(position) = entry["position"].as_object() {
                mapped_positions.insert((
                    page.to_owned(),
                    block.to_owned(),
                    string(&position["row"])?.to_owned(),
                    string(&position["column"])?.to_owned(),
                ));
            }
            if !entry["field"].is_null() {
                used_fields.insert(string(&entry["field"])?.to_owned());
            }
            kept.push(entry.clone());
        }
    }
    for ((page, block), values) in positions {
        let sheet = page_sheet(extraction, &page)?;
        for ((row, column), value) in values {
            if mapped_positions.contains(&(
                page.clone(),
                block.clone(),
                row.clone(),
                column.clone(),
            )) {
                continue;
            }
            ensure!(
                value.is_null() || kind(&value) != "object",
                "new table references require explicit mapping"
            );
            let target = match (
                inserted_position(operations, &sheet, &row, true)?,
                inserted_position(operations, &sheet, &column, false)?,
            ) {
                (Some((insertion, offset)), None) => {
                    excel::column_number(&column).with_context(|| {
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
            while used_fields.contains(&field) {
                field = format!("{field_prefix}-{suffix}");
                suffix += 1;
            }
            used_fields.insert(field.clone());
            kept.push(json!({
                "page":page,
                "block":block,
                "field":field,
                "origins":[],
                "reason":"structural content generated mapping",
                "target":target,
                "writeback":"cell",
                "position":{"row":row,"column":column,"type":kind(&value)}
            }));
        }
    }
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
