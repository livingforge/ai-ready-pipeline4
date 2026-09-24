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
pub(super) fn insertion_for(
    operations: &[excel::StructuralOperation],
    sheet: &str,
    row: Option<u32>,
    column: Option<u32>,
) -> Result<Option<(String, u32, bool)>> {
    let mut matches = vec![];
    for operation in operations
        .iter()
        .filter(|operation| operation.sheet == sheet)
    {
        let (position, row_operation) = if operation.row_operation() {
            (row, true)
        } else {
            (column, false)
        };
        if !operation.insertion() {
            continue;
        }
        if let Some(position) = position
            && (operation.at..operation.at + operation.count).contains(&position)
        {
            matches.push((operation.id.clone(), position - operation.at, row_operation));
        }
    }
    ensure!(
        matches.len() <= 1,
        "multiple insertion operations match new table value"
    );
    Ok(matches.pop())
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
            let row_number = row
                .strip_prefix('r')
                .context("new table rows must use r<number> IDs")?
                .parse::<u32>()?;
            let column_number = excel::column_number(&column)?;
            let (insertion, offset, row_operation) =
                insertion_for(operations, &sheet, Some(row_number), Some(column_number))?.context(
                    "new table value requires a matching row or column insertion operation",
                )?;
            let field_prefix = format!("generated-{row}-{column}");
            let mut field = field_prefix.clone();
            let mut suffix = 2;
            while used_fields.contains(&field) {
                field = format!("{field_prefix}-{suffix}");
                suffix += 1;
            }
            used_fields.insert(field.clone());
            let target = if row_operation {
                json!({"sheet":sheet,"insertion":insertion,"offset":offset,"column":column})
            } else {
                json!({"sheet":sheet,"insertion":insertion,"offset":offset,"row":row_number})
            };
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
