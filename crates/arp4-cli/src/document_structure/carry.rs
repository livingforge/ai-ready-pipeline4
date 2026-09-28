//! Carries an interpretation onto a new version of its original. `documents
//! apply` knows every row and column operation it wrote with its position and
//! count; for an original edited outside ARP, the operations come from
//! aligning the two extractions. Cells, their IDs and header links move with
//! them, rows or columns inserted into a table take the shape of their style
//! row or neighbour, and readings of drawings and images follow what they
//! show. The result is recorded as corrections of the new extraction, so
//! replay reproduces it.
use super::*;
use crate::excel::{Merges, StructuralOperation};
use anyhow::bail;

/// A cell being carried: its interpretation, the address it had in the old
/// extraction (none for a cell an insertion added) and, while it moves, the
/// same fields the interpretation keeps.
struct Carried {
    value: Value,
    origin: Option<String>,
}

fn referenced_regions<'a>(elements: &'a [Value], visuals: &'a [Value]) -> BTreeSet<&'a str> {
    elements
        .iter()
        .chain(visuals.iter())
        .flat_map(|item| item["evidence"].as_array().into_iter().flatten())
        .filter_map(Value::as_str)
        .collect()
}

#[cfg(test)]
mod visual_performance_tests {
    use super::*;
    use std::time::Instant;

    #[test]
    #[ignore = "manual performance measurement"]
    fn benchmark_carry_visuals() {
        let drawings: Vec<_> = (0..800)
            .map(|i| json!({"kind":"picture","text":format!("picture {i}")}))
            .collect();
        let before = json!({"source":{"path":"docs/images.xlsx"},"sheets":[{"name":"S","drawings":drawings}],"assets":[]});
        let structure = json!({"visuals":policy::visuals(&before).unwrap()});
        let mut elapsed = Vec::new();
        for _ in 0..5 {
            let mut reasons = Vec::new();
            let mut affected = Vec::new();
            let started = Instant::now();
            let carried =
                carry_visuals(&structure, &before, &before, &mut reasons, &mut affected).unwrap();
            elapsed.push(started.elapsed().as_secs_f64() * 1000.0);
            assert_eq!(carried.len(), 800);
            assert!(reasons.is_empty() && affected.is_empty());
        }
        elapsed.sort_by(f64::total_cmp);
        eprintln!("carry_visuals_median_ms={:.2}", elapsed[2]);
    }

    #[test]
    #[ignore = "manual performance measurement"]
    fn benchmark_region_references() {
        let elements: Vec<_> = (0..1000)
            .map(|i| json!({"evidence":[format!("region-{i}")]}))
            .collect();
        let visuals: Vec<Value> = vec![];
        let ids: Vec<_> = (0..1000).map(|i| format!("region-{i}")).collect();
        let mut scanned = Vec::new();
        let mut indexed = Vec::new();
        for _ in 0..5 {
            let now = Instant::now();
            let old: Vec<_> = ids
                .iter()
                .map(|id| {
                    elements.iter().any(|element| {
                        element["evidence"]
                            .as_array()
                            .is_some_and(|e| e.contains(&json!(id)))
                    })
                })
                .collect();
            scanned.push(now.elapsed().as_secs_f64() * 1000.0);
            let now = Instant::now();
            let references = referenced_regions(&elements, &visuals);
            let new: Vec<_> = ids
                .iter()
                .map(|id| references.contains(id.as_str()))
                .collect();
            indexed.push(now.elapsed().as_secs_f64() * 1000.0);
            assert_eq!(old, new);
        }
        scanned.sort_by(f64::total_cmp);
        indexed.sort_by(f64::total_cmp);
        eprintln!(
            "regions_scan_ms={:.2} indexed_ms={:.2}",
            scanned[2], indexed[2]
        );
    }
}

struct Element {
    value: Value,
    cells: Vec<Carried>,
}

fn point(cell: &Value) -> Result<(u32, u32)> {
    crate::excel::coordinate(string(&cell["address"])?)
}

fn line((column, row): (u32, u32), rows: bool) -> u32 {
    if rows { row } else { column }
}

fn across((column, row): (u32, u32), rows: bool) -> u32 {
    if rows { column } else { row }
}

fn address(line: u32, across: u32, rows: bool) -> Result<String> {
    let (column, row) = if rows { (across, line) } else { (line, across) };
    Ok(format!("{}{row}", crate::excel::column_name(column)?))
}

/// An extraction cell's content, without the position its ID and address hold.
fn content(cell: &Value) -> Value {
    let mut content = cell.clone();
    if let Some(fields) = content.as_object_mut() {
        fields.remove("id");
        fields.remove("address");
    }
    content
}

/// The cells of each sheet by address.
fn sheet_cells(extraction: &Value) -> Result<BTreeMap<(String, String), &Value>> {
    let mut cells = BTreeMap::new();
    for sheet in array(&extraction["sheets"])? {
        let name = string(&sheet["name"])?;
        for cell in array(&sheet["cells"])? {
            cells.insert(
                (name.to_owned(), string(&cell["address"])?.to_owned()),
                cell,
            );
        }
    }
    Ok(cells)
}

/// Carries `structure`, the interpretation of `before`, onto `after`, the
/// extraction of the original changed by `operations`. Returns the correction
/// journal for `after` and the record of what was carried.
pub(super) fn carry(
    root: &Path,
    structure: &Value,
    before: &Value,
    after: &Value,
    operations: &[StructuralOperation],
    carrier: Carrier,
) -> Result<(Value, Value)> {
    let sheets = array(&before["sheets"])?;
    let names: Vec<&str> = sheets
        .iter()
        .map(|s| s["name"].as_str().unwrap_or(""))
        .collect();
    let after_names: Vec<&str> = array(&after["sheets"])?
        .iter()
        .map(|s| s["name"].as_str().unwrap_or(""))
        .collect();
    ensure!(names == after_names, "the sheets changed");
    let reviewed = structure["review"]["status"] == "accepted"
        && structure["review"]["content_hash"] == content_hash(structure);
    let mut reasons: Vec<String> = vec![];
    let mut moved = 0usize;
    let mut changed = 0usize;
    // What happened to each element's cells, by element ID and kind of change.
    let mut effects: BTreeMap<String, BTreeMap<&'static str, BTreeSet<String>>> = BTreeMap::new();
    let mut affected: Vec<Value> = vec![];

    let mut elements: Vec<Element> = array(&structure["elements"])?
        .iter()
        .map(|element| {
            let mut value = element.clone();
            let cells = value["cells"]
                .as_array_mut()
                .map(std::mem::take)
                .unwrap_or_default()
                .into_iter()
                .map(|cell| Carried {
                    origin: cell["address"].as_str().map(str::to_owned),
                    value: cell,
                })
                .collect();
            Element { value, cells }
        })
        .collect();

    for (index, operation) in operations.iter().enumerate() {
        let rows = operation.row_operation();
        let insert = operation.insertion();
        let (at, count) = (operation.at, operation.count);
        let sheet_index = names
            .iter()
            .position(|name| *name == operation.sheet)
            .context("operation sheet missing")?;
        let prefix = format!("s{}-", sheet_index + 1);
        let merges = crate::excel::merges_after(&sheets[sheet_index], &operations[..=index])?;
        let merges = Merges::new(&merges)?;
        // The style row where the operation's own coordinates, those after the
        // operations before it, place it.
        let style_line = match operation.style_from {
            Some(row) => crate::excel::map_coordinate(
                &operation.sheet,
                &format!("A{row}"),
                &operations[..index],
            )?
            .map(|address| crate::excel::coordinate(&address).map(|(_, row)| row))
            .transpose()?,
            None => None,
        };
        for element in elements
            .iter_mut()
            .filter(|element| element.value["sheet"] == operation.sheet.as_str())
        {
            let element_id = string(&element.value["id"])?.to_owned();
            let lines: BTreeSet<u32> = element
                .cells
                .iter()
                .map(|cell| point(&cell.value).map(|p| line(p, rows)))
                .collect::<Result<_>>()?;
            let (Some(&first), Some(&last)) = (lines.first(), lines.last()) else {
                continue;
            };
            let mut renamed = BTreeMap::new();
            let mut gone = BTreeSet::new();
            let mut kept = Vec::with_capacity(element.cells.len());
            for mut cell in std::mem::take(&mut element.cells) {
                let position = point(&cell.value)?;
                let old = line(position, rows);
                let new = if insert {
                    if old >= at { old + count } else { old }
                } else if (at..at + count).contains(&old) {
                    let id = string(&cell.value["id"])?.to_owned();
                    if cell.origin.is_some() {
                        effect(&mut effects, &element_id, "removed", &id);
                    }
                    gone.insert(id);
                    continue;
                } else if old >= at + count {
                    old - count
                } else {
                    old
                };
                if new != old {
                    let old_address = string(&cell.value["address"])?.to_owned();
                    let new_address = address(new, across(position, rows), rows)?;
                    if cell.value["id"] == format!("{prefix}{old_address}").as_str() {
                        let id = format!("{prefix}{new_address}");
                        renamed.insert(format!("{prefix}{old_address}"), id.clone());
                        cell.value["id"] = json!(id);
                    }
                    cell.value["address"] = json!(new_address);
                    moved += 1;
                    effects
                        .entry(element_id.clone())
                        .or_default()
                        .entry("moved")
                        .or_default();
                }
                kept.push(cell);
            }
            for cell in &mut kept {
                relink(&mut cell.value, &renamed, &gone)?;
            }
            element.cells = kept;
            if !insert || element.value["kind"] != "table" {
                continue;
            }
            let style_in = style_line.is_some_and(|line| lines.contains(&line));
            let inside = at > first && at <= last;
            let below = rows && at == last + 1 && style_in;
            if !(inside || below) {
                continue;
            }
            // The line the new ones copy, where it stands after the insertion.
            let template = if rows && style_in {
                style_line.unwrap()
            } else if lines.contains(&(at - 1)) {
                at - 1
            } else {
                at
            };
            let template = if template >= at {
                template + count
            } else {
                template
            };
            extend(element, &prefix, rows, template, at, count, &merges)?;
        }
    }

    // Settle every carried cell against the new extraction. A cell whose value
    // changed keeps its place in the element; whether it holds text is read
    // from the new value, as the parser reads any cell, unless a reader had
    // found it unreadable or left it unexamined.
    let before_cells = sheet_cells(before)?;
    let after_cells = sheet_cells(after)?;
    let mut owned = BTreeSet::new();
    for element in &mut elements {
        let sheet = string(&element.value["sheet"])?.to_owned();
        let element_id = string(&element.value["id"])?.to_owned();
        let mut dropped = BTreeSet::new();
        let mut settled = Vec::with_capacity(element.cells.len());
        for mut cell in std::mem::take(&mut element.cells) {
            let key = (sheet.clone(), string(&cell.value["address"])?.to_owned());
            let id = string(&cell.value["id"])?.to_owned();
            let now = after_cells.get(&key);
            match &cell.origin {
                None => {
                    let Some(now) = now else {
                        dropped.insert(id);
                        continue;
                    };
                    cell.value["text_state"] = json!(corrections::text_state(Some(now)));
                    effect(&mut effects, &element_id, "added", &id);
                }
                Some(origin) => {
                    let then = before_cells.get(&(sheet.clone(), origin.clone()));
                    match (then, now) {
                        (None, None) => {}
                        (Some(then), Some(now)) if content(then) == content(now) => {}
                        (Some(_), None) => {
                            effect(&mut effects, &element_id, "changed", &id);
                            changed += 1;
                            dropped.insert(id);
                            continue;
                        }
                        (_, Some(now)) => {
                            let examined = ["read", "empty"]
                                .contains(&cell.value["text_state"].as_str().unwrap_or(""));
                            cell.value["text_state"] = if examined {
                                json!(corrections::text_state(Some(now)))
                            } else {
                                json!("not_examined")
                            };
                            effect(&mut effects, &element_id, "changed", &id);
                            changed += 1;
                        }
                    }
                }
            }
            owned.insert(key);
            settled.push(cell);
        }
        for cell in &mut settled {
            relink(&mut cell.value, &BTreeMap::new(), &dropped)?;
        }
        element.cells = settled;
    }
    // An element left without cells is removed with every cell it lost.
    for element in &elements {
        if element.cells.is_empty() {
            let id = string(&element.value["id"])?;
            let cells: BTreeSet<&String> = effects
                .get(id)
                .into_iter()
                .flat_map(|kinds| kinds.values().flatten())
                .collect();
            affected.push(json!({"kind":"element","id":id,"reason":"removed","cells":cells}));
        }
    }
    elements.retain(|element| !element.cells.is_empty());

    // New cells join the element around them, so a correction also holds
    // for what the new version adds inside it.
    let inferred = inference::elements(after)?;
    let joined = join_new_cells(&mut elements, &inferred, &mut owned)?;
    if !joined.is_empty() {
        reasons.push(format!(
            "{} new cells joined the elements around them with the roles of their neighbours; check them",
            joined.len()
        ));
    }
    for (element, cell) in joined {
        effect(&mut effects, &element, "added", &cell);
    }
    let kept_ids: BTreeSet<String> = elements
        .iter()
        .map(|element| string(&element.value["id"]).map(str::to_owned))
        .collect::<Result<_>>()?;
    for (id, kinds) in &effects {
        if !kept_ids.contains(id) {
            continue;
        }
        for (reason, cells) in kinds {
            let mut item = json!({"kind":"element","id":id,"reason":reason});
            if *reason != "moved" {
                item["cells"] = json!(cells);
            }
            affected.push(item);
        }
    }
    let mut carried: Vec<Value> = elements
        .into_iter()
        .map(|mut element| {
            let mut cells: Vec<Value> = element.cells.into_iter().map(|cell| cell.value).collect();
            // A link copied onto a new line can reach a cell that is no header there.
            let headers: BTreeSet<String> = cells
                .iter()
                .filter(|cell| cell["role"] == "row_header" || cell["role"] == "column_header")
                .map(|cell| cell["id"].as_str().unwrap_or("").to_owned())
                .collect();
            for cell in &mut cells {
                let id = cell["id"].as_str().unwrap_or("").to_owned();
                if let Some(links) = cell["headers"].as_array_mut() {
                    links.retain(|link| {
                        link.as_str()
                            .is_some_and(|link| link != id && headers.contains(link))
                    });
                }
            }
            // Cells stay in reading order, as inference lists them.
            cells.sort_by_key(|cell| {
                point(cell)
                    .map(|(column, row)| (row, column))
                    .unwrap_or_default()
            });
            element.value["cells"] = json!(cells);
            if let Some(descriptions) = element
                .value
                .get_mut("descriptions")
                .and_then(Value::as_array_mut)
            {
                descriptions.retain(|id| id.as_str().is_some_and(|id| kept_ids.contains(id)));
                if descriptions.is_empty() {
                    element
                        .value
                        .as_object_mut()
                        .unwrap()
                        .remove("descriptions");
                }
            }
            element.value
        })
        .collect();

    // Cells no carried element holds take the parser's hypothesis, as a
    // re-import does, and need review.
    let leftover: BTreeSet<(String, String)> = after_cells
        .keys()
        .filter(|key| !owned.contains(*key))
        .cloned()
        .collect();
    if !leftover.is_empty() {
        reasons.push(format!(
            "{} new cells lie outside the carried elements and take the parser's hypothesis",
            leftover.len()
        ));
        let mut used: BTreeSet<String> = kept_ids.clone();
        for mut element in inferred {
            let sheet = string(&element["sheet"])?.to_owned();
            element["cells"].as_array_mut().unwrap().retain(|cell| {
                leftover.contains(&(
                    sheet.clone(),
                    cell["address"].as_str().unwrap_or("").to_owned(),
                ))
            });
            if element["cells"].as_array().unwrap().is_empty() {
                continue;
            }
            let local: BTreeSet<String> = element["cells"]
                .as_array()
                .unwrap()
                .iter()
                .map(|cell| cell["id"].as_str().unwrap_or("").to_owned())
                .collect();
            for cell in element["cells"].as_array_mut().unwrap() {
                cell["headers"]
                    .as_array_mut()
                    .unwrap()
                    .retain(|header| local.contains(header.as_str().unwrap_or("")));
            }
            element["id"] = json!(rebase::fresh_id(&mut used, string(&element["id"])?));
            affected.push(
                json!({"kind":"element","id":element["id"],"reason":"inferred","cells":local}),
            );
            carried.push(element);
        }
    }

    // A reading of a drawing or an image belongs to what it shows. It follows
    // the drawing wherever it moved, even when drawings before it were added
    // or removed; one whose content changed is read again.
    let visuals = carry_visuals(structure, before, after, &mut reasons, &mut affected)?;

    // Image regions show the old version. One is still true where the
    // operations moved its range whole and left the cells it shows as they were.
    let referenced_regions = referenced_regions(&carried, &visuals);
    let mut regions = vec![];
    for region in array(&structure["regions"])? {
        let id = string(&region["id"])?;
        let moved_to = moved_range(
            string(&region["sheet"])?,
            string(&region["range"])?,
            operations,
            &before_cells,
            &after_cells,
        )?;
        let used = referenced_regions.contains(id);
        if let Some(range) = moved_to {
            let mut region = region.clone();
            region["range"] = json!(range);
            region["source_sha256"] = after["source"]["sha256"].clone();
            regions.push(region);
        } else if used {
            bail!("image region {id} shows cells that moved or changed; render it again");
        } else {
            affected.push(json!({"kind":"region","id":id,"reason":"removed"}));
        }
    }

    let order: BTreeMap<String, usize> = array(&structure["elements"])?
        .iter()
        .enumerate()
        .map(|(index, element)| Ok((string(&element["id"])?.to_owned(), index)))
        .collect::<Result<_>>()?;
    carried.sort_by_key(|element| {
        order
            .get(element["id"].as_str().unwrap_or(""))
            .copied()
            .unwrap_or(usize::MAX)
    });
    let after_hash = hash(&encoded(after));
    let mut result = json!({"schema_version":1,"document":after["document_id"],
        "source":{"path":after["source"]["path"],"sha256":after["source"]["sha256"]},
        "extraction_hash":after_hash,"regions":regions,"elements":carried,"visuals":visuals,
        "review":{"status":"pending"}});
    if changed > 0 {
        reasons.push(format!(
            "{changed} cells changed in value; check that their elements still read them right"
        ));
    }
    // Recorded operations are facts; aligned ones are a reading of the two
    // versions, which a review confirms.
    if carrier == Carrier::Alignment && !operations.is_empty() {
        reasons.push(format!(
            "{} row or column insertions and deletions were found by aligning the two versions; check the moved elements",
            operations.len()
        ));
    }
    let review = if !reviewed {
        "not_reviewed"
    } else if reasons.is_empty() {
        let mut review = structure["review"].clone();
        review["content_hash"] = json!(content_hash(&result));
        result["review"] = review;
        "kept"
    } else {
        "pending"
    };
    let mut journal = corrections::record(root, after, &result)?;
    let (replayed, _) = rebase::rebase(root, &journal, after, &after_hash, true)?;
    ensure!(
        replayed == result,
        "the carried interpretation does not replay from its corrections"
    );
    let ids: Vec<&str> = operations.iter().map(|o| o.id.as_str()).collect();
    let record = json!({"by":carrier.name(),"operations":ids,"moved":moved,"review":review,
        "reasons":reasons,"affected":affected});
    journal["carried"] = record.clone();
    corrections::validate_journal(&journal)?;
    Ok((journal, record))
}

fn effect(
    effects: &mut BTreeMap<String, BTreeMap<&'static str, BTreeSet<String>>>,
    element: &str,
    kind: &'static str,
    cell: &str,
) {
    effects
        .entry(element.to_owned())
        .or_default()
        .entry(kind)
        .or_default()
        .insert(cell.to_owned());
}

/// The top-left and bottom-right (column, row) of a range.
type Corners = ((u32, u32), (u32, u32));

/// Joins the new cells, which no carried element holds, to the element around
/// them and returns each as (element ID, cell ID). A group of cells the parser
/// forms with old cells of one element joins that element, so a table a
/// reader corrected to text takes a row added to it instead of becoming a
/// table again. Otherwise each cell joins the smallest element whose range
/// covers it, unless the parser finds its group a table of new cells only,
/// which is a new table and stays the parser's. A joined cell takes the role
/// of its nearest cell in the element, in its column above, else in its row
/// to the left, and the header links of that cell moved onto its own line.
fn join_new_cells(
    elements: &mut [Element],
    inferred: &[Value],
    owned: &mut BTreeSet<(String, String)>,
) -> Result<Vec<(String, String)>> {
    let mut owner: BTreeMap<(String, String), usize> = BTreeMap::new();
    // Each element's sheet and the corners of the range its cells cover.
    let mut ranges: Vec<(String, Corners)> = vec![];
    for (index, element) in elements.iter().enumerate() {
        let sheet = string(&element.value["sheet"])?.to_owned();
        let mut low = (u32::MAX, u32::MAX);
        let mut high = (0, 0);
        for cell in &element.cells {
            let (column, row) = point(&cell.value)?;
            low = (low.0.min(column), low.1.min(row));
            high = (high.0.max(column), high.1.max(row));
            owner.insert(
                (sheet.clone(), string(&cell.value["address"])?.to_owned()),
                index,
            );
        }
        ranges.push((sheet, (low, high)));
    }
    let around = |sheet: &str, (column, row): (u32, u32)| -> Option<usize> {
        let mut covering: Vec<(u64, usize)> = ranges
            .iter()
            .enumerate()
            .filter(|(_, (name, (low, high)))| {
                name == sheet
                    && (low.0..=high.0).contains(&column)
                    && (low.1..=high.1).contains(&row)
            })
            .map(|(index, (_, (low, high)))| {
                let area = u64::from(high.0 - low.0 + 1) * u64::from(high.1 - low.1 + 1);
                (area, index)
            })
            .collect();
        covering.sort_unstable();
        match covering.as_slice() {
            [(area, index), rest @ ..] if rest.first().is_none_or(|(next, _)| next > area) => {
                Some(*index)
            }
            _ => None,
        }
    };
    // The new cells of each element, as the parser gives them.
    let mut joining: BTreeMap<usize, Vec<Value>> = BTreeMap::new();
    for group in inferred {
        let sheet = string(&group["sheet"])?;
        let mut owners = BTreeSet::new();
        let mut new = vec![];
        for cell in array(&group["cells"])? {
            let key = (sheet.to_owned(), string(&cell["address"])?.to_owned());
            match owner.get(&key) {
                Some(index) => {
                    owners.insert(*index);
                }
                None if !owned.contains(&key) => new.push(cell),
                None => {}
            }
        }
        if new.is_empty() {
            continue;
        }
        if owners.len() == 1 {
            let index = *owners.first().unwrap();
            joining
                .entry(index)
                .or_default()
                .extend(new.into_iter().cloned());
            continue;
        }
        if owners.is_empty() && group["kind"] == "table" {
            continue;
        }
        for cell in new {
            if let Some(index) = around(sheet, point(cell)?) {
                joining.entry(index).or_default().push(cell.clone());
            }
        }
    }

    let mut joined = vec![];
    for (index, mut cells) in joining {
        let element = &mut elements[index];
        let sheet = string(&element.value["sheet"])?.to_owned();
        let existing: Vec<Value> = element
            .cells
            .iter()
            .map(|cell| cell.value.clone())
            .collect();
        let points: Vec<(u32, u32)> = existing.iter().map(point).collect::<Result<_>>()?;
        cells.sort_by_key(|cell| point(cell).map(|(c, r)| (r, c)).unwrap_or_default());
        // The nearest cell above in the column, else left in the row, else any.
        let template = |(column, row): (u32, u32)| -> usize {
            let nearest = |filter: &dyn Fn(&(u32, u32)) -> bool,
                           distance: &dyn Fn(&(u32, u32)) -> u32| {
                points
                    .iter()
                    .enumerate()
                    .filter(|(_, p)| filter(p))
                    .min_by_key(|(_, p)| distance(p))
                    .map(|(i, _)| i)
            };
            nearest(&|p| p.0 == column && p.1 < row, &|p| row - p.1)
                .or_else(|| nearest(&|p| p.1 == row && p.0 < column, &|p| column - p.0))
                .or_else(|| nearest(&|_| true, &|p| p.0.abs_diff(column) + p.1.abs_diff(row)))
                .unwrap_or(0)
        };
        let mut added: Vec<(Value, usize)> = vec![];
        for cell in &cells {
            let from = template(point(cell)?);
            added.push((
                json!({"id":cell["id"],"address":cell["address"],"role":existing[from]["role"],
                    "headers":[],"text_state":cell["text_state"]}),
                from,
            ));
        }
        // Header links of the template, moved onto the joined cell's line.
        let by_address: BTreeMap<(u32, u32), String> = existing
            .iter()
            .chain(added.iter().map(|(cell, _)| cell))
            .map(|cell| Ok((point(cell)?, string(&cell["id"])?.to_owned())))
            .collect::<Result<_>>()?;
        let by_id: BTreeMap<&str, (u32, u32)> = existing
            .iter()
            .zip(&points)
            .map(|(cell, p)| (cell["id"].as_str().unwrap_or(""), *p))
            .collect();
        for (cell, from) in &mut added {
            let (column, row) = point(cell)?;
            let (from_column, from_row) = points[*from];
            let mut links = vec![];
            for header in array(&existing[*from]["headers"])? {
                let Some(&(header_column, header_row)) = by_id.get(string(header)?) else {
                    continue;
                };
                let link = if from_column == column && header_row == from_row {
                    by_address.get(&(header_column, row)).cloned()
                } else if from_row == row && header_column == from_column {
                    by_address.get(&(column, header_row)).cloned()
                } else {
                    Some(string(header)?.to_owned())
                };
                if let Some(link) = link
                    && !links.contains(&json!(link))
                {
                    links.push(json!(link));
                }
            }
            cell["headers"] = json!(links);
        }
        let element_id = string(&element.value["id"])?.to_owned();
        for (cell, _) in added {
            owned.insert((sheet.clone(), string(&cell["address"])?.to_owned()));
            joined.push((element_id.clone(), string(&cell["id"])?.to_owned()));
            element.cells.push(Carried {
                value: cell,
                origin: None,
            });
        }
    }
    Ok(joined)
}

/// A drawing or an image asset as a reading of it depends on it: what it
/// shows, without where it is placed or how the package names its parts,
/// which change when rows move or other drawings are added.
fn identity(object: &Value) -> Value {
    let mut object = object.clone();
    let Some(fields) = object.as_object_mut() else {
        return object;
    };
    for key in ["anchor", "id", "part", "group", "path", "ocr"] {
        fields.remove(key);
    }
    if let Some(image) = fields.get_mut("image").and_then(Value::as_object_mut) {
        image.remove("part");
        image.remove("asset");
    }
    if let Some(transform) = fields.get_mut("transform").and_then(Value::as_object_mut) {
        transform.remove("off");
        transform.remove("chOff");
    }
    if let Some(connections) = fields.get_mut("connections").and_then(Value::as_array_mut) {
        for connection in connections {
            let target = connection["target"].as_str().map(|target| {
                target
                    .rsplit_once('#')
                    .map_or(target, |(_, id)| id)
                    .to_owned()
            });
            if let Some(target) = target {
                connection["target"] = json!(target);
            }
        }
    }
    object
}

/// The visuals of `after`, each carrying the reading of the visual of
/// `structure` that shows the same objects, with the pointers of its evidence
/// and graph moved to where those objects are now. A visual that was read or
/// edited and matches none is reported to be read again.
fn carry_visuals(
    structure: &Value,
    before: &Value,
    after: &Value,
    reasons: &mut Vec<String>,
    affected: &mut Vec<Value>,
) -> Result<Vec<Value>> {
    struct Candidates {
        order: std::collections::VecDeque<usize>,
        by_id: BTreeMap<String, std::collections::VecDeque<usize>>,
    }
    let shows = |extraction: &Value, visual: &Value| -> Option<Vec<Value>> {
        visual["sources"]
            .as_array()?
            .iter()
            .map(|pointer| extraction.pointer(pointer.as_str()?).map(identity))
            .collect()
    };
    let previous = array(&structure["visuals"])?;
    let mut candidates: BTreeMap<Vec<u8>, Candidates> = BTreeMap::new();
    for (index, old) in previous.iter().enumerate() {
        if let Some(shown) = shows(before, old) {
            let key = serde_json::to_vec(&json!([old["kind"], old["sheet"], shown]))?;
            let bucket = candidates.entry(key).or_insert_with(|| Candidates {
                order: Default::default(),
                by_id: BTreeMap::new(),
            });
            bucket.order.push_back(index);
            bucket
                .by_id
                .entry(string(&old["id"])?.to_owned())
                .or_default()
                .push_back(index);
        }
    }
    let mut taken = vec![false; previous.len()];
    let mut visuals = policy::visuals(after)?;
    for visual in &mut visuals {
        let Some(now) = shows(after, visual) else {
            continue;
        };
        let key = serde_json::to_vec(&json!([visual["kind"], visual["sheet"], now]))?;
        let Some(bucket) = candidates.get_mut(&key) else {
            continue;
        };
        let matching_id = bucket
            .by_id
            .get_mut(string(&visual["id"])?)
            .and_then(|indices| {
                while let Some(index) = indices.pop_front() {
                    if !taken[index] {
                        return Some(index);
                    }
                }
                None
            });
        let index = match matching_id {
            Some(index) => index,
            None => {
                let mut first = None;
                while let Some(index) = bucket.order.pop_front() {
                    if !taken[index] {
                        first = Some(index);
                        break;
                    }
                }
                let Some(index) = first else {
                    continue;
                };
                index
            }
        };
        taken[index] = true;
        let old = &previous[index];
        let moves: BTreeMap<&str, &Value> = array(&old["sources"])?
            .iter()
            .filter_map(Value::as_str)
            .zip(array(&visual["sources"])?)
            .collect();
        let mut carried = old.clone();
        carried["id"] = visual["id"].clone();
        carried["sources"] = visual["sources"].clone();
        let repoint = |references: &mut Value| {
            if let Some(references) = references.as_array_mut() {
                for reference in references {
                    if let Some(new) = reference.as_str().and_then(|old| moves.get(old)) {
                        *reference = (*new).clone();
                    }
                }
            }
        };
        repoint(&mut carried["evidence"]);
        if let Some(graph) = carried.get_mut("graph") {
            for kind in ["nodes", "edges"] {
                if let Some(items) = graph[kind].as_array_mut() {
                    for item in items {
                        repoint(&mut item["sources"]);
                    }
                }
            }
        }
        *visual = carried;
    }
    let unread: BTreeSet<_> = policy::visuals(before)?
        .iter()
        .map(serde_json::to_vec)
        .collect::<serde_json::Result<_>>()?;
    for (index, old) in previous.iter().enumerate() {
        if taken[index] || unread.contains(&serde_json::to_vec(old)?) {
            continue;
        }
        let id = string(&old["id"])?;
        reasons.push(format!("visual {id} changed and needs reading again"));
        affected.push(json!({"kind":"visual","id":id,"reason":"changed"}));
    }
    Ok(visuals)
}

/// Where the range an image region shows lies after `operations`, when they
/// moved it whole and left every cell it shows as it was; None when they
/// inserted or deleted lines inside it or changed a cell it shows.
fn moved_range(
    sheet: &str,
    shown: &str,
    operations: &[StructuralOperation],
    before_cells: &BTreeMap<(String, String), &Value>,
    after_cells: &BTreeMap<(String, String), &Value>,
) -> Result<Option<String>> {
    let ((left, top), (right, bottom)) = range(shown)?;
    let corner = |column: u32, row: u32| -> Option<(u32, u32)> {
        let address = format!("{}{row}", crate::excel::column_name(column).ok()?);
        let mapped = crate::excel::map_coordinate(sheet, &address, operations).ok()??;
        crate::excel::coordinate(&mapped).ok()
    };
    let (Some((new_left, new_top)), Some((new_right, new_bottom))) =
        (corner(left, top), corner(right, bottom))
    else {
        return Ok(None);
    };
    if new_right.checked_sub(new_left) != Some(right - left)
        || new_bottom.checked_sub(new_top) != Some(bottom - top)
    {
        return Ok(None);
    }
    // The cells of each range by their place in it.
    let shown_cells = |cells: &BTreeMap<(String, String), &Value>,
                       left: u32,
                       top: u32,
                       right: u32,
                       bottom: u32| {
        cells
            .iter()
            .filter(|((name, _), _)| name == sheet)
            .filter_map(|((_, address), cell)| {
                let (column, row) = crate::excel::coordinate(address).ok()?;
                ((left..=right).contains(&column) && (top..=bottom).contains(&row))
                    .then(|| ((column - left, row - top), content(cell)))
            })
            .collect::<BTreeMap<_, _>>()
    };
    if shown_cells(before_cells, left, top, right, bottom)
        != shown_cells(after_cells, new_left, new_top, new_right, new_bottom)
    {
        return Ok(None);
    }
    let name = |column: u32, row: u32| -> Result<String> {
        Ok(format!("{}{row}", crate::excel::column_name(column)?))
    };
    Ok(Some(if shown.contains(':') {
        format!(
            "{}:{}",
            name(new_left, new_top)?,
            name(new_right, new_bottom)?
        )
    } else {
        name(new_left, new_top)?
    }))
}

/// Renames header links and drops those to cells that are gone.
fn relink(
    cell: &mut Value,
    renamed: &BTreeMap<String, String>,
    gone: &BTreeSet<String>,
) -> Result<()> {
    let headers = cell["headers"]
        .as_array_mut()
        .context("cell headers missing")?;
    let mut links = vec![];
    for header in headers.iter() {
        let id = string(header)?;
        let id = renamed.get(id).map_or(id, String::as_str);
        if !gone.contains(id) && !links.contains(&json!(id)) {
            links.push(json!(id));
        }
    }
    *headers = links;
    Ok(())
}

/// Adds the cells of `count` lines inserted at `at` to a table, shaped like
/// its line `template` (already moved by the insertion). Each across position
/// copies the template cell there, or the nearest earlier one on that
/// position, so a label merged down over the template row still applies.
/// A header whose merge grows over the new line, and a header across the
/// other axis, stay; any other header moves to the new line's own cell at its
/// position. Cells the writeback left empty, and links to them, are dropped
/// later.
fn extend(
    element: &mut Element,
    prefix: &str,
    rows: bool,
    template: u32,
    at: u32,
    count: u32,
    merges: &Merges<'_>,
) -> Result<()> {
    let across_kind = if rows { "column_header" } else { "row_header" };
    let mut by_position: BTreeMap<u32, BTreeMap<u32, usize>> = BTreeMap::new();
    for (index, cell) in element.cells.iter().enumerate() {
        let position = point(&cell.value)?;
        by_position
            .entry(across(position, rows))
            .or_default()
            .insert(line(position, rows), index);
    }
    let lines: BTreeMap<String, (u32, u32)> = element
        .cells
        .iter()
        .map(|cell| Ok((string(&cell.value["id"])?.to_owned(), point(&cell.value)?)))
        .collect::<Result<_>>()?;
    let mut new_cells = vec![];
    for new_line in at..at + count {
        for (position, cells) in &by_position {
            let Some((_, &index)) = cells.range(..=template).next_back() else {
                continue;
            };
            let source = &element.cells[index].value;
            let mut headers = vec![];
            for header in array(&source["headers"])? {
                let id = string(header)?;
                let Some(&header_point) = lines.get(id) else {
                    continue;
                };
                let header_cell = element
                    .cells
                    .iter()
                    .find(|cell| cell.value["id"] == id)
                    .context("header cell missing")?;
                let own = format!(
                    "{prefix}{}",
                    address(new_line, across(header_point, rows), rows)?
                );
                let header_address = string(&header_cell.value["address"])?;
                let covered = merges
                    .hiding(&address(new_line, across(header_point, rows), rows)?)?
                    .is_some_and(|merge| merge.split(':').next() == Some(header_address));
                let link = if covered || header_cell.value["role"] == across_kind {
                    id.to_owned()
                } else {
                    own
                };
                if !headers.contains(&json!(link)) {
                    headers.push(json!(link));
                }
            }
            let cell_address = address(new_line, *position, rows)?;
            new_cells.push(Carried {
                value: json!({"id":format!("{prefix}{cell_address}"),"address":cell_address,
                    "role":source["role"],"headers":headers,"text_state":"not_examined"}),
                origin: None,
            });
        }
    }
    element.cells.extend(new_cells);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(address: &str, role: &str, headers: &[&str]) -> Value {
        json!({"id":format!("s1-{address}"),"address":address,"role":role,
            "headers":headers.iter().map(|h| format!("s1-{h}")).collect::<Vec<_>>(),"text_state":"read"})
    }

    fn element(id: &str, kind: &str, cells: Vec<Value>) -> Element {
        Element {
            value: json!({"id":id,"kind":kind,"sheet":"S"}),
            cells: cells
                .into_iter()
                .map(|value| Carried {
                    origin: value["address"].as_str().map(str::to_owned),
                    value,
                })
                .collect(),
        }
    }

    fn group(kind: &str, addresses: &[&str]) -> Value {
        json!({"kind":kind,"sheet":"S","cells":addresses.iter().map(|a| cell(a, "data", &[])).collect::<Vec<_>>()})
    }

    fn held<'a>(element: &'a Element, address: &str) -> Option<&'a Value> {
        element
            .cells
            .iter()
            .map(|c| &c.value)
            .find(|c| c["address"] == address)
    }

    #[test]
    fn new_cells_join_the_element_around_them_but_a_new_table_does_not() {
        let mut elements = vec![
            element(
                "table",
                "table",
                vec![
                    cell("A1", "column_header", &[]),
                    cell("B1", "column_header", &[]),
                    cell("A2", "row_header", &["A1"]),
                    cell("B2", "data", &["B1", "A2"]),
                ],
            ),
            element(
                "notes",
                "text",
                vec![cell("D1", "note", &[]), cell("H10", "note", &[])],
            ),
        ];
        let mut owned: BTreeSet<(String, String)> = ["A1", "B1", "A2", "B2", "D1", "H10"]
            .iter()
            .map(|a| ("S".to_owned(), (*a).to_owned()))
            .collect();
        let inferred = [
            // The parser takes the new row 3 into the table.
            group("table", &["A1", "B1", "A2", "B2", "A3", "B3"]),
            // A table of new cells only, inside the range of the notes.
            group("table", &["E3", "F3", "E4", "F4"]),
            // The parser's leftover text: old cells of both elements, a new
            // cell inside the notes' range and one outside every range.
            group("text", &["B2", "D1", "G8", "Z50"]),
        ];
        let joined = join_new_cells(&mut elements, &inferred, &mut owned).unwrap();
        assert_eq!(
            joined,
            [
                ("table".to_owned(), "s1-A3".to_owned()),
                ("table".to_owned(), "s1-B3".to_owned()),
                ("notes".to_owned(), "s1-G8".to_owned()),
            ]
        );
        // The new row takes the roles of the row above, and its header links
        // move onto its own row; column headers stay.
        let a3 = held(&elements[0], "A3").unwrap();
        assert_eq!(a3["role"], "row_header");
        assert_eq!(a3["headers"], json!(["s1-A1"]));
        let b3 = held(&elements[0], "B3").unwrap();
        assert_eq!(b3["role"], "data");
        assert_eq!(b3["headers"], json!(["s1-B1", "s1-A3"]));
        assert_eq!(held(&elements[1], "G8").unwrap()["role"], "note");
        for address in ["E3", "F3", "E4", "F4", "Z50"] {
            assert!(elements.iter().all(|e| held(e, address).is_none()));
            assert!(!owned.contains(&("S".to_owned(), address.to_owned())));
        }
        assert!(owned.contains(&("S".to_owned(), "G8".to_owned())));
    }

    #[test]
    fn a_cell_two_equally_small_ranges_cover_stays_the_parsers() {
        // A1:C3 and B1:D3 both cover the new C2, and neither is smaller.
        let mut elements = vec![
            element(
                "left",
                "text",
                vec![cell("A1", "text", &[]), cell("C3", "text", &[])],
            ),
            element(
                "right",
                "text",
                vec![cell("B1", "text", &[]), cell("D3", "text", &[])],
            ),
        ];
        let mut owned: BTreeSet<(String, String)> = ["A1", "C3", "B1", "D3"]
            .iter()
            .map(|a| ("S".to_owned(), (*a).to_owned()))
            .collect();
        let inferred = [group("text", &["A1", "B1", "C2"])];
        let joined = join_new_cells(&mut elements, &inferred, &mut owned).unwrap();
        assert!(joined.is_empty(), "{joined:?}");
        assert!(elements.iter().all(|e| held(e, "C2").is_none()));
    }
}
