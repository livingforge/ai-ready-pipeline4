//! Aligns the extraction of an original edited outside ARP with the extraction
//! its interpretation was made from. The rows, then the columns, of each sheet
//! that match none on the other side are taken as inserted or deleted and
//! returned as the row and column operations `carry` applies, as `documents
//! apply` records them. A row or column whose values changed is paired with its
//! counterpart and stays where it was, so its cells only change in value.
use super::*;
use crate::excel::{OperationKind, StructuralOperation};
use std::collections::HashMap;

/// The largest number of inserted and deleted lines traced line by line. A
/// larger difference is paired as one block, by similarity or in order.
const MAX_EDITS: usize = 1000;
/// The largest block of unmatched lines, old times new, paired by similarity;
/// a larger one is paired in order.
const MAX_PAIRING: usize = 250_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    /// The old and the new line are the same line, in value or changed.
    Pair(usize, usize),
    Delete(usize),
    Insert(usize),
}

/// The row and column operations that turn the sheets of `before` into those
/// of `after`, in the order `carry` applies them: each position is where it
/// lies after the operations before it.
pub(super) fn operations(before: &Value, after: &Value) -> Result<Vec<StructuralOperation>> {
    let old_sheets = array(&before["sheets"])?;
    let new_sheets = array(&after["sheets"])?;
    let names =
        |sheets: &[Value]| -> Vec<Value> { sheets.iter().map(|s| s["name"].clone()).collect() };
    ensure!(names(old_sheets) == names(new_sheets), "the sheets changed");
    let mut operations = vec![];
    for (index, (old, new)) in old_sheets.iter().zip(new_sheets).enumerate() {
        let sheet = string(&old["name"])?;
        let old_cells = cells(old)?;
        let new_cells = cells(new)?;
        let prefix = format!("aligned-s{}", index + 1);

        // Rows first: a row is the values along it, wherever they stand, so a
        // column inserted without values leaves every row as it was.
        let row_start = first_line(&old_cells, &new_cells, true);
        let rows = align(
            &lines(&old_cells, true, row_start, |_| true),
            &lines(&new_cells, true, row_start, |_| true),
        );
        let row_operations = to_operations(&rows, sheet, true, row_start, &format!("{prefix}-r"));

        // Then columns, compared on the rows both sides share, where the row
        // operations put them: a row inserted with values does not change them.
        let mut paired = BTreeMap::new();
        for step in &rows {
            if let Step::Pair(old, new) = step {
                paired.insert(*old as u32 + row_start, *new as u32 + row_start);
            }
        }
        let moved: BTreeMap<(u32, u32), &str> = old_cells
            .iter()
            .filter_map(|((row, column), key)| {
                paired.get(row).map(|row| ((*row, *column), key.as_str()))
            })
            .collect();
        let kept: BTreeSet<u32> = paired.values().copied().collect();
        let new_moved: BTreeMap<(u32, u32), &str> = new_cells
            .iter()
            .filter(|((row, _), _)| kept.contains(row))
            .map(|(position, key)| (*position, key.as_str()))
            .collect();
        let column_start = first_line(&moved, &new_moved, false);
        let columns = align(
            &lines_of(&moved, false, column_start, |_| true),
            &lines_of(&new_moved, false, column_start, |_| true),
        );
        operations.extend(row_operations);
        operations.extend(to_operations(
            &columns,
            sheet,
            false,
            column_start,
            &format!("{prefix}-c"),
        ));
    }
    Ok(operations)
}

/// Each cell's value by (row, column), without the style or the formula text
/// that moving a row rewrites.
fn cells(sheet: &Value) -> Result<BTreeMap<(u32, u32), String>> {
    let mut cells = BTreeMap::new();
    for cell in array(&sheet["cells"])? {
        let (column, row) = crate::excel::coordinate(string(&cell["address"])?)?;
        let value = &cell["value"];
        if value.is_null() && !cell["formula"].is_string() {
            continue;
        }
        cells.insert((row, column), json!([cell["type"], value]).to_string());
    }
    Ok(cells)
}

fn lines(
    cells: &BTreeMap<(u32, u32), String>,
    rows: bool,
    start: u32,
    across: impl Fn(u32) -> bool,
) -> Vec<Vec<String>> {
    lines_of(
        &cells
            .iter()
            .map(|(position, key)| (*position, key.as_str()))
            .collect(),
        rows,
        start,
        across,
    )
}

/// The rows (or columns) from the first to the last holding a value, each the
/// values along it in order. A row keeps only the values; a column keeps the
/// row of each value too, as rows were aligned before columns are. `across`
/// selects the cells counted by their row, for columns.
fn lines_of(
    cells: &BTreeMap<(u32, u32), &str>,
    rows: bool,
    start: u32,
    across: impl Fn(u32) -> bool,
) -> Vec<Vec<String>> {
    let mut lines: BTreeMap<u32, Vec<(u32, String)>> = BTreeMap::new();
    for ((row, column), key) in cells {
        if rows {
            lines
                .entry(*row)
                .or_default()
                .push((*column, (*key).to_owned()));
        } else if across(*row) {
            lines
                .entry(*column)
                .or_default()
                .push((*row, format!("{row}:{key}")));
        }
    }
    let last = lines.keys().next_back().copied().unwrap_or(0);
    (start..=last)
        .map(|line| {
            let mut values = lines.remove(&line).unwrap_or_default();
            values.sort();
            values.into_iter().map(|(_, key)| key).collect()
        })
        .collect()
}

fn first_line<T, U>(
    old: &BTreeMap<(u32, u32), T>,
    new: &BTreeMap<(u32, u32), U>,
    rows: bool,
) -> u32 {
    if old.is_empty() || new.is_empty() {
        return 1;
    }
    old.keys()
        .chain(new.keys())
        .map(|&(row, column)| if rows { row } else { column })
        .min()
        .unwrap_or(1)
}

/// The steps from `old` to `new`: lines equal on both sides pair, and the lines
/// between two such pairs pair by similarity as far as both sides have lines,
/// the rest being deleted or inserted.
fn align(old: &[Vec<String>], new: &[Vec<String>]) -> Vec<Step> {
    // Lines compared as numbers: equal lines get the same one.
    let mut ids: HashMap<&[String], u32> = HashMap::new();
    let a: Vec<u32> = old
        .iter()
        .map(|line| {
            let next = ids.len() as u32;
            *ids.entry(line.as_slice()).or_insert(next)
        })
        .collect();
    let b: Vec<u32> = new
        .iter()
        .map(|line| {
            let next = ids.len() as u32;
            *ids.entry(line.as_slice()).or_insert(next)
        })
        .collect();
    let matches = common(&a, &b);
    let mut steps = vec![];
    let (mut i, mut j) = (0, 0);
    for (mi, mj) in matches.into_iter().chain([(a.len(), b.len())]) {
        pair_block(old, new, i..mi, j..mj, &mut steps);
        if mi < a.len() {
            steps.push(Step::Pair(mi, mj));
        }
        i = mi + 1;
        j = mj + 1;
    }
    steps
}

/// The pairs of equal lines of the longest common subsequence, found by
/// Myers' algorithm after the common start and end, or only those when the
/// lines between them differ in more than [`MAX_EDITS`] lines.
fn common(a: &[u32], b: &[u32]) -> Vec<(usize, usize)> {
    let start = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let end = a[start..]
        .iter()
        .rev()
        .zip(b[start..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let mut pairs: Vec<(usize, usize)> = (0..start).map(|i| (i, i)).collect();
    let (middle_a, middle_b) = (&a[start..a.len() - end], &b[start..b.len() - end]);
    if let Some(middle) = myers(middle_a, middle_b) {
        pairs.extend(middle.into_iter().map(|(i, j)| (i + start, j + start)));
    }
    pairs.extend((0..end).map(|k| (a.len() - end + k, b.len() - end + k)));
    pairs
}

/// The matched positions of a shortest edit script from `a` to `b`, or None
/// when it needs more than [`MAX_EDITS`] insertions and deletions.
fn myers(a: &[u32], b: &[u32]) -> Option<Vec<(usize, usize)>> {
    let (n, m) = (a.len() as isize, b.len() as isize);
    if n == 0 || m == 0 {
        return Some(vec![]);
    }
    let limit = (n + m).min(MAX_EDITS as isize);
    // The furthest x on each diagonal k after d edits, indexed k + d; one
    // vector per d is kept to trace the script back.
    let mut trace: Vec<Vec<isize>> = vec![];
    let mut previous: Vec<isize> = vec![0; 1];
    for d in 0..=limit {
        let mut current = vec![0isize; (2 * d + 1) as usize];
        for k in (-d..=d).step_by(2) {
            let get = |k: isize| previous[(k + d - 1) as usize];
            let mut x = if d == 0 {
                0
            } else if k == -d || (k != d && get(k - 1) < get(k + 1)) {
                get(k + 1)
            } else {
                get(k - 1) + 1
            };
            let mut y = x - k;
            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            current[(k + d) as usize] = x;
            if x >= n && y >= m {
                trace.push(current);
                return Some(backtrack(&trace, a, b));
            }
        }
        trace.push(current.clone());
        previous = current;
    }
    None
}

fn backtrack(trace: &[Vec<isize>], a: &[u32], b: &[u32]) -> Vec<(usize, usize)> {
    let (mut x, mut y) = (a.len() as isize, b.len() as isize);
    let mut pairs = vec![];
    for d in (0..trace.len() as isize).rev() {
        let k = x - y;
        let (previous_k, previous_x) = if d == 0 {
            (0, 0)
        } else {
            let before = &trace[(d - 1) as usize];
            let get = |k: isize| before[(k + d - 1) as usize];
            let previous_k = if k == -d || (k != d && get(k - 1) < get(k + 1)) {
                k + 1
            } else {
                k - 1
            };
            (previous_k, get(previous_k))
        };
        let previous_y = previous_x - previous_k;
        let (start_x, start_y) = if d == 0 {
            (0, 0)
        } else if previous_k == k + 1 {
            (previous_x, previous_y + 1)
        } else {
            (previous_x + 1, previous_y)
        };
        while x > start_x && y > start_y {
            x -= 1;
            y -= 1;
            pairs.push((x as usize, y as usize));
        }
        x = previous_x;
        y = previous_y;
    }
    pairs.reverse();
    pairs
}

/// Pairs the unmatched old lines `a` with the unmatched new lines `b` between
/// two matches. As many as the shorter side holds pair, the choice of which
/// lines of the longer side stay unpaired maximizing the values they share.
fn pair_block(
    old: &[Vec<String>],
    new: &[Vec<String>],
    a: std::ops::Range<usize>,
    b: std::ops::Range<usize>,
    steps: &mut Vec<Step>,
) {
    let (n, m) = (a.len(), b.len());
    let chosen: Vec<(usize, usize)> = if n == 0 || m == 0 {
        vec![]
    } else if n == m || n * m > MAX_PAIRING {
        (0..n.min(m)).map(|k| (k, k)).collect()
    } else {
        best_pairs(&old[a.clone()], &new[b.clone()])
    };
    let (mut i, mut j) = (0, 0);
    for (pi, pj) in chosen.into_iter().chain([(n, m)]) {
        steps.extend((i..pi).map(|k| Step::Delete(a.start + k)));
        steps.extend((j..pj).map(|k| Step::Insert(b.start + k)));
        if pi < n {
            steps.push(Step::Pair(a.start + pi, b.start + pj));
        }
        i = pi + 1;
        j = pj + 1;
    }
}

/// The monotone pairing of every line of the shorter side with a line of the
/// longer one that shares the most values in total.
fn best_pairs(old: &[Vec<String>], new: &[Vec<String>]) -> Vec<(usize, usize)> {
    let swap = old.len() > new.len();
    let (short, long) = if swap { (new, old) } else { (old, new) };
    let (n, m) = (short.len(), long.len());
    fn sorted(lines: &[Vec<String>]) -> Vec<Vec<&String>> {
        lines
            .iter()
            .map(|line| {
                let mut values: Vec<_> = line.iter().collect();
                values.sort_unstable();
                values
            })
            .collect()
    }
    let short_sorted = sorted(short);
    let long_sorted = sorted(long);
    // best[i][j]: the most shared values pairing the first i short lines
    // within the first j long lines.
    let mut best = vec![vec![i64::MIN; m + 1]; n + 1];
    best[0].fill(0);
    for i in 1..=n {
        for j in i..=m {
            let skip = best[i][j - 1];
            let take = best[i - 1][j - 1]
                .saturating_add(shared(&short_sorted[i - 1], &long_sorted[j - 1]));
            best[i][j] = skip.max(take);
        }
    }
    let mut pairs = vec![];
    let (mut i, mut j) = (n, m);
    while i > 0 {
        if j > i && best[i][j] == best[i][j - 1] {
            j -= 1;
        } else {
            pairs.push((i - 1, j - 1));
            i -= 1;
            j -= 1;
        }
    }
    pairs.reverse();
    if swap {
        pairs.into_iter().map(|(s, l)| (l, s)).collect()
    } else {
        pairs
    }
}

/// How many values two lines share, counting repeated values once per match.
fn shared(left: &[&String], right: &[&String]) -> i64 {
    let (mut i, mut j, mut count) = (0, 0, 0);
    while i < left.len() && j < right.len() {
        match left[i].cmp(right[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                count += 1;
                i += 1;
                j += 1;
            }
        }
    }
    count
}

/// The operations of `steps`, each run of deleted or inserted lines one
/// operation at the position it has once the runs before it are applied. An
/// inserted row takes the format of the row above it, as `rows insert` does.
fn to_operations(
    steps: &[Step],
    sheet: &str,
    rows: bool,
    start: u32,
    prefix: &str,
) -> Vec<StructuralOperation> {
    let (insert, delete) = if rows {
        (OperationKind::InsertRows, OperationKind::DeleteRows)
    } else {
        (OperationKind::InsertColumns, OperationKind::DeleteColumns)
    };
    let mut operations: Vec<StructuralOperation> = vec![];
    // The line the next old line stands on, 1-based, with the runs before applied.
    let mut cursor = start;
    let mut above: Option<u32> = (start > 1).then_some(start - 1);
    let mut last: Option<Step> = None;
    for step in steps {
        match *step {
            Step::Pair(old, _) => {
                cursor += 1;
                above = Some(old as u32 + start);
            }
            Step::Delete(_) => {
                if matches!(last, Some(Step::Delete(_))) {
                    operations.last_mut().unwrap().count += 1;
                } else {
                    operations.push(StructuralOperation {
                        id: format!("{prefix}{}", operations.len() + 1),
                        sheet: sheet.to_owned(),
                        kind: delete.clone(),
                        at: cursor,
                        count: 1,
                        style_from: None,
                    });
                }
            }
            Step::Insert(_) => {
                if matches!(last, Some(Step::Insert(_))) {
                    operations.last_mut().unwrap().count += 1;
                } else {
                    operations.push(StructuralOperation {
                        id: format!("{prefix}{}", operations.len() + 1),
                        sheet: sheet.to_owned(),
                        kind: insert.clone(),
                        at: cursor,
                        count: 1,
                        style_from: if rows { above } else { None },
                    });
                }
                cursor += 1;
            }
        }
        last = Some(*step);
    }
    operations
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{hint::black_box, time::Instant};

    fn sheet(cells: &[(&str, &str)]) -> Value {
        json!({"name":"S","merges":[],"cells":cells.iter().map(|(address, value)|
            json!({"address":address,"type":"string","value":value,"formula":null})).collect::<Vec<_>>()})
    }

    fn book(cells: &[(&str, &str)]) -> Value {
        json!({"sheets":[sheet(cells)]})
    }

    fn summary(operations: &[StructuralOperation]) -> Vec<(OperationKind, u32, u32, Option<u32>)> {
        operations
            .iter()
            .map(|o| (o.kind.clone(), o.at, o.count, o.style_from))
            .collect()
    }

    const TABLE: [(&str, &str); 6] = [
        ("A1", "No"),
        ("B1", "Name"),
        ("A2", "1"),
        ("B2", "alpha"),
        ("A3", "2"),
        ("B3", "beta"),
    ];

    #[test]
    fn identical_sheets_need_no_operation() {
        let before = book(&TABLE);
        assert!(operations(&before, &before).unwrap().is_empty());
    }

    #[test]
    fn a_changed_value_stays_in_its_row() {
        let before = book(&TABLE);
        let mut cells = TABLE.to_vec();
        cells[3] = ("B2", "ALPHA");
        assert!(operations(&before, &book(&cells)).unwrap().is_empty());
    }

    #[test]
    fn inserted_and_deleted_rows_become_operations_in_order() {
        let before = book(&TABLE);
        // A note above the table, and the row of 1 removed.
        let after = book(&[
            ("A1", "note"),
            ("A2", "No"),
            ("B2", "Name"),
            ("A3", "2"),
            ("B3", "beta"),
        ]);
        assert_eq!(
            summary(&operations(&before, &after).unwrap()),
            [
                (OperationKind::InsertRows, 1, 1, None),
                (OperationKind::DeleteRows, 3, 1, None),
            ]
        );
    }

    #[test]
    fn an_inserted_row_takes_the_row_above_as_its_style() {
        let before = book(&TABLE);
        let after = book(&[
            ("A1", "No"),
            ("B1", "Name"),
            ("A2", "1"),
            ("B2", "alpha"),
            ("A3", "1.5"),
            ("B3", "new"),
            ("A4", "2"),
            ("B4", "beta"),
        ]);
        assert_eq!(
            summary(&operations(&before, &after).unwrap()),
            [(OperationKind::InsertRows, 3, 1, Some(2))]
        );
    }

    #[test]
    fn a_row_inserted_next_to_a_changed_row_is_told_apart_by_its_values() {
        let before = book(&TABLE);
        // Row 2 changed one value and a new row came before it.
        let after = book(&[
            ("A1", "No"),
            ("B1", "Name"),
            ("A2", "9"),
            ("B2", "other"),
            ("A3", "1"),
            ("B3", "ALPHA"),
            ("A4", "2"),
            ("B4", "beta"),
        ]);
        assert_eq!(
            summary(&operations(&before, &after).unwrap()),
            [(OperationKind::InsertRows, 2, 1, Some(1))]
        );
    }

    #[test]
    fn inserted_columns_are_found_after_rows() {
        let before = book(&TABLE);
        // A column inserted between A and B with values, and a row appended.
        let after = book(&[
            ("A1", "No"),
            ("B1", "Owner"),
            ("C1", "Name"),
            ("A2", "1"),
            ("B2", "sato"),
            ("C2", "alpha"),
            ("A3", "2"),
            ("B3", "kato"),
            ("C3", "beta"),
            ("A4", "3"),
            ("C4", "gamma"),
        ]);
        assert_eq!(
            summary(&operations(&before, &after).unwrap()),
            [
                (OperationKind::InsertRows, 4, 1, Some(3)),
                (OperationKind::InsertColumns, 2, 1, None),
            ]
        );
    }

    #[test]
    fn a_deleted_column_is_found() {
        let before = book(&TABLE);
        let after = book(&[("A1", "Name"), ("A2", "alpha"), ("A3", "beta")]);
        assert_eq!(
            summary(&operations(&before, &after).unwrap()),
            [(OperationKind::DeleteColumns, 1, 1, None)]
        );
    }

    #[test]
    fn paragraphs_inserted_into_a_document_shift_the_ones_after() {
        // Word, PPTX and PDF number their paragraphs as rows of one column.
        let paragraphs = |texts: &[&str]| {
            let addresses: Vec<String> = (1..=texts.len()).map(|i| format!("A{i}")).collect();
            book(
                &addresses
                    .iter()
                    .map(String::as_str)
                    .zip(texts.iter().copied())
                    .collect::<Vec<_>>(),
            )
        };
        let before = paragraphs(&["Title", "Intro", "Body", "End"]);
        let after = paragraphs(&["Title", "Intro", "New one", "New two", "Body", "End"]);
        assert_eq!(
            summary(&operations(&before, &after).unwrap()),
            [(OperationKind::InsertRows, 3, 2, Some(2))]
        );
    }

    #[test]
    fn renamed_sheets_are_not_aligned() {
        let before = book(&TABLE);
        let mut after = before.clone();
        after["sheets"][0]["name"] = json!("T");
        assert!(operations(&before, &after).is_err());
    }

    #[test]
    fn myers_finds_the_longest_common_subsequence() {
        let a = [1, 2, 3, 4, 5, 6];
        let b = [1, 9, 3, 4, 7, 6];
        let pairs = myers(&a, &b).unwrap();
        assert_eq!(pairs, [(0, 0), (2, 2), (3, 3), (5, 5)]);
        assert_eq!(myers(&[], &[1]).unwrap(), []);
    }

    #[test]
    fn a_cell_near_the_last_excel_row_keeps_its_absolute_position() {
        let before = book(&[("A1048575", "value")]);
        let after = book(&[("A1048576", "value")]);
        assert_eq!(
            summary(&operations(&before, &after).unwrap()),
            [(OperationKind::InsertRows, 1048575, 1, Some(1048574))]
        );
        assert!(operations(&after, &after).unwrap().is_empty());
    }

    #[test]
    fn adding_the_first_cell_keeps_the_empty_sheet_alignment() {
        assert_eq!(
            summary(&operations(&book(&[]), &book(&[("A1000", "value")])).unwrap()),
            [(OperationKind::InsertRows, 1, 1000, None)]
        );
    }

    #[test]
    fn trimming_empty_prefix_preserves_dense_row_operations() {
        let cells_for = |mask: u8| {
            [100, 101, 103]
                .into_iter()
                .enumerate()
                .filter(|(bit, _)| mask & (1 << bit) != 0)
                .map(|(_, row)| (format!("A{row}"), format!("value-{row}")))
                .collect::<Vec<_>>()
        };
        for before in 0..8 {
            for after in 0..8 {
                let old = cells_for(before);
                let new = cells_for(after);
                let old = book(
                    &old.iter()
                        .map(|(address, value)| (address.as_str(), value.as_str()))
                        .collect::<Vec<_>>(),
                );
                let new = book(
                    &new.iter()
                        .map(|(address, value)| (address.as_str(), value.as_str()))
                        .collect::<Vec<_>>(),
                );
                let old = cells(&old["sheets"][0]).unwrap();
                let new = cells(&new["sheets"][0]).unwrap();
                let dense = align(
                    &lines(&old, true, 1, |_| true),
                    &lines(&new, true, 1, |_| true),
                );
                let start = first_line(&old, &new, true);
                let trimmed = align(
                    &lines(&old, true, start, |_| true),
                    &lines(&new, true, start, |_| true),
                );
                assert_eq!(
                    summary(&to_operations(&dense, "S", true, 1, "r")),
                    summary(&to_operations(&trimmed, "S", true, start, "r")),
                    "before={before} after={after}"
                );
            }
        }
    }

    #[test]
    #[ignore = "manual performance measurement"]
    fn benchmark_sparse_prefix_and_pairing() {
        let sheet = book(&[("A1048576", "value")]);
        let cells = cells(&sheet["sheets"][0]).unwrap();
        let start = first_line(&cells, &cells, true);
        let mut dense_ms = Vec::new();
        let mut trimmed_ms = Vec::new();
        for _ in 0..3 {
            let now = Instant::now();
            let dense = align(
                &lines(&cells, true, 1, |_| true),
                &lines(&cells, true, 1, |_| true),
            );
            dense_ms.push(now.elapsed().as_secs_f64() * 1000.0);
            let now = Instant::now();
            let trimmed = align(
                &lines(&cells, true, start, |_| true),
                &lines(&cells, true, start, |_| true),
            );
            trimmed_ms.push(now.elapsed().as_secs_f64() * 1000.0);
            assert_eq!(dense.len(), 1_048_576);
            assert_eq!(trimmed, [Step::Pair(0, 0)]);
            black_box((dense, trimmed));
        }
        dense_ms.sort_by(f64::total_cmp);
        trimmed_ms.sort_by(f64::total_cmp);
        eprintln!(
            "sparse_dense_ms={:.2} trimmed_ms={:.2}",
            dense_ms[1], trimmed_ms[1]
        );

        let old: Vec<Vec<String>> = (0..200)
            .map(|row| (0..32).map(|column| format!("{row}-{column}")).collect())
            .collect();
        let new: Vec<Vec<String>> = (0..250)
            .map(|row| {
                (0..32)
                    .map(|column| format!("{}-{column}", row * 4 / 5))
                    .collect()
            })
            .collect();
        fn old_shared(a: &[String], b: &[String]) -> i64 {
            let mut left: Vec<_> = a.iter().collect();
            let mut right: Vec<_> = b.iter().collect();
            left.sort();
            right.sort();
            shared(&left, &right)
        }
        let old_method = || {
            let mut total = 0;
            for a in &old {
                for b in &new {
                    total += old_shared(a, b);
                }
            }
            total
        };
        let new_method = || {
            fn sorted(lines: &[Vec<String>]) -> Vec<Vec<&String>> {
                lines
                    .iter()
                    .map(|line| {
                        let mut values: Vec<_> = line.iter().collect();
                        values.sort_unstable();
                        values
                    })
                    .collect::<Vec<_>>()
            }
            let old_sorted = sorted(&old);
            let new_sorted = sorted(&new);
            let mut total = 0;
            for a in &old_sorted {
                for b in &new_sorted {
                    total += shared(a, b);
                }
            }
            total
        };
        let mut repeated_ms = Vec::new();
        let mut cached_ms = Vec::new();
        for _ in 0..3 {
            let now = Instant::now();
            let repeated = black_box(old_method());
            repeated_ms.push(now.elapsed().as_secs_f64() * 1000.0);
            let now = Instant::now();
            let cached = black_box(new_method());
            cached_ms.push(now.elapsed().as_secs_f64() * 1000.0);
            assert_eq!(repeated, cached);
        }
        repeated_ms.sort_by(f64::total_cmp);
        cached_ms.sort_by(f64::total_cmp);
        eprintln!(
            "pair_repeated_sort_ms={:.2} cached_sort_ms={:.2}",
            repeated_ms[1], cached_ms[1]
        );
    }
}
