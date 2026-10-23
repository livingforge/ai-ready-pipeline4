//! Bodies and their value-free layout metadata. A table body is the tree of
//! its records: each record a node keyed by the headings over its columns,
//! under the record whose names head it or the category over it, a node of
//! its own holding cells of its first record. Positions in the tree and keys are
//! editing slots, not identities: moving values by hand is an ordinary value
//! edit, and the tree's shape and keys follow from the metadata.
use crate::data::{array, string};
use crate::documents::{Meta, MetaCell};
use anyhow::{Context, Result, bail, ensure};
use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;

/// The member of a node holding the values of its record's cells that head
/// nothing and are headed by nothing, such as the marks drawing a tree.
const MARKS: &str = "marks";
/// The member of a node holding the nodes of the records it heads.
const CHILDREN: &str = "children";
/// The value of a blank cell under a category, which no node shows.
static BLANK: Value = Value::Null;

/// A node of the tree of a table: a record, by its place among the table's
/// records, or a category, by its place among the table's categories.
#[derive(Clone, Copy)]
enum Slot {
    Record(usize),
    Category(usize),
}

/// A category of a table: cells of its first record that head the records
/// under it, shown as a node of their own with those records as children,
/// under a record or a category, or at the root.
struct Category<'a> {
    key: &'a str,
    /// Its cells, in the order of the table's metadata.
    cells: Vec<&'a MetaCell>,
    /// The record holding its cells.
    record: usize,
    parent: Option<&'a str>,
    children: Vec<Slot>,
}

/// The records of a table element of the metadata: its first `headings`
/// records are its column headings and the rest its tree, each record under
/// its `parent`, a record or one of its `categories`.
struct Table<'a> {
    element: &'a Value,
    rows: &'a [crate::documents::MetaRow],
    headings: usize,
    roots: Vec<Slot>,
    children: Vec<Vec<Slot>>,
    categories: Vec<Category<'a>>,
    /// The IDs of the cells of categories, which no record node holds.
    categorized: BTreeSet<&'a str>,
}

impl<'a> Table<'a> {
    fn of(table: &'a Meta) -> Result<Self> {
        let rows = table.records().context("table metadata rows missing")?;
        let element = table.fields().context("table metadata missing")?;
        let headings = match &element["headings"] {
            Value::Null => 0,
            n => usize::try_from(n.as_u64().context("table headings must be a count")?)?,
        };
        ensure!(
            headings <= rows.len(),
            "table has fewer records than headings"
        );
        let mut index = HashMap::new();
        let mut cells = HashMap::new();
        for (i, row) in rows.iter().enumerate() {
            if let Some(id) = row["id"].as_str() {
                index.insert(id, i);
            }
            for cell in &row.cells {
                cells.insert(string(&cell["id"])?, (i, cell));
            }
        }
        let mut categories = vec![];
        let mut named = HashMap::new();
        let mut categorized = BTreeSet::new();
        if let Some(map) = element.get("categories") {
            let map = map.as_object().context("table categories must be a map")?;
            for (key, entry) in map {
                let mut ids = vec![key.as_str()];
                for id in entry["cells"].as_array().into_iter().flatten() {
                    ids.push(string(id)?);
                }
                let mut held = vec![];
                let mut record = None;
                for id in ids {
                    let &(i, cell) = cells
                        .get(id)
                        .with_context(|| format!("category cell {id} is no cell of the table"))?;
                    ensure!(
                        i >= headings && *record.get_or_insert(i) == i && !is_mark(cell),
                        "the cells of category {key} must be cells of one record"
                    );
                    ensure!(categorized.insert(id), "cell {id} is in two categories");
                    held.push(cell);
                }
                named.insert(key.as_str(), categories.len());
                categories.push(Category {
                    key,
                    cells: held,
                    record: record.context("category without cells")?,
                    parent: entry["parent"].as_str(),
                    children: vec![],
                });
            }
        }
        for &(_, cell) in cells.values() {
            if let Some(key) = cell.get("category") {
                let key = key.as_str().context("a cell's category must be its key")?;
                ensure!(
                    named.contains_key(key),
                    "cell {} is under no category {key}",
                    string(&cell["id"])?
                );
            }
        }
        let mut roots = vec![];
        let mut children = vec![vec![]; rows.len()];
        let mut placed = vec![false; categories.len()];
        for (i, row) in rows.iter().enumerate() {
            match row["parent"].as_str() {
                None => {
                    if i >= headings {
                        roots.push(Slot::Record(i));
                    }
                }
                Some(parent) if named.contains_key(parent) => {
                    ensure!(i >= headings, "a heading record is in a category");
                    // Places a category, and those around it, at its first record.
                    let mut chain = vec![];
                    let mut at = Some(named[parent]);
                    while let Some(c) = at.filter(|&c| !placed[c]) {
                        ensure!(chain.len() < categories.len(), "categories nest in a loop");
                        ensure!(
                            categories[c].record == i,
                            "category {} lies after its first record",
                            categories[c].key
                        );
                        chain.push(c);
                        at = categories[c].parent.and_then(|p| named.get(p).copied());
                    }
                    for &c in chain.iter().rev() {
                        match categories[c].parent {
                            Some(p) if named.contains_key(p) => {
                                categories[named[p]].children.push(Slot::Category(c));
                            }
                            Some(p) => {
                                let p = *index.get(p).with_context(|| {
                                    format!("parent {p} is no record or category of the table")
                                })?;
                                ensure!(
                                    headings <= p && p < i,
                                    "category {} lies before its parent",
                                    categories[c].key
                                );
                                children[p].push(Slot::Category(c));
                            }
                            None => roots.push(Slot::Category(c)),
                        }
                        placed[c] = true;
                    }
                    categories[named[parent]].children.push(Slot::Record(i));
                }
                Some(parent) => {
                    let p = *index
                        .get(parent)
                        .with_context(|| format!("parent {parent} is no record of the table"))?;
                    ensure!(
                        headings <= p && p < i,
                        "record {i} of a table lies before its parent"
                    );
                    children[p].push(Slot::Record(i));
                }
            }
        }
        if let Some(c) = placed.iter().position(|p| !p) {
            bail!("category {} heads no records", categories[c].key);
        }
        Ok(Self {
            element,
            rows,
            headings,
            roots,
            children,
            categories,
            categorized,
        })
    }

    /// The nodes under a node of the tree.
    fn under(&self, slot: Slot) -> &[Slot] {
        match slot {
            Slot::Record(record) => &self.children[record],
            Slot::Category(c) => &self.categories[c].children,
        }
    }

    /// Whether a cell of a record is shown in its record's node: neither a
    /// mark, nor a cell of a category or a blank cell under one.
    fn valued(&self, cell: &MetaCell) -> bool {
        !is_mark(cell)
            && cell.get("category").is_none()
            && !cell["id"]
                .as_str()
                .is_some_and(|id| self.categorized.contains(id))
    }

    fn cells(&self, record: usize) -> Result<&'a [MetaCell]> {
        Ok(&self.rows[record].cells)
    }

    /// The key of each cell of the tree that is no mark, by cell ID: the text
    /// of the heading the cell's own `label` names, or else the one the
    /// table's `labels` give its column, or the column's name for a column
    /// without one or with an empty heading. A heading
    /// shared by columns two of which hold cells of one record, or naming a
    /// member of a node, is followed by the column's name, as `備考(L)`.
    /// `value_of` gives the current value of a heading cell.
    fn keys<'v>(
        &self,
        value_of: impl Fn(&str) -> Option<&'v Value>,
    ) -> Result<HashMap<&'a str, Rc<str>>> {
        // The heading a cell takes its key from: its own, as a physical name
        // under a logical one in a column, or its column's.
        let heading = |cell: &'a MetaCell| -> Result<Option<&'a str>> {
            let column = string(&cell["column"])?;
            Ok(cell["label"]
                .as_str()
                .or_else(|| self.element["labels"][column].as_str()))
        };
        let mut texts: HashMap<Option<&str>, Option<String>> = HashMap::new();
        // Whether a column's cells take a heading's text, and whether one of
        // them names a member of a node.
        let mut headed: BTreeSet<&str> = BTreeSet::new();
        let mut qualified: BTreeSet<&str> = BTreeSet::new();
        for record in self.headings..self.rows.len() {
            for cell in self.cells(record)?.iter().filter(|c| keyed(c)) {
                let column = string(&cell["column"])?;
                let head = heading(cell)?;
                let text = texts
                    .entry(head)
                    .or_insert_with(|| head.and_then(&value_of).and_then(label_text));
                if text.is_some() {
                    headed.insert(column);
                }
                if matches!(text.as_deref(), Some(MARKS | CHILDREN)) {
                    qualified.insert(column);
                }
            }
        }
        // The key of the cells of a column under a heading, made once for
        // all of them: a long table has many records.
        let mut made: HashMap<(Option<&'a str>, &'a str, bool), Rc<str>> = HashMap::new();
        let mut key_of = |cell: &'a MetaCell, qualified: bool| -> Result<Rc<str>> {
            let column = string(&cell["column"])?;
            let head = heading(cell)?;
            if let Some(key) = made.get(&(head, column, qualified)) {
                return Ok(key.clone());
            }
            let name = column_name(column);
            let key: Rc<str> = match &texts[&head] {
                Some(text) if qualified => format!("{text}({name})"),
                Some(text) => text.clone(),
                None => name,
            }
            .into();
            made.insert((head, column, qualified), key.clone());
            Ok(key)
        };
        loop {
            let mut clashing = BTreeSet::new();
            for record in self.headings..self.rows.len() {
                let mut seen: HashMap<Rc<str>, &str> = HashMap::new();
                for cell in self.cells(record)?.iter().filter(|c| keyed(c)) {
                    let column = string(&cell["column"])?;
                    let k = key_of(cell, qualified.contains(column))?;
                    if let Some(other) = seen.insert(k, column) {
                        ensure!(other != column, "two cells of a record in one column");
                        clashing.extend([other, column]);
                    }
                }
            }
            let before = qualified.len();
            qualified.extend(clashing.iter().filter(|c| headed.contains(*c)));
            if clashing.is_empty() {
                break;
            }
            ensure!(
                qualified.len() > before,
                "columns {} cannot be told apart by their headings",
                clashing.iter().copied().collect::<Vec<_>>().join(", ")
            );
        }
        let mut keys = HashMap::new();
        for record in self.headings..self.rows.len() {
            for cell in self.cells(record)?.iter().filter(|c| keyed(c)) {
                let column = string(&cell["column"])?;
                keys.insert(
                    string(&cell["id"])?,
                    key_of(cell, qualified.contains(column))?,
                );
            }
        }
        Ok(keys)
    }

    /// The keys of `keys` in the order of the table's columns.
    fn key_order(&self, keys: &HashMap<&'a str, Rc<str>>) -> Result<Vec<String>> {
        let columns: HashMap<&str, usize> = array(&self.element["columns"])?
            .iter()
            .enumerate()
            .filter_map(|(i, c)| Some((c["id"].as_str()?, i)))
            .collect();
        let mut first: HashMap<&str, usize> = HashMap::new();
        for record in self.headings..self.rows.len() {
            for cell in self.cells(record)?.iter().filter(|c| keyed(c)) {
                let at = columns
                    .get(string(&cell["column"])?)
                    .copied()
                    .unwrap_or(usize::MAX);
                let key: &str = &keys[string(&cell["id"])?];
                let slot = first.entry(key).or_insert(at);
                *slot = (*slot).min(at);
            }
        }
        let mut order: Vec<_> = first.into_iter().collect();
        order.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(b.0)));
        Ok(order.into_iter().map(|(k, _)| k.to_owned()).collect())
    }
}

fn is_mark(cell: &MetaCell) -> bool {
    cell["mark"] == true
}

/// Whether a cell of a record has a key: neither a mark nor a blank cell
/// under a category.
fn keyed(cell: &MetaCell) -> bool {
    !is_mark(cell) && cell.get("category").is_none()
}

/// The text of a heading as a key: its words separated by single spaces, or
/// none for an empty heading.
fn label_text(value: &Value) -> Option<String> {
    let text = match value {
        Value::String(s) => s.split_whitespace().collect::<Vec<_>>().join(" "),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        _ => return None,
    };
    (!text.is_empty()).then_some(text)
}

/// The name of the column a logical column ID names, as `B` for `column_B`.
fn column_name(column: &str) -> String {
    let name = column.strip_prefix("column_").unwrap_or(column);
    let mut bytes = vec![];
    let mut rest = name.as_bytes();
    while let Some((&byte, after)) = rest.split_first() {
        let escaped = (byte == b'%')
            .then(|| after.get(..2))
            .flatten()
            .and_then(|hex| u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok());
        match escaped {
            Some(byte) => {
                bytes.push(byte);
                rest = &after[2..];
            }
            None => {
                bytes.push(byte);
                rest = after;
            }
        }
    }
    String::from_utf8(bytes).unwrap_or_else(|_| name.to_owned())
}

/// Separate editable values from identities, topology and source references.
/// The metadata keeps no place for the values, which [`join`] puts back
/// under the element's type or the cell's `value`; only a table cell holding
/// a formula keeps a null `formula`, the mark that its value is a formula.
pub(crate) fn split(elements: Vec<Meta>) -> Result<(Vec<BodyElement>, Vec<Meta>)> {
    let mut metadata = elements;
    let mut body = Vec::with_capacity(metadata.len());
    for e in &mut metadata {
        let kind = match string(&e["type"])? {
            "text" => "text",
            "formula" => "formula",
            "table" => "table",
            "blank" => "blank",
            "image" => "image",
            "chart" => "chart",
            "drawing" => "drawing",
            _ => bail!("unknown element type"),
        };
        let value = match kind {
            "text" | "formula" => e.remove(kind).context("element value missing")?,
            "table" => {
                // Read in place, by the IDs the cells hold: a table of a large
                // sheet has a value for each cell.
                let body = {
                    let mut values = HashMap::new();
                    for row in e.rows()? {
                        for cell in &row.cells {
                            let value = if cell.contains_key("formula") {
                                &cell["formula"]
                            } else {
                                cell.get("value").context("cell value missing")?
                            };
                            values.insert(string(&cell["id"])?, value);
                        }
                    }
                    table_body(e, &values)?
                };
                for row in e.rows_mut()? {
                    for cell in &mut row.cells {
                        if cell.contains_key("formula") {
                            cell.insert("formula", Value::Null);
                        } else {
                            cell.remove("value");
                        }
                    }
                }
                body
            }
            _ => Value::Null,
        };
        body.push(BodyElement { kind, value });
    }
    Ok((body, metadata))
}

/// An element of a page body, `{kind: value}` in the page: held apart, as
/// a page of a Word document has an element for each paragraph, and an
/// object of one member took several times its value.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct BodyElement {
    pub(crate) kind: &'static str,
    pub(crate) value: Value,
}

impl BodyElement {
    pub(crate) fn to_value(&self) -> Value {
        json!({ self.kind: &self.value })
    }

    pub(crate) fn into_value(self) -> Value {
        json!({ self.kind: self.value })
    }
}

/// A content page as encoding makes it: its members besides its elements,
/// and its elements.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Content {
    pub(crate) fields: serde_json::Map<String, Value>,
    pub(crate) elements: Vec<BodyElement>,
}

/// The elements of a content page [`Content::check`] checks together.
const CONTENT_PART: usize = 2048;

impl Content {
    /// Checks the page with `check` a part at a time: with at most
    /// [`CONTENT_PART`] of its elements, as no part of the contract relates
    /// the elements of a page.
    pub(crate) fn check(&self, check: impl Fn(&Value) -> Result<()>) -> Result<()> {
        let mut part = Value::Object(self.fields.clone());
        let mut chunks: Vec<&[BodyElement]> = self.elements.chunks(CONTENT_PART).collect();
        if chunks.is_empty() {
            chunks.push(&[]);
        }
        for chunk in chunks {
            part["elements"] = Value::Array(chunk.iter().map(BodyElement::to_value).collect());
            check(&part)?;
        }
        Ok(())
    }
}

/// The body of the table `element` of the metadata holding `values` by cell
/// ID: the values of its headings as `columns`, one list for one heading
/// record and a list of lists for more, and its tree as `rows`.
fn table_body(element: &Meta, values: &HashMap<&str, &Value>) -> Result<Value> {
    let table = Table::of(element)?;
    let value = |cell: &MetaCell| -> Result<Value> {
        let id = string(&cell["id"])?;
        values
            .get(id)
            .map(|value| (*value).clone())
            .with_context(|| format!("value of {id} missing"))
    };
    let mut body = serde_json::Map::new();
    let mut headings = vec![];
    for record in 0..table.headings {
        headings.push(
            table
                .cells(record)?
                .iter()
                .map(value)
                .collect::<Result<Vec<_>>>()?,
        );
    }
    match headings.len() {
        0 => {}
        1 => {
            body.insert("columns".into(), json!(headings.pop()));
        }
        _ => {
            body.insert("columns".into(), json!(headings));
        }
    }
    let keys = table.keys(|id| values.get(id).copied())?;
    let node = |slot: Slot| -> Result<serde_json::Map<String, Value>> {
        let mut node = serde_json::Map::new();
        let cells: Vec<&MetaCell> = match slot {
            Slot::Record(record) => table
                .cells(record)?
                .iter()
                .filter(|c| table.valued(c) || is_mark(c))
                .collect(),
            Slot::Category(c) => table.categories[c].cells.clone(),
        };
        let mut marks = vec![];
        for cell in cells {
            if is_mark(cell) {
                marks.push(value(cell)?);
            } else {
                node.insert(keys[string(&cell["id"])?].to_string(), value(cell)?);
            }
        }
        if !marks.is_empty() {
            node.insert(MARKS.into(), json!(marks));
        }
        Ok(node)
    };
    fn tree(
        slots: &[Slot],
        table: &Table,
        node: &dyn Fn(Slot) -> Result<serde_json::Map<String, Value>>,
    ) -> Result<Value> {
        let mut nodes = vec![];
        for &slot in slots {
            let mut n = node(slot)?;
            let children = table.under(slot);
            if !children.is_empty() {
                n.insert(CHILDREN.into(), tree(children, table, node)?);
            }
            nodes.push(Value::Object(n));
        }
        Ok(Value::Array(nodes))
    }
    body.insert("rows".into(), tree(&table.roots, &table, &node)?);
    Ok(Value::Object(body))
}

/// The value of each cell of the table body `value`, by cell ID, after
/// checking that the body has the shape the table `element` of the metadata
/// gives it: the same headings, the same tree and in each node the keys of
/// its record's cells.
fn table_values<'e, 'v>(
    element: &'e Meta,
    value: &'v Value,
) -> Result<HashMap<&'e str, &'v Value>> {
    let table = Table::of(element)?;
    let body = value
        .as_object()
        .context("table body must have columns and rows")?;
    if let Some(key) = body
        .keys()
        .find(|k| !matches!(k.as_str(), "columns" | "rows"))
    {
        bail!("table body has unknown member {key}; edit values only");
    }
    ensure!(
        body.contains_key("columns") == (table.headings > 0),
        "table headings changed; use structure-save to change headings"
    );
    let mut values: HashMap<&str, &Value> =
        HashMap::with_capacity(table.rows.iter().map(|row| row.cells.len()).sum());
    let changed = || "table headings changed; use structure-save to change headings";
    let heading_rows: Vec<&[Value]> = match table.headings {
        0 => vec![],
        1 => vec![array(&body["columns"]).context(changed())?],
        n => {
            let rows = array(&body["columns"]).context(changed())?;
            ensure!(rows.len() == n, changed());
            rows.iter()
                .map(|r| array(r).map(Vec::as_slice).context(changed()))
                .collect::<Result<_>>()?
        }
    };
    for (record, row) in heading_rows.into_iter().enumerate() {
        let cells = table.cells(record)?;
        ensure!(row.len() == cells.len(), changed());
        for (cell, value) in cells.iter().zip(row) {
            ensure!(!value.is_array() && !value.is_object(), changed());
            values.insert(string(&cell["id"])?, value);
        }
    }
    let keys = table.keys(|id| values.get(id).copied())?;
    let mut pending: Vec<(&Value, &[Slot])> = vec![(&body["rows"], &table.roots)];
    while let Some((nodes, slots)) = pending.pop() {
        let nodes = array(nodes).context("table rows must be a list of records")?;
        ensure!(
            nodes.len() == slots.len(),
            "table records changed; use ARP row operations or structure-save"
        );
        for (node, &slot) in nodes.iter().zip(slots) {
            let cells: Vec<&MetaCell> = match slot {
                Slot::Record(record) => {
                    let cells = table.cells(record)?;
                    // A blank cell under a category stays blank.
                    for cell in cells.iter().filter(|c| c.get("category").is_some()) {
                        values.insert(string(&cell["id"])?, &BLANK);
                    }
                    cells
                        .iter()
                        .filter(|c| table.valued(c) || is_mark(c))
                        .collect()
                }
                Slot::Category(c) => table.categories[c].cells.clone(),
            };
            // The node as messages name it, written only for one.
            let at = Named(|f: &mut std::fmt::Formatter<'_>| match slot {
                Slot::Record(record) => {
                    f.write_str(table.rows[record]["id"].as_str().unwrap_or("record"))
                }
                Slot::Category(c) => write!(f, "category {}", table.categories[c].key),
            });
            let node = node
                .as_object()
                .with_context(|| format!("{at} must be a map of its values"))?;
            let mut expected = BTreeSet::new();
            let mut marks = vec![];
            for cell in cells {
                let id = string(&cell["id"])?;
                if is_mark(cell) {
                    marks.push(id);
                    continue;
                }
                let key: &str = &keys[id];
                expected.insert(key);
                let value = node.get(key).with_context(|| {
                    format!("{at} has no {key}; keys follow the headings, edit values only")
                })?;
                values.insert(id, value);
            }
            if marks.is_empty() {
                ensure!(!node.contains_key(MARKS), "{at} has no marks");
            } else {
                let given = node
                    .get(MARKS)
                    .and_then(Value::as_array)
                    .with_context(|| format!("{at} lost its marks"))?;
                ensure!(
                    given.len() == marks.len(),
                    "{at} must keep {} marks",
                    marks.len()
                );
                for (id, value) in marks.into_iter().zip(given) {
                    values.insert(id, value);
                }
            }
            let children = table.under(slot);
            if children.is_empty() {
                ensure!(
                    !node.contains_key(CHILDREN),
                    "{at} heads no records; use structure-save to change the tree"
                );
            } else {
                ensure!(
                    node.contains_key(CHILDREN),
                    "{at} lost its children; use structure-save to change the tree"
                );
                pending.push((&node[CHILDREN], children));
            }
            if let Some(key) = node
                .keys()
                .find(|k| !matches!(k.as_str(), MARKS | CHILDREN) && !expected.contains(k.as_str()))
            {
                bail!("{at} has unknown key {key}; keys follow the headings, edit values only");
            }
        }
    }
    for value in values.values() {
        ensure!(
            !value.is_array() && !value.is_object(),
            "table values must be text, numbers, booleans or null"
        );
    }
    Ok(values)
}

/// Displays what its function writes.
struct Named<F>(F);

impl<F: Fn(&mut std::fmt::Formatter<'_>) -> std::fmt::Result> std::fmt::Display for Named<F> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        (self.0)(f)
    }
}

/// Calls `visit` with each value of `body` and the element or table cell of
/// `metadata` it belongs to, in the order of the metadata, after checking
/// that the body still has the shape the metadata records. Nothing is copied.
pub(crate) fn each<'a>(
    body: &'a Value,
    metadata: &'a [Meta],
    mut visit: impl FnMut(Item<'a>, &'a Value) -> Result<()>,
) -> Result<()> {
    let body = array(body)?;
    let elements = metadata;
    ensure!(
        body.len() == elements.len(),
        "element count changed; use ARP structural operations"
    );
    for (value, e) in body.iter().zip(elements) {
        let kind = string(&e["type"])?;
        let fields = value
            .as_object()
            .context("body element must be an object")?;
        ensure!(
            fields.len() == 1 && fields.contains_key(kind),
            "element kind changed; use ARP structural operations"
        );
        let value = &value[kind];
        match kind {
            "text" => visit(Item::Element(e), value)?,
            "formula" => {
                ensure!(value.is_string(), "formula must be a string");
                visit(Item::Element(e), value)?;
            }
            "table" => {
                let values = table_values(e, value)?;
                for row in e.records().context("table metadata rows missing")? {
                    for cell in &row.cells {
                        let value = values[string(&cell["id"])?];
                        ensure!(
                            cell.get("formula").is_none() || value.is_string(),
                            "formula must be a string"
                        );
                        visit(Item::Cell(cell), value)?;
                    }
                }
            }
            "blank" | "image" | "chart" | "drawing" => {
                ensure!(value.is_null(), "non-text element must remain null")
            }
            _ => bail!("unknown element type"),
        }
    }
    Ok(())
}

/// Reconstruct the internal writer elements using only the recorded slots:
/// the metadata with every value of [`each`] in place.
pub(crate) fn join(body: &Value, metadata: &[Meta]) -> Result<Vec<Meta>> {
    let mut values = vec![];
    each(body, metadata, |_, value| {
        values.push(value.clone());
        Ok(())
    })?;
    let mut values = values.into_iter();
    let mut result = metadata.to_vec();
    for e in &mut result {
        match e["type"].as_str() {
            Some(kind @ ("text" | "formula")) => {
                let kind = kind.to_owned();
                e[&kind] = values.next().unwrap();
            }
            Some("table") => {
                for row in e.rows_mut()? {
                    for cell in &mut row.cells {
                        let key = if cell.get("formula").is_some() {
                            "formula"
                        } else {
                            "value"
                        };
                        cell[key] = values.next().unwrap();
                    }
                }
            }
            _ => {}
        }
    }
    Ok(result)
}

/// An element or a table cell of element metadata, as [`each`] visits them.
#[derive(Clone, Copy)]
pub(crate) enum Item<'a> {
    Element(&'a Meta),
    Cell(&'a MetaCell),
}

impl<'a> Item<'a> {
    /// The member `key`, as long as the metadata it is of.
    pub(crate) fn member(self, key: &str) -> &'a Value {
        match self {
            Self::Element(element) => &element[key],
            Self::Cell(cell) => &cell[key],
        }
    }
}

impl std::ops::Index<&str> for Item<'_> {
    type Output = Value;
    fn index(&self, key: &str) -> &Value {
        match self {
            Self::Element(element) => &element[key],
            Self::Cell(cell) => &cell[key],
        }
    }
}

/// The YAML of the content `page` whose elements `metadata` describes, each
/// node of a table with its values in the order of the table's columns, then
/// its marks and its children.
pub(crate) fn yaml(page: &Value, metadata: &[Meta]) -> Result<Vec<u8>> {
    let mut bytes = vec![];
    yaml_to(page, metadata, &mut bytes)?;
    Ok(bytes)
}

/// [`split`] of elements given as JSON, the body and the metadata given back
/// as JSON.
pub fn split_json(elements: Value) -> Result<(Value, Value)> {
    let (body, metadata) = split(crate::documents::metadata_from(elements)?)?;
    Ok((
        Value::Array(body.into_iter().map(BodyElement::into_value).collect()),
        crate::documents::metadata_value(&metadata),
    ))
}

/// [`join`] of element metadata given as JSON, the elements given as JSON.
pub fn join_json(body: &Value, metadata: &Value) -> Result<Value> {
    let metadata = crate::documents::metadata_from(metadata.clone())?;
    Ok(crate::documents::metadata_value(&join(body, &metadata)?))
}

/// [`yaml`] written to `out` as it is made.
pub(crate) fn yaml_to(
    page: &Value,
    metadata: &[Meta],
    out: &mut impl std::io::Write,
) -> Result<()> {
    let fields = page.as_object().context("page must be an object")?;
    let elements = array(&page["elements"])?
        .iter()
        .map(|element| {
            let mut members = element
                .as_object()
                .context("body element must be an object")?
                .iter();
            match (members.next(), members.next()) {
                (Some((kind, value)), None) => Ok((kind.as_str(), value)),
                _ => bail!("body element must have one member"),
            }
        })
        .collect::<Result<Vec<_>>>()?;
    page_yaml(fields, &elements, metadata, out)
}

/// [`yaml_to`] of a page made typed.
pub(crate) fn content_yaml_to(
    page: &Content,
    metadata: &[Meta],
    out: &mut impl std::io::Write,
) -> Result<()> {
    let elements: Vec<_> = page
        .elements
        .iter()
        .map(|element| (element.kind, &element.value))
        .collect();
    page_yaml(&page.fields, &elements, metadata, out)
}

/// The YAML of a page with the members `fields` besides its elements, each
/// element given by its kind and value.
fn page_yaml(
    fields: &serde_json::Map<String, Value>,
    elements: &[(&str, &Value)],
    metadata: &[Meta],
    out: &mut impl std::io::Write,
) -> Result<()> {
    ensure!(elements.len() == metadata.len(), "element count changed");
    let mut orders = HashMap::new();
    for (i, ((_, value), e)) in elements.iter().zip(metadata).enumerate() {
        if e["type"] == "table" {
            let table = Table::of(e)?;
            let values = table_values(e, value)?;
            let keys = table.keys(|id| values.get(id).copied())?;
            orders.insert(i, table.key_order(&keys)?);
        }
    }
    serde_saphyr::to_io_writer(
        out,
        &Page {
            fields,
            elements,
            orders: &orders,
        },
    )?;
    Ok(())
}

struct Page<'a> {
    fields: &'a serde_json::Map<String, Value>,
    elements: &'a [(&'a str, &'a Value)],
    orders: &'a HashMap<usize, Vec<String>>,
}

impl Serialize for Page<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        let elements = Elements(self.elements, self.orders);
        let others = self.fields.iter().filter(|(key, _)| *key != "elements");
        let mut map = s.serialize_map(Some(others.clone().count() + 1))?;
        let mut placed = false;
        for (key, value) in others {
            if !placed && key.as_str() > "elements" {
                map.serialize_entry("elements", &elements)?;
                placed = true;
            }
            map.serialize_entry(key, value)?;
        }
        if !placed {
            map.serialize_entry("elements", &elements)?;
        }
        map.end()
    }
}

struct Elements<'a>(&'a [(&'a str, &'a Value)], &'a HashMap<usize, Vec<String>>);

impl Serialize for Elements<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        let mut seq = s.serialize_seq(Some(self.0.len()))?;
        for (i, &(kind, value)) in self.0.iter().enumerate() {
            seq.serialize_element(&Element(kind, value, self.1.get(&i)))?;
        }
        seq.end()
    }
}

/// An element of a page, with the order of a table's keys.
struct Element<'a>(&'a str, &'a Value, Option<&'a Vec<String>>);

impl Serialize for Element<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        let mut map = s.serialize_map(Some(1))?;
        match self.2 {
            Some(order) => map.serialize_entry(self.0, &TableBody(self.1, order))?,
            None => map.serialize_entry(self.0, self.1)?,
        }
        map.end()
    }
}

struct TableBody<'a>(&'a Value, &'a [String]);

impl Serialize for TableBody<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        let mut map = s.serialize_map(None)?;
        if let Some(columns) = self.0.get("columns") {
            map.serialize_entry("columns", columns)?;
        }
        let rows = self.0["rows"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default();
        map.serialize_entry("rows", &Nodes(rows, self.1))?;
        map.end()
    }
}

struct Nodes<'a>(&'a [Value], &'a [String]);

impl Serialize for Nodes<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        let mut seq = s.serialize_seq(Some(self.0.len()))?;
        for node in self.0 {
            seq.serialize_element(&Node(node, self.1))?;
        }
        seq.end()
    }
}

struct Node<'a>(&'a Value, &'a [String]);

impl Serialize for Node<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        let Some(fields) = self.0.as_object() else {
            return self.0.serialize(s);
        };
        let mut map = s.serialize_map(Some(fields.len()))?;
        for key in self.1 {
            if let Some(value) = fields.get(key) {
                map.serialize_entry(key, value)?;
            }
        }
        for (key, value) in fields {
            if !self.1.contains(key) && key != MARKS && key != CHILDREN {
                map.serialize_entry(key, value)?;
            }
        }
        if let Some(marks) = fields.get(MARKS) {
            map.serialize_entry(MARKS, marks)?;
        }
        if let Some(children) = fields.get(CHILDREN) {
            let nodes = children.as_array().map(Vec::as_slice).unwrap_or_default();
            map.serialize_entry(CHILDREN, &Nodes(nodes, self.1))?;
        }
        map.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparse_merged_table_preserves_slots_types_and_formula_identity() {
        let elements = json!([{"id":"t1","type":"table","headings":1,"labels":{"a":"h","c":"lit"},
            "columns":[{"id":"a"},{"id":"b"},{"id":"c"}],"rows":[
            {"id":"r1","cells":[{"id":"h","column":"a","column_span":2,"value":"Merged header"},{"id":"lit","column":"c","value":"=1+2"}]},
            {"id":"r2","cells":[{"id":"empty","column":"a","value":null},{"id":"flag","column":"b","value":false},{"id":"f","column":"c","formula":"=1+2"}]}
        ]}]);
        let (mut body, metadata) = split_json(elements.clone()).unwrap();
        assert_eq!(
            body,
            json!([{"table":{"columns":["Merged header","=1+2"],"rows":[{"Merged header":null,"b":false,"=1+2":"=1+2"}]}}])
        );
        assert!(
            !serde_json::to_string(&metadata)
                .unwrap()
                .contains("Merged header")
        );
        assert_eq!(join_json(&body, &metadata).unwrap(), elements);
        // A heading names the key of its column: changing one renames it.
        body[0]["table"]["columns"][0] = json!("A");
        body[0]["table"]["rows"][0] = json!({"A":2.5,"b":false,"=1+2":"=3+4"});
        let edited = join_json(&body, &metadata).unwrap();
        assert_eq!(edited[0]["rows"][0]["cells"][0]["value"], "A");
        assert_eq!(edited[0]["rows"][1]["cells"][0]["value"], 2.5);
        assert_eq!(edited[0]["rows"][1]["cells"][2]["formula"], "=3+4");
        body[0]["table"]["rows"][0]["Merged header"] = json!(1);
        assert!(join_json(&body, &metadata).is_err());
        body[0]["table"]["rows"][0] = json!({"A":2.5,"b":false,"=1+2":12});
        assert!(join_json(&body, &metadata).is_err());
    }

    #[test]
    fn tree_nests_records_under_their_parents_and_keeps_marks_and_shared_headings() {
        // B1 heads column B; C1 heads C, D and E, the steps of the tree.
        let cell =
            |id: &str, column: &str, value: &str| json!({"id":id,"column":column,"value":value});
        let mark =
            |id: &str, column: &str| json!({"id":id,"column":column,"value":"+","mark":true});
        let elements = json!([{"id":"t","type":"table","headings":1,
            "labels":{"B":"B1","C":"C1","D":"C1","E":"C1"},
            "columns":[{"id":"B"},{"id":"C"},{"id":"D"},{"id":"E"}],"rows":[
            {"id":"r1","cells":[cell("B1","B","マシン名"),cell("C1","C","フォルダ構成")]},
            {"id":"r2","cells":[cell("B2","B","X"),cell("C2","C","/")]},
            {"id":"r3","parent":"r2","cells":[cell("B3","B","X"),mark("C3","C"),cell("D3","D","xxx")]},
            {"id":"r4","parent":"r3","cells":[cell("B4","B","X"),mark("D4","D"),cell("E4","E","yyy")]},
            {"id":"r5","parent":"r3","cells":[cell("B5","B","X"),cell("E5","E","zzz")]},
            {"id":"r6","cells":[cell("B6","B","Y"),cell("C6","C","/")]}
        ]}]);
        let (body, metadata) = split_json(elements.clone()).unwrap();
        assert_eq!(
            body[0]["table"],
            json!({"columns":["マシン名","フォルダ構成"],"rows":[
                {"マシン名":"X","フォルダ構成":"/","children":[
                    {"マシン名":"X","フォルダ構成":"xxx","marks":["+"],"children":[
                        {"マシン名":"X","フォルダ構成":"yyy","marks":["+"]},
                        {"マシン名":"X","フォルダ構成":"zzz"}]}]},
                {"マシン名":"Y","フォルダ構成":"/"}]})
        );
        assert_eq!(join_json(&body, &metadata).unwrap(), elements);
        let page = json!({"elements":body,"page_id":"p","schema_version":"6","title":"t"});
        // Values in the order of the columns, then marks, then children.
        let typed = crate::documents::metadata_from(metadata.clone()).unwrap();
        let text = String::from_utf8(yaml(&page, &typed).unwrap()).unwrap();
        assert_eq!(
            text,
            r#"elements:
- table:
    columns:
    - マシン名
    - フォルダ構成
    rows:
    - マシン名: X
      フォルダ構成: /
      children:
      - マシン名: X
        フォルダ構成: xxx
        marks:
        - +
        children:
        - マシン名: X
          フォルダ構成: yyy
          marks:
          - +
        - マシン名: X
          フォルダ構成: zzz
    - マシン名: "Y"
      フォルダ構成: /
page_id: p
schema_version: "6"
title: t
"#
        );
        assert_eq!(crate::data::parse(&text, false).unwrap(), page);

        let mut moved = body.clone();
        let zzz = moved[0]["table"]["rows"][0]["children"][0]["children"]
            .as_array_mut()
            .unwrap()
            .pop()
            .unwrap();
        moved[0]["table"]["rows"][0]["children"]
            .as_array_mut()
            .unwrap()
            .push(zzz);
        assert!(join_json(&moved, &metadata).is_err());
        let mut unmarked = body.clone();
        unmarked[0]["table"]["rows"][0]["children"][0]
            .as_object_mut()
            .unwrap()
            .remove("marks");
        assert!(join_json(&unmarked, &metadata).is_err());
    }

    #[test]
    fn clashing_or_missing_headings_take_the_column_name() {
        // Two columns headed 備考 hold cells of one record; D has no heading.
        let elements = json!([{"id":"t","type":"table","headings":1,
            "labels":{"column_B":"B1","column_C":"C1"},
            "columns":[{"id":"column_B"},{"id":"column_C"},{"id":"column_D"}],"rows":[
            {"id":"r1","cells":[{"id":"B1","column":"column_B","value":"備考"},{"id":"C1","column":"column_C","value":"備考"}]},
            {"id":"r2","cells":[{"id":"B2","column":"column_B","value":1},{"id":"C2","column":"column_C","value":2},{"id":"D2","column":"column_D","value":3}]}
        ]}]);
        let (body, metadata) = split_json(elements.clone()).unwrap();
        assert_eq!(
            body[0]["table"]["rows"],
            json!([{"備考(B)":1,"備考(C)":2,"D":3}])
        );
        assert_eq!(join_json(&body, &metadata).unwrap(), elements);
    }
}
