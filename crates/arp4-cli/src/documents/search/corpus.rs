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

struct Corpus<'a> {
    inspected: &'a Inspection,
    ready: bool,
    passages: Vec<Value>,
}

impl Corpus<'_> {
    fn add(&mut self, kind: &str, title: &str, sources: Vec<Value>, context: Vec<Value>) {
        for sources in fragments(sources) {
            let body = sources
                .iter()
                .map(|s| s["text"].as_str().unwrap())
                .collect::<Vec<_>>()
                .join(" | ");
            if body.trim().is_empty() {
                continue;
            }
            let passage = json!({"document_id":self.inspected.meta["document_id"],
                "source_path":self.inspected.meta["source"]["path"],
                "extraction_sha256":self.inspected.meta["extraction"],
                "kind":kind,"title":title,"text":body,"sources":sources,"context":context,
                "structure_reviewed":self.ready});
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
        )?
    } else {
        (json!({"elements":[]}), json!({"ready":false}))
    };
    let mut corpus = Corpus {
        inspected,
        ready: report["ready"] == true,
        passages: vec![],
    };
    let mut by_sheet: BTreeMap<String, BTreeMap<String, Value>> = BTreeMap::new();
    for (si, sheet) in array(&inspected.extraction["sheets"])?.iter().enumerate() {
        let name = string(&sheet["name"])?;
        for (ci, cell) in array(&sheet["cells"])?.iter().enumerate() {
            let key = if cell["type"] == "formula" {
                "formula"
            } else {
                "value"
            };
            by_sheet.entry(name.into()).or_default().insert(
                string(&cell["address"])?.into(),
                source(
                    format!("/sheets/{si}/cells/{ci}/{key}"),
                    name,
                    cell,
                    &cell[key],
                ),
            );
        }
    }
    let mut used = BTreeSet::new();
    // All linked headers are explicit cells in the interpretation, including merged
    // and multi-level headers; no first-row-is-a-header heuristic is added here.
    for element in array(&structure["elements"])? {
        if element["kind"] != "table" {
            continue;
        }
        let name = string(&element["sheet"])?;
        let Some(cells) = by_sheet.get(name) else {
            continue;
        };
        let interpreted = array(&element["cells"])?;
        let by_id: BTreeMap<_, _> = interpreted
            .iter()
            .filter_map(|c| Some((c["id"].as_str()?, c)))
            .collect();
        let mut rows: BTreeMap<u32, Vec<Value>> = BTreeMap::new();
        let mut headers: BTreeMap<u32, BTreeSet<String>> = BTreeMap::new();
        for cell in interpreted {
            let address = string(&cell["address"])?;
            let Some(original) = cells.get(address) else {
                continue;
            };
            let (_, row) = excel::coordinate(address)?;
            rows.entry(row).or_default().push(original.clone());
            used.insert((name.to_owned(), address.to_owned()));
            let mut pending: Vec<&str> = array(&cell["headers"])?
                .iter()
                .filter_map(Value::as_str)
                .collect();
            let mut seen = BTreeSet::new();
            while let Some(id) = pending.pop() {
                if !seen.insert(id) {
                    continue;
                }
                if let Some(header) = by_id.get(id) {
                    headers
                        .entry(row)
                        .or_default()
                        .insert(string(&header["address"])?.into());
                    pending.extend(array(&header["headers"])?.iter().filter_map(Value::as_str));
                }
            }
        }
        for (row, mut sources) in rows {
            sources.sort_by_key(|s| excel::coordinate(s["address"].as_str().unwrap()).unwrap().0);
            let context = headers
                .get(&row)
                .into_iter()
                .flatten()
                .filter_map(|a| cells.get(a))
                .filter(|h| !sources.iter().any(|s| s["pointer"] == h["pointer"]))
                .cloned()
                .collect();
            corpus.add(
                "row",
                &format!(
                    "{} / {name}",
                    inspected.meta["document_id"].as_str().unwrap()
                ),
                sources,
                context,
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
            corpus.add(kind, &title, sources, vec![]);
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
                    vec![],
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
                vec![],
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
