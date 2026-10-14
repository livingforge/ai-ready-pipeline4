//! Private grid projection for writeback. Persisted bodies are document elements;
//! all grid identities and physical positions live in layout.yml.
use super::*;

/// The version of `layout.yml`: font tables, rows and columns as runs of
/// identities in the order of their keys, visuals as their kinds, and no stored bindings, orders, groups, positions,
/// merges, placements, blank row counts, extraction notes, document ID or original hash,
/// which follow from the element IDs, keys, pending operations, metadata,
/// extraction and `document.yml`. The element metadata leaves out what
/// [`elements::complete`] gives back: the IDs of tables, records and blanks,
/// a table's columns, a table cell's column and spans. A table keeps its
/// records as lines of row and column identities ([`stored_tables`]) and the
/// tree its body shows (its heading records, the headings its keys are from,
/// each record's parent and its marks), but no roles or headers, which
/// [`elements::interpret`] gives from the structure interpretation. An element keeps no font that is the font of the
/// extraction cell it was imported from ([`inherited_fonts`]), and empty lists
/// and maps are left out.
pub(super) const LAYOUT_VERSION: &str = "13";

/// The bytes stored for `layout`, whose rows and columns have keys in
/// `extraction`. The groups a page's encoding uses are left
/// out, so they are dropped here: encoding derives them again from the element
/// metadata. The rows and columns are stored as runs of identities by their
/// keys ([`stored_axis`]): their positions follow from the keys and the pending
/// operations ([`update`]), so recording an operation does not rewrite the rows
/// or columns after it, and applying one rewrites a run or two. Merges and the
/// placement of visuals follow from the extraction and the operations
/// ([`elements::geometry`]) and the notes from the extraction ([`notes`]); a
/// visual keeps the kind and text key a body is checked against. The element
/// metadata is stored without what [`load`] gives back ([`LAYOUT_VERSION`]),
/// with the digest of what is stored.
pub(super) fn stored(layout: &mut Value, extraction: &Value) -> Result<Vec<u8>> {
    let mut stored = layout.clone();
    table_cells(&mut stored, |cell| {
        for derived in ["column", "row_span", "column_span"] {
            cell.remove(derived);
        }
        Ok(())
    })?;
    if stored["history"].as_array().is_some_and(Vec::is_empty) {
        stored.as_object_mut().unwrap().remove("history");
    }
    for (page, sheet) in stored["sheets"]
        .as_object_mut()
        .context("layout sheets missing")?
    {
        let current = &layout["sheets"][page];
        let inherited = inherited_fonts(current, extraction)?;
        let mut fonts = elements::Fonts::default();
        font_holders(&mut sheet["element_metadata"], |id, holder| {
            let font = elements::font(current, &Value::Object(holder.clone()))?;
            if inherited.get(id).unwrap_or(&Value::Null) == &font {
                holder.remove("font");
            } else if font.is_null() {
                holder.insert("font".into(), Value::Null);
            } else {
                holder.insert("font".into(), fonts.add(&font));
            }
            Ok(())
        })?;
        sheet["fonts"] = Value::Array(fonts.list);
        for element in sheet["element_metadata"]
            .as_array_mut()
            .context("element metadata missing")?
        {
            let element = element
                .as_object_mut()
                .context("element must be an object")?;
            match element.get("type").and_then(Value::as_str) {
                // The records' IDs go with their rows ([`stored_tables`]).
                Some("table") => {
                    element.remove("id");
                    element.remove("columns");
                }
                Some("blank") => {
                    element.remove("id");
                }
                _ => {}
            }
        }
        stored_tables(&mut sheet["element_metadata"])?;
        sheet["metadata_sha256"] = json!(crate::data::hash_encoded(&sheet["element_metadata"]));
        if cfg!(debug_assertions) {
            let mut restored = sheet.clone();
            expand_tables(&mut restored["element_metadata"])?;
            restore_metadata(&mut restored, &inherited)?;
            ensure!(
                resolved(&restored)? == resolved(current)?,
                "layout of {page} cannot be read back as stored"
            );
        }
        if let Some(sheet) = sheet.as_object_mut() {
            for derived in ["groups", "notes", "merges"] {
                sheet.remove(derived);
            }
        }
        for visual in sheet["visuals"]
            .as_object_mut()
            .context("layout visuals missing")?
            .values_mut()
        {
            let mut kept = json!({"kind": visual["kind"]});
            if let Some(key) = visual.get("text_key") {
                kept["text_key"] = key.clone();
            }
            *visual = kept;
        }
        // The rows and columns pending deletions remove keep their identities
        // until applied: the correction journal names cells of the extraction
        // by them.
        let withdrawn = sheet
            .as_object_mut()
            .and_then(|sheet| sheet.remove(WITHDRAWN))
            .unwrap_or_else(|| json!({}));
        for (axis, rows) in [("rows", true), ("columns", false)] {
            let mut entries = sheet[axis]
                .as_object()
                .context("layout axis missing")?
                .clone();
            if let Some(removed) = withdrawn[axis].as_object() {
                entries.extend(removed.clone());
            }
            sheet[axis] = stored_axis(&entries, rows)?;
        }
        if let Some(sheet) = sheet.as_object_mut() {
            sheet.retain(|key, value| {
                !(DEFAULTS.contains(&key.as_str())
                    && (value.as_object().is_some_and(serde_json::Map::is_empty)
                        || value.as_array().is_some_and(Vec::is_empty)))
            });
        }
    }
    serialized(Path::new("layout.yml"), &stored)
}

/// The members of a layout sheet left out when empty.
const DEFAULTS: [&str; 4] = ["formulas", "visuals", "fonts", "block_titles"];

/// The member of a loaded layout sheet that holds, by axis, the identities
/// pending deletions removed, which [`stored`] keeps.
const WITHDRAWN: &str = "withdrawn";

/// The position a row key or identity `r<n>`, or column letters, names.
fn numbered(name: &str, rows: bool) -> Option<u32> {
    if rows {
        let number: u32 = name.strip_prefix('r')?.parse().ok()?;
        (number > 0 && name == format!("r{number}")).then_some(number)
    } else if !name.is_empty() && name.bytes().all(|b| b.is_ascii_uppercase()) {
        excel::coordinate(&format!("{name}1"))
            .ok()
            .map(|(column, _)| column)
    } else {
        None
    }
}

/// The row key `r<n>` or column letters of `position`.
fn name_of(position: u32, rows: bool) -> Result<String> {
    if rows {
        Ok(format!("r{position}"))
    } else {
        excel::column_name(position)
    }
}

/// The stored form of the identities of an axis: in the order of the keys they
/// have in the extraction, as runs. `r1..r12` is the identities r1 to r12 at
/// the rows of the same numbers, `r13..r38 +1` those one row further on, and
/// `ins-1=r13` an identity naming no row number with its key. An identity that
/// an insertion not yet applied adds has no key in the extraction and is left
/// out: [`update`] adds it again from the operation.
fn stored_axis(entries: &serde_json::Map<String, Value>, rows: bool) -> Result<Value> {
    let mut keyed = BTreeMap::new();
    for (id, entry) in entries {
        let key = string(&entry["key"])?;
        match numbered(key, rows) {
            Some(position) => ensure!(
                keyed.insert(position, id.as_str()).is_none(),
                "duplicate layout key"
            ),
            None => ensure!(key == id, "layout key {key} names no position"),
        }
    }
    let mut runs = vec![];
    // The run being written: its first and last identity number and offset.
    let mut run: Option<(u32, u32, i64)> = None;
    let close = |run: Option<(u32, u32, i64)>, runs: &mut Vec<String>| -> Result<()> {
        if let Some((first, last, offset)) = run {
            let mut text = name_of(first, rows)?;
            if last > first {
                text = format!("{text}..{}", name_of(last, rows)?);
            }
            if offset != 0 {
                text = format!("{text} {offset:+}");
            }
            runs.push(text);
        }
        Ok(())
    };
    for (position, id) in keyed {
        match numbered(id, rows) {
            Some(number) => {
                let offset = i64::from(position) - i64::from(number);
                match run {
                    Some((first, last, o)) if o == offset && last + 1 == number => {
                        run = Some((first, number, offset));
                    }
                    _ => {
                        close(run.take(), &mut runs)?;
                        run = Some((number, number, offset));
                    }
                }
            }
            None => {
                close(run.take(), &mut runs)?;
                runs.push(format!("{id}={}", name_of(position, rows)?));
            }
        }
    }
    close(run, &mut runs)?;
    Ok(json!(runs))
}

/// The identities of the rows and columns of each sheet of an expanded
/// `layout`, at the positions their keys name in the extraction it belongs to:
/// how its correction journal names cells. An identity an insertion not yet
/// applied adds has no such position.
pub(super) fn axes(layout: &Value) -> Result<crate::document_structure::Axes> {
    let mut axes = crate::document_structure::Axes::default();
    for sheet in layout["sheets"]
        .as_object()
        .context("layout sheets missing")?
        .values()
    {
        let mut lines = [vec![], vec![]];
        for (line, (axis, rows)) in lines.iter_mut().zip([("rows", true), ("columns", false)]) {
            for (id, entry) in sheet[axis].as_object().context("layout axis missing")? {
                if let Some(position) = numbered(string(&entry["key"])?, rows) {
                    line.push((position, id.clone()));
                }
            }
        }
        let [rows, columns] = lines;
        axes.insert(string(&sheet["name"])?, rows, columns)?;
    }
    Ok(axes)
}

/// The [`axes`] of the layout stored in `dir`, before its pending operations.
pub(super) fn stored_axes(
    dir: &Path,
    planned: &Planned,
) -> Result<crate::document_structure::Axes> {
    let mut layout = read_planned(
        "layout.yml",
        &dir.join("layout.yml"),
        planned,
        "document-layout",
    )
    .context("Document element bindings missing; re-import the original")?;
    for sheet in layout["sheets"]
        .as_object_mut()
        .context("layout sheets missing")?
        .values_mut()
    {
        for (axis, rows) in [("rows", true), ("columns", false)] {
            sheet[axis] = expand_axis(&sheet[axis], rows)?;
        }
    }
    axes(&layout)
}

/// The identities of an axis with their keys, from its [`stored_axis`] form.
fn expand_axis(stored: &Value, rows: bool) -> Result<Value> {
    let mut entries = serde_json::Map::new();
    let mut add = |id: String, key: String| -> Result<()> {
        ensure!(
            entries.insert(id, json!({ "key": key })).is_none(),
            "duplicate layout identity"
        );
        Ok(())
    };
    for item in array(stored)? {
        let item = string(item)?;
        if let Some((id, key)) = item.split_once('=') {
            identifier(id)?;
            ensure!(
                numbered(key, rows).is_some(),
                "layout key {key} names no position"
            );
            add(id.to_owned(), key.to_owned())?;
            continue;
        }
        let (names, offset) = match item.split_once(' ') {
            Some((names, offset)) => (names, offset.parse::<i64>()?),
            None => (item, 0),
        };
        let (first, last) = names.split_once("..").unwrap_or((names, names));
        let (Some(first), Some(last)) = (numbered(first, rows), numbered(last, rows)) else {
            bail!("invalid layout run {item}");
        };
        ensure!(first <= last, "reversed layout run {item}");
        for number in first..=last {
            let position = u32::try_from(i64::from(number) + offset)
                .ok()
                .filter(|position| *position > 0)
                .with_context(|| format!("layout run {item} leaves the sheet"))?;
            add(name_of(number, rows)?, name_of(position, rows)?)?;
        }
    }
    Ok(Value::Object(entries))
}

/// The notes on what the extraction leaves out, which every page of its
/// document shows.
fn notes(extraction: &Value) -> Value {
    let note = extraction["findings"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|f| f["code"] == "R001")
        .map_or(json!(""), |f| f["message"].clone());
    json!({"title":"未抽出・注意事項","text":note})
}

/// The table elements of the element `metadata` of a page as they are stored:
/// each record as one line naming the identity of its row and those of the
/// columns of its cells, which make the cells' IDs (`r7 B..J`, `r8 B ins-1`),
/// the cells with a formula mark, a font or a tree mark by their IDs, and the
/// row of each record's parent by the record's row. [`expand_tables`] gives
/// them back.
fn stored_tables(metadata: &mut Value) -> Result<()> {
    for element in metadata
        .as_array_mut()
        .context("element metadata missing")?
        .iter_mut()
        .filter(|element| element["type"] == "table")
    {
        let mut lines = vec![];
        let mut marked = serde_json::Map::new();
        let mut parents = serde_json::Map::new();
        let mut record_rows = HashMap::new();
        for row in array(&element["rows"])? {
            let first = string(&row["cells"][0]["id"])?;
            if let (Some(id), Some((axis, _))) = (row["id"].as_str(), elements::cell_axes(first)) {
                record_rows.insert(id.to_owned(), axis);
            }
        }
        for row in array(&element["rows"])? {
            let mut record = None;
            let mut columns = vec![];
            for cell in array(&row["cells"])? {
                let mut cell = cell
                    .as_object()
                    .context("table cell must be an object")?
                    .clone();
                let id = string(&cell.remove("id").unwrap_or_default())?.to_owned();
                let (row, column) = elements::cell_axes(&id)
                    .filter(|(row, column)| {
                        identifier(row).is_ok()
                            && identifier(column).is_ok()
                            && elements::cell_id(row, column) == id
                    })
                    .with_context(|| format!("table cell {id} names no row and column"))?;
                ensure!(
                    record.get_or_insert_with(|| row.clone()) == &row,
                    "table record of {id} holds cells of two rows"
                );
                columns.push(column);
                if !cell.is_empty() {
                    marked.insert(id, Value::Object(cell));
                }
            }
            let record = record.context("table record without cells")?;
            lines.push(json!(format!("{record} {}", column_runs(&columns)?)));
            if let Some(parent) = row["parent"].as_str() {
                let parent = record_rows
                    .get(parent)
                    .with_context(|| format!("parent {parent} is no record of the table"))?;
                parents.insert(record, json!(parent));
            }
        }
        element["rows"] = json!(lines);
        if !marked.is_empty() {
            element["cells"] = Value::Object(marked);
        }
        if !parents.is_empty() {
            element["parents"] = Value::Object(parents);
        }
    }
    Ok(())
}

/// The column identities of a stored record, separated by spaces, three or
/// more columns named by consecutive letters as their first and last (`B..J`).
fn column_runs(columns: &[String]) -> Result<String> {
    let mut items = vec![];
    let mut i = 0;
    while i < columns.len() {
        let mut j = i;
        if let Some(first) = numbered(&columns[i], false) {
            while j + 1 < columns.len()
                && numbered(&columns[j + 1], false) == Some(first + u32::try_from(j + 1 - i)?)
            {
                j += 1;
            }
        }
        if j >= i + 2 {
            items.push(format!("{}..{}", columns[i], columns[j]));
        } else {
            items.extend(columns[i..=j].iter().cloned());
        }
        i = j + 1;
    }
    Ok(items.join(" "))
}

/// The table elements of stored element `metadata` with each record's cells
/// in place, the cells their [`stored_tables`] line names with their formula
/// marks and fonts.
fn expand_tables(metadata: &mut Value) -> Result<()> {
    for element in metadata
        .as_array_mut()
        .context("element metadata missing")?
        .iter_mut()
        .filter(|element| element["type"] == "table")
    {
        let element = element.as_object_mut().context("table must be an object")?;
        let mut marked = match element.remove("cells") {
            Some(Value::Object(marked)) => marked,
            Some(_) => bail!("table cells must be a map"),
            None => serde_json::Map::new(),
        };
        let mut parents = match element.remove("parents") {
            Some(Value::Object(parents)) => parents,
            Some(_) => bail!("table parents must be a map"),
            None => serde_json::Map::new(),
        };
        let mut rows = vec![];
        for line in array(element.get("rows").context("table rows missing")?)? {
            let mut items = string(line)?.split(' ');
            let record = items.next().context("table record missing")?;
            let mut cells = vec![];
            for item in items {
                let columns = match item.split_once("..") {
                    Some((first, last)) => {
                        let (Some(first), Some(last)) =
                            (numbered(first, false), numbered(last, false))
                        else {
                            bail!("invalid table columns {item}");
                        };
                        ensure!(first < last, "reversed table columns {item}");
                        (first..=last)
                            .map(|column| name_of(column, false))
                            .collect::<Result<Vec<_>>>()?
                    }
                    None => vec![item.to_owned()],
                };
                for column in columns {
                    let id = elements::cell_id(record, &column);
                    let mut cell = marked.remove(&id).unwrap_or_else(|| json!({}));
                    cell["id"] = json!(id);
                    cells.push(cell);
                }
            }
            ensure!(!cells.is_empty(), "table record {record} has no cells");
            let mut row = json!({ "cells": cells });
            if let Some(parent) = parents.remove(record) {
                row["parent"] = json!(elements::token("record", &[string(&parent)?]));
            }
            rows.push(row);
        }
        if let Some(id) = marked.keys().next() {
            bail!("table cell {id} is in no record of its table");
        }
        if let Some(record) = parents.keys().next() {
            bail!("table parent of {record} is for no record of its table");
        }
        element.insert("rows".into(), Value::Array(rows));
    }
    Ok(())
}

/// Calls `visit` with each table cell of the element metadata of `layout`.
fn table_cells(
    layout: &mut Value,
    mut visit: impl FnMut(&mut serde_json::Map<String, Value>) -> Result<()>,
) -> Result<()> {
    for sheet in layout["sheets"]
        .as_object_mut()
        .context("layout sheets missing")?
        .values_mut()
    {
        for element in sheet["element_metadata"]
            .as_array_mut()
            .context("element metadata missing")?
            .iter_mut()
            .filter(|element| element["type"] == "table")
        {
            for row in element["rows"]
                .as_array_mut()
                .context("table rows missing")?
            {
                for cell in row["cells"].as_array_mut().context("table cells missing")? {
                    visit(
                        cell.as_object_mut()
                            .context("table cell must be an object")?,
                    )?;
                }
            }
        }
    }
    Ok(())
}

/// Calls `visit` with the ID and the members of each element and table cell
/// of the element `metadata` of a page that can have a font, in order.
fn font_holders(
    metadata: &mut Value,
    mut visit: impl FnMut(&str, &mut serde_json::Map<String, Value>) -> Result<()>,
) -> Result<()> {
    for element in metadata
        .as_array_mut()
        .context("element metadata missing")?
    {
        match element["type"].as_str() {
            Some("text" | "formula") => {
                let id = string(&element["id"])?.to_owned();
                visit(&id, element.as_object_mut().unwrap())?;
            }
            Some("table") => {
                for row in element["rows"]
                    .as_array_mut()
                    .context("table rows missing")?
                {
                    for cell in row["cells"].as_array_mut().context("table cells missing")? {
                        let id = string(&cell["id"])?.to_owned();
                        visit(
                            &id,
                            cell.as_object_mut()
                                .context("table cell must be an object")?,
                        )?;
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// The font each cell of a layout `sheet` has from the cell of the extraction
/// it names by the keys of its row and column, by element ID: the font import
/// gives it. A sheet the extraction does not hold once under its name, such
/// as a copied slide, has none, nor does a row or column an insertion adds.
fn inherited_fonts(sheet: &Value, extraction: &Value) -> Result<HashMap<String, Value>> {
    let sources: Vec<_> = array(&extraction["sheets"])?
        .iter()
        .filter(|source| source["name"] == sheet["name"])
        .collect();
    let [source] = sources.as_slice() else {
        return Ok(HashMap::new());
    };
    let identities = |axis: &str| -> Result<HashMap<String, String>> {
        sheet[axis]
            .as_object()
            .context("layout axis missing")?
            .iter()
            .map(|(id, entry)| Ok((string(&entry["key"])?.to_owned(), id.clone())))
            .collect()
    };
    let rows = identities("rows")?;
    let columns = identities("columns")?;
    let mut fonts = HashMap::new();
    for cell in array(&source["cells"])? {
        let font = crate::extraction::font(source, cell)?;
        if font.is_null() {
            continue;
        }
        let (column, row) = excel::coordinate(string(&cell["address"])?)?;
        if let (Some(row), Some(column)) = (
            rows.get(&name_of(row, true)?),
            columns.get(&name_of(column, false)?),
        ) {
            fonts.insert(elements::token("cell", &[row, column]), font.clone());
        }
    }
    Ok(fonts)
}

/// Puts back in place what [`stored`] leaves out of the element metadata of
/// a layout `sheet` whose rows and columns are placed: the fonts, an element
/// without one taking the font it has from the extraction (`inherited`), and
/// what [`elements::complete`] gives.
fn restore_metadata(sheet: &mut Value, inherited: &HashMap<String, Value>) -> Result<()> {
    let stored = sheet["fonts"].as_array().cloned().unwrap_or_default();
    let mut fonts = elements::Fonts::default();
    let mut metadata = sheet["element_metadata"].take();
    font_holders(&mut metadata, |id, holder| {
        let font = match holder.get("font") {
            None => inherited.get(id).cloned().unwrap_or(Value::Null),
            Some(Value::Null) => Value::Null,
            Some(index) => {
                let index = index.as_u64().context("font index must be a number")?;
                usize::try_from(index)
                    .ok()
                    .and_then(|index| stored.get(index))
                    .with_context(|| format!("unknown font index {index}"))?
                    .clone()
            }
        };
        if font.is_null() {
            holder.remove("font");
        } else {
            holder.insert("font".into(), fonts.add(&font));
        }
        Ok(())
    })?;
    elements::complete(sheet, &mut metadata)?;
    sheet["element_metadata"] = metadata;
    sheet["fonts"] = Value::Array(fonts.list);
    Ok(())
}

/// The element metadata of a layout `sheet` with each font in place of its
/// index, to compare metadata whatever order their font tables have.
fn resolved(sheet: &Value) -> Result<Value> {
    let mut metadata = sheet["element_metadata"].clone();
    font_holders(&mut metadata, |_, holder| {
        let font = elements::font(sheet, &Value::Object(holder.clone()))?;
        if font.is_null() {
            holder.remove("font");
        } else {
            holder.insert("font".into(), font);
        }
        Ok(())
    })?;
    Ok(metadata)
}

/// A stored layout with what [`stored`] leaves out of its rows, columns and
/// sheets in place: each row and column with its key, for [`update`] to
/// place, and the empty history, formulas, visuals, fonts and block titles.
/// The element metadata is checked against its digest; [`restore_metadata`]
/// completes it once the rows and columns are placed.
fn expand(layout: &mut Value) -> Result<()> {
    if layout.get("history").is_none() {
        layout["history"] = json!([]);
    }
    for sheet in layout["sheets"]
        .as_object_mut()
        .context("layout sheets missing")?
        .values_mut()
    {
        let Some(members) = sheet.as_object_mut() else {
            bail!("layout sheet must be an object");
        };
        let digest = members.remove("metadata_sha256");
        ensure!(
            digest.as_ref().and_then(Value::as_str)
                == Some(crate::data::hash_encoded(&sheet["element_metadata"]).as_str()),
            "element structure differs from bindings; use structure-save or row/column operations"
        );
        expand_tables(&mut sheet["element_metadata"])?;
        for (member, empty) in [
            ("formulas", json!({})),
            ("visuals", json!({})),
            ("fonts", json!([])),
            ("block_titles", json!({})),
        ] {
            if sheet.get(member).is_none() {
                sheet[member] = empty;
            }
        }
        for (axis, rows) in [("rows", true), ("columns", false)] {
            sheet[axis] = expand_axis(&sheet[axis], rows)?;
        }
    }
    Ok(())
}

/// A stored layout with its rows and columns placed under the pending
/// `operations` of the same version, as [`load`] places them with the
/// `extraction` of that version, and its element metadata complete. Its
/// elements keep only the fonts stored.
pub(super) fn placed(mut layout: Value, extraction: &Value, operations: &Value) -> Result<Value> {
    expand(&mut layout)?;
    place(&mut layout, extraction, operations, false)?;
    Ok(layout)
}

/// Places the rows and columns of an expanded `layout` under the pending
/// `operations`, with the merges and visuals the `extraction` gives under
/// them, and completes its element metadata ([`restore_metadata`]); with
/// `inherit`, an element without a font stored takes the font of the
/// extraction cell it was imported from.
fn place(layout: &mut Value, extraction: &Value, operations: &Value, inherit: bool) -> Result<()> {
    let operated = operated_extraction(extraction, operations)?;
    let operations = excel::parse_operations(array(operations)?, array(&operated["sheets"])?)?;
    update(layout, &operations)?;
    elements::geometry(layout, &operated, &operations)?;
    for sheet in layout["sheets"]
        .as_object_mut()
        .context("layout sheets missing")?
        .values_mut()
    {
        let inherited = if inherit {
            inherited_fonts(sheet, extraction)?
        } else {
            HashMap::new()
        };
        restore_metadata(sheet, &inherited)?;
    }
    Ok(())
}

pub(super) fn is_excel(extraction: &Value) -> bool {
    extraction["parser"]
        .as_str()
        .is_some_and(|p| p.contains(";cells/"))
}

pub(super) fn page_id(sheet: &Value, index: usize) -> String {
    sheet["page"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| format!("sheet-{}", index + 1))
}

pub(super) fn initial(extraction: &Value) -> Result<Value> {
    let mut sheets = json!({});
    for (index, sheet) in array(&extraction["sheets"])?.iter().enumerate() {
        sheets[page_id(sheet, index)] = initial_sheet(sheet)?;
    }
    let layout = json!({"schema_version":LAYOUT_VERSION,"sheets":sheets,"history":[]});
    check_axes(&layout)?;
    Ok(layout)
}

/// The layout of a page before its content is encoded: its rows, columns and
/// formulas by their positions in the extraction `sheet`. Every row and column
/// up to the last cell has an identity, so they are stored as one run each.
pub(super) fn initial_sheet(sheet: &Value) -> Result<Value> {
    {
        let mut rows = json!({});
        let mut columns = json!({});
        let mut formulas = json!({});
        let (mut max_column, mut max_row) = (0, 0);
        for cell in array(&sheet["cells"])? {
            let (column, row) = excel::coordinate(string(&cell["address"])?)?;
            max_row = max_row.max(row);
            max_column = max_column.max(column);
            if cell["type"] == "formula" {
                formulas[string(&cell["id"])?] =
                    json!({"row":format!("r{row}"),"column":excel::column_name(column)?});
            }
        }
        for row in 1..=max_row {
            rows[format!("r{row}")] = json!({"key":format!("r{row}"),"position":row});
        }
        for column in 1..=max_column {
            let key = excel::column_name(column)?;
            columns[&key] = json!({"key":key,"position":column});
        }
        Ok(
            json!({"name":sheet["name"],"rows":rows,"columns":columns,"formulas":formulas,"element_metadata":[],"fonts":[],"groups":{},"notes":{},"merges":sheet["merges"],"block_titles":{},"visuals":{}}),
        )
    }
}

/// Checks that the rows and columns of every sheet have valid identities and
/// unique keys and positions. Their order follows from the positions and is
/// not stored.
pub(super) fn check_axes(layout: &Value) -> Result<()> {
    for sheet in layout["sheets"]
        .as_object()
        .context("layout sheets missing")?
        .values()
    {
        for axis in ["rows", "columns"] {
            let mut positions = BTreeSet::new();
            let mut keys = BTreeSet::new();
            for (id, entry) in sheet[axis].as_object().context("layout axis missing")? {
                identifier(id)?;
                ensure!(keys.insert(string(&entry["key"])?), "duplicate layout key");
                let position = entry["position"]
                    .as_u64()
                    .context("layout position missing")?;
                ensure!(positions.insert(position), "duplicate layout position");
            }
        }
    }
    Ok(())
}

pub(super) fn load(dir: &Path, extraction: &Value, planned: &Planned) -> Result<Value> {
    let layout = read_planned(
        "layout.yml",
        &dir.join("layout.yml"),
        planned,
        "document-layout",
    )
    .context("Document element bindings missing; re-import the original")?;
    let mut ordered = layout;
    expand(&mut ordered)?;
    let notes = notes(extraction);
    for sheet in ordered["sheets"]
        .as_object_mut()
        .context("layout sheets missing")?
        .values_mut()
    {
        sheet["notes"] = notes.clone();
    }
    let mappings = complete_mappings(read_planned(
        "mappings.yml",
        &dir.join("mappings.yml"),
        planned,
        "mappings",
    )?);
    place(&mut ordered, extraction, &mappings["operations"], true)?;
    // mappings operations are the writeback authority. Project them even when
    // authored directly; the CLI saves the resulting layout atomically and
    // apply persists it through reimport. Only the current element format is accepted.
    Ok(ordered)
}

fn axis_keys(sheet: &Value, axis: &str, to_runtime: bool) -> Result<BTreeMap<String, String>> {
    sheet[axis]
        .as_object()
        .context("layout axis missing")?
        .iter()
        .map(|(id, entry)| {
            let key = string(&entry["key"])?;
            Ok(if to_runtime {
                (id.clone(), key.to_owned())
            } else {
                (key.to_owned(), id.clone())
            })
        })
        .collect()
}

fn formula_keys(sheet: &Value, page: &str, to_runtime: bool) -> Result<BTreeMap<String, String>> {
    let rows = axis_keys(sheet, "rows", true)?;
    let columns = axis_keys(sheet, "columns", true)?;
    sheet["formulas"]
        .as_object()
        .context("layout formulas missing")?
        .iter()
        .map(|(id, target)| {
            let row = rows
                .get(string(&target["row"])?)
                .context("formula row identity missing")?;
            let col = columns
                .get(string(&target["column"])?)
                .context("formula column identity missing")?;
            let key = format!(
                "c-{}-{}{}",
                page.trim_start_matches("sheet-"),
                col,
                row.trim_start_matches('r')
            );
            Ok(if to_runtime {
                (id.clone(), key)
            } else {
                (key, id.clone())
            })
        })
        .collect()
}

/// The version of the writer's private grid pages, values by block, row and
/// column, which content pages are encoded from and decoded to.
pub(super) const GRID_VERSION: &str = "4";

/// Translate internal grid identities to the writer coordinate keys.
pub(super) fn grid(mut page: Value, layout: &Value, to_runtime: bool) -> Result<Value> {
    ensure!(
        page["schema_version"] == GRID_VERSION,
        "Document body requires document elements"
    );
    let page_id = string(&page["page_id"])?.to_owned();
    let sheet = &layout["sheets"][page_id.as_str()];
    ensure!(
        sheet.is_object() && sheet["name"] == page["title"],
        "layout page mismatch"
    );
    let rows = axis_keys(sheet, "rows", to_runtime)?;
    let columns = axis_keys(sheet, "columns", to_runtime)?;
    let formulas = formula_keys(sheet, &page_id, to_runtime)?;
    // The bodies and fonts are converted in place, their values moved.
    let mut take = |pointer: &str| match page.pointer_mut(pointer) {
        Some(Value::Object(object)) => Some(std::mem::take(object)),
        _ => None,
    };
    let table = take("/blocks/table-1/rows");
    let formula_rows = take("/blocks/formulas/rows");
    let fonts = take("/fonts");
    if let Some(body) = table {
        let mut converted = json!({});
        for (row, values) in body {
            let target_row = rows.get(&row).with_context(|| {
                format!(
                    "unknown row identity: deleted by operations or unmapped ({} {row})",
                    sheet["name"].as_str().unwrap_or("sheet")
                )
            })?;
            let Value::Object(values) = values else {
                bail!("invalid body row")
            };
            for (column, value) in values {
                let target_column = columns.get(&column).context("unknown column identity")?;
                converted[target_row][target_column] = value;
            }
        }
        page["blocks"]["table-1"]["rows"] = converted;
    }
    if let Some(body) = formula_rows {
        let mut converted = json!({});
        for (field, value) in body {
            converted[formulas.get(&field).with_context(|| {
                format!(
                    "{}!{} is deleted or has unknown formula identity",
                    sheet["name"].as_str().unwrap_or("sheet"),
                    field.rsplit('-').next().unwrap_or(&field)
                )
            })?] = value;
        }
        page["blocks"]["formulas"]["rows"] = converted;
    }
    // Fonts follow the values they belong to; one whose row, column or formula
    // an operation removed is dropped with its value.
    if let Some(fonts) = fonts {
        let mut converted = json!({});
        for (block, body) in fonts {
            let Value::Object(body) = body else {
                bail!("invalid font block")
            };
            match block.as_str() {
                "table-1" => {
                    for (row, values) in body {
                        let Some(target_row) = rows.get(&row) else {
                            continue;
                        };
                        let Value::Object(values) = values else {
                            bail!("invalid font row")
                        };
                        for (column, font) in values {
                            if let Some(target_column) = columns.get(&column) {
                                converted["table-1"][target_row][target_column] = font;
                            }
                        }
                    }
                }
                "formulas" => {
                    for (field, font) in body {
                        if let Some(target) = formulas.get(&field) {
                            converted["formulas"][target] = font;
                        }
                    }
                }
                _ => converted[&block] = Value::Object(body),
            }
        }
        page["fonts"] = converted;
    }
    let mut out = page;
    if !to_runtime {
        // Formula caches are extraction metadata, not editable body values.
        for target in sheet["formulas"]
            .as_object()
            .context("layout formulas missing")?
            .values()
        {
            let row = string(&target["row"])?;
            let column = string(&target["column"])?;
            if let Some(values) = out["blocks"]["table-1"]["rows"][row].as_object_mut() {
                values.remove(column);
            }
        }
        if let Some(rows) = out["blocks"]["table-1"]["rows"].as_object_mut() {
            rows.retain(|_, values| values.as_object().is_none_or(|v| !v.is_empty()));
        }
    }
    Ok(out)
}

/// A deep copy of `value` without the members at `skip`, each a path of keys.
pub(super) fn copy_except(value: &Value, skip: &[&[&str]]) -> Value {
    let Some(object) = value.as_object() else {
        return value.clone();
    };
    Value::Object(
        object
            .iter()
            .filter(|(key, _)| !skip.iter().any(|path| path == &[key.as_str()]))
            .map(|(key, member)| {
                let below: Vec<&[&str]> = skip
                    .iter()
                    .filter(|path| path.len() > 1 && path[0] == key)
                    .map(|path| &path[1..])
                    .collect();
                (key.clone(), copy_except(member, &below))
            })
            .collect(),
    )
}

pub(super) fn runtime(page: &Value, layout: &Value, extraction: &Value) -> Result<Value> {
    let decoded = super::elements::decode(page, layout)?;
    let mut page = grid(decoded, layout, true)?;
    if is_excel(extraction) {
        let index: usize = string(&page["page_id"])?
            .trim_start_matches("sheet-")
            .parse()?;
        let source = array(&extraction["sheets"])?
            .get(index - 1)
            .context("missing layout sheet")?;
        let fields = formula_keys(
            &layout["sheets"][string(&page["page_id"])?],
            string(&page["page_id"])?,
            true,
        )?;
        for cell in array(&source["cells"])? {
            if cell["type"] == "formula" && fields.values().any(|field| cell["id"] == *field) {
                let (column, row) = excel::coordinate(string(&cell["address"])?)?;
                let row = format!("r{row}");
                let column = excel::column_name(column)?;
                ensure!(
                    page["blocks"]["table-1"]["rows"]
                        .get(&row)
                        .and_then(|r| r.get(&column))
                        .is_none(),
                    "{}!{} is not written back: formula caches belong to extraction metadata, not body YAML",
                    source["name"].as_str().unwrap_or("sheet"),
                    cell["address"].as_str().unwrap_or("cell")
                );
                if page["blocks"]["table-1"].is_null() {
                    page["blocks"]["table-1"] = json!({"title":"本文","rows":{}});
                }
                page["blocks"]["table-1"]["rows"][&row][&column] = cell["value"].clone();
            }
        }
    }
    Ok(page)
}

impl Store {
    pub(super) fn read_content(&self, dir: &Path, name: &str, extraction: &Value) -> Result<Value> {
        let page = read(&under(dir, name)?, Some("content"))?;
        runtime(&page, &load(dir, extraction, &Planned::new())?, extraction)
    }
}

/// Refresh placements without renaming existing identities or body keys.
pub(super) fn update(layout: &mut Value, operations: &[excel::StructuralOperation]) -> Result<()> {
    for operation in operations {
        let history = layout["history"]
            .as_array_mut()
            .context("layout history missing")?;
        {
            let kind = match operation.kind {
                excel::OperationKind::InsertRows => "insert_rows",
                excel::OperationKind::DeleteRows => "delete_rows",
                excel::OperationKind::InsertColumns => "insert_columns",
                excel::OperationKind::DeleteColumns => "delete_columns",
                excel::OperationKind::MoveColumns { .. } => "move_columns",
            };
            let mut record = json!({"id":operation.id,"sheet":operation.sheet,"kind":kind,"at":operation.at,"count":operation.count,"reason":"ARP recorded structural operation"});
            if let excel::OperationKind::MoveColumns { to } = operation.kind {
                record["to"] = json!(to);
            }
            if let Some(style_from) = operation.style_from {
                record["style_from"] = json!(style_from);
            }
            if let Some(existing) = history.iter().find(|o| o["id"] == operation.id) {
                ensure!(
                    *existing == record,
                    "operation history mismatch; do not rewrite applied operations"
                );
            } else {
                history.push(record);
            }
        }
    }
    for sheet in layout["sheets"]
        .as_object_mut()
        .context("layout sheets missing")?
        .values_mut()
    {
        let name = string(&sheet["name"])?.to_owned();
        for (axis, rows) in [("rows", true), ("columns", false)] {
            let axis_operations: Vec<_> = operations
                .iter()
                .filter(|op| op.row_operation() == rows)
                .cloned()
                .collect();
            let entries = sheet[axis].as_object_mut().context("layout axis missing")?;
            for entry in entries.values_mut() {
                let key = string(&entry["key"])?;
                let address = if rows {
                    format!("A{}", key.trim_start_matches('r'))
                } else {
                    format!("{key}1")
                };
                if !key.contains('-') {
                    if let Some(mapped) = excel::map_coordinate(&name, &address, &axis_operations)?
                    {
                        let (column, row) = excel::coordinate(&mapped)?;
                        entry["position"] = json!(if rows { row } else { column });
                    } else {
                        entry["position"] = Value::Null;
                    }
                }
            }
            let removed: Vec<_> = entries
                .iter()
                .filter(|(_, entry)| entry["position"].is_null())
                .map(|(id, entry)| (id.clone(), json!({"key": entry["key"]})))
                .collect();
            entries.retain(|_, e| !e["position"].is_null());
            for (id, entry) in removed {
                sheet[WITHDRAWN][axis][id] = entry;
            }
            let entries = sheet[axis].as_object_mut().context("layout axis missing")?;
            for op in operations
                .iter()
                .filter(|o| o.sheet == name && o.row_operation() == rows && o.insertion())
            {
                for offset in 0..op.count {
                    let id = format!("{}-{}", op.id, offset + 1);
                    let resolved = excel::resolve_insertion(
                        &name,
                        &op.id,
                        offset,
                        if rows { Some("A") } else { None },
                        if rows { None } else { Some(1) },
                        &axis_operations,
                    );
                    if let Ok(address) = resolved {
                        let (column, row) = excel::coordinate(&address)?;
                        if let Some(present) = entries.get(&id) {
                            ensure!(
                                present["key"] == id,
                                "operation identity already used; choose a fresh --id"
                            );
                        }
                        entries.insert(
                            id.clone(),
                            json!({"key":id,"position":if rows{row}else{column}}),
                        );
                    } else {
                        entries.remove(&id);
                    }
                }
            }
        }
        let kept_rows: BTreeSet<_> = sheet["rows"]
            .as_object()
            .context("layout rows missing")?
            .keys()
            .cloned()
            .collect();
        let kept_columns: BTreeSet<_> = sheet["columns"]
            .as_object()
            .context("layout columns missing")?
            .keys()
            .cloned()
            .collect();
        sheet["formulas"]
            .as_object_mut()
            .context("layout formulas missing")?
            .retain(|_, target| {
                target["row"]
                    .as_str()
                    .is_some_and(|row| kept_rows.contains(row))
                    && target["column"]
                        .as_str()
                        .is_some_and(|col| kept_columns.contains(col))
            });
    }
    check_axes(layout)
}

/// The cells of an extraction `sheet` that hold a value or a formula, by
/// address. An empty cell is left out, whether it only carries formatting or
/// holds an empty string: whether a writer stores one, as Excel does for each
/// cell of a merged range, and in which form, says nothing of the grid.
fn filled(sheet: &Value) -> Result<BTreeMap<&str, &Value>> {
    array(&sheet["cells"])?
        .iter()
        .filter(|cell| {
            cell["formula"].is_string() || !(cell["value"].is_null() || cell["value"] == "")
        })
        .map(|cell| Ok((string(&cell["address"])?, cell)))
        .collect()
}

/// An identity for the row or column at the position `key` names, which has
/// none: the key itself, as it would have been from the start, or the key with
/// the first free suffix when an operation has moved another identity onto
/// that name.
fn fresh_identity(entries: &serde_json::Map<String, Value>, key: &str) -> String {
    std::iter::once(key.to_owned())
        .chain((1..).map(|n| format!("{key}_{n}")))
        .find(|id| !entries.contains_key(id))
        .unwrap()
}

/// The trusted apply path knows the placement of every retained identity.
/// External sources have no such proof: only one-cell changes on the same grid
/// are accepted; structural or multi-cell changes require an explicit mapping.
/// Cells that only carry formatting are not compared. A row or column without
/// an identity takes a new one when such a cell is there or, for an external
/// source, the one cell edited: no value moved there.
pub(super) fn reimport(dir: &Path, extraction: &Value, trusted: bool) -> Result<Value> {
    if !dir.join("document.yml").exists() {
        return initial(extraction);
    }
    let before = crate::extraction::read_record(&dir.join("extraction.json"))?;
    let mut layout = load(dir, &before, &Planned::new())?;
    if !trusted {
        let mappings = read_mappings(&dir.join("mappings.yml"))?;
        ensure!(
            array(&mappings["operations"])?.is_empty(),
            "ambiguous correspondence: apply recorded operations before reimport; pending identities must not be guessed"
        );
    }
    if trusted {
        let mappings = read_mappings(&dir.join("mappings.yml"))?;
        let operations =
            excel::parse_operations(array(&mappings["operations"])?, array(&before["sheets"])?)?;
        update(&mut layout, &operations)?;
    } else if before["sheets"] != extraction["sheets"] {
        let old = array(&before["sheets"])?;
        let new = array(&extraction["sheets"])?;
        ensure!(
            old.len() == new.len(),
            "ambiguous correspondence: external sheet structure changed"
        );
        let mut changed = 0;
        for (a, b) in old.iter().zip(new) {
            ensure!(
                a["name"] == b["name"] && a["merges"] == b["merges"],
                "ambiguous correspondence: external sheet structure changed"
            );
            let (ac, bc) = (filled(a)?, filled(b)?);
            let addresses: BTreeSet<_> = ac.keys().chain(bc.keys()).collect();
            for address in addresses {
                match (ac.get(address), bc.get(address)) {
                    (Some(x), Some(y)) => {
                        ensure!(
                            x["type"] == y["type"],
                            "ambiguous correspondence: external coordinates or cell types changed"
                        );
                        changed += usize::from(
                            (x["type"] != "formula" && x["value"] != y["value"])
                                || x["formula"] != y["formula"],
                        );
                    }
                    // A cell filled or emptied.
                    _ => changed += 1,
                }
            }
        }
        ensure!(
            changed <= 1,
            "ambiguous correspondence: external row/column change or multi-cell edit could be a move; no identity guessing"
        );
    }
    let new_sheets = array(&extraction["sheets"])?;
    ensure!(
        layout["sheets"]
            .as_object()
            .context("layout sheets missing")?
            .len()
            == new_sheets.len(),
        "layout sheet coverage mismatch"
    );
    for (index, source) in new_sheets.iter().enumerate() {
        let sheet = &mut layout["sheets"][format!("sheet-{}", index + 1)];
        ensure!(
            sheet["name"] == source["name"],
            "layout sheet order changed"
        );
        // The deletions are applied: their rows and columns are gone.
        if let Some(sheet) = sheet.as_object_mut() {
            sheet.remove(WITHDRAWN);
        }
        for (axis, rows) in [("rows", true), ("columns", false)] {
            for entry in sheet[axis]
                .as_object_mut()
                .context("layout axis missing")?
                .values_mut()
            {
                let position = u32::try_from(
                    entry["position"]
                        .as_u64()
                        .context("layout position missing")?,
                )?;
                entry["key"] = json!(if rows {
                    format!("r{position}")
                } else {
                    excel::column_name(position)?
                });
            }
        }
        for (axis, rows) in [("rows", true), ("columns", false)] {
            let entries = sheet[axis].as_object_mut().context("layout axis missing")?;
            let mut held: BTreeSet<u64> = entries
                .values()
                .filter_map(|e| e["position"].as_u64())
                .collect();
            for cell in array(&source["cells"])?
                .iter()
                .filter(|cell| !trusted || cell["type"] == "null")
            {
                let (column, row) = excel::coordinate(string(&cell["address"])?)?;
                let position = if rows { row } else { column };
                if held.insert(u64::from(position)) {
                    let key = name_of(position, rows)?;
                    let id = fresh_identity(entries, &key);
                    entries.insert(id, json!({"key":key,"position":position}));
                }
            }
            // Every row and column before the last keeps an identity, as from
            // the start. A cell holding a value where the trusted path placed
            // none is left without one: its correspondence is not proven.
            let unproven: BTreeSet<u64> = array(&source["cells"])?
                .iter()
                .filter(|cell| trusted && cell["type"] != "null")
                .map(|cell| {
                    let (column, row) = excel::coordinate(string(&cell["address"])?)?;
                    Ok(u64::from(if rows { row } else { column }))
                })
                .collect::<Result<_>>()?;
            let last = held.last().copied().unwrap_or(0);
            for position in 1..=last {
                if !unproven.contains(&position) && held.insert(position) {
                    let position = u32::try_from(position)?;
                    let key = name_of(position, rows)?;
                    let id = fresh_identity(entries, &key);
                    entries.insert(id, json!({"key":key,"position":position}));
                }
            }
        }
        let identities = |axis: &str| -> Result<BTreeMap<u64, String>> {
            sheet[axis]
                .as_object()
                .context("layout axis missing")?
                .iter()
                .map(|(id, e)| {
                    Ok((
                        e["position"].as_u64().context("layout position missing")?,
                        id.clone(),
                    ))
                })
                .collect()
        };
        let rows = identities("rows")?;
        let columns = identities("columns")?;
        let mut formula_cells = BTreeSet::new();
        for cell in array(&source["cells"])? {
            let (column, row) = excel::coordinate(string(&cell["address"])?)?;
            let (Some(row), Some(column)) =
                (rows.get(&u64::from(row)), columns.get(&u64::from(column)))
            else {
                bail!("layout does not cover imported cell; correspondence not proven");
            };
            if cell["type"] == "formula" {
                formula_cells.insert((row.clone(), column.clone()));
            }
        }
        // The formulas of this version: a cell keeps its formula identity, one
        // that holds a value now loses it, and a formula the writer added, such
        // as a table's calculated column in an inserted row, takes a new one.
        let formulas = sheet["formulas"]
            .as_object_mut()
            .context("layout formulas missing")?;
        formulas.retain(|_, target| {
            formula_cells.remove(&(
                target["row"].as_str().unwrap_or("").to_owned(),
                target["column"].as_str().unwrap_or("").to_owned(),
            ))
        });
        for (row, column) in formula_cells {
            let id = elements::token("formula", &[&row, &column]);
            ensure!(
                !formulas.contains_key(&id),
                "formula identity {id} already used"
            );
            formulas.insert(id, json!({"row":row,"column":column}));
        }
    }
    check_axes(&layout)?;
    Ok(layout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_records_are_stored_as_lines_of_identities() {
        let cell = |row: &str, column: &str| json!({"id": elements::cell_id(row, column)});
        let mut marked = cell("r2", "ins-1");
        marked["font"] = json!(0);
        let mut formula = cell("r2", "D");
        formula["formula"] = Value::Null;
        let metadata = json!([{"type":"text","id":"cell_r1_A"},{"type":"table","rows":[
            {"cells":[cell("r1", "A"), cell("r1", "B"), cell("r1", "C"), cell("r1", "E")]},
            {"cells":[cell("r2", "A"), cell("r2", "B"), marked, formula, cell("r2", "F"), cell("r2", "G"), cell("r2", "H")]}]}]);
        let mut stored = metadata.clone();
        stored_tables(&mut stored).unwrap();
        assert_eq!(
            stored[1],
            json!({"type":"table","rows":["r1 A..C E","r2 A B ins-1 D F..H"],
                "cells":{"cell_r2_ins-1":{"font":0},"cell_r2_D":{"formula":null}}})
        );
        expand_tables(&mut stored).unwrap();
        assert_eq!(stored, metadata);
        let mut two_rows =
            json!([{"type":"table","rows":[{"cells":[cell("r1", "A"), cell("r2", "B")]}]}]);
        assert!(stored_tables(&mut two_rows).is_err());
        let mut stray = json!([{"type":"table","rows":["r1 A"],"cells":{"cell_r2_A":{"font":0}}}]);
        assert!(expand_tables(&mut stray).is_err());
    }

    #[test]
    fn axes_are_stored_as_runs_of_identities_by_their_keys() {
        let entries = |pairs: &[(&str, &str)]| {
            Value::Object(
                pairs
                    .iter()
                    .map(|(id, key)| (id.to_string(), json!({ "key": key })))
                    .collect(),
            )
        };
        let rows = entries(&[
            ("r1", "r1"),
            ("r2", "r2"),
            ("ins-1", "r3"),
            ("r3", "r4"),
            ("r4", "r5"),
            ("r6", "r6"),
            ("r5_1", "r7"),
            ("pending-1", "pending-1"),
        ]);
        let stored = stored_axis(rows.as_object().unwrap(), true).unwrap();
        assert_eq!(
            stored,
            json!(["r1..r2", "ins-1=r3", "r3..r4 +1", "r6", "r5_1=r7"])
        );
        let mut kept = rows.clone();
        kept.as_object_mut().unwrap().remove("pending-1");
        assert_eq!(expand_axis(&stored, true).unwrap(), kept);
        let columns = entries(&[("C", "A"), ("A", "B"), ("B", "C"), ("Z", "AA")]);
        let stored = stored_axis(columns.as_object().unwrap(), false).unwrap();
        assert_eq!(stored, json!(["C -2", "A..B +1", "Z +1"]));
        assert_eq!(expand_axis(&stored, false).unwrap(), columns);
        assert!(expand_axis(&json!(["r1 -1"]), true).is_err());
        assert!(expand_axis(&json!(["r1..r2", "r2"]), true).is_err());
    }

    #[test]
    fn deleting_first_row_keeps_original_and_inserted_columns() {
        let extraction = json!({"document_id":"book.xlsx","source":{"sha256":"hash"},
        "sheets":[{"name":"Sheet1","merges":[],"cells":[
            {"address":"A1","type":"string"},{"address":"B2","type":"string"}
        ]}]});
        let mut layout = initial(&extraction).unwrap();
        let operations = excel::parse_operations(
            &[
                json!({"id":"add","kind":"insert_columns","sheet":"Sheet1","at":2,"count":1,"reason":"test"}),
                json!({"id":"delete","kind":"delete_rows","sheet":"Sheet1","at":1,"count":1,"reason":"test"}),
            ],
            extraction["sheets"].as_array().unwrap(),
        ).unwrap();
        update(&mut layout, &operations).unwrap();
        let sheet = &layout["sheets"]["sheet-1"];
        let position = |axis: &str, id: &str| sheet[axis][id]["position"].clone();
        assert_eq!(
            ["A", "add-1", "B"].map(|id| position("columns", id)),
            [json!(1), json!(2), json!(3)]
        );
        assert!(sheet["rows"].get("r1").is_none());
        assert_eq!(position("rows", "r2"), 1);
    }
}
