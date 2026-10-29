//! Coordinate-free editable document bodies and their separately stored bindings.
use super::*;
use crate::document_body::{Content, Encoded};
use crate::document_structure::{Element, ElementCell, Role};
use crate::extraction::CellValue;
use std::collections::HashSet;

mod groups;
pub(super) use groups::Groups;

/// The bytes an ID part keeps as they are; [`token`] escapes the others.
fn plain(byte: u8) -> bool {
    matches!(byte, b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'.' | b'/' | b'#')
}

pub(super) fn token(kind: &str, parts: &[&str]) -> String {
    // Keep the immutable identity components readable. Escape the separator
    // and escape marker so user-supplied operation IDs cannot alias each other.
    // Written into one string: a large sheet makes an ID for every cell.
    let mut id =
        String::with_capacity(kind.len() + parts.iter().map(|p| p.len() + 1).sum::<usize>());
    id.push_str(kind);
    id.push('_');
    for (index, part) in parts.iter().enumerate() {
        if index > 0 {
            id.push('_');
        }
        for byte in part.bytes() {
            if plain(byte) {
                id.push(char::from(byte));
            } else {
                id.push_str(&format!("%{byte:02X}"));
            }
        }
    }
    id
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
/// other string. A part is read as [`token`] writes it, so the ID is the one
/// it makes of them: kept bytes as they are, any other as `%` and two
/// upper-case hex digits.
fn parts(id: &str) -> Option<(&str, Vec<String>)> {
    let (kind, rest) = id.split_once('_')?;
    let parts = rest
        .split('_')
        .map(|part| {
            let mut bytes = Vec::with_capacity(part.len());
            let mut rest = part.as_bytes();
            while let Some((&byte, after)) = rest.split_first() {
                if byte == b'%' {
                    let hex = after.get(..2)?;
                    if !hex.iter().all(|d| matches!(d, b'0'..=b'9' | b'A'..=b'F')) {
                        return None;
                    }
                    let escaped = u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()?;
                    if plain(escaped) {
                        return None;
                    }
                    bytes.push(escaped);
                    rest = &after[2..];
                } else if plain(byte) {
                    bytes.push(byte);
                    rest = after;
                } else {
                    return None;
                }
            }
            String::from_utf8(bytes).ok()
        })
        .collect::<Option<Vec<_>>>()?;
    Some((kind, parts))
}

/// Where the elements of a page write in its grid, read from their IDs: a
/// shape's text, a formula of the sheet, or a cell of the body.
pub(super) struct Bindings {
    /// The formula in each cell holding one, by row and column: looked up
    /// for every element without copying its row and column.
    formulas: HashMap<String, HashMap<String, String>>,
}

impl Bindings {
    pub(super) fn of(sheet: &Value) -> Result<Self> {
        let mut formulas: HashMap<String, HashMap<String, String>> = HashMap::new();
        for (id, target) in sheet["formulas"]
            .as_object()
            .context("layout formulas missing")?
        {
            let (row, column) = (string(&target["row"])?, string(&target["column"])?);
            ensure!(
                formulas
                    .entry(row.to_owned())
                    .or_default()
                    .insert(column.to_owned(), id.clone())
                    .is_none(),
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
            "cell" => match self.formulas.get(&row).and_then(|row| row.get(&column)) {
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
    /// The JSON of the font being added: a page adds one per cell, and most
    /// are added before.
    key: Vec<u8>,
}

impl Fonts {
    pub(super) fn add(&mut self, font: &Value) -> Value {
        json!(self.add_index(font))
    }

    /// [`Fonts::add`], the index as a number.
    pub(super) fn add_index(&mut self, font: &Value) -> u32 {
        self.key.clear();
        serde_json::to_writer(&mut self.key, font).unwrap();
        let key = std::str::from_utf8(&self.key).unwrap();
        let index = match self.index.get(key) {
            Some(index) => *index,
            None => {
                let index = self.list.len();
                self.index.insert(key.to_owned(), index);
                self.list.push(font.clone());
                index
            }
        };
        u32::try_from(index).expect("fewer fonts than 2^32")
    }
}

/// The member of a grid page that holds the fonts its `fonts` refer to by
/// index: a page has a font for each cell, and few fonts.
pub(super) const FONT_TABLE: &str = "font_table";

/// The font of `table` that `index` refers to, null for none.
/// The index of a font in a font table that `index`, a number or null, gives.
pub(super) fn font_index(index: &Value) -> Result<Option<u32>> {
    if index.is_null() {
        return Ok(None);
    }
    let index = index.as_u64().context("font index must be a number")?;
    Ok(Some(
        u32::try_from(index).with_context(|| format!("unknown font index {index}"))?,
    ))
}

/// [`table_font`] of a font held by its index.
pub(super) fn indexed_font(table: &Value, index: Option<u32>) -> Result<&Value> {
    match index {
        None => Ok(&Value::Null),
        Some(index) => table
            .get(index as usize)
            .with_context(|| format!("unknown font index {index}")),
    }
}

pub(super) fn table_font<'a>(table: &'a Value, index: &Value) -> Result<&'a Value> {
    if index.is_null() {
        return Ok(&Value::Null);
    }
    let index = index.as_u64().context("font index must be a number")?;
    usize::try_from(index)
        .ok()
        .and_then(|index| table.get(index))
        .with_context(|| format!("unknown font index {index}"))
}

/// The font of the field at `block`, `row` and `column` of a grid `page`, null
/// for none.
pub(super) fn grid_font<'a>(
    page: &'a Value,
    block: &str,
    row: &str,
    column: &str,
) -> Result<&'a Value> {
    table_font(&page[FONT_TABLE], &page["fonts"][block][row][column])
}

/// Gives the field at `block`, `row` and `column` of a grid `page` the font
/// `font`, kept once in the page's [`FONT_TABLE`].
pub(super) fn set_grid_font(page: &mut Value, block: &str, row: &str, column: &str, font: Value) {
    if !page[FONT_TABLE].is_array() {
        page[FONT_TABLE] = json!([]);
    }
    let table = page[FONT_TABLE].as_array_mut().unwrap();
    let index = table
        .iter()
        .position(|kept| *kept == font)
        .unwrap_or_else(|| {
            table.push(font);
            table.len() - 1
        });
    page["fonts"][block][row][column] = json!(index);
}

/// The font an element or table cell of `sheet`'s metadata refers to, null
/// for none.
pub(super) fn font<E>(sheet: &Value, element: &E) -> Result<Value>
where
    E: for<'k> std::ops::Index<&'k str, Output = Value> + ?Sized,
{
    Ok(table_font(&sheet["fonts"], &element["font"])?.clone())
}

/// The ID of the first cell of the record `row`.
pub(super) fn first_id(row: &MetaRow) -> Result<&str> {
    match row.cells.first() {
        Some(cell) => cell.id(),
        None => string(&Value::Null),
    }
}

/// What a text written against a table is to it: its title, written above
/// it, or a note on it, written beside or under it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum Description {
    Title,
    Note,
}

impl Description {
    /// The name a record of the table metadata gives it.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Title => "title",
            Self::Note => "note",
        }
    }

    /// The description `name` names, none for none.
    pub(crate) fn named(name: &str) -> Option<Self> {
        match name {
            "title" => Some(Self::Title),
            "note" => Some(Self::Note),
            _ => None,
        }
    }

    /// What the record `row` of a table holds as text written against the
    /// table, none for a record of the table's own.
    pub(crate) fn of(row: &MetaRow) -> Option<Self> {
        row.get("description")
            .and_then(Value::as_str)
            .and_then(Self::named)
    }
}

/// What a table cell of a page takes from the interpretation, and what
/// [`encode`] arranges the table with: its table, whether it is a cell of a
/// heading record (`heading`) or a mark (`mark`), the heading its column's
/// key is from (`label`), the row of its record's parent (`parent`: none when
/// the group places no record, null when the record has no parent), the
/// category it is a cell of or a blank cell under (`category`) and the
/// categories around it, outer first (`within`). A text written against the
/// table, which the table holds apart from its records, has only its table
/// and what it is to it (`description`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub(super) struct Group {
    pub(super) table: Option<Arc<str>>,
    pub(super) description: Option<Description>,
    pub(super) heading: bool,
    pub(super) mark: bool,
    pub(super) label: Option<Arc<str>>,
    pub(super) parent: Option<Option<Arc<str>>>,
    pub(super) category: Option<Arc<str>>,
    pub(super) within: Arc<[Arc<str>]>,
}

/// The strings of groups, each made once and shared by the groups that have
/// it: the cells under a heading name it as their `label`, those of a record
/// the row of its parent, and a page has a group for each cell of its tables.
#[derive(Default)]
pub(super) struct Shared(HashSet<Arc<str>>);

impl Shared {
    pub(super) fn get(&mut self, text: &str) -> Arc<str> {
        if let Some(shared) = self.0.get(text) {
            return shared.clone();
        }
        let shared: Arc<str> = text.into();
        self.0.insert(shared.clone());
        shared
    }

    /// The list of `texts`, each shared.
    fn list<'t>(&mut self, texts: impl IntoIterator<Item = &'t str>) -> Arc<[Arc<str>]> {
        texts.into_iter().map(|text| self.get(text)).collect()
    }
}

/// A block, row or column name of a [`GridPage`], shared by its values: one
/// pointer wide, as a large sheet has a value for each cell, each with three
/// names.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct Name(Arc<Box<str>>);

impl std::ops::Deref for Name {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

impl std::borrow::Borrow<str> for Name {
    fn borrow(&self) -> &str {
        self
    }
}

impl From<&str> for Name {
    fn from(name: &str) -> Self {
        Self(Arc::new(name.into()))
    }
}

impl From<String> for Name {
    fn from(name: String) -> Self {
        Self(Arc::new(name.into_boxed_str()))
    }
}

impl std::fmt::Debug for Name {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&**self, f)
    }
}

impl std::fmt::Display for Name {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self)
    }
}

/// The names of a [`GridPage`], each made once.
#[derive(Default)]
pub(super) struct Names(HashSet<Name>);

impl Names {
    pub(super) fn get(&mut self, name: &str) -> Name {
        if let Some(shared) = self.0.get(name) {
            return shared.clone();
        }
        let shared = Name::from(name);
        self.0.insert(shared.clone());
        shared
    }
}

/// Values by text keys in the order of their keys, found by halves: of
/// values given one key, the last, as a map given them in turn keeps it.
struct Keyed<'k, T>(Vec<(&'k str, T)>);

impl<T> Keyed<'_, T> {
    fn get(&self, key: &str) -> Option<&T> {
        let end = self.0.partition_point(|&(at, _)| at <= key);
        let (at, value) = self.0.get(end.checked_sub(1)?)?;
        (*at == key).then_some(value)
    }
}

/// The group of a cell no table holds.
static NO_GROUP: std::sync::LazyLock<Group> = std::sync::LazyLock::new(Group::default);

/// The group of the cell `cell` of the element metadata, or [`NO_GROUP`].
fn group_of<'a>(groups: &'a Groups, cell: &MetaCell) -> Result<&'a Group> {
    Ok(groups.get(cell.id()?).unwrap_or(&NO_GROUP))
}

/// The group of every table cell, as the element `metadata` of a page records
/// it: its table and what [`arrange`] took from the interpretation, a cell of
/// a heading record as a `heading`, a mark as a `mark`, the heading its
/// column's key is from as its `label`, the row of its record's parent as
/// its `parent`, null for none (for the first record of a category, the
/// parent it has without the category), a cell of a category or a blank one
/// under it the category as its `category`, and the categories its record is
/// in, or a category's cell those around it, as its `within`. A text the
/// table holds as its title or a note has only its table and `description`.
pub(super) fn metadata_groups(metadata: &[Meta]) -> Result<Groups> {
    let mut groups = Groups::default();
    let mut shared = Shared::default();
    for element in metadata {
        if element["type"] != "table" {
            continue;
        }
        let table: Option<Arc<str>> = element["id"].as_str().map(Arc::from);
        let rows = element.rows()?;
        let headings = element["headings"].as_u64().unwrap_or(0);
        let mut record_rows = HashMap::new();
        for row in rows {
            let first = first_id(row)?;
            if let (Some(id), Some((axis, _))) = (row["id"].as_str(), cell_axes(first)) {
                record_rows.insert(id, axis);
            }
        }
        let empty = serde_json::Map::new();
        let categories = element["categories"].as_object().unwrap_or(&empty);
        let mut node_cells: HashMap<&str, &str> = HashMap::new();
        for (key, category) in categories {
            node_cells.insert(key, key);
            for cell in category["cells"].as_array().into_iter().flatten() {
                node_cells.insert(string(cell)?, key);
            }
        }
        // The categories around each record, outer first, by its ID.
        let mut within: HashMap<&str, Vec<&str>> = HashMap::new();
        for (i, row) in (0..).zip(rows) {
            if let Some(description) = Description::of(row) {
                for cell in &row.cells {
                    let group = Group {
                        table: table.clone(),
                        description: Some(description),
                        ..Group::default()
                    };
                    groups.insert(cell.id()?, group);
                }
                continue;
            }
            // The parent the record would have without the categories its
            // cells hold: that of the outermost of them.
            let mut parent = row["parent"].as_str();
            while let Some(key) = parent.filter(|p| categories.contains_key(*p)) {
                let first = |id: &str| cell_axes(id).map(|(row, _)| row);
                if first(key) != first(first_id(row)?) {
                    parent = None;
                    break;
                }
                parent = categories[key]["parent"].as_str();
            }
            let parent = parent
                .and_then(|p| record_rows.get(p))
                .map(|axis| shared.get(axis));
            let mut chain: Vec<&str> = match row["parent"].as_str() {
                Some(p) if categories.contains_key(p) => {
                    let mut chain = around(p, categories, &within)?;
                    chain.push(p);
                    chain
                }
                Some(p) => within.get(p).cloned().unwrap_or_default(),
                None => vec![],
            };
            let node_chains: HashMap<&str, Vec<&str>> = row
                .cells
                .iter()
                .filter_map(|c| node_cells.get(c.id().ok()?).copied())
                .map(|key| Ok((key, around(key, categories, &within)?)))
                .collect::<Result<_>>()?;
            if let Some(id) = row["id"].as_str() {
                within.insert(id, std::mem::take(&mut chain));
            }
            let chain = row["id"].as_str().map_or(&chain, |id| &within[id]);
            let chain = shared.list(chain.iter().copied());
            let node_chains: HashMap<&str, Arc<[Arc<str>]>> = node_chains
                .into_iter()
                .map(|(key, chain)| (key, shared.list(chain)))
                .collect();
            for cell in &row.cells {
                let mut group = Group {
                    table: table.clone(),
                    ..Group::default()
                };
                if i < headings {
                    group.heading = true;
                } else {
                    group.parent = Some(parent.clone());
                }
                let id = cell.id()?;
                let category = node_cells
                    .get(id)
                    .copied()
                    .or_else(|| cell["category"].as_str());
                group.within = match node_cells.get(id) {
                    Some(key) => node_chains[key].clone(),
                    None => chain.clone(),
                };
                group.category = category.map(|key| shared.get(key));
                group.mark = cell["mark"] == true;
                let column = cell.column()?;
                group.label = cell["label"]
                    .as_str()
                    .or_else(|| element["labels"][column].as_str())
                    .map(|label| shared.get(label));
                groups.insert(cell.id()?, group);
            }
        }
    }
    groups.shrink_to_fit();
    Ok(groups)
}

/// The categories around the category `key` of the table `categories`,
/// outer first: those around the record or category it is under, by
/// `within` for a record.
fn around<'a>(
    key: &'a str,
    categories: &'a serde_json::Map<String, Value>,
    within: &HashMap<&'a str, Vec<&'a str>>,
) -> Result<Vec<&'a str>> {
    let mut chain = vec![];
    let mut at = key;
    loop {
        ensure!(chain.len() <= categories.len(), "categories nest in a loop");
        let entry = categories
            .get(at)
            .with_context(|| format!("category {at} is no category"))?;
        match entry["parent"].as_str() {
            Some(parent) if categories.contains_key(parent) => {
                chain.insert(0, parent);
                at = parent;
            }
            Some(record) => {
                let mut outer = within.get(record).cloned().unwrap_or_default();
                outer.extend(chain);
                return Ok(outer);
            }
            None => return Ok(chain),
        }
    }
}

/// The deepest a record may lie in the tree of a table body: a deeper one is
/// put under its ancestor at this depth.
const MAX_TREE_DEPTH: usize = 16;

/// Puts in each table of the element `metadata` of `sheet` the tree its
/// body shows, from the `groups` of its cells: the leading records whose
/// cells are all headings as its `headings`, by logical column the heading
/// most of its cells take their key from as its `labels`, each mark as a
/// `mark` and each other record's `parent`, the latest earlier record a cell
/// of it names. A record none of whose cells has a group parent, as one an
/// insertion adds, takes the parent of the record before it. One whose cells
/// name no earlier record takes the parent of the next record naming one
/// before it, so that a stray record does not cut the records after it off
/// their parents. A record is put under the nearest ancestor of its parent
/// that keeps every record between them in its tree, so that the tree reads
/// in the order of the records. The categories ([`categorize`]) go in the
/// tree as its `categories`, each where the first of its records would be
/// without it, with its records under it: no record is under a record of
/// another category. A blank cell under a category is marked with it as its
/// `category`.
fn arrange(sheet: &LayoutSheet, groups: &Groups, metadata: &mut [Meta]) -> Result<()> {
    let position = |axis: &str, id: &str| sheet.axis(axis).position(id);
    // The last row of each merged cell, by its first row and column.
    let mut bottoms: HashMap<(u64, u64), u64> = HashMap::new();
    for range in sheet["merges"].as_array().into_iter().flatten() {
        let (start, end) = string(range)?
            .split_once(':')
            .context("invalid merge range")?;
        let (x, y) = excel::coordinate(start)?;
        let (_, bottom) = excel::coordinate(end)?;
        bottoms.insert((u64::from(y), u64::from(x)), u64::from(bottom));
    }
    // Whether the record `row` lies below the merge of the cell `key`.
    let beyond = |key: &str, row: &MetaRow| -> bool {
        let record = row
            .cells
            .first()
            .and_then(|cell| cell.id().ok())
            .and_then(cell_axes);
        let (Some((key_row, key_column)), Some((record, _))) = (cell_axes(key), record) else {
            return false;
        };
        match (
            position("rows", &key_row),
            position("columns", &key_column),
            position("rows", &record),
        ) {
            (Some(y), Some(x), Some(at)) => bottoms.get(&(y, x)).is_some_and(|&b| at > b),
            _ => false,
        }
    };
    for element in metadata.iter_mut().filter(|e| e["type"] == "table") {
        let Meta::Table {
            fields: element,
            rows,
        } = element
        else {
            bail!("table rows missing");
        };
        let texts = split_descriptions(rows);
        let group = |cell: &MetaCell| group_of(groups, cell);
        let mut headings = 0;
        for row in rows.iter() {
            let mut cells = row.cells.iter();
            if !cells.all(|c| group(c).is_ok_and(|g| g.heading)) {
                break;
            }
            headings += 1;
        }
        let mut heading_cells = HashMap::new();
        for row in &rows[..headings] {
            for cell in &row.cells {
                heading_cells.insert(cell.id()?.to_owned(), heading_cells.len());
            }
        }
        let mut votes: BTreeMap<String, HashMap<String, usize>> = BTreeMap::new();
        let mut record_index = HashMap::new();
        for (i, row) in rows.iter().enumerate().skip(headings) {
            let first = first_id(row)?;
            if let Some((axis, _)) = cell_axes(first) {
                record_index.insert(axis, i);
            }
        }
        let mut proposed: Vec<(Option<usize>, bool)> = vec![(None, true); rows.len()];
        for (i, row) in rows.iter_mut().enumerate().skip(headings) {
            let mut known = false;
            for cell in &mut row.cells {
                let g = group(cell)?;
                if g.mark {
                    cell["mark"] = json!(true);
                    continue;
                }
                if let Some(label) = g
                    .label
                    .as_deref()
                    .filter(|l| heading_cells.contains_key(*l))
                {
                    *votes
                        .entry(cell.column()?.to_owned())
                        .or_default()
                        .entry(label.to_owned())
                        .or_default() += 1;
                }
                if let Some(parent) = &g.parent {
                    known = true;
                    if let Some(&j) = parent.as_deref().and_then(|p| record_index.get(p))
                        && j < i
                    {
                        proposed[i].0 = proposed[i].0.max(Some(j));
                    }
                }
            }
            proposed[i].1 = known;
        }
        let mut labels = serde_json::Map::new();
        for (column, counts) in votes {
            let best = counts
                .into_iter()
                .max_by(|a, b| {
                    a.1.cmp(&b.1)
                        .then_with(|| heading_cells[&b.0].cmp(&heading_cells[&a.0]))
                })
                .map(|(label, _)| label);
            if let Some(label) = best {
                labels.insert(column, json!(label));
            }
        }
        // A cell under another heading of its own column in the leading
        // records than most of the column, as a physical name on the second
        // line of a record under a logical name, keeps its own. A cell across
        // columns under the heading of another column keeps its column's.
        let mut heading_columns: HashMap<String, String> = HashMap::new();
        for row in &rows[..headings] {
            for cell in &row.cells {
                heading_columns.insert(cell.id()?.to_owned(), cell.column()?.to_owned());
            }
        }
        for row in rows.iter_mut().skip(headings) {
            for cell in &mut row.cells {
                let g = group(cell)?;
                if g.mark {
                    continue;
                }
                let column = cell.column()?;
                if let Some(label) = g
                    .label
                    .as_deref()
                    .filter(|l| heading_columns.get(*l).map(String::as_str) == Some(column))
                    && labels.get(column).and_then(Value::as_str) != Some(label)
                {
                    cell["label"] = json!(label);
                }
            }
        }
        let sorted = categorize(rows, headings, groups, &beyond)?;
        let category = |i: usize| sorted.of[i].as_deref();
        // The records and categories from the root of the tree to the last
        // one put in it.
        let mut path: Vec<Node<'_>> = vec![];
        let mut assigned: Vec<Option<Node<'_>>> = vec![None; rows.len()];
        let mut placed: BTreeMap<&str, Option<Node<'_>>> = BTreeMap::new();
        // The first parent proposed after each record, found in one pass
        // rather than by scanning the rest of the table for every record.
        let mut later = vec![None; rows.len()];
        for i in (0..rows.len().saturating_sub(1)).rev() {
            later[i] = proposed[i + 1].0.or(later[i + 1]);
        }
        for i in headings..rows.len() {
            let (proposal, known) = proposed[i];
            // A category is put where the first of its records would be
            // without it, and that record under it.
            for key in &sorted.starts[i] {
                let around = sorted.nodes[key].parent.as_deref();
                let parent = proposal.filter(|&p| category(p) == around);
                placed.insert(
                    key,
                    put(&mut path, &assigned, around, parent, MAX_TREE_DEPTH - 1),
                );
                path.push(Node::Category(key));
            }
            let same = |p: &usize| category(*p) == category(i);
            let mut parent = proposal.filter(same);
            if parent.is_none() && !known && i > headings && same(&(i - 1)) {
                parent = match assigned[i - 1] {
                    Some(Node::Record(p)) => Some(p),
                    _ => None,
                };
            }
            // A record naming no earlier record that the next record naming
            // one passes over goes with that record, so that it does not cut
            // the records after it off their parents.
            if parent.is_none() && known && i > headings {
                parent = later[i].filter(|&p| p < i && same(&p));
            }
            assigned[i] = put(&mut path, &assigned, category(i), parent, MAX_TREE_DEPTH);
            path.push(Node::Record(i));
        }
        let ids: Vec<Value> = rows.iter().map(|r| r["id"].clone()).collect();
        // A category is named by its first cell, as the cell of a column
        // deleted under it leaves another.
        let name = |key: &str| sorted.nodes[key].cells[0].clone();
        let id_of = |node: &Node<'_>| match node {
            Node::Record(p) => ids[*p].clone(),
            Node::Category(key) => json!(name(key)),
        };
        for (row, parent) in rows.iter_mut().zip(&assigned) {
            if let Some(parent) = parent {
                row["parent"] = id_of(parent);
            }
        }
        for &(i, at, ref key) in &sorted.covered {
            rows[i].cells[at]["category"] = json!(name(key));
        }
        if headings > 0 {
            element["headings"] = json!(headings);
        }
        if !labels.is_empty() {
            element["labels"] = Value::Object(labels);
        }
        if !sorted.nodes.is_empty() {
            let mut categories = serde_json::Map::new();
            for (key, node) in &sorted.nodes {
                let mut entry = json!({});
                if let Some(parent) = &placed[key.as_str()] {
                    entry["parent"] = id_of(parent);
                }
                if node.cells.len() > 1 {
                    entry["cells"] = json!(node.cells[1..]);
                }
                categories.insert(node.cells[0].clone(), entry);
            }
            element["categories"] = Value::Object(categories);
        }
        rows.extend(texts);
    }
    Ok(())
}

/// Takes the title and notes of a table off the end of its `rows`, where
/// they follow its records.
pub(super) fn split_descriptions(rows: &mut Vec<MetaRow>) -> Vec<MetaRow> {
    let records = rows
        .iter()
        .position(|row| Description::of(row).is_some())
        .unwrap_or(rows.len());
    rows.split_off(records)
}

/// A node of the tree of a table as [`arrange`] builds it: a record, by its
/// place among the table's records, or a category, by its key.
#[derive(Clone, Copy, PartialEq)]
enum Node<'a> {
    Record(usize),
    Category(&'a str),
}

/// The parent of a node put in the tree after the nodes on `path`, from the
/// root to the last one put: the nearest ancestor of the record `parent`
/// that keeps every node between them in its tree, so that the tree reads
/// in the order of the records, or else the category `around` it, or none.
/// The path then leads to that parent; a node is put no deeper than at
/// `depth`.
fn put<'a>(
    path: &mut Vec<Node<'a>>,
    assigned: &[Option<Node<'a>>],
    around: Option<&'a str>,
    mut parent: Option<usize>,
    depth: usize,
) -> Option<Node<'a>> {
    let base = around
        .and_then(|key| path.iter().position(|n| *n == Node::Category(key)))
        .map_or(0, |at| at + 1);
    let mut found = false;
    while let Some(p) = parent {
        if let Some(at) = path[base..].iter().position(|n| *n == Node::Record(p)) {
            path.truncate(base + at + 1);
            found = true;
            break;
        }
        parent = match assigned[p] {
            Some(Node::Record(q)) => Some(q),
            _ => None,
        };
    }
    if !found {
        path.truncate(base);
    }
    if path.len() >= depth {
        path.truncate((depth - 1).max(base));
    }
    path.last().copied()
}

/// The categories of a table's records as [`arrange`] puts them in its tree.
struct Categories {
    /// By the key its cells name it by, each category kept.
    nodes: BTreeMap<String, CategoryNode>,
    /// By record, the key of the innermost category it is in.
    of: Vec<Option<String>>,
    /// By record, the keys of the categories whose cells it holds, outer
    /// first.
    starts: Vec<Vec<String>>,
    /// The blank cells under the cells of a category in their columns, by
    /// record and place in it, and the key of the category.
    covered: Vec<(usize, usize, String)>,
}

struct CategoryNode {
    /// The IDs of its cells, all in the first record of the category.
    cells: Vec<String>,
    /// The key of the category around it.
    parent: Option<String>,
}

/// The categories the `groups` of the cells of the table `rows` give, after
/// its first `headings` records. A category's cells lie in the first of its
/// records, which follow each other, each in the categories around it as
/// well; one that does not, or lies too deep for a record under it, is
/// none, and its cells are its first record's. A record whose cells the
/// groups do not place, as one an insertion adds, is in the categories of
/// the record before it, but not in one whose first cell is merged down to
/// rows above it (`beyond` tells whether the record lies below the merge),
/// as Excel does not grow a merge by a row inserted under it.
fn categorize(
    rows: &[MetaRow],
    headings: usize,
    groups: &Groups,
    beyond: &dyn Fn(&str, &MetaRow) -> bool,
) -> Result<Categories> {
    let group = |cell: &MetaCell| group_of(groups, cell);
    // The records holding each category's cells, the first holding the
    // category itself.
    let mut held: BTreeMap<String, Vec<(usize, usize)>> = BTreeMap::new();
    for (i, row) in rows.iter().enumerate().skip(headings) {
        for (at, cell) in row.cells.iter().enumerate() {
            if let Some(key) = group(cell)?.category.as_deref() {
                held.entry(key.to_owned()).or_default().push((i, at));
            }
        }
    }
    let first: BTreeMap<&str, usize> = held
        .iter()
        .map(|(key, at)| (key.as_str(), at.iter().map(|(i, _)| *i).min().unwrap()))
        .collect();
    let node_cell = |i: usize, g: &Group| {
        g.category
            .as_deref()
            .is_some_and(|key| first.get(key) == Some(&i))
    };
    let listed = |g: &Group| -> Vec<String> { g.within.iter().map(|k| k.to_string()).collect() };
    // The categories each record placed by the groups is in, outer first.
    let mut chains: Vec<Option<Vec<String>>> = vec![None; rows.len()];
    for (i, row) in rows.iter().enumerate().skip(headings) {
        let cells = &row.cells;
        let mut own: Option<Vec<String>> = None;
        let mut deepest: Option<Vec<String>> = None;
        for cell in cells {
            let g = group(cell)?;
            if node_cell(i, g) {
                let mut chain = listed(g);
                chain.push(g.category.as_deref().unwrap().to_owned());
                if deepest.as_ref().is_none_or(|d| chain.len() > d.len()) {
                    deepest = Some(chain);
                }
            } else if own.is_none() && g.parent.is_some() {
                own = Some(listed(g));
            }
        }
        chains[i] = own.or(deepest);
    }
    let mut kept: BTreeSet<String> = held.keys().cloned().collect();
    for chain in chains.iter_mut().flatten() {
        chain.retain(|k| kept.contains(k));
    }
    drop_broken(&mut kept, &mut chains, &first);
    let mut nodes = BTreeMap::new();
    for key in &kept {
        let start = first[key.as_str()];
        let chain = chains[start].as_ref().unwrap();
        let at = chain.iter().position(|k| k == key).unwrap();
        let cells = held[key]
            .iter()
            .filter(|(i, _)| *i == start)
            .map(|&(i, at)| Ok(rows[i].cells[at].id()?.to_owned()))
            .collect::<Result<Vec<_>>>()?;
        nodes.insert(
            key.clone(),
            CategoryNode {
                cells,
                parent: at.checked_sub(1).map(|p| chain[p].clone()),
            },
        );
    }
    // A record the groups do not place is in the categories of the record
    // before it that reach it.
    let mut of = vec![None; rows.len()];
    for i in headings..rows.len() {
        let chain = match &chains[i] {
            Some(chain) => chain.clone(),
            None if i > headings => {
                let mut chain = chains[i - 1].clone().unwrap_or_default();
                while let Some(key) = chain.last()
                    && beyond(&nodes[key].cells[0], &rows[i])
                {
                    chain.pop();
                }
                chain
            }
            None => vec![],
        };
        of[i] = chain.last().cloned();
        chains[i] = Some(chain);
    }
    let mut covered = vec![];
    for (key, at) in &held {
        if !kept.contains(key) {
            continue;
        }
        for &(i, place) in at.iter().filter(|(i, _)| *i != first[key.as_str()]) {
            let cell = &rows[i].cells[place];
            let member = chains[i].as_ref().is_some_and(|c| c.contains(key));
            if member && cell.get("formula").is_none() && cell["value"].is_null() {
                covered.push((i, place, key.clone()));
            }
        }
    }
    let mut starts = vec![vec![]; rows.len()];
    for key in &kept {
        starts[first[key.as_str()]].push(key.clone());
    }
    for (i, keys) in starts.iter_mut().enumerate() {
        let chain = chains[i].as_deref().unwrap_or_default();
        keys.sort_by_key(|k| chain.iter().position(|c| c == k));
    }
    Ok(Categories {
        nodes,
        of,
        starts,
        covered,
    })
}

/// Drops from `kept` and the `chains` of the records the categories whose
/// records do not follow each other from their `first`, or are not all in
/// the categories around them, or lie too deep for a record under them: the
/// first such in order, then again, until none is left. Whether a category's
/// records follow each other does not change as others are dropped; the
/// categories around them do, for the categories sharing a record with the
/// one dropped, so only those are looked at again.
fn drop_broken(
    kept: &mut BTreeSet<String>,
    chains: &mut [Option<Vec<String>>],
    first: &BTreeMap<&str, usize>,
) {
    // The records each category is in, in order.
    let mut members: BTreeMap<String, Vec<usize>> =
        kept.iter().map(|key| (key.clone(), vec![])).collect();
    for (i, chain) in chains.iter().enumerate() {
        for key in chain.iter().flatten() {
            if let Some(records) = members.get_mut(key)
                && records.last() != Some(&i)
            {
                records.push(i);
            }
        }
    }
    // The records the groups place, counted up to each record.
    let mut placed = vec![0; chains.len() + 1];
    for (i, chain) in chains.iter().enumerate() {
        placed[i + 1] = placed[i] + usize::from(chain.is_some());
    }
    let follows = |key: &str| match members[key].as_slice() {
        [] => false,
        records => {
            let (start, end) = (records[0], records[records.len() - 1]);
            start == first[key] && placed[end + 1] - placed[start] == records.len()
        }
    };
    let nested = |key: &str, chains: &[Option<Vec<String>>]| {
        let around = |i: usize| {
            let chain = chains[i].as_deref().unwrap_or_default();
            &chain[..chain.iter().position(|k| k == key).unwrap_or(chain.len())]
        };
        let records = &members[key];
        let outer = around(records[0]);
        outer.len() + 2 <= MAX_TREE_DEPTH && records.iter().all(|&i| around(i) == outer)
    };
    let scattered: BTreeSet<String> = kept.iter().filter(|key| !follows(key)).cloned().collect();
    let mut broken: BTreeSet<String> = kept
        .iter()
        .filter(|key| scattered.contains(*key) || !nested(key, chains))
        .cloned()
        .collect();
    while let Some(key) = broken.pop_first() {
        kept.remove(&key);
        let mut sharing = BTreeSet::new();
        for &i in &members[&key] {
            if let Some(chain) = chains[i].as_mut() {
                chain.retain(|k| *k != key);
                sharing.extend(chain.iter().cloned());
            }
        }
        for other in sharing.into_iter().filter(|k| !scattered.contains(k)) {
            if nested(&other, chains) {
                broken.remove(&other);
            } else {
                broken.insert(other);
            }
        }
    }
}

/// The digest of what [`classify`] takes from the elements of a structure
/// interpretation: each table's sheet and its cells with their roles and
/// headers. A layout classified from another interpretation, such as one an
/// earlier inference made, has another.
pub(super) fn interpretation_digest(structure: &[Element]) -> Result<String> {
    use crate::data::Lazy;
    // Hashed cell by cell, as each table is made: the cells are not copied.
    let tables = structure
        .iter()
        .filter(|e| e["kind"] == "table")
        .map(|element| {
            let cells = element.cells.iter().map(|cell| {
                Ok(Lazy::Value(
                    json!({"id":&*cell.id,"address":&*cell.address,"role":cell.role.as_str(),"headers":cell.headers}),
                ))
            });
            Ok(Lazy::Object(vec![
                ("sheet", Lazy::Borrowed(&element["sheet"])),
                ("cells", Lazy::Items(Box::new(cells))),
            ]))
        });
    crate::data::hash_encoded_lazy(Lazy::Items(Box::new(tables)))
}

/// The role and headers `structure`, an interpretation of the extraction the
/// layout belongs to, gives each table cell of the element metadata of
/// `layout`, by page and cell ID: the layout keeps only which table a cell
/// is in. Cells are matched by the keys their rows and columns have in the
/// extraction. [`interpret_cell`] puts them in a cell.
pub(super) fn interpret(layout: &Layout, structure: &[Element]) -> Result<Interpretation> {
    let mut pages = BTreeMap::new();
    for (page, sheet) in &layout.sheets {
        let identities = |axis: &str| -> Result<HashMap<String, String>> {
            Ok(sheet
                .axis(axis)
                .iter()
                .map(|(id, entry)| (entry.key.clone(), id.clone()))
                .collect())
        };
        let rows = identities("rows")?;
        let columns = identities("columns")?;
        let mut ids: HashMap<&str, String> = HashMap::new();
        for element in structure.iter().filter(|e| e["sheet"] == sheet["name"]) {
            for cell in &element.cells {
                let (column, row) = excel::coordinate(&cell.address)?;
                if let (Some(row), Some(column)) = (
                    rows.get(&format!("r{row}")),
                    columns.get(&excel::column_name(column)?),
                ) {
                    ids.insert(&cell.id, cell_id(row, column));
                }
            }
        }
        // The cells of text elements too: a table holds the text written
        // against it as its title and notes.
        let mut found: HashMap<String, (Value, Vec<String>)> = HashMap::new();
        for element in structure.iter().filter(|e| e["sheet"] == sheet["name"]) {
            for cell in &element.cells {
                if let Some(id) = ids.get(&*cell.id) {
                    let headers = cell
                        .headers
                        .iter()
                        .filter_map(|h| ids.get(&**h))
                        .cloned()
                        .collect();
                    found.insert(id.clone(), (json!(cell.role.as_str()), headers));
                }
            }
        }
        pages.insert(page.clone(), found);
    }
    Ok(pages)
}

/// What [`interpret`] gives the table cells of a layout, by page and cell ID.
pub(super) type Interpretation = BTreeMap<String, HashMap<String, (Value, Vec<String>)>>;

/// Puts in the table cell `cell`, given as JSON, of a table whose cells are
/// `present` the role and headers `found` gives it, the headers among those
/// cells. A cell the interpretation does not place in a table, such as one of
/// a row an insertion adds, is `unassigned`.
pub(super) fn interpret_cell(
    cell: &mut Value,
    found: &HashMap<String, (Value, Vec<String>)>,
    present: &HashSet<&str>,
) -> Result<()> {
    let (role, mut headers) = found
        .get(string(&cell["id"])?)
        .cloned()
        .unwrap_or_else(|| (json!("unassigned"), vec![]));
    headers.retain(|h| present.contains(h.as_str()));
    cell["role"] = role;
    cell["headers"] = json!(headers);
    Ok(())
}

/// Attach the interpreted table membership to immutable internal grid identities.
/// Text cells remain individually reversible: no lossy concatenation is performed.
/// The layout records the [`interpretation_digest`] of `structure`; the roles
/// and headers of the cells are not kept, [`interpret`] gives them.
pub(super) fn classify(
    layout: &mut Layout,
    extraction: &Extraction,
    structure: &[Element],
) -> Result<BTreeMap<String, Groups>> {
    layout["structure_sha256"] = json!(interpretation_digest(structure)?);
    interpreted_groups(layout, extraction, structure)
}

/// The groups [`classify`] gives the table cells of each page of `layout`,
/// by page, without classifying the layout.
pub(super) fn interpreted_groups<'e>(
    layout: &Layout,
    extraction: &'e Extraction,
    structure: &[Element],
) -> Result<BTreeMap<String, Groups>> {
    let mut classified = BTreeMap::new();
    for (page, sheet) in &layout.sheets {
        let mut groups = Groups::default();
        let mut shared = Shared::default();
        // The extraction cells of the sheet in the order of their addresses,
        // whose values tell the cells written under a row header from blank
        // ones: a large sheet has many cells, which a map of their values by
        // address took ten times the room of this order for.
        let source = extraction
            .sheets
            .iter()
            .find(|source| source["name"] == sheet["name"]);
        let address = |at: u32| {
            source.map_or("", |s| {
                s.cells
                    .get(at as usize)
                    .expect("an ordered cell is a cell of the sheet")
                    .address()
            })
        };
        let mut by_address: Vec<u32> = (0..source.map_or(0, |s| s.cells.len() as u32)).collect();
        by_address.sort_unstable_by(|&a, &b| address(a).cmp(address(b)));
        let value_at: &dyn Fn(&str) -> CellValue<'e> = &|wanted| {
            by_address
                .binary_search_by(|&at| address(at).cmp(wanted))
                .ok()
                .and_then(|place| source?.cells.get(by_address[place] as usize))
                .map_or(CellValue::Null, |cell| cell.value())
        };
        // The last row of each cell merged down, by its address.
        let mut merged_down: HashMap<&str, u32> = HashMap::new();
        for range in source
            .into_iter()
            .flat_map(|s| s["merges"].as_array().into_iter().flatten())
        {
            let (start, end) = string(range)?
                .split_once(':')
                .context("invalid merge range")?;
            let (_, top) = excel::coordinate(start)?;
            let (_, bottom) = excel::coordinate(end)?;
            if bottom > top {
                merged_down.insert(start, bottom);
            }
        }
        let merged_to = |address: &str| merged_down.get(address).copied();
        // Cells are matched by the keys their rows and columns have in the
        // extraction, which pending operations leave as they are; each
        // identity is held by its number in its axis's list.
        let identities = |axis: &str| -> (HashMap<&str, u32>, Vec<&str>) {
            let mut numbers = HashMap::new();
            let mut list = vec![];
            for (id, entry) in sheet.axis(axis).iter() {
                if numbers
                    .insert(entry.key.as_str(), list.len() as u32)
                    .is_none()
                {
                    list.push(id.as_str());
                } else {
                    // The last identity with a key, as a map of them by key gives it.
                    let number = numbers[entry.key.as_str()];
                    list[number as usize] = id.as_str();
                }
            }
            (numbers, list)
        };
        let (rows, row_ids) = identities("rows");
        let (columns, column_ids) = identities("columns");
        let lines =
            |&(row, column): &(u32, u32)| (row_ids[row as usize], column_ids[column as usize]);
        // The numbers of the row and column identities of each cell of the
        // interpretation on the page, by its ID in the interpretation: in the
        // order of the IDs, made as large as they become, as a large sheet
        // has many cells, and a map of them grown by doubling held them twice.
        let on_page = || structure.iter().filter(|e| e["sheet"] == sheet["name"]);
        let mut ids: Vec<(&str, (u32, u32))> =
            Vec::with_capacity(on_page().map(|e| e.cells.len()).sum());
        for element in on_page() {
            for cell in &element.cells {
                let (column, row) = excel::coordinate(&cell.address)?;
                if let (Some(&row), Some(&column)) = (
                    rows.get(format!("r{row}").as_str()),
                    columns.get(excel::column_name(column)?.as_str()),
                ) {
                    ids.push((&cell.id, (row, column)));
                }
            }
        }
        ids.sort_by(|a, b| a.0.cmp(b.0));
        let ids = Keyed(ids);
        // The tables each text element explains, by its ID: a text that
        // explains one table goes in it as its title or a note, and one that
        // explains several, as a title over tables side by side, stays apart.
        let mut explaining: HashMap<&str, usize> = HashMap::new();
        for element in on_page().filter(|e| e["kind"] == "table") {
            for id in element
                .get("descriptions")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                *explaining.entry(id).or_default() += 1;
            }
        }
        for element in structure
            .iter()
            .filter(|e| e["sheet"] == sheet["name"] && e["kind"] == "table")
        {
            let cells = &element.cells;
            let Some((row, column)) = cells.iter().find_map(|c| ids.get(&c.id)).map(lines) else {
                continue;
            };
            let table: Option<Arc<str>> = Some(token("table", &[&cell_id(row, column)]).into());
            // The role and place of each cell of the table, by its ID in the
            // interpretation, in the order of the IDs.
            let mut places = Vec::with_capacity(cells.len());
            // The names of each row, by column.
            let mut names: HashMap<u32, Vec<(u32, &str)>> = HashMap::new();
            for cell in cells {
                let (x, y) = excel::coordinate(&cell.address)?;
                places.push((&*cell.id, (cell.role, y, x)));
                if cell.role == Role::RowHeader {
                    names.entry(y).or_default().push((x, &*cell.id));
                }
            }
            places.sort_by(|a, b| a.0.cmp(b.0));
            let places = Keyed(places);
            for cell in cells {
                let Some((row, column)) = ids.get(&cell.id).map(lines) else {
                    continue;
                };
                let own = *places
                    .get(&cell.id)
                    .expect("each cell of the table has a place");
                let mut group = Group {
                    table: table.clone(),
                    heading: own.0 == Role::ColumnHeader,
                    mark: own.0 == Role::Text,
                    ..Group::default()
                };
                // The nearest heading over the cell names its key, and the
                // nearest name heading it from an earlier row its parent. A
                // record holding a deeper name that heads no part of the cell,
                // as a folder beside the group the cell is in, is no parent:
                // the cell is beside that folder, not in it.
                let headers: Vec<&str> = cell.headers.iter().map(|h| &**h).collect();
                let beside = |y: u32, x: u32| {
                    names.get(&y).is_some_and(|row| {
                        row.iter()
                            .any(|&(other, id)| other > x && !headers.contains(&id))
                    })
                };
                let mut label = None;
                let mut parent = None;
                for &header in &headers {
                    let (Some(&(role, y, x)), Some(at)) = (places.get(header), ids.get(header))
                    else {
                        continue;
                    };
                    match role {
                        Role::ColumnHeader if label.is_none_or(|(p, _)| (y, x) > p) => {
                            label = Some(((y, x), at));
                        }
                        Role::RowHeader
                            if y < own.1
                                && !beside(y, x)
                                && parent.is_none_or(|(p, _)| (y, x) > p) =>
                        {
                            parent = Some(((y, x), at));
                        }
                        _ => {}
                    }
                }
                group.label = label.map(|(_, at)| {
                    let (row, column) = lines(at);
                    shared.get(&cell_id(row, column))
                });
                group.parent = Some(parent.map(|(_, at)| shared.get(lines(at).0)));
                groups.insert_at(row, column, group);
            }
            // Each cell of a category names it, by the ID of its first cell,
            // as its `category`, and so does a blank cell under it in its
            // column; every other cell of the records of a category lists the
            // categories its row is in, outer first, as its `within`, and a
            // cell of a category those around it.
            let found: Vec<(String, Category)> = categories(cells, value_at, &merged_to)?
                .into_iter()
                .filter_map(|c| {
                    let (row, column) = lines(ids.get(c.cells[0].1)?);
                    Some((cell_id(row, column), c))
                })
                .collect();
            // Indexed once rather than scanned per cell: a table may have a
            // category every few rows. `owners` lists the categories of each
            // cell and `covering` those around each row, both in `found` order.
            let mut owners: HashMap<&str, Vec<usize>> = HashMap::new();
            for (i, (_, category)) in found.iter().enumerate() {
                for &(_, at) in &category.cells {
                    owners.entry(at).or_default().push(i);
                }
            }
            let mut starts: Vec<usize> = (0..found.len()).collect();
            starts.sort_by_key(|&i| found[i].1.row);
            let mut rows = BTreeSet::new();
            for cell in cells {
                rows.insert(excel::coordinate(&cell.address)?.1);
            }
            let mut covering: HashMap<u32, Vec<usize>> = HashMap::new();
            let (mut next, mut active) = (0, BTreeSet::new());
            for y in rows {
                while let Some(&i) = starts.get(next).filter(|&&i| found[i].1.row <= y) {
                    active.insert(i);
                    next += 1;
                }
                active.retain(|&i| found[i].1.end >= y);
                covering.insert(y, active.iter().copied().collect());
            }
            for cell in cells {
                let Some((row, column)) = ids.get(&cell.id).map(lines) else {
                    continue;
                };
                if cell.role == Role::ColumnHeader {
                    continue;
                }
                let (x, y) = excel::coordinate(&cell.address)?;
                let own = owners
                    .get(&*cell.id)
                    .and_then(|list| list.iter().find(|&&i| found[i].1.row == y))
                    .map(|&i| &found[i]);
                let within: Vec<&(String, Category)> = covering[&y]
                    .iter()
                    .map(|&i| &found[i])
                    .filter(|(_, c)| own.is_none_or(|(_, o)| c.cells[0].0 < o.cells[0].0))
                    .collect();
                let category = own.or_else(|| {
                    within.iter().copied().find(|(_, c)| {
                        c.row < y
                            && c.cells.iter().any(|&(at, _)| at == x)
                            && blank(value_at(&cell.address))
                    })
                });
                groups.update_at(row, column, |group| {
                    if let Some((key, _)) = category {
                        group.category = Some(shared.get(key));
                    }
                    if !within.is_empty() {
                        group.within = shared.list(within.iter().map(|(k, _)| k.as_str()));
                    }
                });
            }
            // The text it alone is explained by: the first written above its
            // first row is its title, and the rest are notes on it.
            let mut top = None;
            for cell in cells {
                let y = excel::coordinate(&cell.address)?.1;
                top = Some(top.map_or(y, |t: u32| t.min(y)));
            }
            let mut written = vec![];
            for id in element
                .get("descriptions")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .filter(|id| explaining.get(id) == Some(&1))
            {
                for text in on_page().filter(|e| e["kind"] == "text" && e["id"] == id) {
                    for cell in &text.cells {
                        let (x, y) = excel::coordinate(&cell.address)?;
                        written.push(((y, x), &*cell.id));
                    }
                }
            }
            written.sort_unstable();
            let mut titled = false;
            for ((y, _), id) in written {
                let Some((row, column)) = ids.get(id).map(lines) else {
                    continue;
                };
                let description = if !titled && top.is_some_and(|top| y < top) {
                    titled = true;
                    Description::Title
                } else {
                    Description::Note
                };
                let group = Group {
                    table: table.clone(),
                    description: Some(description),
                    ..Group::default()
                };
                groups.insert_at(row, column, group);
            }
        }
        groups.shrink_to_fit();
        classified.insert(page.clone(), groups);
    }
    Ok(classified)
}

/// The rows and columns the cells of each table of a page span, by table:
/// its top row, left column, bottom row and right column.
type Bounds = BTreeMap<String, (u64, u64, u64, u64)>;

/// The table of `bounds` innermost around each of the cells at `points`, rows
/// and columns: the smallest around it, none when none is or two are as small,
/// as a table nested in another lies inside its bounds. The points are taken
/// down the rows, each looking only at the tables across its row.
fn innermost<'b>(bounds: &'b Bounds, points: &[(u64, u64)]) -> Vec<Option<&'b str>> {
    let mut tables: Vec<_> = bounds
        .iter()
        .map(|(table, b)| (table.as_str(), b))
        .collect();
    tables.sort_by_key(|(_, b)| b.0);
    let mut order: Vec<usize> = (0..points.len()).collect();
    order.sort_by_key(|&i| points[i].0);
    let area = |b: &(u64, u64, u64, u64)| (b.2 - b.0 + 1) * (b.3 - b.1 + 1);
    let mut found = vec![None; points.len()];
    let mut next = 0;
    // The tables whose rows reach from above the point's row down to it.
    let mut across = vec![];
    for i in order {
        let (y, x) = points[i];
        while let Some(&(table, b)) = tables.get(next)
            && b.0 <= y
        {
            across.push((table, b));
            next += 1;
        }
        across.retain(|(_, b)| y <= b.2);
        let around = || across.iter().filter(|(_, b)| b.1 <= x && x <= b.3);
        let smallest = around().map(|(_, b)| area(b)).min();
        let mut candidates = around().filter(|(_, b)| Some(area(b)) == smallest);
        if let (Some(&(table, _)), None) = (candidates.next(), candidates.next()) {
            found[i] = Some(table);
        }
    }
    found
}

/// The cells of a record of a table: how many, how many of them headings, and
/// the group of the first in the order of their IDs.
#[derive(Default)]
struct Record {
    cells: usize,
    headings: usize,
    first: Option<(String, Group)>,
}

impl Record {
    fn add(&mut self, id: &str, group: &Group) {
        self.cells += 1;
        self.headings += usize::from(group.heading);
        if self
            .first
            .as_ref()
            .is_none_or(|(first, _)| id < first.as_str())
        {
            self.first = Some((id.to_owned(), group.clone()));
        }
    }
}

/// Puts each cell of `cells`, given a value the page's element metadata
/// holds none for, in the innermost table of the layout `sheet` whose cells
/// are around it, as an insertion's cells go: a cell of a record joins its
/// record, a heading of the leading records as a heading. The groups the
/// next [`encode`] arranges the page with are those of the metadata and these.
pub(super) fn adopt_cells(
    sheet: &LayoutSheet,
    groups: &mut Groups,
    cells: &[String],
) -> Result<()> {
    let position = |sheet: &LayoutSheet, id: &str| -> Option<(u64, u64)> {
        let (row, column) = cell_axes(id)?;
        Some((sheet.rows.position(&row)?, sheet.columns.position(&column)?))
    };
    let mut bounds = Bounds::new();
    // The cells of each record, by table and row, as the cells adopted join.
    let mut records: HashMap<String, HashMap<String, Record>> = HashMap::new();
    for (id, group) in groups.iter() {
        let Some(table) = group
            .table
            .as_deref()
            .filter(|_| group.description.is_none())
        else {
            continue;
        };
        if let Some((row, _)) = cell_axes(&id) {
            let records = records.entry(table.to_owned()).or_default();
            records.entry(row).or_default().add(&id, group);
        }
        let Some((y, x)) = position(sheet, &id) else {
            continue;
        };
        let b = bounds.entry(table.to_owned()).or_insert((y, x, y, x));
        b.0 = b.0.min(y);
        b.1 = b.1.min(x);
        b.2 = b.2.max(y);
        b.3 = b.3.max(x);
    }
    let placed: Vec<(&String, (u64, u64))> = cells
        .iter()
        .filter_map(|id| Some((id, position(sheet, id)?)))
        .collect();
    let points: Vec<(u64, u64)> = placed.iter().map(|&(_, point)| point).collect();
    for (&(id, _), table) in placed.iter().zip(innermost(&bounds, &points)) {
        if groups.contains_key(id) {
            continue;
        }
        let Some(table) = table else {
            continue;
        };
        let (row, column) = cell_axes(id).context("cell ID")?;
        // The other cells of its record, in the same table.
        let records = records.entry(table.to_owned()).or_default();
        let record = records.entry(row.clone()).or_default();
        let mut group = Group {
            table: Some(table.into()),
            ..Group::default()
        };
        if record.cells > 0 && record.headings == record.cells {
            group.heading = true;
        } else if let Some((_, first)) = &record.first {
            group.parent = first.parent.clone();
            group.within = first.within.clone();
        }
        record.add(&cell_id(&row, &column), &group);
        groups.insert(id, group);
    }
    Ok(())
}

/// Puts in the groups of the layout `sheet` the groups that `classified`,
/// the same sheet classified from the interpretation, gives the cells `ids`
/// coming back to the page, as when an operation deleting them is taken
/// back. A table the sheet holds other cells of keeps the ID the sheet gives
/// it.
pub(super) fn restore_groups(groups: &mut Groups, classified: &Groups, ids: &[String]) {
    let mut names: HashMap<&str, Arc<str>> = HashMap::new();
    for (id, group) in classified.iter() {
        if let (Some(table), Some(current)) = (
            group.table.as_deref(),
            groups.get(&id).and_then(|current| current.table.as_ref()),
        ) {
            names.entry(table).or_insert_with(|| current.clone());
        }
    }
    for id in ids {
        let Some(group) = classified.get(id) else {
            continue;
        };
        let mut group = group.clone();
        if let Some(name) = group.table.as_deref().and_then(|table| names.get(table)) {
            group.table = Some(name.clone());
        }
        groups.insert(id, group);
    }
}

/// The key of the first category of the element `metadata` of a page whose
/// cells are all among the `deleted` cell IDs while a record under it keeps a
/// cell, as when the row holding a category merged down over other records is
/// deleted: Excel would drop the category's value with that row.
pub(super) fn orphaned_category(
    metadata: &[Meta],
    deleted: &BTreeSet<String>,
) -> Result<Option<String>> {
    for element in metadata.iter().filter(|e| e["type"] == "table") {
        let Some(categories) = element["categories"].as_object() else {
            continue;
        };
        // The categories each record is in, by its ID.
        let mut within: HashMap<&str, Vec<&str>> = HashMap::new();
        let mut kept: BTreeSet<&str> = BTreeSet::new();
        for row in element.rows()? {
            let chain = match row["parent"].as_str() {
                Some(p) if categories.contains_key(p) => {
                    let mut chain = around(p, categories, &within)?;
                    chain.push(p);
                    chain
                }
                Some(p) => within.get(p).cloned().unwrap_or_default(),
                None => vec![],
            };
            if row
                .cells
                .iter()
                .any(|c| c.id().ok().is_some_and(|id| !deleted.contains(id)))
            {
                kept.extend(&chain);
            }
            if let Some(id) = row["id"].as_str() {
                within.insert(id, chain);
            }
        }
        for (key, entry) in categories {
            let mut cells = vec![key.as_str()];
            for cell in entry["cells"].as_array().into_iter().flatten() {
                cells.push(string(cell)?);
            }
            if cells.iter().all(|c| deleted.contains(*c)) && kept.contains(key.as_str()) {
                return Ok(Some(key.clone()));
            }
        }
    }
    Ok(None)
}

/// Whether a cell's value is blank: none, or only spaces.
fn blank(value: CellValue<'_>) -> bool {
    match value {
        CellValue::Null => true,
        CellValue::String(s) => s.trim().is_empty(),
        _ => false,
    }
}

/// A category of the records of a table: the cells of a row, as a group
/// merged down beside records or written over the first of them alone, that
/// head the records of its rows down to `end` without being the name of the
/// first of them.
struct Category<'a> {
    /// Its cells by their columns, left to right, by their IDs in the
    /// interpretation.
    cells: Vec<(u32, &'a str)>,
    row: u32,
    end: u32,
}

/// The categories of the table `cells` of an interpretation, the outer ones
/// first. A row header is one when a cell of a later row it heads is beside
/// it, its row holding a name to its right that does not head that cell (the
/// name of the first record), and nothing is written under it in its column
/// down to the last row it heads. Once a column holds a category, every row
/// header of it with a name to its right and nothing under it is one, as a
/// category over a single record. So is a value merged down over two records
/// or more beside their names (`merged_to` gives the last row of a cell
/// merged down), as a department over the functions it runs, when it lies
/// within the categories on its left. Cells of a row reaching as far are one
/// category; one crossing the bounds of a category left of it is none.
fn categories<'a, 'v>(
    cells: &'a [ElementCell],
    value_at: &dyn Fn(&str) -> CellValue<'v>,
    merged_to: &dyn Fn(&str) -> Option<u32>,
) -> Result<Vec<Category<'a>>> {
    // A category is of row headers: a table without them has none, and
    // its cells, which a large sheet has many of, are not gone through.
    if !cells.iter().any(|cell| cell.role == Role::RowHeader) {
        return Ok(vec![]);
    }
    struct Seen<'a> {
        id: &'a str,
        address: &'a str,
        role: Role,
        x: u32,
        y: u32,
        headers: Vec<&'a str>,
    }
    let mut seen = vec![];
    let mut names: HashMap<u32, Vec<(u32, &str)>> = HashMap::new();
    let mut written = BTreeSet::new();
    let mut heads: HashMap<&str, Vec<usize>> = HashMap::new();
    for cell in cells {
        let (x, y) = excel::coordinate(&cell.address)?;
        let id = &*cell.id;
        let headers: Vec<&str> = cell.headers.iter().map(|h| &**h).collect();
        if cell.role == Role::RowHeader {
            names.entry(y).or_default().push((x, id));
        }
        if !blank(value_at(&cell.address)) {
            written.insert((x, y));
        }
        for header in &headers {
            heads.entry(header).or_default().push(seen.len());
        }
        seen.push(Seen {
            id,
            address: &cell.address,
            role: cell.role,
            x,
            y,
            headers,
        });
    }
    let headed = |c: &Seen<'_>| heads.get(c.id).into_iter().flatten().map(|&i| &seen[i]);
    // The last row a row header reaches when nothing is written under it.
    let reach = |c: &Seen<'_>| -> Option<u32> {
        let end = headed(c).map(|h| h.y).fold(c.y, u32::max);
        ((c.y + 1)..=end)
            .all(|y| !written.contains(&(c.x, y)))
            .then_some(end)
    };
    // Whether the row of a cell holds a name to its right, not among `headers`.
    let name_beyond = |c: &Seen<'_>, headers: &[&str]| {
        names
            .get(&c.y)
            .is_some_and(|row| row.iter().any(|&(x, id)| x > c.x && !headers.contains(&id)))
    };
    let row_headers: Vec<&Seen<'_>> = seen
        .iter()
        .filter(|c| {
            names
                .get(&c.y)
                .is_some_and(|row| row.iter().any(|&(_, id)| id == c.id))
        })
        .collect();
    let mut found: Vec<(&Seen<'_>, u32)> = vec![];
    for &c in &row_headers {
        if headed(c).any(|h| h.y > c.y && name_beyond(c, &h.headers))
            && let Some(end) = reach(c)
        {
            found.push((c, end));
        }
    }
    let columns: BTreeSet<u32> = found.iter().map(|(c, _)| c.x).collect();
    let first: BTreeSet<&str> = found.iter().map(|(f, _)| f.id).collect();
    for &c in &row_headers {
        if columns.contains(&c.x)
            && !first.contains(c.id)
            && name_beyond(c, &[])
            && let Some(end) = reach(c)
        {
            found.push((c, end));
        }
    }
    // Values merged down beside the names of the records they cover, within
    // the categories on their left. A value of a record written on several
    // lines, whose name heads the lines after its first, is that record's.
    // Only under a heading of its column, as a title merged down beside the
    // fields of a form is no value of records.
    let records: BTreeSet<u32> = seen.iter().map(|c| c.y).collect();
    let kept: BTreeSet<&str> = found.iter().map(|(f, _)| f.id).collect();
    let headings: BTreeSet<&str> = seen
        .iter()
        .filter(|c| c.role == Role::ColumnHeader)
        .map(|c| c.id)
        .collect();
    let mut merged = vec![];
    for c in &seen {
        let Some(end) = merged_to(c.address) else {
            continue;
        };
        let names_before: Vec<&str> = names
            .get(&c.y)
            .into_iter()
            .flatten()
            .filter(|&&(x, id)| x < c.x && !kept.contains(id))
            .map(|&(_, id)| id)
            .collect();
        let named = !names_before.is_empty()
            && !names_before.iter().any(|id| {
                heads
                    .get(id)
                    .into_iter()
                    .flatten()
                    .any(|&i| seen[i].y > c.y && seen[i].y <= end)
            });
        let within = found
            .iter()
            .filter(|(f, f_end)| f.x < c.x && f.y <= c.y && c.y <= *f_end)
            .all(|&(_, f_end)| end <= f_end);
        if c.role == Role::Data
            && c.headers.iter().any(|h| headings.contains(h))
            && written.contains(&(c.x, c.y))
            && records.range(c.y..=end).nth(1).is_some()
            && named
            && within
        {
            merged.push((c, end));
        }
    }
    found.extend(merged);
    let mut spans: BTreeMap<(u32, u32), Vec<(u32, &str)>> = BTreeMap::new();
    for (c, end) in found {
        spans.entry((c.y, end)).or_default().push((c.x, c.id));
    }
    let mut found: Vec<Category<'_>> = spans
        .into_iter()
        .map(|((row, end), mut cells)| {
            cells.sort_unstable();
            Category { cells, row, end }
        })
        .collect();
    found.sort_by_key(|c| (c.cells[0].0, c.row));
    // A category crossing the bounds of one kept before it is none. One kept
    // overlaps it without holding it when it starts in (row, end] or ends in
    // [row, end): looked up by the rows kept categories start and end on,
    // rather than against every category kept.
    let mut kept: Vec<Category<'_>> = vec![];
    let (mut starts, mut ends) = (BTreeSet::new(), BTreeSet::new());
    for c in found {
        let crossing = starts
            .range((
                std::ops::Bound::Excluded(c.row),
                std::ops::Bound::Included(c.end),
            ))
            .next()
            .is_some()
            || ends.range(c.row..c.end).next().is_some();
        if !crossing {
            starts.insert(c.row);
            ends.insert(c.end);
            kept.push(c);
        }
    }
    Ok(kept)
}

pub(super) fn geometry(
    layout: &mut Layout,
    extraction: &Extraction,
    operations: &[excel::StructuralOperation],
) -> Result<()> {
    for (i, source) in extraction.sheets.iter().enumerate() {
        layout
            .sheets
            .entry(identity::page_id(&source.fields, i))
            .or_insert_with(|| LayoutSheet::new(json!({})))["merges"] =
            json!(excel::merges_after(&source.fields, operations)?);
    }
    super::visual_elements::refresh(layout, extraction, operations)?;
    Ok(())
}

/// The first column position of the logical column of each column position
/// of a table's cells, given as (record, (row, column)) positions. A cell
/// merged across columns (`merged`: its bottom and right by its row and
/// column) makes the columns it spans one logical column when no record holds
/// cells in two of them, as values written under a merged heading in any of
/// its columns. Records holding cells in two of them keep them apart, as the
/// columns under a heading over several headings.
fn logical_columns(
    cells: &[(usize, (u64, u64))],
    merged: &BTreeMap<(u64, u64), (u64, u64)>,
) -> BTreeMap<u64, u64> {
    let mut logical: BTreeMap<u64, u64> = cells.iter().map(|(_, (_, x))| (*x, *x)).collect();
    let mut records: BTreeMap<usize, Vec<u64>> = BTreeMap::new();
    for (record, (_, x)) in cells {
        records.entry(*record).or_default().push(*x);
    }
    let mut kept_apart = BTreeSet::new();
    for (_, (y, x)) in cells {
        let Some((_, right)) = merged.get(&(*y, *x)) else {
            continue;
        };
        let joined: BTreeSet<u64> = logical.range(x..=right).map(|(_, l)| *l).collect();
        if joined.len() < 2 || kept_apart.contains(&joined) {
            continue;
        }
        if records.values().any(|columns| {
            columns
                .iter()
                .filter(|x| joined.contains(&logical[x]))
                .count()
                >= 2
        }) {
            kept_apart.insert(joined);
            continue;
        }
        let first = *joined.first().unwrap();
        for l in logical.values_mut().filter(|l| joined.contains(l)) {
            *l = first;
        }
    }
    logical
}

/// Puts in place in the element `metadata` of a layout `sheet` what follows
/// from the rest, as [`encode`] gives it: the IDs of tables and their records,
/// by their first cell and its row, and of the blanks between elements, by the
/// elements around them; a table's logical columns, in the order of their
/// positions, each named by its first column (see [`logical_columns`]); each
/// table cell's logical column; and the logical rows and columns of its table
/// that a merged cell spans, a span of one being the cell itself. The
/// positions are those of the sheet's rows and columns, the merges its
/// `merges`.
pub(super) fn complete(sheet: &LayoutSheet, metadata: &mut [Meta]) -> Result<()> {
    let position =
        |axis: &str, id: &str, missing: u64| sheet.axis(axis).position(id).unwrap_or(missing);
    let mut merged = BTreeMap::new();
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
    let elements = metadata;
    for element in elements.iter_mut().filter(|e| e["type"] == "table") {
        let Meta::Table {
            fields: element,
            rows,
        } = element
        else {
            bail!("table rows missing");
        };
        // Its title and notes take no part in its records' rows and columns.
        let mut texts = split_descriptions(rows);
        // The place of each cell, in the order of the records: a large table
        // has a cell for each of its values, and the list is made as large
        // as that at once.
        let mut places = Vec::with_capacity(rows.iter().map(|row| row.cells.len()).sum());
        let mut names = BTreeMap::new();
        for (record, row) in rows.iter_mut().enumerate() {
            let first = first_id(row)?;
            let (id, _) =
                cell_axes(first).with_context(|| format!("table cell {first} names no cell"))?;
            row["id"] = json!(token("record", &[&id]));
            for cell in &mut row.cells {
                let id = cell.id()?;
                let (row, column) =
                    cell_axes(id).with_context(|| format!("table cell {id} names no cell"))?;
                let at = (
                    position("rows", &row, u64::MAX),
                    position("columns", &column, 0),
                );
                names
                    .entry(at.1)
                    .or_insert_with(|| Name::from(token("column", &[&column])));
                places.push((record, at));
            }
        }
        let logical = logical_columns(&places, &merged);
        let logical_rows: BTreeSet<u64> = places.iter().map(|(_, (y, _))| *y).collect();
        let logical_columns: BTreeMap<u64, Value> = logical
            .values()
            .map(|first| (*first, json!({"id":&*names[first]})))
            .collect();
        let cells = rows.iter_mut().flat_map(|row| row.cells.iter_mut());
        for (cell, &(_, (y, x))) in cells.zip(&places) {
            cell.set_column(names[&logical[&x]].clone());
            cell.remove("row_span");
            cell.remove("column_span");
            if let Some((bottom, right)) = merged.get(&(y, x)) {
                let spanned: BTreeSet<_> = logical.range(x..=*right).map(|(_, l)| l).collect();
                for (key, count) in [
                    ("row_span", logical_rows.range(y..=*bottom).count()),
                    ("column_span", spanned.len()),
                ] {
                    if count > 1 {
                        cell.insert(key, json!(count));
                    }
                }
            }
        }
        element["columns"] = json!(logical_columns.into_values().collect::<Vec<_>>());
        let first = match rows.first() {
            Some(row) => first_id(row)?,
            None => string(&Value::Null)?,
        }
        .to_owned();
        element["id"] = json!(token("table", &[&first]));
        for cell in texts.iter_mut().flat_map(|row| row.cells.iter_mut()) {
            let id = cell.id()?;
            let (_, column) =
                cell_axes(id).with_context(|| format!("table cell {id} names no cell"))?;
            cell.set_column(Name::from(token("column", &[&column])));
            cell.remove("row_span");
            cell.remove("column_span");
        }
        rows.extend(texts);
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

/// The version of content pages: tables as the trees of their records and
/// categories, with the text written against each as its title and notes.
pub(crate) const CONTENT_VERSION: &str = "9";

/// The YAML of the content `page` of `layout` as a file holds it, each
/// diagram written together, as [`crate::document_body::yaml`] orders it.
pub(super) fn page_bytes(page: &Value, layout: &Layout) -> Result<Vec<u8>> {
    let sheet = layout.sheet(string(&page["page_id"])?)?;
    let diagrams = super::diagrams::of(sheet)?;
    let Content { fields, elements } = Content::from_value(page.clone())?;
    let elements = crate::document_body::expand(elements, &sheet.metadata, &diagrams)?;
    let mut page = Value::Object(fields);
    page["elements"] = Value::Array(elements.into_iter().map(|e| e.into_value()).collect());
    crate::document_body::yaml(&page, &sheet.metadata, &diagrams)
}

/// [`page_bytes`] of a page [`encode`] made.
pub(super) fn content_bytes(page: &Encoded, layout: &Layout) -> Result<Vec<u8>> {
    let mut bytes = vec![];
    let sheet = layout.sheet(string(&page.fields["page_id"])?)?;
    let diagrams = super::diagrams::of(sheet)?;
    crate::document_body::encoded_yaml_to(page, &sheet.metadata, &diagrams, &mut bytes)?;
    Ok(bytes)
}

/// Stores [`content_bytes`] at `path`, written as they are made.
pub(super) fn write_page(path: &Path, page: &Encoded, layout: &Layout) -> Result<()> {
    let sheet = layout.sheet(string(&page.fields["page_id"])?)?;
    let diagrams = super::diagrams::of(sheet)?;
    crate::data::replace_streamed(path, |out| {
        crate::document_body::encoded_yaml_to(page, &sheet.metadata, &diagrams, out)
    })
}

/// Encode the writer's private grid into reading-order elements, updating only
/// the separate layout record with physical targets and a checked body shape.
/// The values and fonts of `page` are moved into the elements, as a page holds
/// one of each per cell. The tables are arranged by `groups`, those classify
/// or an edit gives the page, or else those its element metadata records.
pub(super) fn encode(page: Value, layout: &mut Layout, groups: Option<Groups>) -> Result<Encoded> {
    encode_runtime(GridPage::from_value(page)?, layout, groups)
}

/// [`encode`] of a page given typed by the keys of the writer, as an edit
/// reads it ([`identity::runtime_grid`]).
pub(super) fn encode_runtime(
    page: GridPage,
    layout: &mut Layout,
    groups: Option<Groups>,
) -> Result<Encoded> {
    let grid = identity::stored_grid(page, layout)?;
    encode_grid(grid, layout, groups)
}

/// A grid page as [`encode_grid`] reads it, its values and their fonts typed:
/// a large sheet has one of each per cell, and the JSON object of each row,
/// one of its values and one of their fonts, took several times its cells,
/// the most for a row of one cell, as a Word paragraph is.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct GridPage {
    /// The page without its blocks, fonts and font table.
    pub(super) fields: Value,
    /// The block of extraction notes, null for none.
    pub(super) notes: Value,
    /// The title of each block with rows.
    pub(super) titles: serde_json::Map<String, Value>,
    /// The values of the blocks with rows, in the order of their block, row
    /// and column, as the page lists them.
    pub(super) values: Vec<GridValue>,
    /// The fonts the values refer to.
    pub(super) font_table: Value,
}

/// A value of a [`GridPage`], at `row` and `column` of `block`.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct GridValue {
    pub(super) block: Name,
    /// Shared with the values of its row.
    pub(super) row: Name,
    /// Shared with the values of its column.
    pub(super) column: Name,
    pub(super) value: Value,
    /// The index of its font in the font table, none for none.
    pub(super) font: Option<u32>,
}

impl GridPage {
    /// The grid page given as JSON, its values and fonts moved out of it.
    pub(super) fn from_value(mut page: Value) -> Result<Self> {
        let Value::Object(blocks) = page["blocks"].take() else {
            bail!("grid blocks missing")
        };
        let mut fonts = page["fonts"].take();
        let font_table = page[FONT_TABLE].take();
        let members = page.as_object_mut().context("page must be an object")?;
        members.remove("blocks");
        members.remove("fonts");
        members.remove(FONT_TABLE);
        let notes = blocks.get("extraction-notes").cloned().unwrap_or_default();
        let titles = blocks
            .iter()
            .filter(|(_, body)| body["rows"].is_object())
            .map(|(id, body)| (id.clone(), body["title"].clone()))
            .collect();
        let mut values = vec![];
        // The columns of a block, shared by the values in them.
        let mut columns = Names::default();
        for (block, mut body) in blocks {
            let Value::Object(rows) = body["rows"].take() else {
                continue;
            };
            let mut block_fonts = fonts
                .get_mut(block.as_str())
                .map(Value::take)
                .unwrap_or_default();
            let block: Name = block.into();
            for (row, cells) in rows {
                let row_fonts = block_fonts
                    .get_mut(row.as_str())
                    .map(Value::take)
                    .unwrap_or_default();
                let Value::Object(cells) = cells else {
                    bail!("grid row missing")
                };
                let row: Name = row.into();
                for (column, value) in cells {
                    let font = font_index(&row_fonts[column.as_str()])?;
                    values.push(GridValue {
                        block: block.clone(),
                        row: row.clone(),
                        column: columns.get(&column),
                        value,
                        font,
                    });
                }
            }
        }
        Ok(Self {
            fields: page,
            notes,
            titles,
            values,
            font_table,
        })
    }
}

impl GridPage {
    /// Puts the values in the order of their block, row and column, as the
    /// page lists them, the last of any at the same place kept, as the page
    /// keeps it.
    pub(super) fn order(&mut self) {
        fn place(v: &GridValue) -> (&str, &str, &str) {
            (&v.block, &v.row, &v.column)
        }
        self.values
            .sort_by(|a, b| (&*a.block, &a.row, &a.column).cmp(&(&*b.block, &b.row, &b.column)));
        // Kept in place: a large sheet has a value for each cell, and a list
        // of those kept held them twice.
        let mut kept = 0;
        for at in 0..self.values.len() {
            if kept > 0 && place(&self.values[kept - 1]) == place(&self.values[at]) {
                self.values.swap(kept - 1, at);
            } else {
                self.values.swap(kept, at);
                kept += 1;
            }
        }
        self.values.truncate(kept);
    }

    /// Where the value at `row` and `column` of `block` is among the values
    /// in order, or where it would be.
    fn place(&self, block: &str, row: &str, column: &str) -> std::result::Result<usize, usize> {
        self.values
            .binary_search_by(|v| (&*v.block, &*v.row, &*v.column).cmp(&(block, row, column)))
    }

    /// The value at `row` and `column` of `block`.
    pub(super) fn get(&self, block: &str, row: &str, column: &str) -> Option<&GridValue> {
        self.place(block, row, column)
            .ok()
            .map(|at| &self.values[at])
    }

    /// Removes the value at `row` and `column` of `block`.
    pub(super) fn remove(&mut self, block: &str, row: &str, column: &str) -> Option<GridValue> {
        self.place(block, row, column)
            .ok()
            .map(|at| self.values.remove(at))
    }

    /// Gives `row` and `column` of `block` the value `value`, keeping the
    /// font of one there.
    pub(super) fn set(&mut self, block: &str, row: String, column: String, value: Value) {
        match self.place(block, &row, &column) {
            Ok(at) => self.values[at].value = value,
            Err(at) => self.values.insert(
                at,
                GridValue {
                    block: block.into(),
                    row: row.into(),
                    column: column.into(),
                    value,
                    font: None,
                },
            ),
        }
    }

    /// The values of `block`, from the values in order ([`GridPage::order`]).
    pub(super) fn block(&self, block: &str) -> &[GridValue] {
        let start = self.values.partition_point(|v| &*v.block < block);
        let end = self.values.partition_point(|v| &*v.block <= block);
        &self.values[start..end]
    }

    /// The grid page as JSON, as an edit reads it: its notes, each block of
    /// `titles` with its rows, and the fonts the values have.
    pub(super) fn to_value(&self) -> Value {
        let mut page = self.fields.clone();
        let mut blocks = json!({"extraction-notes":self.notes});
        for (block, title) in &self.titles {
            blocks[block] = json!({"title":title,"rows":{}});
        }
        let mut fonts = json!({});
        for value in &self.values {
            blocks[&*value.block]["rows"][&*value.row][&*value.column] = value.value.clone();
            if let Some(font) = value.font {
                fonts[&*value.block][&*value.row][&*value.column] = json!(font);
            }
        }
        page["blocks"] = blocks;
        if fonts.as_object().is_some_and(|fonts| !fonts.is_empty()) {
            page["fonts"] = fonts;
        }
        page[FONT_TABLE] = self.font_table.clone();
        page
    }
}

/// A text a table holds as its title or a note, as [`encode_grid`] finds
/// it: what it is to the table, its row and column, its last row and the
/// text element it would be on its own.
type Described = (Description, (u64, u64), u64, MetaText);

/// The record of a table holding the text element `text` as its title or a
/// note (`description`): one cell, with the text's ID, value and font, and
/// an ID of the record that no record of the table's own has.
fn description_record(description: Description, text: MetaText) -> Result<MetaRow> {
    let mut text = Meta::Text(text);
    let id = string(&text["id"])?.to_owned();
    let mut cell = MetaCell::with_id(id.clone());
    if let Some(font) = text.remove("font") {
        cell.insert("font", font);
    }
    match (text.remove("formula"), text.remove("text")) {
        (Some(formula), _) => cell.insert("formula", formula),
        (None, Some(value)) => cell.insert("value", value),
        (None, None) => bail!("text of {id} missing"),
    };
    let mut row = MetaRow::new(vec![cell]);
    row["id"] = json!(token(description.name(), &[&id]));
    row["description"] = json!(description.name());
    Ok(row)
}

/// [`encode`] of a grid page given typed, as import makes it of an extraction.
pub(super) fn encode_grid(
    grid: GridPage,
    layout: &mut Layout,
    groups: Option<Groups>,
) -> Result<Encoded> {
    let page_id = string(&grid.fields["page_id"])?.to_owned();
    let sheet = layout.sheet_mut(&page_id)?;
    // Not stored: the interpretation the metadata records, unless classify
    // or an edit gives another.
    let mut groups = match groups {
        Some(groups) => groups,
        None => metadata_groups(&sheet.metadata)?,
    };
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
    let mut bounds = Bounds::new();
    // The cells of the body's values and formulas, by row and column identity.
    let mut places: Vec<(&str, &str)> = vec![];
    for value in &grid.values {
        match &*value.block {
            "table-1" => places.push((&value.row, &value.column)),
            "formulas" => {
                let target = &sheet["formulas"][&*value.row];
                if let (Some(row), Some(column)) =
                    (target["row"].as_str(), target["column"].as_str())
                {
                    places.push((row, column));
                }
            }
            _ => {}
        }
    }
    let body = || places.iter().copied();
    for (row, column) in body() {
        let y = sheet.rows.position(row).context("row position missing")?;
        let x = sheet
            .columns
            .position(column)
            .context("column position missing")?;
        // A title or a note of a table is no part of its records' range.
        if let Some(table) = groups
            .get_at(row, column)
            .filter(|group| group.description.is_none())
            .and_then(|group| group.table.as_deref())
        {
            let b = bounds.entry(table.to_owned()).or_insert((y, x, y, x));
            b.0 = b.0.min(y);
            b.1 = b.1.min(x);
            b.2 = b.2.max(y);
            b.3 = b.3.max(x);
        }
    }
    let inserted = |axis: &Lines, id: &str| axis.key(id).is_some_and(|k| k.contains('-'));
    let mut ids = vec![];
    let mut points = vec![];
    for (row, column) in body() {
        if !inserted(&sheet.rows, row) && !inserted(&sheet.columns, column) {
            continue;
        }
        let id = cell_id(row, column);
        if groups.contains_key(&id) {
            continue;
        }
        points.push((
            sheet.rows.position(row).unwrap(),
            sheet.columns.position(column).unwrap(),
        ));
        ids.push(id);
    }
    // Rows inserted right after a table's last row, or columns right after its
    // last column, within its columns or rows, continue it: records added at
    // its end, as the table reads again once Excel has them.
    for b in bounds.values_mut() {
        loop {
            let below = points
                .iter()
                .any(|&(y, x)| y == b.2 + 1 && b.1 <= x && x <= b.3);
            let right = points
                .iter()
                .any(|&(y, x)| x == b.3 + 1 && b.0 <= y && y <= b.2);
            if below {
                b.2 += 1;
            } else if right {
                b.3 += 1;
            } else {
                break;
            }
        }
    }
    // The innermost table around the insertion takes it.
    for (id, table) in ids.iter().zip(innermost(&bounds, &points)) {
        if let Some(table) = table {
            groups.insert(
                id,
                Group {
                    table: Some(table.into()),
                    ..Group::default()
                },
            );
        }
    }
    let derived = Bindings::of(&sheet.fields)?;
    // The visual of each shape text by its text key, the first in order as
    // a search through them finds it, looked up once per shape rather than by
    // going through every visual.
    let mut visual_of: HashMap<String, (String, Value)> = HashMap::new();
    for (id, v) in sheet["visuals"].as_object().into_iter().flatten() {
        if let Some(key) = v["text_key"].as_str() {
            visual_of
                .entry(key.to_owned())
                .or_insert_with(|| (id.clone(), v.clone()));
        }
    }
    let GridPage {
        fields: mut out,
        notes,
        titles: block_titles,
        mut values,
        font_table,
    } = grid;
    let mut fonts = Fonts::default();
    let mut placed: Vec<((u64, u64), u64, Meta)> = vec![];
    // The cells of each table by the position of their row, each with the
    // position of its column: kept by row, as one list of the cells of a
    // large table, and the rows made of it, held each cell twice at once.
    // Each row is made as large as the row made before it, as the values
    // come row by row: grown by doubling, the rows of a large table held
    // half as much again as their cells.
    type TableRows = BTreeMap<u64, Vec<(u64, MetaCell)>>;
    let mut tables: BTreeMap<String, (TableRows, usize)> = BTreeMap::new();
    // The titles and notes of each table, with their places and last rows,
    // held as text elements until their table is made.
    let mut described: BTreeMap<String, Vec<Described>> = BTreeMap::new();
    // The values are taken in order from the end of the list reversed, which
    // is let go as it empties: a large sheet's took as much room as the cells
    // made of them.
    values.reverse();
    while let Some(GridValue {
        block,
        row,
        column,
        value,
        font,
    }) = values.pop()
    {
        if values.len() < values.capacity() / 2 {
            values = crate::data::exact(values);
        }
        let block = &*block;
        let (axis_row, axis_column) = if block == "formulas" {
            let target = &sheet["formulas"][&*row];
            (string(&target["row"])?, string(&target["column"])?)
        } else {
            (&*row, &*column)
        };
        let id = if block == "shapes" {
            token("shape", &[&row, &column])
        } else {
            cell_id(axis_row, axis_column)
        };
        // Not stored: decode reads the binding back from the ID.
        let (bound_block, bound_row, bound_column) = derived.get(&id)?;
        ensure!(
            bound_block == block && *bound_row == *row && *bound_column == *column,
            "element ID {id} does not name its place in block {block}"
        );
        let mut y = sheet.rows.position(axis_row).unwrap_or(u64::MAX);
        let mut x = sheet.columns.position(axis_column).unwrap_or(0);
        let visual = if block == "shapes" {
            ensure!(sheet["visuals"].is_object(), "visual bindings missing");
            visual_of.get(&*row).cloned()
        } else {
            None
        };
        if let Some((_, v)) = &visual {
            let position = super::visual_elements::position(v);
            y = position.0;
            x = position.1;
        }
        let group = groups.get(&id);
        let table = group.and_then(|group| group.table.as_deref());
        let description = group.and_then(|group| group.description);
        let font = indexed_font(&font_table, font)?;
        let field = if block == "formulas" {
            "formula"
        } else {
            "value"
        };
        if let Some(table) = table
            && description.is_none()
        {
            let mut cell = MetaCell::with_id(id);
            if !font.is_null() {
                cell.insert("font", fonts.add(font));
            }
            cell.insert(field, value);
            let (rows, width) = tables.entry(table.to_owned()).or_default();
            let cells = rows.entry(y).or_insert_with(|| Vec::with_capacity(*width));
            cells.push((x, cell));
            *width = cells.len();
        } else {
            let kind = if block == "formulas" {
                "formula"
            } else {
                "text"
            };
            let mut element = MetaText::default();
            element.insert("id", Value::String(id));
            element.insert("type", json!(kind));
            element.insert(kind, value);
            if let Some((reference, _)) = visual {
                element.insert("visual", json!(reference));
            }
            if !font.is_null() {
                element.insert("font", fonts.add(font));
            }
            let end = spans.get(&(y, x)).map_or(y, |s| s.0);
            match (table, description) {
                (Some(table), Some(description)) => described
                    .entry(table.to_owned())
                    .or_default()
                    .push((description, (y, x), end, element)),
                _ => placed.push(((y, x), end, Meta::Text(element))),
            }
        }
    }
    for (key, (rows, _)) in tables {
        let mut first = None;
        let mut end = 0;
        let mut records = Vec::with_capacity(rows.len());
        for (y, mut cells) in rows {
            cells.sort_by_key(|(x, _)| *x);
            for (x, _) in &cells {
                end = end.max(spans.get(&(y, *x)).map_or(y, |s| s.0));
            }
            if first.is_none() {
                // Named as [`complete`] names it, by its first cell: the
                // blank after the table is named by it.
                first = Some(((y, cells[0].0), token("table", &[cells[0].1.id()?])));
            }
            records.push(MetaRow::new(
                cells.into_iter().map(|(_, cell)| cell).collect(),
            ));
        }
        let ((mut top, left), id) = first.context("table without cells")?;
        // Its title and notes follow its records, the title first: the table
        // starts on its title's row, and ends on the last row of any.
        let mut texts = described.remove(&key).unwrap_or_default();
        texts.sort_by_key(|(description, at, ..)| (*description, *at));
        for (description, (y, _), last, text) in texts {
            top = top.min(y);
            end = end.max(last);
            records.push(description_record(description, text)?);
        }
        let table = Meta::table(json!({"id":id,"type":"table"}), records);
        placed.push(((top, left), end, table));
    }
    // A title or a note of a table that holds no cell any more is a text.
    for (_, texts) in described {
        for (_, at, end, text) in texts {
            placed.push((at, end, Meta::Text(text)));
        }
    }
    // Each place is bound once: checked by the IDs of the elements and cells
    // made, in order, rather than by a set of copies of them made as each is
    // bound, as a large sheet has a cell for each of its values.
    let mut bound: Vec<&str> = vec![];
    for (_, _, element) in &placed {
        match element.records() {
            Some(rows) => {
                for cell in rows.iter().flat_map(|row| &row.cells) {
                    bound.push(cell.id()?);
                }
            }
            None => bound.push(string(&element["id"])?),
        }
    }
    bound.sort_unstable();
    ensure!(
        bound.windows(2).all(|pair| pair[0] != pair[1]),
        "duplicate element binding"
    );
    drop(bound);
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
            Meta::Other(json!({"id":reference,"type":visual["kind"],"ref":reference})),
        ));
    }
    // Of the elements at one cell, the drawings anchored in it come first,
    // from the top of the cell down, then from its left: several check
    // boxes stacked in one cell are read as they are shown, not as the
    // original happens to list them. Each drawing is followed by its text.
    fn within<'v>(visuals: &'v Value, element: &'v Meta) -> Option<(u64, u64, &'v str)> {
        let reference = element
            .get("ref")
            .or_else(|| element.get("visual"))?
            .as_str()?;
        let from = &visuals[reference]["anchor"]["from"];
        Some((
            from["rowOff"].as_u64().unwrap_or(0),
            from["colOff"].as_u64().unwrap_or(0),
            reference,
        ))
    }
    let visuals = &sheet["visuals"];
    placed.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| match (within(visuals, &a.2), within(visuals, &b.2)) {
                (Some(a), Some(b)) => a.cmp(&b),
                (a, b) => b.is_some().cmp(&a.is_some()),
            })
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
            elements.push(Meta::Other(json!({"type":"blank"})));
        }
        previous = Some(previous.map_or(end, |last| last.max(end)));
        elements.push(element);
    }
    out["schema_version"] = json!(CONTENT_VERSION);
    complete(sheet, &mut elements)?;
    arrange(sheet, &groups, &mut elements)?;
    let (body, metadata) = crate::document_body::split_encoded(elements)?;
    sheet.metadata = metadata;
    sheet["fonts"] = Value::Array(fonts.list);
    sheet["notes"] = notes;
    sheet["block_titles"] = Value::Object(block_titles);
    let Value::Object(fields) = out else {
        bail!("page must be an object")
    };
    let mut page = Encoded::new(fields, body);
    page.check(|part| validate("content", part))?;
    Ok(page)
}

/// Bind values to recorded slots. Shape changes are refused; same-shape manual
/// permutations are value edits and cannot move the recorded identities.
pub(super) fn decode(page: &Value, layout: &Layout) -> Result<Value> {
    Ok(decode_grid(Content::from_value(page.clone())?, layout)?.to_value())
}

/// [`decode`] of a page held typed, as an inspection reads the pages of a
/// document. The page is let go as it is read: each value is moved from it
/// into the grid, and each record of a table let go once read, as a table of
/// a large sheet is as large as its page.
pub(super) fn decode_grid(page: Content, layout: &Layout) -> Result<GridPage> {
    ensure!(
        page.fields["schema_version"] == CONTENT_VERSION,
        "Document content must use the tree of table records; re-import the original"
    );
    let typed = layout.sheet(string(&page.fields["page_id"])?)?;
    // The body is read beside its metadata rather than joined into a copy,
    // its shape checked as joining it would.
    let Content { fields, elements } = page;
    let elements =
        crate::document_body::expand(elements, &typed.metadata, &super::diagrams::of(typed)?)?;
    let mut decoder = Decoder::new(typed)?;
    crate::document_body::into_each_element(elements, &typed.metadata, |element, value| {
        decoder.bind(element, value)
    })?;
    decoder.grid(fields, false)
}

/// [`decode_grid`] of the content page `text`, read straight into the grid:
/// no value is made of a table but those of its cells, as a table of a large
/// sheet is several times its values. None for a page this reads
/// differently, such as one ARP did not write, or one that is invalid: the
/// page is then read to say why.
pub(super) fn read_grid(text: &str, layout: &Layout) -> Option<GridPage> {
    // The page's ID follows its elements, and is read first from its line.
    let page_id = text
        .lines()
        .rev()
        .find_map(|line| line.strip_prefix("page_id: "))?
        .trim_matches('\'');
    let typed = layout.sheet(page_id).ok()?;
    let diagrams = super::diagrams::of(typed).ok()?;
    let mut decoder = Decoder::new(typed).ok()?;
    let fields = crate::document_body::read_each_element(
        text,
        &typed.metadata,
        &diagrams,
        |element, value| decoder.bind(element, value),
    )
    .ok()?;
    (fields.get("page_id")?.as_str()? == page_id).then_some(())?;
    ensure_version(&fields).ok()?;
    // Bound in another order than the cells': of values at one place, the
    // last in the cells' order is kept, which only that order tells, so none
    // may be.
    decoder.grid(fields, true).ok()
}

fn ensure_version(fields: &serde_json::Map<String, Value>) -> Result<()> {
    ensure!(
        fields.get("schema_version") == Some(&json!(CONTENT_VERSION)),
        "Document content must use the tree of table records; re-import the original"
    );
    Ok(())
}

/// The grid of a content page as its values are bound to the cells of its
/// sheet of the layout.
struct Decoder<'a> {
    typed: &'a LayoutSheet,
    titles: serde_json::Map<String, Value>,
    bindings: Bindings,
    /// The IDs bound, each once: checked in order once all are bound.
    used: Vec<&'a str>,
    blocks: HashMap<String, Name>,
    values: Vec<GridValue>,
    /// The rows and columns, each shared by the values in it.
    lines: Names,
}

impl<'a> Decoder<'a> {
    fn new(typed: &'a LayoutSheet) -> Result<Self> {
        let sheet = &typed.fields;
        // A value for each text or formula element and each table cell, made
        // as large at once: grown by doubling, the list of a large sheet held
        // up to twice its values, and as long as the grid.
        let count = typed
            .metadata
            .iter()
            .map(|e| match e.records() {
                Some(rows) => rows.iter().map(|row| row.cells.len()).sum(),
                None => usize::from(matches!(e["type"].as_str(), Some("text" | "formula"))),
            })
            .sum();
        Ok(Self {
            typed,
            titles: sheet["block_titles"]
                .as_object()
                .context("block titles missing")?
                .clone(),
            bindings: Bindings::of(sheet)?,
            used: Vec::with_capacity(count),
            blocks: HashMap::new(),
            values: Vec::with_capacity(count),
            lines: Names::default(),
        })
    }

    /// Binds `value` to the element or table cell `element`.
    fn bind(&mut self, element: crate::document_body::Item<'a>, value: Value) -> Result<()> {
        let sheet = &self.typed.fields;
        let id = element.id()?;
        self.used.push(id);
        let (block, row, column) = self.bindings.get(id)?;
        // The fields refer to the fonts of the layout by their indexes.
        let font = element.font();
        let font = if table_font(&sheet["fonts"], &font)?.is_null() {
            None
        } else {
            font_index(&font)?
        };
        if !self.titles.contains_key(&block) {
            let title = match block.as_str() {
                "table-1" => "本文",
                "formulas" => "数式原文",
                _ => "図形の文字",
            };
            self.titles.insert(block.clone(), json!(title));
        }
        let block = self
            .blocks
            .entry(block)
            .or_insert_with_key(|block| block.as_str().into())
            .clone();
        self.values.push(GridValue {
            block,
            row: self.lines.get(&row),
            column: self.lines.get(&column),
            value,
            font,
        });
        Ok(())
    }

    /// The grid of the page whose members besides its elements are `fields`.
    /// The grid of the page whose members besides its elements are
    /// `fields`. With `unique`, no two values may be at one place, as when
    /// they were bound in another order than the cells', and they are put in
    /// order in their own room.
    fn grid(mut self, fields: serde_json::Map<String, Value>, unique: bool) -> Result<GridPage> {
        let sheet = &self.typed.fields;
        self.used.sort_unstable();
        ensure!(
            self.used.windows(2).all(|pair| pair[0] != pair[1]),
            "duplicate element cell ID"
        );
        super::visual_elements::validate_references(&self.typed.metadata, sheet)?;
        let mut fields = Value::Object(fields);
        fields["schema_version"] = json!(identity::GRID_VERSION);
        let mut grid = GridPage {
            fields,
            notes: sheet["notes"].clone(),
            titles: self.titles,
            values: self.values,
            font_table: match &sheet["fonts"] {
                Value::Null => json!([]),
                fonts => fonts.clone(),
            },
        };
        if unique {
            fn place(v: &GridValue) -> (&str, &str, &str) {
                (&v.block, &v.row, &v.column)
            }
            grid.values.sort_unstable_by(|a, b| place(a).cmp(&place(b)));
            ensure!(
                grid.values
                    .windows(2)
                    .all(|pair| place(&pair[0]) != place(&pair[1])),
                "values at one place"
            );
        } else {
            grid.order();
        }
        Ok(grid)
    }
}

#[cfg(test)]
mod tests {
    use super::{GridPage, GridValue, Group, Groups, categories, parts, token};
    use crate::document_structure::{Element, ElementCell};
    use serde_json::{Value, json};

    /// A page has a value for each cell of its sheet: one holds its place,
    /// value and font index in no more than 64 bytes.
    #[test]
    fn a_grid_value_is_small() {
        assert!(std::mem::size_of::<GridValue>() <= 64);
    }

    /// Values are put in the order of their block, row and column, and of
    /// values at one place the last is kept.
    #[test]
    fn grid_values_in_order_keep_the_last_at_a_place() {
        let value = |block: &str, row: &str, column: &str, value: i32| GridValue {
            block: block.into(),
            row: row.into(),
            column: column.into(),
            value: json!(value),
            font: None,
        };
        let mut grid = GridPage {
            fields: json!({}),
            notes: Value::Null,
            titles: serde_json::Map::new(),
            values: vec![
                value("table-1", "r2", "A", 1),
                value("formulas", "r1", "A", 2),
                value("table-1", "r1", "B", 3),
                value("table-1", "r2", "A", 4),
                value("table-1", "r1", "A", 5),
                value("table-1", "r2", "A", 6),
            ],
            font_table: json!([]),
        };
        grid.order();
        let placed: Vec<_> = grid
            .values
            .iter()
            .map(|v| (&*v.block, &*v.row, &*v.column, v.value.as_i64().unwrap()))
            .collect();
        assert_eq!(
            placed,
            [
                ("formulas", "r1", "A", 2),
                ("table-1", "r1", "A", 5),
                ("table-1", "r1", "B", 3),
                ("table-1", "r2", "A", 6),
            ]
        );
    }

    /// The layout sheet given as JSON without its element metadata.
    fn layout_sheet(sheet: &Value) -> crate::documents::LayoutSheet {
        let mut sheet = sheet.clone();
        sheet["element_metadata"] = json!([]);
        for axis in ["rows", "columns"] {
            if sheet.get(axis).is_none() {
                sheet[axis] = json!({});
            }
        }
        crate::documents::LayoutSheet::from_value(sheet).unwrap()
    }

    /// [`super::complete`] of element metadata given as JSON.
    fn complete(sheet: &Value, metadata: &mut Value) -> anyhow::Result<()> {
        let mut typed = crate::documents::metadata_from(metadata.take())?;
        super::complete(&layout_sheet(sheet), &mut typed)?;
        *metadata = crate::documents::metadata_value(&typed);
        Ok(())
    }

    /// [`super::arrange`] of element metadata given as JSON.
    fn arrange(sheet: &Value, groups: &Groups, metadata: &mut Value) -> anyhow::Result<()> {
        let mut typed = crate::documents::metadata_from(metadata.take())?;
        super::arrange(&layout_sheet(sheet), groups, &mut typed)?;
        *metadata = crate::documents::metadata_value(&typed);
        Ok(())
    }

    /// [`super::metadata_groups`] of element metadata given as JSON.
    fn metadata_groups(metadata: &Value) -> anyhow::Result<Groups> {
        super::metadata_groups(&crate::documents::metadata_from(metadata.clone())?)
    }

    /// The groups a test gives as JSON, as [`Group`]s.
    fn typed(groups: &Value) -> Groups {
        groups
            .as_object()
            .unwrap()
            .iter()
            .map(|(id, g)| {
                let group = Group {
                    table: g["table"].as_str().map(Into::into),
                    description: g["description"]
                        .as_str()
                        .and_then(super::Description::named),
                    heading: g["heading"] == true,
                    mark: g["mark"] == true,
                    label: g["label"].as_str().map(Into::into),
                    parent: g.get("parent").map(|p| p.as_str().map(Into::into)),
                    category: g["category"].as_str().map(Into::into),
                    within: g["within"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|k| k.as_str().map(Into::into))
                        .collect(),
                };
                (id.clone(), group)
            })
            .collect()
    }
    use std::collections::{BTreeMap, BTreeSet};

    #[test]
    fn ids_are_read_only_as_token_writes_them() {
        for given in [
            &["r1", "B"][..],
            &["ins-1", "AA"],
            &["a_b", "%", "日本"],
            &["", "x/y.z#1"],
        ] {
            let id = token("cell", given);
            let (kind, read) = parts(&id).unwrap();
            assert_eq!(
                (kind, read.iter().map(String::as_str).collect::<Vec<_>>()),
                ("cell", given.to_vec())
            );
        }
        assert_eq!(token("cell", &["a_b", "%"]), "cell_a%5Fb_%25");
        // Another spelling of the same parts is no ID: a lower-case escape, an
        // escaped byte kept as it is, a byte kept escaped, a short escape.
        for other in [
            "cell_a%5fb",
            "cell_a_b%",
            "cell_%41",
            "cell_a b",
            "cell_%+1",
            "cell_日本",
        ] {
            assert!(parts(other).is_none(), "{other}");
        }
        assert_eq!(parts("cell_").unwrap().1, vec![String::new()]);
        assert!(parts("cell").is_none());
    }

    #[test]
    fn columns_under_a_merged_heading_are_one_unless_a_record_uses_two() {
        // A1:C1 and D1:F1 head values written in A and F; G1:H1 heads G2 and H2.
        let letters = ["A", "B", "C", "D", "E", "F", "G", "H"];
        let sheet = json!({
            "rows":(1..=3).map(|y| (format!("r{y}"), json!({"key":format!("r{y}"),"position":y}))).collect::<serde_json::Map<_, _>>(),
            "columns":letters.iter().zip(1..).map(|(x, at)| (x.to_string(), json!({"key":x,"position":at}))).collect::<serde_json::Map<_, _>>(),
            "merges":["A1:C1", "D1:F2", "G1:H1"],
        });
        let record = |y: u32, columns: &[&str]| json!({"cells":columns.iter().map(|x| json!({"id":format!("cell_r{y}_{x}")})).collect::<Vec<_>>()});
        let mut metadata = json!([{"type":"table","rows":[
            record(1, &["A", "D", "G"]),
            record(2, &["G", "H"]),
            record(3, &["A", "F", "G", "H"]),
        ]}]);
        complete(&sheet, &mut metadata).unwrap();
        let table = &metadata[0];
        assert_eq!(
            table["columns"],
            json!([{"id":"column_A"}, {"id":"column_D"}, {"id":"column_G"}, {"id":"column_H"}])
        );
        let cell = |y: usize, i: usize| &table["rows"][y]["cells"][i];
        assert_eq!(cell(2, 1)["id"], "cell_r3_F");
        assert_eq!(cell(2, 1)["column"], "column_D");
        assert_eq!(cell(2, 3)["column"], "column_H");
        // A heading spans the logical columns under it.
        assert!(cell(0, 0).get("column_span").is_none());
        assert_eq!(cell(0, 1)["row_span"], 2);
        assert!(cell(0, 1).get("column_span").is_none());
        assert_eq!(cell(0, 2)["column_span"], 2);
        let (body, _) = crate::document_body::split_json(
            metadata
                .as_array()
                .unwrap()
                .iter()
                .map(|e| {
                    let mut e = e.clone();
                    for row in e["rows"].as_array_mut().unwrap() {
                        for cell in row["cells"].as_array_mut().unwrap() {
                            cell["value"] = cell["id"].clone();
                        }
                    }
                    e
                })
                .collect::<Value>(),
        )
        .unwrap();
        // Without headings the columns name the keys: F under D1:F2 is D.
        assert_eq!(
            body[0]["table"]["rows"][2],
            json!({"A":"cell_r3_A","D":"cell_r3_F","G":"cell_r3_G","H":"cell_r3_H"})
        );
    }

    #[test]
    fn a_record_naming_no_earlier_record_does_not_cut_the_records_after_it_off() {
        // A heading, then / with xxx under it; a record naming none, as the
        // second line of xxx; yyy and zzz under xxx; and a second tree.
        let records: [(u32, &str, Option<&str>); 8] = [
            (1, "A", None),
            (2, "A", None),
            (3, "B", Some("r2")),
            (4, "C", None),
            (5, "C", Some("r3")),
            (6, "D", Some("r5")),
            (7, "A", None),
            (8, "B", Some("r7")),
        ];
        let mut groups = serde_json::Map::new();
        let mut rows = vec![];
        for (y, x, parent) in records {
            let id = format!("cell_r{y}_{x}");
            let group = if y == 1 {
                json!({"heading":true})
            } else {
                json!({"parent":parent})
            };
            groups.insert(id.clone(), group);
            rows.push(json!({"id":format!("record_r{y}"),"cells":[{"id":id,"column":format!("column_{x}")}]}));
        }
        let sheet = json!({});
        let groups = typed(&json!(groups));
        let mut metadata = json!([{"type":"table","rows":rows}]);
        arrange(&sheet, &groups, &mut metadata).unwrap();
        let parents: Vec<Value> = metadata[0]["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["parent"].clone())
            .collect();
        assert_eq!(
            parents,
            [
                Value::Null,
                Value::Null,
                json!("record_r2"),
                json!("record_r3"),
                json!("record_r3"),
                json!("record_r5"),
                Value::Null,
                json!("record_r7"),
            ]
        );
    }

    /// A cell of the interpretation in `cells` and its value in `values`, the
    /// extraction's, as classify reads them apart.
    /// `cells` as an interpretation holds them.
    fn element_cells(cells: &[Value]) -> Vec<ElementCell> {
        cells
            .iter()
            .map(|cell| ElementCell::from_value(cell.clone()).unwrap())
            .collect()
    }

    fn put(
        cells: &mut Vec<Value>,
        values: &mut BTreeMap<String, Value>,
        address: &str,
        role: &str,
        value: Value,
        headers: &[&str],
    ) {
        cells.push(json!({"id":address,"address":address,"role":role,"headers":headers,"text_state":"read"}));
        values.insert(address.to_owned(), value);
    }

    /// 販売 merged down A4:A8; 受注 and its note merged down B4:C6 beside
    /// JU001-JU003; 出荷 written on row 7 alone with a blank B8 under it.
    /// Each record is named in D.
    fn sales_table() -> (Vec<Value>, BTreeMap<String, Value>) {
        let mut cells = vec![];
        let mut values = BTreeMap::new();
        for x in ["A", "B", "C", "D", "E"] {
            put(
                &mut cells,
                &mut values,
                &format!("{x}3"),
                "column_header",
                json!(x),
                &[],
            );
        }
        put(
            &mut cells,
            &mut values,
            "A4",
            "row_header",
            json!("販売"),
            &["A3"],
        );
        put(
            &mut cells,
            &mut values,
            "B4",
            "row_header",
            json!("受注"),
            &["B3", "A4"],
        );
        put(
            &mut cells,
            &mut values,
            "C4",
            "row_header",
            json!("受注の説明"),
            &["C3", "A4"],
        );
        put(
            &mut cells,
            &mut values,
            "B7",
            "row_header",
            json!("出荷"),
            &["B3", "A4"],
        );
        put(
            &mut cells,
            &mut values,
            "B8",
            "data",
            Value::Null,
            &["B3", "A4", "B7"],
        );
        for (y, around) in [
            (4, ["A4", "B4", "C4"]),
            (5, ["A4", "B4", "C4"]),
            (6, ["A4", "B4", "C4"]),
            (7, ["A4", "B7", "B7"]),
            (8, ["A4", "B7", "B7"]),
        ] {
            let name = format!("D{y}");
            put(
                &mut cells,
                &mut values,
                &name,
                "row_header",
                json!(format!("ID{y}")),
                &[&["D3"][..], &around[..]].concat(),
            );
            put(
                &mut cells,
                &mut values,
                &format!("E{y}"),
                "data",
                json!(y),
                &[&["E3", name.as_str()][..], &around[..]].concat(),
            );
        }
        (cells, values)
    }

    #[test]
    fn categories_nest_hold_the_cells_of_their_first_row_and_cover_blanks_under_them() {
        let (mut cells, mut values) = sales_table();
        let interpreted = element_cells(&cells);
        let found = categories(
            &interpreted,
            &|address| {
                values
                    .get(address)
                    .map_or(crate::extraction::CellValue::Null, |value| {
                        crate::extraction::CellValue::of(value).unwrap()
                    })
            },
            &|_| None,
        )
        .unwrap();
        let shown: Vec<(Vec<&str>, u32, u32)> = found
            .iter()
            .map(|c| (c.cells.iter().map(|(_, id)| *id).collect(), c.row, c.end))
            .collect();
        assert_eq!(
            shown,
            [
                (vec!["A4"], 4, 8),
                (vec!["B4", "C4"], 4, 6),
                (vec!["B7"], 7, 8),
            ]
        );
        // A column written on every row heads nothing as a category.
        for y in 5..=8 {
            put(
                &mut cells,
                &mut values,
                &format!("A{y}"),
                "row_header",
                json!("販売"),
                &["A3"],
            );
        }
        let interpreted = element_cells(&cells);
        let found = categories(
            &interpreted,
            &|address| {
                values
                    .get(address)
                    .map_or(crate::extraction::CellValue::Null, |value| {
                        crate::extraction::CellValue::of(value).unwrap()
                    })
            },
            &|_| None,
        )
        .unwrap();
        assert!(found.iter().all(|c| c.cells[0].1 != "A4"));
    }

    #[test]
    fn classify_reads_what_is_written_under_a_category_from_the_extraction() {
        // The interpretation's cells hold no values: whether a row header has
        // anything written under it is read from the extraction's cells.
        let groups = |cells: &[Value], values: &BTreeMap<String, Value>| {
            let cells_of_sheet: Vec<Value> = values
                .iter()
                .map(|(address, value)| json!({"address":address,"value":value}))
                .collect();
            let extraction = crate::extraction::sample(
                json!({"sheets":[{"name":"S","merges":[],"cells":cells_of_sheet}]}),
            );
            let layout = super::super::identity::initial(&extraction).unwrap();
            let table = Element::new(
                json!({"id":"table-1","kind":"table","sheet":"S"}),
                element_cells(cells),
            );
            super::interpreted_groups(&layout, &extraction, &[table])
                .unwrap()
                .remove("sheet-1")
                .unwrap()
        };
        let categorized = |groups: &Groups| {
            groups
                .iter()
                .any(|(_, g)| g.category.as_deref() == Some("cell_r4_A"))
        };
        let (mut cells, mut values) = sales_table();
        assert!(categorized(&groups(&cells, &values)));
        // 販売 written on every row heads nothing as a category.
        for y in 5..=8 {
            put(
                &mut cells,
                &mut values,
                &format!("A{y}"),
                "row_header",
                json!("販売"),
                &["A3"],
            );
        }
        assert!(!categorized(&groups(&cells, &values)));
    }

    #[test]
    fn a_category_is_a_node_of_its_own_over_the_records_under_it() {
        // The groups classify gives the table above: 販売 around 受注 (with its
        // note) and 出荷, a blank B8 under 出荷.
        let records: [(u32, &[&str]); 5] = [
            (4, &["A", "B", "C", "D"]),
            (5, &["D"]),
            (6, &["D"]),
            (7, &["B", "D"]),
            (8, &["B", "D"]),
        ];
        let mut groups = serde_json::Map::new();
        let headings: Vec<Value> = ["A", "B", "C", "D"]
            .iter()
            .map(|x| json!({"id":format!("cell_r3_{x}"),"column":format!("column_{x}"),"value":x}))
            .collect();
        let mut rows = vec![json!({"id":"record_r3","cells":headings})];
        for x in ["A", "B", "C", "D"] {
            groups.insert(
                format!("cell_r3_{x}"),
                json!({"heading":true,"label":format!("cell_r3_{x}")}),
            );
        }
        for (y, columns) in records {
            let mut cells = vec![];
            for x in columns {
                let id = format!("cell_r{y}_{x}");
                let inner = if y <= 6 { "cell_r4_B" } else { "cell_r7_B" };
                let mut group = json!({"parent":null,"label":format!("cell_r3_{x}")});
                match (y, *x) {
                    (4, "A") => group["category"] = json!("cell_r4_A"),
                    (4, "B" | "C") | (7, "B") => {
                        group["category"] = json!(if y == 4 { "cell_r4_B" } else { "cell_r7_B" });
                        group["within"] = json!(["cell_r4_A"]);
                    }
                    (8, "B") => {
                        group["category"] = json!("cell_r7_B");
                        group["within"] = json!(["cell_r4_A", inner]);
                    }
                    _ => group["within"] = json!(["cell_r4_A", inner]),
                }
                groups.insert(id.clone(), group);
                let value = if (y, *x) == (8, "B") {
                    Value::Null
                } else {
                    json!(format!("{x}{y}"))
                };
                cells.push(json!({"id":id,"column":format!("column_{x}"),"value":value}));
            }
            rows.push(json!({"id":format!("record_r{y}"),"cells":cells}));
        }
        let sheet = json!({});
        let groups = typed(&json!(groups));
        let mut metadata = json!([{"type":"table","rows":rows}]);
        arrange(&sheet, &groups, &mut metadata).unwrap();
        assert_eq!(
            metadata[0]["categories"],
            json!({"cell_r4_A":{},"cell_r4_B":{"parent":"cell_r4_A","cells":["cell_r4_C"]},"cell_r7_B":{"parent":"cell_r4_A"}})
        );
        assert_eq!(metadata[0]["rows"][5]["cells"][0]["category"], "cell_r7_B");
        let (body, stored) = crate::document_body::split_json(metadata.clone()).unwrap();
        assert_eq!(
            body[0]["table"]["rows"],
            json!([{"A":"A4","children":[
                {"B":"B4","C":"C4","children":[{"D":"D4"},{"D":"D5"},{"D":"D6"}]},
                {"B":"B7","children":[{"D":"D7"},{"D":"D8"}]},
            ]}])
        );
        assert_eq!(
            crate::document_body::join_json(&body, &stored).unwrap(),
            metadata
        );
        // The groups the metadata gives back arrange it the same way.
        let regrouped = metadata_groups(&stored).unwrap();
        let mut rearranged = metadata.clone();
        rearranged[0].as_object_mut().unwrap().remove("categories");
        for row in rearranged[0]["rows"].as_array_mut().unwrap() {
            row.as_object_mut().unwrap().remove("parent");
            for cell in row["cells"].as_array_mut().unwrap() {
                cell.as_object_mut().unwrap().remove("category");
            }
        }
        arrange(&sheet, &regrouped, &mut rearranged).unwrap();
        assert_eq!(rearranged, metadata);
    }

    #[test]
    fn a_category_under_a_section_record_stays_under_it() {
        // ＜本番＞ on a row of its own heads the server AP01, merged down over
        // the paths beside it.
        let cell =
            |id: &str, column: &str, value: &str| json!({"id":id,"column":column,"value":value});
        let mut metadata = json!([{"type":"table","rows":[
            {"id":"record_r3","cells":[cell("cell_r3_A", "column_A", "環境"), cell("cell_r3_B", "column_B", "サーバ"), cell("cell_r3_C", "column_C", "パス")]},
            {"id":"record_r4","cells":[cell("cell_r4_A", "column_A", "＜本番＞")]},
            {"id":"record_r5","cells":[cell("cell_r5_B", "column_B", "AP01"), cell("cell_r5_C", "column_C", "/opt")]},
            {"id":"record_r6","cells":[cell("cell_r6_C", "column_C", "/var")]},
        ]}]);
        let groups = json!({
            "cell_r3_A":{"heading":true},"cell_r3_B":{"heading":true},"cell_r3_C":{"heading":true},
            "cell_r4_A":{"parent":null},
            "cell_r5_B":{"parent":"r4","category":"cell_r5_B"},
            "cell_r5_C":{"parent":"r4","within":["cell_r5_B"]},
            "cell_r6_C":{"parent":"r4","within":["cell_r5_B"]},
        });
        let sheet = json!({});
        let groups = typed(&json!(groups));
        arrange(&sheet, &groups, &mut metadata).unwrap();
        assert_eq!(
            metadata[0]["categories"],
            json!({"cell_r5_B":{"parent":"record_r4"}})
        );
        let (body, stored) = crate::document_body::split_json(metadata.clone()).unwrap();
        assert_eq!(
            body[0]["table"]["rows"],
            json!([{"A":"＜本番＞","children":[{"B":"AP01","children":[{"C":"/opt"},{"C":"/var"}]}]}])
        );
        let regrouped = metadata_groups(&stored).unwrap();
        let mut rearranged = metadata.clone();
        rearranged[0].as_object_mut().unwrap().remove("categories");
        for row in rearranged[0]["rows"].as_array_mut().unwrap() {
            row.as_object_mut().unwrap().remove("parent");
        }
        arrange(&sheet, &regrouped, &mut rearranged).unwrap();
        assert_eq!(rearranged, metadata);
    }

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

    /// A table of `categories` categories of `records` records each, the
    /// last `broken` of them with a record in the middle outside them, as
    /// [`super::categorize`] takes it: its rows and their groups.
    fn categorized_table(categories: usize, records: usize, broken: usize) -> (Vec<Value>, Groups) {
        let mut rows = vec![];
        let mut groups = serde_json::Map::new();
        for k in 0..categories {
            let key = format!("cell_r{}_A", rows.len() + 1);
            for r in 0..=records {
                let id = format!(
                    "cell_r{}_{}",
                    rows.len() + 1,
                    if r == 0 { "A" } else { "B" }
                );
                let group = if r == 0 {
                    json!({"category":key,"parent":null})
                } else if k >= categories - broken && r == records / 2 {
                    json!({"parent":null,"within":[]})
                } else {
                    json!({"parent":null,"within":[key]})
                };
                groups.insert(id.clone(), group);
                rows.push(json!({"cells":[{"id":id}]}));
            }
        }
        (rows, typed(&Value::Object(groups)))
    }

    /// [`super::drop_broken`] as it was written first: every category
    /// checked over every record each time one is dropped.
    fn drop_broken_by_scanning(
        kept: &mut BTreeSet<String>,
        chains: &mut [Option<Vec<String>>],
        first: &BTreeMap<&str, usize>,
    ) {
        loop {
            let broken = kept.iter().find(|&key| {
                let members: Vec<(usize, &Vec<String>)> = chains
                    .iter()
                    .enumerate()
                    .filter_map(|(i, c)| Some((i, c.as_ref()?)))
                    .filter(|(_, c)| c.contains(key))
                    .collect();
                let Some(&(start, chain)) = members.first() else {
                    return true;
                };
                let end = members.last().unwrap().0;
                let before =
                    |c: &Vec<String>| c[..c.iter().position(|k| k == key).unwrap()].to_vec();
                let around = before(chain);
                start != first[key.as_str()]
                    || around.len() + 2 > super::MAX_TREE_DEPTH
                    || chains[start..=end]
                        .iter()
                        .flatten()
                        .any(|c| !c.contains(key))
                    || members.iter().any(|(_, c)| before(c) != around)
            });
            let Some(key) = broken.cloned() else {
                break;
            };
            kept.remove(&key);
            for chain in chains.iter_mut().flatten() {
                chain.retain(|k| *k != key);
            }
        }
    }

    #[test]
    fn broken_categories_are_dropped_in_order() {
        // A small linear congruential generator, so that the cases repeat.
        let mut seed: u64 = 7;
        let mut next = |bound: usize| {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (seed >> 33) as usize % bound
        };
        let mut dropped = 0;
        for case in 0..400 {
            let keys: Vec<String> = (0..1 + next(12)).map(|k| format!("k{k:02}")).collect();
            let deep = case % 10 == 0;
            let mut chains: Vec<Option<Vec<String>>> = (0..1 + next(40))
                .map(|_| {
                    (next(6) != 0).then(|| {
                        let length = if deep { 10 + next(8) } else { next(4) };
                        (0..length)
                            .map(|_| keys[next(keys.len())].clone())
                            .collect()
                    })
                })
                .collect();
            let mut first = BTreeMap::new();
            for (i, chain) in chains.iter().enumerate() {
                for key in chain.iter().flatten() {
                    first.entry(key.clone()).or_insert(i);
                }
            }
            for key in &keys {
                if next(5) == 0 {
                    first.insert(key.clone(), next(chains.len()));
                }
                first.entry(key.clone()).or_insert(0);
            }
            let first: BTreeMap<&str, usize> =
                first.iter().map(|(key, at)| (key.as_str(), *at)).collect();
            let mut kept: BTreeSet<String> = keys.iter().cloned().collect();
            let (mut expected_kept, mut expected_chains) = (kept.clone(), chains.clone());
            drop_broken_by_scanning(&mut expected_kept, &mut expected_chains, &first);
            super::drop_broken(&mut kept, &mut chains, &first);
            assert_eq!(
                (&kept, &chains),
                (&expected_kept, &expected_chains),
                "case {case}"
            );
            dropped += keys.len() - kept.len();
        }
        assert!(dropped > 100);
    }

    #[test]
    #[ignore = "manual category performance measurement"]
    fn measure_broken_categories() {
        use std::time::Instant;
        for (categories, broken) in [(500, 0), (500, 50), (2000, 0), (2000, 100)] {
            let (rows, groups) = categorized_table(categories, 10, broken);
            let start = Instant::now();
            let rows: Vec<_> = rows
                .into_iter()
                .map(|row| crate::documents::MetaRow::from_value(row).unwrap())
                .collect();
            let sorted = super::categorize(&rows, 0, &groups, &|_, _| false).unwrap();
            eprintln!(
                "categorize categories={categories} broken={broken} rows={} kept={} ms={:.1}",
                rows.len(),
                sorted.nodes.len(),
                start.elapsed().as_secs_f64() * 1000.0
            );
            assert_eq!(sorted.nodes.len(), categories - broken);
        }
    }

    /// A point takes the smallest table around it, none when two are as
    /// small, whatever the order of the points.
    #[test]
    fn points_take_the_innermost_table_around_them() {
        let bounds = BTreeMap::from([
            ("outer".to_owned(), (1, 1, 10, 10)),
            ("inner".to_owned(), (3, 3, 4, 4)),
            ("twin_a".to_owned(), (6, 1, 7, 2)),
            ("twin_b".to_owned(), (6, 1, 7, 2)),
        ]);
        let points = [(9, 9), (3, 4), (6, 2), (11, 1), (4, 5), (1, 1), (4, 3)];
        assert_eq!(
            super::innermost(&bounds, &points),
            [
                Some("outer"),
                Some("inner"),
                None,
                None,
                Some("outer"),
                Some("outer"),
                Some("inner"),
            ]
        );
    }

    /// An adopted cell joins the innermost table around it, as a heading in
    /// a record of headings and otherwise under the parent of the record's
    /// first cell in the order of the IDs, not of the columns.
    #[test]
    fn adopted_cells_join_the_record_of_their_row() {
        let lines = |ids: &[&str]| {
            let entries: serde_json::Map<String, Value> = ids
                .iter()
                .enumerate()
                .map(|(i, id)| (id.to_string(), json!({"key":id,"position":i + 1})))
                .collect();
            Value::Object(entries)
        };
        let sheet = layout_sheet(&json!({
            "rows": lines(&["r1", "r2", "r3"]),
            "columns": lines(&["A", "B", "C", "AA"]),
        }));
        let mut groups = typed(&json!({
            "cell_r1_A": {"table":"t","heading":true},
            "cell_r1_AA": {"table":"t","heading":true},
            "cell_r2_B": {"table":"t","parent":"b","within":["wb"]},
            "cell_r2_AA": {"table":"t","parent":"aa","within":["waa"]},
            "cell_r3_A": {"table":"t"},
            "cell_r3_B": {"table":"n","parent":"nb"},
            "cell_r3_AA": {"table":"n","parent":"naa"},
        }));
        let cells = [
            "cell_r1_B",
            "cell_r2_C",
            "cell_r3_C",
            "cell_r9_A",
            "cell_r1_B",
            "shape_1",
        ]
        .map(String::from);
        super::adopt_cells(&sheet, &mut groups, &cells).unwrap();
        assert_eq!(groups.len(), 10);
        let expected = typed(&json!({
            "cell_r1_B": {"table":"t","heading":true},
            "cell_r2_C": {"table":"t","parent":"aa","within":["waa"]},
            "cell_r3_C": {"table":"n","parent":"naa"},
        }));
        for (id, group) in expected.iter() {
            assert_eq!(groups.get(&id), Some(group), "{id}");
        }
    }
}
