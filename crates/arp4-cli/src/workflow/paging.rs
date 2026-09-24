//! Bounded agent reading pages. Logical values and pointers remain format independent.
use crate::agent_format::AgentFormat;
use anyhow::{Result, ensure};
use serde::Serialize;
use serde_json::{Value, json};
use std::io::{self, Write};

pub(super) const READ_PAGE_BYTES: usize = 48000;

#[derive(Serialize)]
#[serde(untagged)]
enum Content<'a> {
    Json(&'a Value),
    Rows(&'a [Value]),
    Text(&'a str),
}

#[derive(Serialize)]
struct Fragment<'a> {
    pointer: &'a str,
    value: Content<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    string_offset: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    string_total_bytes: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    array_offset: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    array_total: Option<usize>,
}

#[derive(Default)]
struct ByteCount(usize);
impl Write for ByteCount {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0 += bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn encoded_len(value: &impl Serialize) -> Result<usize> {
    let mut count = ByteCount::default();
    serde_json::to_writer(&mut count, value)?;
    // Match the store's newline-terminated JSON encoding.
    Ok(count.0 + 1)
}

// Reserve indentation for a fragment nested below entries in a TOON page. Keep
// the JSON bound too: callers of the library still receive logical JSON values.
fn delivery_len(value: &impl Serialize, format: AgentFormat) -> Result<usize> {
    let json = encoded_len(value)?;
    if format == AgentFormat::Json {
        return Ok(json);
    }
    let text = format.encode(value)?;
    Ok(json.max(text.len() + 6 * (text.lines().count() + 1) + 32))
}

fn pointer_child(parent: &str, key: &str) -> String {
    format!("{parent}/{}", key.replace('~', "~0").replace('/', "~1"))
}

fn visit(
    pointer: &str,
    value: &Value,
    fragment_bytes: usize,
    format: AgentFormat,
    emit: &mut impl FnMut(&Fragment<'_>) -> Result<()>,
) -> Result<()> {
    let entry = Fragment {
        pointer,
        value: Content::Json(value),
        string_offset: None,
        string_total_bytes: None,
        array_offset: None,
        array_total: None,
    };
    if delivery_len(&entry, format)? <= fragment_bytes {
        return emit(&entry);
    }
    match value {
        Value::Object(values) if !values.is_empty() => {
            for (key, value) in values {
                visit(
                    &pointer_child(pointer, key),
                    value,
                    fragment_bytes,
                    format,
                    emit,
                )?;
            }
        }
        Value::Array(values) if !values.is_empty() => {
            let mut start = 0;
            while start < values.len() {
                let mut chunk = Fragment {
                    pointer,
                    value: Content::Rows(&[]),
                    string_offset: None,
                    string_total_bytes: None,
                    array_offset: Some(start),
                    array_total: Some(values.len()),
                };
                let mut bytes = delivery_len(&chunk, format)?;
                let mut end = start;
                while end < values.len() {
                    let size = delivery_len(&values[end], format)?;
                    if bytes + size > fragment_bytes {
                        break;
                    }
                    bytes += size;
                    end += 1;
                }
                if end == start {
                    visit(
                        &pointer_child(pointer, &start.to_string()),
                        &values[start],
                        fragment_bytes,
                        format,
                        emit,
                    )?;
                    start += 1;
                } else {
                    chunk.value = Content::Rows(&values[start..end]);
                    emit(&chunk)?;
                    start = end;
                }
            }
        }
        Value::String(text) if !text.is_empty() => {
            let mut start = 0;
            while start < text.len() {
                let mut entry = Fragment {
                    pointer,
                    value: Content::Text(""),
                    string_offset: Some(start),
                    string_total_bytes: Some(text.len()),
                    array_offset: None,
                    array_total: None,
                };
                let mut bytes = delivery_len(&entry, format)?;
                let mut end = start;
                for ch in text[start..].chars() {
                    let size = match ch {
                        '"' | '\\' | '\n' | '\r' | '\t' | '\u{08}' | '\u{0c}' => 2,
                        ch if ch <= '\u{1f}' => 6,
                        ch => ch.len_utf8(),
                    };
                    if bytes + size > fragment_bytes {
                        break;
                    }
                    bytes += size;
                    end += ch.len_utf8();
                }
                ensure!(end > start, "JSON pointer is too large for a read fragment");
                entry.value = Content::Text(&text[start..end]);
                emit(&entry)?;
                start = end;
            }
        }
        _ => anyhow::bail!("JSON pointer is too large for a read fragment"),
    }
    Ok(())
}

pub(super) struct ReadOptions<'a> {
    pub format: AgentFormat,
    pub pointer: &'a str,
    pub offset: usize,
    pub limit: usize,
    pub max_bytes: usize,
    pub revision: Option<&'a str>,
}

// These are subcommands: keep the caller's executable, root, run-id and cwd.
// A slash-free pointer avoids Git Bash's native path conversion.
fn next_command(
    task_ref: &str,
    pointer: &str,
    offset: usize,
    limit: usize,
    max_bytes: usize,
    revision: &str,
) -> Value {
    let command = format!(
        "read --task {task_ref} --offset {offset} --limit {limit} --max-bytes {max_bytes} --revision {revision}"
    );
    if pointer.is_empty() {
        return json!({"bash":command,"powershell":command});
    }
    let pointer = if pointer == "/" {
        pointer
    } else {
        pointer.strip_prefix('/').unwrap_or(pointer)
    };
    json!({
        "bash":format!("{command} --pointer '{}'", pointer.replace('\'', "'\"'\"'")),
        "powershell":format!("{command} --pointer '{}'", pointer.replace('\'', "''"))
    })
}

pub(super) fn page(
    task_ref: &str,
    data: &Value,
    sections: &[&str],
    options: ReadOptions<'_>,
) -> Result<Value> {
    let ReadOptions {
        format,
        pointer,
        offset,
        limit,
        max_bytes,
        revision: expected_revision,
    } = options;
    ensure!(limit > 0, "positive fragment limit required");
    ensure!(
        (4096..=READ_PAGE_BYTES).contains(&max_bytes),
        "invalid read byte limit"
    );
    let fragment_bytes = (max_bytes - 2048) / 2;
    let revision = crate::data::hash(&serde_json::to_vec(&json!([
        data, pointer, max_bytes, format
    ]))?);
    ensure!(
        expected_revision.is_none_or(|expected| expected == revision),
        "read content, format or byte limit changed; restart at offset 0 without --revision"
    );
    let value = data
        .pointer(pointer)
        .ok_or_else(|| anyhow::anyhow!("unknown JSON pointer"))?;
    let mut result = json!({"task_ref":task_ref,"format":format,"pointer":pointer,"entries":[],
        "page":{"offset":offset,"total":0,"unit":"fragments","next_offset":null,
        "complete":false,
        "next_command":next_command(task_ref, pointer, usize::MAX, limit, max_bytes, &revision),
        "revision":revision,"max_bytes":max_bytes,"content_bytes":format.encode(value)?.len()+1}});
    // Reserve digits for total/next_offset and the CLI response envelope.
    let mut bytes = delivery_len(&result, format)? + 96;
    let mut total = 0;
    let mut entries = Vec::new();
    let mut full = false;
    let mut emit = |entry: &Fragment<'_>| -> Result<()> {
        let index = total;
        total += 1;
        if index < offset || full {
            return Ok(());
        }
        let size = delivery_len(entry, format)?;
        if entries.len() >= limit || bytes + size > max_bytes {
            full = true;
            return Ok(());
        }
        bytes += size;
        entries.push(serde_json::to_value(entry)?);
        Ok(())
    };
    if pointer.is_empty() {
        for key in sections {
            visit(
                &pointer_child("", key),
                &data[key],
                fragment_bytes,
                format,
                &mut emit,
            )?;
        }
    } else if let Value::Array(values) = value {
        for (index, value) in values.iter().enumerate() {
            visit(
                &pointer_child(pointer, &index.to_string()),
                value,
                fragment_bytes,
                format,
                &mut emit,
            )?;
        }
    } else {
        visit(pointer, value, fragment_bytes, format, &mut emit)?;
    }
    ensure!(offset <= total, "offset exceeds fragment count");
    let end = offset + entries.len();
    ensure!(
        end > offset || offset == total,
        "read page cannot fit a fragment"
    );
    if !pointer.is_empty()
        && !value.is_array()
        && total == 1
        && offset == 0
        && entries[0]["pointer"] == pointer
        && entries[0].get("string_offset").is_none()
    {
        result.as_object_mut().unwrap().remove("entries");
        result["value"] = entries.remove(0)["value"].take();
    } else {
        result["entries"] = json!(entries);
    }
    result["page"]["total"] = json!(total);
    result["page"]["next_offset"] = json!((end < total).then_some(end));
    result["page"]["complete"] = json!(end == total);
    result["page"]["next_command"] = if end < total {
        next_command(task_ref, pointer, end, limit, max_bytes, &revision)
    } else {
        Value::Null
    };
    let mut wire = result.clone();
    wire["ok"] = json!(true);
    ensure!(
        format.encode(&wire)?.len() < max_bytes,
        "read page exceeds delivery byte limit"
    );
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toon_pages_preserve_long_text_and_reject_format_changes() {
        let text = "原文\r\n\t引用,例外\\b\u{0000}".repeat(1800);
        let data = json!({"packet":{"quote":text}});
        let mut offset = 0;
        let mut revision = None;
        let mut restored = String::new();
        loop {
            let mut result = page(
                "12345678",
                &data,
                &["packet"],
                ReadOptions {
                    format: AgentFormat::Toon,
                    pointer: "",
                    offset,
                    limit: 10000,
                    max_bytes: 4096,
                    revision: revision.as_deref(),
                },
            )
            .unwrap();
            result["ok"] = json!(true);
            assert!(AgentFormat::Toon.encode(&result).unwrap().len() < 4096);
            for entry in result["entries"].as_array().unwrap() {
                assert_eq!(entry["string_offset"], restored.len());
                restored.push_str(entry["value"].as_str().unwrap());
            }
            revision = Some(result["page"]["revision"].as_str().unwrap().to_owned());
            match result["page"]["next_offset"].as_u64() {
                Some(next) => {
                    assert!(next as usize > offset);
                    offset = next as usize;
                }
                None => break,
            }
        }
        assert_eq!(restored, text);
        assert!(
            page(
                "12345678",
                &data,
                &["packet"],
                ReadOptions {
                    format: AgentFormat::Json,
                    pointer: "",
                    offset: 0,
                    limit: 10000,
                    max_bytes: 4096,
                    revision: revision.as_deref(),
                }
            )
            .unwrap_err()
            .to_string()
            .contains("format")
        );
    }

    #[test]
    fn targeted_continuation_quotes_pointer_and_preserves_options() {
        let pointer = "/packet/日本語 'quoted' $x; `echo`";
        let data = json!({"packet":{"日本語 'quoted' $x; `echo`":"条件と例外".repeat(3000)}});
        let result = page(
            "12345678",
            &data,
            &["packet"],
            ReadOptions {
                format: AgentFormat::Json,
                pointer,
                offset: 0,
                limit: 1,
                max_bytes: 4096,
                revision: None,
            },
        )
        .unwrap();
        assert_eq!(result["page"]["complete"], false);
        let revision = result["page"]["revision"].as_str().unwrap();
        let prefix = format!(
            "read --task 12345678 --offset 1 --limit 1 --max-bytes 4096 --revision {revision}"
        );
        assert_eq!(
            result["page"]["next_command"]["bash"],
            format!("{prefix} --pointer 'packet/日本語 '\"'\"'quoted'\"'\"' $x; `echo`'")
        );
        assert_eq!(
            result["page"]["next_command"]["powershell"],
            format!("{prefix} --pointer 'packet/日本語 ''quoted'' $x; `echo`'")
        );
        assert!(encoded_len(&result).unwrap() + 11 <= 4096);
    }

    #[test]
    fn table_chunks_preserve_rows_and_reduce_transport_overhead() {
        let rows: Vec<_> = (0..1102)
            .map(|i| {
                json!([
                    format!("s{i}"),
                    0,
                    format!("A{i}"),
                    "value",
                    "原文の条件と例外",
                    "General"
                ])
            })
            .collect();
        let data = json!({"packet":{"sources":{"rows":rows}}});
        let separate_bytes: usize = rows
            .iter()
            .enumerate()
            .map(|(i, row)| {
                encoded_len(&json!({"pointer":format!("/packet/sources/rows/{i}"),"value":row}))
                    .unwrap()
            })
            .sum();
        for (format, max_bytes) in
            [AgentFormat::Toon, AgentFormat::Json]
                .into_iter()
                .flat_map(|format| {
                    [4096, 8192, 12000, 24000, 36000, 48000].map(|budget| (format, budget))
                })
        {
            let mut offset = 0;
            let mut restored = Vec::new();
            let mut pages = 0;
            let mut wire_bytes = 0;
            let mut fragments = 0;
            let mut revision = None;
            loop {
                let result = page(
                    "task",
                    &data,
                    &["packet"],
                    ReadOptions {
                        format,
                        pointer: "",
                        offset,
                        limit: 10000,
                        max_bytes,
                        revision: revision.as_deref(),
                    },
                )
                .unwrap();
                // Include the public CLI envelope and newline in the budget check.
                let mut wire = result.clone();
                wire["ok"] = json!(true);
                let size = format.encode(&wire).unwrap().len() + 1;
                assert!(size <= max_bytes);
                wire_bytes += size;
                pages += 1;
                revision = Some(result["page"]["revision"].as_str().unwrap().to_owned());
                for entry in result["entries"].as_array().unwrap() {
                    assert_eq!(entry["pointer"], "/packet/sources/rows");
                    assert_eq!(entry["array_offset"], restored.len());
                    assert_eq!(entry["array_total"], rows.len());
                    restored.extend(entry["value"].as_array().unwrap().iter().cloned());
                    fragments += 1;
                }
                match result["page"]["next_offset"].as_u64() {
                    Some(next) => offset = next as usize,
                    None => break,
                }
            }
            assert_eq!(restored, rows);
            assert!(fragments < rows.len() / 5);
            assert!(wire_bytes < separate_bytes);
            println!(
                "budget={max_bytes} rows={} fragments={fragments} pages={pages} wire_bytes={wire_bytes} separate_fragment_bytes={separate_bytes}",
                rows.len()
            );
        }
    }
}
