//! Coordinate-free editable document bodies and their separately stored bindings.
use super::*;

pub(super) fn token(kind: &str, parts: &[&str]) -> String {
    // Keep the immutable identity components readable. Escape the separator
    // and escape marker so user-supplied operation IDs cannot alias each other.
    let parts: Vec<String> = parts
        .iter()
        .map(|part| {
            part.bytes()
                .map(|byte| match byte {
                    b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'.' | b'/' | b'#' => {
                        char::from(byte).to_string()
                    }
                    _ => format!("%{byte:02X}"),
                })
                .collect()
        })
        .collect();
    format!("{kind}_{}", parts.join("_"))
}
pub(super) fn cell_id(row: &str, column: &str) -> String {
    token("cell", &[row, column])
}

/// The row and column identities the cell `id` names, or `None` for an ID that
/// names no cell.
pub(super) fn cell_axes(id: &str) -> Option<(String, String)> {
    match parts(id)? {
        ("cell", mut parts) if parts.len() == 2 => {
            let column = parts.pop()?;
            Some((parts.pop()?, column))
        }
        _ => None,
    }
}

/// The kind and the parts of an ID that [`token`] made, or `None` for any
/// other string.
fn parts(id: &str) -> Option<(&str, Vec<String>)> {
    let (kind, rest) = id.split_once('_')?;
    let parts = rest
        .split('_')
        .map(|part| {
            let mut bytes = vec![];
            let mut rest = part.as_bytes();
            while let Some((&byte, after)) = rest.split_first() {
                if byte == b'%' {
                    let hex = std::str::from_utf8(after.get(..2)?).ok()?;
                    bytes.push(u8::from_str_radix(hex, 16).ok()?);
                    rest = &after[2..];
                } else {
                    bytes.push(byte);
                    rest = after;
                }
            }
            String::from_utf8(bytes).ok()
        })
        .collect::<Option<Vec<_>>>()?;
    let refs: Vec<&str> = parts.iter().map(String::as_str).collect();
    (token(kind, &refs) == id).then_some((kind, parts))
}

/// Where the elements of a page write in its grid, read from their IDs: a
/// shape's text, a formula of the sheet, or a cell of the body.
pub(super) struct Bindings {
    formulas: HashMap<(String, String), String>,
}

impl Bindings {
    pub(super) fn of(sheet: &Value) -> Result<Self> {
        let mut formulas = HashMap::new();
        for (id, target) in sheet["formulas"]
            .as_object()
            .context("layout formulas missing")?
        {
            let at = (
                string(&target["row"])?.to_owned(),
                string(&target["column"])?.to_owned(),
            );
            ensure!(
                formulas.insert(at, id.clone()).is_none(),
                "two formulas in one cell"
            );
        }
        Ok(Self { formulas })
    }

    /// The ID of the element at `row` and `column` of `block`, the inverse of
    /// [`Bindings::get`].
    pub(super) fn id(sheet: &Value, block: &str, row: &str, column: &str) -> Result<String> {
        Ok(match block {
            "shapes" => token("shape", &[row, column]),
            "formulas" => {
                let target = &sheet["formulas"][row];
                cell_id(string(&target["row"])?, string(&target["column"])?)
            }
            "table-1" => cell_id(row, column),
            _ => bail!("block {block} has no element IDs"),
        })
    }

    /// The block, row and column of the element `id`.
    pub(super) fn get(&self, id: &str) -> Result<(String, String, String)> {
        let (kind, mut parts) = parts(id).with_context(|| format!("unknown element ID {id}"))?;
        ensure!(parts.len() == 2, "unknown element ID {id}");
        let column = parts.pop().unwrap();
        let row = parts.pop().unwrap();
        Ok(match kind {
            "shape" => ("shapes".into(), row, column),
            "cell" => match self.formulas.get(&(row.clone(), column.clone())) {
                Some(formula) => ("formulas".into(), formula.clone(), "formula".into()),
                None => ("table-1".into(), row, column),
            },
            _ => bail!("unknown element ID {id}"),
        })
    }
}

/// The fonts of a page's elements, each stored once in the sheet's `fonts`
/// and referred to by its index.
#[derive(Default)]
pub(super) struct Fonts {
    pub(super) list: Vec<Value>,
    index: HashMap<String, usize>,
}

impl Fonts {
    pub(super) fn add(&mut self, font: &Value) -> Value {
        let key = serde_json::to_string(font).unwrap();
        let next = self.list.len();
        let index = *self.index.entry(key).or_insert(next);
        if index == next {
            self.list.push(font.clone());
        }
        json!(index)
    }
}

/// The font an element or table cell of `sheet`'s metadata refers to, null
/// for none.
pub(super) fn font(sheet: &Value, element: &Value) -> Result<Value> {
    if element["font"].is_null() {
        return Ok(Value::Null);
    }
    let index = element["font"]
        .as_u64()
        .context("font index must be a number")?;
    let font = usize::try_from(index)
        .ok()
        .and_then(|index| sheet["fonts"].get(index))
        .with_context(|| format!("unknown font index {index}"))?;
    Ok(font.clone())
}

/// The table of every table cell, as the element `metadata` of a page records
/// it.
fn groups(metadata: &Value) -> Result<Value> {
    let mut groups = serde_json::Map::new();
    for element in array(metadata)? {
        if element["type"] != "table" {
            continue;
        }
        for row in array(&element["rows"])? {
            for cell in array(&row["cells"])? {
                groups.insert(
                    string(&cell["id"])?.to_owned(),
                    json!({"table":element["id"]}),
                );
            }
        }
    }
    Ok(Value::Object(groups))
}

/// The digest of what [`classify`] takes from the elements of a structure
/// interpretation: each table's sheet and its cells. A layout classified from
/// another interpretation, such as one an earlier inference made, has
/// another.
pub(super) fn interpretation_digest(structure: &[Value]) -> Result<String> {
    let mut tables = vec![];
    for element in structure.iter().filter(|e| e["kind"] == "table") {
        let cells: Vec<Value> = array(&element["cells"])?
            .iter()
            .map(|cell| json!({"id":cell["id"],"address":cell["address"]}))
            .collect();
        tables.push(json!({"sheet":element["sheet"],"cells":cells}));
    }
    Ok(crate::data::hash_encoded(&json!(tables)))
}

/// Puts in each table cell of the element metadata of `layout` the role and
/// headers `structure`, an interpretation of the extraction the layout belongs
/// to, gives it: the layout keeps only which table a cell is in. Cells are
/// matched by the keys their rows and columns have in the extraction, and a
/// cell's headers are those in the same table of the metadata. A cell the
/// interpretation does not place in a table, such as one of a row an
/// insertion adds, is `unassigned`.
pub(super) fn interpret(layout: &mut Value, structure: &[Value]) -> Result<()> {
    for sheet in layout["sheets"]
        .as_object_mut()
        .context("layout sheets missing")?
        .values_mut()
    {
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
        let mut ids = HashMap::new();
        for element in structure.iter().filter(|e| e["sheet"] == sheet["name"]) {
            for cell in array(&element["cells"])? {
                let (column, row) = excel::coordinate(string(&cell["address"])?)?;
                if let (Some(row), Some(column)) = (
                    rows.get(&format!("r{row}")),
                    columns.get(&excel::column_name(column)?),
                ) {
                    ids.insert(string(&cell["id"])?.to_owned(), cell_id(row, column));
                }
            }
        }
        let mut found: HashMap<String, (Value, Vec<String>)> = HashMap::new();
        for element in structure
            .iter()
            .filter(|e| e["sheet"] == sheet["name"] && e["kind"] == "table")
        {
            for cell in array(&element["cells"])? {
                if let Some(id) = ids.get(string(&cell["id"])?) {
                    let headers = array(&cell["headers"])?
                        .iter()
                        .filter_map(|h| h.as_str().and_then(|h| ids.get(h)))
                        .cloned()
                        .collect();
                    found.insert(id.clone(), (cell["role"].clone(), headers));
                }
            }
        }
        for element in sheet["element_metadata"]
            .as_array_mut()
            .context("element metadata missing")?
            .iter_mut()
            .filter(|e| e["type"] == "table")
        {
            let rows = element["rows"]
                .as_array_mut()
                .context("table rows missing")?;
            let mut present = BTreeSet::new();
            for row in rows.iter() {
                for cell in array(&row["cells"])? {
                    present.insert(string(&cell["id"])?.to_owned());
                }
            }
            for row in rows {
                for cell in row["cells"].as_array_mut().context("table cells missing")? {
                    let (role, mut headers) = found
                        .get(string(&cell["id"])?)
                        .cloned()
                        .unwrap_or_else(|| (json!("unassigned"), vec![]));
                    headers.retain(|h| present.contains(h));
                    cell["role"] = role;
                    cell["headers"] = json!(headers);
                }
            }
        }
    }
    Ok(())
}

/// Attach the interpreted table membership to immutable internal grid identities.
/// Text cells remain individually reversible: no lossy concatenation is performed.
/// The layout records the [`interpretation_digest`] of `structure`; the roles
/// and headers of the cells are not kept, [`interpret`] gives them.
pub(super) fn classify(layout: &mut Value, structure: &[Value]) -> Result<()> {
    layout["structure_sha256"] = json!(interpretation_digest(structure)?);
    for sheet in layout["sheets"]
        .as_object_mut()
        .context("layout sheets missing")?
        .values_mut()
    {
        let mut groups = json!({});
        let positions = |axis: &str| -> Result<BTreeMap<u64, String>> {
            sheet[axis]
                .as_object()
                .context("layout axis missing")?
                .iter()
                .map(|(id, e)| {
                    Ok((
                        e["position"].as_u64().context("position missing")?,
                        id.clone(),
                    ))
                })
                .collect()
        };
        let rows = positions("rows")?;
        let columns = positions("columns")?;
        let mut ids = BTreeMap::new();
        for element in structure.iter().filter(|e| e["sheet"] == sheet["name"]) {
            for cell in array(&element["cells"])? {
                let (column, row) = excel::coordinate(string(&cell["address"])?)?;
                if let (Some(row), Some(column)) =
                    (rows.get(&u64::from(row)), columns.get(&u64::from(column)))
                {
                    ids.insert(string(&cell["id"])?.to_owned(), cell_id(row, column));
                }
            }
        }
        for element in structure
            .iter()
            .filter(|e| e["sheet"] == sheet["name"] && e["kind"] == "table")
        {
            let cells = array(&element["cells"])?;
            let Some(first) = cells.iter().find_map(|c| ids.get(c["id"].as_str()?)) else {
                continue;
            };
            let table = token("table", &[first]);
            for cell in cells {
                if let Some(id) = ids.get(string(&cell["id"])?) {
                    groups[id] = json!({"table":table});
                }
            }
        }
        sheet["groups"] = groups;
    }
    Ok(())
}

pub(super) fn geometry(
    layout: &mut Value,
    extraction: &Value,
    operations: &[excel::StructuralOperation],
) -> Result<()> {
    for (i, source) in array(&extraction["sheets"])?.iter().enumerate() {
        layout["sheets"][identity::page_id(source, i)]["merges"] =
            json!(excel::merges_after(source, operations)?);
    }
    super::visual_elements::refresh(layout, extraction, operations)?;
    Ok(())
}

/// Puts in place in the element `metadata` of a layout `sheet` what follows
/// from the rest, as [`encode`] gives it: the IDs of tables and their records,
/// by their first cell and its row, and of the blanks between elements, by the
/// elements around them; each table cell's column, which its ID names; a
/// table's columns, in the order of their positions;
/// and, with `spans`, the logical rows and columns of its table that a merged
/// cell spans, a span of one being the cell itself. The positions are those of
/// the sheet's rows and columns, the merges its `merges`.
pub(super) fn complete(sheet: &Value, metadata: &mut Value, spans: bool) -> Result<()> {
    let position = |axis: &str, id: &str, missing: u64| {
        sheet[axis][id]["position"].as_u64().unwrap_or(missing)
    };
    let mut merged = BTreeMap::new();
    if spans {
        for range in array(&sheet["merges"])? {
            let (start, end) = string(range)?
                .split_once(':')
                .context("invalid merge range")?;
            let (x, y) = excel::coordinate(start)?;
            let (right, bottom) = excel::coordinate(end)?;
            merged.insert(
                (u64::from(y), u64::from(x)),
                (u64::from(bottom), u64::from(right)),
            );
        }
    }
    let elements = metadata
        .as_array_mut()
        .context("element metadata missing")?;
    for element in elements.iter_mut().filter(|e| e["type"] == "table") {
        let rows = element["rows"]
            .as_array_mut()
            .context("table rows missing")?;
        let mut cells = vec![];
        for row in rows.iter_mut() {
            let first = string(&row["cells"][0]["id"])?;
            let (record, _) =
                cell_axes(first).with_context(|| format!("table cell {first} names no cell"))?;
            row["id"] = json!(token("record", &[&record]));
            for cell in row["cells"].as_array_mut().context("table cells missing")? {
                let id = string(&cell["id"])?;
                let (row, column) =
                    cell_axes(id).with_context(|| format!("table cell {id} names no cell"))?;
                let at = (
                    position("rows", &row, u64::MAX),
                    position("columns", &column, 0),
                );
                cell["column"] = json!(token("column", &[&column]));
                cells.push((at, cell));
            }
        }
        let logical_rows: BTreeSet<u64> = cells.iter().map(|((y, _), _)| *y).collect();
        let logical_columns: BTreeMap<u64, Value> = cells
            .iter()
            .map(|((_, x), cell)| (*x, json!({"id":cell["column"]})))
            .collect();
        for ((y, x), cell) in cells {
            let cell = cell
                .as_object_mut()
                .context("table cell must be an object")?;
            cell.remove("row_span");
            cell.remove("column_span");
            if let Some((bottom, right)) = merged.get(&(y, x)) {
                for (key, count) in [
                    ("row_span", logical_rows.range(y..=*bottom).count()),
                    ("column_span", logical_columns.range(x..=*right).count()),
                ] {
                    if count > 1 {
                        cell.insert(key.into(), json!(count));
                    }
                }
            }
        }
        element["columns"] = json!(logical_columns.into_values().collect::<Vec<_>>());
        let first = string(&element["rows"][0]["cells"][0]["id"])?.to_owned();
        element["id"] = json!(token("table", &[&first]));
    }
    for at in 0..elements.len() {
        if elements[at]["type"] == "blank" {
            let neighbour = |at: Option<usize>| -> Result<String> {
                let element = at
                    .and_then(|at| elements.get(at))
                    .context("a blank lies between two elements")?;
                Ok(string(&element["id"])?.to_owned())
            };
            let id = token(
                "gap",
                &[&neighbour(at.checked_sub(1))?, &neighbour(Some(at + 1))?],
            );
            elements[at]["id"] = json!(id);
        }
    }
    Ok(())
}

/// Encode the writer's private grid into reading-order elements, updating only
/// the separate layout record with physical targets and a checked body shape.
pub(super) fn encode(page: &Value, layout: &mut Value) -> Result<Value> {
    let grid = identity::grid(page.clone(), layout, false)?;
    let page_id = string(&page["page_id"])?;
    let sheet = &mut layout["sheets"][page_id];
    // Not stored: the interpretation the metadata records, unless classify
    // has just set a new one.
    if sheet.get("groups").is_none() {
        sheet["groups"] = groups(&sheet["element_metadata"])?;
    }
    let mut spans = BTreeMap::new();
    for range in array(&sheet["merges"])? {
        let (start, end) = string(range)?
            .split_once(':')
            .context("invalid merge range")?;
        let (x, y) = excel::coordinate(start)?;
        let (right, bottom) = excel::coordinate(end)?;
        spans.insert(
            (u64::from(y), u64::from(x)),
            (u64::from(bottom), u64::from(right)),
        );
    }
    // Pending insertions inside an existing table stay in that table, the
    // innermost when tables are nested. Their semantic role needs
    // interpretation; do not copy a neighbouring header.
    let mut bounds: BTreeMap<String, (u64, u64, u64, u64)> = BTreeMap::new();
    if let Some(rows) = grid["blocks"]["table-1"]["rows"].as_object() {
        for (row, values) in rows {
            let y = sheet["rows"][row]["position"]
                .as_u64()
                .context("row position missing")?;
            for column in values.as_object().context("grid row missing")?.keys() {
                let x = sheet["columns"][column]["position"]
                    .as_u64()
                    .context("column position missing")?;
                if let Some(table) = sheet["groups"][cell_id(row, column)]["table"].as_str() {
                    let b = bounds.entry(table.to_owned()).or_insert((y, x, y, x));
                    b.0 = b.0.min(y);
                    b.1 = b.1.min(x);
                    b.2 = b.2.max(y);
                    b.3 = b.3.max(x);
                }
            }
        }
        for (row, values) in rows {
            let y = sheet["rows"][row]["position"].as_u64().unwrap();
            for column in values.as_object().unwrap().keys() {
                let inserted = [(&sheet["rows"], row), (&sheet["columns"], column)]
                    .iter()
                    .any(|(axis, id)| axis[*id]["key"].as_str().is_some_and(|k| k.contains('-')));
                let id = cell_id(row, column);
                if !inserted || !sheet["groups"][&id].is_null() {
                    continue;
                }
                let x = sheet["columns"][column]["position"].as_u64().unwrap();
                // A table nested in another lies inside its bounds; the
                // innermost one around the insertion takes it.
                let area = |b: &(u64, u64, u64, u64)| (b.2 - b.0 + 1) * (b.3 - b.1 + 1);
                let around: Vec<_> = bounds
                    .iter()
                    .filter(|(_, b)| b.0 <= y && y <= b.2 && b.1 <= x && x <= b.3)
                    .collect();
                let smallest = around.iter().map(|(_, b)| area(b)).min();
                let candidates: Vec<_> = around
                    .into_iter()
                    .filter(|(_, b)| Some(area(b)) == smallest)
                    .collect();
                if let [(table, _)] = candidates.as_slice() {
                    sheet["groups"][&id] = json!({"table":table});
                }
            }
        }
    }
    let derived = Bindings::of(sheet)?;
    let mut ids = BTreeSet::new();
    let mut fonts = Fonts::default();
    let mut placed: Vec<((u64, u64), u64, Value)> = vec![];
    type TableCell = (u64, u64, Value);
    let mut tables: BTreeMap<String, Vec<TableCell>> = BTreeMap::new();
    for (block, body) in grid["blocks"].as_object().context("grid blocks missing")? {
        let Some(rows) = body["rows"].as_object() else {
            continue;
        };
        for (row, values) in rows {
            for (column, value) in values.as_object().context("grid row missing")? {
                let (axis_row, axis_column) = if block == "formulas" {
                    let target = &sheet["formulas"][row];
                    (
                        string(&target["row"])?.to_owned(),
                        string(&target["column"])?.to_owned(),
                    )
                } else {
                    (row.clone(), column.clone())
                };
                let id = if block == "shapes" {
                    token("shape", &[row, column])
                } else {
                    cell_id(&axis_row, &axis_column)
                };
                ensure!(ids.insert(id.clone()), "duplicate element binding");
                // Not stored: decode reads the binding back from the ID.
                ensure!(
                    derived.get(&id)? == (block.clone(), row.clone(), column.clone()),
                    "element ID {id} does not name its place in block {block}"
                );
                let mut y = sheet["rows"][&axis_row]["position"]
                    .as_u64()
                    .unwrap_or(u64::MAX);
                let mut x = sheet["columns"][&axis_column]["position"]
                    .as_u64()
                    .unwrap_or(0);
                let visual = if block == "shapes" {
                    sheet["visuals"]
                        .as_object()
                        .context("visual bindings missing")?
                        .iter()
                        .find(|(_, v)| v["text_key"] == *row)
                        .map(|(id, v)| (id.clone(), v.clone()))
                } else {
                    None
                };
                if let Some((_, v)) = &visual {
                    let position = super::visual_elements::position(v);
                    y = position.0;
                    x = position.1;
                }
                let group = &sheet["groups"][&id];
                let font = grid["fonts"][block.as_str()][row.as_str()][column.as_str()].clone();
                let mut cell = json!({"id":id});
                let field = if block == "formulas" {
                    "formula"
                } else {
                    "value"
                };
                cell[field] = value.clone();
                if !font.is_null() {
                    cell["font"] = fonts.add(&font);
                }
                if let Some(table) = group["table"].as_str() {
                    tables
                        .entry(table.to_owned())
                        .or_default()
                        .push((y, x, cell));
                } else {
                    let mut element = if block == "formulas" {
                        json!({"id":id,"type":"formula","formula":value})
                    } else {
                        json!({"id":id,"type":"text","text":value})
                    };
                    if let Some((reference, _)) = visual {
                        element["visual"] = json!(reference);
                    }
                    if !font.is_null() {
                        element["font"] = fonts.add(&font);
                    }
                    placed.push(((y, x), spans.get(&(y, x)).map_or(y, |s| s.0), element));
                }
            }
        }
    }
    for mut cells in tables.into_values() {
        cells.sort_by_key(|(y, x, _)| (*y, *x));
        let first = (cells[0].0, cells[0].1);
        let end = cells
            .iter()
            .map(|c| spans.get(&(c.0, c.1)).map_or(c.0, |s| s.0))
            .max()
            .unwrap();
        // Named as [`complete`] names it, by its first cell: the blank after
        // the table is named by it.
        let id = token("table", &[string(&cells[0].2["id"])?]);
        let mut rows: BTreeMap<u64, Value> = BTreeMap::new();
        for (y, _, cell) in cells {
            let row = rows.entry(y).or_insert_with(|| json!({"cells":[]}));
            row["cells"].as_array_mut().unwrap().push(cell);
        }
        placed.push((
            first,
            end,
            json!({"id":id,"type":"table","rows":rows.into_values().collect::<Vec<_>>()}),
        ));
    }
    for (reference, visual) in sheet["visuals"]
        .as_object()
        .context("visual bindings missing")?
    {
        let position = super::visual_elements::position(visual);
        let end = visual["anchor"]["to"]["row"]
            .as_u64()
            .and_then(|n| n.checked_add(1))
            .unwrap_or(position.0)
            .max(position.0);
        placed.push((
            position,
            end,
            json!({"id":reference,"type":visual["kind"],"ref":reference}),
        ));
    }
    placed.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| b.2.get("ref").is_some().cmp(&a.2.get("ref").is_some()))
            .then_with(|| a.2["id"].as_str().cmp(&b.2["id"].as_str()))
    });
    let mut elements = vec![];
    // The last row the elements so far end on.
    let mut previous: Option<u64> = None;
    for ((y, _), end, element) in placed {
        if let Some(last) = previous
            && y != u64::MAX
            && last != u64::MAX
            && y > last.saturating_add(1)
        {
            elements.push(json!({"type":"blank"}));
        }
        previous = Some(previous.map_or(end, |last| last.max(end)));
        elements.push(element);
    }
    let mut out = identity::copy_except(page, &[&["blocks"], &["fonts"]]);
    ensure!(out.is_object(), "page must be an object");
    out["schema_version"] = json!("4");
    let mut elements = Value::Array(elements);
    complete(sheet, &mut elements, true)?;
    let (body, metadata) = crate::document_body::split(&elements)?;
    out["elements"] = body;
    sheet["element_metadata"] = metadata;
    sheet["fonts"] = Value::Array(fonts.list);
    sheet["notes"] = grid["blocks"]["extraction-notes"].clone();
    sheet["block_titles"] = Value::Object(
        grid["blocks"]
            .as_object()
            .unwrap()
            .iter()
            .filter(|(_, body)| body["rows"].is_object())
            .map(|(id, body)| (id.clone(), body["title"].clone()))
            .collect(),
    );
    validate("content", &out)?;
    Ok(out)
}

/// Bind values to recorded slots. Shape changes are refused; same-shape manual
/// permutations are value edits and cannot move the recorded identities.
pub(super) fn decode(page: &Value, layout: &Value) -> Result<Value> {
    ensure!(
        page["schema_version"] == "4",
        "Document content must use document elements; re-import the original"
    );
    let sheet = &layout["sheets"][string(&page["page_id"])?];
    // The body is read beside its metadata rather than joined into a copy:
    // its shape is checked first, as joining it would.
    let metadata = &sheet["element_metadata"];
    crate::document_body::each(&page["elements"], metadata, |_, _| Ok(()))?;
    super::visual_elements::validate_references(metadata, sheet)?;
    let mut out = identity::copy_except(page, &[&["elements"]]);
    ensure!(out.is_object(), "page must be an object");
    out["blocks"] = json!({"extraction-notes":sheet["notes"]});
    for (block, title) in sheet["block_titles"]
        .as_object()
        .context("block titles missing")?
    {
        out["blocks"][block] = json!({"title":title,"rows":{}});
    }
    let bindings = Bindings::of(sheet)?;
    let mut used = BTreeSet::new();
    let mut put = |id: &str, value: &Value, element: &Value| -> Result<()> {
        ensure!(used.insert(id.to_owned()), "duplicate element cell ID");
        let (block, row, column) = bindings.get(id)?;
        let (block, row, column) = (block.as_str(), row.as_str(), column.as_str());
        let font = font(sheet, element)?;
        if out["blocks"][block].is_null() {
            out["blocks"][block] = json!({"title":match block {"table-1"=>"本文","formulas"=>"数式原文",_=>"図形の文字"},"rows":{}});
        }
        out["blocks"][block]["rows"][row][column] = value.clone();
        if !font.is_null() {
            out["fonts"][block][row][column] = font;
        }
        Ok(())
    };
    crate::document_body::each(&page["elements"], metadata, |element, value| {
        put(string(&element["id"])?, value, element)
    })?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::token;

    #[test]
    fn readable_ids_keep_component_boundaries_and_literal_escapes_distinct() {
        let inputs = [
            vec!["add_row-1", "B"],
            vec!["add", "row-1_B"],
            vec!["add%5Frow-1", "B"],
            vec!["", "A"],
            vec!["_A"],
            vec!["追加-1", "B"],
        ];
        let ids: std::collections::BTreeSet<_> =
            inputs.iter().map(|parts| token("cell", parts)).collect();
        assert_eq!(ids.len(), inputs.len());
        assert_eq!(token("cell", &["r7", "B"]), "cell_r7_B");
    }
}
