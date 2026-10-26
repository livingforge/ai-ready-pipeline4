use super::*;

fn packet(count: usize, length: usize) -> Value {
    json!({"packet":"fixture","document":"doc","sources":{"tables":[{"sheet":"A"},{"sheet":"B"}],
        "rows":(0..count).map(|i|json!([format!("s{}",i+1),i%2,"A1","value","語".repeat(length)])).collect::<Vec<_>>()}})
}

#[test]
fn short_cells_stay_in_one_document_and_large_payloads_split_without_loss() {
    let small = packet(178, 5);
    assert!(
        partitions(&small, &[], 0, 0, 98304, &json!({}))
            .unwrap()
            .is_empty()
    );
    let large = packet(180, 400);
    let regions = partitions(&large, &[], 0, 0, 98304, &json!({})).unwrap();
    assert!(regions.len() > 2);
    let mut seen = BTreeSet::new();
    for region in &regions {
        let scoped = scoped_packet(&large, region, &json!({})).unwrap();
        assert!(task_text(&scoped).len() <= 98304);
        for source in region["sources"].as_array().unwrap() {
            assert!(seen.insert(source.as_str().unwrap()));
        }
    }
    assert_eq!(seen.len(), 180);
    assert!(
        !partitions(&small, &[], 0, 120, 98304, &json!({}))
            .unwrap()
            .is_empty()
    );
    assert!(partitions(&packet(1, 50000), &[], 0, 0, 98304, &json!({})).is_err());
}

/// Grows each partition one source at a time, building every candidate packet.
fn row_by_row(
    packet: &Value,
    max_chars: usize,
    max_sources: usize,
    max_bytes: usize,
) -> Result<Vec<Value>> {
    let rows = arr(&packet["sources"]["rows"])?;
    let tables = arr(&packet["sources"]["tables"])?;
    let mut sheets: BTreeMap<String, Vec<&Value>> = BTreeMap::new();
    for row in rows {
        let table = &tables[row[1].as_u64().unwrap() as usize];
        sheets
            .entry(table["sheet"].to_string())
            .or_default()
            .push(row);
    }
    let mut result = Vec::new();
    for sheet in sheets.values() {
        let mut start = 0;
        while start < sheet.len() {
            let (mut end, mut chars) = (start, 0);
            while end < sheet.len() && end - start < max_sources {
                let size = s(&sheet[end][4])?.chars().count();
                if end > start && chars + size > max_chars {
                    break;
                }
                let candidate = region(sheet, start, end + 1);
                let bytes = task_text(&scoped_packet(packet, &candidate, &json!({}))?).len();
                if bytes > max_bytes {
                    ensure!(end > start, "byte limit");
                    break;
                }
                chars += size;
                end += 1;
            }
            result.push(region(sheet, start, end));
            start = end;
        }
    }
    Ok(result)
}

#[test]
fn bisected_partitions_match_growing_one_source_at_a_time() {
    // Uneven text lengths, so byte, character and source limits each end partitions.
    let rows: Vec<_> = (0..240)
        .map(|i| {
            json!([
                format!("s{}", i + 1),
                i % 3 / 2,
                "A1",
                "value",
                "語".repeat(1 + (i * 37) % 290)
            ])
        })
        .collect();
    let large = json!({"packet":"fixture","document":"doc",
        "sources":{"tables":[{"sheet":"A"},{"sheet":"B"}],"rows":rows}});
    let base =
        task_text(&scoped_packet(&large, &json!({"sources":[],"context":[]}), &json!({})).unwrap())
            .len();
    for (max_chars, max_sources, extra) in [
        (usize::MAX, usize::MAX, 20_000),
        (usize::MAX, usize::MAX, 60_000),
        (5_000, usize::MAX, 90_000),
        (usize::MAX, 7, 90_000),
        (3_000, 25, 40_000),
    ] {
        let max_bytes = base + extra;
        let expected = row_by_row(&large, max_chars, max_sources, max_bytes).unwrap();
        let actual =
            partitions(&large, &[], max_chars, max_sources, max_bytes, &json!({})).unwrap();
        assert!(expected.len() > 1);
        assert_eq!(
            actual, expected,
            "limits {max_chars} {max_sources} {max_bytes}"
        );
    }
}

#[test]
fn partitions_show_only_their_cells_headers_and_merges() {
    let cell = |id: &str, address: &str, role: &str, headers: Value| json!({"id":id,"address":address,"role":role,"headers":headers,"text_state":"read"});
    let element = |id: &str, kind: &str, cells: Vec<Value>| json!({"id":id,"kind":kind,"sheet":"A","cells":cells,"evidence":[],"reading":{}});
    let mut grid = element(
        "grid",
        "table",
        vec![
            cell("h", "A2", "column_header", json!(["t"])),
            cell("a3", "A3", "data", json!(["h"])),
            cell("a4", "A4", "data", json!(["h"])),
        ],
    );
    grid["descriptions"] = json!(["title", "notes"]);
    let structure = json!({"document":"doc","regions":[],"visuals":[],"elements":[
        element("title", "text", vec![cell("t", "A1", "text", json!([]))]),
        grid,
        element("notes", "text", vec![cell("n", "D9", "note", json!([]))]),
    ]});
    let packet = json!({"packet":"fixture","document":"doc","sources":{
        "tables":[{"document":"doc","sheet":"A","merges":["A1:B1","A3:B3","D9:E9"],"structure":structure},
                  {"document":"doc","sheet":"B","merges":["A1:C1"]}],
        "rows":[["s1",0,"A3","value","x"],["s2",0,"A4","value","y"],["s3",1,"A1","value","z"]]}});
    let scoped =
        scoped_packet(&packet, &json!({"sources":["s1"],"context":[]}), &json!({})).unwrap();
    let tables = arr(&scoped["sources"]["tables"]).unwrap();
    assert_eq!(
        tables.len(),
        2,
        "table indexes stay valid for request_tables"
    );
    assert_eq!(tables[0]["merges"], json!(["A1:B1", "A3:B3"]));
    assert_eq!(tables[1]["merges"], json!([]));
    let elements = arr(&tables[0]["structure"]["elements"]).unwrap();
    assert_eq!(elements.len(), 2);
    assert_eq!(elements[0]["id"], "title");
    assert!(elements[0].get("omitted_cells").is_none());
    assert_eq!(elements[1]["id"], "grid");
    let ids: Vec<_> = arr(&elements[1]["cells"])
        .unwrap()
        .iter()
        .map(|c| c["id"].clone())
        .collect();
    assert_eq!(ids, [json!("h"), json!("a3")]);
    assert_eq!(elements[1]["omitted_cells"], 1);
    assert_eq!(elements[1]["descriptions"], json!(["title"]));
    assert_eq!(
        scoped["sources"]["rows"],
        json!([["s1", 0, "A3", "value", "x"]])
    );
}

/// A small deterministic generator for the comparisons below.
struct Draws(u64);
impl Draws {
    fn next(&mut self, below: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % below as u64) as usize
    }
}

/// Cells on two sheets with headers, nested-table contexts, descriptions and
/// merges, some overlapping and some inverted.
fn structured_packet(draws: &mut Draws, rows: usize) -> Value {
    let address =
        |column: usize, row: usize| format!("{}{}", (b'A' + column as u8) as char, row + 1);
    let mut tables = Vec::new();
    for sheet in ["A", "B"] {
        let mut elements = Vec::new();
        let mut id = 0;
        for e in 0..4 {
            let cells: Vec<_> = (0..1 + draws.next(6))
                .map(|_| {
                    id += 1;
                    let headers: Vec<_> = (0..draws.next(3))
                        .map(|_| json!(format!("{sheet}c{}", 1 + draws.next(20))))
                        .collect();
                    json!({"id":format!("{sheet}c{id}"),"address":address(draws.next(4), draws.next(rows)),
                        "role":"data","headers":headers})
                })
                .collect();
            let mut element = json!({"id":format!("{sheet}e{e}"),"kind":"table","sheet":sheet,"cells":cells,
                "descriptions":[format!("{sheet}e{}", draws.next(5))]});
            if draws.next(2) == 0 {
                element["parent"] = json!({"element":format!("{sheet}e{}", draws.next(5)),
                    "context":[format!("{sheet}c{}", 1 + draws.next(20))]});
            }
            elements.push(element);
        }
        let merges: Vec<_> = (0..draws.next(8))
            .map(|_| {
                let (top, left) = (draws.next(rows), draws.next(4));
                let (bottom, right) = (draws.next(rows), draws.next(4));
                json!(format!("{}:{}", address(left, top), address(right, bottom)))
            })
            .collect();
        tables.push(json!({"document":"doc","sheet":sheet,"merges":merges,
            "structure":{"document":"doc","regions":[],"elements":elements}}));
    }
    tables.push(json!({"document":"doc"}));
    let rows: Vec<_> = (0..rows)
        .map(|i| {
            let table = draws.next(3);
            json!([
                format!("s{}", i + 1),
                table,
                address(draws.next(4), i),
                "value",
                "text"
            ])
        })
        .collect();
    json!({"packet":"fixture","document":"doc","warnings":[],"sources":{"columns":["ref","table","cell_or_location","field","text"],
        "tables":tables,"rows":rows}})
}

#[test]
fn indexed_scopes_match_scanning_every_row_and_cell() {
    let mut draws = Draws(0x9e37_79b9_7f4a_7c15);
    // Regions showing some merges and leaving out some cells, so both are compared.
    let (mut merged, mut trimmed) = (0, 0);
    for _ in 0..40 {
        let rows = 4 + draws.next(30);
        let packet = structured_packet(&mut draws, rows);
        let modules = json!({"app":"Application"});
        let scoper = Scoper::new(&packet, &modules);
        for _ in 0..12 {
            let pick = |draws: &mut Draws| {
                (0..draws.next(6))
                    .map(|_| json!(format!("s{}", 1 + draws.next(rows + 2))))
                    .collect::<Vec<_>>()
            };
            let region = json!({"sources":pick(&mut draws),"context":pick(&mut draws)});
            let expected = scanned_packet(&packet, &region, &modules).unwrap();
            assert_eq!(scoper.scoped(&region).unwrap(), expected, "{region}");
            for table in arr(&expected["sources"]["tables"]).unwrap() {
                merged += table["merges"].as_array().is_some_and(|m| !m.is_empty()) as usize;
                trimmed += table["structure"]["elements"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|e| e.get("omitted_cells").is_some())
                    .count();
            }
        }
    }
    assert!(merged > 0 && trimmed > 0, "{merged} {trimmed}");
}

// The scoped packet as it was made before the packet was indexed: every row,
// structure cell and merge read for each region. Kept as the reference.
fn scanned_packet(packet: &Value, region: &Value, modules: &Value) -> Result<Value> {
    let wanted: BTreeSet<_> = arr(&region["sources"])?
        .iter()
        .chain(arr(&region["context"])?)
        .map(|r| s(r))
        .collect::<Result<_>>()?;
    // Copied without its rows, of which only the region's are kept.
    let mut result = Value::Object(
        packet
            .as_object()
            .context("packet must be an object")?
            .iter()
            .filter(|(key, _)| *key != "sources")
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    );
    let mut sources: Map<String, Value> = packet["sources"]
        .as_object()
        .context("packet sources must be an object")?
        .iter()
        .filter(|(key, _)| *key != "rows" && *key != "tables")
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    let rows: Vec<&Value> = arr(&packet["sources"]["rows"])?
        .iter()
        .filter(|r| wanted.contains(r[0].as_str().unwrap_or("")))
        .collect();
    sources.insert(
        "tables".into(),
        json!(scanned_tables(arr(&packet["sources"]["tables"])?, &rows)?),
    );
    sources.insert("rows".into(), json!(rows));
    result["sources"] = Value::Object(sources);
    result["parent_packet"] = packet["packet"].clone();
    result["scope"] = region.clone();
    result["modules"] = modules.clone();
    result.as_object_mut().unwrap().remove("packet");
    result["packet"] = json!(hash(&encode(&result)));
    Ok(result)
}

/// The packet's tables as a partition shows them. A table's merges and structure
/// describe the whole sheet; a partition keeps the structure cells for its rows
/// and the headers they name, headers of headers included, and the merges
/// covering any of those cells, so a partition of a large sheet stays small.
/// `omitted_cells` counts the element cells left out. Table indexes are kept for
/// `request_tables`.
fn scanned_tables(tables: &[Value], rows: &[&Value]) -> Result<Vec<Value>> {
    let mut shown = SheetCells::new();
    for row in rows {
        let table = &tables[row[1].as_u64().context("table index required")? as usize];
        if let Some(sheet) = table["sheet"].as_str() {
            shown
                .entry(sheet.to_owned())
                .or_default()
                .insert(s(&row[2])?.to_owned());
        }
    }
    let mut result = Vec::with_capacity(tables.len());
    let mut covered = shown.clone();
    for table in tables {
        let mut trimmed: Map<String, Value> = obj(table)?
            .iter()
            .filter(|(key, _)| *key != "merges" && *key != "structure")
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        if let Some(structure) = table.get("structure") {
            trimmed.insert(
                "structure".into(),
                scanned_structure(structure, &shown, &mut covered)?,
            );
        }
        result.push(trimmed);
    }
    for (table, trimmed) in tables.iter().zip(&mut result) {
        let Some(merges) = table.get("merges") else {
            continue;
        };
        // The covered cells by row, so that each merge looks up its own rows
        // rather than going through every covered cell.
        let cells = table["sheet"]
            .as_str()
            .and_then(|sheet| covered.get(sheet))
            .into_iter()
            .flatten()
            .map(|address| crate::excel::coordinate(address).map(|(column, row)| (row, column)))
            .collect::<Result<BTreeSet<_>>>()?;
        let mut kept = Vec::new();
        for merge in arr(merges)? {
            let (from, to) = s(merge)?.split_once(':').context("merge range required")?;
            let (from, to) = (
                crate::excel::coordinate(from)?,
                crate::excel::coordinate(to)?,
            );
            if from.0 <= to.0
                && (from.1..=to.1)
                    .any(|row| cells.range((row, from.0)..=(row, to.0)).next().is_some())
            {
                kept.push(merge.clone());
            }
        }
        trimmed.insert("merges".into(), Value::Array(kept));
    }
    Ok(result.into_iter().map(Value::Object).collect())
}

/// `structure` reduced to the cells at `shown`, the headers they name and the
/// context cells of the tables they are nested in; the kept cells are added to `covered`.
fn scanned_structure(
    structure: &Value,
    shown: &SheetCells,
    covered: &mut SheetCells,
) -> Result<Value> {
    let elements = arr(&structure["elements"])?;
    let mut headers: BTreeMap<&str, &Value> = BTreeMap::new();
    // The cells a nested table's context names in its parent, by cell.
    let mut contexts: BTreeMap<&str, &Value> = BTreeMap::new();
    let mut kept: BTreeSet<&str> = BTreeSet::new();
    for element in elements {
        let cells = shown.get(s(&element["sheet"])?);
        for cell in arr(&element["cells"])? {
            let id = s(&cell["id"])?;
            headers.insert(id, &cell["headers"]);
            if let Some(context) = element.get("parent").map(|p| &p["context"]) {
                contexts.insert(id, context);
            }
            let address = s(&cell["address"])?;
            if cells.is_some_and(|cells| cells.contains(address)) {
                kept.insert(id);
            }
        }
    }
    let mut pending: Vec<&str> = kept.iter().copied().collect();
    while let Some(id) = pending.pop() {
        for header in [headers.get(id), contexts.get(id)]
            .into_iter()
            .flatten()
            .filter_map(|h| h.as_array())
            .flatten()
        {
            let header = s(header)?;
            if headers.contains_key(header) && kept.insert(header) {
                pending.push(header);
            }
        }
    }
    let mut shown_elements = Vec::new();
    for element in elements {
        let cells = arr(&element["cells"])?;
        let cells: Vec<&Value> = cells
            .iter()
            .filter(|cell| cell["id"].as_str().is_some_and(|id| kept.contains(id)))
            .collect();
        if cells.is_empty() {
            continue;
        }
        let sheet = covered.entry(s(&element["sheet"])?.to_owned()).or_default();
        for cell in &cells {
            sheet.insert(s(&cell["address"])?.to_owned());
        }
        let mut shown_element: Map<String, Value> = obj(element)?
            .iter()
            .filter(|(key, _)| *key != "cells")
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        let omitted = arr(&element["cells"])?.len() - cells.len();
        shown_element.insert("cells".into(), json!(cells));
        if omitted > 0 {
            shown_element.insert("omitted_cells".into(), json!(omitted));
        }
        shown_elements.push(Value::Object(shown_element));
    }
    let retained: BTreeSet<String> = shown_elements
        .iter()
        .filter_map(|e| e["id"].as_str().map(str::to_owned))
        .collect();
    for element in &mut shown_elements {
        if let Some(descriptions) = element["descriptions"].as_array_mut() {
            descriptions.retain(|id| id.as_str().is_some_and(|id| retained.contains(id)));
        }
        // A context naming no shown cell leaves its parent out of the scope.
        if element["parent"]["element"]
            .as_str()
            .is_some_and(|parent| !retained.contains(parent))
        {
            element.as_object_mut().unwrap().remove("parent");
        }
    }
    let mut result: Map<String, Value> = obj(structure)?
        .iter()
        .filter(|(key, _)| *key != "elements")
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    result.insert("elements".into(), Value::Array(shown_elements));
    Ok(Value::Object(result))
}
