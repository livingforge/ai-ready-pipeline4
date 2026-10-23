use super::*;

pub(super) fn moved_column(column: u32, from: u32, count: u32, to: u32) -> u32 {
    if (from..from + count).contains(&column) {
        return to + column - from;
    }
    if to < from && (to..from).contains(&column) {
        return column + count;
    }
    if to > from && (from + count..to + count).contains(&column) {
        return column - count;
    }
    column
}

pub fn coordinate(address: &str) -> Result<(u32, u32)> {
    let split = address
        .find(|c: char| c.is_ascii_digit())
        .context("invalid Excel coordinate")?;
    let (letters, row) = address.split_at(split);
    ensure!(
        !letters.is_empty() && !row.starts_with('0'),
        "invalid Excel coordinate"
    );
    let mut column = 0u32;
    for b in letters.bytes() {
        ensure!(b.is_ascii_uppercase(), "invalid Excel column");
        column = column
            .checked_mul(26)
            .and_then(|c| c.checked_add(u32::from(b - b'A' + 1)))
            .context("Excel column overflow")?;
    }
    let row: u32 = row.parse()?;
    ensure!(
        (1..=16384).contains(&column) && (1..=1048576).contains(&row),
        "Excel coordinate outside bounds"
    );
    Ok((column, row))
}
/// Ranges indexed for per-cell lookups: sorted by first row, with the furthest
/// last row reached up to each one, so a lookup stops once every earlier range
/// ends above the cell instead of scanning all ranges for every cell.
struct AreaIndex<T> {
    areas: Vec<IndexedArea<T>>,
    reach: Vec<u32>,
}

/// First and last row or column, inclusive.
type Span = (u32, u32);

struct IndexedArea<T> {
    rows: Span,
    columns: Span,
    /// Position in the sheet's list, so the first listed range wins.
    order: usize,
    value: T,
}

impl<T: Copy> AreaIndex<T> {
    /// Indexes `(rows, columns, value)` in the order the sheet lists them.
    fn new(areas: Vec<(Span, Span, T)>) -> Self {
        let mut areas: Vec<_> = areas
            .into_iter()
            .enumerate()
            .map(|(order, (rows, columns, value))| IndexedArea {
                rows,
                columns,
                order,
                value,
            })
            .collect();
        areas.sort_by_key(|area| area.rows.0);
        let reach = areas
            .iter()
            .scan(0, |reach, area| {
                *reach = area.rows.1.max(*reach);
                Some(*reach)
            })
            .collect();
        Self { areas, reach }
    }

    fn containing(&self, column: u32, row: u32) -> impl Iterator<Item = &IndexedArea<T>> {
        let end = self.areas.partition_point(|area| area.rows.0 <= row);
        (0..end)
            .rev()
            .take_while(move |&i| self.reach[i] >= row)
            .map(move |i| &self.areas[i])
            .filter(move |area| {
                row <= area.rows.1 && (area.columns.0..=area.columns.1).contains(&column)
            })
    }
}

/// A sheet's merged ranges, parsed once for the cells checked against them.
pub struct Merges<'a>(AreaIndex<&'a str>);

impl<'a> Merges<'a> {
    pub fn new(merges: &'a Value) -> Result<Self> {
        let mut areas = vec![];
        for merge in array(merges)? {
            let merge = string(merge)?;
            let (a, b) = merge.split_once(':').context("invalid merge")?;
            let (c1, r1) = coordinate(a)?;
            let (c2, r2) = coordinate(b)?;
            areas.push(((r1, r2), (c1, c2), merge));
        }
        Ok(Self(AreaIndex::new(areas)))
    }

    /// The merged range that hides `address`: one containing it without it being its
    /// top-left cell. Excel shows only the top-left value, so such cells are never written.
    pub fn hiding(&self, address: &str) -> Result<Option<&'a str>> {
        let (column, row) = coordinate(address)?;
        Ok(self.hiding_at(column, row))
    }

    /// [`Merges::hiding`] the cell at `column` and `row`.
    fn hiding_at(&self, column: u32, row: u32) -> Option<&'a str> {
        self.0
            .containing(column, row)
            .filter(|merge| (column, row) != (merge.columns.0, merge.rows.0))
            .min_by_key(|merge| merge.order)
            .map(|merge| merge.value)
    }

    /// Rejects writing `address` of `sheet` when one of the merges hides it.
    pub fn ensure_not_hidden(&self, sheet: &str, address: &str) -> Result<()> {
        let (column, row) = coordinate(address)?;
        self.ensure_not_hidden_at(sheet, (column, row))
    }

    /// [`Merges::ensure_not_hidden`] for the cell at a (column, row) point.
    pub fn ensure_not_hidden_at(&self, sheet: &str, (column, row): (u32, u32)) -> Result<()> {
        if let Some(merge) = self.hiding_at(column, row) {
            let address = format!("{}{row}", column_name(column)?);
            bail!(
                "{sheet}!{address} is hidden by merged range {merge}: Excel shows only the top-left cell of a merge, so write the value there or add the row or column outside the merge"
            );
        }
        Ok(())
    }
}

/// A sheet's computed ranges (see the extraction's `computed`), parsed once for
/// the cells checked against them. Excel overwrites values written there.
pub struct ComputedRanges<'a> {
    sheet: &'a Value,
    index: AreaIndex<(&'a str, &'a str)>,
}

impl<'a> ComputedRanges<'a> {
    pub fn new(sheet: &'a Value) -> Result<Self> {
        let mut areas = vec![];
        for computed in sheet["computed"].as_array().into_iter().flatten() {
            let range = string(&computed["range"])?;
            let area = Area::parse(range)?;
            areas.push((
                area.rows.unwrap_or((1, u32::MAX)),
                area.columns.unwrap_or((1, u32::MAX)),
                (string(&computed["kind"])?, range),
            ));
        }
        Ok(Self {
            sheet,
            index: AreaIndex::new(areas),
        })
    }

    /// The computed range holding `address`, as its kind and range.
    pub fn find(&self, address: &str) -> Result<Option<(&'a str, &'a str)>> {
        let (column, row) = coordinate(address)?;
        Ok(self
            .index
            .containing(column, row)
            .min_by_key(|computed| computed.order)
            .map(|computed| computed.value))
    }

    /// Rejects writing `address` when Excel fills it from a computed range.
    pub fn ensure_not_computed(&self, address: &str) -> Result<()> {
        if let Some((kind, range)) = self.find(address)? {
            let source = match kind {
                "array" => "an array formula or spill",
                "data_table" => "a What-If data table",
                _ => "a pivot table",
            };
            bail!(
                "{}!{address} is filled by {source} over {range}, which Excel recalculates over any value written there; change its source instead",
                string(&self.sheet["name"])?
            );
        }
        Ok(())
    }
}
/// The sheet's merged ranges as the written workbook keeps them after
/// `operations`: an insertion inside a merge grows it, like Excel, and an
/// insertion copying the merges of its style rows makes them again.
pub fn merges_after(sheet: &Value, operations: &[StructuralOperation]) -> Result<Value> {
    let own = sheet_operations(string(&sheet["name"])?, operations);
    if own.iter().any(|operation| operation.merges) {
        let mut state = MergeState::new(sheet)?;
        for operation in own {
            state.apply(operation)?;
        }
        return state.visible();
    }
    let mut merges = vec![];
    for merge in array(&sheet["merges"])? {
        if let Some(mapped) = map_merge(Area::parse(string(merge)?)?, &own)? {
            merges.push(json!(mapped.render()?));
        }
    }
    Ok(Value::Array(merges))
}

/// The merges insertions copy from their style rows, where `operations`
/// leave them, as A1 ranges.
pub fn copied_merges(sheet: &Value, operations: &[StructuralOperation]) -> Result<Vec<String>> {
    let mut state = MergeState::new(sheet)?;
    for operation in sheet_operations(string(&sheet["name"])?, operations) {
        state.apply(operation)?;
    }
    state.copies()
}

/// Whether two areas share a cell; a missing span is the whole sheet.
fn overlap(a: Area, b: Area) -> bool {
    let meet = |x: Option<(u32, u32)>, y: Option<(u32, u32)>| match (x, y) {
        (Some((a1, a2)), Some((b1, b2))) => a1 <= b2 && b1 <= a2,
        _ => true,
    };
    meet(a.rows, b.rows) && meet(a.columns, b.columns)
}

/// Whether an area is one cell, which Excel keeps as no merge.
fn single(area: Area) -> bool {
    area.columns.is_some_and(|(a, b)| a == b) && area.rows.is_some_and(|(a, b)| a == b)
}

/// Keeps merge coordinates current as structural operations are applied in
/// order. Singleton areas remain internally so each operation is applied to
/// the same ranges as prefix recalculation. An insertion copying the merges
/// of its style rows adds them over each group of rows it inserts.
pub(crate) struct MergeState {
    areas: Vec<Area>,
    /// Which of `areas` an insertion copied from its style rows.
    copied: Vec<bool>,
    /// The merges of the original sheet, which style rows name.
    originals: Vec<Area>,
    /// The operations applied so far.
    done: Vec<StructuralOperation>,
}

impl MergeState {
    pub(crate) fn new(sheet: &Value) -> Result<Self> {
        let areas: Vec<Area> = array(&sheet["merges"])?
            .iter()
            .map(|merge| Area::parse(string(merge)?))
            .collect::<Result<_>>()?;
        Ok(Self {
            copied: vec![false; areas.len()],
            originals: areas.clone(),
            areas,
            done: vec![],
        })
    }

    pub(crate) fn apply(&mut self, operation: &StructuralOperation) -> Result<()> {
        let mut areas = Vec::with_capacity(self.areas.len());
        let mut copied = Vec::with_capacity(self.areas.len());
        for (&area, &copy) in self.areas.iter().zip(&self.copied) {
            if let Some(mapped) = map_area(area, &[operation])? {
                areas.push(mapped);
                copied.push(copy);
            }
        }
        if operation.merges && operation.kind == OperationKind::InsertRows {
            let from = operation
                .style_from
                .context("merges are copied from style rows")?;
            let height = operation.style_rows.max(1);
            let done: Vec<&StructuralOperation> = self.done.iter().collect();
            // The merges within the style rows, where the operations before
            // place their columns, with their first and last style row.
            let mut template = vec![];
            for &original in &self.originals {
                let Some((top, bottom)) = original.rows else {
                    continue;
                };
                if single(original) || top < from || bottom > from + height - 1 {
                    continue;
                }
                let current = map_area(original, &done)?.with_context(|| {
                    format!(
                        "operation {} copies the merges of rows an earlier operation deletes",
                        operation.id
                    )
                })?;
                template.push((current.columns, top, bottom));
            }
            // The copies lie in the inserted rows, so only a merge reaching
            // them, or a copy made before, can be under one: they are checked
            // against those, in the order of the areas, rather than all.
            let inserted = Area {
                columns: None,
                rows: Some((
                    operation.at,
                    operation
                        .at
                        .saturating_add(operation.count.saturating_sub(1)),
                )),
            };
            let mut nearby: Vec<Area> = areas
                .iter()
                .copied()
                .filter(|&other| !single(other) && overlap(inserted, other))
                .collect();
            for group in 0..operation.count / height {
                for &(columns, top, bottom) in &template {
                    let first = operation.at + group * height + (top - from);
                    let area = Area {
                        columns,
                        rows: Some((first, first + (bottom - top))),
                    };
                    if let Some(&other) = nearby.iter().find(|&&other| overlap(area, other)) {
                        bail!(
                            "operation {} would merge {} over the merged range {}; insert the rows outside that merge",
                            operation.id,
                            area.render()?,
                            other.render()?
                        );
                    }
                    if !single(area) {
                        nearby.push(area);
                    }
                    areas.push(area);
                    copied.push(true);
                }
            }
        }
        self.areas = areas;
        self.copied = copied;
        self.done.push(operation.clone());
        Ok(())
    }

    pub(crate) fn visible(&self) -> Result<Value> {
        Ok(Value::Array(
            self.areas
                .iter()
                .filter(|area| !single(**area))
                .map(|area| area.render().map(|range| json!(range)))
                .collect::<Result<_>>()?,
        ))
    }

    /// The merges insertions copied, where the operations so far leave them.
    fn copies(&self) -> Result<Vec<String>> {
        self.areas
            .iter()
            .zip(&self.copied)
            .filter(|(area, copy)| **copy && !single(**area))
            .map(|(area, _)| area.render())
            .collect()
    }
}
pub fn column_number(name: &str) -> Result<u32> {
    coordinate(&format!("{name}1")).map(|(column, _)| column)
}
pub fn column_name(mut column: u32) -> Result<String> {
    ensure!((1..=16384).contains(&column), "Excel column outside bounds");
    let mut out = String::new();
    while column > 0 {
        let digit = ((column - 1) % 26) as u8;
        out.push(char::from(b'A' + digit));
        column = (column - 1) / 26;
    }
    Ok(out.chars().rev().collect())
}

pub(super) fn transform_index(
    mut position: u32,
    operations: &[StructuralOperation],
    row: bool,
) -> Option<u32> {
    for operation in operations {
        if operation.row_operation() != row {
            continue;
        }
        match operation.kind {
            OperationKind::MoveColumns { to } => {
                position = moved_column(position, operation.at, operation.count, to);
            }
            OperationKind::InsertRows | OperationKind::InsertColumns => {
                if position >= operation.at {
                    position = position.checked_add(operation.count)?;
                }
            }
            OperationKind::DeleteRows | OperationKind::DeleteColumns => {
                let end = operation.at.checked_add(operation.count)?;
                if (operation.at..end).contains(&position) {
                    return None;
                }
                if position >= end {
                    position -= operation.count;
                }
            }
        }
    }
    Some(position)
}

/// Maps a drawing marker in 1-based row or column `position`. A marker at the
/// very start of its row or column (`boundary`, offset 0) sits on the edge before
/// it, so rows or columns inserted there go after it. Returns the new position
/// and whether the marker's row or column was deleted (its offset then no longer
/// fits and becomes 0).
pub(super) fn map_anchor_index(
    mut position: u32,
    operations: &[StructuralOperation],
    row: bool,
    boundary: bool,
) -> Result<(u32, bool)> {
    let mut deleted = false;
    for operation in operations {
        if operation.row_operation() != row {
            continue;
        }
        match operation.kind {
            OperationKind::MoveColumns { to } => {
                position = moved_column(position, operation.at, operation.count, to);
            }
            OperationKind::InsertRows | OperationKind::InsertColumns => {
                let moves = if boundary {
                    position > operation.at
                } else {
                    position >= operation.at
                };
                if moves {
                    position = position
                        .checked_add(operation.count)
                        .context("drawing anchor coordinate overflow")?;
                }
            }
            OperationKind::DeleteRows | OperationKind::DeleteColumns => {
                let end = operation
                    .at
                    .checked_add(operation.count)
                    .context("drawing operation range overflow")?;
                if (operation.at..end).contains(&position) {
                    deleted |= !(boundary && position == operation.at);
                    position = operation.at;
                } else if position >= end {
                    position -= operation.count;
                }
            }
        }
    }
    Ok((position, deleted))
}

pub fn map_coordinate(
    sheet: &str,
    address: &str,
    operations: &[StructuralOperation],
) -> Result<Option<String>> {
    let (column, row) = coordinate(address)?;
    let mut mapped_column = column;
    let mut mapped_row = row;
    for operation in operations
        .iter()
        .filter(|operation| operation.sheet == sheet)
    {
        if operation.row_operation() {
            mapped_row = match operation.kind {
                OperationKind::InsertRows => {
                    transform_index(mapped_row, std::slice::from_ref(operation), true)
                        .context("row coordinate overflow")?
                }
                OperationKind::DeleteRows => {
                    let Some(value) =
                        transform_index(mapped_row, std::slice::from_ref(operation), true)
                    else {
                        return Ok(None);
                    };
                    value
                }
                _ => unreachable!(),
            };
        } else {
            mapped_column = match operation.kind {
                OperationKind::InsertColumns | OperationKind::MoveColumns { .. } => {
                    transform_index(mapped_column, std::slice::from_ref(operation), false)
                        .context("column coordinate overflow")?
                }
                OperationKind::DeleteColumns => {
                    let Some(value) =
                        transform_index(mapped_column, std::slice::from_ref(operation), false)
                    else {
                        return Ok(None);
                    };
                    value
                }
                _ => unreachable!(),
            };
        }
    }
    Ok(Some(format!(
        "{}{}",
        column_name(mapped_column)?,
        mapped_row
    )))
}

/// [`map_coordinate`] for the many cells of a sheet or a workbook: a row is
/// mapped through the row operations of its sheet and a column through its
/// column operations once, rather than every cell through every operation,
/// which grew with the cells times the pending operations.
pub struct CoordinateMap<'a> {
    operations: &'a [StructuralOperation],
    /// Each axis's mapped positions, by sheet; none for a deleted one.
    rows: std::cell::RefCell<
        std::collections::HashMap<String, std::collections::HashMap<u32, Option<u32>>>,
    >,
    columns: std::cell::RefCell<
        std::collections::HashMap<String, std::collections::HashMap<u32, Option<u32>>>,
    >,
}

impl<'a> CoordinateMap<'a> {
    pub fn new(operations: &'a [StructuralOperation]) -> Self {
        Self {
            operations,
            rows: Default::default(),
            columns: Default::default(),
        }
    }

    /// Where the cell `address` of `sheet` is after the operations, as
    /// [`map_coordinate`] gives it.
    pub fn map(&self, sheet: &str, address: &str) -> Result<Option<String>> {
        self.map_point(sheet, address)?
            .map(|(column, row)| Ok(format!("{}{}", column_name(column)?, row)))
            .transpose()
    }

    /// [`CoordinateMap::map`] as a (column, row) point.
    pub fn map_point(&self, sheet: &str, address: &str) -> Result<Option<(u32, u32)>> {
        let (column, row) = coordinate(address)?;
        let Some(row) = self.axis(sheet, row, true)? else {
            return Ok(None);
        };
        let Some(column) = self.axis(sheet, column, false)? else {
            return Ok(None);
        };
        Ok(Some((column, row)))
    }

    fn axis(&self, sheet: &str, position: u32, rows: bool) -> Result<Option<u32>> {
        let cache = if rows { &self.rows } else { &self.columns };
        if let Some(&mapped) = cache
            .borrow()
            .get(sheet)
            .and_then(|axis| axis.get(&position))
        {
            return Ok(mapped);
        }
        let mut mapped = Some(position);
        for operation in self
            .operations
            .iter()
            .filter(|operation| operation.sheet == sheet && operation.row_operation() == rows)
        {
            let Some(current) = mapped else { break };
            let next = transform_index(current, std::slice::from_ref(operation), rows);
            mapped = if matches!(
                operation.kind,
                OperationKind::DeleteRows | OperationKind::DeleteColumns
            ) {
                next
            } else {
                Some(next.with_context(|| {
                    if rows {
                        "row coordinate overflow"
                    } else {
                        "column coordinate overflow"
                    }
                })?)
            };
        }
        cache
            .borrow_mut()
            .entry(sheet.to_owned())
            .or_default()
            .insert(position, mapped);
        Ok(mapped)
    }
}

/// Whether row or column operations remove an original axis position.
pub(crate) fn axis_deleted(
    mut position: u32,
    operations: &[StructuralOperation],
    row: bool,
) -> Result<bool> {
    for operation in operations {
        if operation.row_operation() != row {
            continue;
        }
        if let OperationKind::MoveColumns { to } = operation.kind {
            position = moved_column(position, operation.at, operation.count, to);
            continue;
        }
        let end = operation
            .at
            .checked_add(operation.count)
            .context("coordinate operation range overflow")?;
        if operation.insertion() {
            if position >= operation.at {
                position = position
                    .checked_add(operation.count)
                    .context("coordinate overflow")?;
            }
        } else if (operation.at..end).contains(&position) {
            return Ok(true);
        } else if position >= end {
            position -= operation.count;
        }
    }
    Ok(false)
}

pub fn resolve_insertion(
    sheet: &str,
    insertion: &str,
    offset: u32,
    column: Option<&str>,
    row: Option<u32>,
    operations: &[StructuralOperation],
) -> Result<String> {
    let operation = operations
        .iter()
        .find(|operation| operation.id == insertion)
        .context("unknown insertion operation")?;
    ensure!(operation.sheet == sheet, "insertion sheet mismatch");
    ensure!(
        operation.insertion(),
        "target requires an insertion operation"
    );
    ensure!(
        offset < operation.count,
        "insertion offset outside operation"
    );
    let target_row = operation.row_operation();
    ensure!(
        (target_row && column.is_some() && row.is_none())
            || (!target_row && row.is_some() && column.is_none()),
        "insertion target must specify column for row insertion or row for column insertion"
    );
    let mut mapped_column = if target_row {
        column.map(column_number).transpose()?.unwrap()
    } else {
        operation.at
    };
    let mut mapped_row = if target_row {
        operation.at
    } else {
        row.unwrap()
    };
    let mut activated = false;
    for candidate in operations
        .iter()
        .filter(|candidate| candidate.sheet == sheet)
    {
        if candidate.id == insertion {
            if target_row {
                mapped_row = mapped_row
                    .checked_add(offset)
                    .context("inserted row coordinate overflow")?;
            } else {
                mapped_column = mapped_column
                    .checked_add(offset)
                    .context("inserted column coordinate overflow")?;
            }
            activated = true;
            continue;
        }
        // `at` already counts the operations listed before it on the same
        // axis; the other axis is given before any operation.
        if !activated && candidate.row_operation() == target_row {
            continue;
        }
        if candidate.row_operation() {
            mapped_row = transform_index(mapped_row, std::slice::from_ref(candidate), true)
                .context("insertion target was deleted or overflowed")?;
        } else {
            mapped_column = transform_index(mapped_column, std::slice::from_ref(candidate), false)
                .context("insertion target was deleted or overflowed")?;
        }
    }
    ensure!(activated, "insertion operation not found");
    Ok(format!("{}{}", column_name(mapped_column)?, mapped_row))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incremental_merges_match_prefix_recalculation() {
        let sheet = json!({"name":"S","merges":["A1:B1","D3:E5","G7:H8"]});
        let operations: Vec<_> = [
            (OperationKind::DeleteColumns, 2, 1),
            (OperationKind::InsertColumns, 2, 1),
            (OperationKind::InsertRows, 4, 2),
            (OperationKind::DeleteRows, 8, 1),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, (kind, at, count))| StructuralOperation {
            id: format!("op-{index}"),
            sheet: "S".into(),
            kind,
            at,
            count,
            style_from: None,
            style_rows: 1,
            merges: false,
            vba_unchanged: false,
        })
        .collect();
        let mut state = MergeState::new(&sheet).unwrap();
        for (index, operation) in operations.iter().enumerate() {
            state.apply(operation).unwrap();
            assert_eq!(
                state.visible().unwrap(),
                merges_after(&sheet, &operations[..=index]).unwrap()
            );
        }
    }

    #[test]
    fn an_insertion_copies_the_merges_of_its_style_rows_over_each_group() {
        // A form laid out over rows 3 and 4: a name merged down both, a
        // label merged across each.
        let sheet = json!({"name":"S","merges":["A3:A4","B3:C3","B4:C4","E1:F1"]});
        let operation = |id: &str, at, count, style_from, style_rows, merges| StructuralOperation {
            id: id.into(),
            sheet: "S".into(),
            kind: OperationKind::InsertRows,
            at,
            count,
            style_from,
            style_rows,
            merges,
            vba_unchanged: false,
        };
        let copy = operation("copy", 5, 4, Some(3), 2, true);
        // A later insertion above moves the copies with the originals.
        let later = operation("later", 2, 1, None, 1, false);
        assert_eq!(
            copied_merges(&sheet, std::slice::from_ref(&copy)).unwrap(),
            ["A5:A6", "B5:C5", "B6:C6", "A7:A8", "B7:C7", "B8:C8"]
        );
        assert_eq!(
            copied_merges(&sheet, &[copy.clone(), later.clone()]).unwrap(),
            ["A6:A7", "B6:C6", "B7:C7", "A8:A9", "B8:C8", "B9:C9"]
        );
        let merges = merges_after(&sheet, &[copy, later]).unwrap();
        assert!(merges.as_array().unwrap().contains(&json!("A4:A5")));
        // Rows inserted inside a merge grow it; a copy over its columns
        // would overlap it.
        let sheet = json!({"name":"S","merges":["A2:B2","A3:A4"]});
        let inside = operation("inside", 4, 1, Some(2), 1, true);
        let error = merges_after(&sheet, &[inside]).unwrap_err().to_string();
        assert!(
            error.contains("A4:B4 over the merged range A3:A5"),
            "{error}"
        );
    }

    #[test]
    fn cached_axis_deletion_matches_coordinate_mapping() {
        let operations = [
            (OperationKind::InsertRows, 3, 2),
            (OperationKind::DeleteRows, 5, 2),
            (OperationKind::InsertColumns, 2, 1),
            (OperationKind::DeleteColumns, 4, 2),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, (kind, at, count))| StructuralOperation {
            id: format!("op-{index}"),
            sheet: "Sheet1".into(),
            kind,
            at,
            count,
            style_from: None,
            style_rows: 1,
            merges: false,
            vba_unchanged: false,
        })
        .collect::<Vec<_>>();
        for row in 1..20 {
            for column in 1..10 {
                let address = format!("{}{}", column_name(column).unwrap(), row);
                let mapped = map_coordinate("Sheet1", &address, &operations).unwrap();
                let deleted = axis_deleted(row, &operations, true).unwrap()
                    || axis_deleted(column, &operations, false).unwrap();
                assert_eq!(mapped.is_none(), deleted, "{address}");
            }
        }
    }

    #[test]
    fn a_coordinate_map_places_cells_as_mapping_each_does() {
        let operations = [
            ("Sheet1", OperationKind::InsertRows, 3, 2),
            ("Sheet2", OperationKind::DeleteRows, 2, 3),
            ("Sheet1", OperationKind::DeleteRows, 5, 2),
            ("Sheet1", OperationKind::InsertColumns, 2, 1),
            ("Sheet1", OperationKind::MoveColumns { to: 7 }, 3, 2),
            ("Sheet1", OperationKind::DeleteColumns, 4, 2),
            ("Sheet1", OperationKind::InsertRows, 1, 1),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, (sheet, kind, at, count))| StructuralOperation {
            id: format!("op-{index}"),
            sheet: sheet.into(),
            kind,
            at,
            count,
            style_from: None,
            style_rows: 1,
            merges: false,
            vba_unchanged: false,
        })
        .collect::<Vec<_>>();
        let map = CoordinateMap::new(&operations);
        // Twice, the second time from what the first saved.
        for _ in 0..2 {
            for sheet in ["Sheet1", "Sheet2", "Sheet3"] {
                for row in 1..20 {
                    for column in 1..12 {
                        let address = format!("{}{}", column_name(column).unwrap(), row);
                        assert_eq!(
                            map.map(sheet, &address).unwrap(),
                            map_coordinate(sheet, &address, &operations).unwrap(),
                            "{sheet}!{address}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn merge_lookups_find_the_hiding_range() {
        // The tall first merge reaches past the later ones, so lookups below
        // them must keep scanning back to it.
        let merges = json!(["A1:A100", "C2:C3", "E5:F6"]);
        let merges = Merges::new(&merges).unwrap();
        assert_eq!(merges.hiding("A50").unwrap(), Some("A1:A100"));
        assert_eq!(merges.hiding("A1").unwrap(), None);
        assert_eq!(merges.hiding("C3").unwrap(), Some("C2:C3"));
        assert_eq!(merges.hiding("F6").unwrap(), Some("E5:F6"));
        assert_eq!(merges.hiding("E5").unwrap(), None);
        assert_eq!(merges.hiding("B50").unwrap(), None);
        assert_eq!(merges.hiding("A101").unwrap(), None);
        assert!(merges.ensure_not_hidden("Data", "C3").is_err());
    }

    #[test]
    fn computed_lookups_prefer_the_first_listed_range() {
        let sheet = json!({"name": "Data", "computed": [
            {"kind": "pivot", "range": "B5:D9"},
            {"kind": "array", "range": "C1:C6"},
        ]});
        let computed = ComputedRanges::new(&sheet).unwrap();
        assert_eq!(computed.find("C5").unwrap(), Some(("pivot", "B5:D9")));
        assert_eq!(computed.find("C2").unwrap(), Some(("array", "C1:C6")));
        assert_eq!(computed.find("A5").unwrap(), None);
        assert_eq!(computed.find("C10").unwrap(), None);
        assert!(computed.ensure_not_computed("D9").is_err());
    }
}
