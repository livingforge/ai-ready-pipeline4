//! Bounded field repairs. Evidence selection never guesses table boundaries.
use super::extraction::ShownTables;
use super::{arr, obj, s};
use crate::data::hash;
use anyhow::{Context, Result, ensure};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap};

const FIELDS: &[&str] = &[
    "name",
    "section",
    "subject",
    "property",
    "condition",
    "value",
    "statement",
    "verification",
    "evidence",
    "classification",
];

pub fn fingerprint(v: &Value) -> String {
    hash(&super::store::encode(v))
}
fn strings(v: &Value) -> Result<BTreeSet<String>> {
    arr(v)?.iter().map(|v| Ok(s(v)?.to_owned())).collect()
}
fn optional(v: &Value) -> Result<Vec<Value>> {
    if v.is_null() {
        Ok(vec![])
    } else {
        Ok(arr(v)?.clone())
    }
}
fn refs(item: &Value) -> Result<BTreeSet<String>> {
    let mut values = optional(&item["evidence"])?;
    values.extend(optional(&item["condition"]["evidence"])?);
    for key in ["basis", "unit_basis", "semantics_basis"] {
        if !item["value"][key].is_null() {
            values.push(item["value"][key].clone());
        }
    }
    values
        .iter()
        .map(|v| Ok(s(if v.is_string() { v } else { &v["source"] })?.to_owned()))
        .collect()
}
fn links(item: &Value) -> Result<Vec<String>> {
    let mut result = Vec::new();
    for field in ["requirements", "related"] {
        for v in optional(&item["classification"][field])? {
            result.push(s(&v)?.to_owned());
        }
    }
    Ok(result)
}
pub fn regions(config: &Value, doc: &str, packet: &Value) -> Result<Vec<Value>> {
    let Some(entry) = config.get(doc) else {
        return Ok(vec![]);
    };
    ensure!(
        obj(entry)?.len() == 2 && entry["packet"] == packet["packet"],
        "stale region plan"
    );
    let regions = arr(&entry["regions"])?;
    ensure!(!regions.is_empty(), "nonempty regions required");
    let col = arr(&packet["sources"]["columns"])?
        .iter()
        .position(|x| x == "ref")
        .context("missing ref column")?;
    let known: BTreeSet<String> = arr(&packet["sources"]["rows"])?
        .iter()
        .map(|r| Ok(s(&r[col])?.to_owned()))
        .collect::<Result<_>>()?;
    let mut seen = BTreeSet::new();
    for r in regions {
        ensure!(
            obj(r)?.len() == 2 && !arr(&r["sources"])?.is_empty(),
            "invalid region"
        );
        let mut local = BTreeSet::new();
        for v in arr(&r["sources"])?.iter().chain(arr(&r["context"])?) {
            let value = s(v)?.to_owned();
            ensure!(
                known.contains(&value) && local.insert(value),
                "unknown or duplicate region source"
            );
        }
        for v in arr(&r["sources"])? {
            ensure!(seen.insert(s(v)?.to_owned()), "overlapping regions");
        }
    }
    ensure!(
        seen == known,
        "regions must audit every source exactly once"
    );
    Ok(regions.clone())
}
/// The items of a reply by key, with their relationships, read once for
/// every repair scope drawn from it rather than once for each.
struct Items<'a> {
    base: &'a Value,
    fingerprint: String,
    items: BTreeMap<String, &'a Value>,
    /// The relationships, read when a scope first needs them.
    links: std::cell::OnceCell<Links>,
}

struct Links {
    prefix: String,
    /// The relationships of each item as stored.
    stored: BTreeMap<String, Vec<String>>,
    /// The local targets of each item's relationships.
    local: BTreeMap<String, BTreeSet<String>>,
    /// The items with a relationship to each local target.
    sources: HashMap<String, Vec<String>>,
}

impl<'a> Items<'a> {
    fn new(base: &'a Value) -> Result<Self> {
        let mut items = BTreeMap::new();
        for item in arr(&base["items"])? {
            ensure!(
                items.insert(s(&item["key"])?.to_owned(), item).is_none(),
                "duplicate item key"
            );
        }
        Ok(Self {
            base,
            fingerprint: fingerprint(base),
            items,
            links: Default::default(),
        })
    }
    fn links(&self) -> Result<&Links> {
        if let Some(links) = self.links.get() {
            return Ok(links);
        }
        let prefix = format!("{}/", s(&self.base["document"])?);
        let mut stored = BTreeMap::new();
        let mut local = BTreeMap::new();
        let mut sources = HashMap::<String, Vec<String>>::new();
        for (key, item) in &self.items {
            let targets = links(item)?;
            let targets_local: BTreeSet<String> = targets
                .iter()
                .map(|r| r.strip_prefix(&prefix).unwrap_or(r).to_owned())
                .collect();
            for target in &targets_local {
                sources.entry(target.clone()).or_default().push(key.clone());
            }
            stored.insert(key.clone(), targets);
            local.insert(key.clone(), targets_local);
        }
        Ok(self.links.get_or_init(|| Links {
            prefix,
            stored,
            local,
            sources,
        }))
    }
    fn scope(&self, keys: &BTreeSet<String>, findings: Value) -> Result<Value> {
        let base = self.base;
        let items = &self.items;
        ensure!(
            !keys.is_empty() && keys.iter().all(|k| items.contains_key(k)),
            "existing target keys required"
        );
        let links = self.links()?;
        // The targets' local relationships and the items related to a target.
        let mut related = keys.clone();
        for key in keys {
            related.extend(
                links.local[key]
                    .iter()
                    .filter(|r| items.contains_key(*r))
                    .cloned(),
            );
            related.extend(links.sources.get(key).into_iter().flatten().cloned());
        }
        let mut external = BTreeSet::new();
        for k in &related {
            external.extend(
                links.stored[k]
                    .iter()
                    .filter(|r| r.contains('/') && !r.starts_with(&links.prefix))
                    .cloned(),
            );
        }
        Ok(
            json!({"base":self.fingerprint,"document":base["document"],"allowed_keys":keys,"findings":findings,
            "modules":base.get("modules").cloned().unwrap_or(json!({})),"open_issues":optional(&base["open_issues"])?,
            "items":keys.iter().map(|k| items[k]).collect::<Vec<_>>(),
            "related_items":related.difference(keys).map(|k| items[k]).collect::<Vec<_>>(),"required_external_items":external}),
        )
    }
}

/// A reply that repair tasks change, read once for all of them: each task's
/// changes are checked against it without copying or hashing it again.
pub struct Base {
    value: Value,
    fingerprint: String,
    /// The position of each item by key, when the reply has unique string
    /// keys and every relationship resolves, so applying no change succeeds.
    clean: Option<HashMap<String, usize>>,
}

impl Base {
    pub fn new(value: Value) -> Self {
        let clean = (|| -> Result<HashMap<String, usize>> {
            let items = value["items"].as_array().context("items required")?;
            let mut positions = HashMap::new();
            for (i, item) in items.iter().enumerate() {
                ensure!(
                    positions.insert(s(&item["key"])?.to_owned(), i).is_none(),
                    "duplicate item key"
                );
            }
            let prefix = format!("{}/", s(&value["document"])?);
            for item in items {
                for reference in links(item)? {
                    let local = reference.strip_prefix(&prefix).unwrap_or(&reference);
                    ensure!(
                        local.contains('/') || positions.contains_key(local),
                        "dangling local relationship"
                    );
                }
            }
            Ok(positions)
        })()
        .ok();
        Self {
            fingerprint: fingerprint(&value),
            value,
            clean,
        }
    }
    /// Whether [`apply`] accepts `patch` in `scope`, failing as it fails.
    /// Only the changed items are copied and checked: the others resolve.
    pub fn check(&self, scope: &Value, patch: &Value) -> Result<()> {
        let Some(positions) = &self.clean else {
            return apply(&self.value, scope, patch).map(drop);
        };
        ensure!(
            obj(patch)?.len() == 2
                && patch["base"] == self.fingerprint
                && scope["base"] == patch["base"],
            "stale base or unexpected patch fields"
        );
        let allowed = strings(&scope["allowed_keys"])?;
        let items = self.value["items"].as_array().context("items required")?;
        let mut seen = BTreeSet::new();
        let mut changed = BTreeMap::new();
        for change in arr(&patch["changes"])? {
            let key = s(&change["key"])?;
            ensure!(
                obj(change)?.len() == 2 && allowed.contains(key) && seen.insert(key),
                "duplicate or out-of-scope change"
            );
            let edits = obj(&change["set"])?;
            ensure!(
                !edits.is_empty() && edits.keys().all(|k| allowed_path(k)),
                "unsupported field edit"
            );
            let position = *positions.get(key).context("missing target")?;
            let item = changed
                .entry(position)
                .or_insert_with(|| items[position].clone());
            edit(item, edits)?;
        }
        let prefix = format!("{}/", s(&self.value["document"])?);
        for item in changed.values() {
            for reference in links(item)? {
                let local = reference.strip_prefix(&prefix).unwrap_or(&reference);
                ensure!(
                    local.contains('/') || positions.contains_key(local),
                    "dangling local relationship"
                );
            }
        }
        Ok(())
    }
}

/// Apply the field edits of one change to its item.
fn edit(item: &mut Value, edits: &Map<String, Value>) -> Result<()> {
    for (field, value) in edits {
        if field.starts_with('/') {
            let parent = field.rsplit_once('/').unwrap().0;
            ensure!(
                !edits.contains_key(parent.trim_start_matches('/')),
                "overlapping parent/leaf edits"
            );
            // Only known leaves are allowed; changing a tagged type requires full replacement.
            if matches!(
                field.as_str(),
                "/condition/reason" | "/classification/reason"
            ) {
                item.pointer_mut(parent)
                    .and_then(Value::as_object_mut)
                    .context("reason requires an existing parent object")?
                    .insert("reason".into(), value.clone());
                continue;
            }
            let slot = item
                .pointer_mut(field)
                .context("leaf does not exist; replace the complete object to change its type")?;
            *slot = value.clone();
        } else {
            item.as_object_mut()
                .context("invalid item")?
                .insert(field.clone(), value.clone());
        }
    }
    Ok(())
}

pub fn apply(base: &Value, scope: &Value, patch: &Value) -> Result<Value> {
    ensure!(
        obj(patch)?.len() == 2
            && patch["base"] == fingerprint(base)
            && scope["base"] == patch["base"],
        "stale base or unexpected patch fields"
    );
    let allowed = strings(&scope["allowed_keys"])?;
    let mut result = base.clone();
    let items = result["items"].as_array_mut().context("items required")?;
    // The position of each item by key; keys are unique and no edit changes one.
    let mut known = HashMap::new();
    for (i, item) in items.iter().enumerate() {
        ensure!(
            known.insert(s(&item["key"])?.to_owned(), i).is_none(),
            "duplicate item key"
        );
    }
    let mut seen = BTreeSet::new();
    for change in arr(&patch["changes"])? {
        let key = s(&change["key"])?;
        ensure!(
            obj(change)?.len() == 2 && allowed.contains(key) && seen.insert(key),
            "duplicate or out-of-scope change"
        );
        let edits = obj(&change["set"])?;
        ensure!(
            !edits.is_empty() && edits.keys().all(|k| allowed_path(k)),
            "unsupported field edit"
        );
        let item = &mut items[*known.get(key).context("missing target")?];
        edit(item, edits)?;
    }
    let prefix = format!("{}/", s(&base["document"])?);
    for item in items {
        for reference in links(item)? {
            let local = reference.strip_prefix(&prefix).unwrap_or(&reference);
            ensure!(
                local.contains('/') || known.contains_key(local),
                "dangling local relationship"
            );
        }
    }
    Ok(result)
}

/// Exact stored edges, including local hops that pulled in external context.
fn dependency_edges(base: &Value, scope: &Value) -> Result<Vec<Value>> {
    let doc = s(&base["document"])?;
    let mut edges = Vec::new();
    for item in arr(&scope["items"])?
        .iter()
        .chain(arr(&scope["related_items"])?)
    {
        let from = format!("{doc}/{}", s(&item["key"])?);
        for field in ["requirements", "related"] {
            for target in optional(&item["classification"][field])? {
                let target = s(&target)?;
                edges.push(json!({"from":from,"field":format!("/classification/{field}"),
                    "to":if target.contains('/') {target.to_owned()} else {format!("{doc}/{target}")}}));
            }
        }
        for target in refs(item)? {
            if target.contains('/') && !target.starts_with(&format!("{doc}/")) {
                edges.push(json!({"from":from,"field":"evidence","to":target}));
            }
        }
    }
    Ok(edges)
}
pub fn context(
    base: &Value,
    scope: &Value,
    evidence: &Value,
    extra: &BTreeSet<usize>,
) -> Result<Value> {
    Evidence::new(base, evidence, extra)?.context(scope, extra)
}

/// The rows and tables of a packet arranged for drawing repair contexts
/// from it, made once for all the repair scopes of one reply.
struct Evidence<'a> {
    base: &'a Value,
    evidence: &'a Value,
    rc: usize,
    tc: usize,
    rows: &'a [Value],
    tables: &'a [Value],
    /// The table of each source reference: that of its last row.
    by_ref: BTreeMap<&'a str, usize>,
    prefix: String,
    /// The tables of each sheet, the sheet of each table, and the rows of each table.
    sheets: Vec<Vec<usize>>,
    sheet_of: Vec<usize>,
    table_rows: Vec<Vec<usize>>,
    /// The rows of each source reference.
    ref_rows: HashMap<&'a str, Vec<usize>>,
    /// The references whose table is on each sheet.
    sheet_refs: Vec<Vec<&'a str>>,
    /// The reply's exclusions with the source each cites, read when first needed.
    exclusions: std::cell::OnceCell<Exclusions>,
    shown: ShownTables<'a>,
}

/// The sources of a repair context: the packet's columns and other fields,
/// the rows shown, and only the tables `included`, each with its number in
/// `table`, as the rows show them: its structure and merges kept to the
/// cells shown and the headers they name. A context then holds what its
/// scope shows, however many tables the packet has.
fn shown_sources<'a>(
    source: &'a Value,
    shown: &ShownTables<'a>,
    rows: &[&Value],
    included: &BTreeSet<usize>,
) -> Result<Value> {
    let which: Vec<usize> = included.iter().copied().collect();
    let tables: Vec<Value> = shown
        .of(arr(&source["tables"])?, rows, &which)?
        .into_iter()
        .zip(&which)
        .map(|(mut table, i)| {
            table["table"] = json!(i);
            table
        })
        .collect();
    let mut sources: Map<String, Value> = obj(source)?
        .iter()
        .filter(|(key, _)| *key != "rows" && *key != "tables")
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    sources.insert("tables".into(), json!(tables));
    sources.insert("rows".into(), json!(rows));
    Ok(Value::Object(sources))
}

/// A table a repair context leaves out in part, with its sheet and how many
/// sources it holds, for `request_tables`.
fn available_table(table: &Value, number: usize, sources: usize) -> Value {
    let mut entry = json!({"table":number,"source_count":sources});
    if let Some(sheet) = table.get("sheet") {
        entry["sheet"] = sheet.clone();
    }
    entry
}

/// Exclusions and the positions of those citing each source.
type Exclusions = (Vec<Value>, HashMap<String, Vec<usize>>);

impl<'a> Evidence<'a> {
    /// Fails as [`context`] does before it reads the scope; `extra` only
    /// needs to name tables of the packet.
    fn new(base: &'a Value, evidence: &'a Value, extra: &BTreeSet<usize>) -> Result<Self> {
        ensure!(
            base["packet"] == evidence["packet"] && base["document"] == evidence["document"],
            "evidence does not match reply"
        );
        let source = &evidence["sources"];
        let columns = arr(&source["columns"])?;
        let rc = columns
            .iter()
            .position(|c| c == "ref")
            .context("missing ref")?;
        let tc = columns
            .iter()
            .position(|c| c == "table")
            .context("missing table")?;
        let rows = arr(&source["rows"])?;
        let tables = arr(&source["tables"])?;
        let mut by_ref = BTreeMap::new();
        for row in rows {
            by_ref.insert(
                s(&row[rc])?,
                row[tc].as_u64().context("invalid table")? as usize,
            );
        }
        ensure!(
            by_ref.values().all(|i| *i < tables.len()) && extra.iter().all(|i| *i < tables.len()),
            "unknown requested table"
        );
        let prefix = format!("{}/", s(&base["document"])?);
        let mut sheets = Vec::<Vec<usize>>::new();
        let mut sheet_of = Vec::with_capacity(tables.len());
        let mut named = HashMap::new();
        for (i, table) in tables.iter().enumerate() {
            let key = crate::data::value_key(&json!([table["document"], table["sheet"]]));
            let sheet = *named.entry(key).or_insert_with(|| {
                sheets.push(Vec::new());
                sheets.len() - 1
            });
            sheets[sheet].push(i);
            sheet_of.push(sheet);
        }
        let mut table_rows = vec![Vec::new(); tables.len()];
        let mut ref_rows = HashMap::<&str, Vec<usize>>::new();
        for (i, row) in rows.iter().enumerate() {
            // Every row names a reference and a table index, checked above; only
            // the last row of a reference is known to name a table of the packet.
            if let Some(table) = table_rows.get_mut(row[tc].as_u64().unwrap() as usize) {
                table.push(i);
            }
            ref_rows.entry(s(&row[rc])?).or_default().push(i);
        }
        let mut sheet_refs = vec![Vec::new(); sheets.len()];
        for (reference, table) in &by_ref {
            sheet_refs[sheet_of[*table]].push(*reference);
        }
        Ok(Self {
            base,
            evidence,
            rc,
            tc,
            rows,
            tables,
            by_ref,
            prefix,
            sheets,
            sheet_of,
            table_rows,
            ref_rows,
            sheet_refs,
            exclusions: Default::default(),
            shown: ShownTables::new(),
        })
    }

    /// The tables on the sheets of `tables`, in order.
    fn on_sheets(&self, tables: &BTreeSet<usize>) -> BTreeSet<usize> {
        let sheets: BTreeSet<usize> = tables.iter().map(|&i| self.sheet_of[i]).collect();
        sheets
            .into_iter()
            .flat_map(|sheet| self.sheets[sheet].iter().copied())
            .collect()
    }

    /// The exclusions of the reply citing any of `included`, in order.
    fn exclusions(&self, included: &BTreeSet<String>) -> Result<Vec<Value>> {
        if self.exclusions.get().is_none() {
            let all = optional(&self.base["exclusions"])?;
            let mut citing = HashMap::<String, Vec<usize>>::new();
            for (i, e) in all.iter().enumerate() {
                for r in refs(&json!({"evidence":[e["evidence"]]}))? {
                    let local = r.strip_prefix(&self.prefix).unwrap_or(&r).to_owned();
                    citing.entry(local).or_default().push(i);
                }
            }
            let _ = self.exclusions.set((all, citing));
        }
        let (all, citing) = self.exclusions.get().unwrap();
        let positions: BTreeSet<usize> = included
            .iter()
            .filter_map(|r| citing.get(r))
            .flatten()
            .copied()
            .collect();
        Ok(positions.into_iter().map(|i| all[i].clone()).collect())
    }

    fn context(&self, scope: &Value, extra: &BTreeSet<usize>) -> Result<Value> {
        let (base, evidence, rc, tc) = (self.base, self.evidence, self.rc, self.tc);
        let (rows, tables, by_ref, prefix) = (self.rows, self.tables, &self.by_ref, &self.prefix);
        let source = &evidence["sources"];
        let mut local = BTreeSet::new();
        for item in arr(&scope["items"])?
            .iter()
            .chain(arr(&scope["related_items"])?)
        {
            for r in refs(item)? {
                local.insert(r.strip_prefix(prefix.as_str()).unwrap_or(&r).to_owned());
            }
        }
        let external: BTreeSet<_> = local.iter().filter(|r| r.contains('/')).cloned().collect();
        ensure!(
            local
                .iter()
                .all(|r| external.contains(r) || by_ref.contains_key(r.as_str())),
            "unknown source reference"
        );
        let mut chosen = extra.clone();
        chosen.extend(local.iter().filter_map(|r| by_ref.get(r.as_str())).copied());
        if chosen.is_empty() {
            chosen.extend(0..tables.len());
        }
        // Every table on the sheet of a chosen one.
        chosen = self.on_sheets(&chosen);
        // Without regions every reference is selected and the chosen tables' rows are
        // shown; with them, the selected references' rows on the chosen tables.
        let regions = match &scope["regions"] {
            Value::Null => &[][..],
            regions => arr(regions)?.as_slice(),
        };
        let mut selected = None;
        if !regions.is_empty() {
            let mut picked = BTreeSet::new();
            for region in regions {
                let sources = strings(&region["sources"])?;
                if !sources.is_disjoint(&local) {
                    picked.extend(sources);
                    picked.extend(strings(&region["context"])?);
                }
            }
            let extra_sheets: BTreeSet<usize> = extra.iter().map(|&j| self.sheet_of[j]).collect();
            picked.extend(
                extra_sheets
                    .iter()
                    .flat_map(|&sheet| self.sheet_refs[sheet].iter().map(|r| (*r).to_owned())),
            );
            ensure!(
                !picked.is_empty() && local.difference(&external).all(|r| picked.contains(r)),
                "region does not cover repair evidence"
            );
            chosen = picked
                .iter()
                .filter_map(|r| by_ref.get(r.as_str()))
                .copied()
                .collect();
            selected = Some(picked);
        }
        let is_selected = |r: &Value| {
            selected
                .as_ref()
                .is_none_or(|s| s.contains(r[rc].as_str().unwrap()))
        };
        let mut shown: Vec<usize> = match &selected {
            None => chosen
                .iter()
                .flat_map(|&t| self.table_rows[t].iter().copied())
                .collect(),
            Some(picked) => picked
                .iter()
                .filter_map(|r| self.ref_rows.get(r.as_str()))
                .flatten()
                .copied()
                .collect(),
        };
        shown.sort_unstable();
        shown.dedup();
        // Whether each table is chosen, looked up for every row shown.
        let mut is_chosen = vec![false; tables.len()];
        for &t in &chosen {
            if let Some(chosen) = is_chosen.get_mut(t) {
                *chosen = true;
            }
        }
        shown.retain(|&i| {
            is_chosen
                .get(rows[i][tc].as_u64().unwrap() as usize)
                .is_some_and(|chosen| *chosen)
                && is_selected(&rows[i])
        });
        let filtered: Vec<&Value> = shown.iter().map(|&i| &rows[i]).collect();
        let included: BTreeSet<String> = filtered
            .iter()
            .map(|r| r[rc].as_str().unwrap().to_owned())
            .collect();
        // A table is left out in part when it is not chosen or any of its rows is not shown.
        let mut kept = vec![0usize; tables.len()];
        for r in &filtered {
            kept[r[tc].as_u64().unwrap() as usize] += 1;
        }
        let available: Vec<Value> = self
            .table_rows
            .iter()
            .enumerate()
            .filter(|(i, all)| !all.is_empty() && (!is_chosen[*i] || kept[*i] < all.len()))
            .map(|(i, all)| available_table(&tables[i], i, all.len()))
            .collect();
        let exclusions = self.exclusions(&included)?;
        let sources = shown_sources(source, &self.shown, &filtered, &chosen)?;
        Ok(
            json!({"document":base["document"],"packet":base["packet"],"sources":sources,"warnings":optional(&evidence["warnings"])?,
        "included_tables":chosen,"expanded_tables":extra,"available_tables":available,"required_external_sources":external,"exclusions":exclusions}),
        )
    }
}
pub fn expand(
    base: &Value,
    scope: &Value,
    evidence: &Value,
    current: &Value,
    request: &Value,
) -> Result<BTreeSet<usize>> {
    ensure!(
        obj(request)?.len() == 3
            && request["base"] == fingerprint(base)
            && scope["base"] == request["base"]
            && !s(&request["reason"])?.trim().is_empty(),
        "invalid context request"
    );
    let indexes = |v: &Value| -> Result<BTreeSet<usize>> {
        arr(v)?
            .iter()
            .map(|i| Ok(i.as_u64().context("invalid table index")? as usize))
            .collect()
    };
    let prior = indexes(&current["expanded_tables"])?;
    ensure!(
        context(base, scope, evidence, &prior)? == *current,
        "context differs from original evidence"
    );
    let requested = indexes(&request["request_tables"])?;
    let available: BTreeSet<usize> = arr(&current["available_tables"])?
        .iter()
        .map(|i| i["table"].as_u64().unwrap() as usize)
        .collect();
    ensure!(
        !requested.is_empty() && requested.is_subset(&available),
        "request must select omitted tables"
    );
    let mut result = if optional(&scope["regions"])?.is_empty() {
        indexes(&current["included_tables"])?
    } else {
        prior
    };
    result.extend(requested);
    Ok(result)
}
pub fn task_text(scope: &Value) -> String {
    let mut scope = scope.clone();
    scope.as_object_mut().unwrap().remove("regions");
    format!(
        "{}\n{}",
        instructions(),
        json!({"scope":scope,"reply_schema":reply_schema()})
    )
}

pub(super) fn instructions() -> String {
    format!(
        "{}\n{}\n{}",
        format_args!(
            "Correct only selected fields using supplied evidence and findings. Return JSON {{\"base\":\"copy base reference\",\"changes\":[{{\"key\":\"target key\",\"set\":{{\"field\":\"replacement\"}}}}]}}. Omit unchanged fields/items; use supported leaf paths or replace nested objects when changing their type. Do not add/delete/rename items or fabricate evidence. Empty changes remain unresolved. Source aliases and table numbers retain original numbering. In this context sources.tables lists only the tables shown, each with its number in table: the table column of sources.rows names that number, not a position in the list, and each table's merges and structure cover only the shown cells and the headers they name. With regions only declared sources/shared context are supplied; otherwise full sheets. For missing sheets return ONLY {{\"base\":\"copy base reference\",\"request_tables\":[0],\"reason\":\"missing context\"}} using the table numbers of available_tables, which names each table's sheet. Never infer missing external evidence. Allowed fields: {}",
            FIELDS.join(", ")
        ),
        crate::semantic::PROMPT,
        "set may use /value/text, /value/unit, /value/unit_basis, /value/basis, /value/semantics_basis, /value/amount, /value/comparison, /value/semantics, /condition/text, /condition/evidence, /condition/reason, /classification/reason, /classification/requirements or /classification/related. Reason leaves may be added to existing parent objects; other leaves must exist. Changing kind/basis requires full object replacement. Never weaken a condition or quantity just to pass validation."
    )
}

fn allowed_path(path: &str) -> bool {
    FIELDS.contains(&path)
        || [
            "/value/text",
            "/value/unit",
            "/value/unit_basis",
            "/value/basis",
            "/value/semantics_basis",
            "/value/amount",
            "/value/comparison",
            "/value/semantics",
            "/condition/text",
            "/condition/evidence",
            "/condition/reason",
            "/classification/reason",
            "/classification/requirements",
            "/classification/related",
        ]
        .contains(&path)
}

pub fn reply_schema() -> Value {
    let full = crate::semantic_contract::reply_schema();
    let mut properties = serde_json::Map::new();
    for field in FIELDS {
        properties.insert(
            (*field).to_owned(),
            full["$defs"]["item"]["properties"][field].clone(),
        );
    }
    for field in [
        "unit",
        "amount",
        "comparison",
        "semantics",
        "basis",
        "unit_basis",
        "semantics_basis",
    ] {
        properties.insert(
            format!("/value/{field}"),
            full["$defs"]["value"]["oneOf"][0]["properties"][field].clone(),
        );
    }
    properties.insert("/value/text".into(), json!({"$ref":"#/$defs/text"}));
    properties.insert("/condition/text".into(), json!({"$ref":"#/$defs/text"}));
    for field in ["/condition/reason", "/classification/reason"] {
        properties.insert(field.into(), json!({"$ref":"#/$defs/text"}));
    }
    properties.insert(
        "/condition/evidence".into(),
        json!({"type":"array","minItems":1,"uniqueItems":true,"items":{"$ref":"#/$defs/span"}}),
    );
    for field in ["requirements", "related"] {
        properties.insert(
            format!("/classification/{field}"),
            full["$defs"]["classification"]["properties"][field].clone(),
        );
    }
    json!({"$defs":full["$defs"],"oneOf":[
        {"type":"object","additionalProperties":false,"required":["base","changes"],"properties":{
            "base":{"type":"string"},"changes":{"type":"array","items":{"type":"object","additionalProperties":false,"required":["key","set"],"properties":{
                "key":{"type":"string"},"set":{"type":"object","additionalProperties":false,"minProperties":1,"properties":properties}}}}}},
        {"type":"object","additionalProperties":false,"required":["base","request_tables","reason"],"properties":{
            "base":{"type":"string"},"request_tables":{"type":"array","minItems":1,"uniqueItems":true,"items":{"type":"integer","minimum":0}},"reason":{"type":"string","minLength":1}}}
    ]})
}

/// Group repairable diagnostics by overlapping evidence, retaining every deferred finding.
/// `declined` holds signatures of findings and diagnostics a repair already
/// returned unchanged; they are deferred instead of reissued.
pub fn plan(
    bases: &BTreeMap<String, Value>,
    packets: &BTreeMap<String, Value>,
    config: &Value,
    issues: &[Value],
    findings: &[Value],
    declined: &BTreeSet<String>,
) -> Result<(Vec<Value>, Vec<Value>)> {
    let mut index = BTreeMap::new();
    for (doc, base) in bases {
        for item in arr(&base["items"])? {
            let key = s(&item["key"])?;
            index.insert(format!("{doc}/{key}"), (doc.clone(), key.to_owned()));
        }
    }
    let mut unsupported = BTreeSet::new();
    for issue in issues {
        if issue["repair"]["action"] == "validator_support" {
            unsupported.extend(strings(&issue["repair"]["items"])?);
        }
    }
    let mut targets: BTreeMap<String, BTreeMap<String, Vec<Value>>> = BTreeMap::new();
    let mut deferred = Vec::new();
    for (value, review) in issues
        .iter()
        .map(|v| (v, false))
        .chain(findings.iter().map(|v| (v, true)))
    {
        if review && value["action"] == "improvement" {
            continue;
        }
        let route = if review { value } else { &value["repair"] };
        let identifiers = strings(&route["items"]).unwrap_or_default();
        let valid = route["action"] == "field_repair"
            && !identifiers.is_empty()
            && identifiers
                .iter()
                .all(|i| index.contains_key(i) && !unsupported.contains(i))
            && (!review
                || value["message"]
                    .as_str()
                    .is_some_and(|x| !x.trim().is_empty()));
        if !valid {
            deferred.push(if review {
                json!({"code":"review_finding","finding":value,"repair":{"action":"manual_triage"}})
            } else {
                value.clone()
            });
            continue;
        }
        // A field repair edits one document's items and cannot declare modules
        // or touch another document. A finding spanning documents (module
        // alignment, cross-document links) goes to restructure, which can.
        let documents: BTreeSet<&str> = identifiers
            .iter()
            .filter_map(|i| i.rsplit_once('/').map(|(d, _)| d))
            .collect();
        if review && documents.len() > 1 {
            deferred.push(json!({"code":"cross_document_finding","finding":value,"items":identifiers,
                "repair":{"action":"scoped_restructure","items":identifiers},
                "message":"A field repair is limited to one document; this finding spans documents and is routed to restructure."}));
            continue;
        }
        let signature = |id: &str| {
            if review {
                super::finding_signature(&json!({"action":"field_repair","items":[id]}))
            } else {
                super::diagnostic_signature(&value["code"], id)
            }
        };
        let (remaining, already): (Vec<String>, Vec<String>) = identifiers
            .into_iter()
            .partition(|id| !declined.contains(&signature(id)));
        if !already.is_empty() {
            let mut entry = if review {
                json!({"code":"review_finding","finding":value})
            } else {
                value.clone()
            };
            entry["repair"] = json!({"action":"manual_triage","items":already});
            entry["declined"] = json!(true);
            entry["message_for_agent"] = json!(
                "Repair already returned this without changes (see unresolved); it is not reissued while the items stay unchanged."
            );
            deferred.push(entry);
        }
        for id in remaining {
            let (doc, key) = &index[&id];
            let finding = if review {
                json!({"code":"review_finding","key":key,"finding":value,"message":value["message"],"instruction":"Verify this candidate against original evidence; do not blindly accept it."})
            } else {
                let mut finding = json!({"code":value["code"],"key":key,"fields":route["fields"]});
                for field in ["message", "basis", "actual", "candidates"] {
                    if let Some(detail) = value.get(field) {
                        finding[field] = detail.clone();
                    }
                }
                finding
            };
            targets
                .entry(doc.clone())
                .or_default()
                .entry(key.clone())
                .or_default()
                .push(finding);
        }
    }
    let mut tasks = Vec::new();
    for (doc, keyed) in targets {
        let base = &bases[&doc];
        let packet = &packets[&doc];
        let regions = regions(config, &doc, packet)?;
        // The reply and packet are read once for every group of the document.
        let items = Items::new(base)?;
        let mut evidence: Option<Evidence> = None;
        let mut context = |scope: &Value| -> Result<Value> {
            if evidence.is_none() {
                evidence = Some(Evidence::new(base, packet, &BTreeSet::new())?);
            }
            evidence.as_ref().unwrap().context(scope, &BTreeSet::new())
        };
        let make_scope = |keys: &BTreeSet<String>| -> Result<Value> {
            let findings: Vec<_> = keys.iter().flat_map(|k| keyed[k].clone()).collect();
            let mut scope = items.scope(keys, json!(findings))?;
            if !regions.is_empty() {
                scope["regions"] = json!(regions);
            }
            Ok(scope)
        };
        let mut groups = RepairGroups::default();
        let mut assigned = BTreeSet::new();
        for key in keyed.keys() {
            if assigned.contains(key) {
                continue;
            }
            let mut keys = BTreeSet::from([key.clone()]);
            // A finding is atomic even when its items cite different sheets.
            // Merge overlapping findings before checking external dependencies.
            let mut pending = vec![key.clone()];
            while let Some(key) = pending.pop() {
                for finding in &keyed[&key] {
                    for id in finding["finding"]["items"].as_array().into_iter().flatten() {
                        if let Some(local) = id
                            .as_str()
                            .and_then(|id| id.strip_prefix(&format!("{doc}/")))
                            && keyed.contains_key(local)
                            && keys.insert(local.to_owned())
                        {
                            pending.push(local.to_owned());
                        }
                    }
                }
            }
            assigned.extend(keys.iter().cloned());
            let scope = make_scope(&keys)?;
            let ctx = context(&scope)?;
            if !arr(&ctx["required_external_sources"])?.is_empty()
                || !arr(&scope["required_external_items"])?.is_empty()
            {
                deferred.push(json!({"code":"external_context_required","document":doc,"keys":keys,
                    "repair":{"action":"scoped_restructure","items":keys.iter().map(|key|format!("{doc}/{key}")).collect::<Vec<_>>()},
                    "current_external_sources":ctx["required_external_sources"],"required_external_items":scope["required_external_items"],
                    "dependency_edges":dependency_edges(base, &scope)?,"findings":scope["findings"],
                    "message":"Existing evidence or relationship dependencies cross documents. These are not newly supplied external materials. Inspect dependency_edges and verify the links against original evidence; adding citations alone does not resolve this diagnostic."}));
                continue;
            }
            let refs: BTreeSet<String> = arr(&ctx["sources"]["rows"])?
                .iter()
                .map(|r| s(&r[0]).map(str::to_owned))
                .collect::<Result<_>>()?;
            groups.add(keys, refs);
        }
        for keys in groups.into_keys() {
            let scope = make_scope(&keys)?;
            let ctx = context(&scope)?;
            tasks
                .push(json!({"document":doc,"scope":scope,"context":ctx,"text":task_text(&scope)}));
        }
    }
    Ok((tasks, deferred))
}

#[derive(Default)]
struct RepairGroups {
    parents: Vec<usize>,
    keys: Vec<BTreeSet<String>>,
    latest: Vec<usize>,
    owners: BTreeMap<String, usize>,
}

impl RepairGroups {
    fn root(&mut self, mut i: usize) -> usize {
        while self.parents[i] != i {
            self.parents[i] = self.parents[self.parents[i]];
            i = self.parents[i];
        }
        i
    }

    fn add(&mut self, keys: BTreeSet<String>, refs: BTreeSet<String>) {
        let mut root = self.parents.len();
        self.parents.push(root);
        self.keys.push(keys);
        self.latest.push(root);
        for reference in refs {
            if let Some(owner) = self.owners.get(&reference).copied() {
                let mut other = self.root(owner);
                if root != other {
                    // Move only the smaller key set when joining components.
                    if self.keys[root].len() < self.keys[other].len() {
                        std::mem::swap(&mut root, &mut other);
                    }
                    self.parents[other] = root;
                    let keys = std::mem::take(&mut self.keys[other]);
                    self.keys[root].extend(keys);
                    self.latest[root] = self.latest[root].max(self.latest[other]);
                }
            }
            self.owners.insert(reference, root);
        }
    }

    fn into_keys(self) -> Vec<BTreeSet<String>> {
        // A merged group occupies the position of its latest contributor.
        let mut groups: Vec<_> = self
            .keys
            .into_iter()
            .enumerate()
            .filter(|(i, _)| self.parents[*i] == *i)
            .map(|(i, keys)| (self.latest[i], keys))
            .collect();
        groups.sort_by_key(|(latest, _)| *latest);
        groups.into_iter().map(|(_, keys)| keys).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prepare(base: &Value, keys: &BTreeSet<String>, findings: Value) -> Result<Value> {
        Items::new(base)?.scope(keys, findings)
    }

    #[test]
    fn repair_groups_merge_bridges_and_keep_latest_order() {
        let mut groups = RepairGroups::default();
        for (key, refs) in [
            ("a", vec!["x"]),
            ("b", vec!["y"]),
            ("c", vec!["z"]),
            ("d", vec!["x", "y"]),
            ("e", vec!["y"]),
        ] {
            groups.add(
                BTreeSet::from([key.to_owned()]),
                refs.into_iter().map(str::to_owned).collect(),
            );
        }
        assert_eq!(
            groups.into_keys(),
            vec![
                BTreeSet::from(["c".to_owned()]),
                ["a", "b", "d", "e"]
                    .into_iter()
                    .map(str::to_owned)
                    .collect()
            ]
        );
    }

    #[test]
    #[ignore = "manual disjoint repair group performance measurement"]
    fn measure_disjoint_repair_groups() {
        for count in [1_000, 4_000] {
            let mut times = vec![];
            for _ in 0..5 {
                let start = std::time::Instant::now();
                let mut groups = RepairGroups::default();
                for index in 0..count {
                    groups.add(
                        BTreeSet::from([format!("k{index}")]),
                        BTreeSet::from([format!("s{index}")]),
                    );
                }
                let keys = groups.into_keys();
                times.push(start.elapsed().as_secs_f64() * 1000.0);
                assert_eq!(keys.len(), count);
                assert_eq!(keys[count - 1], BTreeSet::from([format!("k{}", count - 1)]));
            }
            times.sort_by(f64::total_cmp);
            eprintln!(
                "disjoint_repair_groups count={count} median_ms={:.3}",
                times[2]
            );
        }
    }

    #[test]
    fn linked_findings_keep_cycles_and_disjoint_groups() {
        let (mut base, mut packet) = fixture();
        base["items"] = json!([
            {"key":"a","evidence":["s2"],"classification":{"related":[]}},
            {"key":"b","evidence":["s4"],"classification":{"related":[]}},
            {"key":"c","evidence":["s4"],"classification":{"related":[]}},
            {"key":"d","evidence":["s5"],"classification":{"related":[]}}
        ]);
        packet["sources"]["tables"]
            .as_array_mut()
            .unwrap()
            .push(json!({"document":"doc","sheet":"C"}));
        packet["sources"]["rows"]
            .as_array_mut()
            .unwrap()
            .push(json!(["s5", 3, "A1", "value", "Separate", "General"]));
        let findings = [
            json!({"action":"field_repair","items":["doc/a","doc/b"],"message":"ab"}),
            json!({"action":"field_repair","items":["doc/b","doc/c"],"message":"bc"}),
            json!({"action":"field_repair","items":["doc/c","doc/a"],"message":"ca"}),
            json!({"action":"field_repair","items":["doc/d"],"message":"d"}),
        ];
        let (tasks, deferred) = plan(
            &BTreeMap::from([("doc".into(), base)]),
            &BTreeMap::from([("doc".into(), packet)]),
            &json!({}),
            &[],
            &findings,
            &BTreeSet::new(),
        )
        .unwrap();
        assert!(deferred.is_empty());
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0]["scope"]["allowed_keys"], json!(["a", "b", "c"]));
        assert_eq!(tasks[1]["scope"]["allowed_keys"], json!(["d"]));
        for finding in &findings[..3] {
            assert_eq!(
                tasks[0]["scope"]["findings"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|value| value["finding"] == *finding)
                    .count(),
                2
            );
        }
    }

    #[test]
    #[ignore = "manual linked findings performance measurement"]
    fn measure_linked_findings() {
        use std::time::Instant;
        for count in [200, 800] {
            let (mut base, packet) = fixture();
            base["items"] = json!((0..count).map(|index| json!({
                "key":format!("k{index:04}"),"evidence":["s2"],"classification":{"related":[]}
            })).collect::<Vec<_>>());
            let findings: Vec<_> = (1..count).map(|index| json!({
                "action":"field_repair","items":[format!("doc/k{:04}", index-1),format!("doc/k{index:04}")],"message":"check"
            })).collect();
            let bases = BTreeMap::from([("doc".into(), base)]);
            let packets = BTreeMap::from([("doc".into(), packet)]);
            let mut durations = Vec::new();
            let mut digest = String::new();
            for _ in 0..5 {
                let start = Instant::now();
                let (tasks, deferred) = plan(
                    &bases,
                    &packets,
                    &json!({}),
                    &[],
                    &findings,
                    &BTreeSet::new(),
                )
                .unwrap();
                durations.push(start.elapsed().as_secs_f64() * 1000.0);
                assert!(deferred.is_empty());
                assert_eq!(tasks.len(), 1);
                digest = crate::data::hash(&crate::data::encoded(&json!(tasks)));
            }
            durations.sort_by(f64::total_cmp);
            eprintln!(
                "linked_findings count={count} median_ms={:.3} sha256={digest}",
                durations[2]
            );
        }
    }
    fn fixture() -> (Value, Value) {
        (
            json!({"document":"doc","packet":"same","items":[
            {"key":"a","evidence":["s2"],"classification":{"related":[]}},
            {"key":"b","evidence":["s4"],"classification":{"related":[]}}],
            "exclusions":[{"evidence":"s3","reason":"footnote"},{"evidence":"s4","reason":"other"}]}),
            json!({"document":"doc","packet":"same","warnings":[],"sources":{
            "columns":["ref","table","cell_or_location","field","text","number_format"],
            "tables":[{"document":"doc","sheet":"A"},{"document":"doc","sheet":"A"},{"document":"doc","sheet":"B"}],
            "rows":[["s1",0,"A1","value","Units: ms","General"],["s2",0,"A2","value","5","General"],
                    ["s3",1,"A99","value","Exception footnote","General"],["s4",2,"A1","value","Other","General"]]}}),
        )
    }
    #[test]
    fn preserves_sheet_notes_and_requires_explicit_context_expansion() {
        let (base, packet) = fixture();
        let scope = prepare(&base, &BTreeSet::from(["a".into()]), json!("correct")).unwrap();
        let ctx = context(&base, &scope, &packet, &BTreeSet::new()).unwrap();
        assert_eq!(ctx["included_tables"], json!([0, 1]));
        assert_eq!(ctx["sources"]["rows"].as_array().unwrap().len(), 3);
        assert_eq!(ctx["exclusions"].as_array().unwrap().len(), 1);
        // Only the tables shown, by number; the others by number and sheet.
        assert_eq!(
            ctx["sources"]["tables"],
            json!([{"document":"doc","sheet":"A","table":0},{"document":"doc","sheet":"A","table":1}])
        );
        assert_eq!(
            ctx["available_tables"],
            json!([{"table":2,"sheet":"B","source_count":1}])
        );
        let request =
            json!({"base":scope["base"],"request_tables":[2],"reason":"need cross sheet note"});
        assert_eq!(
            expand(&base, &scope, &packet, &ctx, &request).unwrap(),
            BTreeSet::from([0, 1, 2])
        );
        assert!(apply(&base, &scope, &request).is_err());
        for indices in [json!([]), json!([true]), json!([0]), json!([8])] {
            let mut bad = request.clone();
            bad["request_tables"] = indices;
            assert!(expand(&base, &scope, &packet, &ctx, &bad).is_err());
        }
        let mut changed = ctx.clone();
        changed["sources"]["rows"][0][4] = json!("tampered");
        assert!(expand(&base, &scope, &packet, &changed, &request).is_err());
    }
    /// A context keeps the merges of a table it shows to the cells it shows,
    /// and leaves out the tables it does not, however many the packet has.
    #[test]
    fn contexts_hold_the_merges_of_the_cells_they_show() {
        let (base, mut packet) = fixture();
        packet["sources"]["tables"][0]["merges"] = json!(["A2:B2", "C7:D7"]);
        packet["sources"]["tables"][2]["merges"] = json!(["A1:C1"]);
        let scope = prepare(&base, &BTreeSet::from(["a".into()]), json!("correct")).unwrap();
        let ctx = context(&base, &scope, &packet, &BTreeSet::new()).unwrap();
        assert_eq!(ctx["sources"]["tables"][0]["merges"], json!(["A2:B2"]));
        assert_eq!(ctx["sources"]["tables"].as_array().unwrap().len(), 2);
        let wide = {
            let mut packet = packet.clone();
            let tables = packet["sources"]["tables"].as_array_mut().unwrap();
            tables.extend(
                (0..50)
                    .map(|i| json!({"document":"doc","sheet":format!("S{i}"),"merges":["A1:B1"]})),
            );
            packet
        };
        assert_eq!(
            context(&base, &scope, &wide, &BTreeSet::new()).unwrap()["sources"],
            ctx["sources"]
        );
    }
    #[test]
    fn stale_out_of_scope_duplicate_identity_and_dangling_edits_fail() {
        let (base, _) = fixture();
        let scope = prepare(&base, &BTreeSet::from(["a".into()]), json!([])).unwrap();
        let valid =
            json!({"base":scope["base"],"changes":[{"key":"a","set":{"statement":"after"}}]});
        let result = apply(&base, &scope, &valid).unwrap();
        assert_eq!(result["items"][0]["statement"], "after");
        assert_eq!(result["items"][1], base["items"][1]);
        for variant in 0..5 {
            let mut bad = valid.clone();
            match variant {
                0 => bad["base"] = json!("stale"),
                1 => bad["changes"][0]["key"] = json!("b"),
                2 => bad["changes"] = json!([valid["changes"][0], valid["changes"][0]]),
                3 => bad["changes"][0]["set"] = json!({"key":"new"}),
                _ => bad["changes"][0]["set"] = json!({"classification":{"related":["missing"]}}),
            }
            assert!(apply(&base, &scope, &bad).is_err());
        }
    }
    #[test]
    fn external_context_explains_the_local_hop_and_preserves_the_finding() {
        let (mut base, packet) = fixture();
        base["items"][0]["classification"]["related"] = json!(["b"]);
        base["items"][1]["classification"]["related"] = json!(["other/c"]);
        let finding =
            json!({"items":["doc/a"],"action":"field_repair","message":"Check the linked claim"});
        let (tasks, deferred) = plan(
            &BTreeMap::from([("doc".into(), base)]),
            &BTreeMap::from([("doc".into(), packet)]),
            &json!({}),
            &[],
            std::slice::from_ref(&finding),
            &BTreeSet::new(),
        )
        .unwrap();
        assert!(tasks.is_empty());
        let diagnostic = &deferred[0];
        assert_eq!(diagnostic["required_external_items"], json!(["other/c"]));
        assert!(
            diagnostic["dependency_edges"]
                .as_array()
                .unwrap()
                .contains(&json!({"from":"doc/a","field":"/classification/related","to":"doc/b"}))
        );
        assert!(
            diagnostic["dependency_edges"].as_array().unwrap().contains(
                &json!({"from":"doc/b","field":"/classification/related","to":"other/c"})
            )
        );
        assert_eq!(diagnostic["findings"][0]["finding"], finding);
    }

    #[test]
    fn reason_leaf_can_be_added_without_replacing_other_fields() {
        let (mut base, _) = fixture();
        base["items"][0]["condition"] = json!({"basis":"assumed","text":"original"});
        let scope = prepare(&base, &BTreeSet::from(["a".into()]), json!([])).unwrap();
        let value = json!({"base":scope["base"],"changes":[{"key":"a","set":{"/condition/reason":"explanation","/classification/reason":"classification explanation"}}]});
        let result = apply(&base, &scope, &value).unwrap();
        assert_eq!(result["items"][0]["condition"]["text"], "original");
        assert_eq!(result["items"][0]["condition"]["reason"], "explanation");
        let mut bad = value;
        bad["changes"][0]["set"]["condition"] = json!({"basis":"unspecified"});
        assert!(apply(&base, &scope, &bad).is_err());
    }
    #[test]
    fn source_regions_preserve_shared_context_and_validate_full_coverage() {
        let (base, mut packet) = fixture();
        packet["sources"]["tables"] = json!([{"document":"doc","sheet":"A"}]);
        for row in packet["sources"]["rows"].as_array_mut().unwrap() {
            row[1] = json!(0);
        }
        let config = json!({"doc":{"packet":"same","regions":[{"sources":["s1"],"context":[]},{"sources":["s2"],"context":["s1"]},{"sources":["s3"],"context":[]},{"sources":["s4"],"context":[]}]}});
        let mut scope = prepare(&base, &BTreeSet::from(["a".into()]), json!([])).unwrap();
        scope["regions"] = json!(regions(&config, "doc", &packet).unwrap());
        let ctx = context(&base, &scope, &packet, &BTreeSet::new()).unwrap();
        assert_eq!(ctx["sources"]["rows"].as_array().unwrap().len(), 2);
        let request =
            json!({"base":scope["base"],"request_tables":[0],"reason":"need remaining notes"});
        let extra = expand(&base, &scope, &packet, &ctx, &request).unwrap();
        assert_eq!(
            context(&base, &scope, &packet, &extra).unwrap()["sources"]["rows"]
                .as_array()
                .unwrap()
                .len(),
            4
        );
        let mut bad = config.clone();
        bad["doc"]["regions"].as_array_mut().unwrap().pop();
        assert!(regions(&bad, "doc", &packet).is_err());
        let mut bad = config;
        bad["doc"]["packet"] = json!("stale");
        assert!(regions(&bad, "doc", &packet).is_err());
    }

    struct Draws(u64);
    impl Draws {
        fn next(&mut self, below: usize) -> usize {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 % below as u64) as usize
        }
    }

    /// A reply with links within it, across documents and to nothing, and its
    /// packet with tables sharing sheets, a reference listed twice, and exclusions.
    fn linked(draws: &mut Draws) -> (Value, Value) {
        let keys = 2 + draws.next(10);
        let sources = 2 + draws.next(12);
        let alias = |draws: &mut Draws| -> Value {
            match draws.next(12) {
                0 => json!("other/s1"),
                1 => json!({"source":format!("s{}", 1 + draws.next(sources)),"quote":"x"}),
                _ => json!(format!("s{}", 1 + draws.next(sources))),
            }
        };
        let link = |draws: &mut Draws| -> Value {
            match draws.next(10) {
                0 => json!("other/k1"),
                1 => json!(format!("doc/k{}", draws.next(keys))),
                2 if draws.next(3) == 0 => json!("missing"),
                _ => json!(format!("k{}", draws.next(keys))),
            }
        };
        let items: Vec<_> = (0..keys)
            .map(|k| {
                let mut item = json!({"key":format!("k{k}"),"statement":"s","value":{"kind":"text","text":"t"},
                    "evidence":(0..1 + draws.next(2)).map(|_| alias(draws)).collect::<Vec<_>>(),
                    "classification":{"related":(0..draws.next(3)).map(|_| link(draws)).collect::<Vec<_>>()}});
                if draws.next(3) == 0 {
                    item["classification"]["requirements"] =
                        json!((0..draws.next(2)).map(|_| link(draws)).collect::<Vec<_>>());
                }
                if draws.next(4) == 0 {
                    item["condition"] = json!({"basis":"stated","text":"c","evidence":[alias(draws)]});
                }
                item
            })
            .collect();
        let exclusions: Vec<_> = (0..draws.next(4))
            .map(|_| json!({"evidence":alias(draws),"reason":"r"}))
            .collect();
        let base = json!({"document":"doc","packet":"p","items":items,"exclusions":exclusions,
            "modules":{"app":"Application"},"open_issues":["issue"]});
        let tables = 1 + draws.next(4);
        let mut rows: Vec<_> = (0..sources)
            .map(|i| {
                json!([
                    format!("s{}", i + 1),
                    draws.next(tables),
                    "A1",
                    "value",
                    "text",
                    "General"
                ])
            })
            .collect();
        if draws.next(4) == 0 {
            rows.push(json!([
                "s1",
                draws.next(tables),
                "A2",
                "value",
                "again",
                "General"
            ]));
        }
        let packet = json!({"document":"doc","packet":"p","warnings":["w"],"sources":{
            "columns":["ref","table","cell_or_location","field","text","number_format"],
            "tables":(0..tables).map(|_| json!({"document":"doc","sheet":(["A","B"][draws.next(2)])})).collect::<Vec<_>>(),
            "rows":rows}});
        (base, packet)
    }

    fn outcome<T: PartialEq + std::fmt::Debug>(
        result: Result<T>,
    ) -> std::result::Result<T, String> {
        result.map_err(|error| format!("{error:#}"))
    }

    #[test]
    fn shared_scopes_contexts_and_checks_match_reading_the_reply_for_each() {
        let mut draws = Draws(0x2545_f491_4f6c_dd1d);
        let mut applied = [[0; 2]; 2];
        for _ in 0..300 {
            let (base, packet) = linked(&mut draws);
            let keys = arr(&base["items"]).unwrap().len();
            let items = Items::new(&base).unwrap();
            let evidence = Evidence::new(&base, &packet, &BTreeSet::new()).unwrap();
            let checked = Base::new(base.clone());
            // A region plan covering every source, in a few regions.
            let refs: Vec<String> = arr(&packet["sources"]["rows"])
                .unwrap()
                .iter()
                .map(|r| r[0].as_str().unwrap().to_owned())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            let regions: Vec<Value> = refs
                .chunks(1 + draws.next(4))
                .map(|chunk| json!({"sources":chunk,"context":[refs[0]]}))
                .collect();
            for _ in 0..6 {
                let chosen: BTreeSet<String> = (0..1 + draws.next(3))
                    .map(|_| format!("k{}", draws.next(keys)))
                    .collect();
                let mut scope = items.scope(&chosen, json!(["finding"])).unwrap();
                assert_eq!(
                    scope,
                    scanned_prepare(&base, &chosen, json!(["finding"])).unwrap()
                );
                if draws.next(2) == 0 {
                    scope["regions"] = json!(regions);
                }
                let tables = arr(&packet["sources"]["tables"]).unwrap().len();
                let extra: BTreeSet<usize> =
                    (0..draws.next(2)).map(|_| draws.next(tables)).collect();
                let expected = outcome(scanned_context(&base, &scope, &packet, &extra));
                assert_eq!(outcome(context(&base, &scope, &packet, &extra)), expected);
                if extra.is_empty() {
                    assert_eq!(outcome(evidence.context(&scope, &extra)), expected);
                }
                let changes: Vec<_> = (0..draws.next(3))
                    .map(|_| {
                        let key = format!("k{}", draws.next(keys + 1));
                        let set = match draws.next(5) {
                            0 => json!({"statement":"changed"}),
                            1 => json!({"/classification/related":["missing"]}),
                            2 => json!({"/value/text":"t2","statement":"both"}),
                            3 => json!({"/value/unit":"ms"}),
                            _ => json!({"classification":{"related":[format!("k{}", draws.next(keys))]}}),
                        };
                        json!({"key":key,"set":set})
                    })
                    .collect();
                let patch = json!({"base":scope["base"],"changes":changes});
                let expected = outcome(scanned_apply(&base, &scope, &patch));
                assert_eq!(outcome(apply(&base, &scope, &patch)), expected);
                let fast = checked.clean.is_some();
                applied[usize::from(fast)][usize::from(expected.is_ok())] += 1;
                assert_eq!(outcome(checked.check(&scope, &patch)), expected.map(drop));
            }
        }
        // Patches accepted and refused, checked against clean replies and not.
        assert!(applied.iter().flatten().all(|n| *n > 0), "{applied:?}");
    }

    // Scopes, contexts and patches as they were made before the reply and
    // packet were read once for all of them. Kept as the reference.
    fn scanned_prepare(base: &Value, keys: &BTreeSet<String>, findings: Value) -> Result<Value> {
        let mut items = BTreeMap::new();
        for item in arr(&base["items"])? {
            ensure!(
                items.insert(s(&item["key"])?.to_owned(), item).is_none(),
                "duplicate item key"
            );
        }
        ensure!(
            !keys.is_empty() && keys.iter().all(|k| items.contains_key(k)),
            "existing target keys required"
        );
        let prefix = format!("{}/", s(&base["document"])?);
        let mut related = keys.clone();
        for (key, item) in &items {
            let local: BTreeSet<String> = links(item)?
                .iter()
                .map(|r| r.strip_prefix(&prefix).unwrap_or(r).to_owned())
                .collect();
            if keys.contains(key) {
                related.extend(local.iter().filter(|r| items.contains_key(*r)).cloned());
            }
            if !local.is_disjoint(keys) {
                related.insert(key.clone());
            }
        }
        let mut external = BTreeSet::new();
        for k in &related {
            external.extend(
                links(items[k])?
                    .into_iter()
                    .filter(|r| r.contains('/') && !r.starts_with(&prefix)),
            );
        }
        Ok(
            json!({"base":fingerprint(base),"document":base["document"],"allowed_keys":keys,"findings":findings,
            "modules":base.get("modules").cloned().unwrap_or(json!({})),"open_issues":optional(&base["open_issues"])?,
            "items":keys.iter().map(|k| items[k]).collect::<Vec<_>>(),
            "related_items":related.difference(keys).map(|k| items[k]).collect::<Vec<_>>(),"required_external_items":external}),
        )
    }
    fn scanned_apply(base: &Value, scope: &Value, patch: &Value) -> Result<Value> {
        ensure!(
            obj(patch)?.len() == 2
                && patch["base"] == fingerprint(base)
                && scope["base"] == patch["base"],
            "stale base or unexpected patch fields"
        );
        let allowed = strings(&scope["allowed_keys"])?;
        let mut result = base.clone();
        let items = result["items"].as_array_mut().context("items required")?;
        let mut known = BTreeSet::new();
        for item in items.iter() {
            ensure!(
                known.insert(s(&item["key"])?.to_owned()),
                "duplicate item key"
            );
        }
        let mut seen = BTreeSet::new();
        for change in arr(&patch["changes"])? {
            let key = s(&change["key"])?;
            ensure!(
                obj(change)?.len() == 2 && allowed.contains(key) && seen.insert(key),
                "duplicate or out-of-scope change"
            );
            let edits = obj(&change["set"])?;
            ensure!(
                !edits.is_empty() && edits.keys().all(|k| allowed_path(k)),
                "unsupported field edit"
            );
            let item = items
                .iter_mut()
                .find(|i| i["key"] == key)
                .context("missing target")?;
            for (field, value) in edits {
                if field.starts_with('/') {
                    let parent = field.rsplit_once('/').unwrap().0;
                    ensure!(
                        !edits.contains_key(parent.trim_start_matches('/')),
                        "overlapping parent/leaf edits"
                    );
                    // Only known leaves are allowed; changing a tagged type requires full replacement.
                    if matches!(
                        field.as_str(),
                        "/condition/reason" | "/classification/reason"
                    ) {
                        item.pointer_mut(parent)
                            .and_then(Value::as_object_mut)
                            .context("reason requires an existing parent object")?
                            .insert("reason".into(), value.clone());
                        continue;
                    }
                    let slot = item.pointer_mut(field).context(
                        "leaf does not exist; replace the complete object to change its type",
                    )?;
                    *slot = value.clone();
                } else {
                    item.as_object_mut()
                        .context("invalid item")?
                        .insert(field.clone(), value.clone());
                }
            }
        }
        let prefix = format!("{}/", s(&base["document"])?);
        for item in items {
            for reference in links(item)? {
                let local = reference.strip_prefix(&prefix).unwrap_or(&reference);
                ensure!(
                    local.contains('/') || known.contains(local),
                    "dangling local relationship"
                );
            }
        }
        Ok(result)
    }

    fn scanned_context(
        base: &Value,
        scope: &Value,
        evidence: &Value,
        extra: &BTreeSet<usize>,
    ) -> Result<Value> {
        ensure!(
            base["packet"] == evidence["packet"] && base["document"] == evidence["document"],
            "evidence does not match reply"
        );
        let source = &evidence["sources"];
        let columns = arr(&source["columns"])?;
        let rc = columns
            .iter()
            .position(|c| c == "ref")
            .context("missing ref")?;
        let tc = columns
            .iter()
            .position(|c| c == "table")
            .context("missing table")?;
        let rows = arr(&source["rows"])?;
        let tables = arr(&source["tables"])?;
        let mut by_ref = BTreeMap::new();
        for row in rows {
            by_ref.insert(
                s(&row[rc])?.to_owned(),
                row[tc].as_u64().context("invalid table")? as usize,
            );
        }
        ensure!(
            by_ref.values().all(|i| *i < tables.len()) && extra.iter().all(|i| *i < tables.len()),
            "unknown requested table"
        );
        let prefix = format!("{}/", s(&base["document"])?);
        let mut local = BTreeSet::new();
        for item in arr(&scope["items"])?
            .iter()
            .chain(arr(&scope["related_items"])?)
        {
            for r in refs(item)? {
                local.insert(r.strip_prefix(&prefix).unwrap_or(&r).to_owned());
            }
        }
        let external: BTreeSet<_> = local.iter().filter(|r| r.contains('/')).cloned().collect();
        ensure!(
            local
                .iter()
                .all(|r| external.contains(r) || by_ref.contains_key(r)),
            "unknown source reference"
        );
        let mut chosen = extra.clone();
        chosen.extend(local.iter().filter_map(|r| by_ref.get(r)).copied());
        if chosen.is_empty() {
            chosen.extend(0..tables.len());
        }
        let same_sheet = |a: usize, b: usize| {
            tables[a]["document"] == tables[b]["document"]
                && tables[a]["sheet"] == tables[b]["sheet"]
        };
        chosen = (0..tables.len())
            .filter(|i| chosen.iter().any(|j| same_sheet(*i, *j)))
            .collect();
        let mut selected: BTreeSet<String> = by_ref.keys().cloned().collect();
        let regions = optional(&scope["regions"])?;
        if !regions.is_empty() {
            selected.clear();
            for region in regions {
                let sources = strings(&region["sources"])?;
                if !sources.is_disjoint(&local) {
                    selected.extend(sources);
                    selected.extend(strings(&region["context"])?);
                }
            }
            selected.extend(
                by_ref
                    .iter()
                    .filter(|(_, i)| extra.iter().any(|j| same_sheet(**i, *j)))
                    .map(|(r, _)| r.clone()),
            );
            ensure!(
                !selected.is_empty() && local.difference(&external).all(|r| selected.contains(r)),
                "region does not cover repair evidence"
            );
            chosen = selected
                .iter()
                .filter_map(|r| by_ref.get(r))
                .copied()
                .collect();
        }
        let filtered: Vec<Value> = rows
            .iter()
            .filter(|r| {
                chosen.contains(&(r[tc].as_u64().unwrap() as usize))
                    && selected.contains(r[rc].as_str().unwrap())
            })
            .cloned()
            .collect();
        let included: BTreeSet<String> = filtered
            .iter()
            .map(|r| r[rc].as_str().unwrap().to_owned())
            .collect();
        // Each table's rows counted, and whether one is left out, in one pass
        // over the rows rather than one for each table.
        let mut counts = vec![(0usize, false); tables.len()];
        for r in rows.iter() {
            if let Some(i) = r[tc].as_u64().map(|i| i as usize)
                && let Some((count, left)) = counts.get_mut(i)
            {
                *count += 1;
                *left |= !chosen.contains(&i) || !selected.contains(r[rc].as_str().unwrap());
            }
        }
        let available: Vec<Value> = counts
            .into_iter()
            .enumerate()
            .filter(|(_, (_, left))| *left)
            .map(|(i, (count, _))| available_table(&tables[i], i, count))
            .collect();
        let mut exclusions = Vec::new();
        for e in optional(&base["exclusions"])? {
            if refs(&json!({"evidence":[e["evidence"]]}))?
                .iter()
                .any(|r| included.contains(r.strip_prefix(&prefix).unwrap_or(r)))
            {
                exclusions.push(e);
            }
        }
        let rows: Vec<&Value> = filtered.iter().collect();
        let sources = shown_sources(source, &ShownTables::new(), &rows, &chosen)?;
        Ok(
            json!({"document":base["document"],"packet":base["packet"],"sources":sources,"warnings":optional(&evidence["warnings"])?,
            "included_tables":chosen,"expanded_tables":extra,"available_tables":available,"required_external_sources":external,"exclusions":exclusions}),
        )
    }
}
