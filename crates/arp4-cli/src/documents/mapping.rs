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
    /// The text of an Excel shape, by its extraction ID (`<drawing part>#<id>`).
    Shape {
        sheet: &'a str,
        shape: &'a str,
    },
}
pub(super) fn mapping_target(value: &Value) -> Result<Option<MappingTarget<'_>>> {
    if value.is_null() {
        return Ok(None);
    }
    if value.get("shape").is_some() {
        return Ok(Some(MappingTarget::Shape {
            sheet: string(&value["sheet"])?,
            shape: string(&value["shape"])?,
        }));
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
/// The sheet whose content page is `page`: the `page` a sheet names, or else
/// `sheet-<n>` for the n-th sheet.
pub(super) fn page_sheet(extraction: &Value, page: &str) -> Result<String> {
    let sheets = array(&extraction["sheets"])?;
    if sheets.iter().any(|sheet| sheet.get("page").is_some()) {
        return string(
            &sheets
                .iter()
                .find(|sheet| sheet["page"] == page)
                .with_context(|| format!("content page {page} has no worksheet"))?["name"],
        )
        .map(str::to_owned);
    }
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
/// The mapping entries import derives from `extraction`: one per cell and formula,
/// and one per page block. mappings.yml stores only entries that differ from these.
pub(super) fn default_entries(extraction: &Value) -> Result<Vec<Value>> {
    let native_text = extraction["parser"]
        .as_str()
        .and_then(|parser| parser.split(';').nth(1))
        == Some("native-text/1");
    let mut entries = vec![];
    for (i, sheet) in array(&extraction["sheets"])?.iter().enumerate() {
        let page = match sheet["page"].as_str() {
            Some(page) => page.to_owned(),
            None => format!("sheet-{}", i + 1),
        };
        let computed_ranges = excel::ComputedRanges::new(sheet)?;
        let merges = excel::Merges::new(&sheet["merges"])?;
        let cells = array(&sheet["cells"])?;
        let mut has_formulas = false;
        for c in cells {
            let address = string(&c["address"])?;
            let (_, row) = excel::coordinate(address)?;
            let column = address.trim_end_matches(|c: char| c.is_ascii_digit());
            let target = json!({"sheet":sheet["name"],"cell":address});
            let computed = computed_ranges.find(address)?.map(|(kind, _)| kind);
            let hidden = merges.hiding(address)?.is_some();
            let reason = match computed {
                Some("array") => "配列数式・スピルの計算結果のため書き戻し対象外",
                Some("data_table") => "データテーブルの計算結果のため書き戻し対象外",
                Some(_) => "ピボットテーブルの集計結果のため書き戻し対象外",
                None if hidden => "結合セルの左上以外のため書き戻し対象外",
                None => "原本から転記",
            };
            let excluded = hidden
                || computed.is_some()
                || native_text
                || c["type"] == "formula"
                || c["type"] == "error";
            entries.push(json!({"page":page,"block":"table-1","field":c["id"],"reason":reason,"target":target,"writeback":if excluded{"excluded"}else{"cell"},"position":{"row":format!("r{row}"),"column":column,"type":kind(&c["value"])}}));
            if c["type"] == "formula" {
                has_formulas = true;
                let f = string(&c["id"])?;
                // The formula of an array, spill or data table spans its range, and
                // one hidden by a merge is not shown; those stay as they are.
                let (reason, writeback) = if computed.is_some() || hidden {
                    (
                        "数式原文。配列数式・スピル・データテーブル・結合で隠れたセルのため書き戻し対象外",
                        "excluded",
                    )
                } else {
                    (
                        "数式原文。変更すると書き戻しで数式を置き換え、Excelで再計算",
                        "formula",
                    )
                };
                entries.push(json!({"page":page,"block":"formulas","field":format!("{f}-formula"),"reason":reason,"target":target,"writeback":writeback,"position":{"row":f,"column":"formula","type":"string"}}));
            }
        }
        let mut has_shapes = false;
        for (key, drawing) in shape_texts(sheet)? {
            has_shapes = true;
            entries.push(json!({"page":page,"block":"shapes","field":format!("{key}-text"),"reason":"図形の文字","target":{"sheet":sheet["name"],"shape":drawing["id"]},"writeback":if native_text{"excluded"}else{"shape"},"position":{"row":key,"column":"text","type":"string"}}));
        }
        entries.push(json!({"page":page,"block":"extraction-notes","field":null,"reason":"抽出範囲の申告","target":null,"writeback":"excluded"}));
        for (block, present) in [
            ("table-1", !cells.is_empty()),
            ("formulas", has_formulas),
            ("shapes", has_shapes),
        ] {
            if present {
                entries.push(json!({"page":page,"block":block,"field":null,"reason":"表の構造","target":null,"writeback":"excluded"}));
            }
        }
    }
    Ok(entries)
}

/// The shapes of an Excel sheet that hold text, by their content key
/// (`shape-<drawing ID>`).
pub(super) fn shape_texts(sheet: &Value) -> Result<Vec<(String, &Value)>> {
    let mut shapes = vec![];
    for drawing in sheet["drawings"].as_array().into_iter().flatten() {
        if drawing["text"]
            .as_str()
            .is_some_and(|text| !text.is_empty())
        {
            let id = string(&drawing["id"])?;
            let (_, raw) = id
                .rsplit_once('#')
                .context("shape ID without drawing part")?;
            shapes.push((format!("shape-{raw}"), drawing));
        }
    }
    Ok(shapes)
}

/// The complete entries of stored `mappings`: the defaults of `extraction`, each
/// replaced by a stored entry with the same page, block and field, then the stored
/// entries that replace none. Regeneration drops those whose content is gone.
pub(super) fn with_default_entries(mut mappings: Value, extraction: &Value) -> Result<Value> {
    let Value::Array(stored) = mappings["entries"].take() else {
        bail!("mappings entries must be an array");
    };
    let mut replacements = BTreeMap::new();
    for (index, entry) in stored.iter().enumerate() {
        replacements.entry(entry_key(entry)?).or_insert(index);
    }
    let mut used = vec![false; stored.len()];
    let mut entries = vec![];
    for default in default_entries(extraction)? {
        match replacements.get(&entry_key(&default)?).copied() {
            Some(index) => {
                used[index] = true;
                entries.push(stored[index].clone());
            }
            None => entries.push(default),
        }
    }
    entries.extend(
        stored
            .into_iter()
            .zip(used)
            .filter_map(|(entry, used)| (!used).then_some(entry)),
    );
    mappings["entries"] = Value::Array(entries);
    Ok(mappings)
}

fn entry_key(entry: &Value) -> Result<(&str, &str, &str)> {
    Ok((
        string(&entry["page"])?,
        string(&entry["block"])?,
        entry["field"].as_str().unwrap_or(""),
    ))
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
    let mut by_page = BTreeMap::new();
    for page in pages {
        let page_id = string(&page["page_id"])?;
        by_page.insert(page_id, page);
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
            let Some(page_value) = by_page.get(page) else {
                continue;
            };
            let Some(rows) = page_value["blocks"][block]["rows"].as_object() else {
                continue;
            };
            let columns = table["columns"]
                .as_array_mut()
                .context("table columns required")?;
            let mut known_columns: BTreeSet<String> = columns
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect();
            for row in rows.values() {
                for column in row.as_object().context("invalid table row")?.keys() {
                    if known_columns.insert(column.clone()) {
                        columns.push(json!(column));
                    }
                }
            }
        }
    }
    Ok(regenerated)
}

/// The extraction as the slide operations of `operations` leave it (see
/// [`slide_view`]): the pages deleted slides leave out, and the copies inserted
/// slides add. The extraction itself when there are none.
pub(super) fn operated_extraction<'a>(
    extraction: &'a Value,
    operations: &Value,
) -> Result<Cow<'a, Value>> {
    let operations = array(operations)?;
    if !operations.iter().any(is_slide_operation) {
        return Ok(Cow::Borrowed(extraction));
    }
    let sheets = array(&extraction["sheets"])?;
    let parsed = parse_slide_operations(operations, sheets)?;
    let mut view = extraction.clone();
    view["sheets"] = Value::Array(slide_view(sheets, &parsed)?);
    Ok(Cow::Owned(view))
}

#[cfg(test)]
mod efficiency_tests {
    use super::*;

    #[test]
    #[ignore = "manual performance measurement"]
    fn benchmark_column_membership() {
        use std::hint::black_box;
        use std::time::Instant;

        let columns: Vec<Value> = (0..100).map(|index| json!(format!("c{index}"))).collect();
        let keys: Vec<String> = (0..1000)
            .flat_map(|_| (0..100).map(|index| format!("c{index}")))
            .collect();

        let start = Instant::now();
        let old = keys
            .iter()
            .filter(|key| columns.iter().any(|value| value == *key))
            .count();
        let old_ms = start.elapsed().as_millis();

        let start = Instant::now();
        let known: BTreeSet<String> = columns
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect();
        let new = keys
            .iter()
            .filter(|key| known.contains(key.as_str()))
            .count();
        let new_ms = start.elapsed().as_millis();
        assert_eq!(black_box(old), black_box(new));
        eprintln!(
            "column_membership old_ms={old_ms} new_ms={new_ms} cells={} columns={}",
            keys.len(),
            columns.len()
        );
    }
}
