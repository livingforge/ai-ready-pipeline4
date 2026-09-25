//! Excel reference rewriting for row/column insertion and deletion.
//!
//! Ranges follow Excel: an insertion inside a range grows it, a deletion
//! shrinks it, and a fully deleted reference becomes `#REF!`.
use super::*;
use std::sync::LazyLock;

const MAX_ROW: u32 = 1_048_576;
const MAX_COLUMN: u32 = 16_384;

static REFERENCE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"^(?:(?P<c1>\$?[A-Z]{1,3})(?P<r1>\$?[1-9][0-9]{0,6})(?::(?P<c2>\$?[A-Z]{1,3})(?P<r2>\$?[1-9][0-9]{0,6}))?|(?P<cc1>\$?[A-Z]{1,3}):(?P<cc2>\$?[A-Z]{1,3})|(?P<rr1>\$?[1-9][0-9]{0,6}):(?P<rr2>\$?[1-9][0-9]{0,6}))",
    )
    .unwrap()
});
static SHEET_PREFIX: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"^(?:'(?P<quoted>(?:[^']|'')+)'|(?P<first>[\p{L}\p{N}_.\\]+)(?::(?P<last>[\p{L}\p{N}_.\\]+))?)!",
    )
    .unwrap()
});
static ERROR_LITERAL: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^#[A-Z0-9_/]+[!?]?").unwrap());

fn name_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '.' | '$' | '\\')
}

/// Operations on `sheet`, compared case-insensitively like Excel sheet names.
pub(super) fn sheet_operations<'a>(
    sheet: &str,
    operations: &'a [StructuralOperation],
) -> Vec<&'a StructuralOperation> {
    operations
        .iter()
        .filter(|operation| operation.sheet.to_lowercase() == sheet.to_lowercase())
        .collect()
}

/// Maps an inclusive span on one axis. Insertions at or before the start move
/// the span, insertions inside grow it; deletions shrink it or remove it.
pub(super) fn map_span(
    start: u32,
    end: u32,
    operations: &[&StructuralOperation],
    row: bool,
) -> Result<Option<(u32, u32)>> {
    map_span_with(start, end, operations, row, false)
}

/// Like [`map_span`], but formatting spans also take in rows/columns inserted
/// directly after them: Excel formats inserted rows like the row above and
/// inserted columns like the column to the left.
pub(super) fn map_format_span(
    start: u32,
    end: u32,
    operations: &[&StructuralOperation],
    row: bool,
) -> Result<Option<(u32, u32)>> {
    map_span_with(start, end, operations, row, true)
}

fn map_span_with(
    start: u32,
    end: u32,
    operations: &[&StructuralOperation],
    row: bool,
    formatting: bool,
) -> Result<Option<(u32, u32)>> {
    let max = if row { MAX_ROW } else { MAX_COLUMN };
    let (mut start, mut end) = (start.min(end), start.max(end));
    for operation in operations.iter().filter(|o| o.row_operation() == row) {
        let (at, count) = (operation.at, operation.count);
        if operation.insertion() {
            if at <= start {
                start = start.saturating_add(count);
                end = end.saturating_add(count).min(max);
            } else if at <= end || (formatting && at == end.saturating_add(1)) {
                end = end.saturating_add(count).min(max);
            }
            ensure!(
                start <= max,
                "row/column insertion moves a reference beyond the sheet"
            );
        } else {
            let last = at + count - 1;
            if start >= at && end <= last {
                return Ok(None);
            }
            let new_start = if start < at {
                start
            } else if start > last {
                start - count
            } else {
                at
            };
            let new_end = if end < at {
                end
            } else if end > last {
                end - count
            } else {
                at - 1
            };
            (start, end) = (new_start, new_end);
        }
    }
    Ok(Some((start, end)))
}

/// A rectangular reference; `None` on an axis means the whole row or column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Area {
    pub(super) columns: Option<(u32, u32)>,
    pub(super) rows: Option<(u32, u32)>,
}

impl Area {
    pub(super) fn parse(text: &str) -> Result<Self> {
        let reference = parse_reference(text)
            .filter(|(_, length)| *length == text.len())
            .with_context(|| format!("invalid Excel range: {text}"))?
            .0;
        Ok(reference.area())
    }

    pub(super) fn first_column(&self) -> u32 {
        self.columns.map_or(1, |(start, _)| start)
    }

    pub(super) fn first_row(&self) -> u32 {
        self.rows.map_or(1, |(start, _)| start)
    }

    pub(super) fn render(&self) -> Result<String> {
        let point = |column: Option<u32>, row: Option<u32>| -> Result<String> {
            Ok(format!(
                "{}{}",
                column.map(column_name).transpose()?.unwrap_or_default(),
                row.map(|r| r.to_string()).unwrap_or_default()
            ))
        };
        let (first, last) = (
            point(self.columns.map(|c| c.0), self.rows.map(|r| r.0))?,
            point(self.columns.map(|c| c.1), self.rows.map(|r| r.1))?,
        );
        Ok(
            if first == last && self.columns.is_some() && self.rows.is_some() {
                first
            } else {
                format!("{first}:{last}")
            },
        )
    }
}

pub(super) fn map_area(area: Area, operations: &[&StructuralOperation]) -> Result<Option<Area>> {
    map_area_with(area, operations, false)
}

/// Maps a merged range; `None` when it is deleted or shrinks to one cell,
/// which Excel no longer keeps as a merge.
pub(super) fn map_merge(area: Area, operations: &[&StructuralOperation]) -> Result<Option<Area>> {
    Ok(map_area(area, operations)?.filter(|mapped| {
        !(mapped.columns.is_some_and(|(a, b)| a == b) && mapped.rows.is_some_and(|(a, b)| a == b))
    }))
}

fn map_area_with(
    area: Area,
    operations: &[&StructuralOperation],
    formatting: bool,
) -> Result<Option<Area>> {
    let columns = match area.columns {
        Some((start, end)) => match map_span_with(start, end, operations, false, formatting)? {
            Some(span) => Some(span),
            None => return Ok(None),
        },
        None => None,
    };
    let rows = match area.rows {
        Some((start, end)) => match map_span_with(start, end, operations, true, formatting)? {
            Some(span) => Some(span),
            None => return Ok(None),
        },
        None => None,
    };
    Ok(Some(Area { columns, rows }))
}

/// Maps a space-separated `sqref` list; fully deleted ranges are dropped.
/// `formatting` ranges (conditional formats, validations) grow into rows and
/// columns inserted directly after them, as Excel formats them alike.
pub(super) fn map_sqref(
    text: &str,
    operations: &[&StructuralOperation],
    formatting: bool,
) -> Result<Option<String>> {
    let mut mapped = vec![];
    for part in text.split_whitespace() {
        if let Some(area) = map_area_with(Area::parse(part)?, operations, formatting)? {
            mapped.push(area.render()?);
        }
    }
    Ok((!mapped.is_empty()).then(|| mapped.join(" ")))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Coordinate {
    index: u32,
    absolute: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Point {
    column: Option<Coordinate>,
    row: Option<Coordinate>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Reference {
    first: Point,
    second: Option<Point>,
}

impl Reference {
    fn last(&self) -> Point {
        self.second.unwrap_or(self.first)
    }

    fn area(&self) -> Area {
        let span = |a: Option<Coordinate>, b: Option<Coordinate>| {
            a.zip(b)
                .map(|(a, b)| (a.index.min(b.index), a.index.max(b.index)))
        };
        Area {
            columns: span(self.first.column, self.last().column),
            rows: span(self.first.row, self.last().row),
        }
    }

    fn with_area(&self, area: Area) -> Self {
        let point = |point: Point, column: Option<u32>, row: Option<u32>| Point {
            column: point.column.zip(column).map(|(c, index)| Coordinate {
                index,
                absolute: c.absolute,
            }),
            row: point.row.zip(row).map(|(r, index)| Coordinate {
                index,
                absolute: r.absolute,
            }),
        };
        let first = point(
            self.first,
            area.columns.map(|c| c.0),
            area.rows.map(|r| r.0),
        );
        Self {
            first,
            second: self
                .second
                .map(|second| point(second, area.columns.map(|c| c.1), area.rows.map(|r| r.1))),
        }
    }

    fn render(&self) -> Result<String> {
        let point = |point: Point| -> Result<String> {
            let mut text = String::new();
            if let Some(column) = point.column {
                if column.absolute {
                    text.push('$');
                }
                text.push_str(&column_name(column.index)?);
            }
            if let Some(row) = point.row {
                if row.absolute {
                    text.push('$');
                }
                text.push_str(&row.index.to_string());
            }
            Ok(text)
        };
        Ok(match self.second {
            Some(second) => format!("{}:{}", point(self.first)?, point(second)?),
            None => point(self.first)?,
        })
    }
}

fn coordinate_part(text: &str, row: bool) -> Option<Coordinate> {
    let absolute = text.starts_with('$');
    let text = text.trim_start_matches('$');
    let index = if row {
        text.parse().ok().filter(|r| (1..=MAX_ROW).contains(r))?
    } else {
        column_number(text).ok()?
    };
    Some(Coordinate { index, absolute })
}

/// Parses a reference at the start of `text`, returning it and its length.
fn parse_reference(text: &str) -> Option<(Reference, usize)> {
    let captures = REFERENCE.captures(text)?;
    let part = |name: &str, row: bool| {
        captures
            .name(name)
            .map(|m| coordinate_part(m.as_str(), row))
    };
    let reference = if let Some(column) = part("c1", false) {
        let first = Point {
            column: Some(column?),
            row: Some(part("r1", true)??),
        };
        let second = match part("c2", false) {
            Some(column) => Some(Point {
                column: Some(column?),
                row: Some(part("r2", true)??),
            }),
            None => None,
        };
        Reference { first, second }
    } else if let Some(column) = part("cc1", false) {
        Reference {
            first: Point {
                column: Some(column?),
                row: None,
            },
            second: Some(Point {
                column: Some(part("cc2", false)??),
                row: None,
            }),
        }
    } else {
        Reference {
            first: Point {
                column: None,
                row: Some(part("rr1", true)??),
            },
            second: Some(Point {
                column: None,
                row: Some(part("rr2", true)??),
            }),
        }
    };
    Some((reference, captures.get(0)?.end()))
}

/// Whether a reference ends here rather than continuing as a name or call.
fn reference_ends(rest: &str) -> bool {
    rest.chars()
        .next()
        .is_none_or(|c| !(name_char(c) || matches!(c, '(' | '[' | '!')))
}

enum Replacement {
    Keep,
    Text(String),
    Deleted,
}

/// Where a reference points, as far as rewriting is concerned.
enum Target {
    /// No sheet prefix: the formula's own sheet.
    Local,
    Sheet(String),
    /// 3D or external-workbook reference: never rewritten, only shifted.
    Opaque,
}

/// Calls `visit` for every cell, range, column or row reference outside string
/// literals, structured references and error values.
fn transform(
    formula: &str,
    mut visit: impl FnMut(&Target, Reference) -> Result<Replacement>,
) -> Result<String> {
    let mut output = String::with_capacity(formula.len());
    let mut index = 0;
    let mut external = false;
    let mut emit = |output: &mut String,
                    target: &Target,
                    prefix: &str,
                    raw: &str,
                    reference: Reference|
     -> Result<()> {
        match visit(target, reference)? {
            Replacement::Keep => output.push_str(&format!("{prefix}{raw}")),
            Replacement::Text(text) => output.push_str(&format!("{prefix}{text}")),
            Replacement::Deleted => output.push_str(&format!("{prefix}#REF!")),
        }
        Ok(())
    };
    while index < formula.len() {
        let rest = &formula[index..];
        let c = rest.chars().next().unwrap();
        let previous_is_name = formula[..index].chars().next_back().is_some_and(name_char);
        if c == '"' {
            let mut end = index + 1;
            loop {
                match formula[end..].find('"') {
                    Some(offset) if formula[end + offset + 1..].starts_with('"') => {
                        end += offset + 2
                    }
                    Some(offset) => {
                        end += offset + 1;
                        break;
                    }
                    None => {
                        end = formula.len();
                        break;
                    }
                }
            }
            output.push_str(&formula[index..end]);
            index = end;
            external = false;
        } else if c == '[' {
            let mut depth = 0;
            let mut end = formula.len();
            for (offset, ch) in rest.char_indices() {
                match ch {
                    '[' => depth += 1,
                    ']' => {
                        depth -= 1;
                        if depth == 0 {
                            end = index + offset + 1;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            output.push_str(&formula[index..end]);
            index = end;
            external = true;
        } else if c == '#' {
            let length = ERROR_LITERAL.find(rest).map_or(1, |m| m.end());
            output.push_str(&rest[..length]);
            index += length;
            external = false;
        } else if (c == '\'' || name_char(c)) && !previous_is_name {
            if let Some(prefix) = SHEET_PREFIX.captures(rest) {
                let prefix_text = prefix.get(0).unwrap().as_str();
                let after = &rest[prefix_text.len()..];
                match parse_reference(after).filter(|(_, n)| reference_ends(&after[*n..])) {
                    Some((reference, length)) => {
                        let quoted = prefix.name("quoted").map(|m| m.as_str());
                        let opaque = external
                            || prefix.name("last").is_some()
                            || quoted.is_some_and(|q| q.starts_with('[') || q.contains(':'));
                        let target = if opaque {
                            Target::Opaque
                        } else {
                            Target::Sheet(match quoted {
                                Some(q) => q.replace("''", "'"),
                                None => prefix["first"].to_owned(),
                            })
                        };
                        emit(
                            &mut output,
                            &target,
                            prefix_text,
                            &after[..length],
                            reference,
                        )?;
                        index += prefix_text.len() + length;
                    }
                    None => {
                        output.push_str(prefix_text);
                        index += prefix_text.len();
                    }
                }
            } else if c == '\'' {
                output.push(c);
                index += 1;
            } else if let Some((reference, length)) =
                parse_reference(rest).filter(|(_, n)| reference_ends(&rest[*n..]))
            {
                let target = if external {
                    Target::Opaque
                } else {
                    Target::Local
                };
                emit(&mut output, &target, "", &rest[..length], reference)?;
                index += length;
            } else {
                let length = rest
                    .char_indices()
                    .find(|(_, ch)| !name_char(*ch))
                    .map_or(rest.len(), |(offset, _)| offset);
                output.push_str(&rest[..length]);
                index += length;
            }
            external = false;
        } else {
            output.push(c);
            index += c.len_utf8();
            external = false;
        }
    }
    Ok(output)
}

/// Rewrites references for row/column operations. `current_sheet` resolves
/// unprefixed references; `None` leaves them unchanged (e.g. defined names).
pub(super) fn rewrite_references(
    formula: &str,
    current_sheet: Option<&str>,
    operations: &[StructuralOperation],
) -> Result<String> {
    transform(formula, |target, reference| {
        let sheet = match target {
            Target::Local => match current_sheet {
                Some(sheet) => sheet.to_owned(),
                None => return Ok(Replacement::Keep),
            },
            Target::Sheet(sheet) => sheet.clone(),
            Target::Opaque => return Ok(Replacement::Keep),
        };
        let operations = sheet_operations(&sheet, operations);
        if operations.is_empty() {
            return Ok(Replacement::Keep);
        }
        Ok(match map_area(reference.area(), &operations)? {
            Some(area) => Replacement::Text(reference.with_area(area).render()?),
            None => Replacement::Deleted,
        })
    })
}

/// Row/column operations together with the table columns they delete.
/// Structured references to deleted columns become `#REF!`, as in Excel.
pub(super) struct Moves<'a> {
    pub(super) operations: &'a [StructuralOperation],
    /// Lowercase table name -> lowercase names of its deleted columns.
    removed_columns: BTreeMap<String, BTreeSet<String>>,
}

impl<'a> Moves<'a> {
    pub(super) fn new(
        operations: &'a [StructuralOperation],
        removed_columns: BTreeMap<String, BTreeSet<String>>,
    ) -> Self {
        Self {
            operations,
            removed_columns,
        }
    }

    pub(super) fn rewrite(&self, formula: &str, current_sheet: Option<&str>) -> Result<String> {
        if self.removed_columns.is_empty() {
            return rewrite_references(formula, current_sheet, self.operations);
        }
        let formula = remove_structured_references(formula, &self.removed_columns);
        rewrite_references(&formula, current_sheet, self.operations)
    }
}

/// End of a bracket group starting at `start`, honouring the `'` escape used
/// inside structured references.
fn bracket_end(text: &str, start: usize) -> usize {
    let mut depth = 0;
    let mut escaped = false;
    for (offset, c) in text[start..].char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match c {
            '\'' => escaped = true,
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    return start + offset + 1;
                }
            }
            _ => {}
        }
    }
    text.len()
}

/// Column names named by a structured reference body (`[Col]`, `[[#This Row],[Col]]`,
/// `[[Col1]:[Col2]]`), unescaped and lowercased; special items are skipped.
fn structured_columns(body: &str) -> Vec<String> {
    let inner = &body[1..body.len().saturating_sub(1).max(1)];
    let items: Vec<&str> = if inner.starts_with('[') {
        let mut items = vec![];
        let mut index = 0;
        while let Some(offset) = inner[index..].find('[') {
            let start = index + offset;
            let end = bracket_end(inner, start);
            items.push(&inner[start + 1..end.saturating_sub(1).max(start + 1)]);
            index = end;
        }
        items
    } else {
        vec![inner]
    };
    items
        .into_iter()
        .filter(|item| !item.trim_start().starts_with('#'))
        .map(|item| {
            let mut name = String::new();
            let mut escaped = false;
            for c in item.chars() {
                if c == '\'' && !escaped {
                    escaped = true;
                    continue;
                }
                escaped = false;
                name.push(c);
            }
            name.to_lowercase()
        })
        .collect()
}

fn remove_structured_references(
    formula: &str,
    removed: &BTreeMap<String, BTreeSet<String>>,
) -> String {
    let mut output = String::with_capacity(formula.len());
    let mut index = 0;
    while index < formula.len() {
        let rest = &formula[index..];
        let c = rest.chars().next().unwrap();
        if c == '"' {
            let mut end = index + 1;
            loop {
                match formula[end..].find('"') {
                    Some(offset) if formula[end + offset + 1..].starts_with('"') => {
                        end += offset + 2
                    }
                    Some(offset) => {
                        end += offset + 1;
                        break;
                    }
                    None => {
                        end = formula.len();
                        break;
                    }
                }
            }
            output.push_str(&formula[index..end]);
            index = end;
            continue;
        }
        let previous_is_name = formula[..index].chars().next_back().is_some_and(name_char);
        if name_char(c) && !previous_is_name {
            let length = rest
                .char_indices()
                .find(|(_, ch)| !name_char(*ch))
                .map_or(rest.len(), |(offset, _)| offset);
            let word = &rest[..length];
            if rest[length..].starts_with('[')
                && let Some(columns) = removed.get(&word.to_lowercase())
            {
                let end = bracket_end(formula, index + length);
                let body = &formula[index + length..end];
                if structured_columns(body)
                    .iter()
                    .any(|column| columns.contains(column))
                {
                    output.push_str("#REF!");
                } else {
                    output.push_str(&formula[index..end]);
                }
                index = end;
                continue;
            }
            output.push_str(word);
            index += length;
            continue;
        }
        if c == '[' {
            let end = bracket_end(formula, index);
            output.push_str(&formula[index..end]);
            index = end;
            continue;
        }
        output.push(c);
        index += c.len_utf8();
    }
    output
}

/// Moves relative references by the given offsets, as Excel derives each
/// cell of a shared formula from its master cell.
pub(super) fn shift_relative(formula: &str, rows: i64, columns: i64) -> Result<String> {
    transform(formula, |_, reference| {
        let shift = |coordinate: Option<Coordinate>, delta: i64, max: u32| {
            coordinate.map(|c| {
                if c.absolute {
                    return Some(c);
                }
                let index = i64::from(c.index) + delta;
                (1..=i64::from(max)).contains(&index).then_some(Coordinate {
                    index: index as u32,
                    absolute: false,
                })
            })
        };
        let point = |p: Point| -> Option<Point> {
            Some(Point {
                column: shift(p.column, columns, MAX_COLUMN).map_or(Some(None), |c| c.map(Some))?,
                row: shift(p.row, rows, MAX_ROW).map_or(Some(None), |r| r.map(Some))?,
            })
        };
        let shifted = point(reference.first).and_then(|first| {
            Some(Reference {
                first,
                second: match reference.second {
                    Some(second) => Some(point(second)?),
                    None => None,
                },
            })
        });
        Ok(match shifted {
            Some(shifted) => Replacement::Text(shifted.render()?),
            None => Replacement::Deleted,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn operation(sheet: &str, kind: OperationKind, at: u32, count: u32) -> StructuralOperation {
        StructuralOperation {
            id: "op".into(),
            sheet: sheet.into(),
            kind,
            at,
            count,
            style_from: None,
        }
    }

    #[test]
    fn spans_grow_shrink_and_disappear_like_excel() {
        let insert = operation("S", OperationKind::InsertRows, 3, 2);
        let delete = operation("S", OperationKind::DeleteRows, 3, 2);
        let map = |start, end, op: &StructuralOperation| map_span(start, end, &[op], true).unwrap();
        assert_eq!(map(1, 2, &insert), Some((1, 2)));
        assert_eq!(map(3, 5, &insert), Some((5, 7)));
        assert_eq!(map(1, 5, &insert), Some((1, 7)));
        assert_eq!(map(1, 5, &delete), Some((1, 3)));
        assert_eq!(map(3, 6, &delete), Some((3, 4)));
        assert_eq!(map(2, 3, &delete), Some((2, 2)));
        assert_eq!(map(3, 4, &delete), None);
        assert_eq!(map(6, 8, &delete), Some((4, 6)));
    }

    #[test]
    fn formulas_rewrite_ranges_whole_axes_and_other_sheets() {
        let ops = [operation("Data", OperationKind::DeleteRows, 3, 2)];
        let rewrite = |f: &str, sheet| rewrite_references(f, sheet, &ops).unwrap();
        assert_eq!(rewrite("SUM(A1:A5)+A6", Some("Data")), "SUM(A1:A3)+A4");
        assert_eq!(rewrite("A3+$B$4*2", Some("Data")), "#REF!+#REF!*2");
        assert_eq!(rewrite("Data!$1:$4", None), "Data!$1:$2");
        assert_eq!(rewrite("SUM(A:A)", Some("Data")), "SUM(A:A)");
        assert_eq!(
            rewrite("Data!B6+'Data'!C7+A6", Some("Other")),
            "Data!B4+'Data'!C5+A6"
        );
        assert_eq!(rewrite("data!B6", None), "data!B4");
        assert_eq!(rewrite("Data!A3:A4", None), "Data!#REF!");
    }

    #[test]
    fn formulas_skip_strings_names_functions_and_external_references() {
        let ops = [operation("S", OperationKind::InsertColumns, 1, 1)];
        let rewrite = |f: &str| rewrite_references(f, Some("S"), &ops).unwrap();
        assert_eq!(rewrite(r#"LOG10(A1)&"A1""#), r#"LOG10(B1)&"A1""#);
        assert_eq!(
            rewrite("Table1[Col]+TBL1[[#This Row],[A1]]"),
            "Table1[Col]+TBL1[[#This Row],[A1]]"
        );
        assert_eq!(
            rewrite("[1]S!A1+S1:S3!A1+'[2]S'!A1"),
            "[1]S!A1+S1:S3!A1+'[2]S'!A1"
        );
        assert_eq!(
            rewrite("MyName+R1C1+ATAN2(A1,1E5)"),
            "MyName+R1C1+ATAN2(B1,1E5)"
        );
        assert_eq!(rewrite("#REF!+S!#REF!+A1#"), "#REF!+S!#REF!+B1#");
        assert_eq!(rewrite("A:C 1:2"), "B:D 1:2");
    }

    #[test]
    fn shared_formula_offsets_move_only_relative_parts() {
        assert_eq!(
            shift_relative("A1+$A$1+A$1+S!B2", 2, 1).unwrap(),
            "B3+$A$1+B$1+S!C4"
        );
        assert_eq!(shift_relative("A1", -1, 0).unwrap(), "#REF!");
    }

    #[test]
    fn structured_references_to_deleted_table_columns_become_ref_errors() {
        let removed = BTreeMap::from([(
            "sales".to_owned(),
            BTreeSet::from(["qty".to_owned(), "a]b".to_owned()]),
        )]);
        let ops = [];
        let moves = Moves::new(&ops, removed);
        let rewrite = |f: &str| moves.rewrite(f, Some("S")).unwrap();
        assert_eq!(rewrite("SUM(Sales[Qty])+1"), "SUM(#REF!)+1");
        assert_eq!(
            rewrite("Sales[[#This Row],[Qty]]*Sales[[#This Row],[Price]]"),
            "#REF!*Sales[[#This Row],[Price]]"
        );
        assert_eq!(rewrite("SALES[[Price]:[QTY]]"), "#REF!");
        assert_eq!(rewrite("Sales[[#This Row],[A']b]]"), "#REF!");
        assert_eq!(
            rewrite(r#"Other[Qty]&"Sales[Qty]"&Sales[#All]"#),
            r#"Other[Qty]&"Sales[Qty]"&Sales[#All]"#
        );
    }

    #[test]
    fn sqref_and_area_rendering() {
        let ops = [operation("S", OperationKind::DeleteColumns, 2, 1)];
        let ops: Vec<_> = ops.iter().collect();
        assert_eq!(
            map_sqref("A1:C3 B5 D1", &ops, false).unwrap().as_deref(),
            Some("A1:B3 C1")
        );
        assert_eq!(map_sqref("B1:B9", &ops, false).unwrap(), None);
        let insert = [operation("S", OperationKind::InsertColumns, 3, 1)];
        let insert: Vec<_> = insert.iter().collect();
        assert_eq!(
            map_sqref("B1:B9", &insert, false).unwrap().as_deref(),
            Some("B1:B9")
        );
        assert_eq!(
            map_sqref("B1:B9", &insert, true).unwrap().as_deref(),
            Some("B1:C9")
        );
        assert_eq!(map_format_span(3, 4, &insert, false).unwrap(), Some((4, 5)));
        assert_eq!(Area::parse("$B$2:$C$3").unwrap().render().unwrap(), "B2:C3");
    }
}
