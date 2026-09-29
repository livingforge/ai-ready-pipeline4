//! Bounded field repairs. Evidence selection never guesses table boundaries.
use super::{arr, obj, s};
use crate::data::hash;
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

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
pub fn prepare(base: &Value, keys: &BTreeSet<String>, findings: Value) -> Result<Value> {
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
        tables[a]["document"] == tables[b]["document"] && tables[a]["sheet"] == tables[b]["sheet"]
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
    let available: Vec<Value> = (0..tables.len())
        .filter(|i| {
            rows.iter().any(|r| {
                r[tc] == *i && (!chosen.contains(i) || !selected.contains(r[rc].as_str().unwrap()))
            })
        })
        .map(|i| json!({"table":i,"source_count":rows.iter().filter(|r| r[tc] == i).count()}))
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
    let mut sources = source.clone();
    sources["rows"] = json!(filtered);
    Ok(
        json!({"document":base["document"],"packet":base["packet"],"sources":sources,"warnings":optional(&evidence["warnings"])?,
        "included_tables":chosen,"expanded_tables":extra,"available_tables":available,"required_external_sources":external,"exclusions":exclusions}),
    )
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
            "Correct only selected fields using supplied evidence and findings. Return JSON {{\"base\":\"copy base reference\",\"changes\":[{{\"key\":\"target key\",\"set\":{{\"field\":\"replacement\"}}}}]}}. Omit unchanged fields/items; use supported leaf paths or replace nested objects when changing their type. Do not add/delete/rename items or fabricate evidence. Empty changes remain unresolved. Source aliases and table indexes retain original numbering. With regions only declared sources/shared context are supplied; otherwise full sheets. For missing sheets return ONLY {{\"base\":\"copy base reference\",\"request_tables\":[0],\"reason\":\"missing context\"}} using available_tables. Never infer missing external evidence. Allowed fields: {}",
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
        let make_scope = |keys: &BTreeSet<String>| -> Result<Value> {
            let findings: Vec<_> = keys.iter().flat_map(|k| keyed[k].clone()).collect();
            let mut scope = prepare(base, keys, json!(findings))?;
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
            let ctx = context(base, &scope, packet, &BTreeSet::new())?;
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
            let ctx = context(base, &scope, packet, &BTreeSet::new())?;
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
}
