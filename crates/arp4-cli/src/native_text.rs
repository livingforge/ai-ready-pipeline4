//! Read-only native text extraction with original byte and line locations.
use anyhow::{Result, ensure};
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::ops::Range;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TextPosition {
    pub kind: String,
    pub headings: Vec<String>,
    pub line_start: u32,
    pub line_end: u32,
    pub byte_start: usize,
    pub byte_end: usize,
    pub columns: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<u32>,
}

struct Text<'a> {
    body: &'a str,
    bom: usize,
    starts: Vec<usize>,
}

impl<'a> Text<'a> {
    fn new(raw: &'a str) -> Self {
        let body = raw.strip_prefix('\u{feff}').unwrap_or(raw);
        let mut starts = vec![0];
        let bytes = body.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'\r' {
                if bytes.get(i + 1) == Some(&b'\n') {
                    i += 1;
                }
                starts.push(i + 1);
            } else if bytes[i] == b'\n' {
                starts.push(i + 1);
            }
            i += 1;
        }
        Self {
            body,
            bom: raw.len() - body.len(),
            starts,
        }
    }

    fn position(
        &self,
        range: Range<usize>,
        kind: &str,
        headings: &[String],
        columns: Vec<String>,
    ) -> TextPosition {
        TextPosition {
            kind: kind.into(),
            headings: headings.to_vec(),
            line_start: self.starts.partition_point(|s| *s <= range.start) as u32,
            line_end: self
                .starts
                .partition_point(|s| *s <= range.end.saturating_sub(1).max(range.start))
                as u32,
            byte_start: range.start + self.bom,
            byte_end: range.end + self.bom,
            columns,
            record: None,
            column: None,
        }
    }
}

fn cell(row: usize, value: &str, position: TextPosition) -> Value {
    json!({"id":format!("text-{row}"),"address":format!("A{row}"),"type":"string",
        "value":value,"formula":null,"cached":null,"number_format":"","position":position})
}

fn sheet(cells: Vec<Value>, part: &str) -> Value {
    json!({"name":"text","part":part,"state":"visible","merges":[],"cells":cells})
}

pub fn lines(raw: &str) -> Result<Value> {
    let text = Text::new(raw);
    ensure!(
        text.starts.len() - usize::from(text.starts.last() == Some(&text.body.len())) <= 1_048_576,
        "text document exceeds line budget"
    );
    let mut cells = vec![];
    for (i, &start) in text
        .starts
        .iter()
        .enumerate()
        .take_while(|(_, start)| **start < text.body.len())
    {
        let end = text.starts.get(i + 1).copied().unwrap_or(text.body.len());
        let line = text.body[start..end].trim_end_matches(['\r', '\n']);
        cells.push(cell(
            i + 1,
            line,
            text.position(start..start + line.len(), "line", &[], vec![]),
        ));
    }
    Ok(sheet(cells, "text:lines"))
}

pub fn markdown(raw: &str) -> Result<Value> {
    let text = Text::new(raw);
    ensure!(
        text.starts.len() - usize::from(text.starts.last() == Some(&text.body.len())) <= 1_048_576,
        "text document exceeds line budget"
    );
    let parser = Parser::new_ext(
        text.body,
        Options::ENABLE_TABLES
            | Options::ENABLE_STRIKETHROUGH
            | Options::ENABLE_TASKLISTS
            | Options::ENABLE_FOOTNOTES
            | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS,
    );
    let mut spans = Vec::new();
    let mut depth = 0;
    let mut active = (0..0, "raw", None);
    let mut title = String::new();
    let mut headings: Vec<(usize, String)> = vec![];
    let mut columns = vec![];
    let mut header = false;
    let mut header_cell = None::<String>;
    for (event, range) in parser.into_offset_iter() {
        match event {
            Event::Start(tag) => {
                if depth == 0 {
                    let (kind, level) = match &tag {
                        Tag::Heading { level, .. } => ("heading", Some(*level as usize)),
                        Tag::Paragraph => ("paragraph", None),
                        Tag::List(_) => ("list", None),
                        Tag::Table(_) => ("table", None),
                        Tag::CodeBlock(_) => ("code", None),
                        Tag::BlockQuote(_) => ("quote", None),
                        Tag::HtmlBlock => ("html", None),
                        Tag::FootnoteDefinition(_) => ("footnote", None),
                        // `---` front matter is metadata, not a rule and a setext heading.
                        Tag::MetadataBlock(_) => ("front_matter", None),
                        _ => ("raw", None),
                    };
                    active = (range, kind, level);
                    title.clear();
                    columns.clear();
                }
                if matches!(tag, Tag::TableHead) {
                    header = true;
                }
                if header && matches!(tag, Tag::TableCell) {
                    header_cell = Some(String::new());
                }
                depth += 1;
            }
            Event::End(tag) => {
                if tag == TagEnd::TableCell
                    && let Some(column) = header_cell.take()
                {
                    columns.push(column);
                }
                if tag == TagEnd::TableHead {
                    header = false;
                }
                depth -= 1;
                if depth == 0 {
                    if let Some(level) = active.2 {
                        while headings.last().is_some_and(|(n, _)| *n >= level) {
                            headings.pop();
                        }
                        headings.push((level, title.clone()));
                    }
                    spans.push((
                        active.0.start..active.0.end.max(range.end),
                        active.1,
                        headings.iter().map(|(_, s)| s.clone()).collect::<Vec<_>>(),
                        columns.clone(),
                    ));
                }
            }
            Event::Text(s) | Event::Code(s) => {
                if active.2.is_some() {
                    title.push_str(&s);
                }
                if let Some(column) = &mut header_cell {
                    column.push_str(&s);
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                if active.2.is_some() {
                    title.push(' ');
                }
                if let Some(column) = &mut header_cell {
                    column.push(' ');
                }
            }
            Event::Rule if depth == 0 => spans.push((
                range,
                "rule",
                headings.iter().map(|(_, s)| s.clone()).collect(),
                vec![],
            )),
            _ => {}
        }
    }
    let mut cells = vec![];
    let mut cursor = 0;
    let mut preceding = vec![];
    // Keep non-event text (including link definitions) as evidence, too. Blank
    // lines are those CommonMark treats as blank: spaces and tabs only, so a
    // line of full-width spaces is text.
    let blank = |line: &str| line.trim_matches([' ', '\t', '\r', '\n']).is_empty();
    let mut append =
        |range: Range<usize>, kind: &str, headings: &[String], columns: Vec<String>| {
            // Leading blank lines separate the block from the one before it.
            let mut start = range.start;
            let mut next = text.starts.partition_point(|line| *line <= start);
            loop {
                if start >= range.end {
                    return;
                }
                let end = text
                    .starts
                    .get(next)
                    .copied()
                    .unwrap_or(text.body.len())
                    .min(range.end);
                if !blank(&text.body[start..end]) {
                    break;
                }
                start = end;
                next += 1;
            }
            let value = text.body[start..range.end].trim_end_matches(['\r', '\n']);
            let end = start + value.len();
            cells.push(cell(
                cells.len() + 1,
                value,
                text.position(start..end, kind, headings, columns),
            ));
        };
    for (range, kind, heading, columns) in spans {
        ensure!(
            range.start >= cursor && range.end <= text.body.len(),
            "overlapping Markdown blocks"
        );
        append(cursor..range.start, "raw", &preceding, vec![]);
        append(range.clone(), kind, &heading, columns);
        cursor = range.end;
        preceding = heading;
    }
    append(cursor..text.body.len(), "raw", &preceding, vec![]);
    Ok(sheet(cells, "markdown:blocks"))
}

// The CSV reader decodes fields; this scanner validates quoting and retains the
// exact original field spans (including quotes). Empty physical lines are skipped.
fn field_ranges(body: &str, delimiter: u8) -> Result<Vec<Vec<Range<usize>>>> {
    let bytes = body.as_bytes();
    let (mut i, mut start, mut state) = (0, 0, 0);
    let mut records = vec![];
    let mut fields = vec![];
    let mut count = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if state == 2 {
            if b == b'"' {
                if bytes.get(i + 1) == Some(&b'"') {
                    i += 2;
                    continue;
                }
                state = 3;
            }
            i += 1;
            continue;
        }
        if b == delimiter || b == b'\r' || b == b'\n' {
            fields.push(start..i);
            ensure!(
                fields.len() <= 16_384,
                "delimited text exceeds column/cell budget"
            );
            if b != delimiter {
                if fields.len() != 1 || !fields[0].is_empty() {
                    count += fields.len();
                    ensure!(
                        count <= 1_048_576,
                        "delimited text exceeds column/cell budget"
                    );
                    records.push(std::mem::take(&mut fields));
                } else {
                    fields.clear();
                }
                if b == b'\r' && bytes.get(i + 1) == Some(&b'\n') {
                    i += 1;
                }
            }
            i += 1;
            start = i;
            state = 0;
            continue;
        }
        ensure!(
            state != 3,
            "unexpected character after closing CSV quote at byte {i}"
        );
        if b == b'"' {
            ensure!(state == 0, "quote in unquoted CSV field at byte {i}");
            state = 2;
        } else {
            state = 1;
        }
        i += 1;
    }
    ensure!(state != 2, "unterminated quoted CSV field");
    if start < bytes.len() || !fields.is_empty() {
        fields.push(start..bytes.len());
        count += fields.len();
        ensure!(
            fields.len() <= 16_384 && count <= 1_048_576,
            "delimited text exceeds column/cell budget"
        );
        records.push(fields);
    }
    Ok(records)
}

pub fn delimited(raw: &str, delimiter: u8) -> Result<Value> {
    let text = Text::new(raw);
    let ranges = field_ranges(text.body, delimiter)?;
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .delimiter(delimiter)
        .flexible(false)
        .from_reader(text.body.as_bytes());
    let mut cells = vec![];
    let mut count = 0;
    for (row, record) in reader.records().enumerate() {
        let record = record?;
        ensure!(
            row < ranges.len() && record.len() == ranges[row].len(),
            "CSV field positions do not match parsed record"
        );
        for (column, value) in record.iter().enumerate() {
            let mut position = text.position(ranges[row][column].clone(), "field", &[], vec![]);
            position.record = Some(row as u32 + 1);
            position.column = Some(column as u32 + 1);
            cells.push(json!({"id":format!("record-{}-column-{}",row+1,column+1),
                "address":format!("{}{}",crate::excel::column_name(column as u32+1)?,row+1),
                "type":"string","value":value,"formula":null,"cached":null,"number_format":"","position":position}));
        }
        count += 1;
    }
    ensure!(
        count == ranges.len(),
        "CSV record positions do not match parsed input"
    );
    Ok(
        json!({"name":"records","part":"delimited:records","state":"visible","merges":[],"cells":cells}),
    )
}
