//! Agent-facing presentation only. Persisted evidence never passes through here.
use serde_json::{Value, json};

pub const MAX_RESPONSE_BYTES: usize = 16 * 1024;
const ITEM_BYTES: usize = 2048;
const PAGE_BYTES: usize = 12 * 1024;

#[derive(clap::Args, Default)]
pub struct Output {
    /// Return all details (not supported by spec workflow; use read pagination there).
    #[arg(long, global = true, conflicts_with_all = ["limit", "offset"])]
    pub full: bool,
    /// Maximum items per page (default: 20).
    #[arg(long, global = true, value_parser = clap::value_parser!(u32).range(1..=100))]
    pub limit: Option<u32>,
    /// Zero-based item offset (default: 0); use page.next_offset for the next page.
    #[arg(long, global = true)]
    pub offset: Option<usize>,
}

fn bytes(value: &Value) -> usize {
    serde_json::to_vec(value).expect("JSON value").len()
}

// Never shorten a value in place: missing data must be visibly distinguishable
// from a genuine short string, empty array, null, or object in the source.
fn bounded(mut value: Value, budget: usize) -> Value {
    if bytes(&value) <= budget {
        return value;
    }
    let original_bytes = bytes(&value);
    if !value.is_object() {
        return json!({"omitted":true,"bytes":original_bytes});
    }
    let mut omitted = Vec::new();
    while bytes(&value) > budget.saturating_sub(512) {
        let largest = value
            .as_object()
            .unwrap()
            .iter()
            .filter(|(k, _)| !["ok", "state", "summary", "page"].contains(&k.as_str()))
            .max_by_key(|(k, v)| k.len() + bytes(v))
            .map(|(k, _)| k.clone());
        let Some(key) = largest else { break };
        value.as_object_mut().unwrap().remove(&key);
        omitted.push(key);
    }
    value["omitted_fields"] = json!(omitted);
    if bytes(&value) > budget {
        return json!({"ok":value.get("ok"),"omitted":true,"bytes":original_bytes});
    }
    value
}

impl Output {
    pub fn page(&self, items: Vec<Value>) -> Value {
        let total = items.len();
        let offset = self.offset.unwrap_or(0).min(total);
        let limit = if self.full {
            total
        } else {
            self.limit.unwrap_or(20) as usize
        };
        let mut selected = Vec::new();
        let mut used = 0;
        for (index, item) in items.into_iter().enumerate().skip(offset).take(limit) {
            let mut item = if self.full {
                item
            } else {
                bounded(item, ITEM_BYTES)
            };
            if item.get("omitted_fields").is_some() || item.get("omitted").is_some() {
                item["index"] = json!(index);
            }
            let size = bytes(&item) + 1;
            if !self.full && used + size > PAGE_BYTES {
                break;
            }
            used += size;
            selected.push(item);
        }
        let end = offset + selected.len();
        json!({"items":selected,"page":{"total":total,"offset":offset,"returned":end-offset,
            "next_offset":if end < total { Some(end) } else { None }}})
    }

    pub fn render(&self, mut value: Value, ok: bool) -> String {
        if !value.is_object() {
            value = json!({"items":value});
        }
        value["ok"] = json!(ok);
        if !self.full {
            value = bounded(value, MAX_RESPONSE_BYTES - 1);
        }
        serde_json::to_string(&value).expect("JSON value")
    }

    pub fn emit(&self, value: Value, ok: bool) {
        println!("{}", self.render(value, ok));
    }
}

pub fn omit_hashes(value: &mut Value, keys: &[&str], include: bool) {
    if !include {
        for key in keys {
            value.as_object_mut().unwrap().remove(*key);
        }
    }
}

/// Structured rejection: `error.code` comes from the payload and `error.diagnostics`
/// stays a JSON array. When the diagnostics do not fit the response budget, whole
/// entries are dropped from the end and `diagnostics_omitted` counts them; a
/// diagnostic is never cut in the middle.
pub fn reject(payload: &Value) {
    println!("{}", render_rejection(payload));
}

pub fn render_rejection(payload: &Value) -> String {
    let mut error = payload.clone();
    let code = payload["code"].as_str().unwrap_or("operation_failed");
    error["code"] = json!(code);
    if let Some(diagnostics) = payload["diagnostics"].as_array() {
        let mut kept = Vec::new();
        let mut base = error.clone();
        base["diagnostics"] = json!([]);
        base["diagnostics_omitted"] = json!(diagnostics.len());
        let mut used = bytes(&json!({"error":base,"ok":false}));
        for diagnostic in diagnostics {
            let size = bytes(diagnostic) + 1;
            if used + size > MAX_RESPONSE_BYTES - 1 {
                break;
            }
            used += size;
            kept.push(diagnostic.clone());
        }
        let omitted = diagnostics.len() - kept.len();
        error["diagnostics"] = json!(kept);
        if omitted > 0 {
            error["diagnostics_omitted"] = json!(omitted);
        }
    }
    json!({"error":error,"ok":false}).to_string()
}

pub fn error(code: &str, message: &str) {
    // Error messages may include a complete source value or a schema dump.
    let end = message
        .char_indices()
        .map(|(i, _)| i)
        .take_while(|i| *i <= 1024)
        .last()
        .unwrap_or(0);
    let (message, truncated) = if message.len() > 1024 {
        (&message[..end], true)
    } else {
        (message, false)
    };
    Output::default().emit(
        json!({"error":{"code":code,"message":message,"truncated":truncated}}),
        false,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_fit_budget_and_advance_without_losing_items() {
        let rows: Vec<_> = (0..105)
            .map(|i| json!({"id":i,"after":if i % 2 == 0 { "日本語\n".repeat(1000) } else { "x".repeat(1500) }}))
            .collect();
        let mut output = Output {
            limit: Some(100),
            ..Output::default()
        };
        let mut seen = Vec::new();
        loop {
            let page = output.page(rows.clone());
            let rendered = output.render(page.clone(), true);
            assert!(rendered.len() < MAX_RESPONSE_BYTES);
            for row in page["items"].as_array().unwrap() {
                let id = row["id"].as_u64().unwrap();
                seen.push(id);
                if id % 2 == 0 {
                    assert_eq!(row["omitted_fields"], json!(["after"]));
                    assert!(row.get("after").is_none());
                } else {
                    assert_eq!(row["after"], "x".repeat(1500));
                }
            }
            let Some(next) = page["page"]["next_offset"].as_u64() else {
                break;
            };
            assert!(next > output.offset.unwrap_or(0) as u64);
            output.offset = Some(next as usize);
        }
        assert_eq!(seen, (0..105).collect::<Vec<_>>());
        let full = Output {
            full: true,
            ..Output::default()
        }
        .page(rows.clone());
        assert_eq!(full["items"], json!(rows));
    }

    #[test]
    fn rejection_keeps_whole_diagnostics_and_counts_dropped_ones() {
        let long: Vec<_> = (0..400)
            .map(|i| json!({"code":"schema_violation","path":format!("/items/{i}/value"),"message":"x".repeat(60)}))
            .collect();
        let payload = json!({"code":"reply_validation_failed","document":"doc","diagnostics":long,
            "next_action":"Correct the reported fields."});
        let rendered = render_rejection(&payload);
        assert!(rendered.len() < MAX_RESPONSE_BYTES, "{}", rendered.len());
        let value: Value = serde_json::from_str(&rendered).unwrap();
        assert_eq!(value["ok"], false);
        assert_eq!(value["error"]["code"], "reply_validation_failed");
        assert_eq!(value["error"]["document"], "doc");
        assert!(value["error"].get("message").is_none());
        assert!(value["error"].get("truncated").is_none());
        let kept = value["error"]["diagnostics"].as_array().unwrap();
        let omitted = value["error"]["diagnostics_omitted"].as_u64().unwrap() as usize;
        assert!(kept.len() > 100 && kept.len() < 400);
        assert_eq!(kept.len() + omitted, 400);
        // Entries are dropped whole, from the end, in original order.
        for (i, diagnostic) in kept.iter().enumerate() {
            assert_eq!(diagnostic["path"], format!("/items/{i}/value"));
            assert_eq!(diagnostic["message"].as_str().unwrap().len(), 60);
        }
        let short = json!({"code":"reply_validation_failed","diagnostics":[{"path":"/a"}]});
        let value: Value = serde_json::from_str(&render_rejection(&short)).unwrap();
        assert_eq!(value["error"]["diagnostics"], json!([{"path":"/a"}]));
        assert!(value["error"].get("diagnostics_omitted").is_none());
        let no_code = json!({"diagnostics":[]});
        let value: Value = serde_json::from_str(&render_rejection(&no_code)).unwrap();
        assert_eq!(value["error"]["code"], "operation_failed");
    }

    #[test]
    fn oversized_single_response_and_empty_page_are_explicit() {
        let output = Output::default();
        let rendered = output.render(
            json!({"state":"recorded","actor":"a".repeat(100_000)}),
            true,
        );
        assert!(rendered.len() < MAX_RESPONSE_BYTES);
        let value: Value = serde_json::from_str(&rendered).unwrap();
        assert_eq!(value["ok"], true);
        assert_eq!(value["state"], "recorded");
        assert_eq!(value["omitted_fields"], json!(["actor"]));
        assert_eq!(output.page(vec![])["page"]["next_offset"], Value::Null);
    }
}
