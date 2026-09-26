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
