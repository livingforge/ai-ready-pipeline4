//! Word text laid out on a grid. Paragraphs outside tables take one row each in
//! column A; a table keeps its rows and grid columns, reports merged cells as
//! merges and its range as an explicit table. A block's text joins the text of
//! its paragraphs, and `segments` map that text back to the `w:t` elements so
//! that an edit changes only the runs it touches.
use super::*;
use crate::excel::column_name;
use std::ops::Range;

pub(super) struct Segment<'a, 'input> {
    /// The text element, or None for a paragraph break, line break or tab.
    pub node: Option<Node<'a, 'input>>,
    pub range: Range<usize>,
}

pub(super) struct Block<'a, 'input> {
    pub address: String,
    pub text: String,
    pub segments: Vec<Segment<'a, 'input>>,
    bold: bool,
    fill: bool,
    in_table: bool,
}

pub(super) struct Layout<'a, 'input> {
    pub blocks: Vec<Block<'a, 'input>>,
    merges: Vec<String>,
    tables: Vec<Value>,
}

impl Layout<'_, '_> {
    pub fn sheet(&self, name: &str, part: &str) -> Value {
        let cells: Vec<_> = self
            .blocks
            .iter()
            .enumerate()
            .map(|(i, b)| {
                json!({"id":format!("text-{}",i+1),"address":b.address,"type":"string","value":b.text,
                    "formula":null,"cached":null,"number_format":"",
                    "style":{"bold":b.bold,"fill":u8::from(b.fill),"border":u8::from(b.in_table)}})
            })
            .collect();
        json!({"name":name,"part":part,"state":"visible","merges":self.merges,"cells":cells,"tables":self.tables})
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

#[derive(Default)]
struct Text<'a, 'input> {
    text: String,
    segments: Vec<Segment<'a, 'input>>,
    runs: usize,
    bold_runs: usize,
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

    /// Appends the visible text of one paragraph; nested paragraphs (text boxes)
    /// are blocks of their own.
    fn paragraph(&mut self, paragraph: Node<'a, 'input>) {
        let mark = (self.text.len(), self.segments.len());
        if !self.text.is_empty() {
            self.push(None, "\n");
        }
        let before = self.text.len();
        for node in paragraph
            .descendants()
            .filter(|n| nearest(*n, "p") == Some(paragraph) && !hidden_copy(*n, "docx"))
        {
            let in_run = node.parent().is_some_and(|p| word_element(p, "r"));
            if word_element(node, "t") {
                let text = node.text().unwrap_or("");
                if !text.is_empty() {
                    self.runs += 1;
                    if on(node.parent().and_then(|r| property(r, "rPr", "b"))) {
                        self.bold_runs += 1;
                    }
                }
                self.push(Some(node), text);
            } else if in_run && word_element(node, "tab") {
                self.push(None, "\t");
            } else if in_run && (word_element(node, "br") || word_element(node, "cr")) {
                self.push(None, "\n");
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
    };
    let mut row = 1;
    for node in xml.descendants().filter(|n| !hidden_copy(*n, "docx")) {
        if word_element(node, "tbl") && nearest(node, "tbl").is_none() {
            row += table(node, row, &mut output)?;
        } else if word_element(node, "p") && nearest(node, "tc").is_none() {
            let mut text = Text::default();
            text.paragraph(node);
            if !text.text.is_empty() {
                output
                    .blocks
                    .push(text.block(format!("A{row}"), false, false));
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
) -> Result<usize> {
    let rows: Vec<_> = table
        .descendants()
        .filter(|n| word_element(*n, "tr") && nearest(*n, "tbl") == Some(table))
        .collect();
    let mut cells = vec![];
    for (r, tr) in rows.iter().enumerate() {
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
    for cell in &cells {
        let mut bottom = cell.row;
        if cell.restart {
            while cells
                .iter()
                .any(|c| c.row == bottom + 1 && c.column == cell.column && c.continued)
            {
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
        let mut text = Text::default();
        for paragraph in cell
            .node
            .descendants()
            .filter(|n| word_element(*n, "p") && !hidden_copy(*n, "docx"))
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
    let emphasized = (0..rows.len())
        .take_while(|r| {
            let row: Vec<_> = emphasis.iter().filter(|(row, _)| row == r).collect();
            !row.is_empty() && row.iter().all(|(_, emphasized)| *emphasized)
        })
        .count();
    if emphasized > 0 && emphasized < rows.len() {
        emphasized
    } else if width >= 3 && rows.len() >= 2 {
        1
    } else {
        0
    }
}

/// Maps an edit of a block's text to new text for the elements it touches. The
/// change between the old and new text must stay within one paragraph; when it
/// spans runs, the first run takes the new text as Word does when typing over
/// a selection.
pub(super) fn edit<'a, 'input>(
    block: &Block<'a, 'input>,
    after: &str,
) -> Result<Vec<(Node<'a, 'input>, String)>> {
    let before = block.text.as_str();
    let prefix: usize = before
        .chars()
        .zip(after.chars())
        .take_while(|(a, b)| a == b)
        .map(|(a, _)| a.len_utf8())
        .sum();
    let suffix: usize = before[prefix..]
        .chars()
        .rev()
        .zip(after[prefix..].chars().rev())
        .take_while(|(a, b)| a == b)
        .map(|(a, _)| a.len_utf8())
        .sum();
    let (start, end) = (prefix, before.len() - suffix);
    let inserted = &after[prefix..after.len() - suffix];
    if start == end && inserted.is_empty() {
        return Ok(vec![]);
    }
    ensure!(
        !inserted.contains(['\n', '\r', '\t']),
        "Word text replacement cannot add line breaks or tabs; edit existing paragraphs separately"
    );
    let touched: Vec<_> = if start == end {
        // An insertion joins the run ending there, else the run starting there.
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
    };
    ensure!(
        !touched.is_empty() && touched.iter().all(|s| s.node.is_some()),
        "the edit crosses a paragraph, line break or tab in {}; edit each part separately",
        block.address
    );
    let last = touched.len() - 1;
    Ok(touched
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
            (s.node.unwrap(), new)
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

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
            .map(|(node, new)| (node.text().unwrap_or("").to_owned(), new))
            .collect())
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
        for after in [
            "受注番号二行目",
            "受注番号\n二行目\n三行目",
            "受注\t番号\n二行目",
        ] {
            let error = edits(split, after).unwrap_err().to_string();
            assert!(
                error.contains("crosses") || error.contains("line breaks"),
                "{after}: {error}"
            );
        }
    }
}
