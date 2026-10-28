//! The cells of a worksheet whose operations shift cells within a band
//! (`insert_cells`, `delete_cells`). Such cells leave their rows, so the
//! sheet is laid out cell by cell rather than row by row: its cells and rows
//! are taken from the original and every operation is applied in the order
//! recorded, Excel's way. Whole rows carry their attributes; cells move with
//! the operations that move them, and inserted cells take the format of the
//! cell before them, as Excel formats inserted rows, columns and cells. The
//! cells are then written where they end, with their formulas rewritten.
use super::*;

/// What a place of the laid-out sheet holds.
enum Slot<'a> {
    /// A cell of the original.
    Cell(Node<'a, 'a>),
    /// A cell an insertion formats, holding only that format (`s`).
    Styled(String),
}

/// A row of the laid-out sheet: the original row's opening tag, or the one
/// an inserted row takes from its style row; none for a row only cells make.
type RowTag = Option<String>;

/// The worksheet laid out as the operations go.
struct Layout<'a> {
    /// By (row, column).
    cells: BTreeMap<(u32, u32), Slot<'a>>,
    rows: BTreeMap<u32, RowTag>,
}

impl<'a> Layout<'a> {
    fn style(&self, place: (u32, u32)) -> Option<String> {
        match self.cells.get(&place)? {
            Slot::Cell(cell) => cell.attribute("s").map(str::to_owned),
            Slot::Styled(style) => Some(style.clone()),
        }
        .filter(|style| style != "0")
    }

    /// Applies one operation of the sheet; `style_rows` gives an inserted
    /// row's tag from the original row its style comes from, and `done` are
    /// the operations applied before.
    fn apply(
        &mut self,
        operation: &StructuralOperation,
        style_rows: &BTreeMap<u32, &str>,
        done: &[StructuralOperation],
        borders: &mut super::writeback::SharedBorders<'_>,
    ) -> Result<()> {
        ensure!(
            !matches!(operation.kind, OperationKind::MoveColumns { .. }),
            "{} moves columns of a sheet whose cells other operations shift within a band; record them separately",
            operation.id
        );
        let rows = operation.row_operation();
        // The formats inserted cells take: those of the cells before them
        // on the operation's axis, read before anything moves.
        let mut formats = vec![];
        if operation.insertion() && operation.at > 1 {
            let before = operation.at - 1;
            let crossing: Vec<u32> = match operation.band {
                Some((first, last)) => (first..=last).collect(),
                None if rows => vec![],
                None => self
                    .cells
                    .keys()
                    .map(|(row, _)| *row)
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect(),
            };
            for across in crossing {
                let place = if rows {
                    (before, across)
                } else {
                    (across, before)
                };
                if let Some(style) = self.style(place) {
                    // Across columns, Excel keeps only the borders the cells
                    // on both sides of the insertion draw alike.
                    let style = if rows {
                        style
                    } else {
                        let next = self.style((across, operation.at));
                        borders.between(&style, next.as_deref())?
                    };
                    formats.push((across, style));
                }
            }
        }
        let mut cells = BTreeMap::new();
        for ((row, column), slot) in std::mem::take(&mut self.cells) {
            if !operation.moves_cell(column, row) {
                cells.insert((row, column), slot);
                continue;
            }
            let position = if rows { row } else { column };
            let Some(moved) = shift_index(position, operation) else {
                ensure!(
                    !operation.insertion(),
                    "{} would push cells past the end of the sheet",
                    operation.id
                );
                continue;
            };
            ensure!(
                moved <= if rows { MAX_ROW } else { MAX_COLUMN },
                "{} would push cells past the end of the sheet; delete cells at the end first",
                operation.id
            );
            cells.insert(if rows { (moved, column) } else { (row, moved) }, slot);
        }
        self.cells = cells;
        if rows && operation.whole() {
            let mut moved = BTreeMap::new();
            for (row, tag) in std::mem::take(&mut self.rows) {
                if let Some(row) = shift_index(row, operation) {
                    moved.insert(row, tag);
                }
            }
            self.rows = moved;
            if operation.insertion() {
                let current = operation
                    .style_from
                    .and_then(|from| transform_index(from, done, true));
                for offset in 0..operation.count {
                    let row = operation.at + offset;
                    let template = operation
                        .style_from
                        .map(|from| from + offset % operation.style_rows.max(1));
                    let tag = template
                        .and_then(|from| style_rows.get(&from))
                        .map(|raw| inserted_row_tag(raw, row))
                        .transpose()?;
                    self.rows.insert(row, tag);
                    // Excel formats every cell of an inserted row like its
                    // style row, the empty ones that draw borders included.
                    let Some(source) =
                        current.map(|row| row + offset % operation.style_rows.max(1))
                    else {
                        continue;
                    };
                    let source = if source >= operation.at {
                        source + operation.count
                    } else {
                        source
                    };
                    let styled: Vec<(u32, String)> = self
                        .cells
                        .range((source, 0)..=(source, u32::MAX))
                        .filter_map(|(&(_, column), _)| {
                            Some((column, self.style((source, column))?))
                        })
                        .collect();
                    for (column, style) in styled {
                        self.cells.insert((row, column), Slot::Styled(style));
                    }
                }
                return Ok(());
            }
        }
        for offset in 0..if operation.insertion() {
            operation.count
        } else {
            0
        } {
            let along = operation.at + offset;
            for (across, style) in &formats {
                let place = if rows {
                    (along, *across)
                } else {
                    (*across, along)
                };
                self.cells.insert(place, Slot::Styled(style.clone()));
            }
        }
        Ok(())
    }
}

/// The opening tag an inserted row `row` takes from its style row's `raw`
/// XML: shown even when that row is hidden or collapsed.
fn inserted_row_tag(raw: &str, row: u32) -> Result<String> {
    let (opening, _) = xml_opening(raw)?;
    let opening = replace_xml_attribute(opening, "r", &row.to_string())?;
    let opening = remove_xml_attribute(&opening, "hidden")?;
    remove_xml_attribute(&opening, "collapsed")
}

/// The rows of `sheet_data` as the sheet's `operations` leave them, with the
/// values of `changes` and the calculated column formulas of
/// `table_formulas` written in, both by final (row, column).
pub(super) fn shifted_rows(
    sheet_data: Node<'_, '_>,
    prefix: &str,
    context: &RowTransform<'_, '_, '_>,
    formats: &super::writeback::CellFormats<'_>,
    table_formulas: &BTreeMap<(u32, u32), &str>,
    borders: &mut super::writeback::SharedBorders<'_>,
) -> Result<Vec<RowOutput>> {
    let mut layout = Layout {
        cells: BTreeMap::new(),
        rows: BTreeMap::new(),
    };
    let mut style_rows = BTreeMap::new();
    for row in sheet_data
        .children()
        .filter(|node| node.has_tag_name((NS, "row")))
    {
        let number: u32 = row.attribute("r").context("missing row number")?.parse()?;
        let raw = &context.original[row.range()];
        style_rows.insert(number, raw);
        let (opening, _) = xml_opening(raw)?;
        layout.rows.insert(number, Some(opening.to_owned()));
        for cell in row.children().filter(|node| node.has_tag_name((NS, "c"))) {
            let (column, row) = coordinate(cell.attribute("r").context("missing cell address")?)?;
            layout.cells.insert((row, column), Slot::Cell(cell));
        }
    }
    for (index, operation) in context.operations.iter().enumerate() {
        layout.apply(
            operation,
            &style_rows,
            &context.operations[..index],
            borders,
        )?;
    }
    // Rows the cells end in, the values and the formulas need.
    let mut numbers: BTreeSet<u32> = layout.rows.keys().copied().collect();
    numbers.extend(layout.cells.keys().map(|(row, _)| *row));
    numbers.extend(context.changes.keys().map(|(row, _)| *row));
    numbers.extend(table_formulas.keys().map(|(row, _)| *row));
    let mut output = Vec::with_capacity(numbers.len());
    for number in numbers {
        let opening = match layout.rows.get(&number).cloned().flatten() {
            Some(tag) => {
                // Cells moved into or out of the row leave its span of
                // columns behind; Excel does not need it.
                let tag = replace_xml_attribute(&tag, "r", &number.to_string())?;
                let tag = remove_xml_attribute(&tag, "spans")?;
                format!(
                    "{}>",
                    tag.trim_end_matches('>').trim_end_matches('/').trim_end()
                )
            }
            None => format!("<{prefix}row r=\"{number}\">"),
        };
        let tag = element_tag(&opening)?.to_owned();
        let mut row = RowOutput {
            number,
            raw: format!("{opening}</{tag}>"),
            cells: BTreeSet::new(),
        };
        let mut columns: BTreeSet<u32> = layout
            .cells
            .range((number, 0)..=(number, u32::MAX))
            .map(|(&(_, column), _)| column)
            .collect();
        columns.extend(
            context
                .changes
                .range((number, 0)..=(number, u32::MAX))
                .map(|(&(_, column), _)| column),
        );
        columns.extend(
            table_formulas
                .range((number, 0)..=(number, u32::MAX))
                .map(|(&(_, column), _)| column),
        );
        let mut cells = vec![];
        for column in columns {
            let address = format!("{}{number}", column_name(column)?);
            let slot = layout.cells.get(&(number, column));
            let value = context.changes.get(&(number, column));
            let rendered = match (slot, value) {
                (Some(Slot::Cell(cell)), Some(value)) => {
                    render_cell(context.original, *cell, &address, value, context.shared)?
                }
                (Some(Slot::Cell(cell)), None) => rewrite_cell(*cell, &address, context)?,
                (Some(Slot::Styled(style)), Some(value)) => {
                    render_new_cell(&address, value, prefix, Some(style))?
                }
                (None, Some(value)) => render_new_cell(
                    &address,
                    value,
                    prefix,
                    formats.style(&row, column).as_deref(),
                )?,
                (slot, None) => {
                    let style: Option<String> = match slot {
                        Some(Slot::Styled(style)) => Some(style.clone()),
                        _ => formats.style(&row, column),
                    };
                    let style = style
                        .map(|s: String| format!(" s=\"{}\"", xml_attr(&s)))
                        .unwrap_or_default();
                    match table_formulas.get(&(number, column)) {
                        Some(formula) => format!(
                            "<{prefix}c r=\"{address}\"{style}><{prefix}f>{}</{prefix}f></{prefix}c>",
                            xml_attr(formula)
                        ),
                        None => format!("<{prefix}c r=\"{address}\"{style}/>"),
                    }
                }
            };
            cells.push((column, rendered));
        }
        row.insert_cells(cells)?;
        output.push(row);
    }
    Ok(output)
}
