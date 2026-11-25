//! The structure a tagged PDF gives its pages: which strings are one
//! paragraph, heading, list item or table cell, and the rows and cells of its
//! tables.
//!
//! A tagged PDF marks the content of its pages with numbers (MCIDs) and
//! names, in its structure tree, the element each belongs to. What a page
//! draws for one paragraph is then known however many lines and strings it
//! takes, and a table's rows and cells are known without rulings. The tree
//! is read for that alone: where the elements stand on the page is still
//! read from the page.
//!
//! A table is taken from the tree when no table is nested in it. A
//! workbook's sheet, which Excel tags as one table of all its cells, and a
//! table holding another are left to the rulings of the page, which show
//! the tables as they are drawn; their cells' paragraphs are still known
//! from the tree.
use super::page_layout::{Marked, MarkedCell};
use lopdf::{Dictionary, Document as Pdf, Object, ObjectId};
use std::collections::{BTreeMap, BTreeSet};

/// How deep the structure tree is read.
const MAX_DEPTH: usize = 128;
/// The most elements and content items of a structure tree that are read.
const MAX_NODES: usize = 4_000_000;

/// What the structure tree says of a page.
#[derive(Default)]
pub(super) struct PageTags {
    /// The unit (paragraph, heading, list item, cell content) of the
    /// content each MCID of the page marks.
    pub units: BTreeMap<i64, usize>,
    /// The tables of the page, their cells naming units.
    pub tables: Vec<Marked>,
    /// The units made so far.
    count: usize,
}

/// What the structure tree says of each page of a document.
#[derive(Default)]
pub(super) struct Tags {
    pages: BTreeMap<ObjectId, PageTags>,
}

impl Tags {
    pub fn page(&self, id: ObjectId) -> Option<&PageTags> {
        self.pages.get(&id)
    }
}

/// How an element takes part in the units of a page.
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    /// Part of the text of the element it is in: a span, a link, the label
    /// and body of a list item.
    Inline,
    /// A unit of its own: a paragraph, a heading, anything else.
    Block,
    Item,
    Table,
    /// The head, body or foot of a table.
    Rows,
    Row,
    Cell,
}

fn kind(role: &[u8]) -> Kind {
    match role {
        b"Span" | b"Quote" | b"Reference" | b"BibEntry" | b"Code" | b"Link" | b"Annot"
        | b"Ruby" | b"RB" | b"RT" | b"RP" | b"Warichu" | b"WT" | b"WP" | b"Em" | b"Strong"
        | b"Sub" | b"Lbl" | b"LBody" | b"NonStruct" => Kind::Inline,
        b"LI" => Kind::Item,
        b"Table" => Kind::Table,
        b"THead" | b"TBody" | b"TFoot" => Kind::Rows,
        b"TR" => Kind::Row,
        b"TH" | b"TD" => Kind::Cell,
        _ => Kind::Block,
    }
}

/// The unit that content met in an element is of: the element that owns it
/// (by its place in memory), and whether it is a list item, whose paragraphs
/// are its own text.
#[derive(Clone, Copy)]
struct Owner {
    element: usize,
    item: bool,
}

/// A cell of a table being read: the units of its content by page, and the
/// columns and rows it spans.
struct Cell {
    units: Vec<(ObjectId, usize)>,
    columns: usize,
    rows: usize,
    heading: bool,
}

/// A row of a table being read: its cells, and whether it is of the
/// table's head.
type Row = (Vec<Cell>, bool);

/// A table being read: its rows, and whether it can be taken from the
/// tree.
struct Table {
    rows: Vec<Row>,
    usable: bool,
}

struct Reader<'a> {
    doc: &'a Pdf,
    roles: Option<&'a Dictionary>,
    tags: Tags,
    /// The unit of each element on each page.
    units: BTreeMap<(usize, ObjectId), usize>,
    /// The tables being read, the innermost last.
    open: Vec<Table>,
    /// The elements being read, which may not hold themselves.
    path: BTreeSet<ObjectId>,
    nodes: usize,
}

/// The structure of the pages of `doc`: none of a document that is not
/// tagged, or whose tags its writer marks as doubtful.
pub(super) fn read(doc: &Pdf) -> Tags {
    fn dictionary<'a>(doc: &'a Pdf, object: &'a Object) -> Option<&'a Dictionary> {
        match object {
            Object::Reference(id) => doc.get_dictionary(*id).ok(),
            object => object.as_dict().ok(),
        }
    }
    let dictionary = |object| dictionary(doc, object);
    let Ok(catalog) = doc.catalog() else {
        return Tags::default();
    };
    let flag = |key: &[u8]| {
        catalog
            .get(b"MarkInfo")
            .ok()
            .and_then(dictionary)
            .and_then(|info| info.get(key).ok())
            .and_then(|flag| flag.as_bool().ok())
            .unwrap_or(false)
    };
    let Some(root) = catalog.get(b"StructTreeRoot").ok().and_then(dictionary) else {
        return Tags::default();
    };
    if !flag(b"Marked") || flag(b"Suspects") {
        return Tags::default();
    }
    let mut reader = Reader {
        doc,
        roles: root.get(b"RoleMap").ok().and_then(dictionary),
        tags: Tags::default(),
        units: BTreeMap::new(),
        open: vec![],
        path: BTreeSet::new(),
        nodes: 0,
    };
    if let Ok(kids) = root.get(b"K") {
        let top = Owner {
            element: 0,
            item: false,
        };
        reader.kids(kids, None, top, false, false, 0);
    }
    reader.tags
}

impl<'a> Reader<'a> {
    /// The standard type an element of type `name` is, as the role map
    /// says.
    fn role(&self, name: &'a [u8]) -> &'a [u8] {
        let mut name = name;
        for _ in 0..8 {
            match self
                .roles
                .and_then(|roles| roles.get(name).ok())
                .and_then(|mapped| mapped.as_name().ok())
            {
                Some(mapped) if mapped != name => name = mapped,
                _ => break,
            }
        }
        name
    }

    /// The content `mcid` of `page` is of the unit of `owner`.
    fn content(&mut self, mcid: i64, page: Option<ObjectId>, owner: Owner) {
        let Some(page) = page else { return };
        let tags = self.tags.pages.entry(page).or_default();
        let unit = *self.units.entry((owner.element, page)).or_insert_with(|| {
            tags.count += 1;
            tags.count - 1
        });
        tags.units.entry(mcid).or_insert(unit);
        if let Some(cell) = self
            .open
            .last_mut()
            .and_then(|table| table.rows.last_mut())
            .and_then(|(cells, _)| cells.last_mut())
            && !cell.units.contains(&(page, unit))
        {
            cell.units.push((page, unit));
        }
    }

    /// Reads the kids `object` of an element on `page`: content of `owner`,
    /// and elements. `sheet` says they are in a workbook's sheet, `head` in
    /// the head of a table.
    fn kids(
        &mut self,
        object: &'a Object,
        page: Option<ObjectId>,
        owner: Owner,
        sheet: bool,
        head: bool,
        depth: usize,
    ) {
        self.nodes += 1;
        if depth > MAX_DEPTH || self.nodes > MAX_NODES {
            return;
        }
        let (id, object) = match object {
            Object::Reference(id) => match self.doc.get_object(*id) {
                Ok(object) => (Some(*id), object),
                Err(_) => return,
            },
            object => (None, object),
        };
        match object {
            Object::Array(kids) => {
                for kid in kids {
                    self.kids(kid, page, owner, sheet, head, depth + 1);
                }
            }
            Object::Integer(mcid) => self.content(*mcid, page, owner),
            Object::Dictionary(element) => {
                if let Some(id) = id
                    && !self.path.insert(id)
                {
                    return;
                }
                self.element(element, page, owner, sheet, head, depth);
                if let Some(id) = id {
                    self.path.remove(&id);
                }
            }
            _ => {}
        }
    }

    fn element(
        &mut self,
        element: &'a Dictionary,
        page: Option<ObjectId>,
        owner: Owner,
        sheet: bool,
        head: bool,
        depth: usize,
    ) {
        let page = element
            .get(b"Pg")
            .ok()
            .and_then(|page| page.as_reference().ok())
            .or(page);
        let named = |key: &[u8]| element.get(key).ok().and_then(|name| name.as_name().ok());
        match named(b"Type") {
            // Content of a page; that of a form drawn on it is not read.
            Some(b"MCR") => {
                if element.get(b"Stm").is_err()
                    && let Some(mcid) = element.get(b"MCID").ok().and_then(|n| n.as_i64().ok())
                {
                    self.content(mcid, page, owner);
                }
                return;
            }
            Some(b"OBJR") => return,
            _ => {}
        }
        let Some(name) = named(b"S") else { return };
        let Ok(kids) = element.get(b"K") else { return };
        let own = Owner {
            element: std::ptr::from_ref(element) as usize,
            item: false,
        };
        let sheet = sheet || name == b"Worksheet";
        match kind(self.role(name)) {
            Kind::Inline => self.kids(kids, page, owner, sheet, head, depth + 1),
            // The paragraphs of a list item are its text.
            Kind::Block if owner.item && self.role(name) == b"P" => {
                self.kids(kids, page, owner, sheet, head, depth + 1);
            }
            Kind::Block => self.kids(kids, page, own, sheet, head, depth + 1),
            Kind::Item => {
                let item = Owner { item: true, ..own };
                self.kids(kids, page, item, sheet, head, depth + 1);
            }
            Kind::Rows => {
                let head = head || self.role(name) == b"THead";
                self.kids(kids, page, own, sheet, head, depth + 1);
            }
            Kind::Row => {
                if let Some(table) = self.open.last_mut() {
                    table.rows.push((vec![], head));
                }
                self.kids(kids, page, own, sheet, head, depth + 1);
            }
            Kind::Cell => {
                let span = |key: &[u8]| {
                    self.attribute(element, key)
                        .filter(|n| (1..=10_000).contains(n))
                        .unwrap_or(1) as usize
                };
                let cell = Cell {
                    units: vec![],
                    columns: span(b"ColSpan"),
                    rows: span(b"RowSpan"),
                    heading: self.role(name) == b"TH",
                };
                if let Some((cells, _)) =
                    self.open.last_mut().and_then(|table| table.rows.last_mut())
                {
                    cells.push(cell);
                }
                self.kids(kids, page, own, sheet, head, depth + 1);
            }
            Kind::Table => {
                // A table in a table: neither is taken from the tree.
                let nested = !self.open.is_empty();
                for table in &mut self.open {
                    table.usable = false;
                }
                self.open.push(Table {
                    rows: vec![],
                    usable: !sheet && !nested,
                });
                self.kids(kids, page, own, sheet, false, depth + 1);
                let table = self.open.pop().expect("the table opened");
                if table.usable {
                    self.table(table);
                }
            }
        }
    }

    /// The whole number an attribute `key` of `element` gives, in any of
    /// its attribute objects.
    fn attribute(&self, element: &'a Dictionary, key: &[u8]) -> Option<i64> {
        let resolved = |object: &'a Object| match object {
            Object::Reference(id) => self.doc.get_object(*id).ok(),
            object => Some(object),
        };
        let of = |object: &'a Object| {
            let dictionary = match resolved(object)? {
                Object::Dictionary(dictionary) => dictionary,
                Object::Stream(stream) => &stream.dict,
                _ => return None,
            };
            let value = resolved(dictionary.get(key).ok()?)?;
            value
                .as_i64()
                .ok()
                .or_else(|| value.as_float().ok().map(|n| n as i64))
        };
        match resolved(element.get(b"A").ok()?)? {
            Object::Array(list) => list.iter().find_map(of),
            attributes => of(attributes),
        }
    }

    /// Records a table read from the tree on each page its rows are on. A
    /// row is of the page its first content is on, a row without content of
    /// the page of the row before it.
    fn table(&mut self, table: Table) {
        let mut by_page: Vec<(ObjectId, Vec<Row>)> = vec![];
        for (cells, head) in table.rows {
            let page = cells
                .iter()
                .find_map(|cell| cell.units.first().map(|(page, _)| *page))
                .or(by_page.last().map(|(page, _)| *page));
            let Some(page) = page else { continue };
            match by_page.last_mut() {
                Some((last, rows)) if *last == page => rows.push((cells, head)),
                _ => by_page.push((page, vec![(cells, head)])),
            }
        }
        for (page, rows) in by_page {
            let width = rows
                .iter()
                .map(|(cells, _)| cells.iter().map(|cell| cell.columns).sum::<usize>())
                .max()
                .unwrap_or(0);
            if width == 0 || (width < 2 && rows.len() < 2) {
                continue;
            }
            let leading = |of: &dyn Fn(&Row) -> bool| {
                let count = rows.iter().take_while(|row| of(row)).count();
                (count < rows.len())
                    .then_some(count)
                    .filter(|count| *count > 0)
            };
            let header_rows = leading(&|(_, head)| *head)
                .or_else(|| {
                    leading(&|(cells, _)| !cells.is_empty() && cells.iter().all(|c| c.heading))
                })
                .unwrap_or(0);
            let rows = rows
                .into_iter()
                .map(|(cells, _)| {
                    cells
                        .into_iter()
                        .map(|cell| MarkedCell {
                            units: cell
                                .units
                                .into_iter()
                                .filter(|(of, _)| *of == page)
                                .map(|(_, unit)| unit)
                                .collect(),
                            columns: cell.columns,
                            rows: cell.rows,
                        })
                        .collect()
                })
                .collect();
            self.tags
                .pages
                .entry(page)
                .or_default()
                .tables
                .push(Marked { rows, header_rows });
        }
    }
}
