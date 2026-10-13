//! How corrections name cells: by the identities of their rows and columns,
//! which a layout keeps while rows and columns move, and as ranges of the
//! cells of an extraction.
use super::*;
use std::collections::HashMap;

/// A cell position: its column and row number, as [`crate::excel::coordinate`]
/// gives them.
pub(super) type Point = (u32, u32);

/// The identities of the rows and columns of an axis by position.
#[derive(Default)]
struct Axis {
    ids: BTreeMap<u32, String>,
    positions: HashMap<String, u32>,
}

impl Axis {
    fn new(identities: impl IntoIterator<Item = (u32, String)>) -> Result<Self> {
        let mut axis = Self::default();
        for (position, id) in identities {
            ensure!(
                axis.positions.insert(id.clone(), position).is_none()
                    && axis.ids.insert(position, id).is_none(),
                "duplicate row or column identity"
            );
        }
        Ok(axis)
    }
}

/// The identities of the rows and columns of each sheet, at the positions of
/// the extraction a correction journal belongs to. A correction names a cell
/// by them, so a row or column inserted before it changes its position but
/// not its name. A sheet without identities names its cells by address.
#[derive(Default)]
pub struct Axes {
    sheets: HashMap<String, (Axis, Axis)>,
}

impl Axes {
    /// Names the cells of `sheet` by the identities of `rows` and `columns`,
    /// each with its position: a row identity `r<n>` reads as the number n and
    /// a column identity of capital letters as itself, so cells keep their
    /// addresses until rows or columns move; any other identity reads in
    /// brackets, as in `J[ins-1]`.
    pub fn insert(
        &mut self,
        sheet: &str,
        rows: impl IntoIterator<Item = (u32, String)>,
        columns: impl IntoIterator<Item = (u32, String)>,
    ) -> Result<()> {
        ensure!(
            self.sheets
                .insert(sheet.to_owned(), (Axis::new(rows)?, Axis::new(columns)?))
                .is_none(),
            "duplicate sheet axes"
        );
        Ok(())
    }

    /// The name of the cell at `point` on `sheet`.
    pub(super) fn name(&self, sheet: &str, (column, row): Point) -> Result<String> {
        let Some((rows, columns)) = self.sheets.get(sheet) else {
            return Ok(format!("{}{row}", crate::excel::column_name(column)?));
        };
        let column = columns
            .ids
            .get(&column)
            .with_context(|| format!("{sheet}: column {column} has no identity"))?;
        let row = rows
            .ids
            .get(&row)
            .with_context(|| format!("{sheet}: row {row} has no identity"))?;
        let column = if !column.is_empty() && column.bytes().all(|b| b.is_ascii_uppercase()) {
            column.clone()
        } else {
            format!("[{column}]")
        };
        let row = match row.strip_prefix('r') {
            Some(number) if number.parse::<u32>().is_ok_and(|n| format!("r{n}") == *row) => {
                number.to_owned()
            }
            _ => format!("[{row}]"),
        };
        Ok(format!("{column}{row}"))
    }

    /// The position of the cell `name` names on `sheet`.
    pub(super) fn point(&self, sheet: &str, name: &str) -> Result<Point> {
        let Some((rows, columns)) = self.sheets.get(sheet) else {
            return crate::excel::coordinate(name);
        };
        let (column, row) = if let Some(rest) = name.strip_prefix('[') {
            let (column, row) = rest
                .split_once(']')
                .with_context(|| format!("invalid cell name {name}"))?;
            (column.to_owned(), row)
        } else {
            let split = name
                .find(|c: char| !c.is_ascii_uppercase())
                .with_context(|| format!("invalid cell name {name}"))?;
            (name[..split].to_owned(), &name[split..])
        };
        let row = match row.strip_prefix('[') {
            Some(id) => id
                .strip_suffix(']')
                .with_context(|| format!("invalid cell name {name}"))?
                .to_owned(),
            None => format!("r{row}"),
        };
        let column = columns
            .positions
            .get(&column)
            .with_context(|| format!("{sheet}: no column {column} for {name}"))?;
        let row = rows
            .positions
            .get(&row)
            .with_context(|| format!("{sheet}: no row {row} for {name}"))?;
        Ok((*column, *row))
    }

    /// The name of the range from `first` to `last` on `sheet`: one cell's
    /// name when they are the same cell.
    pub(super) fn range(&self, sheet: &str, (first, last): (Point, Point)) -> Result<String> {
        let first_name = self.name(sheet, first)?;
        Ok(if first == last {
            first_name
        } else {
            format!("{first_name}:{}", self.name(sheet, last)?)
        })
    }

    /// The first and last cell of the range `name` names on `sheet`.
    pub(super) fn bounds(&self, sheet: &str, name: &str) -> Result<(Point, Point)> {
        let (first, last) = name.split_once(':').unwrap_or((name, name));
        let (first, last) = (self.point(sheet, first)?, self.point(sheet, last)?);
        ensure!(
            first.0 <= last.0 && first.1 <= last.1,
            "{sheet}: reversed range {name}"
        );
        Ok((first, last))
    }
}

/// The cells of a sheet of an extraction, by row and column.
#[derive(Default)]
pub(super) struct Cells(BTreeMap<u32, BTreeSet<u32>>);

impl Cells {
    pub(super) fn of(sheet: &Value) -> Result<Self> {
        let mut cells = Self::default();
        for cell in array(&sheet["cells"])? {
            let (column, row) = crate::excel::coordinate(string(&cell["address"])?)?;
            cells.0.entry(row).or_default().insert(column);
        }
        Ok(cells)
    }

    /// How many cells lie between columns `left` and `right` of rows `top`
    /// to `bottom`.
    fn count(&self, (left, top): Point, (right, bottom): Point) -> usize {
        if left > right || top > bottom {
            return 0;
        }
        self.0
            .range(top..=bottom)
            .map(|(_, columns)| columns.range(left..=right).count())
            .sum()
    }

    /// The cells of the range from `first` to `last`, row by row.
    pub(super) fn within(&self, (first, last): (Point, Point)) -> Vec<Point> {
        self.0
            .range(first.1..=last.1)
            .flat_map(|(row, columns)| {
                columns
                    .range(first.0..=last.0)
                    .map(move |column| (*column, *row))
            })
            .collect()
    }

    /// The fewest ranges, found greedily, whose cells read row by row and range
    /// after range are `points` in their order. Each point must be a cell of
    /// the sheet, and no point may come twice.
    pub(super) fn ranges(&self, points: &[Point]) -> Vec<(Point, Point)> {
        let mut ranges = vec![];
        let mut start = 0;
        while start < points.len() {
            // The points from `start` on are taken into a range while the
            // cells of their bounds, row by row, begin with them; the range
            // ends at the last point that completes those cells.
            let (mut left, mut right, top) = (points[start].0, points[start].0, points[start].1);
            let mut bottom = top;
            // The cells within the bounds.
            let mut cells = 1;
            let mut end = start;
            let mut next = start + 1;
            while let Some(&(column, row)) = points.get(next) {
                let (previous_column, previous_row) = points[next - 1];
                if (row, column) <= (previous_row, previous_column) {
                    break;
                }
                if column < left || column > right {
                    (left, right) = (left.min(column), right.max(column));
                    cells = self.count((left, top), (right, bottom));
                }
                if row > bottom {
                    cells += self.count((left, bottom + 1), (right, row));
                    bottom = row;
                }
                // The cells after this point in its row, the last of the bounds.
                let after = self.count((column + 1, row), (right, row));
                let taken = next - start + 1;
                if cells - after != taken {
                    break;
                }
                if after == 0 {
                    end = next;
                }
                next += 1;
            }
            let bounds = points[start..=end].iter().fold(
                ((u32::MAX, u32::MAX), (0, 0)),
                |((left, top), (right, bottom)), &(column, row)| {
                    (
                        (left.min(column), top.min(row)),
                        (right.max(column), bottom.max(row)),
                    )
                },
            );
            ranges.push(bounds);
            start = end + 1;
        }
        ranges
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cells(addresses: &[&str]) -> Cells {
        Cells::of(
            &json!({"cells":addresses.iter().map(|a| json!({"address":a})).collect::<Vec<_>>()}),
        )
        .unwrap()
    }

    fn points(addresses: &[&str]) -> Vec<Point> {
        addresses
            .iter()
            .map(|a| crate::excel::coordinate(a).unwrap())
            .collect()
    }

    #[test]
    fn ranges_read_back_the_cells_in_their_order() {
        let sheet = cells(&["B2", "D2", "B3", "C3", "D3", "F3", "B5", "D5"]);
        for members in [
            vec!["B2", "D2", "B3", "C3", "D3"],
            vec!["B2", "D2", "B3", "D3", "B5"],
            vec!["D3", "B2", "F3"],
            vec!["B5", "D5"],
        ] {
            let members = points(&members);
            let ranges = sheet.ranges(&members);
            let read: Vec<_> = ranges.iter().flat_map(|r| sheet.within(*r)).collect();
            assert_eq!(read, members, "{ranges:?}");
        }
        assert_eq!(
            sheet.ranges(&points(&["B2", "D2", "B3", "C3", "D3"])).len(),
            1
        );
    }

    #[test]
    fn cells_are_named_by_the_identities_of_their_rows_and_columns() {
        let mut axes = Axes::default();
        axes.insert(
            "S",
            [(1, "r1".into()), (2, "ins-1".into()), (3, "r2".into())],
            [(1, "A".into()), (2, "add-1".into()), (3, "B".into())],
        )
        .unwrap();
        for (point, name) in [
            ((1, 1), "A1"),
            ((3, 3), "B2"),
            ((1, 2), "A[ins-1]"),
            ((2, 3), "[add-1]2"),
        ] {
            assert_eq!(axes.name("S", point).unwrap(), name);
            assert_eq!(axes.point("S", name).unwrap(), point);
        }
        assert_eq!(axes.name("other", (2, 3)).unwrap(), "B3");
        assert!(axes.name("S", (4, 1)).is_err());
    }
}
