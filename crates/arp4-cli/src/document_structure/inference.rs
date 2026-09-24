//! Deterministic layout hypotheses; never used to change source cells.
use super::*;
use crate::excel::{column_name, coordinate};

#[derive(Clone)]
struct Cell<'a> {
    source: &'a Value,
    left: u32,
    top: u32,
    right: u32,
    bottom: u32,
}
impl Cell<'_> {
    fn nonempty(&self) -> bool {
        !self.source["value"].is_null() && self.source["value"] != ""
    }
    fn bold(&self) -> bool {
        self.source["style"]["bold"] == true
    }
}

fn root(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

fn touching(a: &Cell<'_>, b: &Cell<'_>) -> bool {
    // Corner-only contacts do not join two tables.
    (a.left <= b.right
        && b.left <= a.right
        && a.top <= b.bottom.saturating_add(1)
        && b.top <= a.bottom.saturating_add(1))
        || (a.top <= b.bottom
            && b.top <= a.bottom
            && a.left <= b.right.saturating_add(1)
            && b.left <= a.right.saturating_add(1))
}

pub(super) fn elements(extraction: &Value) -> Result<Vec<Value>> {
    let mut output = vec![];
    for (sheet_index, sheet) in array(&extraction["sheets"])?.iter().enumerate() {
        let mut spans = BTreeMap::new();
        for m in array(&sheet["merges"])? {
            let (a, b) = string(m)?.split_once(':').context("invalid merge")?;
            spans.insert(a, coordinate(b)?);
        }
        let mut cells = vec![];
        for c in array(&sheet["cells"])? {
            let (left, top) = coordinate(string(&c["address"])?)?;
            let (right, bottom) = spans
                .get(string(&c["address"])?)
                .copied()
                .unwrap_or((left, top));
            cells.push(Cell {
                source: c,
                left,
                top,
                right,
                bottom,
            });
        }
        cells.sort_by_key(|c| (c.top, c.left));
        let mut assigned = BTreeSet::new();
        let mut groups = vec![];
        // Explicit table boundaries have precedence over layout heuristics.
        if let Some(tables) = sheet["tables"].as_array() {
            for t in tables {
                let bounds = range(string(&t["range"])?)?;
                let indices: Vec<_> = cells
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| contains(bounds, (c.left, c.top)))
                    .map(|(i, _)| i)
                    .collect();
                ensure!(
                    indices.iter().all(|i| assigned.insert(*i)),
                    "overlapping Excel table definitions"
                );
                if !indices.is_empty() {
                    groups.push((
                        indices,
                        Some((bounds.0.1, t["header_rows"].as_u64().unwrap_or(0) as u32)),
                    ));
                }
            }
        }
        let mut parent: Vec<_> = (0..cells.len()).collect();
        let mut active: Vec<usize> = vec![];
        let mut budget = 2_000_000usize;
        let mut exhausted = false;
        for (i, c) in cells
            .iter()
            .enumerate()
            .filter(|(i, _)| !assigned.contains(i))
        {
            active.retain(|j| cells[*j].bottom.saturating_add(1) >= c.top);
            for j in &active {
                if budget == 0 {
                    exhausted = true;
                    break;
                }
                budget -= 1;
                if touching(c, &cells[*j]) {
                    let r = root(&mut parent, *j);
                    let s = root(&mut parent, i);
                    parent[s] = r;
                }
            }
            if exhausted {
                break;
            }
            active.push(i);
        }
        let mut components: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for i in 0..cells.len() {
            if !assigned.contains(&i) {
                components
                    .entry(if exhausted { i } else { root(&mut parent, i) })
                    .or_default()
                    .push(i);
            }
        }
        let mut remaining: Vec<_> = components.into_values().collect();
        // Attach a standalone caption across a single blank row, except the document heading.
        let captions: Vec<_> = remaining
            .iter()
            .enumerate()
            .filter(|(_, g)| g.len() == 1)
            .map(|(i, g)| (i, g[0]))
            .collect();
        for (slot, id) in captions {
            if remaining[slot].is_empty() {
                continue;
            }
            let caption = &cells[id];
            if caption.top <= 2 || !caption.bold() || caption.left == caption.right {
                continue;
            }
            let candidates: Vec<_> = remaining
                .iter()
                .enumerate()
                .filter(|(i, g)| {
                    *i != slot
                        && g.len() > 1
                        && g.iter().map(|i| cells[*i].top).min() == Some(caption.bottom + 2)
                        && g.iter().map(|i| cells[*i].left).min() == Some(caption.left)
                        && g.iter().map(|i| cells[*i].right).max() == Some(caption.right)
                })
                .map(|(i, _)| i)
                .collect();
            if let [target] = candidates.as_slice() {
                remaining[*target].push(id);
                remaining[slot].clear();
            }
        }
        groups.extend(
            remaining
                .into_iter()
                .filter(|g| !g.is_empty())
                .map(|g| (g, None)),
        );
        groups.sort_by_key(|(g, _)| {
            g.iter()
                .map(|i| (cells[*i].top, cells[*i].left))
                .min()
                .unwrap()
        });
        let mut leftovers = vec![];
        for (indices, explicit) in groups {
            let mut group: Vec<_> = indices.iter().map(|i| &cells[*i]).collect();
            group.sort_by_key(|c| (c.top, c.left));
            let left = group.iter().map(|c| c.left).min().unwrap();
            let right = group.iter().map(|c| c.right).max().unwrap();
            let top = group.iter().map(|c| c.top).min().unwrap();
            let bottom = group.iter().map(|c| c.bottom).max().unwrap();
            let nonempty: Vec<_> = group.iter().copied().filter(|c| c.nonempty()).collect();
            let mut row_counts = BTreeMap::<u32, usize>::new();
            for c in &nonempty {
                *row_counts.entry(c.top).or_default() += 1;
            }
            let caption = explicit.is_none()
                && group.iter().filter(|c| c.top == top).count() == 1
                && group[0].left == left
                && group[0].right == right
                && group[0].bold();
            let multi_rows = row_counts.values().filter(|v| **v >= 2).count();
            let is_table = explicit.is_some()
                || (!exhausted
                    && nonempty.len() >= 3
                    && (multi_rows >= 2
                        || (caption
                            && multi_rows >= 1
                            && nonempty
                                .iter()
                                .filter(|c| row_counts[&c.top] >= 2)
                                .all(|c| c.source["style"]["border"].as_u64().unwrap_or(0) > 0))));
            if !is_table {
                leftovers.extend(indices);
                continue;
            }
            let header_top = if caption { group[0].bottom + 1 } else { top };
            let mut header_bottom = explicit
                .map(|(top, count)| {
                    top.checked_add(count.saturating_sub(1))
                        .context("table header range overflow")
                })
                .transpose()?;
            if explicit.is_some_and(|(_, count)| count == 0) {
                header_bottom = None;
            }
            if explicit.is_none() {
                let first: Vec<_> = group
                    .iter()
                    .filter(|c| c.top == header_top && c.nonempty())
                    .collect();
                if first.len() >= 2 && first.iter().all(|c| c.bold()) {
                    let mut end = first.iter().map(|c| c.bottom).max().unwrap();
                    loop {
                        let next: Vec<_> = group
                            .iter()
                            .filter(|c| c.top == end + 1 && c.nonempty())
                            .collect();
                        if next.len() < 2 || !next.iter().all(|c| c.bold()) {
                            break;
                        }
                        end = next.iter().map(|c| c.bottom).max().unwrap();
                    }
                    header_bottom = Some(end);
                }
            }
            let headers: Vec<_> = group
                .iter()
                .copied()
                .filter(|c| header_bottom.is_some_and(|end| c.top >= header_top && c.top <= end))
                .collect();
            let stub_end = headers
                .iter()
                .filter(|c| {
                    c.top == header_top && header_bottom == Some(c.bottom) && c.bottom > c.top
                })
                .map(|c| c.right)
                .max();
            let row_header_limit = stub_end.unwrap_or_else(|| {
                group
                    .iter()
                    .filter(|c| c.left == left)
                    .map(|c| c.right)
                    .min()
                    .unwrap_or(left)
            });
            let id_for = |c: &Cell| {
                format!(
                    "s{}-{}",
                    sheet_index + 1,
                    c.source["address"].as_str().unwrap()
                )
            };
            let role = |c: &Cell| {
                if caption && c.top == top {
                    "text"
                } else if header_bottom.is_some_and(|end| c.top >= header_top && c.top <= end) {
                    "column_header"
                } else if c.right <= row_header_limit && c.source["type"] == "string" {
                    "row_header"
                } else {
                    "data"
                }
            };
            let interpreted:Vec<_>=group.iter().map(|c| {
                let current_role=role(c);
                let links:Vec<_>=group.iter().filter(|h| {
                    if current_role=="text" || h.source["address"]==c.source["address"] {return false;}
                    match role(h) {
                        "column_header"=> h.bottom<c.top && h.left<=c.left && h.right>=c.right,
                        "row_header"=> current_role!="column_header" && h.right<c.left && h.top<=c.top && h.bottom>=c.top,
                        _=>false
                    }
                }).map(|h|id_for(h)).collect();
                json!({"id":id_for(c),"address":c.source["address"],"role":current_role,"headers":links,
                    "text_state":if c.nonempty() || c.source["formula"].is_string() {"read"} else {"empty"}})
            }).collect();
            output.push(json!({"id":format!("sheet-{}-table-{}",sheet_index+1,output.len()+1),"kind":"table","sheet":sheet["name"],"evidence":[],
                "reading":{"method":"parser","actor":"arp4","reason":format!("{}; range {}{}:{}{}. Header links are layout hypotheses requiring review.",if explicit.is_some() {"Explicit Excel table boundary"} else {"Adjacent merged-cell regions with repeated row fields; bold headers and aligned row labels"},column_name(left)?,top,column_name(right)?,bottom)},
                "cells":interpreted}));
        }
        if !leftovers.is_empty() {
            leftovers.sort_unstable();
            let interpreted:Vec<_>=leftovers.iter().map(|i| {
                let c=&cells[*i];
                json!({"id":format!("s{}-{}",sheet_index+1,c.source["address"].as_str().unwrap()),"address":c.source["address"],
                    "role":"unassigned","headers":[],"text_state":if c.nonempty() || c.source["formula"].is_string() {"read"} else {"empty"}})
            }).collect();
            output.push(json!({"id":format!("sheet-{}",sheet_index+1),"kind":"text","sheet":sheet["name"],"cells":interpreted,"evidence":[],
                "reading":{"method":"parser","actor":"arp4","reason":if exhausted {"Layout comparison budget reached; roles remain unassigned."} else {"Captured cells outside confident table candidates; roles remain unassigned."}}}));
        }
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn cell(address: &str, value: &str) -> Value {
        json!({"address":address,"value":value,"type":"string","formula":null,"style":{"bold":false,"fill":0,"border":0}})
    }
    #[test]
    fn explicit_table_takes_precedence_and_honors_no_header() {
        let extraction = json!({"sheets":[{"name":"S","merges":[],"tables":[{"range":"B2:C3","header_rows":1}],
            "cells":[cell("A2","outside"),cell("B2","Key"),cell("C2","Value"),cell("B3","x"),cell("C3","y")]}]});
        let result = elements(&extraction).unwrap();
        let table = result.iter().find(|e| e["kind"] == "table").unwrap();
        assert_eq!(table["cells"].as_array().unwrap().len(), 4);
        let data = table["cells"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["address"] == "C3")
            .unwrap();
        assert!(
            data["headers"]
                .as_array()
                .unwrap()
                .contains(&json!("s1-C2"))
        );
        let mut no_header = extraction.clone();
        no_header["sheets"][0]["tables"][0]["header_rows"] = json!(0);
        let result = elements(&no_header).unwrap();
        assert!(
            result
                .iter()
                .flat_map(|e| e["cells"].as_array().unwrap())
                .all(|c| c["role"] != "column_header")
        );
    }
    #[test]
    fn diagonal_tables_stay_separate_and_notes_remain_unassigned() {
        let mut cells = vec![];
        for address in ["A1", "B1", "A2", "B2", "C3", "D3", "C4", "D4", "J10"] {
            cells.push(cell(address, "text"));
        }
        let result = elements(&json!({"sheets":[{"name":"S","merges":[],"cells":cells}]})).unwrap();
        assert_eq!(result.iter().filter(|e| e["kind"] == "table").count(), 2);
        let note = result.iter().find(|e| e["kind"] == "text").unwrap();
        assert_eq!(note["cells"][0]["address"], "J10");
        assert_eq!(note["cells"][0]["role"], "unassigned");
    }
}
