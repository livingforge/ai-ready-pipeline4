//! Excel reference rewriting for row/column insertion and deletion.
//!
//! Ranges follow Excel: an insertion inside a range grows it, a deletion
//! shrinks it, and a fully deleted reference becomes `#REF!`.
use super::*;
use std::sync::LazyLock;

pub(super) const MAX_ROW: u32 = 1_048_576;
pub(super) const MAX_COLUMN: u32 = 16_384;

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
    let (mut start, mut end) = (start.min(end), start.max(end));
    // Cells shifted in a band move no whole row or column.
    for operation in operations
        .iter()
        .filter(|o| o.row_operation() == row && o.whole())
    {
        match span_through(start, end, operation, formatting)? {
            Some(span) => (start, end) = span,
            None => return Ok(None),
        }
    }
    Ok(Some((start, end)))
}

/// Maps an inclusive span on the axis of one operation, as [`map_span`].
fn span_through(
    start: u32,
    end: u32,
    operation: &StructuralOperation,
    formatting: bool,
) -> Result<Option<(u32, u32)>> {
    let row = operation.row_operation();
    let max = if row { MAX_ROW } else { MAX_COLUMN };
    let (mut start, mut end) = (start, end);
    let (at, count) = (operation.at, operation.count);
    if let OperationKind::MoveColumns { to } = operation.kind {
        let mapped: Vec<_> = (start..=end)
            .map(|column| moved_column(column, at, count, to))
            .collect();
        let first = *mapped.first().context("empty moved range")?;
        let last = *mapped.last().context("empty moved range")?;
        ensure!(
            mapped.windows(2).all(|pair| pair[1] == pair[0] + 1)
                && last.checked_sub(first) == Some(end - start),
            "column move makes a reference non-contiguous or changes its element order; edit in Excel"
        );
        return Ok(Some((first, last)));
    }
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
/// which Excel no longer keeps as a merge. Cells shifted in a band through
/// part of a merge are refused: Excel would take the merge apart.
pub(super) fn map_merge(area: Area, operations: &[&StructuralOperation]) -> Result<Option<Area>> {
    let mut areas = vec![area];
    for operation in operations {
        areas = areas_through(areas, operation, false, Straddle::Refuse)?;
    }
    Ok(areas.pop().filter(|mapped| {
        !(mapped.columns.is_some_and(|(a, b)| a == b) && mapped.rows.is_some_and(|(a, b)| a == b))
    }))
}

/// The ranges the cells of the format range `area` take after `operations`:
/// one, or the parts cells shifted within a band split it into, as Excel
/// moves their formats with them. A `merge` such a shift reaches only part of
/// is refused: Excel would take it apart.
pub(super) fn map_format_area(
    area: Area,
    operations: &[&StructuralOperation],
    merge: bool,
) -> Result<Vec<Area>> {
    let straddle = || {
        if merge {
            Straddle::Refuse
        } else {
            Straddle::Split
        }
    };
    let mut areas = vec![area];
    for operation in operations {
        areas = areas_through(areas, operation, false, straddle())?;
    }
    Ok(areas)
}

fn map_area_with(
    area: Area,
    operations: &[&StructuralOperation],
    formatting: bool,
) -> Result<Option<Area>> {
    let mut areas = vec![area];
    for operation in operations {
        areas = areas_through(areas, operation, formatting, Straddle::Keep)?;
    }
    Ok(areas.pop())
}

/// Maps a space-separated `sqref` list; fully deleted ranges are dropped.
/// `formatting` ranges (conditional formats, validations) grow into rows and
/// columns inserted directly after them, as Excel formats them alike. A range
/// cells shifted in a band reach part of is split, as Excel splits it.
pub(super) fn map_sqref(
    text: &str,
    operations: &[&StructuralOperation],
    formatting: bool,
) -> Result<Option<String>> {
    let mut mapped = vec![];
    for part in text.split_whitespace() {
        let mut areas = vec![Area::parse(part)?];
        for operation in operations {
            areas = areas_through(areas, operation, formatting, Straddle::Split)?;
        }
        for area in areas {
            mapped.push(area.render()?);
        }
    }
    Ok((!mapped.is_empty()).then(|| mapped.join(" ")))
}

/// The operations as the range `area` meets them, in order: whole rows and
/// columns as they are, cells shifted in a band that holds the range across
/// as whole rows or columns, and those that miss it or end before it left
/// out. A band reaching only part of the range is refused, as Excel refuses
/// to move part of `what`.
pub(super) fn operations_meeting(
    area: Area,
    operations: &[&StructuralOperation],
    what: &str,
) -> Result<Vec<StructuralOperation>> {
    let mut current = area;
    let mut met = vec![];
    for operation in operations {
        let whole = match operation.band {
            None => Some((*operation).clone()),
            Some((first, last)) => {
                let rows = operation.row_operation();
                let along = if rows { current.rows } else { current.columns };
                let across = if rows { current.columns } else { current.rows };
                let max = if rows { MAX_COLUMN } else { MAX_ROW };
                let (low, high) = across.unwrap_or((1, max));
                let reached = along.is_some_and(|(_, end)| end >= operation.at);
                if !reached || high < first || low > last {
                    None
                } else {
                    ensure!(
                        first <= low && high <= last,
                        "the cells {} shifts reach only part of {what} {}; Excel does not move part of it, so shift cells across all of it",
                        operation.id,
                        current.render()?
                    );
                    Some(StructuralOperation {
                        band: None,
                        ..(*operation).clone()
                    })
                }
            }
        };
        if let Some(whole) = whole {
            current = map_area(current, &[&whole])?.unwrap_or(current);
            met.push(whole);
        }
    }
    Ok(met)
}

/// The operations moving the range `area` as whole rows or columns, in
/// order: whole ones, and cells shifted in a band holding it across. Bands
/// missing it, ending before it or holding only part of it are left out.
pub(super) fn operations_holding(
    area: Area,
    operations: &[&StructuralOperation],
) -> Result<Vec<StructuralOperation>> {
    let mut current = area;
    let mut held = vec![];
    for operation in operations {
        let whole = match operation.band {
            None => Some((*operation).clone()),
            Some((first, last)) => {
                let rows = operation.row_operation();
                let across = if rows { current.columns } else { current.rows };
                let max = if rows { MAX_COLUMN } else { MAX_ROW };
                let (low, high) = across.unwrap_or((1, max));
                (first <= low && high <= last).then(|| StructuralOperation {
                    band: None,
                    ..(*operation).clone()
                })
            }
        };
        if let Some(whole) = whole {
            if let Some(moved) = map_area(current, &[&whole])? {
                current = moved;
            }
            held.push(whole);
        }
    }
    Ok(held)
}

/// What becomes of a range that cells shifted in a band reach only part of.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Straddle {
    /// A reference stays as written, as Excel keeps it.
    Keep,
    /// Formatting ranges split into what stays and what moves.
    Split,
    /// A merge is refused: Excel takes it apart.
    Refuse,
}

/// Maps `areas` through one operation. Whole rows or columns map each area
/// on the operation's axis. Cells shifted in a band move an area the band
/// holds across, leave alone one it misses or that ends before them, and
/// treat one it holds only part of by `straddle`.
fn areas_through(
    areas: Vec<Area>,
    operation: &StructuralOperation,
    formatting: bool,
    straddle: Straddle,
) -> Result<Vec<Area>> {
    let rows = operation.row_operation();
    let along = |area: &Area| if rows { area.rows } else { area.columns };
    let across = |area: &Area| if rows { area.columns } else { area.rows };
    let with = |along: (u32, u32), across: Option<(u32, u32)>| {
        if rows {
            Area {
                rows: Some(along),
                columns: across,
            }
        } else {
            Area {
                columns: Some(along),
                rows: across,
            }
        }
    };
    let mut mapped = Vec::with_capacity(areas.len());
    for area in areas {
        let Some((start, end)) = along(&area) else {
            mapped.push(area);
            continue;
        };
        let Some((first, last)) = operation.band else {
            if let Some(span) = span_through(start, end, operation, formatting)? {
                mapped.push(with(span, across(&area)));
            }
            continue;
        };
        let max = if rows { MAX_COLUMN } else { MAX_ROW };
        let (low, high) = across(&area).unwrap_or((1, max));
        let reached = end >= operation.at
            || formatting && operation.insertion() && end.saturating_add(1) == operation.at;
        if !reached || high < first || low > last {
            mapped.push(area);
            continue;
        }
        if first <= low && high <= last {
            if let Some(span) = span_through(start, end, operation, formatting)? {
                mapped.push(with(span, across(&area)));
            }
            continue;
        }
        match straddle {
            Straddle::Refuse => bail!(
                "the cells {} shifts reach only part of the merged range {}, which Excel would take apart; take the merge apart first (documents format --unmerge), or shift cells across the whole merge",
                operation.id,
                area.render()?
            ),
            Straddle::Split => {
                // The parts outside the band stay; the part in it moves.
                if low < first {
                    mapped.push(with((start, end), Some((low, first - 1))));
                }
                if high > last {
                    mapped.push(with((start, end), Some((last + 1, high))));
                }
                let inside = (low.max(first), high.min(last));
                if let Some(span) = span_through(start, end, operation, formatting)? {
                    mapped.push(with(span, Some(inside)));
                }
            }
            Straddle::Keep => {
                // A reference whose cells in the band are all deleted loses
                // them when they lie at its edge, as Excel shrinks it.
                let deleted = !operation.insertion()
                    && start >= operation.at
                    && end < operation.at + operation.count;
                if deleted && low < first && high <= last {
                    mapped.push(with((start, end), Some((low, first - 1))));
                } else if deleted && first <= low && high > last {
                    mapped.push(with((start, end), Some((last + 1, high))));
                } else {
                    mapped.push(area);
                }
            }
        }
    }
    Ok(mapped)
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
/// Whether all of `text` reads as a cell, range, row or column reference,
/// in any case.
pub(super) fn reads_as_reference(text: &str) -> bool {
    let text = text.to_uppercase();
    parse_reference(&text).is_some_and(|(_, length)| length == text.len())
}

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
    /// The reference as it is, after another sheet prefix.
    Prefixed(String),
    /// Another text in place of the reference and its sheet prefix.
    Whole(String),
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
            Replacement::Prefixed(other) => output.push_str(&format!("{other}{raw}")),
            Replacement::Whole(text) => output.push_str(&text),
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

/// The prefix a reference to the sheet `name` is written with, `name!`,
/// quoted as Excel quotes it when it holds more than letters, digits,
/// underscores and periods, starts with a digit, or reads as a reference.
pub(super) fn sheet_prefix(name: &str) -> String {
    let plain = name
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '.')
        && !name.starts_with(|c: char| c.is_ascii_digit())
        && !reads_as_reference(name)
        && !name.eq_ignore_ascii_case("R")
        && !name.eq_ignore_ascii_case("C");
    if plain {
        format!("{name}!")
    } else {
        format!("'{}'!", name.replace('\'', "''"))
    }
}

/// `formula` with its references to the sheet `from` naming the sheet `to`,
/// as Excel writes the formulas of a sheet it copies.
pub(crate) fn rename_sheet(formula: &str, from: &str, to: &str) -> Result<String> {
    let from = from.to_lowercase();
    transform(formula, |target, _| {
        Ok(match target {
            Target::Sheet(sheet) if sheet.to_lowercase() == from => {
                Replacement::Prefixed(sheet_prefix(to))
            }
            _ => Replacement::Keep,
        })
    })
}

/// Where the rows `first` to `last` moved before the row `at` put the row
/// `row`, as Excel moves cut rows it inserts: the rows between follow them
/// up or down.
pub(crate) fn moved_row(row: u32, (first, last): (u32, u32), at: u32) -> u32 {
    let count = last - first + 1;
    if (first..=last).contains(&row) {
        if at > last {
            at - count + row - first
        } else {
            at + row - first
        }
    } else if at > last && row > last && row < at {
        row - count
    } else if at < first && row >= at && row < first {
        row + count
    } else {
        row
    }
}

/// `formula`, of a cell of the sheet `own`, with its references to the rows
/// `first` to `last` of the sheet `sheet` moved before the row `at`, as Excel
/// moves cut rows it inserts: a cell follows its row; a range keeps its ends
/// when the rows and their new place are both inside it, its ends follow
/// theirs otherwise, and where an end would pass the other the range keeps
/// the rows of it that stay.
pub(crate) fn moved_rows_formula(
    formula: &str,
    own: &str,
    sheet: &str,
    rows: (u32, u32),
    at: u32,
) -> Result<String> {
    let (first, last) = rows;
    transform(formula, |target, reference| {
        let ours = match target {
            Target::Local => own == sheet,
            Target::Sheet(name) => name.to_lowercase() == sheet.to_lowercase(),
            Target::Opaque => false,
        };
        let area = reference.area();
        let Some((a, b)) = area.rows.filter(|_| ours) else {
            return Ok(Replacement::Keep);
        };
        let inside = a < b && a <= first && last <= b && a < at && at <= b;
        if inside {
            return Ok(Replacement::Keep);
        }
        let (x, y) = (moved_row(a, rows, at), moved_row(b, rows, at));
        let span = if x <= y {
            (x, y)
        } else {
            // The rows of the range that stay where they are, moved.
            let start = if (first..=last).contains(&a) {
                last + 1
            } else {
                a
            };
            let end = if (first..=last).contains(&b) {
                first - 1
            } else {
                b
            };
            (moved_row(start, rows, at), moved_row(end, rows, at))
        };
        let moved = Area {
            rows: Some(span),
            ..area
        };
        if moved == area {
            return Ok(Replacement::Keep);
        }
        Ok(Replacement::Text(reference.with_area(moved).render()?))
    })
}

/// `formula` with its references to the sheet `sheet`, and to the tables
/// `tables` (by their names in lowercase) it holds, read `#REF!`, as Excel
/// writes the formulas of the other sheets when it deletes a sheet.
pub(crate) fn delete_sheet_references(
    formula: &str,
    sheet: &str,
    tables: &BTreeSet<String>,
) -> Result<String> {
    let sheet = sheet.to_lowercase();
    let formula = transform(formula, |target, _| {
        Ok(match target {
            Target::Sheet(name) if name.to_lowercase() == sheet => {
                Replacement::Whole("#REF!".to_owned())
            }
            _ => Replacement::Keep,
        })
    })?;
    if tables.is_empty() {
        return Ok(formula);
    }
    Ok(map_structured_references(&formula, |table, _| {
        tables.contains(table).then(|| "#REF!".to_owned())
    }))
}

/// `formula` with the tables `renamed` (by their names in lowercase) named
/// by their new names, outside string literals and sheet names.
pub(crate) fn rename_tables(formula: &str, renamed: &BTreeMap<String, String>) -> String {
    if renamed.is_empty() {
        return formula.to_owned();
    }
    let mut output = String::with_capacity(formula.len());
    let mut index = 0;
    while index < formula.len() {
        let rest = &formula[index..];
        let c = rest.chars().next().unwrap();
        if c == '"' || c == '\'' {
            // A string literal, or a quoted sheet name, as written.
            let mut end = index + 1;
            loop {
                match formula[end..].find(c) {
                    Some(offset) if formula[end + offset + 1..].starts_with(c) => end += offset + 2,
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
            let next = rest[length..].chars().next();
            match renamed.get(&word.to_lowercase()) {
                Some(new) if !matches!(next, Some('(' | '!')) => output.push_str(new),
                _ => output.push_str(word),
            }
            index += length;
            continue;
        }
        output.push(c);
        index += c.len_utf8();
    }
    output
}

/// `formula`, of a cell of `sheet`, with its references moved as
/// `operations` move the cells they name.
pub(crate) fn rewrite_formula(
    formula: &str,
    sheet: &str,
    operations: &[StructuralOperation],
) -> Result<String> {
    rewrite_references(formula, Some(sheet), operations)
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

/// Prove that each translated reference keeps its ordered members. Merely
/// reproducing Excel's range expansion is not proof of equivalent contents.
pub(super) fn reference_equivalence(
    before: &str,
    after: &str,
    sheet: &str,
    operations: &[StructuralOperation],
) -> bool {
    let upper = before.to_ascii_uppercase();
    if upper.contains("INDIRECT(") || upper.contains("OFFSET(") {
        return false;
    }
    let checked = transform(before, |target, reference| {
        let name = match target {
            Target::Local => sheet,
            Target::Sheet(name) => name,
            Target::Opaque => return Ok(Replacement::Keep),
        };
        let own = sheet_operations(name, operations);
        if own.is_empty() {
            return Ok(Replacement::Keep);
        }
        let old = reference.area();
        let new = map_area(old, &own)?.context("reference deleted")?;
        for (a, b, rows) in [
            (old.rows, new.rows, true),
            (old.columns, new.columns, false),
        ] {
            match (a, b) {
                (Some((a1, a2)), Some((b1, b2))) => {
                    ensure!(a2 - a1 == b2 - b1, "reference member count changed")
                }
                (None, None) => ensure!(
                    !own.iter().any(|o| o.row_operation() == rows),
                    "whole-axis reference membership changed"
                ),
                _ => bail!("reference shape changed"),
            }
        }
        Ok(Replacement::Text(reference.with_area(new).render()?))
    });
    !operations.is_empty() && checked.is_ok_and(|rewritten| rewritten == after)
}

/// Row/column operations together with the table columns they delete and the
/// columns whose headers are renamed. Structured references to deleted columns
/// become `#REF!` and those to renamed columns take the new name, as in Excel.
pub(super) struct Moves<'a> {
    pub(super) operations: &'a [StructuralOperation],
    /// Lowercase table name -> lowercase names of its deleted columns.
    removed_columns: BTreeMap<String, BTreeSet<String>>,
    /// Lowercase table name -> lowercase old column name -> new column name.
    renamed_columns: BTreeMap<String, BTreeMap<String, String>>,
}

impl<'a> Moves<'a> {
    pub(super) fn new(
        operations: &'a [StructuralOperation],
        removed_columns: BTreeMap<String, BTreeSet<String>>,
    ) -> Self {
        Self {
            operations,
            removed_columns,
            renamed_columns: BTreeMap::new(),
        }
    }

    pub(super) fn with_renamed_columns(
        mut self,
        renamed_columns: BTreeMap<String, BTreeMap<String, String>>,
    ) -> Self {
        self.renamed_columns = renamed_columns;
        self
    }

    pub(super) fn rewrite(&self, formula: &str, current_sheet: Option<&str>) -> Result<String> {
        if self.removed_columns.is_empty() && self.renamed_columns.is_empty() {
            return rewrite_references(formula, current_sheet, self.operations);
        }
        let formula = remove_structured_references(formula, &self.removed_columns);
        let formula = rename_structured_references(&formula, &self.renamed_columns);
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
        .map(|item| unescape_structured(item).to_lowercase())
        .collect()
}

fn remove_structured_references(
    formula: &str,
    removed: &BTreeMap<String, BTreeSet<String>>,
) -> String {
    if removed.is_empty() {
        return formula.to_owned();
    }
    map_structured_references(formula, |table, body| {
        let columns = removed.get(table)?;
        structured_columns(body)
            .iter()
            .any(|column| columns.contains(column))
            .then(|| "#REF!".to_owned())
    })
}

/// Gives the renamed columns of `renamed` their new names in structured
/// references, keeping each reference's other items as written.
fn rename_structured_references(
    formula: &str,
    renamed: &BTreeMap<String, BTreeMap<String, String>>,
) -> String {
    if renamed.is_empty() {
        return formula.to_owned();
    }
    map_structured_references(formula, |table, body| {
        let columns = renamed.get(table)?;
        let inner = &body[1..body.len().saturating_sub(1).max(1)];
        let rename = |item: &str| -> Option<String> {
            columns
                .get(&unescape_structured(item).to_lowercase())
                .map(|new| escape_structured(new))
        };
        if !inner.starts_with('[') {
            // `Table1[Col]`: a name with special characters needs its own brackets.
            // `Table1[#Totals]` names a special item, never a column.
            if inner.starts_with('#') {
                return None;
            }
            let new = rename(inner)?;
            let plain = new.chars().all(|c| c.is_alphanumeric() || c == '_');
            return Some(if plain {
                format!("[{new}]")
            } else {
                format!("[[{new}]]")
            });
        }
        let mut output = String::from("[");
        let mut index = 0;
        let mut changed = false;
        while let Some(offset) = inner[index..].find('[') {
            let start = index + offset;
            let end = bracket_end(inner, start);
            output.push_str(&inner[index..start]);
            let item = &inner[start + 1..end.saturating_sub(1).max(start + 1)];
            match rename(item).filter(|_| !item.trim_start().starts_with('#')) {
                Some(new) => {
                    output.push_str(&format!("[{new}]"));
                    changed = true;
                }
                None => output.push_str(&inner[start..end]),
            }
            index = end;
        }
        output.push_str(&inner[index..]);
        output.push(']');
        changed.then_some(output)
    })
}

/// A column name as written in a structured reference, where `'` escapes the
/// character after it.
fn unescape_structured(item: &str) -> String {
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
    name
}

/// A column name for a structured reference: `[`, `]`, `#` and `'` take a `'`.
fn escape_structured(name: &str) -> String {
    let mut escaped = String::with_capacity(name.len());
    for c in name.chars() {
        if matches!(c, '[' | ']' | '#' | '\'') {
            escaped.push('\'');
        }
        escaped.push(c);
    }
    escaped
}

/// Calls `replace` with the lowercase table name and bracketed body of each
/// structured reference outside string literals (`Table1` and `[[#This Row],[Qty]]`);
/// a returned body replaces the original one, `#REF!` the whole reference.
fn map_structured_references(
    formula: &str,
    mut replace: impl FnMut(&str, &str) -> Option<String>,
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
            if rest[length..].starts_with('[') {
                let end = bracket_end(formula, index + length);
                let body = &formula[index + length..end];
                match replace(&word.to_lowercase(), body) {
                    Some(error) if error == "#REF!" => output.push_str(&error),
                    Some(body) => {
                        output.push_str(word);
                        output.push_str(&body);
                    }
                    None => output.push_str(&formula[index..end]),
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

/// A cell, range, row or column a formula refers to: its sheet (none for
/// the formula's own, `*` for a 3D or external one) and its rows (none for
/// whole columns).
pub(crate) struct Referenced {
    pub(crate) sheet: Option<String>,
    pub(crate) rows: Option<(u32, u32)>,
}

/// The references of `formula`, outside string literals, structured
/// references and error values.
pub(crate) fn referenced(formula: &str) -> Result<Vec<Referenced>> {
    let mut found = vec![];
    transform(formula, |target, reference| {
        let area = reference.area();
        found.push(Referenced {
            sheet: match target {
                Target::Local => None,
                Target::Sheet(sheet) => Some(sheet.clone()),
                Target::Opaque => Some("*".to_owned()),
            },
            rows: area.rows,
        });
        Ok(Replacement::Keep)
    })?;
    Ok(found)
}

/// Moves relative references by the given offsets, as Excel derives each
/// cell of a shared formula from its master cell.
pub(crate) fn shift_relative(formula: &str, rows: i64, columns: i64) -> Result<String> {
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

    /// The cases measured against Excel's cut and insert of rows, with
    /// `=SUM(A5:A10)`, `=A5` and `=A10` on another row.
    #[test]
    fn moved_rows_follow_excel() {
        let cases = [
            ((10, 10), 7, "SUM(A5:A10)", "A5", "A7"),
            ((5, 5), 9, "SUM(A5:A10)", "A8", "A10"),
            ((7, 7), 13, "SUM(A5:A9)", "A5", "A9"),
            ((12, 12), 7, "SUM(A5:A11)", "A5", "A11"),
            ((5, 10), 13, "SUM(A7:A12)", "A7", "A12"),
            ((9, 10), 13, "SUM(A5:A12)", "A5", "A12"),
            ((4, 5), 9, "SUM(A8:A10)", "A8", "A10"),
            ((12, 12), 5, "SUM(A6:A11)", "A6", "A11"),
            ((3, 3), 11, "SUM(A4:A9)", "A4", "A9"),
            ((3, 3), 6, "SUM(A4:A10)", "A4", "A10"),
            ((5, 5), 2, "SUM(A2:A10)", "A2", "A10"),
            ((5, 5), 8, "SUM(A5:A10)", "A7", "A10"),
            ((5, 5), 14, "SUM(A5:A9)", "A13", "A9"),
            ((10, 10), 2, "SUM(A6:A10)", "A6", "A2"),
            ((10, 10), 8, "SUM(A5:A10)", "A5", "A8"),
            ((10, 10), 14, "SUM(A5:A13)", "A5", "A13"),
            ((7, 7), 2, "SUM(A6:A10)", "A6", "A10"),
            ((7, 8), 2, "SUM(A7:A10)", "A7", "A10"),
            ((7, 8), 14, "SUM(A5:A8)", "A5", "A8"),
            ((4, 5), 2, "SUM(A3:A10)", "A3", "A10"),
            ((4, 5), 8, "SUM(A7:A10)", "A7", "A10"),
            ((4, 5), 14, "SUM(A4:A8)", "A13", "A8"),
            ((10, 11), 2, "SUM(A7:A11)", "A7", "A2"),
            ((10, 11), 8, "SUM(A5:A8)", "A5", "A8"),
            ((10, 11), 14, "SUM(A5:A12)", "A5", "A12"),
            ((5, 10), 2, "SUM(A2:A7)", "A2", "A7"),
            ((5, 10), 14, "SUM(A8:A13)", "A8", "A13"),
            ((4, 11), 2, "SUM(A3:A8)", "A3", "A8"),
            ((4, 11), 14, "SUM(A7:A12)", "A7", "A12"),
            ((3, 4), 2, "SUM(A5:A10)", "A5", "A10"),
            ((3, 4), 8, "SUM(A3:A10)", "A3", "A10"),
            ((3, 4), 14, "SUM(A3:A8)", "A3", "A8"),
            ((11, 12), 2, "SUM(A7:A12)", "A7", "A12"),
            ((11, 12), 8, "SUM(A5:A12)", "A5", "A12"),
            ((11, 12), 14, "SUM(A5:A10)", "A5", "A10"),
            ((7, 7), 5, "SUM(A6:A10)", "A6", "A10"),
            ((7, 7), 11, "SUM(A5:A9)", "A5", "A9"),
            ((7, 7), 6, "SUM(A5:A10)", "A5", "A10"),
            ((3, 3), 5, "SUM(A5:A10)", "A5", "A10"),
            ((12, 12), 11, "SUM(A5:A10)", "A5", "A10"),
            ((5, 5), 11, "SUM(A5:A9)", "A10", "A9"),
            ((5, 5), 10, "SUM(A5:A10)", "A9", "A10"),
            ((10, 10), 5, "SUM(A6:A10)", "A6", "A5"),
            ((10, 10), 6, "SUM(A5:A10)", "A5", "A6"),
            ((6, 7), 5, "SUM(A7:A10)", "A7", "A10"),
            ((8, 9), 11, "SUM(A5:A8)", "A5", "A8"),
        ];
        for (rows, at, sum, start, end) in cases {
            let moved = |formula| moved_rows_formula(formula, "S", "S", rows, at).unwrap();
            assert_eq!(moved("SUM(A5:A10)"), sum, "{rows:?} before {at}");
            assert_eq!(moved("A5"), start, "{rows:?} before {at}");
            assert_eq!(moved("A10"), end, "{rows:?} before {at}");
        }
        assert_eq!(
            moved_rows_formula("S!$A$7+T!A7+A7", "T", "S", (7, 7), 14).unwrap(),
            "S!$A$13+T!A7+A7"
        );
    }

    fn operation(sheet: &str, kind: OperationKind, at: u32, count: u32) -> StructuralOperation {
        StructuralOperation {
            id: "op".into(),
            sheet: sheet.into(),
            kind,
            at,
            count,
            style_from: None,
            style_rows: 1,
            merges: false,
            vba_unchanged: false,
            band: None,
        }
    }

    #[test]
    fn renamed_columns_take_their_new_name_in_structured_references() {
        let renamed = BTreeMap::from([(
            "orders".to_owned(),
            BTreeMap::from([
                ("qty".to_owned(), "Quantity".to_owned()),
                ("unit price".to_owned(), "Price [JPY]".to_owned()),
            ]),
        )]);
        let rename = |f: &str| rename_structured_references(f, &renamed);
        assert_eq!(rename("SUM(Orders[Qty])"), "SUM(Orders[Quantity])");
        assert_eq!(
            rename("Orders[[#This Row],[qty]]*orders[[#This Row],[Unit Price]]"),
            "Orders[[#This Row],[Quantity]]*orders[[#This Row],[Price '[JPY']]]"
        );
        assert_eq!(rename("Orders[Unit Price]"), "Orders[[Price '[JPY']]]");
        assert_eq!(
            rename("SUM(Orders[[Qty]:[Unit Price]])"),
            "SUM(Orders[[Quantity]:[Price '[JPY']]])"
        );
        // Other tables, other columns, special items and strings stay as written.
        for unchanged in [
            "Other[Qty]",
            "Orders[Total]",
            "Orders[#Totals]",
            "\"Orders[Qty]\"&Orders[[#Headers],[Total]]",
        ] {
            assert_eq!(rename(unchanged), unchanged);
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
