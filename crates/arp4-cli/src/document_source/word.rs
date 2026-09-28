//! Word text laid out on a grid. Paragraphs outside tables take one row each in
//! column A; a table keeps its rows and grid columns, reports merged cells as
//! merges and its range as an explicit table. A block's text joins the text of
//! its paragraphs, and `segments` map that text back to the `w:t` elements so
//! that an edit changes only the runs it touches.
use super::*;
use crate::excel::{OperationKind, StructuralOperation, column_name};
use std::ops::Range;

/// The text of rows operations insert: (operation ID, offset, column) to text.
pub(super) type InsertedText = BTreeMap<(String, u32, String), String>;

/// A row after row operations: an original row, or one an operation inserted
/// that copies the paragraph or table row of the original row `template`.
pub(super) enum Placed {
    Original(usize),
    Inserted {
        operation: String,
        offset: u32,
        template: usize,
    },
}

/// Attributes a copied element must not repeat: paragraph IDs are unique and
/// revision IDs name the editing session.
fn fresh_opening(raw: &str) -> Result<String> {
    static IDENTITY: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r#"\s+(?:w14:paraId|w14:textId|w:rsid\w*)\s*=\s*"[^"]*""#).unwrap()
    });
    let end = raw.find('>').context("invalid element")?;
    Ok(IDENTITY
        .replace_all(raw[..end].trim_end_matches('/'), "")
        .into_owned())
}

/// The qualified name of an element and its namespace prefix with the colon.
fn element_names(raw: &str) -> Result<(&str, String)> {
    let name = raw[1..]
        .split(|c: char| c.is_whitespace() || c == '/' || c == '>')
        .next()
        .context("invalid element")?;
    Ok((name, super::element_prefix(name)))
}

/// A paragraph like `template` (its properties, less a section break, and the
/// format of its first run) holding `text`.
fn new_paragraph(original: &str, template: Node<'_, '_>, text: &str) -> Result<String> {
    let raw = &original[template.range()];
    let (name, prefix) = element_names(raw)?;
    let mut xml = format!("{}>", fresh_opening(raw)?);
    if let Some(properties) = template.children().find(|n| word_element(*n, "pPr")) {
        let mut properties_xml = original[properties.range()].to_owned();
        if let Some(section) = properties.children().find(|n| word_element(*n, "sectPr")) {
            let base = properties.range().start;
            properties_xml
                .replace_range(section.range().start - base..section.range().end - base, "");
        }
        xml.push_str(&properties_xml);
    }
    if !text.is_empty() {
        let format = template
            .descendants()
            .find(|n| word_element(*n, "r") && nearest(*n, "p") == Some(template))
            .and_then(|run| run.children().find(|n| word_element(*n, "rPr")))
            .map_or("", |properties| &original[properties.range()]);
        let content = super::word_run_content(
            text,
            &format!("<{prefix}t xml:space=\"preserve\""),
            &format!("{prefix}t"),
            &prefix,
        )?;
        xml.push_str(&format!("<{prefix}r>{format}{content}</{prefix}r>"));
    }
    xml.push_str(&format!("</{name}>"));
    Ok(xml)
}

/// A table row like `template` (its row and cell properties, and the format
/// of each cell's first paragraph) whose cells hold `text` by grid column.
fn new_table_row(
    original: &str,
    template: Node<'_, '_>,
    row: usize,
    text: impl Fn(&str) -> Option<String>,
) -> Result<String> {
    let raw = &original[template.range()];
    let (name, _) = element_names(raw)?;
    let mut xml = format!("{}>", fresh_opening(raw)?);
    for properties in template
        .children()
        .filter(|n| word_element(*n, "tblPrEx") || word_element(*n, "trPr"))
    {
        xml.push_str(&original[properties.range()]);
    }
    let mut column = number(property(template, "trPr", "gridBefore"), 0)?;
    for cell in template
        .descendants()
        .filter(|n| word_element(*n, "tc") && nearest(*n, "tr") == Some(template))
    {
        ensure!(
            property(cell, "tcPr", "vMerge").is_none(),
            "row {row} is part of a vertically merged cell, so it cannot be copied; insert the row in Word"
        );
        let cell_raw = &original[cell.range()];
        let (cell_name, _) = element_names(cell_raw)?;
        xml.push_str(&format!("{}>", fresh_opening(cell_raw)?));
        if let Some(properties) = cell.children().find(|n| word_element(*n, "tcPr")) {
            xml.push_str(&original[properties.range()]);
        }
        let paragraph = cell
            .children()
            .find(|n| word_element(*n, "p"))
            .with_context(|| format!("a cell of row {row} has no paragraph to copy"))?;
        let letter = column_name(column as u32 + 1)?;
        xml.push_str(&new_paragraph(
            original,
            paragraph,
            &text(&letter).unwrap_or_default(),
        )?);
        xml.push_str(&format!("</{cell_name}>"));
        column += number(property(cell, "tcPr", "gridSpan"), 1)?.max(1);
    }
    xml.push_str(&format!("</{name}>"));
    Ok(xml)
}

/// Whether row operations can change `node`: text box paragraphs have a copy
/// for older readers, and a section break ends a section.
fn ensure_row_editable(node: Node<'_, '_>, row: usize) -> Result<()> {
    ensure!(
        !node
            .ancestors()
            .any(|a| word_element(a, "txbxContent") || alternate_branch(a)),
        "row {row} is in a text box; insert or delete its paragraphs in Word"
    );
    Ok(())
}

/// The rows after `operations` (row insertions and deletions, in order) of a
/// document whose rows are `rows`. A new row copies `style_from`, else the row
/// above it.
pub(super) fn placed_rows(
    rows: &BTreeMap<usize, Node<'_, '_>>,
    operations: &[&StructuralOperation],
    application: &str,
) -> Result<Vec<Placed>> {
    let last = rows.keys().max().copied().unwrap_or(0);
    let mut placed: Vec<Placed> = (1..=last).map(Placed::Original).collect();
    for operation in operations {
        let (at, count) = (operation.at as usize, operation.count as usize);
        match operation.kind {
            OperationKind::InsertRows => {
                ensure!(
                    (1..=placed.len() + 1).contains(&at),
                    "row {at} is past the end of the document"
                );
                let template = match operation.style_from {
                    Some(row) => row as usize,
                    None => match placed
                        .get(at.saturating_sub(2))
                        .context("the document has no row to copy")?
                    {
                        Placed::Original(row) => *row,
                        Placed::Inserted { template, .. } => *template,
                    },
                };
                ensure!(
                    rows.contains_key(&template),
                    "row {template} has no paragraph or table row to copy"
                );
                placed.splice(
                    at - 1..at - 1,
                    (0..count).map(|offset| Placed::Inserted {
                        operation: operation.id.clone(),
                        offset: offset as u32,
                        template,
                    }),
                );
            }
            OperationKind::DeleteRows => {
                ensure!(
                    at >= 1 && at - 1 + count <= placed.len(),
                    "rows {at} to {} are past the end of the document",
                    at + count - 1
                );
                placed.drain(at - 1..at - 1 + count);
            }
            _ => bail!(
                "{application} documents take row insertions and deletions only; table columns are laid out by {application}, so change them in {application}"
            ),
        }
    }
    Ok(placed)
}

/// The closest original row on either side of each placed row. Inserted rows
/// do not become anchors for later inserted rows.
pub(super) fn original_neighbors(placed: &[Placed]) -> (Vec<Option<usize>>, Vec<Option<usize>>) {
    let mut before = Vec::with_capacity(placed.len());
    let mut previous = None;
    for entry in placed {
        before.push(previous);
        if let Placed::Original(row) = entry {
            previous = Some(*row);
        }
    }
    let mut after = vec![None; placed.len()];
    let mut next = None;
    for (index, entry) in placed.iter().enumerate().rev() {
        after[index] = next;
        if let Placed::Original(row) = entry {
            next = Some(*row);
        }
    }
    (before, after)
}

impl Layout<'_, '_> {
    /// Edits that insert and delete paragraphs and table rows as `operations`
    /// (the row insertions and deletions of this part, in order) do. A new row
    /// copies the paragraph or table row it takes its format from (the row
    /// above it, or `style_from`), with the text `values` gives it; a table
    /// whose rows are all deleted is deleted.
    pub(super) fn restructure(
        &self,
        original: &str,
        operations: &[&StructuralOperation],
        values: &InsertedText,
    ) -> Result<Vec<(Range<usize>, String)>> {
        if operations.is_empty() {
            return Ok(vec![]);
        }
        let placed = placed_rows(&self.rows, operations, "Word")?;
        let (before_rows, after_rows) = original_neighbors(&placed);
        let mut insertions: BTreeMap<usize, String> = BTreeMap::new();
        for (index, entry) in placed.iter().enumerate() {
            let Placed::Inserted {
                operation,
                offset,
                template,
            } = entry
            else {
                continue;
            };
            let node = self.rows[template];
            ensure_row_editable(node, *template)?;
            let table_row = word_element(node, "tr");
            let text = |column: &str| {
                values
                    .get(&(operation.clone(), *offset, column.to_owned()))
                    .cloned()
            };
            let xml = if table_row {
                new_table_row(original, node, *template, text)?
            } else {
                new_paragraph(original, node, &text("A").unwrap_or_default())?
            };
            // Next to the nearest original row that can hold it: a paragraph goes
            // after a paragraph or a table, a table row next to a row of its table.
            let before = before_rows[index].map(|row| self.rows[&row]);
            let after = after_rows[index].map(|row| self.rows[&row]);
            let table = nearest(node, "tbl");
            let position = if table_row {
                before
                    .filter(|n| word_element(*n, "tr") && nearest(*n, "tbl") == table)
                    .map(|n| n.range().end)
                    .or_else(|| {
                        after
                            .filter(|n| word_element(*n, "tr") && nearest(*n, "tbl") == table)
                            .map(|n| n.range().start)
                    })
            } else {
                before
                    .map(|n| match nearest(n, "tbl").filter(|_| word_element(n, "tr")) {
                        Some(table) => table.range().end,
                        None => n.range().end,
                    })
                    .or_else(|| {
                        after
                            .filter(|n| word_element(*n, "p"))
                            .map(|n| n.range().start)
                    })
            }
            .with_context(|| {
                format!(
                    "a {} copied from row {template} has no place next to the rows around it; choose --style-from a row of the same kind",
                    if table_row { "table row" } else { "paragraph" }
                )
            })?;
            insertions.entry(position).or_default().push_str(&xml);
        }
        let kept: BTreeSet<usize> = placed
            .iter()
            .filter_map(|p| match p {
                Placed::Original(row) => Some(*row),
                Placed::Inserted { .. } => None,
            })
            .collect();
        let kept_tables: BTreeSet<usize> = kept
            .iter()
            .filter_map(|row| {
                let node = self.rows[row];
                word_element(node, "tr")
                    .then(|| nearest(node, "tbl").map(|table| table.range().start))
                    .flatten()
            })
            .collect();
        let mut edits: Vec<(Range<usize>, String)> = insertions
            .into_iter()
            .map(|(position, xml)| (position..position, xml))
            .collect();
        let mut removed_tables = BTreeSet::new();
        for (row, node) in &self.rows {
            if kept.contains(row) {
                continue;
            }
            ensure_row_editable(*node, *row)?;
            if word_element(*node, "tr") {
                ensure!(
                    node.descendants()
                        .filter(|n| word_element(*n, "tc") && nearest(*n, "tr") == Some(*node))
                        .all(|cell| property(cell, "tcPr", "vMerge").is_none()),
                    "row {row} is part of a vertically merged cell; delete it in Word"
                );
                let table = nearest(*node, "tbl").context("table row outside a table")?;
                let whole = !kept_tables.contains(&table.range().start);
                if whole {
                    if removed_tables.insert(table.range().start) {
                        edits.push((table.range(), String::new()));
                    }
                    continue;
                }
            } else {
                ensure!(
                    property(*node, "pPr", "sectPr").is_none(),
                    "row {row} ends a section of the document; delete it in Word"
                );
            }
            edits.push((node.range(), String::new()));
        }
        Ok(edits)
    }
}

pub(super) struct Segment<'a, 'input> {
    /// The text element or the line break or tab of a run ([`text_break`]),
    /// or None for a paragraph break or a character Word draws itself.
    pub node: Option<Node<'a, 'input>>,
    pub range: Range<usize>,
}

pub(super) struct Block<'a, 'input> {
    pub address: String,
    pub text: String,
    pub segments: Vec<Segment<'a, 'input>>,
    /// The application that lays the text out, named in messages.
    application: &'static str,
    /// Whether an edit can write new line breaks, as Word and PowerPoint
    /// paragraphs hold them.
    breaks: bool,
    bold: bool,
    fill: bool,
    in_table: bool,
}

impl<'a, 'input> Block<'a, 'input> {
    /// Text laid out by `application` outside Word, such as an Excel shape's.
    /// Its line and paragraph breaks are segments without an element, so an
    /// edit cannot add or remove them.
    pub(super) fn plain(
        address: String,
        text: String,
        segments: Vec<Segment<'a, 'input>>,
        application: &'static str,
    ) -> Self {
        Self {
            address,
            text,
            segments,
            application,
            breaks: false,
            bold: false,
            fill: false,
            in_table: false,
        }
    }

    /// PowerPoint text: a paragraph, or the paragraphs of a table cell, whose
    /// line breaks and tabs an edit can change.
    pub(super) fn slide(
        address: String,
        text: String,
        segments: Vec<Segment<'a, 'input>>,
        bold: bool,
        in_table: bool,
    ) -> Self {
        Self {
            address,
            text,
            segments,
            application: "PowerPoint",
            breaks: true,
            bold,
            fill: false,
            in_table,
        }
    }
}

/// The extraction sheet of text laid out as `blocks`.
pub(super) fn blocks_sheet(
    name: &str,
    part: &str,
    blocks: &[Block<'_, '_>],
    merges: &[String],
    tables: &[Value],
) -> Value {
    let cells: Vec<_> = blocks
        .iter()
        .enumerate()
        .map(|(i, b)| {
            json!({"id":format!("text-{}",i+1),"address":b.address,"type":"string","value":b.text,
                "formula":null,"cached":null,"number_format":"",
                "style":{"bold":b.bold,"fill":u8::from(b.fill),"border":u8::from(b.in_table)}})
        })
        .collect();
    json!({"name":name,"part":part,"state":"visible","merges":merges,"cells":cells,"tables":tables})
}

pub(super) struct Layout<'a, 'input> {
    pub blocks: Vec<Block<'a, 'input>>,
    merges: Vec<String>,
    tables: Vec<Value>,
    /// The element of each row: a paragraph or a table row.
    rows: BTreeMap<usize, Node<'a, 'input>>,
}

impl Layout<'_, '_> {
    pub fn sheet(&self, name: &str, part: &str) -> Value {
        blocks_sheet(name, part, &self.blocks, &self.merges, &self.tables)
    }
}

fn word_element(node: Node<'_, '_>, name: &str) -> bool {
    node.has_tag_name((WORD, name))
}

fn nearest<'a, 'input>(node: Node<'a, 'input>, name: &str) -> Option<Node<'a, 'input>> {
    node.ancestors().skip(1).find(|n| word_element(*n, name))
}

fn property<'a, 'input>(
    node: Node<'a, 'input>,
    properties: &str,
    name: &str,
) -> Option<Node<'a, 'input>> {
    node.children()
        .find(|n| word_element(*n, properties))?
        .children()
        .find(|n| word_element(*n, name))
}

fn value<'a>(node: Node<'a, '_>) -> Option<&'a str> {
    node.attribute((WORD, "val"))
}

/// Whether Word shows `node` as text of its paragraph. The reading of ruby
/// (furigana) sits above its base text, text deleted or moved away under
/// tracked changes is gone from the document, hidden text is not shown, and a
/// content control showing its placeholder ("click or tap here to enter text")
/// holds a prompt rather than content.
fn shown(node: Node<'_, '_>) -> bool {
    !node.ancestors().any(|a| {
        word_element(a, "rt")
            || word_element(a, "del")
            || word_element(a, "moveFrom")
            || (word_element(a, "sdt") && property(a, "sdtPr", "showingPlcHdr").is_some())
    }) && !node
        .ancestors()
        .find(|a| word_element(*a, "r"))
        .is_some_and(|run| on(property(run, "rPr", "vanish")))
}

/// Elements inside a field's instructions, such as the result of the inner
/// field in `{ IF { MERGEFIELD 性別 } = "男" ... }`: Word shows only the outer
/// field's result. Keyed by their start in the part.
fn field_instructions(xml: &Document<'_>, hidden: &HiddenBranches) -> BTreeSet<usize> {
    let mut open: Vec<bool> = vec![];
    let mut inside = BTreeSet::new();
    for node in xml.descendants().filter(|n| !hidden.contains(*n)) {
        if word_element(node, "fldChar") {
            match node.attribute((WORD, "fldCharType")) {
                Some("begin") => open.push(false),
                Some("separate") => {
                    if let Some(separated) = open.last_mut() {
                        *separated = true;
                    }
                }
                Some("end") => {
                    open.pop();
                }
                _ => {}
            }
        } else if node.is_element() && open.iter().any(|separated| !separated) {
            inside.insert(node.range().start);
        }
    }
    inside
}

/// Field codes are recorded for inspection, never treated as body text or
/// edited through the document content mapping.
pub(super) fn extract_field_codes(xml: &Document<'_>, part: &str) -> Vec<Value> {
    let hidden = HiddenBranches::new(xml, "docx");
    let mut result = Vec::new();
    let mut open: Vec<(String, bool)> = Vec::new();
    for node in xml.descendants().filter(|n| !hidden.contains(*n)) {
        if word_element(node, "fldSimple") {
            if let Some(instruction) = node.attribute((WORD, "instr")) {
                result.push(json!({"part":part,"instruction":instruction}));
            }
        } else if word_element(node, "fldChar") {
            match node.attribute((WORD, "fldCharType")) {
                Some("begin") => open.push((String::new(), false)),
                Some("separate") => {
                    if let Some((_, separated)) = open.last_mut() {
                        *separated = true;
                    }
                }
                Some("end") => {
                    if let Some((instruction, _)) = open.pop() {
                        if !instruction.trim().is_empty() {
                            result.push(json!({"part":part,"instruction":instruction}));
                        }
                    }
                }
                _ => {}
            }
        } else if word_element(node, "instrText") {
            if let Some((instruction, false)) = open.last_mut() {
                instruction.push_str(node.text().unwrap_or(""));
            }
        }
    }
    result
}

/// The content control bound to XML data (a cover page's title or author, for
/// example) that shows `node`; Word refills it from that data on opening.
pub(super) fn bound_control<'a, 'input>(node: Node<'a, 'input>) -> Option<Node<'a, 'input>> {
    node.ancestors()
        .filter(|a| word_element(*a, "sdt"))
        .find(|sdt| property(*sdt, "sdtPr", "dataBinding").is_some())
}

/// The store item and XPath of the data a content control is bound to.
pub(super) fn binding_key(sdt: Node<'_, '_>) -> Option<(String, String)> {
    let binding = property(sdt, "sdtPr", "dataBinding")?;
    Some((
        binding
            .attribute((WORD, "storeItemID"))
            .unwrap_or("")
            .to_ascii_uppercase(),
        binding.attribute((WORD, "xpath"))?.to_owned(),
    ))
}

/// Where a content control's data lives: the store item (a custom XML part or
/// the document properties), the XPath of its element and the namespace
/// prefixes the XPath uses.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Binding {
    pub store: String,
    pub xpath: String,
    pub prefixes: String,
    pub multiline: bool,
}

/// The binding of a plain text content control. Date, list and picture
/// controls show their data converted or chosen from a list, so an edit to
/// the text they show cannot be written back to the data.
pub(super) fn binding(sdt: Node<'_, '_>, place: &str) -> Result<Binding> {
    let binding =
        property(sdt, "sdtPr", "dataBinding").context("content control without binding")?;
    let text = property(sdt, "sdtPr", "text").with_context(|| {
        format!("{place} is a date, list, picture or other content control bound to document data, whose shown text differs from the data; edit it in Word")
    })?;
    Ok(Binding {
        store: binding
            .attribute((WORD, "storeItemID"))
            .unwrap_or("")
            .to_ascii_uppercase(),
        xpath: binding
            .attribute((WORD, "xpath"))
            .context("content control binding without xpath")?
            .to_owned(),
        prefixes: binding
            .attribute((WORD, "prefixMappings"))
            .unwrap_or("")
            .to_owned(),
        multiline: matches!(
            text.attribute((WORD, "multiLine")),
            Some("1" | "true" | "on")
        ),
    })
}

/// The text elements, tabs and line breaks a content control shows.
pub(super) fn control_nodes<'a, 'input>(sdt: Node<'a, 'input>) -> Vec<Node<'a, 'input>> {
    let Some(content) = sdt.children().find(|n| word_element(*n, "sdtContent")) else {
        return vec![];
    };
    content
        .descendants()
        .filter(|n| {
            (word_element(*n, "t") || text_break(*n)) && shown(*n) && !hidden_copy(*n, "docx")
        })
        .collect()
}

/// The text a content control shows once `edits` are made.
pub(super) fn control_text(sdt: Node<'_, '_>, edited: &BTreeMap<usize, &str>) -> String {
    control_nodes(sdt)
        .into_iter()
        .map(|node| match edited.get(&node.range().start) {
            Some(new) => (*new).to_owned(),
            None if word_element(node, "t") => node.text().unwrap_or("").to_owned(),
            None if word_element(node, "tab") => "\t".to_owned(),
            None => "\n".to_owned(),
        })
        .collect()
}

/// Edits that make a content control show `value`: its first text element
/// takes it and the other text, tabs and breaks are cleared.
pub(super) fn fill_control<'a, 'input>(
    sdt: Node<'a, 'input>,
    value: &str,
    place: &str,
) -> Result<Vec<(Node<'a, 'input>, String)>> {
    let nodes = control_nodes(sdt);
    let first = nodes
        .iter()
        .position(|n| word_element(*n, "t"))
        .with_context(|| {
            format!(
                "{place} shows the same document data but has no text to replace; edit it in Word"
            )
        })?;
    Ok(nodes
        .into_iter()
        .enumerate()
        .map(|(i, node)| {
            (
                node,
                if i == first {
                    value.to_owned()
                } else {
                    String::new()
                },
            )
        })
        .collect())
}

/// A tab or line break of a run, which an edit can remove or replace. Page and
/// column breaks lay out pages; they are not text.
pub(super) fn text_break(node: Node<'_, '_>) -> bool {
    node.parent().is_some_and(|p| word_element(p, "r"))
        && (word_element(node, "tab")
            || word_element(node, "cr")
            || word_element(node, "br")
                && !matches!(node.attribute((WORD, "type")), Some("page" | "column")))
}

/// On/off properties are on unless their value says otherwise.
fn on(node: Option<Node<'_, '_>>) -> bool {
    node.is_some_and(|n| !matches!(value(n), Some("0" | "false" | "off")))
}

fn number(node: Option<Node<'_, '_>>, default: usize) -> Result<usize> {
    node.and_then(value)
        .map(|v| v.parse().context("invalid Word table grid value"))
        .transpose()
        .map(|v| v.unwrap_or(default))
}

struct Text<'a, 'input, 'f> {
    text: String,
    segments: Vec<Segment<'a, 'input>>,
    runs: usize,
    bold_runs: usize,
    /// See [`field_instructions`].
    instructions: &'f BTreeSet<usize>,
    hidden: &'f HiddenBranches,
}

impl<'a, 'input, 'f> Text<'a, 'input, 'f> {
    fn new(instructions: &'f BTreeSet<usize>, hidden: &'f HiddenBranches) -> Self {
        Self {
            text: String::new(),
            segments: vec![],
            runs: 0,
            bold_runs: 0,
            instructions,
            hidden,
        }
    }

    fn push(&mut self, node: Option<Node<'a, 'input>>, text: &str) {
        let start = self.text.len();
        self.text.push_str(text);
        self.segments.push(Segment {
            node,
            range: start..self.text.len(),
        });
    }

    /// Appends the visible text of one paragraph; nested paragraphs (text boxes)
    /// are blocks of their own.
    fn paragraph(&mut self, paragraph: Node<'a, 'input>) {
        let mark = (self.text.len(), self.segments.len());
        if !self.text.is_empty() {
            self.push(None, "\n");
        }
        let before = self.text.len();
        for node in paragraph.descendants().filter(|n| {
            nearest(*n, "p") == Some(paragraph)
                && !self.hidden.contains(*n)
                && shown(*n)
                && !self.instructions.contains(&n.range().start)
        }) {
            let in_run = node.parent().is_some_and(|p| word_element(p, "r"));
            // Characters Word draws itself are shown but cannot be edited as text.
            if in_run && word_element(node, "noBreakHyphen") {
                self.push(None, "\u{2011}");
            } else if in_run && word_element(node, "sym") {
                if let Some(symbol) = node
                    .attribute((WORD, "char"))
                    .and_then(|code| u32::from_str_radix(code, 16).ok())
                    .and_then(char::from_u32)
                {
                    self.push(None, &symbol.to_string());
                }
            } else if in_run && word_element(node, "ptab") {
                self.push(None, "\t");
            } else if node.has_tag_name((MATH, "t")) {
                self.push(None, node.text().unwrap_or(""));
            } else if word_element(node, "t") {
                let text = node.text().unwrap_or("");
                if !text.is_empty() {
                    self.runs += 1;
                    if on(node.parent().and_then(|r| property(r, "rPr", "b"))) {
                        self.bold_runs += 1;
                    }
                }
                self.push(Some(node), text);
            } else if text_break(node) {
                self.push(
                    Some(node),
                    if word_element(node, "tab") {
                        "\t"
                    } else {
                        "\n"
                    },
                );
            }
        }
        if self.text.len() == before {
            // An empty paragraph adds no line.
            self.text.truncate(mark.0);
            self.segments.truncate(mark.1);
        }
    }

    fn block(self, address: String, fill: bool, in_table: bool) -> Block<'a, 'input> {
        Block {
            address,
            application: "Word",
            breaks: true,
            bold: self.runs > 0 && self.runs == self.bold_runs,
            text: self.text,
            segments: self.segments,
            fill,
            in_table,
        }
    }
}

struct GridCell<'a, 'input> {
    node: Node<'a, 'input>,
    row: usize,
    column: usize,
    span: usize,
    restart: bool,
    continued: bool,
}

pub(super) fn layout<'a, 'input>(xml: &'a Document<'input>) -> Result<Layout<'a, 'input>> {
    let mut output = Layout {
        blocks: vec![],
        merges: vec![],
        tables: vec![],
        rows: BTreeMap::new(),
    };
    let hidden = HiddenBranches::new(xml, "docx");
    let instructions = field_instructions(xml, &hidden);
    let mut row = 1;
    for node in xml.descendants().filter(|n| !hidden.contains(*n)) {
        if word_element(node, "tbl") && nearest(node, "tbl").is_none() {
            row += table(node, row, &mut output, &instructions, &hidden)?;
        } else if word_element(node, "p") && nearest(node, "tc").is_none() {
            let mut text = Text::new(&instructions, &hidden);
            text.paragraph(node);
            if !text.text.is_empty() {
                output
                    .blocks
                    .push(text.block(format!("A{row}"), false, false));
                output.rows.insert(row, node);
                row += 1;
            }
        }
    }
    Ok(output)
}

/// Lays out one top-level table from `top`; returns the rows it occupies.
/// Nested tables stay inside the text of their outer cell.
fn table<'a, 'input>(
    table: Node<'a, 'input>,
    top: usize,
    output: &mut Layout<'a, 'input>,
    instructions: &BTreeSet<usize>,
    hidden: &HiddenBranches,
) -> Result<usize> {
    let rows: Vec<_> = table
        .descendants()
        .filter(|n| word_element(*n, "tr") && nearest(*n, "tbl") == Some(table))
        .collect();
    let mut cells = vec![];
    for (r, tr) in rows.iter().enumerate() {
        output.rows.insert(top + r, *tr);
        let mut column = number(property(*tr, "trPr", "gridBefore"), 0)?;
        for tc in tr
            .descendants()
            .filter(|n| word_element(*n, "tc") && nearest(*n, "tr") == Some(*tr))
        {
            let span = number(property(tc, "tcPr", "gridSpan"), 1)?.max(1);
            let merge = property(tc, "tcPr", "vMerge");
            cells.push(GridCell {
                node: tc,
                row: r,
                column,
                span,
                restart: merge.is_some_and(|m| value(m) == Some("restart")),
                continued: merge.is_some_and(|m| value(m).is_none_or(|v| v == "continue")),
            });
            column += span;
        }
    }
    if rows.is_empty() {
        return Ok(0);
    }
    let width = cells
        .iter()
        .map(|c| c.column + c.span)
        .max()
        .unwrap_or(1)
        .max(1);
    let address = |row: usize, column: usize| -> Result<String> {
        Ok(format!("{}{}", column_name(column as u32 + 1)?, top + row))
    };
    let mut emphasis = vec![];
    let continued: BTreeSet<(usize, usize)> = cells
        .iter()
        .filter(|cell| cell.continued)
        .map(|cell| (cell.row, cell.column))
        .collect();
    for cell in &cells {
        let mut bottom = cell.row;
        if cell.restart {
            while continued.contains(&(bottom + 1, cell.column)) {
                bottom += 1;
            }
        }
        if cell.span > 1 || bottom > cell.row {
            output.merges.push(format!(
                "{}:{}",
                address(cell.row, cell.column)?,
                address(bottom, cell.column + cell.span - 1)?
            ));
        }
        let mut text = Text::new(instructions, hidden);
        for paragraph in cell
            .node
            .descendants()
            .filter(|n| word_element(*n, "p") && !hidden.contains(*n))
        {
            text.paragraph(paragraph);
        }
        let fill = property(cell.node, "tcPr", "shd")
            .and_then(|s| s.attribute((WORD, "fill")))
            .is_some_and(|f| !f.eq_ignore_ascii_case("auto") && !f.eq_ignore_ascii_case("FFFFFF"));
        if !text.text.is_empty() {
            let block = text.block(address(cell.row, cell.column)?, fill, true);
            emphasis.push((cell.row, block.bold || fill));
            output.blocks.push(block);
        } else if fill {
            emphasis.push((cell.row, true));
        }
    }
    let header_rows = header_rows(&rows, &emphasis, width);
    output.tables.push(json!({
        "name":property(table, "tblPr", "tblCaption").and_then(value).map_or_else(|| format!("table-{}", output.tables.len() + 1), str::to_owned),
        "range":format!("{}:{}", address(0, 0)?, address(rows.len() - 1, width - 1)?),
        "header_rows":header_rows,"totals_rows":0}));
    Ok(rows.len())
}

/// Header rows as a hypothesis for review: rows Word repeats as a header;
/// otherwise leading rows that are entirely bold or shaded; otherwise the first
/// row of a table with three or more columns. Two-column tables are usually
/// label/value lists, so they get none.
fn header_rows(rows: &[Node<'_, '_>], emphasis: &[(usize, bool)], width: usize) -> usize {
    let repeated = rows
        .iter()
        .take_while(|tr| on(property(**tr, "trPr", "tblHeader")))
        .count();
    if repeated > 0 {
        return repeated;
    }
    let mut row_emphasis = vec![(0usize, true); rows.len()];
    for &(row, emphasized) in emphasis {
        row_emphasis[row].0 += 1;
        row_emphasis[row].1 &= emphasized;
    }
    let emphasized = row_emphasis
        .iter()
        .take_while(|(count, all)| *count > 0 && *all)
        .count();
    if emphasized > 0 && emphasized < rows.len() {
        emphasized
    } else if width >= 3 && rows.len() >= 2 {
        1
    } else {
        0
    }
}

/// Maps an edit of a block's text to new content for the elements it touches:
/// text, in which a line break or tab stands for a break or tab of the run. The
/// change between the old and new text must stay within one paragraph; when it
/// spans runs, the first run takes the new text as Word does when typing over
/// a selection.
pub(super) fn edit<'a, 'input>(
    block: &Block<'a, 'input>,
    after: &str,
) -> Result<Vec<(Node<'a, 'input>, String)>> {
    let before = block.text.as_str();
    let (start, end) = changed_range(before, after, true);
    let inserted = &after[start..after.len() - (before.len() - end)];
    if start == end && inserted.is_empty() {
        return Ok(vec![]);
    }
    let application = block.application;
    ensure!(
        !inserted.contains('\r'),
        "{application} text replacement cannot contain a carriage return in {}; write a line break as \\n",
        block.address
    );
    ensure!(
        block.breaks || !inserted.contains('\n'),
        "{application} text in {} cannot take new line breaks; edit the lines in {application}",
        block.address
    );
    let touched = touched_segments(block, start, end);
    // Repeated text lets the same change sit further left, as when deleting the
    // first of "注意" + "注意事項"; when that would leave other text in the runs,
    // which runs the user meant, and so the formatting the result keeps, is unknown.
    let (left_start, left_end) = changed_range(before, after, false);
    let left_inserted = &after[left_start..after.len() - (before.len() - left_end)];
    ensure!(
        run_texts(block, start, end, inserted)
            == run_texts(block, left_start, left_end, left_inserted),
        "the edit in {} could apply to more than one run because the text around it repeats; include unrepeated text in the change, or edit it in {application}",
        block.address
    );
    ensure!(
        !touched.is_empty() && touched.iter().all(|s| s.node.is_some()),
        "{}",
        if application == "Word" {
            format!(
                "the edit crosses a paragraph boundary or a character Word draws itself (non-breaking hyphen, symbol, equation) in {}; line breaks and tabs can be edited within a paragraph, but paragraphs cannot be split or joined",
                block.address
            )
        } else if block.breaks {
            format!(
                "the edit crosses a paragraph boundary in {}; line breaks and tabs can be edited within a paragraph, but paragraphs cannot be split or joined (add or delete paragraphs with documents rows)",
                block.address
            )
        } else {
            format!(
                "the edit crosses a line or paragraph break in {}; edit each line separately",
                block.address
            )
        }
    );
    Ok(touched
        .iter()
        .zip(replaced(before, &touched, start, end, inserted))
        .map(|(s, new)| (s.node.unwrap(), new))
        .collect())
}

/// The new text of each touched segment when `start..end` of `before` becomes
/// `inserted`.
fn replaced(
    before: &str,
    touched: &[&Segment<'_, '_>],
    start: usize,
    end: usize,
    inserted: &str,
) -> Vec<String> {
    let last = touched.len().saturating_sub(1);
    touched
        .iter()
        .enumerate()
        .map(|(k, s)| {
            let text = &before[s.range.clone()];
            let mut new = String::new();
            if k == 0 {
                new.push_str(&text[..start.saturating_sub(s.range.start).min(text.len())]);
                new.push_str(inserted);
            }
            if k == last {
                new.push_str(&text[end.saturating_sub(s.range.start).min(text.len())..]);
            }
            new
        })
        .collect()
}

/// The text each run of the block holds after `start..end` becomes `inserted`,
/// keyed by the run's position in the part.
fn run_texts(
    block: &Block<'_, '_>,
    start: usize,
    end: usize,
    inserted: &str,
) -> Vec<(usize, String)> {
    let touched = touched_segments(block, start, end);
    let new = replaced(&block.text, &touched, start, end, inserted);
    let mut runs: Vec<(usize, String)> = vec![];
    let mut touched_index = 0;
    for segment in &block.segments {
        let changed = touched
            .get(touched_index)
            .is_some_and(|next| std::ptr::eq(*next, segment));
        if changed {
            touched_index += 1;
        }
        let Some(node) = segment.node else {
            continue;
        };
        let text = if changed {
            new[touched_index - 1].as_str()
        } else {
            &block.text[segment.range.clone()]
        };
        let run = node
            .parent()
            .map_or(node.range().start, |r| r.range().start);
        match runs.last_mut() {
            Some((last, joined)) if *last == run => joined.push_str(text),
            _ => runs.push((run, text.to_owned())),
        }
    }
    runs
}

/// The byte range of `before` that `after` replaces, found by matching the
/// longest common prefix first (`prefix_first`) or the longest common suffix first.
fn changed_range(before: &str, after: &str, prefix_first: bool) -> (usize, usize) {
    let common_prefix = |a: &str, b: &str| -> usize {
        a.chars()
            .zip(b.chars())
            .take_while(|(x, y)| x == y)
            .map(|(x, _)| x.len_utf8())
            .sum()
    };
    let common_suffix = |a: &str, b: &str| -> usize {
        a.chars()
            .rev()
            .zip(b.chars().rev())
            .take_while(|(x, y)| x == y)
            .map(|(x, _)| x.len_utf8())
            .sum()
    };
    if prefix_first {
        let prefix = common_prefix(before, after);
        let suffix = common_suffix(&before[prefix..], &after[prefix..]);
        (prefix, before.len() - suffix)
    } else {
        let suffix = common_suffix(before, after);
        let prefix = common_prefix(
            &before[..before.len() - suffix],
            &after[..after.len() - suffix],
        );
        (prefix, before.len() - suffix)
    }
}

/// The segments an edit of `start..end` changes. An insertion joins the run
/// ending there, else the run starting there.
fn touched_segments<'b, 'a, 'input>(
    block: &'b Block<'a, 'input>,
    start: usize,
    end: usize,
) -> Vec<&'b Segment<'a, 'input>> {
    if start == end {
        let runs = || {
            block
                .segments
                .iter()
                .filter(|s| s.node.is_some() && s.range.start <= start && start <= s.range.end)
        };
        runs()
            .find(|s| s.range.end == start && !s.range.is_empty())
            .or_else(|| runs().next())
            .into_iter()
            .collect()
    } else {
        block
            .segments
            .iter()
            .filter(|s| s.range.start < end && start < s.range.end)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "manual performance measurement"]
    fn bench_word_efficiency() {
        use std::time::Instant;
        let rows = 1200;
        let row =
            "<w:tr><w:tc><w:p><w:r><w:rPr><w:b/></w:rPr><w:t>x</w:t></w:r></w:p></w:tc></w:tr>";
        let xml = document(&format!("<w:tbl>{}</w:tbl>", row.repeat(rows)));
        let parsed = Document::parse(&xml).unwrap();
        let start = Instant::now();
        let result = layout(&parsed).unwrap();
        eprintln!(
            "table_ms={} rows={}",
            start.elapsed().as_millis(),
            result.rows.len()
        );

        let choice = "<w:p><w:r><w:drawing/></w:r></w:p>".repeat(1200);
        let fallback = "<w:p><w:r><w:t>x</w:t></w:r></w:p>".repeat(1200);
        let xml = document(&format!(
            "<mc:AlternateContent><mc:Choice Requires=\"w\">{choice}</mc:Choice><mc:Fallback>{fallback}</mc:Fallback></mc:AlternateContent>"
        ));
        let parsed = Document::parse(&xml).unwrap();
        let start = Instant::now();
        let result = layout(&parsed).unwrap();
        eprintln!(
            "alternate_ms={} blocks={}",
            start.elapsed().as_millis(),
            result.blocks.len()
        );

        let xml = document(&format!(
            "<w:p>{}</w:p>",
            "<w:r><w:t>x</w:t></w:r>".repeat(1200)
        ));
        let parsed = Document::parse(&xml).unwrap();
        let result = layout(&parsed).unwrap();
        let start = Instant::now();
        let edited = edit(&result.blocks[0], "replacement").unwrap();
        eprintln!(
            "edit_ms={} runs={}",
            start.elapsed().as_millis(),
            edited.len()
        );
    }

    #[test]
    fn cached_hidden_branches_match_direct_scan() {
        let xml = document(
            "<mc:AlternateContent><mc:Choice Requires=\"w\"><w:p><w:r><w:t>first</w:t></w:r></w:p></mc:Choice><mc:Fallback><w:p><w:r><w:t>copy</w:t></w:r></w:p></mc:Fallback></mc:AlternateContent><mc:AlternateContent><mc:Choice Requires=\"w\"><w:p><w:r><w:drawing/></w:r></w:p></mc:Choice><mc:Fallback><w:p><w:r><w:t>visible</w:t></w:r></w:p></mc:Fallback></mc:AlternateContent>",
        );
        let parsed = Document::parse(&xml).unwrap();
        let hidden = HiddenBranches::new(&parsed, "docx");
        for node in parsed.descendants() {
            assert_eq!(hidden.contains(node), hidden_copy(node, "docx"));
        }
    }

    fn document(body: &str) -> String {
        format!(
            r#"<w:document xmlns:w="{WORD}" xmlns:mc="{MARKUP_COMPATIBILITY}"><w:body>{body}</w:body></w:document>"#
        )
    }
    fn p(text: &str) -> String {
        format!("<w:p><w:r><w:t>{text}</w:t></w:r></w:p>")
    }
    fn tc(properties: &str, content: &str) -> String {
        format!("<w:tc><w:tcPr>{properties}</w:tcPr>{content}</w:tc>")
    }
    fn cells(layout: &Layout<'_, '_>) -> Vec<(String, String)> {
        layout
            .blocks
            .iter()
            .map(|b| (b.address.clone(), b.text.clone()))
            .collect()
    }

    #[test]
    fn tables_keep_rows_columns_and_merges() {
        let nested = format!(
            "<w:tbl><w:tr>{}{}</w:tr></w:tbl>",
            tc("", &p("内1")),
            tc("", &p("内2"))
        );
        let body = document(&format!(
            "{}<w:tbl><w:tr>{}{}{}</w:tr><w:tr>{}{}{}</w:tr><w:tr>{}{}{}</w:tr><w:tr><w:trPr><w:gridBefore w:val=\"1\"/></w:trPr>{}</w:tr></w:tbl>{}",
            p("受注一覧"),
            tc(r#"<w:gridSpan w:val="2"/>"#, &p("見出し")),
            tc("", &p("型")),
            tc("", ""),
            tc(r#"<w:vMerge w:val="restart"/>"#, &p("ヘッダ")),
            tc(
                "",
                "<w:p><w:r><w:t>受注</w:t></w:r><w:proofErr/><w:r><w:rPr><w:b/></w:rPr><w:t>番号</w:t></w:r></w:p><w:p/>"
            ),
            tc("", &(nested + &p("後"))),
            tc("<w:vMerge/>", "<w:p/>"),
            tc("", &p("数量")),
            tc("", ""),
            tc(r#"<w:gridSpan w:val="2"/>"#, &p("備考")),
            p("本文")
        ));
        let xml = Document::parse(&body).unwrap();
        let layout = layout(&xml).unwrap();
        assert_eq!(
            cells(&layout),
            [
                ("A1", "受注一覧"),
                ("A2", "見出し"),
                ("C2", "型"),
                ("A3", "ヘッダ"),
                ("B3", "受注番号"),
                ("C3", "内1\n内2\n後"),
                ("B4", "数量"),
                ("B5", "備考"),
                ("A6", "本文"),
            ]
            .map(|(a, t)| (a.to_owned(), t.to_owned()))
        );
        assert_eq!(layout.merges, ["A2:B2", "A3:A4", "B5:C5"]);
        assert_eq!(
            layout.tables,
            [json!({"name":"table-1","range":"A2:D5","header_rows":1,"totals_rows":0})]
        );
        let sheet = layout.sheet("document", "word/document.xml");
        assert_eq!(
            sheet["cells"][4]["style"],
            json!({"bold":false,"fill":0,"border":1})
        );
        assert_eq!(sheet["cells"][0]["style"]["border"], 0);
    }

    #[test]
    fn header_rows_follow_repeat_marks_emphasis_or_width() {
        let row = |header: &str, cells: &[&str]| {
            format!(
                "<w:tr><w:trPr>{header}</w:trPr>{}</w:tr>",
                cells.iter().map(|c| tc("", c)).collect::<String>()
            )
        };
        let bold = "<w:p><w:r><w:rPr><w:b/></w:rPr><w:t>項目</w:t></w:r></w:p>";
        let shaded = tc(r#"<w:shd w:val="clear" w:fill="D9D9D9"/>"#, &p("値"));
        let cases = [
            (
                format!(
                    "{}{}{}",
                    row("<w:tblHeader/>", &[&p("a"), &p("b")]),
                    row("<w:tblHeader/>", &[&p("c"), &p("d")]),
                    row("", &[&p("e"), &p("f")])
                ),
                2,
            ),
            (
                format!(
                    "<w:tr>{}{shaded}</w:tr>{}",
                    tc("", bold),
                    row("", &[&p("x"), &p("y")])
                ),
                1,
            ),
            (
                format!(
                    "{}{}",
                    row("", &[&p("項目"), &p("値")]),
                    row("", &[&p("x"), &p("y")])
                ),
                0,
            ),
            (
                format!(
                    "{}{}",
                    row("", &[&p("a"), &p("b"), &p("c")]),
                    row("", &[&p("x"), &p("y"), &p("z")])
                ),
                1,
            ),
        ];
        for (rows, expected) in cases {
            let body = document(&format!("<w:tbl>{rows}</w:tbl>"));
            let xml = Document::parse(&body).unwrap();
            assert_eq!(
                layout(&xml).unwrap().tables[0]["header_rows"],
                expected,
                "{rows}"
            );
        }
    }

    #[test]
    fn text_box_in_a_cell_joins_the_cell_and_breaks_and_tabs_are_kept() {
        let text_box = format!(
            "<w:p><w:r><w:t>画面</w:t></w:r><w:r><mc:AlternateContent><mc:Choice Requires=\"wps\"><w:txbxContent>{}</w:txbxContent></mc:Choice><mc:Fallback><w:txbxContent>{}</w:txbxContent></mc:Fallback></mc:AlternateContent></w:r></w:p>",
            p("注記"),
            p("注記")
        );
        let body = document(&format!(
            "<w:tbl><w:tr>{}</w:tr></w:tbl><w:p><w:r><w:t>a</w:t><w:tab/><w:t>b</w:t><w:br/><w:t>c</w:t></w:r><w:pPr><w:tabs><w:tab w:val=\"left\" w:pos=\"100\"/></w:tabs></w:pPr></w:p>",
            tc("", &text_box)
        ));
        let xml = Document::parse(&body).unwrap();
        assert_eq!(
            cells(&layout(&xml).unwrap()),
            [("A1", "画面\n注記"), ("A2", "a\tb\nc")].map(|(a, t)| (a.to_owned(), t.to_owned()))
        );
    }

    /// Edits the block of a one-cell table holding `cell`.
    fn edits(cell: &str, after: &str) -> Result<Vec<(String, String)>> {
        let body = document(&format!("<w:tbl><w:tr><w:tc>{cell}</w:tc></w:tr></w:tbl>"));
        let xml = Document::parse(&body).unwrap();
        let layout = layout(&xml).unwrap();
        Ok(edit(&layout.blocks[0], after)?
            .into_iter()
            .map(|(node, new)| (element(node), new))
            .collect())
    }

    /// The text of a text element, or `<name>` for a tab or break.
    fn element(node: Node<'_, '_>) -> String {
        if word_element(node, "t") {
            node.text().unwrap_or("").to_owned()
        } else {
            format!("<{}>", node.tag_name().name())
        }
    }

    #[test]
    fn edits_touch_only_the_runs_that_change() {
        let split = "<w:p><w:r><w:t>受注</w:t></w:r><w:r><w:rPr><w:b/></w:rPr><w:t>番</w:t></w:r><w:r><w:t>号</w:t></w:r></w:p><w:p><w:r><w:t>二行目</w:t></w:r></w:p>";
        let pairs = |v: &[(&str, &str)]| -> Vec<(String, String)> {
            v.iter()
                .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
                .collect()
        };
        assert_eq!(edits(split, "受注番号\n二行目").unwrap(), []);
        assert_eq!(
            edits(split, "受注NO\n二行目").unwrap(),
            pairs(&[("番", "NO"), ("号", "")])
        );
        assert_eq!(
            edits(split, "受注番号\n三行目").unwrap(),
            pairs(&[("二行目", "三行目")])
        );
        assert_eq!(
            edits(split, "発注番号\n二行目").unwrap(),
            pairs(&[("受注", "発注")])
        );
        // Insertion at a run boundary joins the run before it.
        assert_eq!(
            edits(split, "受注の番号\n二行目").unwrap(),
            pairs(&[("受注", "受注の")])
        );
        // Clearing text inside one paragraph keeps the first run as the target.
        assert_eq!(
            edits(split, "\n二行目").unwrap(),
            pairs(&[("受注", ""), ("番", ""), ("号", "")])
        );
        // Paragraphs are neither joined nor split.
        let error = edits(split, "受注番号二行目").unwrap_err().to_string();
        assert!(error.contains("crosses"), "{error}");
        let error = edits(split, "受注番号\r\n二行目").unwrap_err().to_string();
        assert!(error.contains("carriage return"), "{error}");
    }

    /// A line break or tab in new text becomes a break or tab of the run it
    /// joins; existing ones can be removed or replaced within the paragraph.
    #[test]
    fn line_breaks_and_tabs_are_edited_within_a_paragraph() {
        let split = "<w:p><w:r><w:t>受注</w:t></w:r><w:r><w:rPr><w:b/></w:rPr><w:t>番</w:t></w:r><w:r><w:t>号</w:t></w:r></w:p><w:p><w:r><w:t>二行目</w:t></w:r></w:p>";
        let pairs = |v: &[(&str, &str)]| -> Vec<(String, String)> {
            v.iter()
                .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
                .collect()
        };
        assert_eq!(
            edits(split, "受注番号\n二行目\n三行目").unwrap(),
            pairs(&[("二行目", "二行目\n三行目")])
        );
        assert_eq!(
            edits(split, "受注\t番号\n二行目").unwrap(),
            pairs(&[("受注", "受注\t")])
        );
        let broken =
            "<w:p><w:r><w:t>項目</w:t><w:tab/><w:t>値</w:t><w:br/><w:t>注</w:t></w:r></w:p>";
        assert_eq!(
            edits(broken, "項目値\n注").unwrap(),
            pairs(&[("<tab>", "")])
        );
        assert_eq!(
            edits(broken, "項目：値\n注").unwrap(),
            pairs(&[("<tab>", "：")])
        );
        assert_eq!(edits(broken, "項目\t値注").unwrap(), pairs(&[("<br>", "")]));
        assert_eq!(
            edits(broken, "項目\t値\n\n注").unwrap(),
            pairs(&[("<br>", "\n\n")])
        );
        assert_eq!(
            edits(broken, "項目\t\t値\n注").unwrap(),
            pairs(&[("<tab>", "\t\t")])
        );
        assert_eq!(
            edits(broken, "項目 X 値\n注").unwrap(),
            pairs(&[("<tab>", " X ")])
        );
        // A page break is not text, so a line break added beside it joins the text.
        let page = "<w:p><w:r><w:t>前</w:t><w:br w:type=\"page\"/><w:t>後</w:t></w:r></w:p>";
        assert_eq!(edits(page, "前\n後").unwrap(), pairs(&[("前", "前\n")]));
    }

    /// Ruby readings, tracked deletions and moves, and hidden text are not
    /// part of what Word shows in the paragraph.
    #[test]
    fn readings_tracked_deletions_and_hidden_text_are_not_extracted() {
        let body = document(concat!(
            "<w:p><w:ruby><w:rt><w:r><w:t>かん</w:t></w:r></w:rt><w:rubyBase><w:r><w:t>漢</w:t></w:r></w:rubyBase></w:ruby><w:r><w:t>字</w:t></w:r></w:p>",
            "<w:p><w:r><w:t>残</w:t></w:r><w:del><w:r><w:tab/><w:delText>消</w:delText></w:r></w:del><w:moveFrom><w:r><w:t>移動元</w:t></w:r></w:moveFrom><w:moveTo><w:r><w:t>移動先</w:t></w:r></w:moveTo></w:p>",
            "<w:p><w:r><w:rPr><w:vanish/></w:rPr><w:t>隠し</w:t></w:r><w:r><w:rPr><w:vanish w:val=\"0\"/></w:rPr><w:t>表示</w:t></w:r></w:p>",
        ));
        let xml = Document::parse(&body).unwrap();
        assert_eq!(
            cells(&layout(&xml).unwrap()),
            [("A1", "漢字"), ("A2", "残移動先"), ("A3", "表示")]
                .map(|(a, t)| (a.to_owned(), t.to_owned()))
        );
    }

    /// With repeated text the change could fall in different runs, so the
    /// formatting the result keeps is unknown and the edit is refused.
    #[test]
    fn edits_that_repeated_text_makes_ambiguous_are_refused() {
        let repeated = "<w:p><w:r><w:rPr><w:b/></w:rPr><w:t>注意</w:t></w:r><w:r><w:t>注意事項</w:t></w:r></w:p>";
        let error = edits(repeated, "注意事項").unwrap_err().to_string();
        assert!(error.contains("more than one run"), "{error}");
        assert_eq!(
            edits(repeated, "注意注意事項一覧").unwrap(),
            [("注意事項".to_owned(), "注意事項一覧".to_owned())]
        );
        // Repeats inside one run change that run either way.
        assert_eq!(
            edits("<w:p><w:r><w:t>ああい</w:t></w:r></w:p>", "あい").unwrap(),
            [("ああい".to_owned(), "あい".to_owned())]
        );
    }

    #[test]
    fn content_controls_bound_to_data_are_recognized() {
        let body = document(concat!(
            "<w:sdt><w:sdtPr><w:dataBinding w:xpath=\"/ns0:coreProperties[1]/ns1:title[1]\"/></w:sdtPr><w:sdtContent><w:p><w:r><w:t>表題</w:t></w:r></w:p></w:sdtContent></w:sdt>",
            "<w:sdt><w:sdtPr/><w:sdtContent><w:p><w:r><w:t>自由</w:t></w:r></w:p></w:sdtContent></w:sdt>",
        ));
        let xml = Document::parse(&body).unwrap();
        let bound: Vec<_> = xml
            .descendants()
            .filter(|n| word_element(*n, "t"))
            .map(|n| bound_control(n).is_some())
            .collect();
        assert_eq!(bound, [true, false]);
    }

    /// A new row goes next to a row of its kind, and a table whose rows all
    /// go is deleted.
    #[test]
    fn restructured_rows_follow_their_kind_and_whole_tables_go() {
        let operation = |kind, at, style_from| StructuralOperation {
            id: "op".into(),
            sheet: "document".into(),
            kind,
            at,
            count: 1,
            style_from,
        };
        let body = document(&format!(
            "{}<w:tbl><w:tr>{}</w:tr><w:tr>{}</w:tr></w:tbl>{}",
            p("前"),
            tc("", &p("a")),
            tc("", &p("b")),
            p("後")
        ));
        let xml = Document::parse(&body).unwrap();
        let layout = layout(&xml).unwrap();
        let apply = |operations: &[StructuralOperation]| -> Result<String> {
            let operations: Vec<_> = operations.iter().collect();
            let values = InsertedText::from([(("op".into(), 0, "A".into()), "新".into())]);
            splice(
                &body,
                layout.restructure(&body, &operations, &values)?,
                "test",
            )
        };
        // A paragraph copied from the one above a table's end goes after the table.
        let inserted = apply(&[operation(OperationKind::InsertRows, 4, Some(1))]).unwrap();
        assert!(
            inserted.contains("</w:tbl><w:p><w:r><w:t xml:space=\"preserve\">新</w:t></w:r></w:p><w:p><w:r><w:t>後</w:t>"),
            "{inserted}"
        );
        // Deleting both rows deletes the table.
        let deleted = apply(&[
            operation(OperationKind::DeleteRows, 2, None),
            operation(OperationKind::DeleteRows, 2, None),
        ])
        .unwrap();
        assert!(
            !deleted.contains("w:tbl") && deleted.contains("後"),
            "{deleted}"
        );
        // A table row goes at the top of its table, but has no place after the
        // last paragraph.
        let top = apply(&[operation(OperationKind::InsertRows, 2, Some(2))]).unwrap();
        assert!(
            top.contains(
                "<w:tbl><w:tr><w:tc><w:tcPr></w:tcPr><w:p><w:r><w:t xml:space=\"preserve\">新"
            ),
            "{top}"
        );
        let error = apply(&[operation(OperationKind::InsertRows, 5, Some(2))])
            .unwrap_err()
            .to_string();
        assert!(error.contains("no place"), "{error}");
    }

    /// Placeholder prompts, the inner results in a field's instructions and
    /// page breaks are not text; characters Word draws itself are kept.
    #[test]
    fn prompts_field_instructions_and_page_breaks_are_not_text() {
        let body = document(concat!(
            "<w:sdt><w:sdtPr><w:showingPlcHdr/></w:sdtPr><w:sdtContent><w:p><w:r><w:t>クリックまたはタップしてテキストを入力してください。</w:t></w:r></w:p></w:sdtContent></w:sdt>",
            "<w:p><w:r><w:fldChar w:fldCharType=\"begin\"/></w:r><w:r><w:instrText>IF </w:instrText></w:r><w:r><w:fldChar w:fldCharType=\"begin\"/></w:r><w:r><w:instrText>MERGEFIELD 性別</w:instrText></w:r><w:r><w:fldChar w:fldCharType=\"separate\"/></w:r><w:r><w:t>男</w:t></w:r><w:r><w:fldChar w:fldCharType=\"end\"/></w:r><w:r><w:instrText> = \"男\" \"様\" \"殿\"</w:instrText></w:r><w:r><w:fldChar w:fldCharType=\"separate\"/></w:r><w:r><w:t>様</w:t></w:r><w:r><w:fldChar w:fldCharType=\"end\"/></w:r></w:p>",
            "<w:p><w:r><w:br w:type=\"page\"/></w:r><w:r><w:t>第2章</w:t></w:r></w:p>",
            "<w:p><w:r><w:t>03</w:t><w:noBreakHyphen/><w:t>1234</w:t><w:sym w:font=\"Wingdings\" w:char=\"F0FC\"/><w:ptab w:relativeTo=\"margin\" w:alignment=\"right\" w:leader=\"none\"/><w:t>右</w:t></w:r></w:p>",
            "<w:p xmlns:m=\"http://schemas.openxmlformats.org/officeDocument/2006/math\"><w:r><w:t>式 </w:t></w:r><m:oMath><m:r><m:t>x=1</m:t></m:r></m:oMath></w:p>",
        ));
        let xml = Document::parse(&body).unwrap();
        let layout = layout(&xml).unwrap();
        assert_eq!(
            cells(&layout),
            [
                ("A1", "様"),
                ("A2", "第2章"),
                ("A3", "03\u{2011}1234\u{f0fc}\t右"),
                ("A4", "式 x=1"),
            ]
            .map(|(a, t)| (a.to_owned(), t.to_owned()))
        );
        // Characters Word draws itself cannot be replaced as text.
        let error = edit(&layout.blocks[2], "0312345\u{f0fc}\t右")
            .unwrap_err()
            .to_string();
        assert!(error.contains("draws itself"), "{error}");
        assert_eq!(
            extract_field_codes(&xml, "word/document.xml"),
            ["MERGEFIELD 性別", "IF  = \"男\" \"様\" \"殿\""]
                .map(|instruction| json!({"part":"word/document.xml","instruction":instruction}))
        );
        assert_eq!(
            edit(&layout.blocks[2], "03\u{2011}9999\u{f0fc}\t右")
                .unwrap()
                .into_iter()
                .map(|(node, new)| (node.text().unwrap().to_owned(), new))
                .collect::<Vec<_>>(),
            [("1234".to_owned(), "9999".to_owned())]
        );
    }
}
