//! Output-only TOON using explicit nesting, comma delimiters and two-space indent.
//! No key folding, path expansion, input parser or storage serialization. The
//! syntax follows toon-format/spec (objects, list/primitive/tabular arrays and
//! quoted strings, including the control-character escapes in spec 4.1).
use serde_json::Value;

pub(super) fn encode(value: &Value) -> String {
    let mut lines = Vec::new();
    emit(value, None, 0, &mut lines);
    lines.join("\n")
}

fn quote(text: &str) -> String {
    // TOON uses Unicode escapes for controls other than newline, CR and tab.
    use std::fmt::Write;
    let mut out = String::from("\"");
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if ch <= '\u{001f}' => {
                write!(out, "\\u{:04x}", ch as u32).unwrap();
            }
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}

fn key(text: &str) -> String {
    let mut chars = text.chars();
    if chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
    {
        text.to_owned()
    } else {
        quote(text)
    }
}

fn scalar(value: &Value) -> String {
    match value {
        Value::String(text) => {
            let reserved = text.is_empty()
                || text.trim() != text
                || matches!(text.as_str(), "true" | "false" | "null")
                || text.starts_with(|c: char| {
                    c.is_ascii_digit() || matches!(c, '-' | '+' | '.' | '#')
                })
                || text
                    .chars()
                    .any(|c| c.is_control() || ",:[]{}\"\\".contains(c));
            if reserved { quote(text) } else { text.clone() }
        }
        Value::Number(n) if n.is_f64() => {
            let value = n.as_f64().unwrap();
            if value == 0.0 {
                "0".to_owned()
            } else {
                value.to_string()
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => value.to_string(),
        _ => unreachable!("containers have dedicated TOON forms"),
    }
}

fn primitive(value: &Value) -> bool {
    !value.is_object() && !value.is_array()
}

fn line(lines: &mut Vec<String>, depth: usize, text: String) {
    lines.push(format!("{}{text}", "  ".repeat(depth)));
}

fn emit(value: &Value, name: Option<&str>, depth: usize, lines: &mut Vec<String>) {
    match value {
        Value::Object(object) => {
            if let Some(name) = name {
                line(lines, depth, format!("{}:", key(name)));
            }
            for (k, v) in object {
                emit(v, Some(k), depth + usize::from(name.is_some()), lines);
            }
        }
        Value::Array(items) => {
            let prefix = format!("{}[{}]", name.map(key).unwrap_or_default(), items.len());
            if items.iter().all(primitive) {
                let values = items.iter().map(scalar).collect::<Vec<_>>().join(",");
                let suffix = if values.is_empty() {
                    String::new()
                } else {
                    format!(" {values}")
                };
                line(lines, depth, format!("{prefix}:{suffix}"));
                return;
            }
            let columns = items.first().and_then(Value::as_object).filter(|first| {
                (name.is_some() || depth == 0)
                    && !first.is_empty()
                    && items.iter().all(|item| {
                        item.as_object()
                            .is_some_and(|o| o.keys().eq(first.keys()) && o.values().all(primitive))
                    })
            });
            if let Some(columns) = columns {
                line(
                    lines,
                    depth,
                    format!(
                        "{prefix}{{{}}}:",
                        columns.keys().map(|k| key(k)).collect::<Vec<_>>().join(",")
                    ),
                );
                for item in items {
                    line(
                        lines,
                        depth + 1,
                        columns
                            .keys()
                            .map(|k| scalar(&item[k]))
                            .collect::<Vec<_>>()
                            .join(","),
                    );
                }
            } else {
                line(lines, depth, format!("{prefix}:"));
                for item in items {
                    list_item(item, depth + 1, lines);
                }
            }
        }
        _ => line(
            lines,
            depth,
            match name {
                Some(name) => format!("{}: {}", key(name), scalar(value)),
                None => scalar(value),
            },
        ),
    }
}

fn list_item(value: &Value, depth: usize, lines: &mut Vec<String>) {
    if let Value::Object(object) = value {
        let mut fields = object.iter();
        if let Some((name, value)) = fields.next() {
            // The first field follows the hyphen. Its children are two levels
            // deeper than the hyphen; sibling fields are only one level deeper.
            let start = lines.len();
            emit(value, Some(name), depth + 1, lines);
            let body = lines[start].trim_start().to_owned();
            lines[start] = format!("{}- {body}", "  ".repeat(depth));
            for (name, value) in fields {
                emit(value, Some(name), depth + 1, lines);
            }
        } else {
            line(lines, depth, "-".to_owned());
        }
    } else {
        let start = lines.len();
        emit(value, None, depth, lines);
        let body = lines[start].trim_start().to_owned();
        lines[start] = format!("{}- {body}", "  ".repeat(depth));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn heterogeneous_lists_preserve_missing_null_and_nested_first_fields() {
        assert_eq!(
            encode(&json!([
                {"a":[{"id":"s1"},{"id":"s2"}],"b":null},
                {"a":{"z":"原文"}}, {}, [[], [1, 2]], "001"
            ])),
            "[5]:\n  - a[2]{id}:\n      s1\n      s2\n    b: null\n  - a:\n      z: 原文\n  -\n  - [2]:\n    - [0]:\n    - [2]: 1,2\n  - \"001\""
        );
    }

    #[test]
    fn literal_backslashes_and_controls_are_distinct() {
        assert_eq!(scalar(&json!("\\b\\f")), "\"\\\\b\\\\f\"");
        assert_eq!(scalar(&json!("\u{0008}\u{000c}")), "\"\\u0008\\u000c\"");
    }

    #[test]
    fn nested_arrays_of_objects_use_lists_not_keyless_tables() {
        // A keyless table header is only valid at the document root. Checked
        // against the reference decoder @toon-format/toon 4.1.1.
        assert_eq!(
            encode(&json!([[{}, {}], [{"a":1}, {"a":2}]])),
            "[2]:\n  - [2]:\n    -\n    -\n  - [2]:\n    - a: 1\n    - a: 2"
        );
    }
}
