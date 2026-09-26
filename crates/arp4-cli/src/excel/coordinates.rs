use super::*;

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
        Ok(self
            .0
            .containing(column, row)
            .filter(|merge| (column, row) != (merge.columns.0, merge.rows.0))
            .min_by_key(|merge| merge.order)
            .map(|merge| merge.value))
    }

    /// Rejects writing `address` of `sheet` when one of the merges hides it.
    pub fn ensure_not_hidden(&self, sheet: &str, address: &str) -> Result<()> {
        if let Some(merge) = self.hiding(address)? {
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
/// `operations`: an insertion inside a merge grows it, like Excel.
pub fn merges_after(sheet: &Value, operations: &[StructuralOperation]) -> Result<Value> {
    let own = sheet_operations(string(&sheet["name"])?, operations);
    let mut merges = vec![];
    for merge in array(&sheet["merges"])? {
        if let Some(mapped) = map_merge(Area::parse(string(merge)?)?, &own)? {
            merges.push(json!(mapped.render()?));
        }
    }
    Ok(Value::Array(merges))
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
                OperationKind::InsertColumns => {
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
