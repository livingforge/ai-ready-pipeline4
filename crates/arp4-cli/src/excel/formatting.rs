//! Format operations (`format_cells`) written into a workbook last, after
//! its values, fonts and row and column operations: each operation names its
//! range in the cells the operations recorded before it leave, so those
//! recorded after it move the range as they move the cells. Only formats,
//! row heights, column widths and merges change; every value stays.
use super::fonts::{Line, StyleFormats};
use super::*;

/// The most cells one operation formats.
const MAX_CELLS: u64 = 1_000_000;

/// Writes `formats` into the workbook at `path`, which the other edits and
/// the row and column operations `structural` wrote, and returns the parts
/// they change.
pub(super) fn apply_formats(
    path: &Path,
    structural: &[StructuralOperation],
    formats: &[FormatOperation],
) -> Result<Vec<String>> {
    if formats.is_empty() {
        return Ok(vec![]);
    }
    let book = Workbook::open(path)?;
    let styles_text = book
        .parts
        .get("xl/styles.xml")
        .map(|bytes| std::str::from_utf8(bytes))
        .transpose()?;
    let mut styles = None;
    let mut patched: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for operation in formats {
        let part = book
            .sheets
            .iter()
            .find(|sheet| sheet["name"] == operation.sheet.as_str())
            .map(|sheet| string(&sheet["part"]))
            .transpose()?
            .with_context(|| format!("format sheet {} is missing", operation.sheet))?
            .to_owned();
        let later: Vec<&StructuralOperation> = structural[operation.preceding..]
            .iter()
            .filter(|later| later.sheet == operation.sheet)
            .collect();
        if let Some(to) = &operation.copy_to {
            let text = match patched.get(&part) {
                Some(bytes) => String::from_utf8(bytes.clone())?,
                None => std::str::from_utf8(&book.parts[&part])?.to_owned(),
            };
            patched.insert(
                part,
                copy_formats(&text, operation, to, &later)?.into_bytes(),
            );
            continue;
        }
        let Some(range) = map_area(Area::parse(&operation.range)?, &later)? else {
            continue;
        };
        // The cells' formats and a merge go with the cells a later shift
        // moves within its band; rows and columns keep their sizes. Cells
        // later insertions format like the formatted ones take the format too.
        let Reach { cells, sized } = reach(
            Area::parse(&operation.range)?,
            structural,
            operation.preceding,
            &operation.sheet,
            operation.merge == Some(true),
        )?;
        let from = operation
            .from
            .as_deref()
            .map(|cell| -> Result<(u32, u32)> {
                let area = map_area(Area::parse(cell)?, &later)?.with_context(|| {
                    format!(
                        "{}!{cell}, whose format {} copies, is deleted by a later operation",
                        operation.sheet, operation.id
                    )
                })?;
                Ok((area.first_column(), area.first_row()))
            })
            .transpose()?;
        let text = match patched.get(&part) {
            Some(bytes) => String::from_utf8(bytes.clone())?,
            None => std::str::from_utf8(&book.parts[&part])?.to_owned(),
        };
        let needs_styles = from.is_some()
            || !operation.alignment.is_empty()
            || !operation.look.is_empty()
            || !operation.borders.is_empty();
        if needs_styles && styles.is_none() {
            let source = styles_text
                .context("the workbook has no styles part, so ARP cannot format cells")?;
            styles = Some(StyleFormats::new(source)?);
        }
        let text = format_sheet(
            &text,
            operation,
            (range, &cells, &sized),
            from,
            styles.as_mut(),
        )?;
        patched.insert(part, text.into_bytes());
    }
    if let Some(styles) = styles {
        patched.insert("xl/styles.xml".into(), styles.finish()?.into_bytes());
    }
    patched.retain(|part, bytes| book.parts.get(part) != Some(bytes));
    if patched.is_empty() {
        return Ok(vec![]);
    }
    let staged = path.with_extension(format!(
        "formatted.{}",
        path.extension()
            .context("workbook without an extension")?
            .to_string_lossy()
    ));
    write_archive_without(&book.raw, &staged, &patched, &BTreeSet::new())?;
    // Formats change no value: every cell reads back as it was written.
    let written = Workbook::open(&staged)?;
    for (before, after) in book.cells.iter().zip(&written.cells) {
        let values = |cells: &[Cell]| -> BTreeMap<String, (Value, Option<Box<str>>)> {
            cells
                .iter()
                .map(|cell| {
                    (
                        cell.address.to_string(),
                        (cell.value.clone(), cell.formula.clone()),
                    )
                })
                .collect()
        };
        ensure!(
            values(before) == values(after),
            "a format operation changed cell values"
        );
    }
    fs::rename(&staged, path)?;
    Ok(patched.into_keys().collect())
}

/// What a format operation reaches after the operations recorded after it.
struct Reach {
    /// The parts its cells take, with the cells inserted after it that
    /// Excel formats like them.
    cells: Vec<Area>,
    /// The whole rows and columns inserted after it that Excel sizes like
    /// its own.
    sized: Vec<Area>,
}

/// The cells of the format range `area` after the operations of `sheet`
/// recorded at `preceding` and after in `structural`. Excel formats an
/// inserted row like its style row, and an inserted column or cell like the
/// one before it, so a format the operation gives those reaches the inserted
/// cells as well: a row formatted and then copied by inserting rows after it
/// copies the format given, as in Excel.
fn reach(
    area: Area,
    structural: &[StructuralOperation],
    preceding: usize,
    sheet: &str,
    merge: bool,
) -> Result<Reach> {
    let mut cells = vec![area];
    let mut sized = vec![];
    for (index, operation) in structural.iter().enumerate().skip(preceding) {
        if operation.sheet != sheet {
            continue;
        }
        let mut alike = vec![];
        if operation.insertion() {
            let done: Vec<StructuralOperation> = structural[..index]
                .iter()
                .filter(|done| done.sheet == sheet)
                .cloned()
                .collect();
            for part in &cells {
                alike.extend(formatted_alike(*part, operation, &done));
            }
        }
        cells = map_format_area_parts(cells, operation, merge)?;
        let mut moved = vec![];
        for area in sized {
            moved.extend(map_area(area, &[operation])?);
        }
        sized = moved;
        // Whole rows and columns take the height or width of the ones they
        // are formatted like.
        if operation.whole() {
            for area in &alike {
                sized.push(if operation.row_operation() {
                    Area {
                        columns: None,
                        rows: area.rows,
                    }
                } else {
                    Area {
                        columns: area.columns,
                        rows: None,
                    }
                });
            }
        }
        cells.extend(alike);
    }
    Ok(Reach { cells, sized })
}

/// `areas` after one more operation, as [`map_format_area`] maps them.
fn map_format_area_parts(
    areas: Vec<Area>,
    operation: &StructuralOperation,
    merge: bool,
) -> Result<Vec<Area>> {
    let mut mapped = vec![];
    for area in areas {
        mapped.extend(map_format_area(area, &[operation], merge)?);
    }
    Ok(mapped)
}

/// The cells `operation` inserts that take their format from the cells of
/// `part` (both as the operations `done` before it leave them): the rows
/// whose style row lies in it, or the columns or cells after one of it.
fn formatted_alike(
    part: Area,
    operation: &StructuralOperation,
    done: &[StructuralOperation],
) -> Vec<Area> {
    let (Some(columns), Some(rows)) = (part.columns, part.rows) else {
        return vec![];
    };
    let inside = |span: (u32, u32), position: u32| (span.0..=span.1).contains(&position);
    let inserted = (operation.at, operation.at + operation.count - 1);
    let before = operation.at.checked_sub(1).filter(|&before| before > 0);
    match (operation.row_operation(), operation.band) {
        // Whole rows take their style rows in turn.
        (true, None) => {
            let mut found: Vec<Area> = vec![];
            for offset in 0..operation.count {
                let source = operation
                    .style_from
                    .map(|from| from + offset % operation.style_rows.max(1))
                    .and_then(|from| transform_index(from, done, true));
                if !source.is_some_and(|source| inside(rows, source)) {
                    continue;
                }
                let row = operation.at + offset;
                match found.last_mut() {
                    Some(last) if last.rows.is_some_and(|(_, end)| end + 1 == row) => {
                        last.rows = last.rows.map(|(start, _)| (start, row));
                    }
                    _ => found.push(Area {
                        columns: Some(columns),
                        rows: Some((row, row)),
                    }),
                }
            }
            found
        }
        (false, None) if before.is_some_and(|before| inside(columns, before)) => vec![Area {
            columns: Some(inserted),
            rows: Some(rows),
        }],
        // Cells shifted down or right take the cells above or left of them,
        // in the columns or rows of the band.
        (true, Some((first, last))) if before.is_some_and(|before| inside(rows, before)) => {
            let (low, high) = (columns.0.max(first), columns.1.min(last));
            if low > high {
                return vec![];
            }
            vec![Area {
                columns: Some((low, high)),
                rows: Some(inserted),
            }]
        }
        (false, Some((first, last))) if before.is_some_and(|before| inside(columns, before)) => {
            let (low, high) = (rows.0.max(first), rows.1.min(last));
            if low > high {
                return vec![];
            }
            vec![Area {
                columns: Some(inserted),
                rows: Some((low, high)),
            }]
        }
        _ => vec![],
    }
}

/// The worksheet `text` with `operation` applied to `range` (in the cells
/// of the written workbook), its format copied from the cell `from`, and
/// its row heights and column widths given to the rows and columns `sized`.
fn format_sheet(
    text: &str,
    operation: &FormatOperation,
    (range, parts, sized): (Area, &[Area], &[Area]),
    from: Option<(u32, u32)>,
    styles: Option<&mut StyleFormats<'_>>,
) -> Result<String> {
    let bounds = |area: Area| -> Result<((u32, u32), (u32, u32))> {
        Ok((
            area.columns.context("format range without columns")?,
            area.rows.context("format range without rows")?,
        ))
    };
    let (columns, rows) = bounds(range)?;
    let mut result = text.to_owned();
    if let Some(styles) = styles {
        let cells: u64 = parts
            .iter()
            .map(|&part| {
                let (columns, rows) = bounds(part)?;
                Ok(u64::from(columns.1 - columns.0 + 1) * u64::from(rows.1 - rows.0 + 1))
            })
            .sum::<Result<u64>>()?;
        ensure!(
            cells <= MAX_CELLS,
            "format range {} holds {cells} cells; format at most {MAX_CELLS} at a time",
            operation.range
        );
        // The format copied is read before any cell changes.
        let source = from.map(|from| format_at(&result, from)).transpose()?;
        for &part in parts {
            let (columns, rows) = bounds(part)?;
            result = restyle(&result, columns, rows, |column, row, current| {
                let mut style = source.unwrap_or(current);
                if !operation.alignment.is_empty() {
                    style = styles.with_alignment(style, &operation.alignment)?;
                }
                let lines = sides_of(&operation.borders, bounds(range)?, (column, row));
                Ok(Some(styles.with_look(style, &operation.look, &lines)?))
            })?;
        }
        if !operation.borders.is_empty() {
            result = face_neighbors(&result, bounds(range)?, styles)?;
        }
    }
    if let Some(height) = operation.row_height {
        result = set_row_heights(&result, rows, height)?;
        for area in sized.iter().filter(|area| area.columns.is_none()) {
            result = set_row_heights(&result, area.rows.context("sized rows")?, height)?;
        }
    }
    if let Some(width) = operation.column_width {
        result = set_column_widths(&result, columns, width)?;
        for area in sized.iter().filter(|area| area.rows.is_none()) {
            result = set_column_widths(&result, area.columns.context("sized columns")?, width)?;
        }
    }
    for &part in parts {
        let (columns, rows) = bounds(part)?;
        match operation.merge {
            Some(true) => result = merge(&result, &operation.range, columns, rows)?,
            Some(false) => result = unmerge(&result, &operation.range, columns, rows)?,
            None => {}
        }
    }
    Ok(result)
}

/// The worksheet `text` with the formats and merges of the cells of the
/// range of `operation` (a `copy_cells`) copied to the range of its size at
/// `to`, both moved by the operations recorded after it (`later`), as Excel
/// pastes them: the merges inside the place pasted to give way to those of
/// the range copied, and one reaching out of it is refused.
fn copy_formats(
    text: &str,
    operation: &FormatOperation,
    to: &str,
    later: &[&StructuralOperation],
) -> Result<String> {
    let recorded = Area::parse(&operation.range)?;
    let size = |area: Area| -> Result<(u32, u32)> {
        let (columns, rows) = (
            area.columns.context("copied range without columns")?,
            area.rows.context("copied range without rows")?,
        );
        Ok((columns.1 - columns.0, rows.1 - rows.0))
    };
    let (width, height) = size(recorded)?;
    let Some(source) = map_area(recorded, later)? else {
        bail!(
            "the cells {} copies from {} are deleted by a later operation",
            operation.id,
            operation.range
        );
    };
    let Some(target) = map_area(Area::parse(to)?, later)? else {
        bail!(
            "the cell {to} {} copies to is deleted by a later operation",
            operation.id
        );
    };
    ensure!(
        size(source)? == (width, height),
        "a later operation inserts or deletes rows or columns inside the cells {} copies from ({}); record the copy after it",
        operation.id,
        operation.range
    );
    let (from_column, from_row) = (source.first_column(), source.first_row());
    let (to_column, to_row) = (target.first_column(), target.first_row());
    let columns = (to_column, to_column + width);
    let rows = (to_row, to_row + height);
    ensure!(
        columns.1 <= MAX_COLUMN && rows.1 <= MAX_ROW,
        "the cells {} copies to run past the end of the sheet",
        operation.id
    );
    // The formats of the cells copied, read before any changes.
    let formats = formats_at(
        text,
        (0..=height).flat_map(|r| (0..=width).map(move |c| (from_column + c, from_row + r))),
    )?;
    let mut result = restyle(text, columns, rows, |column, row, _| {
        Ok(Some(
            formats[&(from_column + column - to_column, from_row + row - to_row)],
        ))
    })?;
    // The merges of the cells copied, and those of the place pasted to.
    let doc = xml(result.as_bytes())?;
    let inside = |(c, r): Span, columns: (u32, u32), rows: (u32, u32)| {
        columns.0 <= c.0 && c.1 <= columns.1 && rows.0 <= r.0 && r.1 <= rows.1
    };
    let meets = |(c, r): Span, columns: (u32, u32), rows: (u32, u32)| {
        c.0 <= columns.1 && columns.0 <= c.1 && r.0 <= rows.1 && rows.0 <= r.1
    };
    let source_columns = (from_column, from_column + width);
    let source_rows = (from_row, from_row + height);
    let mut copied = vec![];
    let mut covered = false;
    for (span, node) in merges(&doc)? {
        covered |= inside(span, columns, rows);
        if meets(span, columns, rows) && !inside(span, columns, rows) {
            bail!(
                "the cells {} copies to reach part of the merged range {}, which Excel would refuse; take it apart first (documents format --unmerge) or copy to another place",
                operation.id,
                node.attribute("ref").unwrap_or_default()
            );
        }
        if meets(span, source_columns, source_rows) && !inside(span, source_columns, source_rows) {
            bail!(
                "the cells {} copies reach part of the merged range {}; copy the whole merge",
                operation.id,
                node.attribute("ref").unwrap_or_default()
            );
        }
        if inside(span, source_columns, source_rows) {
            let (c, r) = span;
            copied.push(
                Area {
                    columns: Some((c.0 - from_column + to_column, c.1 - from_column + to_column)),
                    rows: Some((r.0 - from_row + to_row, r.1 - from_row + to_row)),
                }
                .render()?,
            );
        }
    }
    let range = Area {
        columns: Some(columns),
        rows: Some(rows),
    }
    .render()?;
    if covered {
        result = unmerge(&result, &range, columns, rows)?;
    }
    if !copied.is_empty() {
        result = add_merges(&result, &copied)?;
    }
    Ok(result)
}

/// The rows of a worksheet by number, with the formats of their cells.
struct Grid<'a> {
    prefix: String,
    rows: BTreeMap<u32, Node<'a, 'a>>,
}

impl<'a> Grid<'a> {
    fn read(doc: &'a Document<'a>, text: &str) -> Result<Self> {
        let data = child(doc.root_element(), "sheetData").context("missing sheetData")?;
        let prefix = element_tag(&text[data.range()])?
            .strip_suffix("sheetData")
            .context("invalid sheetData tag")?
            .to_owned();
        let mut rows = BTreeMap::new();
        for row in data.children().filter(|n| n.has_tag_name((NS, "row"))) {
            rows.insert(
                row.attribute("r").context("row without number")?.parse()?,
                row,
            );
        }
        Ok(Self { prefix, rows })
    }

    fn cell(&self, column: u32, row: u32) -> Option<Node<'a, 'a>> {
        self.rows.get(&row)?.children().find(|cell| {
            cell.has_tag_name((NS, "c"))
                && cell
                    .attribute("r")
                    .and_then(|r| coordinate(r).ok())
                    .is_some_and(|(c, _)| c == column)
        })
    }
}

/// The format of the cell at (`column`, `row`) of the worksheet `text`, or
/// of the cell left out there: its row's, else its column's.
fn format_at(text: &str, cell: (u32, u32)) -> Result<usize> {
    Ok(formats_at(text, std::iter::once(cell))?[&cell])
}

/// The format a cell left out of `row` shows in `column`: the row's, else
/// the column's.
fn shown_format(doc: &Document<'_>, row: Option<Node<'_, '_>>, column: u32) -> usize {
    row.filter(|r| matches!(r.attribute("customFormat"), Some("1" | "true")))
        .and_then(|r| r.attribute("s"))
        .and_then(|s| s.parse().ok())
        .or_else(|| {
            child(doc.root_element(), "cols")
                .into_iter()
                .flat_map(|cols| cols.children().filter(|n| n.has_tag_name((NS, "col"))))
                .find_map(|col| {
                    let min: u32 = col.attribute("min")?.parse().ok()?;
                    let max: u32 = col.attribute("max")?.parse().ok()?;
                    (min..=max)
                        .contains(&column)
                        .then(|| col.attribute("style")?.parse().ok())
                        .flatten()
                })
        })
        .unwrap_or(0)
}

/// The sides (left, right, top, bottom) the `borders` of a format operation
/// draw on the cell at (`column`, `row`) of its range `columns` × `rows`; a
/// cell of the format's reach past the range takes the lines of the cell
/// of the range it is formatted like, nearest to it.
fn sides_of(
    borders: &[BorderLine],
    (columns, rows): ((u32, u32), (u32, u32)),
    (column, row): (u32, u32),
) -> [Option<Option<Line>>; 4] {
    let (column, row) = (
        column.clamp(columns.0, columns.1),
        row.clamp(rows.0, rows.1),
    );
    let mut sides: [Option<Option<Line>>; 4] = Default::default();
    for border in borders {
        let edges = border.edges.as_str();
        let outer = |side: &str| matches!(edges, "all" | "outline") || edges == side;
        let inner = |across: &str| matches!(edges, "all" | "inside") || edges == across;
        let drawn = [
            if column == columns.0 {
                outer("left")
            } else {
                inner("vertical")
            },
            if column == columns.1 {
                outer("right")
            } else {
                inner("vertical")
            },
            if row == rows.0 {
                outer("top")
            } else {
                inner("horizontal")
            },
            if row == rows.1 {
                outer("bottom")
            } else {
                inner("horizontal")
            },
        ];
        let line = (border.style != "none").then(|| (border.style.clone(), border.color.clone()));
        for (side, drawn) in sides.iter_mut().zip(drawn) {
            if drawn {
                *side = Some(line.clone());
            }
        }
    }
    sides
}

/// The worksheet `text` with the lines of the cells around the range
/// `columns` × `rows` facing it taken away where the range's cells draw that
/// edge, as Excel keeps a line of an edge with the cells it formats: the
/// line goes with them when they move or are deleted.
fn face_neighbors(
    text: &str,
    (columns, rows): ((u32, u32), (u32, u32)),
    styles: &mut StyleFormats<'_>,
) -> Result<String> {
    let mut result = text.to_owned();
    // The cells past each edge, the side of theirs facing it, and the side
    // of the range's cells there.
    let strips = [
        (
            rows.0
                .checked_sub(1)
                .filter(|&r| r > 0)
                .map(|r| (columns, (r, r))),
            3,
            2,
        ),
        (
            (rows.1 < MAX_ROW).then_some((columns, (rows.1 + 1, rows.1 + 1))),
            2,
            3,
        ),
        (
            columns
                .0
                .checked_sub(1)
                .filter(|&c| c > 0)
                .map(|c| ((c, c), rows)),
            1,
            0,
        ),
        (
            (columns.1 < MAX_COLUMN).then_some(((columns.1 + 1, columns.1 + 1), rows)),
            0,
            1,
        ),
    ];
    const SIDES: [&str; 4] = ["left", "right", "top", "bottom"];
    for (strip, facing, side) in strips {
        let Some((strip_columns, strip_rows)) = strip else {
            continue;
        };
        // The formats of the range's cells along the edge, read once.
        let inside = formats_at(
            &result,
            (strip_rows.0..=strip_rows.1).flat_map(|row| {
                (strip_columns.0..=strip_columns.1).map(move |column| {
                    (
                        column.clamp(columns.0, columns.1),
                        row.clamp(rows.0, rows.1),
                    )
                })
            }),
        )?;
        result = restyle(
            &result,
            strip_columns,
            strip_rows,
            |column, row, current| {
                let cell = (
                    column.clamp(columns.0, columns.1),
                    row.clamp(rows.0, rows.1),
                );
                if !styles.draws(current, SIDES[facing])?
                    || !styles.draws(inside[&cell], SIDES[side])?
                {
                    return Ok(None);
                }
                let mut lines: [Option<Option<Line>>; 4] = Default::default();
                lines[facing] = Some(None);
                Ok(Some(styles.with_look(
                    current,
                    &CellLook::default(),
                    &lines,
                )?))
            },
        )?;
    }
    Ok(result)
}

/// The formats of the `cells` (column, row) of the worksheet `text`, as
/// [`format_at`] gives each.
fn formats_at(
    text: &str,
    cells: impl Iterator<Item = (u32, u32)>,
) -> Result<BTreeMap<(u32, u32), usize>> {
    let doc = xml(text.as_bytes())?;
    let grid = Grid::read(&doc, text)?;
    let mut formats = BTreeMap::new();
    for (column, row) in cells {
        let format = match grid.cell(column, row) {
            Some(cell) => cell.attribute("s").unwrap_or("0").parse()?,
            None => shown_format(&doc, grid.rows.get(&row).copied(), column),
        };
        formats.insert((column, row), format);
    }
    Ok(formats)
}

/// The worksheet with each cell of `columns` × `rows` given the format
/// `style_for` makes of the format it has or shows (none: unchanged). Cells
/// the worksheet leaves out are added, holding only the format.
pub(super) fn restyle(
    text: &str,
    columns: (u32, u32),
    rows: (u32, u32),
    mut style_for: impl FnMut(u32, u32, usize) -> Result<Option<usize>>,
) -> Result<String> {
    let doc = xml(text.as_bytes())?;
    let grid = Grid::read(&doc, text)?;
    let style_of = |cell: Option<Node<'_, '_>>| -> Result<usize> {
        Ok(cell.and_then(|c| c.attribute("s")).unwrap_or("0").parse()?)
    };
    let mut edits = vec![];
    let mut new_rows = BTreeMap::new();
    let prefix = &grid.prefix;
    for number in rows.0..=rows.1 {
        let row = grid.rows.get(&number).copied();
        let mut added = vec![];
        for column in columns.0..=columns.1 {
            let cell = grid.cell(column, number);
            let current = match cell {
                Some(_) => style_of(cell)?,
                None => shown_format(&doc, row, column),
            };
            let Some(style) = style_for(column, number, current)? else {
                continue;
            };
            match cell {
                Some(cell) => {
                    if cell.attribute("s").unwrap_or("0") != style.to_string() {
                        let opening = opening_range(text, cell)?;
                        edits.push((
                            opening.clone(),
                            set_xml_attribute(&text[opening], "s", &style.to_string())?,
                        ));
                    }
                }
                None if style != 0 => added.push((column, style)),
                None => {}
            }
        }
        if added.is_empty() {
            continue;
        }
        let cell = |(column, style): &(u32, usize)| -> Result<String> {
            Ok(format!(
                r#"<{prefix}c r="{}{number}" s="{style}"/>"#,
                column_name(*column)?
            ))
        };
        match row {
            Some(row) => {
                // Each new cell goes before the first cell right of it.
                let raw = &text[row.range()];
                if raw.trim_end().ends_with("/>") {
                    let (opening, _) = xml_opening(raw)?;
                    let opening = remove_xml_attribute(opening, "spans")?;
                    let cells: String = added.iter().map(cell).collect::<Result<_>>()?;
                    edits.push((
                        row.range(),
                        format!(
                            "{}>{cells}</{prefix}row>",
                            opening
                                .trim_end_matches('>')
                                .trim_end_matches('/')
                                .trim_end()
                        ),
                    ));
                    continue;
                }
                let opening = opening_range(text, row)?;
                edits.push((
                    opening.clone(),
                    remove_xml_attribute(&text[opening], "spans")?,
                ));
                let end = row.range().start + raw.rfind("</").context("invalid row")?;
                for entry in &added {
                    let at = row
                        .children()
                        .filter(|c| c.has_tag_name((NS, "c")))
                        .find(|c| {
                            c.attribute("r")
                                .and_then(|r| coordinate(r).ok())
                                .is_some_and(|(column, _)| column > entry.0)
                        })
                        .map_or(end, |c| c.range().start);
                    edits.push((at..at, cell(entry)?));
                }
            }
            None => {
                let cells: String = added.iter().map(cell).collect::<Result<_>>()?;
                new_rows.insert(
                    number,
                    format!(r#"<{prefix}row r="{number}">{cells}</{prefix}row>"#),
                );
            }
        }
    }
    insert_rows(text, &doc, &grid, edits, new_rows, (columns, rows))
}

/// `text` with `edits` made and the rows `new_rows` put in order among the
/// worksheet's rows, its used range covering `used`.
fn insert_rows(
    text: &str,
    doc: &Document<'_>,
    grid: &Grid<'_>,
    mut edits: Vec<(std::ops::Range<usize>, String)>,
    new_rows: BTreeMap<u32, String>,
    used: ((u32, u32), (u32, u32)),
) -> Result<String> {
    let data = child(doc.root_element(), "sheetData").context("missing sheetData")?;
    let raw = &text[data.range()];
    if raw.trim_end().ends_with("/>") && !new_rows.is_empty() {
        let prefix = &grid.prefix;
        edits.push((
            data.range(),
            format!(
                "<{prefix}sheetData>{}</{prefix}sheetData>",
                new_rows.values().cloned().collect::<String>()
            ),
        ));
    } else {
        let end = data.range().start + raw.rfind("</").unwrap_or(raw.len());
        for (number, row) in new_rows {
            let at = grid
                .rows
                .range(number..)
                .next()
                .map_or(end, |(_, next)| next.range().start);
            edits.push((at..at, row));
        }
    }
    // A cell put before another goes in ahead of that cell's own change.
    edits.sort_by_key(|(range, _)| (range.start, range.end));
    let result = splice(text, edits, "format")?;
    cover_dimension(
        &result,
        Area {
            columns: Some(used.0),
            rows: Some(used.1),
        },
    )
}

/// The worksheet with the rows of `rows` `height` points high, or fitted by
/// Excel to their text when `height` is none.
fn set_row_heights(text: &str, rows: (u32, u32), height: Option<f64>) -> Result<String> {
    let doc = xml(text.as_bytes())?;
    let grid = Grid::read(&doc, text)?;
    let mut edits = vec![];
    let mut new_rows = BTreeMap::new();
    for number in rows.0..=rows.1 {
        match (grid.rows.get(&number), height) {
            (Some(row), _) => {
                let opening = opening_range(text, *row)?;
                let raw = &text[opening.clone()];
                let edited = match height {
                    Some(height) => set_xml_attribute(
                        &set_xml_attribute(raw, "ht", &height.to_string())?,
                        "customHeight",
                        "1",
                    )?,
                    None => {
                        remove_xml_attribute(&remove_xml_attribute(raw, "ht")?, "customHeight")?
                    }
                };
                if edited != raw {
                    edits.push((opening, edited));
                }
            }
            (None, Some(height)) => {
                let prefix = &grid.prefix;
                new_rows.insert(
                    number,
                    format!(r#"<{prefix}row r="{number}" ht="{height}" customHeight="1"/>"#),
                );
            }
            (None, None) => {}
        }
    }
    let data = child(doc.root_element(), "sheetData").context("missing sheetData")?;
    if new_rows.is_empty() {
        return splice(text, edits, "row height");
    }
    let raw = &text[data.range()];
    if raw.trim_end().ends_with("/>") {
        let prefix = &grid.prefix;
        edits.push((
            data.range(),
            format!(
                "<{prefix}sheetData>{}</{prefix}sheetData>",
                new_rows.values().cloned().collect::<String>()
            ),
        ));
    } else {
        let end = data.range().start + raw.rfind("</").context("invalid sheetData")?;
        for (number, row) in new_rows {
            let at = grid
                .rows
                .range(number..)
                .next()
                .map_or(end, |(_, next)| next.range().start);
            edits.push((at..at, row));
        }
    }
    splice(text, edits, "row height")
}

/// The worksheet with the columns of `columns` `width` characters wide: the
/// column ranges are split where the change begins and ends.
fn set_column_widths(text: &str, columns: (u32, u32), width: f64) -> Result<String> {
    let doc = xml(text.as_bytes())?;
    let root = doc.root_element();
    let prefix = element_tag(&text[root.range()])?
        .strip_suffix("worksheet")
        .context("invalid worksheet tag")?
        .to_owned();
    let cols = child(root, "cols");
    // Each column range as (min, max, its opening tag without min and max).
    let mut ranges: Vec<(u32, u32, String)> = vec![];
    for col in cols
        .into_iter()
        .flat_map(|cols| cols.children().filter(|n| n.has_tag_name((NS, "col"))))
    {
        let min: u32 = col.attribute("min").context("col without min")?.parse()?;
        let max: u32 = col.attribute("max").context("col without max")?.parse()?;
        let raw = &text[col.range()];
        let (opening, _) = xml_opening(raw)?;
        let opening = remove_xml_attribute(&remove_xml_attribute(opening, "min")?, "max")?;
        let opening = opening
            .trim_end_matches('>')
            .trim_end_matches('/')
            .trim_end();
        let attributes = opening
            .split_once(char::is_whitespace)
            .map_or("", |(_, rest)| rest)
            .trim()
            .to_owned();
        ranges.push((min, max, attributes));
    }
    let width_attributes = |attributes: &str| -> Result<String> {
        let opening = format!("<col {attributes}/>");
        let opening = set_xml_attribute(&opening, "width", &width.to_string())?;
        let opening = set_xml_attribute(&opening, "customWidth", "1")?;
        Ok(opening
            .trim_start_matches("<col")
            .trim_end_matches("/>")
            .trim()
            .to_owned())
    };
    let mut result = vec![];
    let mut covered = vec![];
    for (min, max, attributes) in &ranges {
        let (min, max) = (*min, *max);
        if max < columns.0 || min > columns.1 {
            result.push((min, max, attributes.clone()));
            continue;
        }
        if min < columns.0 {
            result.push((min, columns.0 - 1, attributes.clone()));
        }
        let (first, last) = (min.max(columns.0), max.min(columns.1));
        result.push((first, last, width_attributes(attributes)?));
        covered.push((first, last));
        if max > columns.1 {
            result.push((columns.1 + 1, max, attributes.clone()));
        }
    }
    // Columns no range held take the width alone.
    let mut next = columns.0;
    covered.sort_unstable();
    let mut gaps = vec![];
    for (first, last) in covered {
        if first > next {
            gaps.push((next, first - 1));
        }
        next = next.max(last + 1);
    }
    if next <= columns.1 {
        gaps.push((next, columns.1));
    }
    for (first, last) in gaps {
        result.push((first, last, width_attributes("")?));
    }
    result.sort_by_key(|(min, _, _)| *min);
    let rendered: String = result
        .iter()
        .map(|(min, max, attributes)| {
            let attributes = if attributes.is_empty() {
                String::new()
            } else {
                format!(" {attributes}")
            };
            format!(r#"<{prefix}col min="{min}" max="{max}"{attributes}/>"#)
        })
        .collect();
    let element = format!("<{prefix}cols>{rendered}</{prefix}cols>");
    let mut output = text.to_owned();
    match cols {
        Some(cols) => output.replace_range(cols.range(), &element),
        None => {
            // cols goes before sheetData, after the sheet's format.
            let data = child(root, "sheetData").context("missing sheetData")?;
            output.insert_str(data.range().start, &element);
        }
    }
    Ok(output)
}

/// The first and last columns, and the first and last rows, of a range.
type Span = ((u32, u32), (u32, u32));

/// The merges of a worksheet as (columns, rows) with their elements.
fn merges<'a>(doc: &'a Document<'a>) -> Result<Vec<(Span, Node<'a, 'a>)>> {
    child(doc.root_element(), "mergeCells")
        .into_iter()
        .flat_map(|merges| {
            merges
                .children()
                .filter(|n| n.has_tag_name((NS, "mergeCell")))
        })
        .map(|merge| {
            let area = Area::parse(merge.attribute("ref").context("merge without ref")?)?;
            Ok((
                (
                    area.columns.context("merge without columns")?,
                    area.rows.context("merge without rows")?,
                ),
                merge,
            ))
        })
        .collect()
}

/// The worksheet with `columns` × `rows` merged, as Excel merges a range
/// whose cells other than the top left hold nothing.
fn merge(text: &str, range: &str, columns: (u32, u32), rows: (u32, u32)) -> Result<String> {
    let doc = xml(text.as_bytes())?;
    let overlaps = |(c, r): ((u32, u32), (u32, u32))| {
        c.0 <= columns.1 && columns.0 <= c.1 && r.0 <= rows.1 && rows.0 <= r.1
    };
    if let Some((_, existing)) = merges(&doc)?.into_iter().find(|(area, _)| overlaps(*area)) {
        bail!(
            "{range} overlaps the merged range {}; take that merge apart first",
            existing.attribute("ref").unwrap_or_default()
        );
    }
    let grid = Grid::read(&doc, text)?;
    for row in rows.0..=rows.1 {
        for column in columns.0..=columns.1 {
            if (column, row) == (columns.0, rows.0) {
                continue;
            }
            let held = grid.cell(column, row).is_some_and(|cell| {
                child(cell, "f").is_some()
                    || child(cell, "is").is_some()
                    || child(cell, "v").is_some_and(|v| v.text().is_some_and(|t| !t.is_empty()))
            });
            ensure!(
                !held,
                "{}{row} holds a value the merge of {range} would hide; clear it, or merge a range only its top left cell fills",
                column_name(column)?
            );
        }
    }
    let area = Area {
        columns: Some(columns),
        rows: Some(rows),
    };
    add_merges(text, &[area.render()?])
}

/// The worksheet with the merges inside `columns` × `rows` taken apart.
fn unmerge(text: &str, range: &str, columns: (u32, u32), rows: (u32, u32)) -> Result<String> {
    let doc = xml(text.as_bytes())?;
    let inside: Vec<_> = merges(&doc)?
        .into_iter()
        .filter(|((c, r), _)| {
            columns.0 <= c.0 && c.1 <= columns.1 && rows.0 <= r.0 && r.1 <= rows.1
        })
        .map(|(_, node)| node.range())
        .collect();
    ensure!(
        !inside.is_empty(),
        "{range} holds no merged range to take apart"
    );
    tidy_containers(apply_edits(text, vec![], inside)?)
}
