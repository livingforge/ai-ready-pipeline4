//! PowerPoint text laid out like Word's (see [`super::word`]): each paragraph
//! outside tables takes one row in column A, in the order of the slide, and a
//! table keeps its rows and grid columns, reports merged cells as merges and
//! its range as a table. A block's segments map its text back to the `a:t` and
//! `a:br` elements, so that an edit changes only the runs it touches.
use super::word::{Block, InsertedText, Placed, Segment, blocks_sheet, placed_rows};
use super::*;
use crate::excel::{StructuralOperation, column_name};
use std::ops::Range;

fn drawing(node: Node<'_, '_>, name: &str) -> bool {
    node.has_tag_name((DRAWING, name))
}

/// On/off attributes are off unless set to 1 or true.
fn on(node: Node<'_, '_>, attribute: &str) -> bool {
    matches!(node.attribute(attribute), Some("1" | "true"))
}

fn nearest<'a, 'input>(node: Node<'a, 'input>, name: &str) -> Option<Node<'a, 'input>> {
    node.ancestors().skip(1).find(|n| drawing(*n, name))
}

fn span(node: Node<'_, '_>, attribute: &str) -> usize {
    node.attribute(attribute)
        .and_then(|v| v.parse().ok())
        .unwrap_or(1)
        .max(1)
}

/// Whether `node` is shown: not a copy for older readers in
/// `mc:AlternateContent`, and not in a notes page's slide number, date,
/// header or footer, which PowerPoint fills from the notes master.
fn shown(node: Node<'_, '_>) -> bool {
    let notes = node
        .document()
        .root_element()
        .has_tag_name((PRESENTATION, "notes"));
    !hidden_copy(node, "pptx")
        && !(notes
            && node.ancestors().any(|shape| {
                shape.has_tag_name((PRESENTATION, "sp"))
                    && shape
                        .descendants()
                        .find(|p| p.has_tag_name((PRESENTATION, "ph")))
                        .is_some_and(|placeholder| {
                            matches!(
                                placeholder.attribute("type"),
                                Some("sldNum" | "dt" | "hdr" | "ftr")
                            )
                        })
            }))
}

/// The qualified name of the element `raw` starts, and its prefix with the colon.
fn names(raw: &str) -> (&str, String) {
    let name = raw
        .trim_start_matches('<')
        .split(|c: char| c.is_whitespace() || c == '/' || c == '>')
        .next()
        .unwrap_or("");
    let prefix = name
        .split_once(':')
        .map_or(String::new(), |(prefix, _)| format!("{prefix}:"));
    (name, prefix)
}

/// Runs holding `text` in the run format `format` (an `a:rPr` element, or
/// empty), with an `a:br` of the same format for each line break.
fn runs(text: &str, prefix: &str, format: &str) -> Result<String> {
    let mut xml = String::new();
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            if format.is_empty() {
                xml.push_str(&format!("<{prefix}br/>"));
            } else {
                xml.push_str(&format!("<{prefix}br>{format}</{prefix}br>"));
            }
        }
        if !line.is_empty() {
            xml.push_str(&format!(
                "<{prefix}r>{format}<{prefix}t>{}</{prefix}t></{prefix}r>",
                xml_text(line)?
            ));
        }
    }
    Ok(xml)
}

/// The markup replacing an edited `a:t` or `a:br` of `original` so that it
/// shows `text`: tabs stay in the text, and a line break becomes an `a:br`
/// between runs of the same format.
pub(super) fn replacement(
    original: &str,
    node: Node<'_, '_>,
    text: &str,
) -> Result<(Range<usize>, String)> {
    let format = |element: Node<'_, '_>| {
        element
            .children()
            .find(|n| drawing(*n, "rPr"))
            .map_or("", |properties| &original[properties.range()])
    };
    let raw = &original[node.range()];
    let (name, prefix) = names(raw);
    if drawing(node, "br") {
        return Ok((node.range(), runs(text, &prefix, format(node))?));
    }
    if !text.contains('\n') {
        let end = raw.find('>').context("invalid text element")?;
        let opening = raw[..end].trim_end_matches('/');
        return Ok((
            node.range(),
            format!("{opening}>{}</{name}>", xml_text(text)?),
        ));
    }
    let run = node
        .parent()
        .filter(|parent| drawing(*parent, "r"))
        .context("a line break can be added to the text of a run only")?;
    Ok((run.range(), runs(text, &prefix, format(run))?))
}

#[derive(Default)]
struct Text<'a, 'input> {
    text: String,
    segments: Vec<Segment<'a, 'input>>,
    runs: usize,
    bold_runs: usize,
    /// The runs (`a:r`, and `a:fld` for fields) whose text the text shows.
    run_nodes: Vec<Node<'a, 'input>>,
}

impl<'a, 'input> Text<'a, 'input> {
    fn push(&mut self, node: Option<Node<'a, 'input>>, text: &str) {
        let start = self.text.len();
        self.text.push_str(text);
        self.segments.push(Segment {
            node,
            range: start..self.text.len(),
        });
    }

    /// Appends the text of one paragraph; an empty paragraph adds nothing.
    fn paragraph(&mut self, paragraph: Node<'a, 'input>) {
        let mark = (self.text.len(), self.segments.len());
        if !self.text.is_empty() {
            self.push(None, "\n");
        }
        let before = self.text.len();
        for node in paragraph
            .descendants()
            .filter(|n| nearest(*n, "p") == Some(paragraph) && !hidden_copy(*n, "pptx"))
        {
            if drawing(node, "t") {
                let text = node.text().unwrap_or("");
                if !text.is_empty() {
                    self.runs += 1;
                    if let Some(run) = node
                        .parent()
                        .filter(|p| drawing(*p, "r") || drawing(*p, "fld"))
                        && self.run_nodes.last() != Some(&run)
                    {
                        self.run_nodes.push(run);
                    }
                    if node
                        .parent()
                        .and_then(|run| run.children().find(|n| drawing(*n, "rPr")))
                        .is_some_and(|properties| on(properties, "b"))
                    {
                        self.bold_runs += 1;
                    }
                }
                self.push(Some(node), text);
            } else if drawing(node, "br") {
                self.push(Some(node), "\n");
            }
        }
        if self.text.len() == before {
            self.text.truncate(mark.0);
            self.segments.truncate(mark.1);
        }
    }

    fn block(
        self,
        address: String,
        place: Option<super::word_fonts::TablePlace>,
    ) -> Block<'a, 'input> {
        let bold = self.runs > 0 && self.runs == self.bold_runs;
        Block::slide(
            address,
            self.text,
            self.segments,
            bold,
            self.run_nodes,
            place,
        )
    }
}

pub(super) struct SlideLayout<'a, 'input> {
    pub blocks: Vec<Block<'a, 'input>>,
    merges: Vec<String>,
    tables: Vec<Value>,
    /// The element of each row: a paragraph or a table row.
    rows: BTreeMap<usize, Node<'a, 'input>>,
}

pub(super) fn layout<'a, 'input>(xml: &'a Document<'input>) -> Result<SlideLayout<'a, 'input>> {
    let mut output = SlideLayout {
        blocks: vec![],
        merges: vec![],
        tables: vec![],
        rows: BTreeMap::new(),
    };
    let mut row = 1;
    for node in xml.descendants() {
        if drawing(node, "tbl") && shown(node) {
            row += table(node, row, &mut output)?;
        } else if drawing(node, "p") && nearest(node, "tc").is_none() && shown(node) {
            let mut text = Text::default();
            text.paragraph(node);
            if !text.text.is_empty() {
                output.blocks.push(text.block(format!("A{row}"), None));
                output.rows.insert(row, node);
                row += 1;
            } else if empty_placeholder(node) {
                // An empty placeholder holds the text to write, as one empty line.
                let mut block = text.block(format!("A{row}"), None);
                block.empty = Some(node);
                output.blocks.push(block);
                output.rows.insert(row, node);
                row += 1;
            }
        }
    }
    Ok(output)
}

/// Whether `paragraph` is the first of a text placeholder that holds no text,
/// as one is on a slide made from a layout: its text is written there. The
/// date, footer, slide number and header placeholders show fields instead,
/// and picture, chart, table and media placeholders hold objects.
fn empty_placeholder(paragraph: Node<'_, '_>) -> bool {
    let Some(body) = paragraph
        .parent()
        .filter(|b| b.has_tag_name((PRESENTATION, "txBody")))
    else {
        return false;
    };
    let Some(shape) = body
        .parent()
        .filter(|s| s.has_tag_name((PRESENTATION, "sp")))
    else {
        return false;
    };
    let placeholder = shape
        .children()
        .find(|n| n.has_tag_name((PRESENTATION, "nvSpPr")))
        .and_then(|n| {
            n.children()
                .find(|n| n.has_tag_name((PRESENTATION, "nvPr")))
        })
        .and_then(|n| n.children().find(|n| n.has_tag_name((PRESENTATION, "ph"))));
    placeholder.is_some_and(|ph| {
        !matches!(
            ph.attribute("type"),
            Some(
                "dt" | "ftr"
                    | "sldNum"
                    | "hdr"
                    | "sldImg"
                    | "pic"
                    | "chart"
                    | "tbl"
                    | "media"
                    | "clipArt"
                    | "dgm"
            )
        )
    }) && body.children().find(|n| drawing(*n, "p")) == Some(paragraph)
        && !body
            .descendants()
            .any(|n| drawing(n, "t") && n.text().is_some_and(|t| !t.is_empty()))
}

/// The edit writing `text` into the empty placeholder paragraph `paragraph`:
/// runs in the format its end mark (`a:endParaRPr`) sets, as PowerPoint makes
/// them when text is typed into the placeholder.
pub(super) fn fill_empty(
    original: &str,
    paragraph: Node<'_, '_>,
    text: &str,
) -> Result<(Range<usize>, String)> {
    let raw = &original[paragraph.range()];
    let (name, prefix) = names(raw);
    let end = paragraph.children().find(|n| drawing(*n, "endParaRPr"));
    let format = match end {
        Some(end) => {
            let raw = &original[end.range()];
            let (end_name, _) = names(raw);
            raw.replacen(end_name, &format!("{prefix}rPr"), 1)
                .replace(&format!("</{end_name}>"), &format!("</{prefix}rPr>"))
        }
        None => String::new(),
    };
    let content = runs(text, &prefix, &format)?;
    Ok(match end {
        Some(end) => (end.range().start..end.range().start, content),
        None if raw.ends_with("/>") => (
            paragraph.range(),
            format!("{}>{content}</{name}>", raw[..raw.len() - 2].trim_end()),
        ),
        None => {
            let at = paragraph.range().start + raw.rfind("</").context("invalid paragraph")?;
            (at..at, content)
        }
    })
}

/// Lays out one table from row `top`; returns the rows it occupies. A cell
/// merged into its neighbor (`hMerge`, `vMerge`) is not shown, whatever text
/// it still holds.
fn table<'a, 'input>(
    table: Node<'a, 'input>,
    top: usize,
    output: &mut SlideLayout<'a, 'input>,
) -> Result<usize> {
    let rows: Vec<_> = table.children().filter(|n| drawing(*n, "tr")).collect();
    if rows.is_empty() {
        return Ok(0);
    }
    let address = |row: usize, column: usize| -> Result<String> {
        Ok(format!("{}{}", column_name(column as u32 + 1)?, top + row))
    };
    let width = rows
        .iter()
        .map(|tr| tr.children().filter(|n| drawing(*n, "tc")).count())
        .max()
        .unwrap_or(1)
        .max(1);
    for (r, tr) in rows.iter().enumerate() {
        output.rows.insert(top + r, *tr);
        for (c, tc) in tr.children().filter(|n| drawing(*n, "tc")).enumerate() {
            if on(tc, "hMerge") || on(tc, "vMerge") {
                continue;
            }
            let (columns, down) = (span(tc, "gridSpan"), span(tc, "rowSpan"));
            if columns > 1 || down > 1 {
                output.merges.push(format!(
                    "{}:{}",
                    address(r, c)?,
                    address(r + down - 1, c + columns - 1)?
                ));
            }
            let mut text = Text::default();
            for paragraph in tc.descendants().filter(|n| drawing(*n, "p")) {
                text.paragraph(paragraph);
            }
            if !text.text.is_empty() {
                let place = super::word_fonts::TablePlace {
                    first_row: r == 0,
                    last_row: r + down == rows.len(),
                    first_column: c == 0,
                    last_column: c + columns == width,
                };
                output.blocks.push(text.block(address(r, c)?, Some(place)));
            }
        }
    }
    let properties = table.children().find(|n| drawing(*n, "tblPr"));
    let flag = |name: &str| usize::from(properties.is_some_and(|p| on(p, name)));
    let header_rows = if rows.len() >= 2 { flag("firstRow") } else { 0 };
    let totals_rows = if rows.len() >= header_rows + 2 {
        flag("lastRow")
    } else {
        0
    };
    let name = table
        .ancestors()
        .find(|n| n.has_tag_name((PRESENTATION, "graphicFrame")))
        .and_then(|frame| {
            frame
                .descendants()
                .find(|n| n.has_tag_name((PRESENTATION, "cNvPr")))
        })
        .and_then(|n| n.attribute("name"))
        .map_or_else(
            || format!("table-{}", output.tables.len() + 1),
            str::to_owned,
        );
    output.tables.push(json!({
        "name":name,
        "range":format!("{}:{}", address(0, 0)?, address(rows.len() - 1, width - 1)?),
        "header_rows":header_rows,"totals_rows":totals_rows}));
    Ok(rows.len())
}

/// A paragraph like `template` (its properties, the format of its first run
/// and its end mark) holding `text`.
fn new_paragraph(original: &str, template: Node<'_, '_>, text: &str) -> Result<String> {
    let raw = &original[template.range()];
    let (name, prefix) = names(raw);
    let end = raw.find('>').context("invalid paragraph")?;
    let mut xml = format!("{}>", raw[..end].trim_end_matches('/'));
    let child = |kind: &str| {
        template
            .children()
            .find(|n| drawing(*n, kind))
            .map(|n| &original[n.range()])
    };
    if let Some(properties) = child("pPr") {
        xml.push_str(properties);
    }
    let end_mark = child("endParaRPr");
    let format = match template
        .children()
        .find(|n| drawing(*n, "r"))
        .and_then(|run| run.children().find(|n| drawing(*n, "rPr")))
    {
        Some(properties) => original[properties.range()].to_owned(),
        // A paragraph without runs keeps the format of its end mark.
        None => end_mark.map_or(String::new(), |mark| {
            mark.replacen(&format!("<{prefix}endParaRPr"), &format!("<{prefix}rPr"), 1)
                .replacen(
                    &format!("</{prefix}endParaRPr>"),
                    &format!("</{prefix}rPr>"),
                    1,
                )
        }),
    };
    xml.push_str(&runs(text, &prefix, &format)?);
    if let Some(mark) = end_mark {
        xml.push_str(mark);
    }
    xml.push_str(&format!("</{name}>"));
    Ok(xml)
}

fn vertically_merged(row: Node<'_, '_>) -> bool {
    row.children()
        .filter(|n| drawing(*n, "tc"))
        .any(|cell| on(cell, "vMerge") || span(cell, "rowSpan") > 1)
}

/// A table row like `template` (its height, and each cell's properties, text
/// body format and first paragraph format) whose cells hold `text` by grid column.
fn new_table_row(
    original: &str,
    template: Node<'_, '_>,
    row: usize,
    text: impl Fn(&str) -> Option<String>,
) -> Result<String> {
    ensure!(
        !vertically_merged(template),
        "row {row} is part of a vertically merged cell, so it cannot be copied; insert the row in PowerPoint"
    );
    let raw = &original[template.range()];
    let (name, _) = names(raw);
    let end = raw.find('>').context("invalid table row")?;
    let mut xml = format!("{}>", raw[..end].trim_end_matches('/'));
    for (column, cell) in template
        .children()
        .filter(|n| drawing(*n, "tc"))
        .enumerate()
    {
        let cell_raw = &original[cell.range()];
        let (cell_name, prefix) = names(cell_raw);
        let end = cell_raw.find('>').context("invalid table cell")?;
        xml.push_str(&format!("{}>", cell_raw[..end].trim_end_matches('/')));
        if let Some(body) = cell.children().find(|n| drawing(*n, "txBody")) {
            xml.push_str(&format!("<{prefix}txBody>"));
            for format in body
                .children()
                .filter(|n| drawing(*n, "bodyPr") || drawing(*n, "lstStyle"))
            {
                xml.push_str(&original[format.range()]);
            }
            let paragraph = body
                .children()
                .find(|n| drawing(*n, "p"))
                .with_context(|| format!("a cell of row {row} has no paragraph to copy"))?;
            let letter = column_name(column as u32 + 1)?;
            let value = if on(cell, "hMerge") {
                String::new()
            } else {
                text(&letter).unwrap_or_default()
            };
            xml.push_str(&new_paragraph(original, paragraph, &value)?);
            xml.push_str(&format!("</{prefix}txBody>"));
        }
        if let Some(properties) = cell.children().find(|n| drawing(*n, "tcPr")) {
            xml.push_str(&original[properties.range()]);
        }
        xml.push_str(&format!("</{cell_name}>"));
    }
    xml.push_str(&format!("</{name}>"));
    Ok(xml)
}

/// Whether row operations can change `node`: text kept twice for older
/// readers cannot be copied or removed in step.
fn ensure_row_editable(node: Node<'_, '_>, row: usize) -> Result<()> {
    ensure!(
        !node.ancestors().any(alternate_branch),
        "row {row} has a copy for older readers (mc:AlternateContent); insert or delete it in PowerPoint"
    );
    Ok(())
}

impl SlideLayout<'_, '_> {
    pub fn sheet(&self, name: &str, part: &str, fonts: &super::slide_fonts::SlideFonts) -> Value {
        blocks_sheet(
            name,
            part,
            &self.blocks,
            &self.merges,
            &self.tables,
            Some(&|block| fonts.block_font(block)),
        )
    }

    /// Edits that insert and delete paragraphs and table rows as `operations`
    /// (the row insertions and deletions of this page, in order) do. A new
    /// row copies the paragraph or table row it takes its format from (the
    /// row above it, or `style_from`) into the same shape or table, with the
    /// text `values` gives it. A shape keeps an empty paragraph when all of
    /// its paragraphs are deleted, and a table whose rows are all deleted is
    /// deleted.
    pub(super) fn restructure(
        &self,
        original: &str,
        operations: &[&StructuralOperation],
        values: &InsertedText,
    ) -> Result<Vec<(Range<usize>, String)>> {
        if operations.is_empty() {
            return Ok(vec![]);
        }
        let placed = placed_rows(&self.rows, operations, "PowerPoint")?;
        let kept: BTreeSet<usize> = placed
            .iter()
            .filter_map(|p| match p {
                Placed::Original(row) => Some(*row),
                Placed::Inserted { .. } => None,
            })
            .collect();
        let mut first_by_container = BTreeMap::new();
        let mut kept_tables = BTreeSet::new();
        for (row, node) in &self.rows {
            if let Some(container) = node.parent() {
                first_by_container
                    .entry(container.range().start)
                    .or_insert(node.range().start);
                if kept.contains(row) && drawing(*node, "tr") {
                    kept_tables.insert(container.range().start);
                }
            }
        }
        let mut before = vec![None; placed.len()];
        let mut last_by_container = BTreeMap::new();
        for (index, entry) in placed.iter().enumerate() {
            let row = match entry {
                Placed::Original(row) => *row,
                Placed::Inserted { template, .. } => *template,
            };
            let container = self.rows[&row]
                .parent()
                .context("row outside a shape or table")?;
            let key = container.range().start;
            before[index] = last_by_container.get(&key).copied();
            if matches!(entry, Placed::Original(_)) {
                last_by_container.insert(key, row);
            }
        }
        let mut after = vec![None; placed.len()];
        let mut next_by_container = BTreeMap::new();
        for (index, entry) in placed.iter().enumerate().rev() {
            let row = match entry {
                Placed::Original(row) => *row,
                Placed::Inserted { template, .. } => *template,
            };
            let container = self.rows[&row]
                .parent()
                .context("row outside a shape or table")?;
            let key = container.range().start;
            after[index] = next_by_container.get(&key).copied();
            if matches!(entry, Placed::Original(_)) {
                next_by_container.insert(key, row);
            }
        }
        let mut insertions: BTreeMap<usize, String> = BTreeMap::new();
        // Shapes and tables that new rows go into, by position.
        let mut filled = BTreeSet::new();
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
            let table_row = drawing(node, "tr");
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
            // Next to the nearest original row of the same shape or table.
            let container = node.parent().context("row outside a shape or table")?;
            let before = before[index].map(|row| self.rows[&row]);
            let after = after[index].map(|row| self.rows[&row]);
            ensure!(
                !table_row
                    || !after.is_some_and(|row| {
                        row.children()
                            .filter(|n| drawing(*n, "tc"))
                            .any(|cell| on(cell, "vMerge"))
                    }),
                "a row copied from row {template} would split a vertically merged cell; insert it in PowerPoint"
            );
            // When every row of the shape or table is deleted, the first one's place.
            let first = first_by_container.get(&container.range().start).copied();
            let position = before
                .map(|n| n.range().end)
                .or_else(|| after.map(|n| n.range().start))
                .or(first)
                .context("the row to copy has no place")?;
            filled.insert(container.range().start);
            insertions.entry(position).or_default().push_str(&xml);
        }
        let mut edits: Vec<(Range<usize>, String)> = insertions
            .into_iter()
            .map(|(position, xml)| (position..position, xml))
            .collect();
        let mut removed_frames = BTreeSet::new();
        // Deleted paragraphs by the position of their text body.
        let mut deleted: BTreeMap<usize, (Node<'_, '_>, Vec<usize>)> = BTreeMap::new();
        for (row, node) in &self.rows {
            if kept.contains(row) {
                continue;
            }
            ensure_row_editable(*node, *row)?;
            if drawing(*node, "tr") {
                ensure!(
                    !vertically_merged(*node),
                    "row {row} is part of a vertically merged cell; delete it in PowerPoint"
                );
                let table = node.parent().context("table row outside a table")?;
                let whole = !filled.contains(&table.range().start)
                    && !kept_tables.contains(&table.range().start);
                if whole {
                    let frame = table
                        .ancestors()
                        .find(|n| n.has_tag_name((PRESENTATION, "graphicFrame")))
                        .context("table outside a graphic frame")?;
                    if removed_frames.insert(frame.range().start) {
                        edits.push((frame.range(), String::new()));
                    }
                    continue;
                }
            } else {
                let body = node.parent().context("paragraph outside a text body")?;
                deleted
                    .entry(body.range().start)
                    .or_insert((body, vec![]))
                    .1
                    .push(edits.len());
            }
            edits.push((node.range(), String::new()));
        }
        // A text body holds at least one paragraph.
        for (start, (body, removals)) in deleted {
            let paragraphs = body.children().filter(|n| drawing(*n, "p")).count();
            if paragraphs == removals.len() && !filled.contains(&start) {
                let last = *removals.last().unwrap();
                let (_, prefix) = names(&original[edits[last].0.clone()]);
                edits[last].1 = format!("<{prefix}p/>");
            }
        }
        Ok(edits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::excel::OperationKind;

    fn slide(shapes: &str) -> String {
        format!(
            r#"<p:sld xmlns:p="{PRESENTATION}" xmlns:a="{DRAWING}"><p:cSld><p:spTree>{shapes}</p:spTree></p:cSld></p:sld>"#
        )
    }

    fn shape(paragraphs: &str) -> String {
        format!(r#"<p:sp><p:txBody><a:bodyPr/>{paragraphs}</p:txBody></p:sp>"#)
    }

    fn table(rows: &str) -> String {
        format!(
            r#"<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="4" name="Prices"/></p:nvGraphicFramePr><a:graphic><a:graphicData><a:tbl><a:tblPr firstRow="1"/><a:tblGrid><a:gridCol/><a:gridCol/></a:tblGrid>{rows}</a:tbl></a:graphicData></a:graphic></p:graphicFrame>"#
        )
    }

    fn cell(text: &str, attributes: &str) -> String {
        format!(
            r#"<a:tc{attributes}><a:txBody><a:bodyPr/><a:p><a:r><a:rPr sz="1200"/><a:t>{text}</a:t></a:r></a:p></a:txBody><a:tcPr/></a:tc>"#
        )
    }

    fn deck() -> String {
        slide(&format!(
            "{}{}{}",
            shape(r#"<a:p><a:r><a:rPr b="1"/><a:t>Title</a:t></a:r></a:p>"#),
            shape(
                r#"<a:p><a:pPr lvl="1"/><a:r><a:rPr lang="en-US"/><a:t>One</a:t></a:r><a:br><a:rPr lang="en-US"/></a:br><a:r><a:rPr lang="en-US"/><a:t>more</a:t></a:r></a:p><a:p><a:endParaRPr lang="en-US"/></a:p><a:p><a:r><a:t>Two</a:t></a:r><a:fld id="{1}" type="slidenum"><a:t>3</a:t></a:fld></a:p>"#
            ),
            table(&format!(
                r#"<a:tr h="100">{}{}</a:tr><a:tr h="100">{}{}</a:tr><a:tr h="100">{}{}</a:tr>"#,
                cell("Item", ""),
                cell("Price", ""),
                cell("Both", r#" gridSpan="2""#),
                cell("hidden", r#" hMerge="1""#),
                cell("Pen", ""),
                cell("100", ""),
            )),
        ))
    }

    fn cells(layout: &SlideLayout<'_, '_>) -> Vec<(String, String)> {
        layout
            .blocks
            .iter()
            .map(|b| (b.address.clone(), b.text.clone()))
            .collect()
    }

    fn operation(
        kind: OperationKind,
        at: u32,
        count: u32,
        style_from: Option<u32>,
    ) -> StructuralOperation {
        StructuralOperation {
            id: "op".into(),
            sheet: "slide-1".into(),
            kind,
            at,
            count,
            style_from,
        }
    }

    fn restructured(
        xml: &str,
        operations: &[StructuralOperation],
        values: &[(u32, &str, &str)],
    ) -> Result<String> {
        let document = Document::parse(xml)?;
        let layout = layout(&document)?;
        let values: InsertedText = values
            .iter()
            .map(|(offset, column, text)| {
                (("op".into(), *offset, (*column).into()), (*text).into())
            })
            .collect();
        let operations: Vec<_> = operations.iter().collect();
        let edits = layout.restructure(xml, &operations, &values)?;
        splice(xml, edits, "slide")
    }

    #[test]
    fn paragraphs_take_rows_and_tables_keep_their_grid() {
        let xml = deck();
        let document = Document::parse(&xml).unwrap();
        let layout = layout(&document).unwrap();
        assert_eq!(
            cells(&layout),
            [
                ("A1", "Title"),
                ("A2", "One\nmore"),
                ("A3", "Two3"),
                ("A4", "Item"),
                ("B4", "Price"),
                ("A5", "Both"),
                ("A6", "Pen"),
                ("B6", "100"),
            ]
            .map(|(a, t)| (a.to_owned(), t.to_owned()))
        );
        let fonts = super::super::slide_fonts::SlideFonts::new(
            &BTreeMap::new(),
            "ppt/slides/slide1.xml",
            &document,
        )
        .unwrap();
        let sheet = layout.sheet("slide-1", "ppt/slides/slide1.xml", &fonts);
        assert_eq!(sheet["merges"], json!(["A5:B5"]));
        assert_eq!(
            sheet["tables"],
            json!([{"name":"Prices","range":"A4:B6","header_rows":1,"totals_rows":0}])
        );
        assert_eq!(sheet["cells"][0]["style"]["bold"], true);
    }

    #[test]
    fn new_rows_copy_their_paragraph_or_table_row_into_the_same_shape_or_table() {
        let xml = deck();
        let result = restructured(
            &xml,
            &[
                operation(OperationKind::InsertRows, 3, 1, Some(2)),
                operation(OperationKind::InsertRows, 8, 1, None),
            ],
            &[(0, "A", "New\nline")],
        )
        .unwrap();
        assert!(
            result.contains(r#"<a:p><a:pPr lvl="1"/><a:r><a:rPr lang="en-US"/><a:t>New</a:t></a:r><a:br><a:rPr lang="en-US"/></a:br><a:r><a:rPr lang="en-US"/><a:t>line</a:t></a:r></a:p><a:p><a:endParaRPr"#),
            "{result}"
        );
        // Both operations are named op, so the new table row, which copies the
        // row above it, takes the same text in column A.
        assert!(
            result.contains(r#"<a:tr h="100"><a:tc><a:txBody><a:bodyPr/><a:p><a:r><a:rPr sz="1200"/><a:t>New</a:t></a:r><a:br><a:rPr sz="1200"/></a:br>"#),
            "{result}"
        );
        assert_eq!(result.matches("<a:tr ").count(), 4, "{result}");
    }

    #[test]
    fn deleted_rows_leave_a_shape_one_paragraph_and_take_a_whole_table() {
        let xml = deck();
        let result = restructured(
            &xml,
            &[
                operation(OperationKind::DeleteRows, 1, 1, None),
                operation(OperationKind::DeleteRows, 3, 1, None),
                operation(OperationKind::DeleteRows, 3, 1, None),
            ],
            &[],
        )
        .unwrap();
        assert!(
            result.contains(r#"<p:txBody><a:bodyPr/><a:p/></p:txBody>"#),
            "{result}"
        );
        assert_eq!(result.matches("<a:tr ").count(), 1, "{result}");
        assert!(
            !result.contains("Item") && !result.contains("Both"),
            "{result}"
        );
        let result = restructured(
            &xml,
            &[operation(OperationKind::DeleteRows, 4, 3, None)],
            &[],
        )
        .unwrap();
        assert!(!result.contains("graphicFrame"), "{result}");
        // A shape keeps its empty paragraph, which is not a row.
        let result = restructured(
            &xml,
            &[operation(OperationKind::DeleteRows, 2, 2, None)],
            &[],
        )
        .unwrap();
        assert!(
            result.contains(
                r#"<p:txBody><a:bodyPr/><a:p><a:endParaRPr lang="en-US"/></a:p></p:txBody>"#
            ),
            "{result}"
        );
    }

    #[test]
    fn rows_of_a_vertical_merge_are_not_copied_or_deleted() {
        let xml = slide(&table(&format!(
            r#"<a:tr h="1">{}</a:tr><a:tr h="1">{}</a:tr><a:tr h="1">{}</a:tr>"#,
            cell("Top", r#" rowSpan="2""#),
            cell("", r#" vMerge="1""#),
            cell("Last", ""),
        )));
        for (operations, message) in [
            (
                vec![operation(OperationKind::DeleteRows, 1, 1, None)],
                "vertically merged",
            ),
            (
                vec![operation(OperationKind::DeleteRows, 2, 1, None)],
                "vertically merged",
            ),
            (
                vec![operation(OperationKind::InsertRows, 2, 1, Some(3))],
                "split a vertically merged",
            ),
            (
                vec![operation(OperationKind::InsertRows, 4, 1, Some(1))],
                "cannot be copied",
            ),
        ] {
            let error = format!("{:#}", restructured(&xml, &operations, &[]).unwrap_err());
            assert!(error.contains(message), "{message}: {error}");
        }
    }
}
