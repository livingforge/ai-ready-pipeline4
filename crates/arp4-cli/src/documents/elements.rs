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

/// The group of every table cell, as the element `metadata` of a page records
/// it: its table and what [`arrange`] took from the interpretation, a cell of
/// a heading record as a `heading`, a mark as a `mark`, the heading its
/// column's key is from as its `label`, the row of its record's parent as
/// its `parent`, null for none (for the first record of a category, the
/// parent it has without the category), a cell of a category or a blank one
/// under it the category as its `category`, and the categories its record is
/// in, or a category's cell those around it, as its `within`.
fn groups(metadata: &Value) -> Result<Value> {
    let mut groups = serde_json::Map::new();
    for element in array(metadata)? {
        if element["type"] != "table" {
            continue;
        }
        let rows = array(&element["rows"])?;
        let headings = element["headings"].as_u64().unwrap_or(0);
        let mut record_rows = HashMap::new();
        for row in rows {
            let first = string(&row["cells"][0]["id"])?;
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
            // The parent the record would have without the categories its
            // cells hold: that of the outermost of them.
            let mut parent = row["parent"].as_str();
            while let Some(key) = parent.filter(|p| categories.contains_key(*p)) {
                let first = |id: &str| cell_axes(id).map(|(row, _)| row);
                if first(key) != first(string(&row["cells"][0]["id"])?) {
                    parent = None;
                    break;
                }
                parent = categories[key]["parent"].as_str();
            }
            let parent = parent
                .and_then(|p| record_rows.get(p))
                .map_or(Value::Null, |axis| json!(axis));
            let mut chain: Vec<&str> = match row["parent"].as_str() {
                Some(p) if categories.contains_key(p) => {
                    let mut chain = around(p, categories, &within)?;
                    chain.push(p);
                    chain
                }
                Some(p) => within.get(p).cloned().unwrap_or_default(),
                None => vec![],
            };
            let node_chains: HashMap<&str, Vec<&str>> = array(&row["cells"])?
                .iter()
                .filter_map(|c| node_cells.get(c["id"].as_str()?).copied())
                .map(|key| Ok((key, around(key, categories, &within)?)))
                .collect::<Result<_>>()?;
            if let Some(id) = row["id"].as_str() {
                within.insert(id, std::mem::take(&mut chain));
            }
            let chain = row["id"].as_str().map_or(&chain, |id| &within[id]);
            for cell in array(&row["cells"])? {
                let mut group = json!({"table":element["id"]});
                if i < headings {
                    group["heading"] = json!(true);
                } else {
                    group["parent"] = parent.clone();
                }
                let id = string(&cell["id"])?;
                let category = node_cells
                    .get(id)
                    .copied()
                    .or_else(|| cell["category"].as_str());
                let around = match node_cells.get(id) {
                    Some(key) => &node_chains[key],
                    None => chain,
                };
                if let Some(key) = category {
                    group["category"] = json!(key);
                }
                if !around.is_empty() {
                    group["within"] = json!(around);
                }
                if cell["mark"] == true {
                    group["mark"] = json!(true);
                }
                let column = string(&cell["column"])?;
                if let Some(label) = cell["label"]
                    .as_str()
                    .or_else(|| element["labels"][column].as_str())
                {
                    group["label"] = json!(label);
                }
                groups.insert(string(&cell["id"])?.to_owned(), group);
            }
        }
    }
    Ok(Value::Object(groups))
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
fn arrange(sheet: &Value, metadata: &mut Value) -> Result<()> {
    let groups = &sheet["groups"];
    let position = |axis: &str, id: &str| sheet[axis][id]["position"].as_u64();
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
    let beyond = |key: &str, row: &Value| -> bool {
        let record = row["cells"][0]["id"].as_str().and_then(cell_axes);
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
    for element in metadata
        .as_array_mut()
        .context("element metadata missing")?
        .iter_mut()
        .filter(|e| e["type"] == "table")
    {
        let rows = element["rows"]
            .as_array_mut()
            .context("table rows missing")?;
        let group = |cell: &Value| -> Result<&Value> { Ok(&groups[string(&cell["id"])?]) };
        let mut headings = 0;
        for row in rows.iter() {
            let mut cells = array(&row["cells"])?.iter();
            if !cells.all(|c| group(c).is_ok_and(|g| g["heading"] == true)) {
                break;
            }
            headings += 1;
        }
        let mut heading_cells = HashMap::new();
        for row in &rows[..headings] {
            for cell in array(&row["cells"])? {
                heading_cells.insert(string(&cell["id"])?.to_owned(), heading_cells.len());
            }
        }
        let mut votes: BTreeMap<String, HashMap<String, usize>> = BTreeMap::new();
        let mut record_index = HashMap::new();
        for (i, row) in rows.iter().enumerate().skip(headings) {
            let first = string(&row["cells"][0]["id"])?;
            if let Some((axis, _)) = cell_axes(first) {
                record_index.insert(axis, i);
            }
        }
        let mut proposed: Vec<(Option<usize>, bool)> = vec![(None, true); rows.len()];
        for (i, row) in rows.iter_mut().enumerate().skip(headings) {
            let mut known = false;
            for cell in row["cells"].as_array_mut().context("table cells missing")? {
                let g = group(cell)?;
                if g["mark"] == true {
                    cell["mark"] = json!(true);
                    continue;
                }
                if let Some(label) = g["label"]
                    .as_str()
                    .filter(|l| heading_cells.contains_key(*l))
                {
                    *votes
                        .entry(string(&cell["column"])?.to_owned())
                        .or_default()
                        .entry(label.to_owned())
                        .or_default() += 1;
                }
                if let Some(parent) = g.get("parent") {
                    known = true;
                    if let Some(&j) = parent.as_str().and_then(|p| record_index.get(p))
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
            for cell in array(&row["cells"])? {
                heading_columns.insert(
                    string(&cell["id"])?.to_owned(),
                    string(&cell["column"])?.to_owned(),
                );
            }
        }
        for row in rows.iter_mut().skip(headings) {
            for cell in row["cells"].as_array_mut().context("table cells missing")? {
                let g = group(cell)?;
                if g["mark"] == true {
                    continue;
                }
                let column = string(&cell["column"])?;
                if let Some(label) = g["label"]
                    .as_str()
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
                parent = proposed[i + 1..]
                    .iter()
                    .find_map(|&(p, _)| p)
                    .filter(|&p| p < i && same(&p));
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
            rows[i]["cells"][at]["category"] = json!(name(key));
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
    }
    Ok(())
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
    rows: &[Value],
    headings: usize,
    groups: &Value,
    beyond: &dyn Fn(&str, &Value) -> bool,
) -> Result<Categories> {
    let group = |cell: &Value| -> Result<&Value> { Ok(&groups[string(&cell["id"])?]) };
    // The records holding each category's cells, the first holding the
    // category itself.
    let mut held: BTreeMap<String, Vec<(usize, usize)>> = BTreeMap::new();
    for (i, row) in rows.iter().enumerate().skip(headings) {
        for (at, cell) in array(&row["cells"])?.iter().enumerate() {
            if let Some(key) = group(cell)?["category"].as_str() {
                held.entry(key.to_owned()).or_default().push((i, at));
            }
        }
    }
    let first: BTreeMap<&str, usize> = held
        .iter()
        .map(|(key, at)| (key.as_str(), at.iter().map(|(i, _)| *i).min().unwrap()))
        .collect();
    let node_cell = |i: usize, g: &Value| {
        g["category"]
            .as_str()
            .is_some_and(|key| first.get(key) == Some(&i))
    };
    let listed = |g: &Value| -> Vec<String> {
        g["within"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|k| k.as_str().map(str::to_owned))
            .collect()
    };
    // The categories each record placed by the groups is in, outer first.
    let mut chains: Vec<Option<Vec<String>>> = vec![None; rows.len()];
    for (i, row) in rows.iter().enumerate().skip(headings) {
        let cells = array(&row["cells"])?;
        let mut own: Option<Vec<String>> = None;
        let mut deepest: Option<Vec<String>> = None;
        for cell in cells {
            let g = group(cell)?;
            if node_cell(i, g) {
                let mut chain = listed(g);
                chain.push(g["category"].as_str().unwrap().to_owned());
                if deepest.as_ref().is_none_or(|d| chain.len() > d.len()) {
                    deepest = Some(chain);
                }
            } else if own.is_none() && g.get("parent").is_some() {
                own = Some(listed(g));
            }
        }
        chains[i] = own.or(deepest);
    }
    let mut kept: BTreeSet<String> = held.keys().cloned().collect();
    for chain in chains.iter_mut().flatten() {
        chain.retain(|k| kept.contains(k));
    }
    // A category whose records do not follow each other from its first, or
    // are not all in the categories around it, is none.
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
            let before = |c: &Vec<String>| c[..c.iter().position(|k| k == key).unwrap()].to_vec();
            let around = before(chain);
            start != first[key.as_str()]
                || around.len() + 2 > MAX_TREE_DEPTH
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
    let mut nodes = BTreeMap::new();
    for key in &kept {
        let start = first[key.as_str()];
        let chain = chains[start].as_ref().unwrap();
        let at = chain.iter().position(|k| k == key).unwrap();
        let cells = held[key]
            .iter()
            .filter(|(i, _)| *i == start)
            .map(|&(i, at)| Ok(string(&rows[i]["cells"][at]["id"])?.to_owned()))
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
            let cell = &rows[i]["cells"][place];
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

/// The digest of what [`classify`] takes from the elements of a structure
/// interpretation: each table's sheet and its cells with their roles and
/// headers. A layout classified from another interpretation, such as one an
/// earlier inference made, has another.
pub(super) fn interpretation_digest(structure: &[Value]) -> Result<String> {
    let mut tables = vec![];
    for element in structure.iter().filter(|e| e["kind"] == "table") {
        let cells: Vec<Value> = array(&element["cells"])?
            .iter()
            .map(|cell| {
                json!({"id":cell["id"],"address":cell["address"],"role":cell["role"],"headers":cell["headers"]})
            })
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
        // Cells are matched by the keys their rows and columns have in the
        // extraction, which pending operations leave as they are.
        let identities = |axis: &str| -> Result<HashMap<String, String>> {
            sheet[axis]
                .as_object()
                .context("layout axis missing")?
                .iter()
                .map(|(id, e)| Ok((string(&e["key"])?.to_owned(), id.clone())))
                .collect()
        };
        let rows = identities("rows")?;
        let columns = identities("columns")?;
        let mut ids = BTreeMap::new();
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
        for element in structure
            .iter()
            .filter(|e| e["sheet"] == sheet["name"] && e["kind"] == "table")
        {
            let cells = array(&element["cells"])?;
            let Some(first) = cells.iter().find_map(|c| ids.get(c["id"].as_str()?)) else {
                continue;
            };
            let table = token("table", &[first]);
            // The role and place of each cell of the table, by its ID in the
            // interpretation.
            let mut places = HashMap::new();
            // The names of each row, by column.
            let mut names: HashMap<u32, Vec<(u32, &str)>> = HashMap::new();
            for cell in cells {
                let (x, y) = excel::coordinate(string(&cell["address"])?)?;
                places.insert(string(&cell["id"])?, (cell["role"].as_str(), y, x));
                if cell["role"] == "row_header" {
                    names.entry(y).or_default().push((x, string(&cell["id"])?));
                }
            }
            for cell in cells {
                let Some(id) = ids.get(string(&cell["id"])?) else {
                    continue;
                };
                let own = places[string(&cell["id"])?];
                let mut group = json!({"table":table});
                match own.0 {
                    Some("column_header") => group["heading"] = json!(true),
                    Some("text") => group["mark"] = json!(true),
                    _ => {}
                }
                // The nearest heading over the cell names its key, and the
                // nearest name heading it from an earlier row its parent. A
                // record holding a deeper name that heads no part of the cell,
                // as a folder beside the group the cell is in, is no parent:
                // the cell is beside that folder, not in it.
                let headers: Vec<&str> = array(&cell["headers"])?
                    .iter()
                    .filter_map(Value::as_str)
                    .collect();
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
                        Some("column_header") if label.is_none_or(|(p, _)| (y, x) > p) => {
                            label = Some(((y, x), at));
                        }
                        Some("row_header")
                            if y < own.1
                                && !beside(y, x)
                                && parent.is_none_or(|(p, _)| (y, x) > p) =>
                        {
                            parent = Some(((y, x), at));
                        }
                        _ => {}
                    }
                }
                if let Some((_, at)) = label {
                    group["label"] = json!(at);
                }
                group["parent"] = parent
                    .and_then(|(_, at)| cell_axes(at))
                    .map_or(Value::Null, |(row, _)| json!(row));
                groups[id] = group;
            }
            // Each cell of a category names it, by the ID of its first cell,
            // as its `category`, and so does a blank cell under it in its
            // column; every other cell of the records of a category lists the
            // categories its row is in, outer first, as its `within`, and a
            // cell of a category those around it.
            let found: Vec<(String, Category)> = categories(cells)?
                .into_iter()
                .filter_map(|c| Some((ids.get(c.cells[0].1)?.clone(), c)))
                .collect();
            for cell in cells {
                let Some(id) = ids.get(string(&cell["id"])?) else {
                    continue;
                };
                if cell["role"] == "column_header" {
                    continue;
                }
                let (x, y) = excel::coordinate(string(&cell["address"])?)?;
                let own = found
                    .iter()
                    .find(|(_, c)| c.row == y && c.cells.iter().any(|&(_, at)| at == cell["id"]));
                let within: Vec<&(String, Category)> = found
                    .iter()
                    .filter(|(_, c)| c.row <= y && y <= c.end)
                    .filter(|(_, c)| own.is_none_or(|(_, o)| c.cells[0].0 < o.cells[0].0))
                    .collect();
                let category = own.or_else(|| {
                    within.iter().copied().find(|(_, c)| {
                        c.row < y && c.cells.iter().any(|&(at, _)| at == x) && blank(&cell["value"])
                    })
                });
                if let Some((key, _)) = category {
                    groups[id]["category"] = json!(key);
                }
                if !within.is_empty() {
                    groups[id]["within"] = json!(within.iter().map(|(k, _)| k).collect::<Vec<_>>());
                }
            }
        }
        sheet["groups"] = groups;
    }
    Ok(())
}

/// Puts each cell of `cells`, given a value the page's element metadata
/// holds none for, in the innermost table of the layout `sheet` whose cells
/// are around it, as an insertion's cells go: a cell of a record joins its
/// record, a heading of the leading records as a heading. The groups the
/// next [`encode`] arranges the page with are those of the metadata and these.
pub(super) fn adopt_cells(sheet: &mut Value, cells: &[String]) -> Result<()> {
    if sheet.get("groups").is_none() {
        sheet["groups"] = groups(&sheet["element_metadata"])?;
    }
    let position = |sheet: &Value, id: &str| -> Option<(u64, u64)> {
        let (row, column) = cell_axes(id)?;
        Some((
            sheet["rows"][&row]["position"].as_u64()?,
            sheet["columns"][&column]["position"].as_u64()?,
        ))
    };
    let mut bounds: BTreeMap<String, (u64, u64, u64, u64)> = BTreeMap::new();
    for (id, group) in sheet["groups"].as_object().context("groups missing")? {
        let (Some(table), Some((y, x))) = (group["table"].as_str(), position(sheet, id)) else {
            continue;
        };
        let b = bounds.entry(table.to_owned()).or_insert((y, x, y, x));
        b.0 = b.0.min(y);
        b.1 = b.1.min(x);
        b.2 = b.2.max(y);
        b.3 = b.3.max(x);
    }
    for id in cells {
        if !sheet["groups"][id].is_null() {
            continue;
        }
        let Some((y, x)) = position(sheet, id) else {
            continue;
        };
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
        let [(table, _)] = candidates.as_slice() else {
            continue;
        };
        let (row, _) = cell_axes(id).context("cell ID")?;
        // The other cells of its record, in the same table.
        let record: Vec<&Value> = sheet["groups"]
            .as_object()
            .unwrap()
            .iter()
            .filter(|(other, group)| {
                group["table"] == table.as_str() && cell_axes(other).is_some_and(|(r, _)| r == row)
            })
            .map(|(_, group)| group)
            .collect();
        let mut group = json!({"table":table});
        if !record.is_empty() && record.iter().all(|g| g["heading"] == true) {
            group["heading"] = json!(true);
        } else if let Some(first) = record.first() {
            for key in ["parent", "within"] {
                if let Some(value) = first.get(key) {
                    group[key] = value.clone();
                }
            }
        }
        sheet["groups"][id] = group;
    }
    Ok(())
}

/// Puts in the groups of the layout `sheet` the groups that `classified`,
/// the same sheet classified from the interpretation, gives the cells `ids`
/// coming back to the page, as when an operation deleting them is taken
/// back. A table the sheet holds other cells of keeps the ID the sheet gives
/// it.
pub(super) fn restore_groups(sheet: &mut Value, classified: &Value, ids: &[String]) -> Result<()> {
    if sheet.get("groups").is_none() {
        sheet["groups"] = groups(&sheet["element_metadata"])?;
    }
    let mut names: HashMap<&str, String> = HashMap::new();
    for (id, group) in classified["groups"].as_object().into_iter().flatten() {
        if let (Some(table), Some(current)) = (
            group["table"].as_str(),
            sheet["groups"][id]["table"].as_str(),
        ) {
            names.entry(table).or_insert_with(|| current.to_owned());
        }
    }
    for id in ids {
        let mut group = classified["groups"][id].clone();
        if group.is_null() {
            continue;
        }
        if let Some(name) = group["table"].as_str().and_then(|table| names.get(table)) {
            group["table"] = json!(name);
        }
        sheet["groups"][id] = group;
    }
    Ok(())
}

/// The key of the first category of the element `metadata` of a page whose
/// cells are all among the `deleted` cell IDs while a record under it keeps a
/// cell, as when the row holding a category merged down over other records is
/// deleted: Excel would drop the category's value with that row.
pub(super) fn orphaned_category(
    metadata: &Value,
    deleted: &BTreeSet<String>,
) -> Result<Option<String>> {
    for element in array(metadata)?.iter().filter(|e| e["type"] == "table") {
        let Some(categories) = element["categories"].as_object() else {
            continue;
        };
        // The categories each record is in, by its ID.
        let mut within: HashMap<&str, Vec<&str>> = HashMap::new();
        let mut kept: BTreeSet<&str> = BTreeSet::new();
        for row in array(&element["rows"])? {
            let chain = match row["parent"].as_str() {
                Some(p) if categories.contains_key(p) => {
                    let mut chain = around(p, categories, &within)?;
                    chain.push(p);
                    chain
                }
                Some(p) => within.get(p).cloned().unwrap_or_default(),
                None => vec![],
            };
            if array(&row["cells"])?
                .iter()
                .any(|c| c["id"].as_str().is_some_and(|id| !deleted.contains(id)))
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
fn blank(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(s) => s.trim().is_empty(),
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
/// category over a single record. Cells of a row reaching as far are one
/// category; one crossing the bounds of a category left of it is none.
fn categories(cells: &[Value]) -> Result<Vec<Category<'_>>> {
    struct Seen<'a> {
        id: &'a str,
        x: u32,
        y: u32,
        headers: Vec<&'a str>,
    }
    let mut seen = vec![];
    let mut names: HashMap<u32, Vec<(u32, &str)>> = HashMap::new();
    let mut written = BTreeSet::new();
    let mut heads: HashMap<&str, Vec<usize>> = HashMap::new();
    for cell in cells {
        let (x, y) = excel::coordinate(string(&cell["address"])?)?;
        let id = string(&cell["id"])?;
        let headers: Vec<&str> = array(&cell["headers"])?
            .iter()
            .filter_map(Value::as_str)
            .collect();
        if cell["role"] == "row_header" {
            names.entry(y).or_default().push((x, id));
        }
        if !blank(&cell["value"]) {
            written.insert((x, y));
        }
        for header in &headers {
            heads.entry(header).or_default().push(seen.len());
        }
        seen.push(Seen { id, x, y, headers });
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
    for &c in &row_headers {
        if columns.contains(&c.x)
            && !found.iter().any(|(f, _)| f.id == c.id)
            && name_beyond(c, &[])
            && let Some(end) = reach(c)
        {
            found.push((c, end));
        }
    }
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
    let mut kept: Vec<Category<'_>> = vec![];
    for c in found {
        if !kept
            .iter()
            .any(|k| k.row <= c.end && c.row <= k.end && !(k.row <= c.row && c.end <= k.end))
        {
            kept.push(c);
        }
    }
    Ok(kept)
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
pub(super) fn complete(sheet: &Value, metadata: &mut Value) -> Result<()> {
    let position = |axis: &str, id: &str, missing: u64| {
        sheet[axis][id]["position"].as_u64().unwrap_or(missing)
    };
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
    let elements = metadata
        .as_array_mut()
        .context("element metadata missing")?;
    for element in elements.iter_mut().filter(|e| e["type"] == "table") {
        let rows = element["rows"]
            .as_array_mut()
            .context("table rows missing")?;
        let mut cells = vec![];
        let mut names = BTreeMap::new();
        for (record, row) in rows.iter_mut().enumerate() {
            let first = string(&row["cells"][0]["id"])?;
            let (id, _) =
                cell_axes(first).with_context(|| format!("table cell {first} names no cell"))?;
            row["id"] = json!(token("record", &[&id]));
            for cell in row["cells"].as_array_mut().context("table cells missing")? {
                let id = string(&cell["id"])?;
                let (row, column) =
                    cell_axes(id).with_context(|| format!("table cell {id} names no cell"))?;
                let at = (
                    position("rows", &row, u64::MAX),
                    position("columns", &column, 0),
                );
                names.insert(at.1, token("column", &[&column]));
                cells.push((record, at, cell));
            }
        }
        let places: Vec<_> = cells.iter().map(|(record, at, _)| (*record, *at)).collect();
        let logical = logical_columns(&places, &merged);
        let logical_rows: BTreeSet<u64> = cells.iter().map(|(_, (y, _), _)| *y).collect();
        let logical_columns: BTreeMap<u64, Value> = logical
            .values()
            .map(|first| (*first, json!({"id":names[first]})))
            .collect();
        for (_, (y, x), cell) in cells {
            let cell = cell
                .as_object_mut()
                .context("table cell must be an object")?;
            cell.insert("column".into(), json!(names[&logical[&x]]));
            cell.remove("row_span");
            cell.remove("column_span");
            if let Some((bottom, right)) = merged.get(&(y, x)) {
                let spanned: BTreeSet<_> = logical.range(x..=*right).map(|(_, l)| l).collect();
                for (key, count) in [
                    ("row_span", logical_rows.range(y..=*bottom).count()),
                    ("column_span", spanned.len()),
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

/// The version of content pages: tables as the trees of their records and
/// categories.
pub(crate) const CONTENT_VERSION: &str = "6";

/// The YAML of the content `page` of `layout`, as [`crate::document_body::yaml`]
/// orders it.
pub(super) fn page_bytes(page: &Value, layout: &Value) -> Result<Vec<u8>> {
    let page_id = string(&page["page_id"])?;
    crate::document_body::yaml(page, &layout["sheets"][page_id]["element_metadata"])
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
    out["schema_version"] = json!(CONTENT_VERSION);
    let mut elements = Value::Array(elements);
    complete(sheet, &mut elements)?;
    arrange(sheet, &mut elements)?;
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
        page["schema_version"] == CONTENT_VERSION,
        "Document content must use the tree of table records; re-import the original"
    );
    let sheet = &layout["sheets"][string(&page["page_id"])?];
    // The body is read beside its metadata rather than joined into a copy:
    // its shape is checked first, as joining it would.
    let metadata = &sheet["element_metadata"];
    crate::document_body::each(&page["elements"], metadata, |_, _| Ok(()))?;
    super::visual_elements::validate_references(metadata, sheet)?;
    let mut out = identity::copy_except(page, &[&["elements"]]);
    ensure!(out.is_object(), "page must be an object");
    out["schema_version"] = json!(identity::GRID_VERSION);
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
    use super::{arrange, categories, complete, token};
    use serde_json::{Value, json};

    #[test]
    fn columns_under_a_merged_heading_are_one_unless_a_record_uses_two() {
        // A1:C1 and D1:F1 head values written in A and F; G1:H1 heads G2 and H2.
        let letters = ["A", "B", "C", "D", "E", "F", "G", "H"];
        let sheet = json!({
            "rows":(1..=3).map(|y| (format!("r{y}"), json!({"position":y}))).collect::<serde_json::Map<_, _>>(),
            "columns":letters.iter().zip(1..).map(|(x, at)| (x.to_string(), json!({"position":at}))).collect::<serde_json::Map<_, _>>(),
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
        let (body, _) = crate::document_body::split(
            &metadata
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
        let sheet = json!({"groups":groups});
        let mut metadata = json!([{"type":"table","rows":rows}]);
        arrange(&sheet, &mut metadata).unwrap();
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

    #[test]
    fn categories_nest_hold_the_cells_of_their_first_row_and_cover_blanks_under_them() {
        // 販売 merged down A4:A8; 受注 and its note merged down B4:C6 beside
        // JU001-JU003; 出荷 written on row 7 alone with a blank B8 under it.
        // Each record is named in D.
        fn put(cells: &mut Vec<Value>, address: &str, role: &str, value: Value, headers: &[&str]) {
            cells.push(
                json!({"id":address,"address":address,"role":role,"value":value,"headers":headers}),
            );
        }
        let mut cells = vec![];
        for x in ["A", "B", "C", "D", "E"] {
            put(&mut cells, &format!("{x}3"), "column_header", json!(x), &[]);
        }
        put(&mut cells, "A4", "row_header", json!("販売"), &["A3"]);
        put(&mut cells, "B4", "row_header", json!("受注"), &["B3", "A4"]);
        put(
            &mut cells,
            "C4",
            "row_header",
            json!("受注の説明"),
            &["C3", "A4"],
        );
        put(&mut cells, "B7", "row_header", json!("出荷"), &["B3", "A4"]);
        put(&mut cells, "B8", "data", Value::Null, &["B3", "A4", "B7"]);
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
                &name,
                "row_header",
                json!(format!("ID{y}")),
                &[&["D3"][..], &around[..]].concat(),
            );
            put(
                &mut cells,
                &format!("E{y}"),
                "data",
                json!(y),
                &[&["E3", name.as_str()][..], &around[..]].concat(),
            );
        }
        let found = categories(&cells).unwrap();
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
                &format!("A{y}"),
                "row_header",
                json!("販売"),
                &["A3"],
            );
        }
        let found = categories(&cells).unwrap();
        assert!(found.iter().all(|c| c.cells[0].1 != "A4"));
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
        let sheet = json!({"groups":groups});
        let mut metadata = json!([{"type":"table","rows":rows}]);
        arrange(&sheet, &mut metadata).unwrap();
        assert_eq!(
            metadata[0]["categories"],
            json!({"cell_r4_A":{},"cell_r4_B":{"parent":"cell_r4_A","cells":["cell_r4_C"]},"cell_r7_B":{"parent":"cell_r4_A"}})
        );
        assert_eq!(metadata[0]["rows"][5]["cells"][0]["category"], "cell_r7_B");
        let (body, stored) = crate::document_body::split(&metadata).unwrap();
        assert_eq!(
            body[0]["table"]["rows"],
            json!([{"A":"A4","children":[
                {"B":"B4","C":"C4","children":[{"D":"D4"},{"D":"D5"},{"D":"D6"}]},
                {"B":"B7","children":[{"D":"D7"},{"D":"D8"}]},
            ]}])
        );
        assert_eq!(
            crate::document_body::join(&body, &stored).unwrap(),
            metadata
        );
        // The groups the metadata gives back arrange it the same way.
        let regrouped = json!({"groups":super::groups(&stored).unwrap()});
        let mut rearranged = metadata.clone();
        rearranged[0].as_object_mut().unwrap().remove("categories");
        for row in rearranged[0]["rows"].as_array_mut().unwrap() {
            row.as_object_mut().unwrap().remove("parent");
            for cell in row["cells"].as_array_mut().unwrap() {
                cell.as_object_mut().unwrap().remove("category");
            }
        }
        arrange(&regrouped, &mut rearranged).unwrap();
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
        let sheet = json!({"groups":groups});
        arrange(&sheet, &mut metadata).unwrap();
        assert_eq!(
            metadata[0]["categories"],
            json!({"cell_r5_B":{"parent":"record_r4"}})
        );
        let (body, stored) = crate::document_body::split(&metadata).unwrap();
        assert_eq!(
            body[0]["table"]["rows"],
            json!([{"A":"＜本番＞","children":[{"B":"AP01","children":[{"C":"/opt"},{"C":"/var"}]}]}])
        );
        let regrouped = json!({"groups":super::groups(&stored).unwrap()});
        let mut rearranged = metadata.clone();
        rearranged[0].as_object_mut().unwrap().remove("categories");
        for row in rearranged[0]["rows"].as_array_mut().unwrap() {
            row.as_object_mut().unwrap().remove("parent");
        }
        arrange(&regrouped, &mut rearranged).unwrap();
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
}
