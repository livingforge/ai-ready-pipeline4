//! Reuse decisions only when source topology and shared context are unchanged.
use super::*;
use std::collections::{HashMap, HashSet};

fn references(item: &Value) -> BTreeSet<String> {
    let mut selectors: Vec<&Value> = item["evidence"].as_array().into_iter().flatten().collect();
    selectors.extend(
        item["condition"]["evidence"]
            .as_array()
            .into_iter()
            .flatten(),
    );
    for field in ["title", "description", "cells", "notes"] {
        selectors.extend(item["value"][field].as_array().into_iter().flatten());
    }
    for key in ["basis", "unit_basis", "semantics_basis"] {
        if let Some(value) = item["value"].get(key) {
            selectors.push(value);
        }
    }
    selectors
        .into_iter()
        .filter_map(|v| {
            (if v.is_string() { v } else { &v["source"] })
                .as_str()
                .map(str::to_owned)
        })
        .collect()
}

pub(super) fn external_changed(
    base: &Value,
    before: &crate::specifications::Input,
    after: &crate::specifications::Input,
) -> Result<bool> {
    let owner = s(&base["document"])?;
    let mut documents = BTreeSet::new();
    for item in arr(&base["items"])? {
        for reference in references(item) {
            if let Some((doc, _)) = reference.rsplit_once('/')
                && doc != owner
            {
                documents.insert(doc.to_owned());
            }
        }
    }
    for doc in documents {
        if !after.sources.iter().any(|source| source.document == doc)
            || crate::semantic::packet(before, &doc)?["packet"]
                != crate::semantic::packet(after, &doc)?["packet"]
        {
            return Ok(true);
        }
    }
    Ok(false)
}

// Ancestor sections and the preamble are shared context. Unstructured text
// and Office content retain whole-sheet scope.
fn group(row: &Value) -> String {
    json!([row[1], row.get(6).and_then(|p| p["headings"].as_array())]).to_string()
}

fn ancestors(row: &Value) -> Vec<String> {
    row.get(6)
        .and_then(|p| p["headings"].as_array())
        .map(|headings| {
            (0..headings.len())
                .map(|length| json!([row[1], &headings[..length]]).to_string())
                .collect()
        })
        .unwrap_or_default()
}

pub(super) fn review_regions(packet: &Value) -> Result<Vec<Value>> {
    let rows = arr(&packet["sources"]["rows"])?;
    if !rows.iter().any(|r| {
        r.get(6)
            .is_some_and(|p| p["headings"].as_array().is_some_and(|h| !h.is_empty()))
    }) {
        return Ok(vec![]);
    }
    let shape = Shape::new(rows);
    let mut groups: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    // The rows with each reference: a group's parents are those of every row
    // with one of its references.
    let mut by_ref = HashMap::<String, Vec<usize>>::new();
    for (i, key) in shape.keys.iter().enumerate() {
        groups.entry(key).or_default().push(i);
        by_ref
            .entry(crate::data::value_key(&rows[i][0]))
            .or_default()
            .push(i);
    }
    Ok(groups
        .values()
        .map(|members| {
            let sources: Vec<_> = members.iter().map(|&i| &rows[i][0]).collect();
            let parents: BTreeSet<&str> = sources
                .iter()
                .filter_map(|r| by_ref.get(&crate::data::value_key(r)))
                .flatten()
                .flat_map(|&i| shape.ancestors[i].iter().map(String::as_str))
                .collect();
            let mut context: Vec<usize> = parents
                .iter()
                .filter_map(|parent| groups.get(parent))
                .flatten()
                .copied()
                .collect();
            context.sort_unstable();
            let context: Vec<_> = context.into_iter().map(|i| &rows[i][0]).collect();
            json!({"sources":sources,"context":context})
        })
        .collect())
}

/// The section keys of packet rows, worked out once for each row.
struct Shape {
    /// The group of each row.
    keys: Vec<String>,
    /// The groups of each row's ancestor sections.
    ancestors: Vec<Vec<String>>,
}

impl Shape {
    fn new(rows: &[Value]) -> Self {
        Self {
            keys: rows.iter().map(group).collect(),
            ancestors: rows.iter().map(ancestors).collect(),
        }
    }
}

pub(super) fn plan(old: &Value, new: &Value, base: &Value) -> Result<Option<Value>> {
    // Issues are document-scoped; their ownership cannot be safely partitioned.
    if !arr(&base["open_issues"])?.is_empty() {
        return Ok(None);
    }
    let mut old_shape = old.clone();
    let mut new_shape = new.clone();
    for packet in [&mut old_shape, &mut new_shape] {
        for row in packet["sources"]["rows"]
            .as_array_mut()
            .context("missing rows")?
        {
            row[4] = Value::Null;
        }
    }
    if crate::semantic::packet_fingerprint(&old_shape)
        != crate::semantic::packet_fingerprint(&new_shape)
    {
        return Ok(None);
    }
    let rows = arr(&new["sources"]["rows"])?;
    let old_rows = arr(&old["sources"]["rows"])?;
    let shape = Shape::new(rows);
    let mut groups: BTreeSet<&str> = shape
        .keys
        .iter()
        .zip(rows.iter().zip(old_rows))
        .filter(|(_, (a, b))| a[4] != b[4])
        .map(|(key, _)| key.as_str())
        .collect();
    if groups.is_empty() {
        return Ok(None);
    }
    // A changed preamble can change the interpretation of every section.
    if rows.iter().zip(&shape.keys).any(|(r, key)| {
        groups.contains(key.as_str())
            && r.get(6)
                .is_some_and(|p| p["headings"].as_array().is_some_and(Vec::is_empty))
    }) {
        return Ok(None);
    }
    let known: BTreeSet<_> = rows
        .iter()
        .map(|r| s(&r[0]).map(str::to_owned))
        .collect::<Result<_>>()?;
    let items = arr(&base["items"])?;
    let references: Vec<BTreeSet<String>> = items.iter().map(references).collect();
    if references.iter().any(|refs| !refs.is_subset(&known)) {
        return Ok(None);
    }
    // The groups reached from the changed ones: those nested in a group, and
    // those of the sources an item cites along with a source of a group.
    // Visiting each group and item once reaches what repeating both rules
    // until nothing changes reaches.
    let mut members = HashMap::<&str, Vec<usize>>::new();
    let mut children = HashMap::<&str, Vec<usize>>::new();
    let mut by_alias = HashMap::<&str, Vec<usize>>::new();
    for (i, row) in rows.iter().enumerate() {
        members.entry(&shape.keys[i]).or_default().push(i);
        for parent in &shape.ancestors[i] {
            children.entry(parent).or_default().push(i);
        }
        by_alias.entry(s(&row[0])?).or_default().push(i);
    }
    let mut citing = HashMap::<&str, Vec<usize>>::new();
    for (j, refs) in references.iter().enumerate() {
        for reference in refs {
            citing.entry(reference).or_default().push(j);
        }
    }
    let mut cited = vec![false; items.len()];
    let mut pending: Vec<&str> = groups.iter().copied().collect();
    while let Some(key) = pending.pop() {
        let mut reached = Vec::new();
        for &child in children.get(key).into_iter().flatten() {
            reached.push(child);
        }
        for &row in members.get(key).into_iter().flatten() {
            for &j in citing.get(s(&rows[row][0])?).into_iter().flatten() {
                if !std::mem::replace(&mut cited[j], true) {
                    for reference in &references[j] {
                        reached.extend(by_alias.get(reference.as_str()).into_iter().flatten());
                    }
                }
            }
        }
        for i in reached {
            if groups.insert(&shape.keys[i]) {
                pending.push(&shape.keys[i]);
            }
        }
    }
    let targets: Vec<_> = rows
        .iter()
        .zip(&shape.keys)
        .filter(|(_, key)| groups.contains(key.as_str()))
        .map(|(r, _)| r[0].clone())
        .collect();
    if targets.len() == rows.len() {
        return Ok(None);
    }
    let owned: HashSet<&str> = targets.iter().filter_map(Value::as_str).collect();
    let owned = |row: &Value| row[0].as_str().is_some_and(|alias| owned.contains(alias));
    let parents: BTreeSet<&str> = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| owned(row))
        .flat_map(|(i, _)| shape.ancestors[i].iter().map(String::as_str))
        .collect();
    let context: Vec<_> = rows
        .iter()
        .zip(&shape.keys)
        .filter(|(parent, key)| !owned(parent) && parents.contains(key.as_str()))
        .map(|(r, _)| r[0].clone())
        .collect();
    Ok(Some(json!({"sources":targets,"context":context})))
}

pub(super) fn merge(base: &Value, reply: &Value, packet: &Value) -> Result<Value> {
    let targets: BTreeSet<_> = arr(&packet["scope"]["sources"])?
        .iter()
        .map(|v| s(v).map(str::to_owned))
        .collect::<Result<_>>()?;
    let mut merged = base.clone();
    merged["packet"] = packet["parent_packet"].clone();
    merged["items"]
        .as_array_mut()
        .unwrap()
        .retain(|item| references(item).is_disjoint(&targets));
    let retained: BTreeSet<_> = arr(&merged["items"])?
        .iter()
        .map(|i| s(&i["key"]).map(str::to_owned))
        .collect::<Result<_>>()?;
    for item in arr(&reply["items"])? {
        ensure!(
            !retained.contains(s(&item["key"])?),
            "incremental key collides with retained item"
        );
        let mut item = item.clone();
        for key in ["requirements", "related"] {
            item["classification"].as_object_mut().unwrap().remove(key);
        }
        merged["items"].as_array_mut().unwrap().push(item);
    }
    merged["exclusions"].as_array_mut().unwrap().retain(|e| {
        let v = &e["evidence"];
        !targets.contains(if v.is_string() {
            v.as_str().unwrap()
        } else {
            v["source"].as_str().unwrap()
        })
    });
    merged["exclusions"]
        .as_array_mut()
        .unwrap()
        .extend(arr(&reply["exclusions"])?.clone());
    for (slug, name) in obj(&reply["modules"])? {
        ensure!(
            merged["modules"].get(slug).is_none_or(|v| v == name),
            "module vocabulary conflict"
        );
        merged["modules"][slug] = name.clone();
    }
    merged["open_issues"] = reply["open_issues"].clone();
    Ok(merged)
}

impl Workflow {
    pub(super) fn incremental_task(&self, doc: &str, packet: &Value) -> Result<Option<Task>> {
        let Some(plan) = self.state.incremental.get(doc) else {
            return Ok(None);
        };
        let base = self.store.json(s(&plan["base"])?)?;
        let scoped = extraction::scoped_packet(packet, &plan["region"], &self.state.modules)?;
        let targets: BTreeSet<_> = arr(&plan["region"]["sources"])?
            .iter()
            .map(|v| s(v).map(str::to_owned))
            .collect::<Result<_>>()?;
        let previous: Vec<_> = arr(&base["items"])?
            .iter()
            .filter(|i| !references(i).is_disjoint(&targets))
            .collect();
        let reserved: Vec<_> = arr(&base["items"])?
            .iter()
            .filter(|i| references(i).is_disjoint(&targets))
            .map(|i| &i["key"])
            .collect();
        let text = format!(
            "{}\nRe-extract the entire owned scope from the current sources. Previous items are context, not authoritative facts. Preserve an existing key only when its claim is retained; use new keys for new claims or splits. Omit relationships for the linking stage. Reserved keys: {}\nPrevious items: {}",
            extraction::task_text(&scoped),
            json!(reserved),
            json!(previous)
        );
        if text.len() > self.state.extract_max_bytes {
            return Ok(None);
        }
        Ok(Some(self.task(Stage::Extract, text.as_bytes(), json!({"document":doc,"packet":self.store.put_json(&scoped)?,"base":plan["base"],"context":self.store.put_json(&json!({"previous_items":previous,"reserved_keys":reserved}))?,"incremental":true}))?))
    }

    pub(super) fn repair_missing_links(&self, next: &mut Value) -> Result<()> {
        let mut replies = BTreeMap::new();
        let mut known = BTreeSet::new();
        for (doc, digest) in obj(next)? {
            let reply = self.store.json(s(digest)?)?;
            for item in arr(&reply["items"])? {
                known.insert(format!("{doc}/{}", s(&item["key"])?));
            }
            replies.insert(doc.clone(), reply);
        }
        for (doc, mut reply) in replies {
            for item in reply["items"].as_array_mut().unwrap() {
                for field in ["requirements", "related"] {
                    let missing = item["classification"][field]
                        .as_array()
                        .is_some_and(|links| {
                            links.iter().any(|v| {
                                let id = v.as_str().unwrap_or("");
                                !known.contains(&if id.contains('/') {
                                    id.to_owned()
                                } else {
                                    format!("{doc}/{id}")
                                })
                            })
                        });
                    if missing {
                        item["classification"]
                            .as_object_mut()
                            .unwrap()
                            .remove(field);
                    }
                }
            }
            next[&doc] = json!(self.store.put_json(&reply)?);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet() -> Value {
        let rows: Vec<_> = [("# Title",vec!["Title"]),("## A",vec!["Title","A"]),("Alpha",vec!["Title","A"]),("## B",vec!["Title","B"]),("Beta",vec!["Title","B"])].iter().enumerate().map(|(i,(text,headings))| {
            json!([format!("s{}",i+1),0,format!("A{}",i+1),"value",text,"General",{"kind":if text.starts_with('#') {"heading"} else {"paragraph"},"headings":headings,"columns":[],"line_start":i+1,"line_end":i+1,"byte_start":i*10,"byte_end":i*10+text.len()}])
        }).collect();
        json!({"document":"doc","sources":{"columns":["ref","table","cell_or_location","field","text","number_format","position"],"tables":[{"document":"doc","sheet":"text","merges":[]}],"rows":rows},"warnings":[],"contract":"test"})
    }

    #[test]
    fn sections_keep_ancestors_and_expand_cross_section_evidence() {
        let old = packet();
        let mut new = old.clone();
        new["sources"]["rows"][2][4] = json!("Changed");
        new["sources"]["rows"][4][6]["byte_start"] = json!(999);
        let base = json!({"items":[{"evidence":["s3"]},{"evidence":["s5"]}],"open_issues":[]});
        assert_eq!(
            plan(&old, &new, &base).unwrap().unwrap(),
            json!({"sources":["s2","s3"],"context":["s1"]})
        );
        let regions = review_regions(&new).unwrap();
        assert!(regions.contains(&json!({"sources":["s4","s5"],"context":["s1"]})));
        let base = json!({"items":[{"evidence":["s3","s5"]}],"open_issues":[]});
        assert_eq!(
            plan(&old, &new, &base).unwrap().unwrap()["sources"],
            json!(["s2", "s3", "s4", "s5"])
        );
    }

    #[test]
    fn structural_and_shared_context_changes_do_not_reuse_partial_claims() {
        let old = packet();
        let base = json!({"items":[],"open_issues":[]});
        for pointer in [
            "/sources/rows/0/4",
            "/sources/rows/2/6/headings/1",
            "/sources/rows/2/5",
            "/contract",
        ] {
            let mut new = old.clone();
            *new.pointer_mut(pointer).unwrap() = json!("Changed");
            assert!(plan(&old, &new, &base).unwrap().is_none(), "{pointer}");
        }
        let mut new = old.clone();
        new["sources"]["rows"].as_array_mut().unwrap().remove(2);
        assert!(plan(&old, &new, &base).unwrap().is_none());
    }

    #[test]
    fn partial_merge_rejects_overwriting_retained_keys() {
        let base = json!({"packet":"old","document":"doc","items":[{"key":"a","evidence":["s3"]},{"key":"b","evidence":["s5"]}],"exclusions":[],"modules":{},"open_issues":[]});
        let reply = json!({"items":[{"key":"b","evidence":["s3"]}],"exclusions":[],"modules":{},"open_issues":[]});
        let packet = json!({"scope":{"sources":["s2","s3"]},"parent_packet":"new"});
        assert!(
            merge(&base, &reply, &packet)
                .unwrap_err()
                .to_string()
                .contains("collides")
        );
    }

    /// Sections of random depth on two tables, a few changed texts, and items
    /// citing a few sources each, some of them unknown.
    fn sectioned(seed: u64) -> (Value, Value, Value) {
        let mut state = seed;
        let mut next = |below: usize| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % below as u64) as usize
        };
        let count = 3 + next(25);
        let rows: Vec<_> = (0..count)
            .map(|i| {
                let headings: Vec<_> = (0..next(4))
                    .map(|_| Value::from(["H", "I", "J"][next(3)]))
                    .collect();
                let mut position = json!({"kind":"paragraph","headings":headings});
                if next(8) == 0 {
                    position = json!({"kind":"paragraph"});
                }
                json!([
                    format!("s{}", i + 1),
                    next(2),
                    format!("A{}", i + 1),
                    "value",
                    "text",
                    "General",
                    position
                ])
            })
            .collect();
        let old = json!({"document":"doc","sources":{"columns":["ref","table","cell_or_location","field","text","number_format","position"],
            "tables":[{"sheet":"a"},{"sheet":"b"}],"rows":rows},"warnings":[],"contract":"test"});
        let mut new = old.clone();
        for _ in 0..1 + next(3) {
            new["sources"]["rows"][next(count)][4] = json!("changed");
        }
        let items: Vec<_> = (0..next(8))
            .map(|_| {
                let evidence: Vec<_> = (0..1 + next(3))
                    .map(|_| json!(format!("s{}", 1 + next(count + 1))))
                    .collect();
                json!({"evidence":evidence})
            })
            .collect();
        (old, new, json!({"items":items,"open_issues":[]}))
    }

    #[test]
    fn indexed_regions_and_plans_match_scanning_every_row() {
        // Cases with regions and with partial plans, so both are compared.
        let (mut regions, mut planned) = (0, 0);
        for seed in 1..400u64 {
            let (old, new, base) = sectioned(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15));
            let expected = scanned_regions(&new).unwrap();
            assert_eq!(review_regions(&new).unwrap(), expected, "{seed}");
            regions += !expected.is_empty() as usize;
            let expected = scanned_plan(&old, &new, &base).unwrap();
            assert_eq!(plan(&old, &new, &base).unwrap(), expected, "{seed}");
            planned += expected.is_some() as usize;
        }
        assert!(regions > 0 && planned > 0, "{regions} {planned}");
    }

    // The regions and plan as they were found before the rows were indexed,
    // every row filtered for each group and the groups grown until unchanged.
    // Kept as the reference.
    fn scanned_regions(packet: &Value) -> Result<Vec<Value>> {
        let rows = arr(&packet["sources"]["rows"])?;
        if !rows.iter().any(|r| {
            r.get(6)
                .is_some_and(|p| p["headings"].as_array().is_some_and(|h| !h.is_empty()))
        }) {
            return Ok(vec![]);
        }
        let mut groups: BTreeMap<String, Vec<Value>> = BTreeMap::new();
        for row in rows {
            groups.entry(group(row)).or_default().push(row[0].clone());
        }
        Ok(groups
            .into_values()
            .map(|sources| {
                let parents: BTreeSet<_> = rows
                    .iter()
                    .filter(|row| sources.contains(&row[0]))
                    .flat_map(ancestors)
                    .collect();
                let context: Vec<_> = rows
                    .iter()
                    .filter(|parent| parents.contains(&group(parent)))
                    .map(|r| r[0].clone())
                    .collect();
                json!({"sources":sources,"context":context})
            })
            .collect())
    }

    fn scanned_plan(old: &Value, new: &Value, base: &Value) -> Result<Option<Value>> {
        // Issues are document-scoped; their ownership cannot be safely partitioned.
        if !arr(&base["open_issues"])?.is_empty() {
            return Ok(None);
        }
        let mut old_shape = old.clone();
        let mut new_shape = new.clone();
        for packet in [&mut old_shape, &mut new_shape] {
            for row in packet["sources"]["rows"]
                .as_array_mut()
                .context("missing rows")?
            {
                row[4] = Value::Null;
            }
        }
        if crate::semantic::packet_fingerprint(&old_shape)
            != crate::semantic::packet_fingerprint(&new_shape)
        {
            return Ok(None);
        }
        let rows = arr(&new["sources"]["rows"])?;
        let old_rows = arr(&old["sources"]["rows"])?;
        let mut groups: BTreeSet<_> = rows
            .iter()
            .zip(old_rows)
            .filter(|(a, b)| a[4] != b[4])
            .map(|(r, _)| group(r))
            .collect();
        if groups.is_empty() {
            return Ok(None);
        }
        // A changed preamble can change the interpretation of every section.
        if rows.iter().any(|r| {
            groups.contains(&group(r))
                && r.get(6)
                    .is_some_and(|p| p["headings"].as_array().is_some_and(Vec::is_empty))
        }) {
            return Ok(None);
        }
        let known: BTreeSet<_> = rows
            .iter()
            .map(|r| s(&r[0]).map(str::to_owned))
            .collect::<Result<_>>()?;
        for item in arr(&base["items"])? {
            if !references(item).is_subset(&known) {
                return Ok(None);
            }
        }
        loop {
            let before = groups.len();
            let descendants: Vec<_> = rows
                .iter()
                .filter(|child| {
                    ancestors(child)
                        .iter()
                        .any(|parent| groups.contains(parent))
                })
                .map(group)
                .collect();
            groups.extend(descendants);
            let targets: BTreeSet<_> = rows
                .iter()
                .filter(|r| groups.contains(&group(r)))
                .map(|r| r[0].as_str().unwrap().to_owned())
                .collect();
            for item in arr(&base["items"])? {
                let refs = references(item);
                if !refs.is_disjoint(&targets) {
                    groups.extend(
                        rows.iter()
                            .filter(|r| refs.contains(r[0].as_str().unwrap()))
                            .map(group),
                    );
                }
            }
            if groups.len() == before {
                break;
            }
        }
        let targets: Vec<_> = rows
            .iter()
            .filter(|r| groups.contains(&group(r)))
            .map(|r| r[0].clone())
            .collect();
        if targets.len() == rows.len() {
            return Ok(None);
        }
        let parents: BTreeSet<_> = rows
            .iter()
            .filter(|row| targets.contains(&row[0]))
            .flat_map(ancestors)
            .collect();
        let context: Vec<_> = rows
            .iter()
            .filter(|parent| !targets.contains(&parent[0]) && parents.contains(&group(parent)))
            .map(|r| r[0].clone())
            .collect();
        Ok(Some(json!({"sources":targets,"context":context})))
    }
}
