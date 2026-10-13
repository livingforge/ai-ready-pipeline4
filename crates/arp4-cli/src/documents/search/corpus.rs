use super::super::*;

fn text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        _ => value.to_string(),
    }
}

fn source(pointer: String, sheet: &str, cell: &Value, value: &Value) -> Value {
    let body = text(value);
    json!({"pointer":pointer,"sheet":sheet,"address":cell["address"].as_str().unwrap_or(""),
        "position":cell.get("position").cloned().unwrap_or(Value::Null),
        "start":0,"end":body.chars().count(),"text":body})
}

/// Bounded fragments retain exact offsets within each extracted value. Long
/// Markdown/code blocks are split; whole normal table rows stay together.
fn fragments(sources: Vec<Value>) -> Vec<Vec<Value>> {
    const SIZE: usize = 1500;
    let mut rows = vec![];
    let mut row = vec![];
    let mut used = 0;
    for mut source in sources {
        let body = source["text"].take();
        let chars: Vec<_> = body.as_str().unwrap().chars().collect();
        if chars.is_empty() {
            continue;
        }
        let mut start = 0;
        loop {
            let end = (start + SIZE).min(chars.len());
            if used + end - start > SIZE && !row.is_empty() {
                rows.push(std::mem::take(&mut row));
                used = 0;
            }
            let mut part = source.clone();
            part["text"] = json!(chars[start..end].iter().collect::<String>());
            part["start"] = json!(start);
            part["end"] = json!(end);
            used += end - start;
            row.push(part);
            if end == chars.len() {
                break;
            }
            rows.push(std::mem::take(&mut row));
            used = 0;
            start = end - 80;
        }
    }
    if !row.is_empty() {
        rows.push(row);
    }
    rows
}

/// How many header links lie above a header cell; a heading without headers of
/// its own is 0. Validated header links are acyclic; a cycle adds no level.
fn header_level<'a>(
    id: &'a str,
    cells: &BTreeMap<&'a str, &'a Value>,
    levels: &mut BTreeMap<&'a str, usize>,
) -> Result<usize> {
    let mut stack = vec![(id, false)];
    let mut open = BTreeSet::new();
    while let Some((current, expanded)) = stack.pop() {
        if levels.contains_key(current) {
            continue;
        }
        let mut above = vec![];
        if let Some(cell) = cells.get(current) {
            for header in array(&cell["headers"])?.iter().filter_map(Value::as_str) {
                if cells.contains_key(header) {
                    above.push(header);
                }
            }
        }
        if expanded {
            open.remove(current);
            let level = above
                .iter()
                .filter_map(|header| levels.get(header))
                .map(|level| level + 1)
                .max()
                .unwrap_or(0);
            levels.insert(current, level);
        } else if open.insert(current) {
            stack.push((current, true));
            stack.extend(
                above
                    .into_iter()
                    .filter(|header| !open.contains(header))
                    .map(|header| (header, false)),
            );
        }
    }
    Ok(levels[id])
}

fn ids(value: &Value) -> Result<Vec<&str>> {
    Ok(array(value)?.iter().filter_map(Value::as_str).collect())
}

/// Order header cells outermost first: headers higher in the header chain
/// before those they head, then by row and column.
fn ordered<'a>(
    ids: impl IntoIterator<Item = &'a str>,
    cells: &BTreeMap<&'a str, &'a Value>,
    levels: &mut BTreeMap<&'a str, usize>,
) -> Result<Vec<&'a str>> {
    let mut keyed = ids
        .into_iter()
        .map(|id| {
            let (column, row) = excel::coordinate(string(&cells[id]["address"])?)?;
            Ok(((header_level(id, cells, levels)?, row, column), id))
        })
        .collect::<Result<Vec<_>>>()?;
    keyed.sort();
    keyed.dedup();
    Ok(keyed.into_iter().map(|(_, id)| id).collect())
}

/// The headers above a cell, not passing through `skip`. With `split_rows`, a
/// row header qualifies the row rather than the column: it is returned apart
/// and what heads it is left to its own path.
fn headers_of<'a>(
    cell: &'a Value,
    cells: &BTreeMap<&'a str, &'a Value>,
    skip: &BTreeSet<&'a str>,
    split_rows: bool,
) -> Result<(BTreeSet<&'a str>, BTreeSet<&'a str>)> {
    let mut column = BTreeSet::new();
    let mut rows = BTreeSet::new();
    let mut pending = ids(&cell["headers"])?;
    while let Some(id) = pending.pop() {
        if skip.contains(id) || column.contains(id) || rows.contains(id) {
            continue;
        }
        let Some(header) = cells.get(id) else {
            continue;
        };
        if split_rows && header["role"] == "row_header" {
            rows.insert(id);
            continue;
        }
        column.insert(id);
        pending.extend(ids(&header["headers"])?);
    }
    Ok((column, rows))
}

/// A cell read with the headers above it, as `{headers, text}`; a cell
/// without text gives nothing.
fn field<'a>(
    id: &'a str,
    cells: &BTreeMap<&'a str, &'a Value>,
    levels: &mut BTreeMap<&'a str, usize>,
    skip: &BTreeSet<&'a str>,
    text: &impl Fn(&str) -> Option<String>,
) -> Result<Option<Value>> {
    let Some(value) = text(id) else {
        return Ok(None);
    };
    let (above, _) = headers_of(cells[id], cells, skip, false)?;
    let headers: Vec<_> = ordered(above, cells, levels)?
        .into_iter()
        .filter_map(text)
        .collect();
    Ok(Some(json!({"headers":headers,"text":value})))
}

/// One row of an interpreted table: every cell of the row in column order
/// with the column headers above it, the row headers of other rows that
/// qualify it, and the table it belongs to.
struct TableRow {
    table: Value,
    row: u32,
    cells: Vec<RowCell>,
    context: Vec<Value>,
}

struct RowCell {
    address: String,
    column: String,
    headers: Vec<String>,
    /// None when a reader could not read the cell or has not examined it.
    text: Option<String>,
    unread: Option<String>,
}

/// The headings a passage sits under, as the index searches them: the
/// table's headings and parent records, its column headers and the row
/// headers of its row.
pub(super) fn headings(passage: &Value) -> String {
    let table = &passage["table"];
    let list = |value: &Value| value.as_array().cloned().unwrap_or_default();
    let fields = |value: &Value| {
        list(value)
            .into_iter()
            .flat_map(|f| list(&f["headers"]).into_iter().chain([f["text"].clone()]))
            .collect::<Vec<_>>()
    };
    let mut texts: Vec<String> = vec![];
    for text in list(&table["headings"])
        .into_iter()
        .chain(fields(&table["within"]))
        .chain(
            list(&table["columns"])
                .iter()
                .flat_map(|c| list(&c["headers"])),
        )
        .chain(fields(&passage["row_context"]))
    {
        if let Some(text) = text.as_str()
            && !texts.iter().any(|t| t == text)
        {
            texts.push(text.to_owned());
        }
    }
    texts.join(" | ")
}

struct Corpus<'a> {
    inspected: &'a Inspection,
    ready: bool,
    passages: Vec<Value>,
}

impl Corpus<'_> {
    fn add(&mut self, kind: &str, title: &str, sources: Vec<Value>, row: Option<&TableRow>) {
        let parts = fragments(sources);
        let whole = parts.len() == 1;
        for sources in parts {
            let body = sources
                .iter()
                .map(|s| s["text"].as_str().unwrap())
                .collect::<Vec<_>>()
                .join(" | ");
            if body.trim().is_empty() {
                continue;
            }
            let mut passage = json!({"document_id":self.inspected.meta["document_id"],
                "source_path":self.inspected.meta["source"]["path"],
                "extraction_sha256":self.inspected.meta["extraction"],
                "kind":kind,"title":title,"text":body,"sources":sources,
                "structure_reviewed":self.ready});
            if let Some(row) = row {
                // A row split into fragments lists the pieces each fragment holds.
                let cells: Vec<(&RowCell, Value)> = if whole {
                    row.cells
                        .iter()
                        .map(|cell| (cell, json!(cell.text)))
                        .collect()
                } else {
                    sources
                        .iter()
                        .filter_map(|source| {
                            let cell = row.cells.iter().find(|c| c.address == source["address"])?;
                            Some((cell, source["text"].clone()))
                        })
                        .collect()
                };
                let mut table = row.table.clone();
                table["columns"] = cells
                    .iter()
                    .map(|(cell, _)| json!({"column":cell.column,"headers":cell.headers}))
                    .collect();
                passage["table"] = table;
                passage["row"] = json!(row.row);
                passage["values"] = cells.iter().map(|(_, value)| value.clone()).collect();
                if !row.context.is_empty() {
                    passage["row_context"] = json!(row.context);
                }
                let unread: serde_json::Map<String, Value> = cells
                    .iter()
                    .filter_map(|(cell, _)| {
                        Some((cell.address.clone(), json!(cell.unread.as_ref()?)))
                    })
                    .collect();
                if !unread.is_empty() {
                    passage["unread"] = Value::Object(unread);
                }
            }
            self.passages.push(passage);
        }
    }
}

pub(super) fn build(store: &Store, inspected: &Inspection) -> Result<Vec<Value>> {
    // Searching the adopted extraction remains possible when its original changed.
    // That version is explicitly marked stale by the search response.
    let (structure, report) = if super::super::inspection::structure_format(&inspected.meta)? {
        crate::document_structure::replay_earlier_corrections(
            &store.root,
            &inspected.mappings["interpretation"],
            &inspected.extraction,
            string(&inspected.meta["extraction"])?,
            &inspected.axes,
        )?
    } else {
        (json!({"elements":[]}), json!({"ready":false}))
    };
    let mut corpus = Corpus {
        inspected,
        ready: report["ready"] == true,
        passages: vec![],
    };
    // A cell reads as the workbook shows it: a formula's result under the
    // cell's number format. The stored value and the formula stay beside it.
    let date1904 = inspected.extraction["date1904"] == true;
    let mut by_sheet: BTreeMap<String, BTreeMap<String, Value>> = BTreeMap::new();
    let mut uncalculated = BTreeSet::new();
    for (si, sheet) in array(&inspected.extraction["sheets"])?.iter().enumerate() {
        let name = string(&sheet["name"])?;
        for (ci, cell) in array(&sheet["cells"])?.iter().enumerate() {
            let value = &cell["value"];
            let shown = if cell["type"] == "error" {
                text(value)
            } else {
                excel::number_format::display(
                    value,
                    cell["number_format"].as_str().unwrap_or(""),
                    date1904,
                )
            };
            let address = string(&cell["address"])?;
            let mut entry = source(
                format!("/sheets/{si}/cells/{ci}/value"),
                name,
                cell,
                &json!(shown),
            );
            if text(value) != shown {
                entry["value"] = value.clone();
            }
            if let Some(formula) = cell["formula"].as_str() {
                entry["formula"] = json!(formula);
                if value.is_null() {
                    uncalculated.insert((name.to_owned(), address.to_owned()));
                }
            }
            by_sheet
                .entry(name.into())
                .or_default()
                .insert(address.into(), entry);
        }
    }
    let mut used = BTreeSet::new();
    let elements = array(&structure["elements"])?;
    let tables: BTreeMap<&str, &Value> = elements
        .iter()
        .filter_map(|e| Some((e["id"].as_str()?, e)))
        .collect();
    let all_cells: BTreeMap<&str, &Value> = elements
        .iter()
        .flat_map(|e| e["cells"].as_array().into_iter().flatten())
        .filter_map(|c| Some((c["id"].as_str()?, c)))
        .collect();
    // All linked headers are explicit cells in the interpretation, including merged
    // and multi-level headers; no first-row-is-a-header heuristic is added here.
    let mut levels = BTreeMap::new();
    for element in elements {
        if element["kind"] != "table" {
            continue;
        }
        let name = string(&element["sheet"])?;
        let Some(cells) = by_sheet.get(name) else {
            continue;
        };
        let text = |id: &str| -> Option<String> {
            let address = all_cells.get(id)?["address"].as_str()?;
            Some(cells.get(address)?["text"].as_str()?.to_owned())
        };
        // A nested table sits under the context its parents give it: their
        // column headers above it, and the cells of the parent records it
        // belongs to, each with its own headers. Outermost first.
        let mut contexts = vec![];
        let mut current = element;
        while let Some(parent) = current.get("parent")
            && contexts.len() < elements.len()
        {
            contexts.push(ids(&parent["context"])?);
            let Some(next) = parent["element"].as_str().and_then(|id| tables.get(id)) else {
                break;
            };
            current = next;
        }
        let mut headings: Vec<String> = vec![];
        let mut within = vec![];
        for context in contexts.into_iter().rev() {
            let (above, records): (Vec<&str>, Vec<&str>) = context
                .into_iter()
                .filter(|id| all_cells.contains_key(id))
                .partition(|id| all_cells[id]["role"] == "column_header");
            let records: BTreeSet<&str> = records.into_iter().collect();
            for id in above {
                let (over, _) = headers_of(all_cells[id], &all_cells, &records, false)?;
                for header in ordered(over.into_iter().chain([id]), &all_cells, &mut levels)? {
                    if let Some(text) = text(header)
                        && !headings.contains(&text)
                    {
                        headings.push(text);
                    }
                }
            }
            let mut placed = records
                .iter()
                .map(|id| {
                    let (column, row) = excel::coordinate(string(&all_cells[id]["address"])?)?;
                    Ok(((row, column), *id))
                })
                .collect::<Result<Vec<_>>>()?;
            placed.sort();
            for (_, id) in placed {
                within.extend(field(id, &all_cells, &mut levels, &records, &text)?);
            }
        }
        let mut table = json!({"element":element["id"],"sheet":name});
        if !headings.is_empty() {
            table["headings"] = json!(headings);
        }
        if !within.is_empty() {
            table["within"] = json!(within);
        }
        let mut rows: BTreeMap<u32, Vec<(u32, &Value)>> = BTreeMap::new();
        for cell in array(&element["cells"])? {
            let address = string(&cell["address"])?;
            let (column, row) = excel::coordinate(address)?;
            used.insert((name.to_owned(), address.to_owned()));
            rows.entry(row).or_default().push((column, cell));
        }
        let title = format!(
            "{} / {name}",
            inspected.meta["document_id"].as_str().unwrap()
        );
        for (row, mut members) in rows {
            members.sort_by_key(|(column, _)| *column);
            // Headers within the row are values of the row; they head no column.
            let in_row: BTreeSet<&str> = members
                .iter()
                .filter_map(|(_, cell)| cell["id"].as_str())
                .collect();
            let mut row_headers = BTreeSet::new();
            let mut row_cells = vec![];
            let mut sources = vec![];
            for (_, cell) in members {
                let address = string(&cell["address"])?;
                let (over, beside) = headers_of(cell, &all_cells, &in_row, true)?;
                row_headers.extend(beside);
                let headers = ordered(over, &all_cells, &mut levels)?
                    .into_iter()
                    .filter_map(text)
                    .collect();
                let original = cells.get(address);
                if let Some(original) = original {
                    sources.push(original.clone());
                }
                let unread = cell["text_state"]
                    .as_str()
                    .filter(|state| ["unreadable", "not_examined"].contains(state))
                    .or_else(|| {
                        uncalculated
                            .contains(&(name.to_owned(), address.to_owned()))
                            .then_some("uncalculated")
                    });
                let value = original
                    .and_then(|o| o["text"].as_str())
                    .filter(|_| unread != Some("uncalculated"))
                    .map(str::to_owned)
                    .or_else(|| unread.is_none().then(String::new));
                row_cells.push(RowCell {
                    address: address.to_owned(),
                    column: address
                        .trim_end_matches(|c: char| c.is_ascii_digit())
                        .to_owned(),
                    headers,
                    text: value,
                    unread: unread.map(str::to_owned),
                });
            }
            let mut context = vec![];
            for id in ordered(row_headers, &all_cells, &mut levels)? {
                context.extend(field(id, &all_cells, &mut levels, &in_row, &text)?);
            }
            corpus.add(
                "row",
                &title,
                sources,
                Some(&TableRow {
                    table: table.clone(),
                    row,
                    cells: row_cells,
                    context,
                }),
            );
        }
    }
    for (si, sheet) in array(&inspected.extraction["sheets"])?.iter().enumerate() {
        let name = string(&sheet["name"])?;
        let mut rows: BTreeMap<u32, Vec<Value>> = BTreeMap::new();
        for cell in array(&sheet["cells"])? {
            let address = string(&cell["address"])?;
            if used.contains(&(name.to_owned(), address.to_owned())) {
                continue;
            }
            let original = by_sheet[name][address].clone();
            let (_, row) = excel::coordinate(address)?;
            rows.entry(row).or_default().push(original);
        }
        for (_, mut sources) in rows {
            sources.sort_by_key(|s| excel::coordinate(s["address"].as_str().unwrap()).unwrap().0);
            let headings: BTreeSet<_> = sources
                .iter()
                .flat_map(|s| {
                    s["position"]["headings"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                })
                .collect();
            let title = format!(
                "{} / {name} / {}",
                inspected.meta["document_id"].as_str().unwrap(),
                headings.into_iter().collect::<Vec<_>>().join(" / ")
            );
            let kind = if sources.len() == 1 { "block" } else { "row" };
            corpus.add(kind, &title, sources, None);
        }
        for (key, kind, fields) in [
            ("comments", "comment", vec!["text"]),
            ("drawings", "drawing", vec!["text", "description"]),
        ] {
            for (i, object) in sheet[key].as_array().into_iter().flatten().enumerate() {
                let sources = fields
                    .iter()
                    .filter(|field| object[**field].is_string())
                    .map(|field| {
                        source(
                            format!("/sheets/{si}/{key}/{i}/{field}"),
                            name,
                            object,
                            &object[*field],
                        )
                    })
                    .collect();
                corpus.add(
                    kind,
                    &format!(
                        "{} / {name}",
                        inspected.meta["document_id"].as_str().unwrap()
                    ),
                    sources,
                    None,
                );
            }
        }
    }
    for (i, asset) in array(&inspected.extraction["assets"])?.iter().enumerate() {
        if asset["ocr"]["text"].is_string() {
            corpus.add(
                "ocr",
                string(&inspected.meta["document_id"])?,
                vec![source(
                    format!("/assets/{i}/ocr/text"),
                    "",
                    &Value::Null,
                    &asset["ocr"]["text"],
                )],
                None,
            );
        }
    }
    Ok(corpus.passages)
}

#[cfg(test)]
mod efficiency_tests {
    use super::*;

    #[test]
    #[ignore = "manual performance measurement"]
    fn benchmark_long_source_fragments() {
        use std::time::Instant;

        let source = json!({"pointer":"/long","sheet":"Text","address":"A1",
            "position":null,"start":0,"end":2_000_000,"text":"a".repeat(2_000_000)});
        let old_source = source.clone();
        let old_start = Instant::now();
        let chars: Vec<_> = old_source["text"].as_str().unwrap().chars().collect();
        let mut old = vec![];
        let mut offset = 0;
        loop {
            let end = (offset + 1500).min(chars.len());
            let mut part = old_source.clone();
            part["text"] = json!(chars[offset..end].iter().collect::<String>());
            part["start"] = json!(offset);
            part["end"] = json!(end);
            old.push(part);
            if end == chars.len() {
                break;
            }
            offset = end - 80;
        }
        let old_ms = old_start.elapsed().as_millis();
        let start = Instant::now();
        let result = fragments(vec![source]);
        eprintln!(
            "long_source_fragments old_ms={old_ms} new_ms={} fragments={}",
            start.elapsed().as_millis(),
            result.len()
        );
        assert!(result.len() > 1_000);
        assert_eq!(old, result.into_iter().flatten().collect::<Vec<_>>());
    }
}
