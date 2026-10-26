//! Agent-facing presentation only. Persisted evidence never passes through here.
use serde_json::{Map, Value, json};
use std::collections::HashMap;

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

/// The length of the JSON of `value`, counted without writing it out.
fn bytes(value: &Value) -> usize {
    struct Count(usize);
    impl std::io::Write for Count {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 += bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut count = Count(0);
    serde_json::to_writer(&mut count, value).expect("JSON value");
    count.0
}

/// The lengths of a query's hits and their sum.
type Hits = (Vec<usize>, usize);

/// The length of the JSON of a search batch by part, kept up to date as its
/// hits are dropped. An array or object is 2 bytes of brackets, its members
/// and a comma between each two.
struct BatchSizes {
    /// The batch with its queries, and any tables it lists, emptied.
    rest: usize,
    /// The length of each query, and of each of its hits with their sum
    /// when it has a list.
    queries: Vec<(usize, Option<Hits>)>,
    query_total: usize,
    /// The queries with hits by the length of their hit list, then position.
    by_hits: std::collections::BTreeSet<(usize, usize)>,
    /// The total length and number of the listed tables' entries.
    tables: Option<(usize, usize)>,
    /// How many hits refer to each table.
    refers: HashMap<String, usize>,
}

fn joined(total: usize, count: usize) -> usize {
    2 + total + count.saturating_sub(1)
}

fn entry(key: &str, value: &Value) -> usize {
    bytes(&Value::String(key.to_owned())) + 1 + bytes(value)
}

impl BatchSizes {
    fn new(value: &mut Value) -> Self {
        // A batch without tables gets them as null once a hit is dropped, as
        // looking them up inserts them; a batch that cannot fit is not kept.
        if value.get("tables").is_none() {
            value["tables"] = Value::Null;
        }
        let queries = std::mem::replace(&mut value["queries"], json!([]));
        let listed = value
            .get("tables")
            .is_some_and(Value::is_object)
            .then(|| std::mem::replace(&mut value["tables"], json!({})));
        let rest = bytes(value);
        value["queries"] = queries;
        let tables = listed.map(|listed| {
            let entries = listed.as_object().unwrap();
            let total = entries.iter().map(|(key, value)| entry(key, value)).sum();
            let count = entries.len();
            value["tables"] = listed;
            (total, count)
        });
        let mut sizes = Self {
            rest,
            queries: Vec::new(),
            query_total: 0,
            by_hits: Default::default(),
            tables,
            refers: HashMap::new(),
        };
        for (index, query) in value["queries"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .enumerate()
        {
            let hits = query["items"].as_array().map(|items| {
                for item in items {
                    if let Some(table) = item["table_ref"].as_str() {
                        *sizes.refers.entry(table.to_owned()).or_default() += 1;
                    }
                }
                let hits: Vec<usize> = items.iter().map(bytes).collect();
                let sum = hits.iter().sum();
                (hits, sum)
            });
            let length = match &hits {
                Some((hits, sum)) => Self::without_hits(query) + joined(*sum, hits.len()),
                None => bytes(query),
            };
            if let Some((hits, sum)) = &hits
                && !hits.is_empty()
            {
                sizes.by_hits.insert((joined(*sum, hits.len()), index));
            }
            sizes.query_total += length;
            sizes.queries.push((length, hits));
        }
        sizes
    }
    /// The length of a query with a hit list, less that of the list.
    fn without_hits(query: &mut Value) -> usize {
        let items = std::mem::replace(&mut query["items"], json!([]));
        let length = bytes(query) - 2;
        query["items"] = items;
        length
    }
    /// The query whose hits are longest, the last of those as long.
    fn largest(&self) -> Option<usize> {
        self.by_hits.last().map(|&(_, index)| index)
    }
    /// `query`, at `index`, without its last hit and with its page updated.
    fn shortened(&mut self, query: &mut Value, index: usize) {
        let without = Self::without_hits(query);
        let (length, hits) = &mut self.queries[index];
        let (hits, sum) = hits.as_mut().unwrap();
        self.by_hits.remove(&(joined(*sum, hits.len()), index));
        *sum -= hits.pop().unwrap();
        let listed = joined(*sum, hits.len());
        if !hits.is_empty() {
            self.by_hits.insert((listed, index));
        }
        self.query_total = self.query_total - *length + without + listed;
        *length = without + listed;
    }
    /// Count one hit fewer referring to `table`; true when none is left.
    fn unrefer(&mut self, table: &str) -> bool {
        let count = self.refers.get_mut(table).unwrap();
        *count -= 1;
        *count == 0
    }
    fn referenced(&self, table: &str) -> bool {
        self.refers.get(table).is_some_and(|count| *count > 0)
    }
    /// The listed table `key` taken out.
    fn unlist(&mut self, key: &str, value: &Value) {
        let (total, count) = self.tables.as_mut().unwrap();
        *total -= entry(key, value);
        *count -= 1;
    }
    fn total(&self) -> usize {
        let queries = joined(self.query_total, self.queries.len()) - 2;
        let tables = self
            .tables
            .map_or(0, |(total, count)| joined(total, count) - 2);
        self.rest + queries + tables
    }
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
    let mut omitted = match value.as_object_mut().unwrap().remove("omitted_fields") {
        Some(Value::Array(earlier)) => earlier,
        _ => Vec::new(),
    };
    // Each member is measured once: the object is its braces and its
    // members, `"key":value`, with a comma between each two.
    let mut members: Vec<(String, usize)> = value
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), bytes(v)))
        .collect();
    let total = |members: &[(String, usize)]| {
        1 + members.len()
            + members
                .iter()
                .map(|(k, v)| bytes(&json!(k)) + 1 + v)
                .sum::<usize>()
    };
    while total(&members).max(2) > budget.saturating_sub(512) {
        let largest = members
            .iter()
            .enumerate()
            .filter(|(_, (k, _))| !["ok", "state", "summary", "page"].contains(&k.as_str()))
            .max_by_key(|(_, (k, v))| k.len() + v)
            .map(|(at, _)| at);
        let Some(at) = largest else { break };
        let (key, _) = members.remove(at);
        value.as_object_mut().unwrap().remove(&key);
        omitted.push(json!(key));
    }
    value["omitted_fields"] = json!(omitted);
    if bytes(&value) > budget {
        return json!({"ok":value.get("ok"),"omitted":true,"bytes":original_bytes});
    }
    value
}

const TABLE_BYTES: usize = 4096;

/// Remove the fields named by `steps` in order, as far as needed to fit
/// `budget`, naming each removed one in `omitted_fields`.
fn shed(mut value: Value, steps: &[&'static str], budget: usize) -> Value {
    let mut omitted = Vec::new();
    for step in steps {
        if bytes(&value) <= budget {
            break;
        }
        let mut removed = false;
        match step.split_once("[].") {
            Some((list, field)) => {
                for entry in value[list].as_array_mut().into_iter().flatten() {
                    if let Some(entry) = entry.as_object_mut() {
                        removed |= entry.remove(field).is_some();
                    }
                }
            }
            None => {
                if let Some(object) = value.as_object_mut() {
                    removed = object.remove(*step).is_some();
                }
            }
        }
        if removed {
            omitted.push(*step);
            value["omitted_fields"] = json!(omitted);
        }
    }
    value
}

/// Fit one search hit, shedding what its pointers can recover before the
/// values and the row headers. A table row's table, listed once per
/// response, is fitted on its own and keeps its column headers last.
pub fn fit_search_item(mut item: Value) -> Value {
    // Room for the `index` that `Output::page` adds to a shortened item.
    const INDEX_BYTES: usize = 32;
    let table = item
        .as_object_mut()
        .and_then(|object| object.remove("table"));
    let mut item = shed(
        item,
        &[
            "sources[].position",
            "text",
            "sources",
            "values",
            "row_context",
        ],
        ITEM_BYTES - INDEX_BYTES,
    );
    if let Some(table) = table {
        item["table"] = shed(table, &["within", "headings", "columns"], TABLE_BYTES);
    }
    item
}

/// Tables listed once per search response. Rows of the same table under the
/// same headers name it by `table_ref` instead of repeating it.
#[derive(Default)]
pub struct SearchTables {
    ids: HashMap<String, String>,
    listed: Map<String, Value>,
}

/// A table first seen on a hit, listed once that hit is kept on the page.
struct NewTable {
    key: String,
    id: String,
    table: Value,
}

impl SearchTables {
    /// Move a hit's table out of it. A hit outside a table gets no reference.
    fn detach(&self, item: &mut Value) -> Option<NewTable> {
        let table = item.as_object_mut()?.remove("table")?;
        let key = table.to_string();
        if let Some(id) = self.ids.get(&key) {
            item["table_ref"] = json!(id);
            return None;
        }
        let id = format!("t{}", self.ids.len() + 1);
        item["table_ref"] = json!(id);
        Some(NewTable { key, id, table })
    }

    fn add(&mut self, new: NewTable) {
        self.ids.insert(new.key, new.id.clone());
        self.listed.insert(new.id, new.table);
    }

    pub fn into_value(self) -> Value {
        Value::Object(self.listed)
    }
}

impl Output {
    /// Keep every query visible when a search batch exceeds the response budget.
    /// Drop trailing hits from the largest pages, preserving their pagination.
    pub fn fit_search_batch(&self, mut value: Value) -> Result<Value, Value> {
        if self.full || bytes(&value) + 16 <= MAX_RESPONSE_BYTES {
            return Ok(value);
        }
        value["detail"] = json!(
            "Some hits were omitted to fit the response. Read affected queries separately when items are omitted."
        );
        if !value["queries"].is_array() {
            return Ok(value);
        }
        // The length of the JSON is kept as hits are dropped, rather than
        // written out again for each: only the shortened query and the
        // tables it no longer refers to are measured again.
        let mut sizes = BatchSizes::new(&mut value);
        let mut first = true;
        loop {
            let Some(index) = sizes.largest() else {
                return Err(json!({"error":{"code":"search_response_too_large",
                    "message":"Use fewer or shorter queries to fit the search response."}}));
            };
            let query = &mut value["queries"][index];
            let dropped = query["items"].as_array_mut().unwrap().pop().unwrap();
            let returned = query["items"].as_array().unwrap().len();
            query["page"]["returned"] = json!(returned);
            let offset = query["page"]["offset"].as_u64().unwrap();
            let total = query["page"]["total"].as_u64().unwrap();
            query["page"]["next_offset"] = if returned == 0 {
                query["omitted_fields"] = json!(["items"]);
                Value::Null
            } else if offset + returned as u64 >= total {
                Value::Null
            } else {
                json!(offset + returned as u64)
            };
            sizes.shortened(query, index);
            // The tables no hit refers to any more: at first every unreferenced one.
            let unreferenced = dropped["table_ref"]
                .as_str()
                .filter(|table| sizes.unrefer(table))
                .map(str::to_owned);
            if let Some(listed) = value["tables"].as_object_mut() {
                if first {
                    let gone: Vec<String> = listed
                        .keys()
                        .filter(|id| !sizes.referenced(id))
                        .cloned()
                        .collect();
                    for id in gone {
                        let removed = listed.remove(&id).unwrap();
                        sizes.unlist(&id, &removed);
                    }
                } else if let Some(id) = unreferenced
                    && let Some(removed) = listed.remove(&id)
                {
                    sizes.unlist(&id, &removed);
                }
            }
            first = false;
            if sizes.total() + 16 <= MAX_RESPONSE_BYTES {
                return Ok(value);
            }
        }
    }

    pub fn page(&self, items: Vec<Value>) -> Value {
        self.page_with(items, None)
    }

    /// Page search hits, listing each table once in `tables`. A table counts
    /// toward the page budget only on the hit that first lists it.
    pub fn page_search(&self, items: Vec<Value>, tables: &mut SearchTables) -> Value {
        self.page_with(items, Some(tables))
    }

    fn page_with(&self, items: Vec<Value>, mut tables: Option<&mut SearchTables>) -> Value {
        let total = items.len();
        let offset = self.offset.unwrap_or(0).min(total);
        let limit = if self.full {
            total
        } else {
            self.limit.unwrap_or(20) as usize
        };
        let mut selected = Vec::new();
        let mut used = 0;
        for (index, mut item) in items.into_iter().enumerate().skip(offset).take(limit) {
            let new = tables.as_deref().and_then(|t| t.detach(&mut item));
            let mut item = if self.full {
                item
            } else {
                bounded(item, ITEM_BYTES)
            };
            if item.get("omitted_fields").is_some() || item.get("omitted").is_some() {
                item["index"] = json!(index);
            }
            let listed = new
                .as_ref()
                .map_or(0, |new| bytes(&new.table) + new.id.len() + 4);
            let size = bytes(&item) + 1 + listed;
            if !self.full && used + size > PAGE_BYTES {
                break;
            }
            used += size;
            if let (Some(tables), Some(new)) = (tables.as_deref_mut(), new) {
                tables.add(new);
            }
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
    fn bounded_values_drop_the_members_whole_serialization_would() {
        // [`bounded`] as written first: every member measured again after
        // each one is dropped.
        fn by_serializing(mut value: Value, budget: usize) -> Value {
            let size = |value: &Value| serde_json::to_vec(value).unwrap().len();
            if size(&value) <= budget {
                return value;
            }
            let original_bytes = size(&value);
            if !value.is_object() {
                return json!({"omitted":true,"bytes":original_bytes});
            }
            let mut omitted = match value.as_object_mut().unwrap().remove("omitted_fields") {
                Some(Value::Array(earlier)) => earlier,
                _ => Vec::new(),
            };
            while size(&value) > budget.saturating_sub(512) {
                let largest = value
                    .as_object()
                    .unwrap()
                    .iter()
                    .filter(|(k, _)| !["ok", "state", "summary", "page"].contains(&k.as_str()))
                    .max_by_key(|(k, v)| k.len() + size(v))
                    .map(|(k, _)| k.clone());
                let Some(key) = largest else { break };
                value.as_object_mut().unwrap().remove(&key);
                omitted.push(json!(key));
            }
            value["omitted_fields"] = json!(omitted);
            if size(&value) > budget {
                return json!({"ok":value.get("ok"),"omitted":true,"bytes":original_bytes});
            }
            value
        }
        let long = |n: usize| "引用\"\\\n\u{1}".repeat(n);
        let values = [
            json!({}),
            json!([long(900)]),
            json!({"ok":true,"page":{"total":3},"a\"b":long(300),"日本":long(300),"b":long(300)}),
            json!({"tie":"x".repeat(1000),"tied":"x".repeat(999),"z":[long(50), {"k":long(50)}]}),
            json!({"state":long(2000),"summary":"s","omitted_fields":["earlier"],"x":long(10)}),
            json!({"items":(0..200).map(|i| json!({"i":i,"t":long(i % 7)})).collect::<Vec<_>>()}),
        ];
        for value in values {
            for budget in [600, 1500, 4096, MAX_RESPONSE_BYTES - 1] {
                assert_eq!(
                    bounded(value.clone(), budget),
                    by_serializing(value.clone(), budget),
                    "{budget}"
                );
            }
            assert_eq!(bytes(&value), serde_json::to_vec(&value).unwrap().len());
        }
    }

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

    #[test]
    fn search_items_keep_values_and_table_headers_longest() {
        let table = json!({"element":"t","sheet":"S",
            "within":[{"headers":["項目名"],"text":"顧客コード".repeat(100)}],
            "columns":[{"column":"A","headers":["大項目","中項目"]}]});
        let item = json!({"document_id":"d","title":"d / S","kind":"row","text":"本文".repeat(250),
            "sources":[{"pointer":"/sheets/0/cells/9/value","sheet":"S","address":"A11",
                "position":{"headings":["見出し".repeat(20)]},"start":0,"end":600}],
            "table":table,"row":11,"values":["本文".repeat(250)]});
        let fitted = fit_search_item(item.clone());
        let mut rest = fitted.clone();
        rest.as_object_mut().unwrap().remove("table");
        assert!(bytes(&rest) < ITEM_BYTES);
        assert_eq!(
            fitted["omitted_fields"],
            json!(["sources[].position", "text"])
        );
        assert_eq!(fitted["values"], item["values"]);
        assert_eq!(fitted["sources"][0]["address"], "A11");
        // The table is fitted apart: its parent records go before its columns.
        assert!(bytes(&fitted["table"]) <= TABLE_BYTES);
        assert_eq!(fitted["table"]["columns"], table["columns"]);
        assert!(fitted["table"].get("omitted_fields").is_none());
        let mut wide = item.clone();
        wide["table"]["within"][0]["text"] = json!("顧客".repeat(2000));
        let fitted = fit_search_item(wide);
        assert_eq!(fitted["table"]["omitted_fields"], json!(["within"]));
        assert_eq!(fitted["table"]["columns"], table["columns"]);

        let mut huge = item.clone();
        huge["title"] = json!("t".repeat(3000));
        let page = Output::default()
            .page_search(vec![fit_search_item(huge)], &mut SearchTables::default());
        let kept = &page["items"][0];
        assert!(bytes(kept) <= ITEM_BYTES);
        assert_eq!(kept["index"], 0);
        assert_eq!(kept["table_ref"], "t1");
        assert_eq!(
            kept["omitted_fields"],
            json!(["sources[].position", "text", "sources", "values", "title"])
        );

        let short = json!({"text":"x","table":{"columns":[]}});
        assert_eq!(fit_search_item(short.clone()), short);
    }

    #[test]
    fn shared_tables_count_once_toward_the_page() {
        let table =
            json!({"element":"t","columns":[{"column":"A","headers":["見出し".repeat(150)]}]});
        let items: Vec<_> = (0..40)
            .map(|i| json!({"values":[format!("row {i}")],"table":table}))
            .collect();
        let output = Output {
            limit: Some(100),
            ..Output::default()
        };
        let inline = output.page(items.clone());
        let mut tables = SearchTables::default();
        let shared = output.page_search(items, &mut tables);
        let returned = shared["page"]["returned"].as_u64().unwrap();
        assert_eq!(returned, 40);
        assert!(inline["page"]["returned"].as_u64().unwrap() < returned);
        let listed = tables.into_value();
        assert_eq!(listed, json!({"t1":table}));
        let page = &shared["items"];
        assert!(
            page.as_array()
                .unwrap()
                .iter()
                .all(|i| i["table_ref"] == "t1" && i.get("table").is_none())
        );

        let mut tables = SearchTables::default();
        let distinct: Vec<_> = (0..2)
            .map(|i| json!({"text":"x".repeat(1200),"table":{"element":format!("t{i}")}}))
            .collect();
        let queries: Vec<_> = (0..16)
            .map(|i| {
                let mut query = output.page_search(
                    if i == 0 {
                        distinct.clone()
                    } else {
                        vec![json!({"text":"x".repeat(1200)})]
                    },
                    &mut tables,
                );
                query["query"] = json!(format!("q{i}"));
                query
            })
            .collect();
        let fitted = output
            .fit_search_batch(json!({"queries":queries,"tables":tables.into_value()}))
            .unwrap();
        let kept = fitted["queries"][0]["items"].as_array().unwrap();
        let listed = fitted["tables"].as_object().unwrap();
        assert_eq!(listed.len(), kept.len(), "{fitted}");
        assert!(
            kept.iter()
                .all(|i| listed.contains_key(i["table_ref"].as_str().unwrap()))
        );
    }

    #[test]
    fn search_batch_keeps_query_list_when_hits_exceed_budget() {
        let queries: Vec<_> = (0..32)
            .map(|i| {
                json!({"query":format!("term{i}"),"ok":true,
                "items":[{"text":"x".repeat(1200)}],
                "page":{"total":2,"offset":0,"returned":1,"next_offset":1}})
            })
            .collect();
        let output = Output::default();
        let fitted = output
            .fit_search_batch(json!({"queries":queries,"revision":"r"}))
            .unwrap();
        let rendered: Value = serde_json::from_str(&output.render(fitted, true)).unwrap();
        let kept = rendered["queries"].as_array().unwrap();
        assert_eq!(kept.len(), 32);
        assert!(kept.iter().any(|q| q["omitted_fields"] == json!(["items"])));
        assert!(kept.iter().all(|q| q["query"].is_string()));
        assert!(kept.iter().all(|q| {
            q["page"]["returned"].as_u64().unwrap() == q["items"].as_array().unwrap().len() as u64
        }));
    }

    #[test]
    fn fitted_batches_match_serializing_after_each_dropped_hit() {
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = |below: usize| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % below as u64) as usize
        };
        let output = Output::default();
        for _ in 0..60 {
            let queries: Vec<_> = (0..2 + next(24))
                .map(|q| {
                    let count = next(14);
                    let items: Vec<_> = (0..count)
                        .map(|_| {
                            let mut item = json!({"text":"語\"".repeat(1 + next(160))});
                            if next(3) > 0 {
                                item["table_ref"] = json!(format!("t{}", next(9)));
                            }
                            item
                        })
                        .collect();
                    let total = count + next(3);
                    let mut query = json!({"query":format!("q{q}"),"ok":true,"items":items,
                        "page":{"offset":next(3),"total":total,"returned":count,"next_offset":null}});
                    if next(10) == 0 {
                        query = json!({"query":format!("q{q}"),"ok":false,"error":"failed"});
                    }
                    query
                })
                .collect();
            let tables: Map<String, Value> = (0..12)
                .map(|t| (format!("t{t}"), json!({"sheet":"x".repeat(next(400))})))
                .collect();
            let mut batch = json!({"queries":queries,"revision":"r"});
            if next(4) > 0 {
                batch["tables"] = Value::Object(tables);
            }
            assert_eq!(
                output.fit_search_batch(batch.clone()),
                scanned_fit(&output, batch)
            );
        }
    }

    // The fitting as it was done before the lengths were kept: the whole batch
    // written out again after each dropped hit. Kept as the reference.
    fn scanned_fit(output: &Output, mut value: Value) -> Result<Value, Value> {
        use std::collections::HashSet;
        let this = output;
        if this.full {
            return Ok(value);
        }
        let mut shortened = false;
        while bytes(&value) + 16 > MAX_RESPONSE_BYTES {
            if !shortened {
                value["detail"] = json!(
                    "Some hits were omitted to fit the response. Read affected queries separately when items are omitted."
                );
            }
            let Some(queries) = value["queries"].as_array_mut() else {
                break;
            };
            let candidate = queries
                .iter()
                .enumerate()
                .filter(|(_, q)| q["items"].as_array().is_some_and(|items| !items.is_empty()))
                .max_by_key(|(_, q)| bytes(&q["items"]))
                .map(|(index, _)| index);
            let Some(index) = candidate else {
                return Err(json!({"error":{"code":"search_response_too_large",
                    "message":"Use fewer or shorter queries to fit the search response."}}));
            };
            let query = &mut queries[index];
            query["items"].as_array_mut().unwrap().pop();
            let returned = query["items"].as_array().unwrap().len();
            query["page"]["returned"] = json!(returned);
            let offset = query["page"]["offset"].as_u64().unwrap();
            let total = query["page"]["total"].as_u64().unwrap();
            query["page"]["next_offset"] = if returned == 0 {
                query["omitted_fields"] = json!(["items"]);
                Value::Null
            } else if offset + returned as u64 >= total {
                Value::Null
            } else {
                json!(offset + returned as u64)
            };
            shortened = true;
            let referenced: HashSet<String> = value["queries"]
                .as_array()
                .into_iter()
                .flatten()
                .flat_map(|q| q["items"].as_array().into_iter().flatten())
                .filter_map(|item| item["table_ref"].as_str().map(str::to_owned))
                .collect();
            if let Some(listed) = value["tables"].as_object_mut() {
                listed.retain(|id, _| referenced.contains(id));
            }
        }
        Ok(value)
    }
}
