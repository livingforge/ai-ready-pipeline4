//! Source-preserving extraction partitions. Shared context never counts as disposed text.
use super::*;
use std::{cell::OnceCell, collections::HashMap};

pub fn reply_schema() -> Value {
    reply_schema_for(false)
}

/// The reply schema of an extraction task. A partition or an incremental
/// scope leaves relationships to the linking stage; a whole document may
/// give its local links.
pub fn reply_schema_for(links: bool) -> Value {
    let mut full = crate::semantic_contract::reply_schema();
    if !links {
        for field in ["requirements", "related"] {
            full["$defs"]["classification"]["properties"][field]["maxItems"] = json!(0);
        }
    }
    let defs = full.as_object_mut().unwrap().remove("$defs").unwrap();
    json!({"$defs":defs,"oneOf":[full,{"type":"object","additionalProperties":false,"required":["packet","document","request_tables","reason"],"properties":{
        "packet":{"type":"string"},"document":{"type":"string"},"request_tables":{"type":"array","minItems":1,"uniqueItems":true,"items":{"type":"integer","minimum":0}},"reason":{"type":"string","minLength":1}}}]})
}

pub fn task_text(packet: &Value) -> String {
    format!(
        "{}\nExtract claims for scope.sources only; scope.context provides context but is not an extraction target. Keys belong to this partition; omit relationships until the linking stage after all partitions are assembled. Context rows are candidates, not confirmed headers. In this packet, sources.tables merges and structure cover only the scope's cells and the headers they name; omitted_cells counts element cells left out. If context is insufficient, return ONLY packet, document, request_tables (indexes into sources.tables), reason to request additional table context. Do not guess or exclude an unexplained claim.\nReply schema: {}\n{}",
        crate::semantic::PROMPT,
        reply_schema(),
        packet
    )
}

pub fn document_task_text(packet: &Value, modules: &Value) -> String {
    format!(
        "{}\n\n{}\n\nProject module vocabulary: {}\nReply schema: {}",
        crate::semantic::PROMPT,
        packet,
        modules,
        crate::semantic_contract::reply_schema()
    )
}

pub fn partitions(
    packet: &Value,
    declared: &[Value],
    max_chars: usize,
    max_sources: usize,
    max_bytes: usize,
    modules: &Value,
) -> Result<Vec<Value>> {
    if !declared.is_empty() {
        return Ok(declared.to_vec());
    }
    let rows = arr(&packet["sources"]["rows"])?;
    let max_sources = if max_sources == 0 {
        usize::MAX
    } else {
        max_sources
    };
    let max_chars = if max_chars == 0 {
        usize::MAX
    } else {
        max_chars
    };
    if rows.len() <= max_sources
        && document_task_text(packet, modules).len() <= max_bytes
        && rows
            .iter()
            .map(|r| r[4].as_str().unwrap_or("").chars().count())
            .sum::<usize>()
            <= max_chars
    {
        return Ok(vec![]);
    }
    let tables = arr(&packet["sources"]["tables"])?;
    let mut sheets: BTreeMap<String, Vec<&Value>> = BTreeMap::new();
    for row in rows {
        let table = &tables[row[1].as_u64().context("table index required")? as usize];
        sheets
            .entry(table["sheet"].to_string())
            .or_default()
            .push(row);
    }
    // A task is its fixed text and the packet, so only the packet is measured.
    let fixed = task_text(&Value::Null).len() - "null".len();
    let scoper = Scoper::new(packet, modules);
    let mut result = Vec::new();
    for sheet in sheets.values() {
        let mut start = 0;
        while start < sheet.len() {
            // The sources the source and character limits allow.
            let mut limit = start;
            let mut chars = 0;
            while limit < sheet.len() && limit - start < max_sources {
                let size = s(&sheet[limit][4])?.chars().count();
                if limit > start && chars + size > max_chars {
                    break;
                }
                chars += size;
                limit += 1;
            }
            // The most of them within the byte limit. A task only grows as sources
            // are added (a context row that becomes a source is already in the
            // packet), so bisection finds it without building a packet per source.
            let bytes = |end: usize| -> Result<usize> {
                Ok(fixed + scoper.scoped(&region(sheet, start, end))?.to_string().len())
            };
            let first = bytes(start + 1)?;
            ensure!(
                first <= max_bytes,
                "extraction source and context exceed byte limit ({first} > {max_bytes}); increase --extract-max-bytes or provide an explicit region plan"
            );
            let (mut fits, mut exceeds) = (start + 1, limit + 1);
            while exceeds - fits > 1 {
                let middle = fits + (exceeds - fits) / 2;
                if bytes(middle)? <= max_bytes {
                    fits = middle;
                } else {
                    exceeds = middle;
                }
            }
            result.push(region(sheet, start, fits));
            start = fits;
        }
    }
    Ok(result)
}

fn region(sheet: &[&Value], start: usize, end: usize) -> Value {
    let targets: BTreeSet<_> = sheet[start..end].iter().map(|r| r[0].to_string()).collect();
    let context: BTreeSet<_> = sheet
        .iter()
        .take(8)
        .chain(sheet[start.saturating_sub(8)..start].iter())
        .map(|r| r[0].to_string())
        .filter(|r| !targets.contains(r))
        .collect();
    json!({"sources":sheet[start..end].iter().map(|r|r[0].clone()).collect::<Vec<_>>(),
        "context":context.iter().map(|r|serde_json::from_str::<Value>(r).unwrap()).collect::<Vec<_>>()})
}

#[cfg(test)]
mod sizing_tests;

pub fn scoped_packet(packet: &Value, region: &Value, modules: &Value) -> Result<Value> {
    Scoper::new(packet, modules).scoped(region)
}

/// A packet indexed for the scoped packets of many regions: its rows by
/// reference, and each table's structure cells and merged ranges. Each index
/// is made where a scoped packet first reads what it covers, so a scoped
/// packet fails as [`scoped_packet`] fails; a region then costs what it
/// shows rather than the whole packet.
pub struct Scoper<'a> {
    packet: &'a Value,
    modules: &'a Value,
    /// The rows of each reference, in packet order.
    rows: OnceCell<HashMap<&'a str, Vec<usize>>>,
    shown: ShownTables<'a>,
}

/// The structure and merged ranges of each table of a packet, indexed when a
/// table is first shown, for showing the tables of many scopes.
pub(super) struct ShownTables<'a> {
    structures: OnceCell<Vec<OnceCell<Structure<'a>>>>,
    merges: OnceCell<Vec<OnceCell<Merges<'a>>>>,
}

/// The value `make` gives, kept in `cell`; a failure is not kept.
fn cached<T>(cell: &OnceCell<T>, make: impl FnOnce() -> Result<T>) -> Result<&T> {
    if let Some(value) = cell.get() {
        return Ok(value);
    }
    let value = make()?;
    Ok(cell.get_or_init(|| value))
}

impl<'a> Scoper<'a> {
    pub fn new(packet: &'a Value, modules: &'a Value) -> Self {
        Self {
            packet,
            modules,
            rows: OnceCell::new(),
            shown: ShownTables::new(),
        }
    }

    pub fn scoped(&self, region: &Value) -> Result<Value> {
        let packet = self.packet;
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
        let all = arr(&packet["sources"]["rows"])?;
        let by_ref = self.rows.get_or_init(|| {
            let mut by_ref = HashMap::<&str, Vec<usize>>::new();
            for (i, row) in all.iter().enumerate() {
                by_ref
                    .entry(row[0].as_str().unwrap_or(""))
                    .or_default()
                    .push(i);
            }
            by_ref
        });
        let mut positions: Vec<usize> = wanted
            .iter()
            .filter_map(|r| by_ref.get(r))
            .flatten()
            .copied()
            .collect();
        positions.sort_unstable();
        let rows: Vec<&Value> = positions.into_iter().map(|i| &all[i]).collect();
        sources.insert(
            "tables".into(),
            json!(self.shown_tables(arr(&packet["sources"]["tables"])?, &rows)?),
        );
        sources.insert("rows".into(), json!(rows));
        result["sources"] = Value::Object(sources);
        result["parent_packet"] = packet["packet"].clone();
        result["scope"] = region.clone();
        result["modules"] = self.modules.clone();
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
    fn shown_tables(&self, tables: &'a [Value], rows: &[&Value]) -> Result<Vec<Value>> {
        let all: Vec<usize> = (0..tables.len()).collect();
        self.shown.of(tables, rows, &all)
    }
}

impl<'a> ShownTables<'a> {
    pub(super) fn new() -> Self {
        Self {
            structures: OnceCell::new(),
            merges: OnceCell::new(),
        }
    }

    /// The tables `which` of `tables`, in that order, as `rows` show them:
    /// each one's structure and merges kept to the cells of the rows on its
    /// sheet and the headers they name, as [`Scoper`] shows a partition's.
    pub(super) fn of(
        &self,
        tables: &'a [Value],
        rows: &[&Value],
        which: &[usize],
    ) -> Result<Vec<Value>> {
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
        let structures = self
            .structures
            .get_or_init(|| tables.iter().map(|_| OnceCell::new()).collect());
        let mut result = Vec::with_capacity(which.len());
        let mut covered = shown.clone();
        for &i in which {
            let (table, index) = (&tables[i], &structures[i]);
            let mut trimmed: Map<String, Value> = obj(table)?
                .iter()
                .filter(|(key, _)| *key != "merges" && *key != "structure")
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            if let Some(structure) = table.get("structure") {
                let index = cached(index, || Structure::new(structure))?;
                trimmed.insert(
                    "structure".into(),
                    index.shown(structure, &shown, &mut covered)?,
                );
            }
            result.push(trimmed);
        }
        let merged = self
            .merges
            .get_or_init(|| tables.iter().map(|_| OnceCell::new()).collect());
        for (&i, trimmed) in which.iter().zip(&mut result) {
            let (table, index) = (&tables[i], &merged[i]);
            let Some(merges) = table.get("merges") else {
                continue;
            };
            let cells = table["sheet"]
                .as_str()
                .and_then(|sheet| covered.get(sheet))
                .into_iter()
                .flatten()
                .map(|address| crate::excel::coordinate(address).map(|(column, row)| (row, column)))
                .collect::<Result<BTreeSet<_>>>()?;
            let index = cached(index, || Merges::new(merges))?;
            trimmed.insert("merges".into(), Value::Array(index.covering(&cells)));
        }
        Ok(result.into_iter().map(Value::Object).collect())
    }
}

/// Cell addresses by sheet name.
type SheetCells = BTreeMap<String, BTreeSet<String>>;

/// The cells of a table's structure by address and by ID.
struct Structure<'a> {
    elements: &'a [Value],
    /// The headers and the nested-table context named for each cell ID, by its last cell.
    headers: BTreeMap<&'a str, &'a Value>,
    contexts: BTreeMap<&'a str, &'a Value>,
    /// The IDs of the cells at each address of each sheet.
    at: HashMap<(&'a str, &'a str), Vec<&'a str>>,
    /// The (element, cell) positions of the cells with each ID.
    with_id: HashMap<&'a str, Vec<(usize, usize)>>,
}

impl<'a> Structure<'a> {
    fn new(structure: &'a Value) -> Result<Self> {
        let elements = arr(&structure["elements"])?;
        let mut index = Self {
            elements,
            headers: BTreeMap::new(),
            contexts: BTreeMap::new(),
            at: HashMap::new(),
            with_id: HashMap::new(),
        };
        for (e, element) in elements.iter().enumerate() {
            let sheet = s(&element["sheet"])?;
            for (c, cell) in arr(&element["cells"])?.iter().enumerate() {
                let id = s(&cell["id"])?;
                index.headers.insert(id, &cell["headers"]);
                if let Some(context) = element.get("parent").map(|p| &p["context"]) {
                    index.contexts.insert(id, context);
                }
                let address = s(&cell["address"])?;
                index.at.entry((sheet, address)).or_default().push(id);
                index.with_id.entry(id).or_default().push((e, c));
            }
        }
        Ok(index)
    }

    /// `structure` reduced to the cells at `shown`, the headers they name and the
    /// context cells of the tables they are nested in; the kept cells are added to `covered`.
    fn shown(
        &self,
        structure: &Value,
        shown: &SheetCells,
        covered: &mut SheetCells,
    ) -> Result<Value> {
        let mut kept: BTreeSet<&str> = BTreeSet::new();
        for (sheet, addresses) in shown {
            for address in addresses {
                if let Some(ids) = self.at.get(&(sheet.as_str(), address.as_str())) {
                    kept.extend(ids.iter().copied());
                }
            }
        }
        let headers = &self.headers;
        let mut pending: Vec<&str> = kept.iter().copied().collect();
        while let Some(id) = pending.pop() {
            for header in [headers.get(id), self.contexts.get(id)]
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
        // The kept cells by element, in order.
        let mut positions: Vec<(usize, usize)> = kept
            .iter()
            .filter_map(|id| self.with_id.get(id))
            .flatten()
            .copied()
            .collect();
        positions.sort_unstable();
        let mut shown_elements = Vec::new();
        for group in positions.chunk_by(|a, b| a.0 == b.0) {
            let element = &self.elements[group[0].0];
            let all = arr(&element["cells"])?;
            let cells: Vec<&Value> = group.iter().map(|&(_, c)| &all[c]).collect();
            let sheet = covered.entry(s(&element["sheet"])?.to_owned()).or_default();
            for cell in &cells {
                sheet.insert(s(&cell["address"])?.to_owned());
            }
            let mut shown_element: Map<String, Value> = obj(element)?
                .iter()
                .filter(|(key, _)| *key != "cells")
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            let omitted = all.len() - cells.len();
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
}

/// A table's merged ranges, found by the cells they cover.
struct Merges<'a> {
    merges: &'a [Value],
    /// (top, bottom, left, right, position) of each nonempty range, by top row.
    spans: Vec<(u32, u32, u32, u32, usize)>,
    /// The last bottom row under each node of a binary tree over `spans`.
    bottoms: Vec<u32>,
    leaves: usize,
}

impl<'a> Merges<'a> {
    fn new(merges: &'a Value) -> Result<Self> {
        let merges = arr(merges)?;
        let mut spans = Vec::new();
        for (i, merge) in merges.iter().enumerate() {
            let (from, to) = s(merge)?.split_once(':').context("merge range required")?;
            let (from, to) = (
                crate::excel::coordinate(from)?,
                crate::excel::coordinate(to)?,
            );
            if from.0 <= to.0 && from.1 <= to.1 {
                spans.push((from.1, to.1, from.0, to.0, i));
            }
        }
        spans.sort_unstable();
        let leaves = spans.len().next_power_of_two();
        let mut bottoms = vec![0; 2 * leaves];
        for (i, span) in spans.iter().enumerate() {
            bottoms[leaves + i] = span.1;
        }
        for node in (1..leaves).rev() {
            bottoms[node] = bottoms[2 * node].max(bottoms[2 * node + 1]);
        }
        Ok(Self {
            merges,
            spans,
            bottoms,
            leaves,
        })
    }

    /// The ranges covering any of `cells` (row, column), in their order.
    fn covering(&self, cells: &BTreeSet<(u32, u32)>) -> Vec<Value> {
        let mut kept = BTreeSet::new();
        for &(row, column) in cells {
            // The ranges starting at or above the row that reach down to it.
            let end = self.spans.partition_point(|span| span.0 <= row);
            let mut pending = vec![(1, 0, self.leaves)];
            while let Some((node, low, high)) = pending.pop() {
                if low >= end || self.bottoms[node] < row {
                    continue;
                }
                if high - low == 1 {
                    let span = self.spans[low];
                    if span.2 <= column && column <= span.3 {
                        kept.insert(span.4);
                    }
                    continue;
                }
                let middle = (low + high) / 2;
                pending.push((2 * node, low, middle));
                pending.push((2 * node + 1, middle, high));
            }
        }
        kept.into_iter().map(|i| self.merges[i].clone()).collect()
    }
}

fn selector_source(value: &Value) -> Result<&str> {
    s(if value.is_string() {
        value
    } else {
        &value["source"]
    })
}

pub fn validate_scope(reply: &Value, packet: &Value) -> Result<()> {
    let Some(scope) = packet.get("scope") else {
        return Ok(());
    };
    let targets: BTreeSet<_> = arr(&scope["sources"])?
        .iter()
        .map(s)
        .collect::<Result<_>>()?;
    let allowed: BTreeSet<_> = targets
        .iter()
        .copied()
        .chain(
            arr(&scope["context"])?
                .iter()
                .map(s)
                .collect::<Result<Vec<_>>>()?,
        )
        .collect();
    for item in arr(&reply["items"])? {
        for field in ["requirements", "related"] {
            ensure!(
                item["classification"]
                    .get(field)
                    .is_none_or(|value| value.as_array().is_some_and(Vec::is_empty)),
                "partition extraction cannot contain links; complete them in the linking stage"
            );
        }
        let evidence = arr(&item["evidence"])?;
        ensure!(
            evidence
                .iter()
                .any(|e| selector_source(e).is_ok_and(|s| targets.contains(s))),
            "partition item needs evidence from an owned source"
        );
        for e in evidence
            .iter()
            .chain(
                item["condition"]["evidence"]
                    .as_array()
                    .into_iter()
                    .flatten(),
            )
            .chain(
                ["basis", "unit_basis", "semantics_basis"]
                    .iter()
                    .filter_map(|k| item["value"].get(k)),
            )
        {
            ensure!(
                allowed.contains(selector_source(e)?),
                "partition cites source outside supplied context"
            );
        }
    }
    for exclusion in arr(&reply["exclusions"])? {
        ensure!(
            targets.contains(selector_source(&exclusion["evidence"])?),
            "shared context must not be excluded by this partition"
        );
    }
    Ok(())
}

impl Workflow {
    pub(super) fn expand_extraction(&self, task: &Task, reply: &Value) -> Result<Task> {
        let scoped = self.artifact(&task.packet)?;
        ensure!(
            reply["packet"] == scoped["packet"] && reply["document"] == scoped["document"],
            "stale extraction context request"
        );
        let mut region = scoped
            .get("scope")
            .context("complete document already supplied")?
            .clone();
        let full = self.store.json(s(
            &self.state.packets[task.document.as_deref().context("task document required")?]
        )?)?;
        let tables = arr(&full["sources"]["tables"])?;
        let requested: BTreeSet<usize> = arr(&reply["request_tables"])?
            .iter()
            .map(|v| {
                v.as_u64()
                    .map(|v| v as usize)
                    .context("table index required")
            })
            .collect::<Result<_>>()?;
        ensure!(
            requested.iter().all(|i| *i < tables.len()),
            "unknown extraction context table"
        );
        let targets: BTreeSet<String> = arr(&region["sources"])?
            .iter()
            .map(|v| s(v).map(str::to_owned))
            .collect::<Result<_>>()?;
        let mut context: BTreeSet<String> = arr(&region["context"])?
            .iter()
            .map(|v| s(v).map(str::to_owned))
            .collect::<Result<_>>()?;
        let before = context.len();
        for row in arr(&full["sources"]["rows"])? {
            if requested.contains(&(row[1].as_u64().unwrap() as usize))
                && !targets.contains(s(&row[0])?)
            {
                context.insert(s(&row[0])?.to_owned());
            }
        }
        ensure!(context.len() > before, "requested context already supplied");
        region["context"] = json!(context);
        let packet = scoped_packet(&full, &region, &self.state.modules)?;
        let mut meta = json!({"document":task.document,"packet":self.store.put_json(&packet)?});
        for key in ["partition", "base", "incremental", "context"] {
            if let Some(value) = serde_json::to_value(task)?.get(key) {
                meta[key] = value.clone();
            }
        }
        let text = if task.incremental == Some(true) {
            // Keep the original identity instructions and previous items when adding context.
            format!(
                "{}\nUpdated packet (use this packet and scope): {}",
                String::from_utf8(self.store.get(task.task.as_str())?)?,
                packet
            )
        } else {
            task_text(&packet)
        };
        self.task(Stage::Extract, text.as_bytes(), meta)
    }

    pub(super) fn extraction_candidate(&self, tasks: &[Task]) -> Result<Value> {
        let mut by_doc: BTreeMap<String, Vec<&Task>> = BTreeMap::new();
        for task in tasks {
            by_doc
                .entry(
                    task.document
                        .as_deref()
                        .context("task document required")?
                        .to_owned(),
                )
                .or_default()
                .push(task);
        }
        let mut next = self.state.replies.clone();
        let mut vocabulary = None;
        for (doc, tasks) in by_doc {
            if tasks.len() == 1 && tasks[0].incremental == Some(true) {
                let task = tasks[0];
                let merged = incremental::merge(
                    &self.artifact(&task.base)?,
                    &self.artifact(&task.reply)?,
                    &self.artifact(&task.packet)?,
                )?;
                next[&doc] = json!(self.store.put_json(&merged)?);
                continue;
            }
            if tasks.len() == 1 && tasks[0].partition.is_none() {
                next[&doc] = json!(tasks[0].reply);
                continue;
            }
            let packet = self.store.json(s(&self.state.packets[&doc])?)?;
            let mut merged = json!({"packet":packet["packet"],"document":doc,"modules":{},"items":[],"exclusions":[],"open_issues":[]});
            for task in tasks {
                let reply = self.artifact(&task.reply)?;
                let prefix = format!("p{}_", task.partition.context("partition id required")?);
                for (slug, name) in obj(&reply["modules"])? {
                    ensure!(
                        merged["modules"].get(slug).is_none_or(|old| old == name),
                        "module vocabulary conflict: {slug}; update a corrected reply with a consistent name"
                    );
                    merged["modules"][slug] = name.clone();
                }
                for item in arr(&reply["items"])? {
                    let mut item = item.clone();
                    item["key"] = json!(format!("{prefix}{}", s(&item["key"])?));
                    merged["items"].as_array_mut().unwrap().push(item);
                }
                merged["exclusions"]
                    .as_array_mut()
                    .unwrap()
                    .extend(arr(&reply["exclusions"])?.clone());
                merged["open_issues"]
                    .as_array_mut()
                    .unwrap()
                    .extend(arr(&reply["open_issues"])?.clone());
            }
            // The vocabulary is read once for every merged document.
            if vocabulary.is_none() {
                vocabulary = Some(self.module_vocabulary()?);
            }
            self.validate_reply_with(vocabulary.as_ref().unwrap(), &merged)?;
            next[&doc] = json!(self.store.put_json(&merged)?);
        }
        if tasks.iter().any(|task| task.incremental == Some(true)) {
            self.repair_missing_links(&mut next)?;
        }
        self.validate_replies(&next)?;
        Ok(next)
    }
}
