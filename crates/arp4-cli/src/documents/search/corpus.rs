use super::super::*;
use crate::document_structure::{Element, ElementCell, Role, Structure};

fn text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        _ => value.to_string(),
    }
}

/// The passage source of `value` at `pointer`, of the cell at `address` of
/// `sheet` or none, which lies at `position` of a text original.
fn source(pointer: String, sheet: &str, address: &str, position: &Value, value: &Value) -> Value {
    let body = text(value);
    json!({"pointer":pointer,"sheet":sheet,"address":address,"position":position,
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
    cells: &BTreeMap<&'a str, &'a ElementCell>,
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
            for header in cell.headers.iter().map(|h| &**h) {
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

/// The IDs of the headers of `cell`.
fn header_ids(cell: &ElementCell) -> Vec<&str> {
    cell.headers.iter().map(|h| &**h).collect()
}

/// Order header cells outermost first: headers higher in the header chain
/// before those they head, then by row and column.
fn ordered<'a>(
    ids: impl IntoIterator<Item = &'a str>,
    cells: &BTreeMap<&'a str, &'a ElementCell>,
    levels: &mut BTreeMap<&'a str, usize>,
) -> Result<Vec<&'a str>> {
    let mut keyed = ids
        .into_iter()
        .map(|id| {
            let (column, row) = excel::coordinate(&cells[id].address)?;
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
    cell: &'a ElementCell,
    cells: &BTreeMap<&'a str, &'a ElementCell>,
    skip: &BTreeSet<&'a str>,
    split_rows: bool,
) -> Result<(BTreeSet<&'a str>, BTreeSet<&'a str>)> {
    headers_from(header_ids(cell), cells, skip, split_rows)
}

/// [`headers_of`] a cell whose own headers are `direct`.
fn headers_from<'a>(
    direct: Vec<&'a str>,
    cells: &BTreeMap<&'a str, &'a ElementCell>,
    skip: &BTreeSet<&'a str>,
    split_rows: bool,
) -> Result<(BTreeSet<&'a str>, BTreeSet<&'a str>)> {
    let mut column = BTreeSet::new();
    let mut rows = BTreeSet::new();
    let mut pending = direct;
    while let Some(id) = pending.pop() {
        if skip.contains(id) || column.contains(id) || rows.contains(id) {
            continue;
        }
        let Some(header) = cells.get(id) else {
            continue;
        };
        if split_rows && header.role == Role::RowHeader {
            rows.insert(id);
            continue;
        }
        column.insert(id);
        pending.extend(header_ids(header));
    }
    Ok((column, rows))
}

/// Header texts in order, each once. A header repeated down a table is
/// headed by the repeat before it, so a cell under the last repeat reaches
/// every repeat, each with the same text.
fn distinct(texts: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    texts
        .into_iter()
        .filter(|text| seen.insert(text.clone()))
        .collect()
}

/// What the headers of a cell lead to, read once for all the cells under the
/// same headers: every header reached, the texts of the column headers in
/// order, each once, and the row headers that qualify the row.
struct Heads<'a> {
    reached: BTreeSet<&'a str>,
    texts: Vec<String>,
    beside: BTreeSet<&'a str>,
}

/// A cell read with the headers above it, as `{headers, text}`; a cell
/// without text gives nothing.
fn field<'a>(
    id: &'a str,
    cells: &BTreeMap<&'a str, &'a ElementCell>,
    levels: &mut BTreeMap<&'a str, usize>,
    skip: &BTreeSet<&'a str>,
    text: &impl Fn(&str) -> Option<String>,
) -> Result<Option<Value>> {
    let Some(value) = text(id) else {
        return Ok(None);
    };
    let (above, _) = headers_of(cells[id], cells, skip, false)?;
    let headers = distinct(ordered(above, cells, levels)?.into_iter().filter_map(text));
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

struct Corpus<'a, E> {
    inspected: &'a Inspection,
    ready: bool,
    /// Takes each passage as it is made: a document's passages together
    /// held several times the memory of the document.
    emit: E,
}

impl<E: FnMut(Value) -> Result<()>> Corpus<'_, E> {
    fn add(
        &mut self,
        kind: &str,
        title: &str,
        sources: Vec<Value>,
        row: Option<&TableRow>,
    ) -> Result<()> {
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
                    // The cells of a wide row by address, looked up once per
                    // source of each fragment.
                    let by_address: HashMap<&str, &RowCell> = row
                        .cells
                        .iter()
                        .rev()
                        .map(|cell| (cell.address.as_str(), cell))
                        .collect();
                    sources
                        .iter()
                        .filter_map(|source| {
                            let cell = by_address.get(source["address"].as_str()?)?;
                            Some((*cell, source["text"].clone()))
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
            (self.emit)(passage)?;
        }
        Ok(())
    }
}

/// Gives `emit` the passages of `inspected`, in the order they are indexed.
pub(super) fn build(
    store: &Store,
    inspected: &Inspection,
    emit: impl FnMut(Value) -> Result<()>,
) -> Result<()> {
    // Searching the adopted extraction remains possible when its original changed.
    // That version is explicitly marked stale by the search response.
    let (structure, report) = if super::super::inspection::structure_format(&inspected.meta)? {
        crate::document_structure::replay_earlier_corrections(
            &store.root,
            &inspected.mappings["interpretation"],
            inspected.original_extraction(),
            string(&inspected.meta["extraction"])?,
            &inspected.axes,
        )?
    } else {
        (Structure::new(json!({}), vec![]), json!({"ready":false}))
    };
    let mut corpus = Corpus {
        inspected,
        ready: report["ready"] == true,
        emit,
    };
    // A cell reads as the workbook shows it: a formula's result under the
    // cell's number format. The stored value and the formula stay beside it.
    // What each cell shows is read once; its source is made when a passage
    // takes it, as the sources of every cell together held several times
    // the memory of the extraction.
    let date1904 = inspected.original_extraction()["date1904"] == true;
    let sheets = &inspected.original_extraction().sheets;
    let mut shown: Vec<Vec<String>> = Vec::with_capacity(sheets.len());
    let mut by_sheet: BTreeMap<&str, HashMap<&str, (usize, usize)>> = BTreeMap::new();
    let mut uncalculated = BTreeSet::new();
    for (si, sheet) in sheets.iter().enumerate() {
        let name = string(&sheet["name"])?;
        let mut texts = Vec::with_capacity(sheet.cells.len());
        let by_address = by_sheet.entry(name).or_default();
        for (ci, cell) in sheet.cells.iter().enumerate() {
            let value = &cell.value().to_json();
            texts.push(if cell.kind() == CellKind::Error {
                text(value)
            } else {
                excel::number_format::display(value, cell.number_format(), date1904)
            });
            let address = cell.address();
            if cell.formula().is_some() && value.is_null() {
                uncalculated.insert((name.to_owned(), address.to_owned()));
            }
            by_address.insert(address, (si, ci));
        }
        shown.push(texts);
    }
    // The source of the `ci`th cell of the `si`th sheet, as a passage holds it.
    let source_of = |(si, ci): (usize, usize)| -> Result<Value> {
        let sheet = &sheets[si];
        let cell = sheet.cells.get(ci).context("passage cell missing")?;
        let shown = &shown[si][ci];
        let mut entry = source(
            format!("/sheets/{si}/cells/{ci}/value"),
            string(&sheet["name"])?,
            cell.address(),
            cell.position().unwrap_or(&Value::Null),
            &json!(shown),
        );
        let value = cell.value().to_json();
        if text(&value) != *shown {
            entry["value"] = value;
        }
        // The index takes the format out into its own tables.
        if let Some(format) = super::format::of(
            crate::extraction::font(sheet, cell)?,
            crate::extraction::style(sheet, cell)?,
        ) {
            entry["format"] = format;
        }
        if let Some(formula) = cell.formula() {
            entry["formula"] = json!(formula);
        }
        Ok(entry)
    };
    let shown_at = |(si, ci): (usize, usize)| shown[si][ci].as_str();
    let mut used = BTreeSet::new();
    let elements = &structure.elements;
    let tables: BTreeMap<&str, &Element> = elements
        .iter()
        .filter_map(|e| Some((e["id"].as_str()?, e)))
        .collect();
    let all_cells: BTreeMap<&str, &ElementCell> = elements
        .iter()
        .flat_map(|e| &e.cells)
        .map(|c| (&*c.id, c))
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
            let address = &*all_cells.get(id)?.address;
            Some(shown_at(*cells.get(address)?).to_owned())
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
                .partition(|id| all_cells[id].role == Role::ColumnHeader);
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
                    let (column, row) = excel::coordinate(&all_cells[id].address)?;
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
        let mut rows: BTreeMap<u32, Vec<(u32, &ElementCell)>> = BTreeMap::new();
        for cell in &element.cells {
            let address = &*cell.address;
            let (column, row) = excel::coordinate(address)?;
            used.insert((name.to_owned(), address.to_owned()));
            rows.entry(row).or_default().push((column, cell));
        }
        let title = format!(
            "{} / {name}",
            inspected.meta["document_id"].as_str().unwrap()
        );
        // Cells under the same headers share what the headers lead to; a
        // table repeating its header every few rows chains each repeat to
        // the one before, so reading the chain for every cell grows with
        // the cells times the repeats.
        let mut heads: HashMap<Vec<&str>, std::rc::Rc<Heads<'_>>> = HashMap::new();
        let mut fields: HashMap<&str, (BTreeSet<&str>, Option<Value>)> = HashMap::new();
        for (row, mut members) in rows {
            members.sort_by_key(|(column, _)| *column);
            // Headers within the row are values of the row; they head no column.
            let in_row: BTreeSet<&str> = members.iter().map(|(_, cell)| &*cell.id).collect();
            let mut row_headers = BTreeSet::new();
            let mut row_cells = vec![];
            let mut sources = vec![];
            for (_, cell) in members {
                let address = &*cell.address;
                // A header in the row is passed over, so it is no part of
                // what the cell shares with the cells of other rows.
                let mut direct: Vec<&str> = header_ids(cell)
                    .into_iter()
                    .filter(|id| !in_row.contains(id))
                    .collect();
                direct.sort_unstable();
                direct.dedup();
                let shared = match heads.get(&direct) {
                    Some(shared) => shared.clone(),
                    None => {
                        let (over, beside) =
                            headers_from(direct.clone(), &all_cells, &BTreeSet::new(), true)?;
                        let shared = std::rc::Rc::new(Heads {
                            reached: over.iter().chain(&beside).copied().collect(),
                            texts: distinct(
                                ordered(over, &all_cells, &mut levels)?
                                    .into_iter()
                                    .filter_map(text),
                            ),
                            beside,
                        });
                        heads.insert(direct, shared.clone());
                        shared
                    }
                };
                // A header in the cell's own row is a value of the row, not a
                // header: such a cell is read on its own.
                let headers = if in_row.iter().any(|id| shared.reached.contains(id)) {
                    let (over, beside) = headers_of(cell, &all_cells, &in_row, true)?;
                    row_headers.extend(beside);
                    distinct(
                        ordered(over, &all_cells, &mut levels)?
                            .into_iter()
                            .filter_map(text),
                    )
                } else {
                    row_headers.extend(shared.beside.iter().copied());
                    shared.texts.clone()
                };
                let original = cells.get(address).copied();
                if let Some(original) = original {
                    sources.push(source_of(original)?);
                }
                let unread = Some(cell.text_state.as_str())
                    .filter(|state| ["unreadable", "not_examined"].contains(state))
                    .or_else(|| {
                        uncalculated
                            .contains(&(name.to_owned(), address.to_owned()))
                            .then_some("uncalculated")
                    });
                let value = original
                    .map(shown_at)
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
                // A row header heads many rows; it is read once for them all,
                // unless what heads it lies in this row.
                let (reached, read) = match fields.get(id) {
                    Some(known) => known,
                    None => {
                        let (above, _) =
                            headers_of(all_cells[id], &all_cells, &BTreeSet::new(), false)?;
                        let read = field(id, &all_cells, &mut levels, &BTreeSet::new(), &text)?;
                        fields.insert(id, (above, read));
                        &fields[id]
                    }
                };
                if in_row.iter().any(|id| reached.contains(id)) {
                    context.extend(field(id, &all_cells, &mut levels, &in_row, &text)?);
                } else {
                    context.extend(read.clone());
                }
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
            )?;
        }
    }
    for (si, sheet) in inspected.original_extraction().sheets.iter().enumerate() {
        let name = string(&sheet["name"])?;
        let mut rows: BTreeMap<u32, Vec<Value>> = BTreeMap::new();
        for cell in &sheet.cells {
            let address = cell.address();
            if used.contains(&(name.to_owned(), address.to_owned())) {
                continue;
            }
            let original = source_of(by_sheet[name][address])?;
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
            corpus.add(kind, &title, sources, None)?;
        }
        for (key, kind, fields) in [
            ("comments", "comment", vec!["text"]),
            ("drawings", "drawing", vec!["text", "description"]),
        ] {
            for (i, object) in sheet[key].as_array().into_iter().flatten().enumerate() {
                // The paragraphs of a slide's shapes are cells of the slide,
                // which are indexed already; their description is not.
                let slide = key == "drawings" && object["anchor"]["kind"] == "slide";
                let sources = fields
                    .iter()
                    .filter(|field| object[**field].is_string() && !(slide && **field == "text"))
                    .map(|field| {
                        let mut entry = source(
                            format!("/sheets/{si}/{key}/{i}/{field}"),
                            name,
                            object["address"].as_str().unwrap_or(""),
                            object.get("position").unwrap_or(&Value::Null),
                            &object[*field],
                        );
                        // A shape's font and emphasis are those of its text, not of
                        // its description.
                        if *field == "text"
                            && let Some(format) =
                                super::format::of(&object["font"], &object["style"])
                        {
                            entry["format"] = format;
                        }
                        entry
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
                )?;
            }
        }
    }
    for (i, asset) in array(&inspected.original_extraction()["assets"])?
        .iter()
        .enumerate()
    {
        if asset["ocr"]["text"].is_string() {
            corpus.add(
                "ocr",
                string(&inspected.meta["document_id"])?,
                vec![source(
                    format!("/assets/{i}/ocr/text"),
                    "",
                    "",
                    &Value::Null,
                    &asset["ocr"]["text"],
                )],
                None,
            )?;
        }
    }
    Ok(())
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
