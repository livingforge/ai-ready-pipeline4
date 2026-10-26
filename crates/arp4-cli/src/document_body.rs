//! Bodies and their value-free layout metadata. A table body is the tree of
//! its records: each record a node keyed by the headings over its columns,
//! under the record whose names head it or the category over it, a node of
//! its own holding cells of its first record. Positions in the tree and keys are
//! editing slots, not identities: moving values by hand is an ordinary value
//! edit, and the tree's shape and keys follow from the metadata.
use crate::data::{YamlNode, YamlReader, array, string};
use crate::documents::{Meta, MetaCell};
use anyhow::{Context, Result, bail, ensure};
use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;

mod diagrams;
use diagrams::Ordered;
pub(crate) use diagrams::{
    Arrow, DIAGRAM, Diagram, DiagramLine, DiagramShape, Diagrams, LineEnd, expand,
};

/// The member of a node holding the values of its record's cells that head
/// nothing and are headed by nothing, such as the marks drawing a tree.
const MARKS: &str = "marks";
/// The member of a node holding the nodes of the records it heads.
const CHILDREN: &str = "children";
/// The value of a blank cell under a category, which no node shows.
static BLANK: Value = Value::Null;

/// Something for each cell of a table, by the place of its record among the
/// table's records and its place in the record: a large table has a cell for
/// each cell of its sheet, and a map of them by cell ID took several times
/// what they hold.
struct PerCell<T> {
    /// The place in `items` of the first cell of each record.
    starts: Vec<usize>,
    items: Vec<Option<T>>,
}

impl<T> PerCell<T> {
    fn new(rows: &[crate::documents::MetaRow]) -> Self {
        let mut starts = Vec::with_capacity(rows.len());
        let mut count = 0;
        for row in rows {
            starts.push(count);
            count += row.cells.len();
        }
        Self {
            starts,
            items: std::iter::repeat_with(|| None).take(count).collect(),
        }
    }

    fn get(&self, record: usize, cell: usize) -> Option<&T> {
        self.items[self.starts[record] + cell].as_ref()
    }

    fn set(&mut self, record: usize, cell: usize, item: T) {
        self.items[self.starts[record] + cell] = Some(item);
    }

    fn take(&mut self, record: usize, cell: usize) -> Option<T> {
        self.items[self.starts[record] + cell].take()
    }
}

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
    /// Its cells, each with its place in its record, in the order of the
    /// table's metadata.
    cells: Vec<(usize, &'a MetaCell)>,
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
        for (i, row) in rows.iter().enumerate() {
            if let Some(id) = row["id"].as_str() {
                index.insert(id, i);
            }
            for cell in &row.cells {
                cell.id()?;
            }
        }
        let mut categories = vec![];
        let mut named = HashMap::new();
        let mut categorized = BTreeSet::new();
        if let Some(map) = element.get("categories") {
            let map = map.as_object().context("table categories must be a map")?;
            // The cells by ID, made only for a table with categories: a large
            // table has a cell for each cell of its sheet, and a layout read
            // back gives each table its categories, none or some.
            let mut cells = HashMap::new();
            if !map.is_empty() {
                for (i, row) in rows.iter().enumerate() {
                    for (j, cell) in row.cells.iter().enumerate() {
                        cells.insert(cell.id()?, (i, j, cell));
                    }
                }
            }
            for (key, entry) in map {
                let mut ids = vec![key.as_str()];
                for id in entry["cells"].as_array().into_iter().flatten() {
                    ids.push(string(id)?);
                }
                let mut held = vec![];
                let mut record = None;
                for id in ids {
                    let &(i, j, cell) = cells
                        .get(id)
                        .with_context(|| format!("category cell {id} is no cell of the table"))?;
                    ensure!(
                        i >= headings && *record.get_or_insert(i) == i && !is_mark(cell),
                        "the cells of category {key} must be cells of one record"
                    );
                    ensure!(categorized.insert(id), "cell {id} is in two categories");
                    held.push((j, cell));
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
        for cell in rows.iter().flat_map(|row| &row.cells) {
            if let Some(key) = cell.get("category") {
                let key = key.as_str().context("a cell's category must be its key")?;
                ensure!(
                    named.contains_key(key),
                    "cell {} is under no category {key}",
                    cell.id()?
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
            && !cell.id().is_ok_and(|id| self.categorized.contains(id))
    }

    fn cells(&self, record: usize) -> Result<&'a [MetaCell]> {
        Ok(&self.rows[record].cells)
    }

    /// The cells a node shows, each with its record and its place in it: a
    /// record's cells that are [`Table::valued`] or marks, or a category's.
    fn node_cells(&self, slot: Slot) -> Result<Vec<(usize, usize, &'a MetaCell)>> {
        Ok(match slot {
            Slot::Record(record) => self
                .cells(record)?
                .iter()
                .enumerate()
                .filter(|(_, c)| self.valued(c) || is_mark(c))
                .map(|(j, c)| (record, j, c))
                .collect(),
            Slot::Category(c) => {
                let category = &self.categories[c];
                category
                    .cells
                    .iter()
                    .map(|&(j, cell)| (category.record, j, cell))
                    .collect()
            }
        })
    }

    /// The place of each cell whose value a heading takes, by its ID: the
    /// cells the `label` of a cell or the table's `labels` name.
    fn label_places(&self) -> Result<HashMap<&'a str, (usize, usize)>> {
        let mut named: std::collections::HashSet<&str> = self.element["labels"]
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(_, id)| id.as_str())
            .collect();
        for row in self.rows {
            named.extend(row.cells.iter().filter_map(|cell| cell["label"].as_str()));
        }
        let mut places = HashMap::with_capacity(named.len());
        for (i, row) in self.rows.iter().enumerate() {
            for (j, cell) in row.cells.iter().enumerate() {
                let id = cell.id()?;
                if named.contains(id) {
                    places.insert(id, (i, j));
                }
            }
        }
        Ok(places)
    }

    /// The key of each cell of the tree that is no mark, by its place: the text
    /// of the heading the cell's own `label` names, or else the one the
    /// table's `labels` give its column, or the column's name for a column
    /// without one or with an empty heading. A heading
    /// shared by columns two of which hold cells of one record, or naming a
    /// member of a node, is followed by the column's name, as `備考(L)`.
    /// `value_at` gives the current value of a heading cell by its place.
    fn keys<'v>(
        &self,
        value_at: impl Fn(usize, usize) -> Option<&'v Value>,
    ) -> Result<PerCell<Rc<str>>> {
        let places = self.label_places()?;
        let value_of = |id: &str| {
            let &(i, j) = places.get(id)?;
            value_at(i, j)
        };
        // The heading a cell takes its key from: its own, as a physical name
        // under a logical one in a column, or its column's.
        let heading = |cell: &'a MetaCell| -> Result<Option<&'a str>> {
            let column = cell.column()?;
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
                let column = cell.column()?;
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
            let column = cell.column()?;
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
                    let column = cell.column()?;
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
        let mut keys = PerCell::new(self.rows);
        for record in self.headings..self.rows.len() {
            for (j, cell) in self.cells(record)?.iter().enumerate() {
                if keyed(cell) {
                    let column = cell.column()?;
                    keys.set(record, j, key_of(cell, qualified.contains(column))?);
                }
            }
        }
        Ok(keys)
    }

    /// The key of the cell at `cell` of `record`, which has one.
    fn key(keys: &PerCell<Rc<str>>, record: usize, cell: usize) -> Result<&str> {
        keys.get(record, cell)
            .map(|key| &**key)
            .context("table cell without a key")
    }

    /// The keys of `keys` in the order of the table's columns.
    fn key_order(&self, keys: &PerCell<Rc<str>>) -> Result<Vec<String>> {
        let columns: HashMap<&str, usize> = array(&self.element["columns"])?
            .iter()
            .enumerate()
            .filter_map(|(i, c)| Some((c["id"].as_str()?, i)))
            .collect();
        let mut first: HashMap<&str, usize> = HashMap::new();
        for record in self.headings..self.rows.len() {
            for (j, cell) in self.cells(record)?.iter().enumerate() {
                if !keyed(cell) {
                    continue;
                }
                let at = columns.get(cell.column()?).copied().unwrap_or(usize::MAX);
                let key = Self::key(keys, record, j)?;
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
        let kind = BodyElement::kind_named(string(&e["type"])?).context("unknown element type")?;
        let value = match kind {
            "table" => {
                let values = cell_values(e)?;
                table_body(e, values)?
            }
            _ => element_value(e, kind)?,
        };
        body.push(BodyElement { kind, value });
    }
    Ok((body, metadata))
}

/// The value a text or formula element `e` of `kind` gives the body, taken
/// out of it; null for an element of no value.
fn element_value(e: &mut Meta, kind: &str) -> Result<Value> {
    Ok(match kind {
        "text" | "formula" => e.remove(kind).context("element value missing")?,
        _ => Value::Null,
    })
}

/// The values of the cells of the table `e`, moved out of the cells by their
/// places: a table of a large sheet has a value for each cell.
fn cell_values(e: &mut Meta) -> Result<PerCell<Value>> {
    let mut values = PerCell::new(e.rows()?);
    for (i, row) in e.rows_mut()?.iter_mut().enumerate() {
        for (j, cell) in row.cells.iter_mut().enumerate() {
            let value = if cell.contains_key("formula") {
                cell.insert("formula", Value::Null)
                    .expect("a cell with a formula has one")
            } else {
                cell.remove("value").context("cell value missing")?
            };
            values.set(i, j, value);
        }
    }
    Ok(values)
}

/// [`split`] of the elements of a page to be written ([`Encoded`]): a table
/// whose cells each hold a text, number, boolean or null, keyed by no member
/// a node has of its own, keeps the values of its cells by their places.
pub(crate) fn split_encoded(elements: Vec<Meta>) -> Result<(EncodedBody, Vec<Meta>)> {
    let mut metadata = elements;
    let mut body = Vec::with_capacity(metadata.len());
    for e in &mut metadata {
        let kind = BodyElement::kind_named(string(&e["type"])?).context("unknown element type")?;
        body.push(match kind {
            "table" => {
                let values = cell_values(e)?;
                if plain_table(e, &values)? {
                    Encoding::Cells(values)
                } else {
                    Encoding::Element(BodyElement {
                        kind,
                        value: table_body(e, values)?,
                    })
                }
            }
            _ => Encoding::Element(BodyElement {
                kind,
                value: element_value(e, kind)?,
            }),
        });
    }
    Ok((EncodedBody(body), metadata))
}

/// Whether the tree of the table `e` with the cell `values` is one the
/// content contract takes as it is made: each cell holds a text, number,
/// boolean or null, and no key is a member a node has of its own.
fn plain_table(e: &Meta, values: &PerCell<Value>) -> Result<bool> {
    if values
        .items
        .iter()
        .any(|value| value.as_ref().is_none_or(|value| !scalar(value)))
    {
        return Ok(false);
    }
    let keys = Table::of(e)?.keys(|i, j| values.get(i, j))?;
    Ok(keys
        .items
        .iter()
        .flatten()
        .all(|key| !matches!(&**key, MARKS | CHILDREN)))
}

/// A page as encoding makes it, to be written: its members besides its
/// elements, and its elements, each table whose tree is valid as it is made
/// by the values of its cells rather than as that tree, which a table of a
/// large sheet took several times the room of its values as.
pub(crate) struct Encoded {
    pub(crate) fields: serde_json::Map<String, Value>,
    elements: Vec<Encoding>,
}

/// The elements of an [`Encoded`] page, as [`split_encoded`] gives them.
pub(crate) struct EncodedBody(Vec<Encoding>);

/// An element of an [`Encoded`] page.
enum Encoding {
    Element(BodyElement),
    /// The values of the cells of a table, by their places.
    Cells(PerCell<Value>),
}

impl Encoded {
    pub(crate) fn new(
        fields: serde_json::Map<String, Value>,
        EncodedBody(elements): EncodedBody,
    ) -> Self {
        Self { fields, elements }
    }

    /// [`Content::check`] of the page: a table kept by its cells is valid
    /// as it is made, and checked as a table of no rows in its place.
    pub(crate) fn check(&mut self, check: impl Fn(&Value) -> Result<()>) -> Result<()> {
        let empty = || BodyElement {
            kind: "table",
            value: json!({"rows":[]}),
        };
        let mut page = Content {
            fields: std::mem::take(&mut self.fields),
            elements: self
                .elements
                .iter_mut()
                .map(|element| match element {
                    // Moved into the page and back, its place holding a
                    // value of no room meanwhile.
                    Encoding::Element(element) => std::mem::replace(
                        element,
                        BodyElement {
                            kind: "blank",
                            value: Value::Null,
                        },
                    ),
                    Encoding::Cells(_) => empty(),
                })
                .collect(),
        };
        let result = page.check(check);
        self.fields = page.fields;
        for (element, checked) in self.elements.iter_mut().zip(page.elements) {
            if let Encoding::Element(element) = element {
                *element = checked;
            }
        }
        result
    }
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
    /// The kind of body element `name` names, none for no kind.
    pub(crate) fn kind_named(name: &str) -> Option<&'static str> {
        [
            "text", "formula", "table", "blank", "image", "chart", "drawing",
        ]
        .into_iter()
        .find(|kind| *kind == name)
    }

    pub(crate) fn into_value(self) -> Value {
        json!({ self.kind: self.value })
    }

    /// The kind of element `name` names in a page as written, where the
    /// elements of each diagram are written together as one.
    fn written_kind(name: &str) -> Option<&'static str> {
        Self::kind_named(name).or((name == DIAGRAM).then_some(DIAGRAM))
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
    /// The content page given as JSON, each element an object of one member
    /// of a known kind.
    pub(crate) fn from_value(page: Value) -> Result<Self> {
        let Value::Object(mut fields) = page else {
            bail!("page must be an object")
        };
        let Some(Value::Array(elements)) = fields.remove("elements") else {
            bail!("page elements missing")
        };
        let elements = elements
            .into_iter()
            .map(|element| {
                let Value::Object(element) = element else {
                    bail!("body element must be an object")
                };
                let mut members = element.into_iter();
                match (members.next(), members.next()) {
                    (Some((kind, value)), None) => Ok(BodyElement {
                        kind: BodyElement::written_kind(&kind).context("unknown element type")?,
                        value,
                    }),
                    _ => bail!("body element must have one member"),
                }
            })
            .collect::<Result<_>>()?;
        Ok(Self { fields, elements })
    }

    /// The content page `text` of the file `name`, valid against the content
    /// contract: read typed, as [`crate::data::parse_listing`] reads it, and
    /// checked a part at a time; a page it reads no such page of, or that
    /// fails the contract, is read whole as JSON, as before, to say why.
    pub(crate) fn read(text: &str) -> Result<Self> {
        let typed = crate::data::parse_listing(text, "elements").and_then(|(fields, items)| {
            let elements = items
                .into_iter()
                .map(|(kind, value)| {
                    Some(BodyElement {
                        kind: BodyElement::written_kind(&kind)?,
                        value,
                    })
                })
                .collect::<Option<_>>()?;
            Some(Self { fields, elements })
        });
        if let Some(mut page) = typed
            && page
                .check(|part| crate::data::validate("content", part))
                .is_ok()
        {
            return Ok(page);
        }
        let page = crate::data::parse(text, false)?;
        crate::data::validate("content", &page)?;
        Self::from_value(page)
    }

    /// Checks the page with `check` a part at a time: with at most
    /// [`CONTENT_PART`] of its elements, as no part of the contract relates
    /// the elements of a page. The members and values of the page are moved
    /// into each part and back rather than copied: a table of a large sheet
    /// is one element, as large as the page.
    pub(crate) fn check(&mut self, check: impl Fn(&Value) -> Result<()>) -> Result<()> {
        let mut part = Value::Object(std::mem::take(&mut self.fields));
        let mut start = 0;
        let result = loop {
            let end = (start + CONTENT_PART).min(self.elements.len());
            let chunk = &mut self.elements[start..end];
            part["elements"] = chunk
                .iter_mut()
                .map(|e| json!({ e.kind: std::mem::take(&mut e.value) }))
                .collect();
            let result = check(&part);
            let Value::Array(checked) = part["elements"].take() else {
                unreachable!("the part holds the elements as a list")
            };
            for (element, mut checked) in chunk.iter_mut().zip(checked) {
                element.value = checked[element.kind].take();
            }
            start = end;
            if result.is_err() || start >= self.elements.len() {
                break result;
            }
        };
        let Value::Object(mut fields) = part else {
            unreachable!("the part is an object")
        };
        fields.remove("elements");
        self.fields = fields;
        result
    }
}

/// The body of the table `element` of the metadata holding `values` by the
/// places of its cells: the values of its headings as `columns`, one list for
/// one heading record and a list of lists for more, and its tree as `rows`.
/// The values are moved into the body.
fn table_body(element: &Meta, mut values: PerCell<Value>) -> Result<Value> {
    let table = Table::of(element)?;
    let keys = table.keys(|i, j| values.get(i, j))?;
    let mut value = |record: usize, j: usize, cell: &MetaCell| -> Result<Value> {
        values
            .take(record, j)
            .with_context(|| format!("value of {} missing", cell.id().unwrap_or("a cell")))
    };
    let mut body = serde_json::Map::new();
    let mut headings = vec![];
    for record in 0..table.headings {
        headings.push(
            table
                .cells(record)?
                .iter()
                .enumerate()
                .map(|(j, cell)| value(record, j, cell))
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
    let mut node = |slot: Slot| -> Result<serde_json::Map<String, Value>> {
        let mut node = serde_json::Map::new();
        let mut marks = vec![];
        for (record, j, cell) in table.node_cells(slot)? {
            if is_mark(cell) {
                marks.push(value(record, j, cell)?);
            } else {
                let key = Table::key(&keys, record, j)?;
                node.insert(key.to_owned(), value(record, j, cell)?);
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
        node: &mut dyn FnMut(Slot) -> Result<serde_json::Map<String, Value>>,
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
    body.insert("rows".into(), tree(&table.roots, &table, &mut node)?);
    Ok(Value::Object(body))
}

/// The value of each cell of the table body `value`, by its place, after
/// checking that the body has the shape the table `element` of the metadata
/// gives it: the same headings, the same tree and in each node the keys of
/// its record's cells.
fn table_values<V: Body>(element: &Meta, value: V) -> Result<PerCell<V>> {
    let table = Table::of(element)?;
    let mut body = value;
    let members = body
        .value()
        .as_object()
        .context("table body must have columns and rows")?;
    if let Some(key) = members
        .keys()
        .find(|k| !matches!(k.as_str(), "columns" | "rows"))
    {
        bail!("table body has unknown member {key}; edit values only");
    }
    ensure!(
        members.contains_key("columns") == (table.headings > 0),
        "table headings changed; use structure-save to change headings"
    );
    let mut values = PerCell::new(table.rows);
    let columns = body.take("columns");
    set_headings(&table, &mut values, columns)?;
    let keys = table.keys(|i, j| values.get(i, j).map(Body::value))?;
    let rows = body.take("rows");
    let mut pending: Vec<(Option<V>, &[Slot])> = vec![(rows, &table.roots)];
    while let Some((nodes, slots)) = pending.pop() {
        let nodes = nodes
            .and_then(V::items)
            .context("table rows must be a list of records")?;
        ensure!(
            nodes.len() == slots.len(),
            "table records changed; use ARP row operations or structure-save"
        );
        // Each node is let go once read, when it is owned.
        for (mut node, &slot) in nodes.into_iter().zip(slots) {
            if let Slot::Record(record) = slot {
                // A blank cell under a category stays blank.
                for (j, cell) in table.cells(record)?.iter().enumerate() {
                    if cell.get("category").is_some() {
                        values.set(record, j, V::blank());
                    }
                }
            }
            let cells = table.node_cells(slot)?;
            // The node as messages name it, written only for one.
            let at = Named(|f: &mut std::fmt::Formatter<'_>| match slot {
                Slot::Record(record) => {
                    f.write_str(table.rows[record]["id"].as_str().unwrap_or("record"))
                }
                Slot::Category(c) => write!(f, "category {}", table.categories[c].key),
            });
            ensure!(node.value().is_object(), "{at} must be a map of its values");
            let mut expected = BTreeSet::new();
            let mut marks = vec![];
            for (record, j, cell) in cells {
                if is_mark(cell) {
                    marks.push((record, j));
                    continue;
                }
                let key = Table::key(&keys, record, j)?;
                expected.insert(key);
                let value = node.take(key).with_context(|| {
                    format!("{at} has no {key}; keys follow the headings, edit values only")
                })?;
                values.set(record, j, value);
            }
            if marks.is_empty() {
                ensure!(node.value().get(MARKS).is_none(), "{at} has no marks");
            } else {
                let given = node
                    .take(MARKS)
                    .and_then(V::items)
                    .with_context(|| format!("{at} lost its marks"))?;
                ensure!(
                    given.len() == marks.len(),
                    "{at} must keep {} marks",
                    marks.len()
                );
                for ((record, j), value) in marks.into_iter().zip(given) {
                    values.set(record, j, value);
                }
            }
            let children = table.under(slot);
            if children.is_empty() {
                ensure!(
                    node.value().get(CHILDREN).is_none(),
                    "{at} heads no records; use structure-save to change the tree"
                );
            } else {
                ensure!(
                    node.value().get(CHILDREN).is_some(),
                    "{at} lost its children; use structure-save to change the tree"
                );
                pending.push((node.take(CHILDREN), children));
            }
            // An owned node holds only what no cell took: none of it is
            // expected, as none of a borrowed one beyond what cells read.
            if let Some(key) = node
                .value()
                .as_object()
                .into_iter()
                .flat_map(|members| members.keys())
                .find(|k| !matches!(k.as_str(), MARKS | CHILDREN) && !expected.contains(k.as_str()))
            {
                bail!("{at} has unknown key {key}; keys follow the headings, edit values only");
            }
        }
    }
    for value in values.items.iter().flatten() {
        ensure!(
            scalar(value.value()),
            "table values must be text, numbers, booleans or null"
        );
    }
    Ok(values)
}

/// Gives the cells of the heading records of `table` the values of the
/// `columns` of its body: one list for one heading record, and a list of
/// lists for more.
fn set_headings<V: Body>(table: &Table, values: &mut PerCell<V>, columns: Option<V>) -> Result<()> {
    let changed = || "table headings changed; use structure-save to change headings";
    let heading_rows: Vec<Vec<V>> = match table.headings {
        0 => vec![],
        1 => vec![columns.and_then(V::items).context(changed())?],
        n => {
            let rows = columns.and_then(V::items).context(changed())?;
            ensure!(rows.len() == n, changed());
            rows.into_iter()
                .map(|r| r.items().context(changed()))
                .collect::<Result<_>>()?
        }
    };
    for (record, row) in heading_rows.into_iter().enumerate() {
        let cells = table.cells(record)?;
        ensure!(row.len() == cells.len(), changed());
        for (j, value) in (0..cells.len()).zip(row) {
            ensure!(scalar(value.value()), changed());
            values.set(record, j, value);
        }
    }
    Ok(())
}

/// [`table_values`] of a table body read node by node from `reader`, the
/// body's mapping started at `depth`: each node is read as it comes, and no
/// value is made of it but those of its cells. Fails for a body [`table_values`]
/// reads differently, such as one naming its rows before its columns; it
/// then reads the body to say why.
fn read_table(reader: &mut YamlReader<'_>, element: &Meta, depth: usize) -> Result<PerCell<Value>> {
    let table = Table::of(element)?;
    let mut values = PerCell::new(table.rows);
    let (mut columns, mut rows) = (false, false);
    while let Some(key) = reader.key()? {
        let node = reader.node(depth + 1)?;
        match &*key {
            "columns" if !columns && !rows => {
                columns = true;
                // The headings are few, and read whole.
                let value = reader.value(node, depth + 1)?;
                ensure!(table.headings > 0, "table without headings has columns");
                set_headings(&table, &mut values, Some(value))?;
            }
            "rows" if !rows && columns == (table.headings > 0) => {
                rows = true;
                ensure!(
                    matches!(node, YamlNode::Sequence),
                    "table rows must be a list"
                );
                let keys = table.keys(|i, j| values.get(i, j))?;
                read_nodes(reader, &table, &keys, &mut values, &table.roots, depth + 1)?;
            }
            key => bail!("table body member {key} is not read in order"),
        }
    }
    ensure!(rows, "table rows missing");
    for value in values.items.iter().flatten() {
        ensure!(scalar(value), "table value is a list or a map");
    }
    Ok(values)
}

/// Reads the nodes of the list being read from `reader`, one at `depth`, as
/// the nodes of `slots` of `table`, into `values`.
fn read_nodes(
    reader: &mut YamlReader<'_>,
    table: &Table,
    keys: &PerCell<Rc<str>>,
    values: &mut PerCell<Value>,
    slots: &[Slot],
    depth: usize,
) -> Result<()> {
    // Each cell is given a value once: the cells of nodes are each their own.
    let set = |values: &mut PerCell<Value>, record: usize, j: usize, value: Value| {
        let cell = &mut values.items[values.starts[record] + j];
        ensure!(cell.is_none(), "table cell read twice");
        *cell = Some(value);
        Ok(())
    };
    let mut slots = slots.iter();
    while let Some(node) = reader.item(depth + 1)? {
        let slot = *slots.next().context("table records changed")?;
        ensure!(
            matches!(node, YamlNode::Mapping),
            "table node must be a map"
        );
        if let Slot::Record(record) = slot {
            for (j, cell) in table.cells(record)?.iter().enumerate() {
                if cell.get("category").is_some() {
                    set(values, record, j, Value::Null)?;
                }
            }
        }
        // The keys of the node's cells in order, each with its cell's place.
        let mut expected = vec![];
        let mut marks = vec![];
        for (record, j, cell) in table.node_cells(slot)? {
            if is_mark(cell) {
                marks.push((record, j));
            } else {
                expected.push((Table::key(keys, record, j)?, record, j));
            }
        }
        expected.sort_unstable_by_key(|&(key, ..)| key);
        ensure!(
            expected.windows(2).all(|pair| pair[0].0 != pair[1].0)
                && expected.iter().all(|e| ![MARKS, CHILDREN].contains(&e.0)),
            "table node keys repeat or name its marks or children"
        );
        let mut found = vec![false; expected.len()];
        let under = table.under(slot);
        let (mut marked, mut headed) = (false, false);
        while let Some(key) = reader.key()? {
            let node = reader.node(depth + 2)?;
            match &*key {
                MARKS if !marked && !marks.is_empty() => {
                    marked = true;
                    let Value::Array(given) = reader.value(node, depth + 2)? else {
                        bail!("table marks must be a list");
                    };
                    ensure!(given.len() == marks.len(), "table marks changed");
                    for (&(record, j), value) in marks.iter().zip(given) {
                        set(values, record, j, value)?;
                    }
                }
                CHILDREN if !headed && !under.is_empty() => {
                    headed = true;
                    ensure!(
                        matches!(node, YamlNode::Sequence),
                        "table children must be a list"
                    );
                    read_nodes(reader, table, keys, values, under, depth + 2)?;
                }
                key => {
                    let at = expected
                        .binary_search_by(|&(expected, ..)| expected.cmp(key))
                        .ok()
                        .context("unknown table node key")?;
                    ensure!(!std::mem::replace(&mut found[at], true), "duplicate key");
                    let (_, record, j) = expected[at];
                    set(values, record, j, reader.value(node, depth + 2)?)?;
                }
            }
        }
        ensure!(
            found.iter().all(|&found| found)
                && marked == !marks.is_empty()
                && headed == !under.is_empty(),
            "table node changed"
        );
    }
    ensure!(slots.next().is_none(), "table records changed");
    Ok(())
}

/// Whether `value` is a value a table cell may hold: neither a list nor an
/// object.
fn scalar(value: &Value) -> bool {
    !value.is_array() && !value.is_object()
}

/// A table body as [`table_values`] reads it: borrowed, its values read in
/// place, or owned, each value taken out of it as it is read and each record
/// let go once read, as a table of a large sheet is as large as its page.
trait Body: Sized {
    fn value(&self) -> &Value;
    /// The member `key` of an object, taken out of an owned one.
    fn take(&mut self, key: &str) -> Option<Self>;
    /// The items of a list.
    fn items(self) -> Option<Vec<Self>>;
    /// The value of a blank cell under a category.
    fn blank() -> Self;
}

impl Body for &Value {
    fn value(&self) -> &Value {
        self
    }
    fn take(&mut self, key: &str) -> Option<Self> {
        self.get(key)
    }
    fn items(self) -> Option<Vec<Self>> {
        self.as_array().map(|items| items.iter().collect())
    }
    fn blank() -> Self {
        &BLANK
    }
}

impl Body for Value {
    fn value(&self) -> &Value {
        self
    }
    fn take(&mut self, key: &str) -> Option<Self> {
        self.as_object_mut()?.remove(key)
    }
    fn items(self) -> Option<Vec<Self>> {
        match self {
            Value::Array(items) => Some(items),
            _ => None,
        }
    }
    fn blank() -> Self {
        Value::Null
    }
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
        element(e, kind, &value[kind], &mut visit)?;
    }
    Ok(())
}

/// [`each`] of a body held typed, as a content page is read: the body is
/// let go as it is read, each value given to `visit` moved out of it.
pub(crate) fn into_each_element<'a>(
    body: Vec<BodyElement>,
    metadata: &'a [Meta],
    mut visit: impl FnMut(Item<'a>, Value) -> Result<()>,
) -> Result<()> {
    ensure!(
        body.len() == metadata.len(),
        "element count changed; use ARP structural operations"
    );
    for (value, e) in body.into_iter().zip(metadata) {
        let kind = string(&e["type"])?;
        ensure!(
            value.kind == kind,
            "element kind changed; use ARP structural operations"
        );
        element(e, kind, value.value, &mut visit)?;
    }
    Ok(())
}

/// Calls `visit` with the value of the body element of `kind` that the
/// metadata element `e` describes, or of each of its table cells.
fn element<'a, V: Body>(
    e: &'a Meta,
    kind: &str,
    value: V,
    visit: &mut impl FnMut(Item<'a>, V) -> Result<()>,
) -> Result<()> {
    match kind {
        "text" => visit(Item::Element(e), value)?,
        "formula" => {
            ensure!(value.value().is_string(), "formula must be a string");
            visit(Item::Element(e), value)?;
        }
        "table" => visit_cells(e, table_values(e, value)?, visit)?,
        "blank" | "image" | "chart" | "drawing" => {
            ensure!(value.value().is_null(), "non-text element must remain null")
        }
        _ => bail!("unknown element type"),
    }
    Ok(())
}

/// Calls `visit` with the value of each cell of the table `e` of the
/// metadata, by its place in `values`.
fn visit_cells<'a, V: Body>(
    e: &'a Meta,
    mut values: PerCell<V>,
    visit: &mut impl FnMut(Item<'a>, V) -> Result<()>,
) -> Result<()> {
    for (i, row) in e
        .records()
        .context("table metadata rows missing")?
        .iter()
        .enumerate()
    {
        for (j, cell) in row.cells.iter().enumerate() {
            let value = values.take(i, j).with_context(|| {
                format!(
                    "table body holds no value of {}",
                    cell.id().unwrap_or("a cell")
                )
            })?;
            ensure!(
                cell.get("formula").is_none() || value.value().is_string(),
                "formula must be a string"
            );
            visit(Item::Cell(cell), value)?;
        }
    }
    Ok(())
}

/// [`visit_cells`] from the last cell to the first, the room of the values
/// let go as they are taken: a table of a large sheet has a value for each
/// cell, and they were held together with all the values bound.
fn visit_cells_back<'a>(
    e: &'a Meta,
    values: PerCell<Value>,
    visit: &mut impl FnMut(Item<'a>, Value) -> Result<()>,
) -> Result<()> {
    let records = e.records().context("table metadata rows missing")?;
    let mut items = values.items;
    for row in records.iter().rev() {
        for cell in row.cells.iter().rev() {
            let value = items.pop().flatten().with_context(|| {
                format!(
                    "table body holds no value of {}",
                    cell.id().unwrap_or("a cell")
                )
            })?;
            if items.len() < items.capacity() / 2 {
                items = crate::data::exact(items);
            }
            ensure!(
                cell.get("formula").is_none() || value.is_string(),
                "formula must be a string"
            );
            visit(Item::Cell(cell), value)?;
        }
    }
    ensure!(items.is_empty(), "table body holds values of no cell");
    Ok(())
}

/// [`Content::read`] of the content page `text` and [`into_each_element`]
/// of it together, the page read node by node: no value is made of a table
/// but those of its cells, as a table of a large sheet is several times its
/// values. The cells of a table are visited from the last to the first. Gives the members of the page besides its elements, each checked
/// as the content contract checks it. Fails for a page these read
/// differently, such as one whose table names its rows before its headings:
/// they then read it to say why.
pub(crate) fn read_each_element<'a>(
    text: &str,
    metadata: &'a [Meta],
    diagrams: &Diagrams,
    mut visit: impl FnMut(Item<'a>, Value) -> Result<()>,
) -> Result<serde_json::Map<String, Value>> {
    let mut reader = YamlReader::new(text)?;
    ensure!(
        matches!(reader.node(0)?, YamlNode::Mapping),
        "page must be a map"
    );
    let mut fields = serde_json::Map::new();
    let mut read = false;
    while let Some(key) = reader.key()? {
        let node = reader.node(1)?;
        if key != "elements" {
            let value = reader.value(node, 1)?;
            ensure!(
                fields.insert(key.into_owned(), value).is_none(),
                "duplicate key"
            );
            continue;
        }
        ensure!(!std::mem::replace(&mut read, true), "duplicate key");
        ensure!(
            matches!(node, YamlNode::Sequence),
            "page elements must be a list"
        );
        let mut place = 0;
        // The values of the elements of diagrams read, which come in their places.
        let mut pending: HashMap<usize, Value> = HashMap::new();
        let placed = |place: &mut usize,
                      pending: &mut HashMap<usize, Value>,
                      visit: &mut dyn FnMut(Item<'a>, Value) -> Result<()>|
         -> Result<()> {
            while let Some(value) = pending.remove(place) {
                let e = &metadata[*place];
                element(e, string(&e["type"])?, value, &mut |item, value| {
                    visit(item, value)
                })?;
                *place += 1;
            }
            Ok(())
        };
        while let Some(item) = reader.item(2)? {
            placed(&mut place, &mut pending, &mut visit)?;
            ensure!(
                matches!(item, YamlNode::Mapping),
                "body element must be a map"
            );
            let kind = reader.key()?.context("body element must have one member")?;
            let kind = BodyElement::written_kind(&kind).context("unknown element type")?;
            let e = metadata.get(place).context("element count changed")?;
            let node = reader.node(3)?;
            if kind == DIAGRAM {
                let (diagram, first) = diagrams
                    .of(place)
                    .context("a diagram is written where the original's drawings make none")?;
                ensure!(first, "element count changed");
                pending.extend(diagram.values(reader.value(node, 3)?)?);
                placed(&mut place, &mut pending, &mut visit)?;
                ensure!(reader.key()?.is_none(), "body element must have one member");
                continue;
            }
            ensure!(
                diagrams.of(place).is_none(),
                "the drawings lines join are written as one diagram; edit their texts there"
            );
            ensure!(string(&e["type"])? == kind, "element kind changed");
            place += 1;
            if kind == "table" {
                ensure!(
                    matches!(node, YamlNode::Mapping),
                    "table body must be a map"
                );
                visit_cells_back(e, read_table(&mut reader, e, 3)?, &mut visit)?;
            } else {
                // Each kind of element as the content contract has it.
                let value = reader.value(node, 3)?;
                ensure!(
                    match kind {
                        "text" => scalar(&value),
                        "formula" => value.is_string(),
                        _ => value.is_null(),
                    },
                    "body element value does not match its kind"
                );
                element(e, kind, value, &mut visit)?;
            }
            ensure!(reader.key()?.is_none(), "body element must have one member");
        }
        placed(&mut place, &mut pending, &mut visit)?;
        ensure!(place == metadata.len(), "element count changed");
    }
    reader.end()?;
    ensure!(read, "page elements missing");
    // The members besides the elements, as the contract checks them.
    let mut part = Value::Object(fields);
    part["elements"] = json!([]);
    crate::data::validate("content", &part)?;
    let Value::Object(mut fields) = part else {
        unreachable!("the part is an object")
    };
    fields.remove("elements");
    Ok(fields)
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
    /// The ID of the element or the cell.
    pub(crate) fn id(self) -> Result<&'a str> {
        match self {
            Self::Element(element) => string(&element["id"]),
            Self::Cell(cell) => cell.id(),
        }
    }

    /// The font of the element or the cell, null for none.
    pub(crate) fn font(self) -> std::borrow::Cow<'a, Value> {
        match self {
            Self::Element(element) => std::borrow::Cow::Borrowed(&element["font"]),
            Self::Cell(cell) => cell.font().unwrap_or(std::borrow::Cow::Owned(Value::Null)),
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
pub(crate) fn yaml(page: &Value, metadata: &[Meta], diagrams: &Diagrams) -> Result<Vec<u8>> {
    let mut bytes = vec![];
    yaml_to(page, metadata, diagrams, &mut bytes)?;
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
    diagrams: &Diagrams,
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
    page_yaml(fields, &elements, metadata, diagrams, out)
}

/// [`yaml_to`] of a page encoding made, written to `out` as it is made.
pub(crate) fn encoded_yaml_to(
    page: &Encoded,
    metadata: &[Meta],
    diagrams: &Diagrams,
    out: &mut impl std::io::Write,
) -> Result<()> {
    ensure!(
        page.elements.len() == metadata.len(),
        "element count changed"
    );
    let text = |place: usize| -> Result<&Value> {
        match &page.elements[place] {
            Encoding::Element(element) => Ok(&element.value),
            Encoding::Cells(_) => bail!("a table is no text of a diagram"),
        }
    };
    let mut elements = Vec::with_capacity(page.elements.len());
    for (place, (element, e)) in page.elements.iter().zip(metadata).enumerate() {
        if let Some((diagram, first)) = diagrams.of(place) {
            if first {
                elements.push(Out::Diagram(diagram.body(&text)?));
            }
            continue;
        }
        elements.push(match element {
            Encoding::Element(element) => Out::Value(element.kind, &element.value),
            Encoding::Cells(values) => {
                let table = Table::of(e)?;
                let keys = table.keys(|i, j| values.get(i, j))?;
                let order = table.key_order(&keys)?;
                Out::Cells(Box::new(CellsBody {
                    table,
                    keys,
                    values,
                    order: order.iter().cloned().collect(),
                    listed: order,
                }))
            }
        });
    }
    write_page_yaml(&page.fields, &elements, out)
}

/// The YAML of a page with the members `fields` besides its elements, each
/// element given by its kind and value.
fn page_yaml(
    fields: &serde_json::Map<String, Value>,
    elements: &[(&str, &Value)],
    metadata: &[Meta],
    diagrams: &Diagrams,
    out: &mut impl std::io::Write,
) -> Result<()> {
    ensure!(elements.len() == metadata.len(), "element count changed");
    let text = |place: usize| -> Result<&Value> { Ok(elements[place].1) };
    let mut outs = Vec::with_capacity(elements.len());
    for (place, (&(kind, value), e)) in elements.iter().zip(metadata).enumerate() {
        if let Some((diagram, first)) = diagrams.of(place) {
            if first {
                outs.push(Out::Diagram(diagram.body(&text)?));
            }
            continue;
        }
        outs.push(if e["type"] == "table" {
            let table = Table::of(e)?;
            let values = table_values(e, value)?;
            let keys = table.keys(|i, j| values.get(i, j).copied())?;
            Out::Tree(kind, value, table.key_order(&keys)?)
        } else {
            Out::Value(kind, value)
        });
    }
    write_page_yaml(fields, &outs, out)
}

/// Writes the page with the members `fields` besides its elements.
fn write_page_yaml(
    fields: &serde_json::Map<String, Value>,
    elements: &[Out<'_>],
    out: &mut impl std::io::Write,
) -> Result<()> {
    serde_saphyr::to_io_writer(out, &Page { fields, elements })?;
    Ok(())
}

/// An element of a page as it is written: its kind and value, a table's with
/// the order of its keys, or a table by the values of its cells.
enum Out<'a> {
    Value(&'a str, &'a Value),
    Tree(&'a str, &'a Value, Vec<String>),
    Cells(Box<CellsBody<'a>>),
    /// The elements of a diagram, written together.
    Diagram(Ordered<'a>),
}

/// A table written from the values of its cells: as [`table_body`] makes its
/// tree, with the keys of each node in the order [`Node`] writes them.
struct CellsBody<'a> {
    table: Table<'a>,
    keys: PerCell<Rc<str>>,
    values: &'a PerCell<Value>,
    /// The keys in the order they are written first.
    listed: Vec<String>,
    order: std::collections::HashSet<String>,
}

struct Page<'a> {
    fields: &'a serde_json::Map<String, Value>,
    elements: &'a [Out<'a>],
}

impl Serialize for Page<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        let elements = Elements(self.elements);
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

struct Elements<'a>(&'a [Out<'a>]);

impl Serialize for Elements<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        let mut seq = s.serialize_seq(Some(self.0.len()))?;
        for element in self.0 {
            seq.serialize_element(&Element(element))?;
        }
        seq.end()
    }
}

/// An element of a page.
struct Element<'a>(&'a Out<'a>);

impl Serialize for Element<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        let mut map = s.serialize_map(Some(1))?;
        match self.0 {
            Out::Value(kind, value) => map.serialize_entry(kind, value)?,
            Out::Tree(kind, value, order) => map.serialize_entry(kind, &TableBody(value, order))?,
            Out::Cells(body) => map.serialize_entry("table", body)?,
            Out::Diagram(body) => map.serialize_entry(DIAGRAM, body)?,
        }
        map.end()
    }
}

impl Serialize for CellsBody<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        let mut map = s.serialize_map(None)?;
        let value = |record: usize, j: usize| {
            self.values
                .get(record, j)
                .ok_or_else(|| serde::ser::Error::custom("table cell value missing"))
        };
        let heading = |record: usize| -> std::result::Result<Vec<&Value>, S::Error> {
            let cells = self
                .table
                .cells(record)
                .map_err(serde::ser::Error::custom)?;
            (0..cells.len()).map(|j| value(record, j)).collect()
        };
        match self.table.headings {
            0 => {}
            1 => map.serialize_entry("columns", &heading(0)?)?,
            n => map.serialize_entry(
                "columns",
                &(0..n)
                    .map(heading)
                    .collect::<std::result::Result<Vec<_>, _>>()?,
            )?,
        }
        map.serialize_entry("rows", &CellNodes(self, &self.table.roots))?;
        map.end()
    }
}

/// Nodes of a [`CellsBody`].
struct CellNodes<'a>(&'a CellsBody<'a>, &'a [Slot]);

impl Serialize for CellNodes<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        let mut seq = s.serialize_seq(Some(self.1.len()))?;
        for &slot in self.1 {
            seq.serialize_element(&CellNode(self.0, slot))?;
        }
        seq.end()
    }
}

/// A node of a [`CellsBody`], written as [`Node`] writes the node
/// [`table_body`] makes of it.
struct CellNode<'a>(&'a CellsBody<'a>, Slot);

impl Serialize for CellNode<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        let CellNode(body, slot) = *self;
        let error = serde::ser::Error::custom;
        let mut entries: Vec<(&str, &Value)> = vec![];
        let mut marks = vec![];
        for (record, j, _) in body.table.node_cells(slot).map_err(error)? {
            let value = body
                .values
                .get(record, j)
                .ok_or_else(|| serde::ser::Error::custom("table cell value missing"))?;
            if is_mark(&body.table.cells(record).map_err(error)?[j]) {
                marks.push(value);
            } else {
                entries.push((Table::key(&body.keys, record, j).map_err(error)?, value));
            }
        }
        // As a map of them holds them: in the order of their keys, the
        // last value of a key.
        entries.sort_by_key(|&(key, _)| key);
        let mut unique: Vec<(&str, &Value)> = Vec::with_capacity(entries.len());
        for entry in entries {
            match unique.last_mut() {
                Some(last) if last.0 == entry.0 => *last = entry,
                _ => unique.push(entry),
            }
        }
        let children = body.table.under(slot);
        let len = unique.len() + usize::from(!marks.is_empty()) + usize::from(!children.is_empty());
        let mut map = s.serialize_map(Some(len))?;
        for key in &body.listed {
            if let Ok(at) = unique.binary_search_by(|&(entry, _)| entry.cmp(key)) {
                map.serialize_entry(key, unique[at].1)?;
            }
        }
        for &(key, value) in &unique {
            if !body.order.contains(key) {
                map.serialize_entry(key, value)?;
            }
        }
        if !marks.is_empty() {
            map.serialize_entry(MARKS, &marks)?;
        }
        if !children.is_empty() {
            map.serialize_entry(CHILDREN, &CellNodes(body, children))?;
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

    /// Checking a page moves its members and values into each part and back:
    /// the page is as it was after a check that passes and one that fails.
    #[test]
    fn checking_a_page_gives_its_values_back() {
        let mut fields = serde_json::Map::new();
        fields.insert("page_id".into(), json!("sheet-1"));
        let elements = (0..CONTENT_PART + 1)
            .map(|i| BodyElement {
                kind: "text",
                value: json!(format!("text {i}")),
            })
            .collect();
        let page = Content { fields, elements };
        let parts = std::cell::Cell::new(0);
        let mut checked = page.clone();
        checked
            .check(|part| {
                parts.set(parts.get() + 1);
                assert_eq!(part["page_id"], "sheet-1");
                assert!(part["elements"][0]["text"].is_string());
                Ok(())
            })
            .unwrap();
        assert_eq!((parts.get(), &checked), (2, &page));
        let mut failed = page.clone();
        assert!(failed.check(|_| bail!("invalid")).is_err());
        assert_eq!(failed, page);
    }

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
        let page = json!({"elements":body,"page_id":"p","schema_version":"7","title":"t"});
        // Values in the order of the columns, then marks, then children.
        let typed = crate::documents::metadata_from(metadata.clone()).unwrap();
        let text = String::from_utf8(yaml(&page, &typed, &Diagrams::default()).unwrap()).unwrap();
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
schema_version: "7"
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
