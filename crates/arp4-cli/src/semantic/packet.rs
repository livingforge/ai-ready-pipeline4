use super::*;

/// Physical offsets are provenance, not semantic dependencies. All text, ordering,
/// headings, cell coordinates, formats, merges and warnings remain significant.
pub fn packet_fingerprint(packet: &Value) -> String {
    fn position(value: &mut Value) {
        if let Some(object) = value.as_object_mut() {
            for key in ["line_start", "line_end", "byte_start", "byte_end"] {
                object.remove(key);
            }
        }
    }
    let mut semantic = packet.clone();
    semantic.as_object_mut().unwrap().remove("packet");
    for key in ["sources", "context_sources"] {
        let column = semantic[key]["columns"]
            .as_array()
            .and_then(|columns| columns.iter().position(|v| v == "position"));
        if let Some(column) = column
            && let Some(rows) = semantic[key]["rows"].as_array_mut()
        {
            for row in rows {
                // TXT cell coordinates encode physical line numbers. The ordered
                // source aliases already retain the logical order of nonblank text.
                if row[column]["kind"] == "line" {
                    row[2] = Value::Null;
                }
                position(&mut row[column]);
            }
        }
    }
    if let Some(sources) = semantic["external_sources"].as_array_mut() {
        for source in sources {
            if source["context"]["position"]["kind"] == "line" {
                source["context"].as_object_mut().unwrap().remove("row");
                source["context"].as_object_mut().unwrap().remove("cell");
                source.as_object_mut().unwrap().remove("location");
            }
            position(&mut source["context"]["position"]);
        }
    }
    hash(&encoded(&semantic))
}

/// Lossless transport only: fingerprints and validation use the logical packet.
pub fn compact_review(packet: &Value) -> Value {
    fn encode(
        value: &Value,
        spans: &mut Vec<Value>,
        indices: &mut BTreeMap<String, usize>,
    ) -> Value {
        match value {
            Value::Object(object)
                if object.len() == 4
                    && ["source", "start", "end", "quote"]
                        .iter()
                        .all(|k| object.contains_key(*k)) =>
            {
                let key = value.to_string();
                let next = spans.len();
                let index = *indices.entry(key).or_insert_with(|| {
                    spans.push(json!([
                        value["source"],
                        value["start"],
                        value["end"],
                        value["quote"]
                    ]));
                    next
                });
                json!({"span_ref":index})
            }
            Value::Object(object) => Value::Object(
                object
                    .iter()
                    .map(|(k, v)| (k.clone(), encode(v, spans, indices)))
                    .collect(),
            ),
            Value::Array(array) => {
                Value::Array(array.iter().map(|v| encode(v, spans, indices)).collect())
            }
            _ => value.clone(),
        }
    }
    let mut spans = Vec::new();
    let mut compact = encode(packet, &mut spans, &mut BTreeMap::new());
    if let Some(items) = compact["items"].as_array() {
        let columns: BTreeSet<String> = items
            .iter()
            .filter_map(Value::as_object)
            .flat_map(|o| o.keys().cloned())
            .collect();
        // Retain missing-vs-null distinctions; fall back if field sets differ.
        if items.iter().all(|i| {
            i.as_object()
                .is_some_and(|o| o.keys().cloned().collect::<BTreeSet<_>>() == columns)
        }) {
            let rows: Vec<Value> = items
                .iter()
                .map(|i| Value::Array(columns.iter().map(|c| i[c].clone()).collect()))
                .collect();
            compact["items"] = json!({"columns":columns,"rows":rows});
        }
    }
    compact["span_table"] = json!({"columns":["source","start","end","quote"],"rows":spans});
    compact["encoding"] = json!("review-tables-v1");
    compact
}

pub(crate) fn sources<'a>(input: &'a Input, document: Option<&str>) -> Result<Vec<&'a Source>> {
    let mut rows: Vec<_> = input
        .sources
        .iter()
        .filter(|s| document.is_none_or(|d| s.document == d))
        .collect();
    ensure!(!rows.is_empty(), "unknown document or empty input");
    rows.sort_by_key(|s| {
        (
            &s.document,
            s.context.as_ref().map(|c| (&c.sheet, c.row, c.column)),
            &s.location,
        )
    });
    Ok(rows)
}

pub(super) fn source_rows(input: &Input, rows: &[&Source]) -> Value {
    let mut tables = Vec::<Value>::new();
    let mut table_indices = BTreeMap::new();
    let has_positions = rows
        .iter()
        .any(|s| s.context.as_ref().is_some_and(|c| c.position.is_some()));
    let data = rows
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let (table, position, field, format) = if let Some(c) = &s.context {
                let index = *table_indices
                    .entry((&s.document, &c.sheet))
                    .or_insert_with(|| {
                        let mut table =
                            json!({"document":s.document,"sheet":c.sheet,"merges":c.merges});
                        if let Some(structure) = input.structures.get(&s.document) {
                            table["structure"] =
                                crate::document_structure::sheet_context(structure, &c.sheet);
                        }
                        tables.push(table);
                        tables.len() - 1
                    });
                (
                    json!(index),
                    json!(c.cell),
                    json!(s.location.split('/').skip(5).collect::<Vec<_>>().join("/")),
                    json!(c.number_format),
                )
            } else {
                let mut table = json!({"document":s.document});
                if let Some(structure) = input.structures.get(&s.document) {
                    table["structure"] = structure.clone();
                }
                let index = tables.iter().position(|t| *t == table).unwrap_or_else(|| {
                    tables.push(table);
                    tables.len() - 1
                });
                (json!(index), json!(s.location), json!("text"), Value::Null)
            };
            let mut row = vec![
                json!(format!("s{}", i + 1)),
                table,
                position,
                field,
                json!(s.text),
                format,
            ];
            if has_positions {
                row.push(json!(s.context.as_ref().and_then(|c| c.position.as_ref())));
            }
            json!(row)
        })
        .collect::<Vec<_>>();
    let mut columns = vec![
        "ref",
        "table",
        "cell_or_location",
        "field",
        "text",
        "number_format",
    ];
    if has_positions {
        columns.push("position");
    }
    json!({"columns":columns,"tables":tables,"rows":data})
}

/// The fingerprint includes the protocol and source context, not unrelated document revisions.
pub(super) fn relevant_warnings<'a>(
    input: &'a Input,
    documents: &BTreeSet<String>,
) -> Vec<&'a String> {
    input
        .warnings
        .iter()
        .filter(|warning| {
            input
                .revisions
                .keys()
                .find(|doc| warning.starts_with(&format!("{doc}: ")))
                .is_none_or(|doc| documents.contains(doc))
        })
        .collect()
}

pub fn packet(input: &Input, document: &str) -> Result<Value> {
    let rows = sources(input, Some(document))?;
    let mut result = json!({"version":1,"document":document,"sources":source_rows(input,&rows),
        "warnings":relevant_warnings(input, &[document.to_owned()].into()),"contract":hash(PROMPT.as_bytes())});
    result["packet"] = json!(packet_fingerprint(&result));
    Ok(result)
}

pub fn changes(before: &Input, after: &Input) -> Result<Value> {
    let old: BTreeSet<_> = before.sources.iter().map(|s| s.document.as_str()).collect();
    let new: BTreeSet<_> = after.sources.iter().map(|s| s.document.as_str()).collect();
    let mut reextract = Vec::new();
    let mut reusable = Vec::new();
    for doc in &new {
        if old.contains(doc) && packet(before, doc)?["packet"] == packet(after, doc)?["packet"] {
            reusable.push(*doc);
        } else {
            reextract.push(*doc);
        }
    }
    let removed: Vec<_> = old.difference(&new).copied().collect();
    Ok(
        json!({"reextract":reextract,"reusable":reusable,"removed":removed,
        "global_review_required":!reextract.is_empty() || !removed.is_empty(),
        "identity_policy":"Reuse replies only, never infer persistent identity. Updates require explicit retain/replace/retire via assign-ids or registry apply."}),
    )
}

/// A string means the entire source. An object selects an exact quote; ambiguous matches fail.
pub(crate) fn span(value: &Value, rows: &[&Source]) -> Result<Value> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Selection {
        source: String,
        quote: String,
        occurrence: Option<usize>,
    }
    let (alias, quote, occurrence) = if let Some(alias) = value.as_str() {
        (alias.to_owned(), None, None)
    } else {
        let s: Selection = serde_json::from_value(value.clone())?;
        (s.source, Some(s.quote), s.occurrence)
    };
    let index: usize = alias
        .strip_prefix('s')
        .context("source ref must be s<number>")?
        .parse()?;
    ensure!(
        alias == format!("s{index}") && index > 0,
        "invalid source ref"
    );
    let source = rows.get(index - 1).context("unknown source ref")?;
    let quote = quote.unwrap_or_else(|| source.text.clone());
    ensure!(!quote.is_empty(), "empty quote");
    // char_indices includes overlapping occurrences and gives Unicode scalar offsets below.
    let matches: Vec<_> = source
        .text
        .char_indices()
        .filter(|(i, _)| source.text[*i..].starts_with(&quote))
        .map(|(i, _)| i)
        .collect();
    let start = if let Some(n) = occurrence {
        ensure!(n > 0, "occurrence is one-based");
        *matches.get(n - 1).context("quote occurrence not found")?
    } else {
        ensure!(
            matches.len() == 1,
            "quote missing or ambiguous; specify occurrence"
        );
        matches[0]
    };
    let start = source.text[..start].chars().count();
    Ok(json!({"source":source.id,"start":start,"end":start+quote.chars().count(),"quote":quote}))
}
