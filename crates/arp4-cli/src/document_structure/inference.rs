//! Deterministic layout hypotheses; never used to change source cells.
//!
//! The inference reads positions, merges and formatting (bold, fill, border).
//! It compares cell texts for equality and recognizes note/list markers when
//! layout also supports prose, but does not assign table roles from words alone.
use super::*;
use crate::excel::{column_name, coordinate};
use std::collections::{HashMap, HashSet};

#[derive(Clone)]
struct Cell<'a> {
    source: &'a Value,
    left: u32,
    top: u32,
    right: u32,
    bottom: u32,
}
impl Cell<'_> {
    fn nonempty(&self) -> bool {
        !self.source["value"].is_null() && self.source["value"] != ""
    }
    fn bold(&self) -> bool {
        self.source["style"]["bold"] == true
    }
    fn fill(&self) -> u64 {
        self.source["style"]["fill"].as_u64().unwrap_or(0)
    }
    fn bordered(&self) -> bool {
        self.source["style"]["border"].as_u64().unwrap_or(0) > 0
    }
    fn string(&self) -> bool {
        self.source["type"] == "string"
    }
    fn text(&self) -> &str {
        self.source["value"].as_str().unwrap_or("")
    }
    fn span(&self) -> (u32, u32) {
        (self.left, self.right)
    }
    /// Leading spaces, which authors use to indent a child under its parent.
    fn indent(&self) -> usize {
        self.text()
            .chars()
            .take_while(|c| matches!(c, ' ' | '\u{3000}'))
            .count()
    }
}

/// Cells of a candidate table, as indices into the sheet's cells.
type Group = Vec<usize>;
/// Header rows of an Excel table definition: (first row, row count).
type Explicit = Option<(u32, u32)>;

fn root(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

fn bounds(cells: &[Cell<'_>], group: &[usize]) -> (u32, u32, u32, u32) {
    let of = || group.iter().map(|i| &cells[*i]);
    (
        of().map(|c| c.left).min().unwrap(),
        of().map(|c| c.top).min().unwrap(),
        of().map(|c| c.right).max().unwrap(),
        of().map(|c| c.bottom).max().unwrap(),
    )
}

/// Cells that start on `top`, in column order.
fn row<'a, 'b>(group: &[&'b Cell<'a>], top: u32) -> Vec<&'b Cell<'a>> {
    group.iter().copied().filter(|c| c.top == top).collect()
}

fn valued<'a, 'b>(cells: Vec<&'b Cell<'a>>) -> Vec<&'b Cell<'a>> {
    cells.into_iter().filter(|c| c.nonempty()).collect()
}

/// Groups of cells that share an edge. Corner-only contacts do not join two
/// tables. Returns single cells when the sheet exceeds the comparison budget.
fn components(cells: &[Cell<'_>], assigned: &BTreeSet<usize>) -> (Vec<Group>, bool) {
    let free: Vec<_> = (0..cells.len()).filter(|i| !assigned.contains(i)).collect();
    let mut budget = 2_000_000u64;
    let mut parent: Vec<_> = (0..cells.len()).collect();
    let mut occupied: HashMap<(u32, u32), usize> = HashMap::new();
    let mut exhausted = false;
    for &i in &free {
        let c = &cells[i];
        let area = u64::from(c.right - c.left + 1) * u64::from(c.bottom - c.top + 1);
        if area > budget {
            exhausted = true;
            break;
        }
        budget -= area;
        for y in c.top..=c.bottom {
            for x in c.left..=c.right {
                if let Some(j) = occupied.insert((x, y), i) {
                    let (r, s) = (root(&mut parent, j), root(&mut parent, i));
                    parent[s] = r;
                }
            }
        }
    }
    if !exhausted {
        for &i in &free {
            let c = &cells[i];
            let right = (c.top..=c.bottom).map(|y| (c.right.saturating_add(1), y));
            let below = (c.left..=c.right).map(|x| (x, c.bottom.saturating_add(1)));
            for position in right.chain(below) {
                if let Some(&j) = occupied.get(&position) {
                    let (r, s) = (root(&mut parent, j), root(&mut parent, i));
                    parent[s] = r;
                }
            }
        }
    }
    let mut groups: BTreeMap<usize, Group> = BTreeMap::new();
    for &i in &free {
        let key = if exhausted { i } else { root(&mut parent, i) };
        groups.entry(key).or_default().push(i);
    }
    (groups.into_values().collect(), exhausted)
}

/// Sparse, unbordered text with an explicit note marker beside a bordered
/// column is a side annotation, not a bridge between neighboring tables. A
/// bordered cell in that column (typically its header) keeps it in the grid.
fn side_annotations(cells: &[Cell<'_>], assigned: &BTreeSet<usize>) -> Vec<usize> {
    let mut columns = HashMap::<u32, Vec<&Cell<'_>>>::new();
    let mut bordered_rows = HashMap::<u32, Vec<u32>>::new();
    for cell in cells.iter().filter(|c| c.nonempty()) {
        columns.entry(cell.left).or_default().push(cell);
        if cell.bordered() {
            bordered_rows.entry(cell.left).or_default().push(cell.top);
        }
    }
    // A column may host another table farther down the sheet. Only the
    // contiguous bordered run next to the annotation defines its context.
    let mut bordered_runs = HashMap::<(u32, u32), (u32, u32)>::new();
    for (column, rows) in &mut bordered_rows {
        rows.sort_unstable();
        rows.dedup();
        let mut start = 0;
        while start < rows.len() {
            let mut end = start + 1;
            while end < rows.len() && rows[end - 1].checked_add(1) == Some(rows[end]) {
                end += 1;
            }
            if end - start >= 3 {
                for &row in &rows[start..end] {
                    bordered_runs.insert((*column, row), (rows[start], rows[end - 1]));
                }
            }
            start = end;
        }
    }
    let annotation = |c: &Cell<'_>| {
        c.string()
            && c.text().trim_start().starts_with('※')
            && !c.bold()
            && !c.bordered()
            && c.fill() == 0
            && c.left == c.right
    };
    cells
        .iter()
        .enumerate()
        .filter(|(i, c)| {
            !assigned.contains(i)
                && c.nonempty()
                && annotation(c)
                && c.left.checked_sub(1).is_some_and(|left| {
                    bordered_runs
                        .get(&(left, c.top))
                        .is_some_and(|(top, bottom)| {
                            columns.get(&c.left).is_some_and(|column| {
                                let first = column.partition_point(|cell| cell.top < *top);
                                let last = column.partition_point(|cell| cell.top <= *bottom);
                                let local = &column[first..last];
                                !local.is_empty()
                                    && local.len() <= 2
                                    && local.iter().all(|cell| annotation(cell))
                            })
                        })
                })
        })
        .map(|(i, _)| i)
        .collect()
}

/// Plain list items between grids, or a short list beside many bordered
/// records, are prose candidates. A marker alone is insufficient: a list can
/// also be the value of a column in an otherwise regular table.
fn prose_bullets(cells: &[Cell<'_>], assigned: &BTreeSet<usize>) -> Vec<usize> {
    let bullet = |c: &Cell<'_>| {
        c.string()
            && c.nonempty()
            && !c.bordered()
            && !c.bold()
            && c.fill() == 0
            && matches!(
                c.text().trim_start().chars().next(),
                Some('・' | '●' | '○' | '•')
            )
    };
    let mut row_counts = HashMap::<u32, (usize, usize, usize)>::new();
    let mut bordered_left = HashMap::<u32, BTreeSet<u32>>::new();
    let mut bullet_rows = HashMap::<u32, Vec<u32>>::new();
    for c in cells.iter().filter(|c| c.nonempty()) {
        let counts = row_counts.entry(c.top).or_default();
        counts.0 += 1;
        counts.1 += usize::from(c.bordered());
        counts.2 += usize::from(c.bordered() && (c.bold() || c.fill() != 0));
        if c.bordered() {
            bordered_left.entry(c.right + 1).or_default().insert(c.top);
        }
        if bullet(c) {
            bullet_rows.entry(c.left).or_default().push(c.top);
        }
    }
    for rows in bullet_rows.values_mut() {
        rows.sort_unstable();
        rows.dedup();
    }
    cells
        .iter()
        .enumerate()
        .filter(|(i, c)| {
            if assigned.contains(i) || !bullet(c) {
                return false;
            }
            let isolated = row_counts.get(&c.top).is_some_and(|counts| counts.0 == 1)
                && c.top
                    .checked_sub(1)
                    .and_then(|row| row_counts.get(&row))
                    .is_some_and(|counts| counts.1 >= 2)
                && c.top
                    .checked_add(1)
                    .and_then(|row| row_counts.get(&row))
                    .is_some_and(|counts| counts.2 >= 2);
            if isolated {
                return true;
            }
            let Some(rows) = bullet_rows.get(&c.left) else {
                return false;
            };
            let Some(position) = rows.iter().position(|row| *row == c.top) else {
                return false;
            };
            let mut start = position;
            while start > 0 && rows[start - 1].checked_add(1) == Some(rows[start]) {
                start -= 1;
            }
            let mut end = position + 1;
            while end < rows.len() && rows[end - 1].checked_add(1) == Some(rows[end]) {
                end += 1;
            }
            let run = &rows[start..end];
            bordered_left.get(&c.left).is_some_and(|neighbor| {
                run.len() >= 2
                    && neighbor.len() >= run.len() * 2
                    && run.iter().all(|row| neighbor.contains(row))
            })
        })
        .map(|(i, _)| i)
        .collect()
}

/// Attaches a standalone caption across a single blank row, except the document heading.
fn attach_captions(cells: &[Cell<'_>], remaining: &mut [Group]) {
    let captions: Vec<_> = remaining
        .iter()
        .enumerate()
        .filter(|(_, g)| g.len() == 1)
        .map(|(i, g)| (i, g[0]))
        .collect();
    for (slot, id) in captions {
        if remaining[slot].is_empty() {
            continue;
        }
        let caption = &cells[id];
        if caption.top <= 2 || !caption.bold() || caption.left == caption.right {
            continue;
        }
        let candidates: Vec<_> = remaining
            .iter()
            .enumerate()
            .filter(|(i, g)| {
                *i != slot
                    && g.len() > 1
                    && bounds(cells, g)
                        == (
                            caption.left,
                            caption.bottom + 2,
                            caption.right,
                            bounds(cells, g).3,
                        )
            })
            .map(|(i, _)| i)
            .collect();
        if let [target] = candidates.as_slice() {
            remaining[*target].push(id);
            remaining[slot].clear();
        }
    }
}

/// Attaches a row of bold headers to the records that begin one blank row
/// below it in the same columns.
fn attach_headers(cells: &[Cell<'_>], remaining: &mut [Group]) {
    for slot in 0..remaining.len() {
        let head = &remaining[slot];
        let (left, top, right, bottom) = match head.as_slice() {
            [] | [_] => continue,
            group => bounds(cells, group),
        };
        if top != bottom
            || !head
                .iter()
                .all(|i| cells[*i].bold() && cells[*i].nonempty())
        {
            continue;
        }
        let spans: BTreeSet<_> = head.iter().map(|i| cells[*i].span()).collect();
        let targets: Vec<_> = (0..remaining.len())
            .filter(|&other| {
                let group = &remaining[other];
                if other == slot || group.is_empty() {
                    return false;
                }
                let (l, t, r, _) = bounds(cells, group);
                let first: Vec<_> = group.iter().filter(|i| cells[**i].top == t).collect();
                (l, t, r) == (left, bottom + 2, right)
                    && first
                        .iter()
                        .map(|i| cells[**i].span())
                        .collect::<BTreeSet<_>>()
                        == spans
                    && !first.iter().all(|i| cells[**i].bold())
            })
            .collect();
        if let [target] = targets.as_slice() {
            let head = std::mem::take(&mut remaining[slot]);
            remaining[*target].extend(head);
        }
    }
}

/// Joins columns set apart by one blank column to the table on their left
/// when they cover the same rows and have no row labels of their own.
fn join_columns(cells: &[Cell<'_>], remaining: &mut [Group]) {
    loop {
        let mut joined = None;
        'search: for a in 0..remaining.len() {
            if remaining[a].is_empty() {
                continue;
            }
            let (left, top, right, bottom) = bounds(cells, &remaining[a]);
            if remaining[a].iter().all(|i| cells[*i].left == left) {
                continue;
            }
            for (b, candidate) in remaining.iter().enumerate() {
                if b == a || candidate.is_empty() {
                    continue;
                }
                let (l, t, _, bt) = bounds(cells, candidate);
                let labelled = candidate
                    .iter()
                    .map(|i| &cells[*i])
                    .any(|c| c.left == l && c.top > t && c.string() && c.nonempty());
                let single = candidate.iter().all(|i| cells[*i].left == l);
                if right.checked_add(2) == Some(l)
                    && (t, bt) == (top, bottom)
                    && (single || !labelled)
                {
                    joined = Some((a, b));
                    break 'search;
                }
            }
        }
        let Some((a, b)) = joined else { break };
        let moved = std::mem::take(&mut remaining[b]);
        remaining[a].extend(moved);
    }
}

/// A stepped row-label column may not touch the rest of its row. Join short,
/// bordered fragments under the same merged header when their rows already
/// contain records to the right. A shared outer border alone does not qualify.
fn attach_stepped_labels(cells: &[Cell<'_>], groups: &mut Vec<(Group, Explicit)>) -> Result<()> {
    let mut candidate_headers = vec![];
    for (table, explicit) in groups.iter() {
        if explicit.is_some() || table.len() < 4 {
            candidate_headers.push(None);
            continue;
        }
        let view: Vec<_> = table.iter().map(|&i| &cells[i]).collect();
        let layout = layout(&view, None)?;
        let stub = layout.header.and_then(|header| {
            view.iter()
                .find(|c| {
                    c.top == header.0
                        && c.bottom == header.1
                        && c.left == layout.left
                        && c.right < layout.right
                })
                .map(|c| (header.1, c.left, c.right, layout.bottom))
        });
        candidate_headers.push(stub);
    }
    let mut moves = vec![];
    for (source, (fragment, explicit)) in groups.iter().enumerate() {
        if explicit.is_some()
            || fragment.len() > 3
            || fragment
                .iter()
                .any(|&i| !cells[i].nonempty() || !cells[i].bordered() || !cells[i].string())
        {
            continue;
        }
        let targets: Vec<_> = groups
            .iter()
            .zip(&candidate_headers)
            .enumerate()
            .filter_map(|(target, ((table, _), header))| {
                if target == source {
                    return None;
                }
                let &(header_end, stub_left, stub_right, bottom) = header.as_ref()?;
                let fits = fragment.iter().all(|&i| {
                    let c = &cells[i];
                    c.top > header_end
                        && c.bottom <= bottom
                        && c.left >= stub_left
                        && c.right <= stub_right
                        && table.iter().map(|&i| &cells[i]).any(|record| {
                            record.top <= c.top
                                && record.bottom >= c.top
                                && record.left > stub_right
                                && record.nonempty()
                                && record.bordered()
                        })
                });
                fits.then_some(target)
            })
            .collect();
        if let [target] = targets.as_slice() {
            moves.push((source, *target));
        }
    }
    for (source, target) in moves {
        let fragment = std::mem::take(&mut groups[source].0);
        groups[target].0.extend(fragment);
    }
    groups.retain(|(group, _)| !group.is_empty());
    for (group, _) in groups {
        group.sort_by_key(|&i| (cells[i].top, cells[i].left));
    }
    Ok(())
}

/// Where a table's title and header rows are.
struct Layout {
    left: u32,
    top: u32,
    right: u32,
    bottom: u32,
    /// Every cell is in one column.
    single: bool,
    /// The first row is one cell across the whole table.
    caption: bool,
    /// First and last header row.
    header: Option<(u32, u32)>,
}

/// A repeated, numbered record key such as `TC-510`. The suffix is a value
/// shape, not a document-specific word; it can link bands whose formatting
/// and column widths change between pages.
fn numbered_key(text: &str) -> Option<(&str, usize, u64)> {
    let start = text.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    let (prefix, digits) = text.split_at(start);
    if digits.len() < 2 || !prefix.chars().any(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    Some((prefix, digits.len(), digits.parse().ok()?))
}

fn record_series(group: &[&Cell<'_>], left: u32) -> Option<(String, usize, u64, u64)> {
    let keys: Vec<_> = group
        .iter()
        .filter(|c| c.left == left && c.nonempty())
        .filter_map(|c| numbered_key(c.text()))
        .collect();
    let first = *keys.first()?;
    if keys.len() < 2
        || !keys.iter().enumerate().all(|(offset, key)| {
            key.0 == first.0
                && key.1 == first.1
                && first.2.checked_add(offset as u64) == Some(key.2)
        })
    {
        return None;
    }
    Some((first.0.to_owned(), first.1, first.2, keys.last()?.2))
}

/// The last header row when the rows from `header_top` are set apart from the
/// records by bold text or by a fill the records do not use.
fn header_end(group: &[&Cell<'_>], header_top: u32, single: bool) -> Option<u32> {
    let first_all = row(group, header_top);
    let first = valued(first_all.clone());
    let last = |cells: &[&Cell<'_>]| cells.iter().map(|c| c.bottom).max().unwrap();
    let all_bold = |cells: &[&Cell<'_>]| cells.iter().all(|c| c.bold());
    if first.is_empty() {
        return None;
    }
    // Merged headings may have unfilled placeholder cells. Compare the cells
    // that actually carry headings, rather than the placeholders underneath.
    let shade = first[0].fill();
    let shaded = shade != 0 && first.iter().all(|c| c.fill() == shade);
    // A fill that the header rows share and no record uses.
    let mut fill_end = None;
    if shaded && (first.len() >= 2 || single) {
        let mut end = last(&first_all);
        loop {
            let next = valued(row(group, end + 1));
            if next.is_empty() || !next.iter().all(|c| c.fill() == shade) {
                break;
            }
            end = last(&next);
        }
        let next_row = group
            .iter()
            .filter(|c| c.top > end && c.nonempty())
            .map(|c| c.top)
            .min();
        if next_row.is_some_and(|at| {
            group
                .iter()
                .filter(|c| c.top == at && c.nonempty())
                .all(|c| c.fill() != shade)
        }) {
            fill_end = Some(end);
        }
    }
    let bold = first.iter().filter(|c| c.bold()).count();
    // One header cell left plain does not make the row a record.
    let mostly_bold = bold == first.len() || (bold >= 2 && bold * 3 >= first.len() * 2);
    let under = valued(row(group, last(&first) + 1));
    // A lone bold cell merged across several columns of a bold row is a group header.
    let group_header = first.len() == 1
        && first[0].bold()
        && first[0].right > first[0].left
        && under.len() >= 2
        && all_bold(&under);
    let mut bold_end = None;
    if (first.len() >= 2 && mostly_bold) || group_header || (single && first[0].bold()) {
        let mut end = last(&first);
        if !single {
            loop {
                let next_all = row(group, end + 1);
                let next = valued(next_all.clone());
                // A bold row in the records' fill under shaded headers is a record, such as a total.
                if next.len() < 2
                    || !all_bold(&next)
                    || (shaded && next_all.iter().any(|c| c.fill() == 0))
                {
                    break;
                }
                end = last(&next);
            }
        }
        bold_end = Some(end);
        // Bold text tells nothing when every row is bold.
        if !group.iter().any(|c| c.top > end) {
            bold_end = Some(fill_end.unwrap_or_else(|| last(&first)));
        }
    }
    bold_end.max(fill_end)
}

fn layout(group: &[&Cell<'_>], explicit: Explicit) -> Result<Layout> {
    let left = group.iter().map(|c| c.left).min().unwrap();
    let right = group.iter().map(|c| c.right).max().unwrap();
    let top = group.iter().map(|c| c.top).min().unwrap();
    let bottom = group.iter().map(|c| c.bottom).max().unwrap();
    let single = group.iter().all(|c| c.span() == group[0].span());
    let first = row(group, top);
    let caption = explicit.is_none()
        && !single
        && first.len() == 1
        && first[0].span() == (left, right)
        && (first[0].bold() || right > left);
    let header_top = if caption { first[0].bottom + 1 } else { top };
    let mut header = match explicit {
        Some((_, 0)) => None,
        Some((top, count)) => Some((
            top,
            top.checked_add(count - 1)
                .context("table header range overflow")?,
        )),
        None => header_end(group, header_top, single).map(|end| (header_top, end)),
    };
    if header.is_none() && explicit.is_none() && !single {
        let first = valued(row(group, header_top));
        if first.len() >= 3
            && first.iter().all(|c| c.string())
            && numbered_key(first[0].text()).is_none()
            && record_series(group, left).is_some()
            && group
                .iter()
                .any(|c| c.top > header_top && c.left == left && numbered_key(c.text()).is_some())
        {
            header = Some((header_top, header_top));
        }
    }
    // Records in columns: every row starts with a marked label and nothing else is marked.
    if header.is_none() && explicit.is_none() && !single && !caption {
        let marked = |c: &Cell<'_>| c.bold() || c.fill() != 0;
        let tops: BTreeSet<_> = group.iter().map(|c| c.top).collect();
        let columns: BTreeSet<_> = group.iter().map(|c| c.left).collect();
        let labelled = group
            .iter()
            .all(|c| (c.left == left) == marked(c) && (c.left != left || c.string()));
        let complete = tops
            .iter()
            .all(|t| group.iter().any(|c| c.top == *t && c.left == left));
        if labelled && complete && tops.len() >= 2 && columns.len() >= 3 {
            header = Some((
                header_top,
                row(group, header_top)
                    .iter()
                    .map(|c| c.bottom)
                    .max()
                    .unwrap(),
            ));
        }
    }
    Ok(Layout {
        left,
        top,
        right,
        bottom,
        single,
        caption,
        header,
    })
}

/// Rows of bold labels after the first record. A row that repeats the last
/// header row is a repeated header; a row with other columns begins another
/// table, from the returned row.
fn later_headers(
    group: &[&Cell<'_>],
    header: (u32, u32),
    split: bool,
) -> (BTreeSet<u32>, Option<u32>) {
    let labels = |cells: Vec<&Cell<'_>>| -> BTreeMap<(u32, u32), String> {
        cells
            .iter()
            .map(|c| (c.span(), c.text().to_owned()))
            .collect()
    };
    let leaf = labels(valued(
        group
            .iter()
            .copied()
            .filter(|c| c.top >= header.0 && c.top <= header.1 && c.bottom == header.1)
            .collect(),
    ));
    let tops: Vec<_> = group
        .iter()
        .map(|c| c.top)
        .filter(|t| *t > header.1)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut repeated = BTreeSet::new();
    for k in 1..tops.len().saturating_sub(1) {
        let cells = valued(row(group, tops[k]));
        if cells.len() < 2 || !cells.iter().all(|c| c.bold() && c.string()) {
            continue;
        }
        let found = labels(cells);
        if found == leaf || (found.len() == leaf.len() && found.values().eq(leaf.values())) {
            repeated.insert(tops[k]);
        } else if split
            && (found.keys().eq(leaf.keys()) || !found.keys().all(|span| leaf.contains_key(span)))
        {
            // A lone cell directly above the new header is that table's title.
            let titled = k >= 2 && valued(row(group, tops[k - 1])).len() == 1;
            return (repeated, Some(tops[if titled { k - 1 } else { k }]));
        }
    }
    (repeated, None)
}

/// Last row of the initial column header, compared only to suggest a possible
/// continuation. Equal headings never merge two tables by themselves.
fn leaf_signature(
    cells: &[Cell<'_>],
    indices: &[usize],
    explicit: Explicit,
) -> Result<Vec<((u32, u32), String)>> {
    let mut group: Vec<_> = indices.iter().map(|i| &cells[*i]).collect();
    group.sort_by_key(|c| (c.top, c.left));
    let Some((first, last)) = layout(&group, explicit)?.header else {
        return Ok(vec![]);
    };
    Ok(group
        .into_iter()
        .filter(|c| {
            c.top >= first && c.top <= last && c.bottom == last && c.string() && c.nonempty()
        })
        .map(|c| (c.span(), c.text().to_owned()))
        .collect())
}

/// Removes unformatted rows from the top and the bottom of a bordered table:
/// a title, a unit note or a footnote written against the table.
fn strip(cells: &[Cell<'_>], group: &mut Group, leftovers: &mut Vec<usize>) {
    let valued: Vec<_> = group.iter().filter(|i| cells[**i].nonempty()).collect();
    if valued.iter().filter(|i| cells[***i].bordered()).count() * 2 < valued.len().max(1) {
        return;
    }
    let plain = |i: &usize| !cells[*i].bordered() && cells[*i].fill() == 0;
    loop {
        if group.is_empty() {
            return;
        }
        let (left, top, right, _) = bounds(cells, group);
        let first: Vec<_> = group
            .iter()
            .copied()
            .filter(|i| cells[*i].top == top)
            .collect();
        let next = first.iter().map(|i| cells[*i].bottom).max().unwrap() + 1;
        let under = group.iter().filter(|i| cells[**i].top == next).count();
        let title = first.len() == 1 && cells[first[0]].span() == (left, right) && right > left;
        if title || !first.iter().all(plain) || first.len() == under {
            break;
        }
        group.retain(|i| !first.contains(i));
        leftovers.extend(first);
    }
    loop {
        if group.is_empty() {
            return;
        }
        let top = group.iter().map(|i| cells[*i].top).max().unwrap();
        let last: Vec<_> = group
            .iter()
            .copied()
            .filter(|i| cells[*i].top == top)
            .collect();
        if !last.iter().all(plain) {
            break;
        }
        group.retain(|i| !last.contains(i));
        leftovers.extend(last);
    }
}

/// Splits a group of touching cells into the tables it holds.
fn refine(cells: &[Cell<'_>], mut group: Group, leftovers: &mut Vec<usize>) -> Result<Vec<Group>> {
    strip(cells, &mut group, leftovers);
    if group.is_empty() {
        return Ok(vec![]);
    }
    group.sort_by_key(|i| (cells[*i].top, cells[*i].left));
    // Two grids can touch along a full vertical edge. A different final row,
    // together with a header and record labels on both sides, gives a much
    // stronger boundary than adjacency gives evidence of one wide grid.
    if let Some((left, right)) = split_touching_tables(cells, &group)? {
        let mut tables = refine(cells, left, leftovers)?;
        tables.extend(refine(cells, right, leftovers)?);
        return Ok(tables);
    }
    let view: Vec<_> = group.iter().map(|i| &cells[*i]).collect();
    let at = match layout(&view, None)?.header {
        Some(header) => later_headers(&view, header, true).1,
        None => None,
    };
    let Some(at) = at else {
        return Ok(vec![group]);
    };
    let (upper, lower) = group.into_iter().partition(|i| cells[*i].top < at);
    let mut tables = vec![upper];
    tables.extend(refine(cells, lower, leftovers)?);
    Ok(tables)
}

fn split_touching_tables(cells: &[Cell<'_>], group: &[usize]) -> Result<Option<(Group, Group)>> {
    let (left, top, right, _) = bounds(cells, group);
    if right - left < 3 {
        return Ok(None);
    }
    let seams: BTreeSet<_> = group
        .iter()
        .map(|&i| cells[i].right)
        .filter(|seam| *seam > left && *seam + 1 < right)
        .collect();
    for seam in seams.into_iter().rev() {
        if group
            .iter()
            .any(|&i| cells[i].left <= seam && cells[i].right > seam)
        {
            continue;
        }
        let (a, b): (Group, Group) = group.iter().copied().partition(|&i| cells[i].right <= seam);
        let plausible = |side: &[usize]| -> Result<Option<u32>> {
            let (l, t, r, bottom) = bounds(cells, side);
            if l == r || t != top || bottom < top + 2 {
                return Ok(None);
            }
            let view: Vec<_> = side.iter().map(|&i| &cells[i]).collect();
            if layout(&view, None)?.header != Some((top, top)) {
                return Ok(None);
            }
            let labels = view
                .iter()
                .filter(|c| c.top > top && c.left == l && c.nonempty() && c.string())
                .count();
            Ok((labels >= 2).then_some(bottom))
        };
        if a.is_empty() || b.is_empty() {
            continue;
        }
        if let (Some(a_bottom), Some(b_bottom)) = (plausible(&a)?, plausible(&b)?)
            && a_bottom != b_bottom
        {
            return Ok(Some((a, b)));
        }
    }
    Ok(None)
}

/// Which cells of the records are row labels.
enum Labels {
    /// Text cells that end at or before this column.
    Through(u32),
    /// Text cells that begin in one of these columns.
    Columns(BTreeSet<u32>),
    None,
}

/// Roles and header links of one table, or `None` when the group is not a table.
fn interpret(
    cells: &[Cell<'_>],
    indices: &[usize],
    explicit: Explicit,
    sheet_index: usize,
) -> Result<Option<(Vec<Value>, String)>> {
    let mut group: Vec<_> = indices.iter().map(|i| &cells[*i]).collect();
    group.sort_by_key(|c| (c.top, c.left));
    let Layout {
        left,
        top,
        right,
        bottom,
        single,
        caption,
        header,
    } = layout(&group, explicit)?;
    let nonempty = valued(group.clone());
    // Numbered prose steps have the same rectangular occupancy as a two-column
    // table, but their punctuation, long sentence cells and lack of grid styling
    // identify a list rather than records with fields.
    if explicit.is_none()
        && right == left + 1
        && header.is_none()
        && nonempty
            .iter()
            .all(|c| !c.bordered() && !c.bold() && c.fill() == 0)
    {
        let rows: BTreeSet<_> = nonempty.iter().map(|c| c.top).collect();
        let steps = rows.iter().enumerate().all(|(index, at)| {
            let line = valued(row(&group, *at));
            line.len() == 2
                && line[0].left == left
                && (line[0].text().trim() == format!("{}.", index + 1)
                    || line[0].source["value"]
                        .as_f64()
                        .is_some_and(|number| number == (index + 1) as f64))
                && line[1].string()
                && line[1].text().chars().count() >= 8
        });
        if rows.len() >= 3 && steps {
            return Ok(None);
        }
    }
    let mut row_counts = BTreeMap::<u32, usize>::new();
    for c in &nonempty {
        *row_counts.entry(c.top).or_default() += 1;
    }
    let multi_rows = row_counts.values().filter(|v| **v >= 2).count();
    let is_table = explicit.is_some()
        || (nonempty.len() >= 3
            && (multi_rows >= 2
                || (caption
                    && group[0].bold()
                    && multi_rows >= 1
                    && nonempty
                        .iter()
                        .filter(|c| row_counts[&c.top] >= 2)
                        .all(|c| c.bordered()))
                || (single && header.is_some() && nonempty.iter().all(|c| c.bordered()))));
    if !is_table {
        return Ok(None);
    }
    let in_header =
        |c: &Cell<'_>| header.is_some_and(|(first, last)| c.top >= first && c.top <= last);
    let repeated = match header {
        Some(header) => later_headers(&group, header, false).0,
        None => BTreeSet::new(),
    };
    let body_top = header.map_or(if caption { group[0].bottom + 1 } else { top }, |h| h.1 + 1);
    // A cell across the whole table heads the records below it.
    let band = |c: &Cell<'_>| {
        !single
            && c.span() == (left, right)
            && c.top >= body_top
            && c.bottom < bottom
            && c.nonempty()
    };
    let body = |c: &Cell<'_>| c.top >= body_top && !repeated.contains(&c.top) && !band(c);
    let leaf: Vec<_> = group
        .iter()
        .filter(|c| in_header(c) && header.is_some_and(|h| c.bottom == h.1) && c.nonempty())
        .collect();
    // A list folded into several blocks repeats its header labels; each block starts with its own label.
    let one_row = header.is_some_and(|(first, last)| first == last);
    let folded = (2..=leaf.len() / 2).find(|period| {
        one_row
            && leaf.len() % period == 0
            && (0..leaf.len()).all(|i| leaf[i].text() == leaf[i % period].text())
    });
    let stub_end = group
        .iter()
        .filter(|c| {
            header
                .is_some_and(|(first, last)| c.top == first && c.bottom == last && c.bottom > c.top)
        })
        .map(|c| c.right)
        .max()
        .or_else(|| {
            // A broad first header may name staggered row labels in several
            // columns, without a vertically merged stub below it.
            group
                .iter()
                .filter(|c| {
                    in_header(c)
                        && c.left == left
                        && c.right < right
                        && c.right > left
                        && group
                            .iter()
                            .filter(|other| {
                                body(other)
                                    && other.left >= left
                                    && other.right <= c.right
                                    && other.string()
                                    && other.nonempty()
                            })
                            .map(|other| other.left)
                            .collect::<BTreeSet<_>>()
                            .len()
                            >= 2
                })
                .map(|c| c.right)
                .max()
        });
    let through = stub_end.unwrap_or_else(|| {
        group
            .iter()
            .filter(|c| c.left == left)
            .map(|c| c.right)
            .min()
            .unwrap_or(left)
    });
    let columns: BTreeSet<_> = group.iter().filter(|c| body(c)).map(|c| c.left).collect();
    let column_all = |column: u32, test: &dyn Fn(&Cell<'_>) -> bool| {
        let mut cells = group
            .iter()
            .filter(|c| body(c) && c.left == column && c.nonempty())
            .peekable();
        cells.peek().is_some() && cells.all(|c| test(c))
    };
    // A vertically merged label groups several records. When the adjoining
    // column names each record and later columns contain numbers, both left
    // columns are row labels, even though only the first touches the edge.
    let nested_labels = !single
        && through == left
        && group
            .iter()
            .any(|c| body(c) && c.left == left && c.bottom > c.top && c.string() && c.nonempty())
        && left
            .checked_add(1)
            .is_some_and(|next| column_all(next, &|c| c.string()))
        && group
            .iter()
            .any(|c| body(c) && c.left > left + 1 && c.nonempty() && !c.string());
    // A year column is an identifier axis even when Excel stores the years as
    // numbers. Require a year heading, several distinct four-digit years and
    // numeric measures to avoid treating ordinary measures as row labels.
    let year_labels = header.is_some() && {
        let year_heading = group.iter().any(|c| {
            in_header(c)
                && c.left == left
                && (c.text().contains('年')
                    || c.text().eq_ignore_ascii_case("year")
                    || c.text().eq_ignore_ascii_case("fy"))
        });
        let years: Vec<_> = group
            .iter()
            .filter(|c| body(c) && c.left == left && c.nonempty())
            .collect();
        year_heading
            && years.len() >= 3
            && years.iter().all(|c| {
                c.source["value"]
                    .as_f64()
                    .is_some_and(|year| (1900.0..=2100.0).contains(&year) && year.fract() == 0.0)
            })
            && years
                .windows(2)
                .all(|pair| pair[0].source["value"] != pair[1].source["value"])
            && group
                .iter()
                .any(|c| body(c) && c.left > left && c.nonempty() && !c.string())
    };
    let labels = if single {
        Labels::None
    } else if let Some(period) = folded {
        Labels::Columns(leaf.iter().step_by(period).map(|c| c.left).collect())
    } else {
        // A form marks its labels with a fill and leaves the values plain.
        let shaded: BTreeSet<_> = columns
            .iter()
            .copied()
            .filter(|column| column_all(*column, &|c| c.fill() != 0 && c.string()))
            .collect();
        let plain = columns
            .iter()
            .any(|column| column_all(*column, &|c| c.fill() == 0));
        let texts: Vec<_> = columns
            .iter()
            .copied()
            .filter(|column| column_all(*column, &|c| c.string()))
            .collect();
        if header.is_none() && plain && shaded.contains(&left) && shaded.len() >= 2 {
            Labels::Columns(shaded)
        } else if nested_labels {
            Labels::Through(left + 1)
        } else if year_labels {
            Labels::Through(left)
        } else if group
            .iter()
            .any(|c| body(c) && c.right <= through && c.string())
        {
            Labels::Through(through)
        } else if let (Some(_), [column]) = (header, texts.as_slice()) {
            // The only column of text among numbers names the records.
            Labels::Columns(BTreeSet::from([*column]))
        } else {
            Labels::Through(through)
        }
    };
    let role = |c: &Cell<'_>| {
        if caption && c.top == top {
            "text"
        } else if in_header(c) || repeated.contains(&c.top) {
            "column_header"
        } else if band(c) {
            "row_header"
        } else {
            let label = match &labels {
                Labels::Through(limit) => c.right <= *limit,
                Labels::Columns(columns) => columns.contains(&c.left),
                Labels::None => false,
            };
            if label && (c.string() || (year_labels && c.left == left)) {
                "row_header"
            } else {
                "data"
            }
        }
    };
    let roles: Vec<_> = group.iter().map(|c| role(c)).collect();
    // Every cell looks for its headers, so roles are computed once and
    // only header cells are searched, not the whole table per cell.
    let mut header_rows: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
    let mut bands = vec![];
    let mut label_rows: HashMap<u32, Vec<usize>> = HashMap::new();
    let mut label_columns: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
    for (i, c) in group.iter().enumerate() {
        match roles[i] {
            "column_header" => header_rows.entry(c.top).or_default().push(i),
            "row_header" if band(c) => bands.push(i),
            "row_header" => {
                for y in c.top..=c.bottom {
                    label_rows.entry(y).or_default().push(i);
                }
                label_columns.entry(c.left).or_default().push(i);
            }
            _ => {}
        }
    }
    // A repeated header stands in for the header cells it repeats.
    let mut replaced: HashMap<usize, Vec<usize>> = HashMap::new();
    for (row, headers) in header_rows.iter().filter(|(row, _)| repeated.contains(row)) {
        for &i in headers {
            replaced.insert(
                i,
                header_rows
                    .range(..row)
                    .flat_map(|(_, earlier)| earlier)
                    .copied()
                    .filter(|&j| {
                        group[j].span() == group[i].span() && group[j].text() == group[i].text()
                    })
                    .collect(),
            );
        }
    }
    // Rows that a label does not reach across.
    let barriers: BTreeSet<_> = bands
        .iter()
        .map(|i| group[*i].top)
        .chain(repeated.iter().copied())
        .collect();
    let occupied: HashSet<_> = group
        .iter()
        .filter(|c| label_columns.contains_key(&c.left))
        .flat_map(|c| (c.top..=c.bottom).map(|y| (c.left, y)))
        .collect();
    // Indented labels belong to the nearest label above with a smaller indent.
    let mut parents: HashMap<usize, Vec<usize>> = HashMap::new();
    for column in label_columns.values() {
        let mut open: Vec<usize> = vec![];
        for (k, &i) in column.iter().enumerate() {
            if k > 0
                && barriers
                    .range(group[column[k - 1]].top..=group[i].top)
                    .next()
                    .is_some()
            {
                open.clear();
            }
            while open
                .last()
                .is_some_and(|p| group[*p].indent() >= group[i].indent())
            {
                open.pop();
            }
            if !open.is_empty() {
                parents.insert(i, open.clone());
            }
            if group[i].nonempty() {
                open.push(i);
            }
        }
    }
    let id_for = |c: &Cell| {
        format!(
            "s{}-{}",
            sheet_index + 1,
            c.source["address"].as_str().unwrap()
        )
    };
    let interpreted: Vec<_> = group
        .iter()
        .enumerate()
        .map(|(index, c)| {
            let current = roles[index];
            let mut links = vec![];
            let wide = !single && c.span() == (left, right);
            if current != "text" && !(wide && !in_header(c) && current != "column_header") {
                for headers in header_rows.range(..c.top).map(|(_, headers)| headers) {
                    let above: Vec<_> = headers
                        .iter()
                        .copied()
                        .filter(|&i| group[i].bottom < c.top)
                        .collect();
                    let covering: Vec<_> = above
                        .iter()
                        .copied()
                        .filter(|&i| group[i].left <= c.left && group[i].right >= c.right)
                        .collect();
                    if !covering.is_empty() || current == "column_header" {
                        links.extend(covering);
                        continue;
                    }
                    // A value merged across several headers, or wider than its header.
                    let within: Vec<_> = above
                        .iter()
                        .copied()
                        .filter(|&i| group[i].left >= c.left && group[i].right <= c.right)
                        .collect();
                    if !within.is_empty() {
                        links.extend(within);
                    } else if let Some(widest) = above
                        .iter()
                        .copied()
                        .filter(|&i| group[i].left <= c.right && group[i].right >= c.left)
                        .max_by_key(|&i| {
                            let h = group[i];
                            (h.right.min(c.right) - h.left.max(c.left), std::cmp::Reverse(h.left))
                        })
                    {
                        links.push(widest);
                    }
                }
                let shadowed: Vec<_> = links.iter().filter_map(|i| replaced.get(i)).flatten().collect();
                links.retain(|i| !shadowed.contains(&i));
            }
            if matches!(current, "data" | "row_header") && !band(c) {
                let mut beside: Vec<_> = label_rows.get(&c.top).cloned().unwrap_or_default();
                // A label left blank continues the label above it.
                for (column, labels) in &label_columns {
                    if occupied.contains(&(*column, c.top)) {
                        continue;
                    }
                    let above = labels.partition_point(|&i| group[i].top < c.top);
                    if let Some(&i) = above.checked_sub(1).and_then(|k| labels.get(k))
                        && barriers.range(group[i].top..=c.top).next().is_none()
                    {
                        beside.push(i);
                    }
                }
                beside.retain(|&i| i != index);
                let mut before: Vec<_> = beside
                    .iter()
                    .copied()
                    .filter(|&i| group[i].right < c.left)
                    .collect();
                before.sort_by_key(|&i| std::cmp::Reverse(group[i].right));
                // Labels in adjoining columns form one hierarchy; a label of
                // another block of columns is not this cell's label.
                let mut edge = c.left;
                let mut chosen = vec![];
                for (k, &i) in before.iter().enumerate() {
                    let adjoining = group[i].right.checked_add(1) == Some(edge);
                    if (k == 0 && current == "data") || adjoining {
                        chosen.push(i);
                        edge = group[i].left;
                    } else {
                        break;
                    }
                }
                if chosen.is_empty()
                    && current == "data"
                    && let Some(&i) = beside
                        .iter()
                        .filter(|&&i| group[i].left > c.right)
                        .min_by_key(|&&i| group[i].left)
                {
                    chosen.push(i);
                }
                for i in chosen.into_iter().chain([index]) {
                    if i != index {
                        links.push(i);
                    }
                    links.extend(parents.get(&i).into_iter().flatten());
                }
                let above = bands.partition_point(|&i| group[i].top < c.top);
                links.extend(above.checked_sub(1).map(|k| bands[k]));
            }
            links.retain(|&i| i != index);
            links.sort_unstable();
            links.dedup();
            let links: Vec<_> = links.into_iter().map(|i| id_for(group[i])).collect();
            json!({"id":id_for(c),"address":c.source["address"],"role":current,"headers":links,
                "text_state":if c.nonempty() || c.source["formula"].is_string() {"read"} else {"empty"}})
        })
        .collect();
    let range = format!(
        "{}{}:{}{}",
        column_name(left)?,
        top,
        column_name(right)?,
        bottom
    );
    Ok(Some((interpreted, range)))
}

pub(super) fn elements(extraction: &Value) -> Result<Vec<Value>> {
    let mut output = vec![];
    for (sheet_index, sheet) in array(&extraction["sheets"])?.iter().enumerate() {
        let mut spans = BTreeMap::new();
        for m in array(&sheet["merges"])? {
            let (a, b) = string(m)?.split_once(':').context("invalid merge")?;
            spans.insert(a, coordinate(b)?);
        }
        let mut cells = vec![];
        for c in array(&sheet["cells"])? {
            let (left, top) = coordinate(string(&c["address"])?)?;
            let (right, bottom) = spans
                .get(string(&c["address"])?)
                .copied()
                .unwrap_or((left, top));
            cells.push(Cell {
                source: c,
                left,
                top,
                right,
                bottom,
            });
        }
        cells.sort_by_key(|c| (c.top, c.left));
        let mut assigned = BTreeSet::new();
        let mut groups: Vec<(Group, Explicit)> = vec![];
        // Explicit table boundaries have precedence over layout heuristics.
        if let Some(tables) = sheet["tables"].as_array() {
            let mut rows: BTreeMap<u32, Vec<(u32, usize)>> = BTreeMap::new();
            if !tables.is_empty() {
                for (i, cell) in cells.iter().enumerate() {
                    rows.entry(cell.top).or_default().push((cell.left, i));
                }
            }
            let within = |row: &[(u32, usize)], first: u32, last: u32| -> Vec<usize> {
                let start = row.partition_point(|(column, _)| *column < first);
                let end = row.partition_point(|(column, _)| *column <= last);
                row[start..end].iter().map(|(_, i)| *i).collect()
            };
            let mut found = vec![];
            for t in tables {
                let bounds = range(string(&t["range"])?)?;
                let indices: Vec<_> = rows
                    .range(bounds.0.1..=bounds.1.1)
                    .flat_map(|(_, row)| within(row, bounds.0.0, bounds.1.0))
                    .collect();
                ensure!(
                    indices.iter().all(|i| assigned.insert(*i)),
                    "overlapping table definitions"
                );
                found.push((
                    indices,
                    bounds,
                    t["header_rows"].as_u64().unwrap_or(0) as u32,
                ));
            }
            for (mut indices, bounds, mut header_rows) in found {
                let mut header_top = bounds.0.1;
                // Group headers written directly above the table: bold cells
                // within its columns, at least one merged across several.
                if header_rows > 0
                    && let Some(row) = header_top.checked_sub(1).and_then(|top| rows.get(&top))
                {
                    let above: Vec<_> = within(row, bounds.0.0, bounds.1.0)
                        .into_iter()
                        .filter(|i| !assigned.contains(i))
                        .collect();
                    let cell = |i: &usize| &cells[*i];
                    if above.iter().all(|i| {
                        let c = cell(i);
                        c.bold() && c.nonempty() && c.bottom == c.top && c.right <= bounds.1.0
                    }) && above.iter().any(|i| cell(i).right > cell(i).left)
                    {
                        assigned.extend(above.iter().copied());
                        indices.extend(above);
                        header_top -= 1;
                        header_rows += 1;
                    }
                }
                if !indices.is_empty() {
                    groups.push((indices, Some((header_top, header_rows))));
                }
            }
        }
        let mut leftovers = side_annotations(&cells, &assigned);
        assigned.extend(leftovers.iter().copied());
        let prose = prose_bullets(&cells, &assigned);
        leftovers.extend(prose);
        assigned.extend(leftovers.iter().copied());
        let (mut remaining, exhausted) = components(&cells, &assigned);
        if exhausted {
            leftovers.extend(remaining.into_iter().flatten());
        } else {
            attach_captions(&cells, &mut remaining);
            attach_headers(&cells, &mut remaining);
            join_columns(&cells, &mut remaining);
            for group in remaining.into_iter().filter(|g| !g.is_empty()) {
                for table in refine(&cells, group, &mut leftovers)? {
                    groups.push((table, None));
                }
            }
        }
        groups.sort_by_key(|(g, _)| {
            g.iter()
                .map(|i| (cells[*i].top, cells[*i].left))
                .min()
                .unwrap()
        });
        // A short blank band can be a page or writing break inside one table.
        // Join only when numbered record keys continue exactly and the two
        // bands occupy the same horizontal extent. This also tolerates changed
        // logical column widths in the lower band.
        let mut at = 0;
        while at + 1 < groups.len() {
            let (a, b) = (&groups[at], &groups[at + 1]);
            let same_table = if a.1.is_none() && b.1.is_none() {
                let (al, _, ar, ab) = bounds(&cells, &a.0);
                let (bl, bt, br, _) = bounds(&cells, &b.0);
                let view = |ids: &[usize]| -> Vec<_> {
                    let mut v: Vec<_> = ids.iter().map(|&i| &cells[i]).collect();
                    v.sort_by_key(|c| (c.top, c.left));
                    v
                };
                let later = view(&b.0);
                let later_layout = layout(&later, None)?;
                let titled_later = later_layout.caption
                    && later
                        .iter()
                        .any(|c| c.top == later_layout.top && c.nonempty());
                let a_headers = leaf_signature(&cells, &a.0, None)?;
                let b_headers = leaf_signature(&cells, &b.0, None)?;
                let different_headers = !a_headers.is_empty()
                    && !b_headers.is_empty()
                    && (a_headers.len() != b_headers.len()
                        || !a_headers
                            .iter()
                            .zip(&b_headers)
                            .all(|(first, second)| first.1 == second.1));
                match (record_series(&view(&a.0), al), record_series(&later, bl)) {
                    (Some((ap, aw, _, last)), Some((bp, bw, first, _))) => {
                        !titled_later
                            && !different_headers
                            && al == bl
                            && ar == br
                            && bt > ab
                            && bt - ab <= 5
                            && ap == bp
                            && aw == bw
                            && last.checked_add(1) == Some(first)
                    }
                    _ => false,
                }
            } else {
                false
            };
            if same_table {
                let next = groups.remove(at + 1).0;
                groups[at].0.extend(next);
                groups[at].0.sort_by_key(|&i| (cells[i].top, cells[i].left));
            } else {
                at += 1;
            }
        }
        attach_stepped_labels(&cells, &mut groups)?;
        type PrecedingTable = (
            (u32, u32, u32, u32),
            Vec<((u32, u32), String)>,
            String,
            bool,
        );
        let mut preceding: Option<PrecedingTable> = None;
        for (indices, explicit) in groups {
            let Some((interpreted, range)) = interpret(&cells, &indices, explicit, sheet_index)?
            else {
                leftovers.extend(indices);
                preceding = None;
                continue;
            };
            let rectangle = bounds(&cells, &indices);
            let signature = if explicit.is_none() {
                leaf_signature(&cells, &indices, explicit)?
            } else {
                vec![]
            };
            let id = format!("sheet-{}-table-{}", sheet_index + 1, output.len() + 1);
            let mut reason = format!(
                "{}; range {range}. Header links are layout hypotheses requiring review.",
                if explicit.is_some() {
                    "Explicit table boundary from the source"
                } else {
                    "Adjacent cell regions with repeated row fields; headers set apart by bold text or fill, and aligned row labels"
                }
            );
            if explicit.is_none()
                && signature.len() >= 2
                && let Some((above, headings, previous_id, true)) = &preceding
                && (above.0, above.2) == (rectangle.0, rectangle.2)
                && above.3.checked_add(2) == Some(rectangle.1)
                && *headings == signature
            {
                reason.push_str(&format!(" Possible continuation of {previous_id}: matching leaf headers after one blank row; confirm whether these are one table or two."));
            }
            preceding = Some((rectangle, signature, id.clone(), explicit.is_none()));
            output.push(
                json!({"id":id,"kind":"table","sheet":sheet["name"],"evidence":[],
                "reading":{"method":"parser","actor":"arp4","reason":reason},
                "cells":interpreted}),
            );
        }
        if !leftovers.is_empty() {
            leftovers.sort_unstable();
            let interpreted:Vec<_>=leftovers.iter().map(|i| {
                let c=&cells[*i];
                json!({"id":format!("s{}-{}",sheet_index+1,c.source["address"].as_str().unwrap()),"address":c.source["address"],
                    "role":"unassigned","headers":[],"text_state":if c.nonempty() || c.source["formula"].is_string() {"read"} else {"empty"}})
            }).collect();
            output.push(json!({"id":format!("sheet-{}",sheet_index+1),"kind":"text","sheet":sheet["name"],"cells":interpreted,"evidence":[],
                "reading":{"method":"parser","actor":"arp4","reason":if exhausted {"Layout comparison budget reached; roles remain unassigned."} else {"Captured cells outside confident table candidates; roles remain unassigned."}}}));
        }
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "manual explicit table inference performance measurement"]
    fn measure_explicit_tables() {
        for count in [100, 400, 1_600] {
            let cells: Vec<_> = (1..=count * 5)
                .flat_map(|row| {
                    [
                        cell(&format!("A{row}"), "key"),
                        cell(&format!("B{row}"), "value"),
                    ]
                })
                .collect();
            let tables: Vec<_> = (0..count)
                .map(
                    |index| json!({"range":format!("A{}:B{}",index*5+1,index*5+5),"header_rows":1}),
                )
                .collect();
            let extraction =
                json!({"sheets":[{"name":"S","cells":cells,"tables":tables,"merges":[]}]});
            let mut times = vec![];
            let mut digest = String::new();
            for _ in 0..5 {
                let start = std::time::Instant::now();
                let output = elements(&extraction).unwrap();
                times.push(start.elapsed().as_secs_f64() * 1000.0);
                assert_eq!(output.len(), count as usize);
                digest = hash(&encoded(&json!(output)));
            }
            times.sort_by(f64::total_cmp);
            eprintln!(
                "explicit_tables count={count} median_ms={:.3} sha256={digest}",
                times[2]
            );
        }
    }
    fn cell(address: &str, value: &str) -> Value {
        json!({"address":address,"value":value,"type":"string","formula":null,"style":{"bold":false,"fill":0,"border":0}})
    }
    #[test]
    fn explicit_table_takes_precedence_and_honors_no_header() {
        let extraction = json!({"sheets":[{"name":"S","merges":[],"tables":[{"range":"B2:C3","header_rows":1}],
            "cells":[cell("A2","outside"),cell("B2","Key"),cell("C2","Value"),cell("B3","x"),cell("C3","y")]}]});
        let result = elements(&extraction).unwrap();
        let table = result.iter().find(|e| e["kind"] == "table").unwrap();
        assert_eq!(table["cells"].as_array().unwrap().len(), 4);
        let data = table["cells"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["address"] == "C3")
            .unwrap();
        assert!(
            data["headers"]
                .as_array()
                .unwrap()
                .contains(&json!("s1-C2"))
        );
        let mut no_header = extraction.clone();
        no_header["sheets"][0]["tables"][0]["header_rows"] = json!(0);
        let result = elements(&no_header).unwrap();
        assert!(
            result
                .iter()
                .flat_map(|e| e["cells"].as_array().unwrap())
                .all(|c| c["role"] != "column_header")
        );
    }
    #[test]
    fn explicit_tables_select_sparse_columns_and_reject_overlaps() {
        let mut extraction = json!({"sheets":[{"name":"S","merges":[],
            "tables":[{"range":"D2:E5","header_rows":0},{"range":"A2:B5","header_rows":0}],
            "cells":[cell("E5","e"),cell("A2","a"),cell("D2","d"),cell("B5","b")]}]});
        let result = elements(&extraction).unwrap();
        let addresses = |table: &Value| {
            table["cells"]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| c["address"].as_str().unwrap().to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(addresses(&result[0]), ["A2", "B5"]);
        assert_eq!(addresses(&result[1]), ["D2", "E5"]);
        extraction["sheets"][0]["tables"][1]["range"] = json!("A2:E5");
        assert!(
            elements(&extraction)
                .unwrap_err()
                .to_string()
                .contains("overlapping")
        );
    }
    #[test]
    fn cells_link_column_headers_and_merged_row_headers_in_table_order() {
        let cells: Vec<_> = [
            ("A1", "Group"),
            ("B1", "Item"),
            ("C1", "Value"),
            ("A2", "G1"),
            ("B2", "x"),
            ("C2", "1"),
            ("B3", "y"),
            ("C3", "2"),
            ("A4", "G2"),
            ("B4", "z"),
            ("C4", "3"),
        ]
        .into_iter()
        .map(|(address, value)| cell(address, value))
        .collect();
        let extraction = json!({"sheets":[{"name":"S","merges":["A2:A3"],
            "tables":[{"range":"A1:C4","header_rows":1}],"cells":cells}]});
        let result = elements(&extraction).unwrap();
        let headers = |address: &str| {
            result[0]["cells"]
                .as_array()
                .unwrap()
                .iter()
                .find(|c| c["address"] == address)
                .unwrap()["headers"]
                .clone()
        };
        assert_eq!(headers("C3"), json!(["s1-C1", "s1-A2"]));
        assert_eq!(headers("B3"), json!(["s1-B1", "s1-A2"]));
        assert_eq!(headers("C4"), json!(["s1-C1", "s1-A4"]));
        assert_eq!(headers("A4"), json!(["s1-A1"]));
        assert_eq!(headers("B1"), json!([]));
    }
    #[test]
    fn diagonal_tables_stay_separate_and_notes_remain_unassigned() {
        let mut cells = vec![];
        for address in ["A1", "B1", "A2", "B2", "C3", "D3", "C4", "D4", "J10"] {
            cells.push(cell(address, "text"));
        }
        let result = elements(&json!({"sheets":[{"name":"S","merges":[],"cells":cells}]})).unwrap();
        assert_eq!(result.iter().filter(|e| e["kind"] == "table").count(), 2);
        let note = result.iter().find(|e| e["kind"] == "text").unwrap();
        assert_eq!(note["cells"][0]["address"], "J10");
        assert_eq!(note["cells"][0]["role"], "unassigned");
    }
}
