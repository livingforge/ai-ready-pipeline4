//! Deterministic layout hypotheses; never used to change source cells.
//!
//! The inference reads positions, merges and formatting (bold, fill, border).
//! It compares cell texts for equality and recognizes note/list markers when
//! layout also supports prose, but does not assign table roles from words alone.
use super::*;
use crate::excel::coordinate;
use std::collections::{HashMap, HashSet};

mod outline;

#[derive(Clone)]
struct Cell<'a> {
    source: &'a Value,
    /// The cell's style, from its sheet's table.
    style: &'a Value,
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
        self.style["bold"] == true
    }
    fn fill(&self) -> u64 {
        self.style["fill"].as_u64().unwrap_or(0)
    }
    fn bordered(&self) -> bool {
        self.style["border"].as_u64().unwrap_or(0) > 0
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
    /// Leading spaces and the indent level of the cell's format, which
    /// authors use to indent a child under its parent.
    fn indent(&self) -> usize {
        self.text()
            .chars()
            .take_while(|c| matches!(c, ' ' | '\u{3000}'))
            .count()
            + self.style["indent"].as_u64().unwrap_or(0) as usize
    }
}

/// Cells of a candidate table, as indices into the sheet's cells.
type Group = Vec<usize>;
/// A rectangle of the sheet: left, top, right, bottom.
type Extent = (u32, u32, u32, u32);
/// Header rows of an Excel table definition: (first row, row count).
type Explicit = Option<(u32, u32)>;

/// A table candidate and the candidate it is nested in, by key.
struct Candidate {
    key: usize,
    group: Group,
    explicit: Explicit,
    parent: Option<usize>,
}

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
    in_rows(group, top, top).to_vec()
}

/// Cells of a group sorted by (top, left) that start on the rows `first` to
/// `last`, found by bisection so that row-by-row reading stays linear.
fn in_rows<'g, 'a, 'b>(group: &'g [&'b Cell<'a>], first: u32, last: u32) -> &'g [&'b Cell<'a>] {
    debug_assert!(group.is_sorted_by_key(|c| (c.top, c.left)));
    let start = group.partition_point(|c| c.top < first);
    let end = start + group[start..].partition_point(|c| c.top <= last);
    &group[start..end]
}

/// The most rows a cell of the group is merged down over, after its first.
fn height(group: &[&Cell<'_>]) -> u32 {
    group.iter().map(|c| c.bottom - c.top).max().unwrap_or(0)
}

/// Cells of a group sorted by (top, left) that cover row `y`, in the group's
/// order. Only the rows within `height` (see [`height`]) above `y` can hold one.
fn covering<'g, 'a, 'b>(
    group: &'g [&'b Cell<'a>],
    height: u32,
    y: u32,
) -> impl Iterator<Item = &'b Cell<'a>> + 'g {
    in_rows(group, y.saturating_sub(height), y)
        .iter()
        .copied()
        .filter(move |c| c.bottom >= y)
}

fn valued<'a, 'b>(cells: Vec<&'b Cell<'a>>) -> Vec<&'b Cell<'a>> {
    cells.into_iter().filter(|c| c.nonempty()).collect()
}

/// The empty boxes (left, top, right, bottom, and whether it is an empty
/// merged range) that are fields of a ruled table. An empty merged range is a
/// field drawn on purpose; an empty ruled cell is one only when its row reaches
/// a value of `free` through boxes, so a ruled blank line between two tables
/// separates them.
fn fields(
    cells: &[Cell<'_>],
    free: &[usize],
    boxes: &[(Extent, bool)],
) -> Vec<(u32, u32, u32, u32)> {
    let mut at: HashMap<(u32, u32), usize> = HashMap::new();
    for (k, &((left, top, right, bottom), _)) in boxes.iter().enumerate() {
        for y in top..=bottom {
            for x in left..=right {
                at.insert((x, y), k);
            }
        }
    }
    let beside = |(left, top, right, bottom): (u32, u32, u32, u32)| {
        (top..=bottom).flat_map(move |y| [(left.wrapping_sub(1), y), (right.saturating_add(1), y)])
    };
    let mut reached = vec![false; boxes.len()];
    let mut pending = vec![];
    for c in free.iter().map(|&i| &cells[i]) {
        for position in beside((c.left, c.top, c.right, c.bottom)) {
            if let Some(&k) = at.get(&position)
                && !reached[k]
            {
                reached[k] = true;
                pending.push(k);
            }
        }
    }
    while let Some(k) = pending.pop() {
        for position in beside(boxes[k].0) {
            if let Some(&next) = at.get(&position)
                && !reached[next]
            {
                reached[next] = true;
                pending.push(next);
            }
        }
    }
    boxes
        .iter()
        .zip(reached)
        .filter(|((_, merged), reached)| *merged || *reached)
        .map(|((extent, _), _)| *extent)
        .collect()
}

/// Groups of cells that share an edge, directly or through the empty fields of
/// a ruled table (`boxes`: left, top, right, bottom; see [`fields`]). A box
/// joins what lies above or below it only across the same columns, and what
/// lies beside it only across the same rows, as the fields of one grid line up.
/// Boxes belong to no group. Corner-only contacts do not join two tables.
/// Returns single cells when the sheet exceeds the comparison budget.
fn components(
    cells: &[Cell<'_>],
    boxes: &[(u32, u32, u32, u32)],
    assigned: &BTreeSet<usize>,
) -> (Vec<Group>, bool) {
    let free: Vec<_> = (0..cells.len()).filter(|i| !assigned.contains(i)).collect();
    let mut budget = 2_000_000u64;
    // Cells, then boxes after them, as nodes of one union-find.
    let extent = |i: usize| -> (u32, u32, u32, u32) {
        match cells.get(i) {
            Some(c) => (c.left, c.top, c.right, c.bottom),
            None => boxes[i - cells.len()],
        }
    };
    let nodes: Vec<usize> = free
        .iter()
        .copied()
        .chain(cells.len()..cells.len() + boxes.len())
        .collect();
    let mut parent: Vec<_> = (0..cells.len() + boxes.len()).collect();
    let mut occupied: HashMap<(u32, u32), usize> = HashMap::new();
    let mut exhausted = false;
    for &i in &nodes {
        let (left, top, right, bottom) = extent(i);
        let area = u64::from(right - left + 1) * u64::from(bottom - top + 1);
        if area > budget {
            exhausted = true;
            break;
        }
        budget -= area;
        for y in top..=bottom {
            for x in left..=right {
                if let Some(j) = occupied.insert((x, y), i) {
                    let (r, s) = (root(&mut parent, j), root(&mut parent, i));
                    parent[s] = r;
                }
            }
        }
    }
    if !exhausted {
        for &i in &nodes {
            let (left, top, right, bottom) = extent(i);
            let beside = (top..=bottom).map(|y| ((right.saturating_add(1), y), true));
            let below = (left..=right).map(|x| ((x, bottom.saturating_add(1)), false));
            for (position, across) in beside.chain(below) {
                let Some(&j) = occupied.get(&position) else {
                    continue;
                };
                let other = extent(j);
                let aligned = if across {
                    (other.1, other.3) == (top, bottom)
                } else {
                    (other.0, other.2) == (left, right)
                };
                if (i < cells.len() && j < cells.len()) || aligned {
                    let (r, s) = (root(&mut parent, j), root(&mut parent, i));
                    parent[s] = r;
                }
            }
        }
        // A cell merged across columns joins the empty fields of one row that
        // line its whole top or bottom edge, as a name merged to the end of
        // a band under a band of fields.
        for &i in &free {
            let (left, top, right, bottom) = extent(i);
            if right == left {
                continue;
            }
            for y in [top.checked_sub(1), bottom.checked_add(1)]
                .into_iter()
                .flatten()
            {
                let along: Option<Vec<usize>> = (left..=right)
                    .map(|x| {
                        occupied.get(&(x, y)).copied().filter(|&j| {
                            let (l, t, r, b) = extent(j);
                            j >= cells.len() && t == y && b == y && l >= left && r <= right
                        })
                    })
                    .collect();
                for j in along.into_iter().flatten() {
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
/// So is sparse plain text in the margin left of a bordered grid.
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
    // Marks written in the margin left of a bordered grid, such as revision
    // marks: plain text in the column just outside its first column, beside a
    // few of its records but not its first row.
    let plain = |c: &Cell<'_>| c.string() && !c.bordered() && c.fill() == 0 && !c.bold();
    let margin = |c: &Cell<'_>| {
        plain(c)
            && bordered_runs
                .get(&(c.right + 1, c.top))
                .is_some_and(|&(top, bottom)| {
                    let local: Vec<_> = cells
                        .iter()
                        .filter(|o| {
                            o.nonempty() && o.right == c.right && o.top >= top && o.top <= bottom
                        })
                        .collect();
                    c.top > top
                        && local.iter().all(|o| plain(o) && o.top > top)
                        && local.len() * 2 <= (bottom - top) as usize
                })
    };
    cells
        .iter()
        .enumerate()
        .filter(|(i, c)| !assigned.contains(i) && c.nonempty() && margin(c))
        .map(|(i, _)| i)
        .chain(
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
                                        let last =
                                            column.partition_point(|cell| cell.top <= *bottom);
                                        let local = &column[first..last];
                                        !local.is_empty()
                                            && local.len() <= 2
                                            && local.iter().all(|cell| annotation(cell))
                                    })
                                })
                        })
                })
                .map(|(i, _)| i),
        )
        .collect::<BTreeSet<_>>()
        .into_iter()
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

/// The extent of each group, `None` for an empty one.
fn extents(cells: &[Cell<'_>], groups: &[Group]) -> Vec<Option<Extent>> {
    groups
        .iter()
        .map(|group| (!group.is_empty()).then(|| bounds(cells, group)))
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
    let mut extents = extents(cells, remaining);
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
                    && extents[*i].is_some_and(|(left, top, right, _)| {
                        (left, top, right) == (caption.left, caption.bottom + 2, caption.right)
                    })
            })
            .map(|(i, _)| i)
            .collect();
        if let [target] = candidates.as_slice() {
            remaining[*target].push(id);
            remaining[slot].clear();
            extents[*target] = Some(bounds(cells, &remaining[*target]));
            extents[slot] = None;
        }
    }
}

/// Attaches a row of bold headers to the records that begin one blank row
/// below it in the same columns.
fn attach_headers(cells: &[Cell<'_>], remaining: &mut [Group]) {
    let mut extents = extents(cells, remaining);
    for slot in 0..remaining.len() {
        let head = &remaining[slot];
        let (left, top, right, bottom) = match (head.as_slice(), extents[slot]) {
            ([] | [_], _) | (_, None) => continue,
            (_, Some(extent)) => extent,
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
                let Some((l, t, r, _)) = extents[other] else {
                    return false;
                };
                if (l, t, r) != (left, bottom + 2, right) {
                    return false;
                }
                let first: Vec<_> = group.iter().filter(|i| cells[**i].top == t).collect();
                first
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
            extents[*target] = Some(bounds(cells, &remaining[*target]));
            extents[slot] = None;
        }
    }
}

/// Joins columns set apart by one blank column to the table on their left
/// when they cover the same rows and have no row labels of their own.
fn join_columns(cells: &[Cell<'_>], remaining: &mut [Group]) {
    // A group's extent, whether it is one column, and whether it has row
    // labels of its own, kept for the groups not yet joined.
    let shape = |group: &Group| {
        let (left, top, right, bottom) = bounds(cells, group);
        let single = group.iter().all(|i| cells[*i].left == left);
        let labelled = group
            .iter()
            .map(|i| &cells[*i])
            .any(|c| c.left == left && c.top > top && c.string() && c.nonempty());
        ((left, top, right, bottom), single, labelled)
    };
    let mut shapes: Vec<_> = remaining
        .iter()
        .map(|group| (!group.is_empty()).then(|| shape(group)))
        .collect();
    loop {
        // Groups by their first column and their first and last rows, in order.
        let mut by_edge: HashMap<(u32, u32, u32), Vec<usize>> = HashMap::new();
        for (b, found) in shapes.iter().enumerate() {
            if let Some(((left, top, _, bottom), ..)) = found {
                by_edge.entry((*left, *top, *bottom)).or_default().push(b);
            }
        }
        let joined = shapes.iter().enumerate().find_map(|(a, found)| {
            let ((_, top, right, bottom), single, _) = (*found)?;
            if single {
                return None;
            }
            let b = by_edge
                .get(&(right.checked_add(2)?, top, bottom))?
                .iter()
                .copied()
                .find(|&b| {
                    b != a && shapes[b].is_some_and(|(_, single, labelled)| single || !labelled)
                })?;
            Some((a, b))
        });
        let Some((a, b)) = joined else { break };
        let moved = std::mem::take(&mut remaining[b]);
        remaining[a].extend(moved);
        shapes[a] = Some(shape(&remaining[a]));
        shapes[b] = None;
    }
}

/// A stepped row-label column may not touch the rest of its row. Join short,
/// bordered fragments under the same merged header when their rows already
/// contain records to the right. A shared outer border alone does not qualify.
fn attach_stepped_labels(cells: &[Cell<'_>], groups: &mut Vec<Candidate>) -> Result<()> {
    let mut candidate_headers = vec![];
    for Candidate {
        group: table,
        explicit,
        ..
    } in groups.iter()
    {
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
    for (source, candidate) in groups.iter().enumerate() {
        let (fragment, explicit) = (&candidate.group, candidate.explicit);
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
            .filter_map(|(target, (Candidate { group: table, .. }, header))| {
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
        let fragment = std::mem::take(&mut groups[source].group);
        groups[target].group.extend(fragment);
    }
    groups.retain(|candidate| !candidate.group.is_empty());
    for candidate in groups {
        candidate
            .group
            .sort_by_key(|&i| (cells[i].top, cells[i].left));
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
    // Headings may be filled differently, such as a group of columns or the
    // weekend in its own color; each is still a fill.
    let shades: BTreeSet<u64> = first.iter().map(|c| c.fill()).collect();
    let shaded = !shades.contains(&0);
    let shade = |c: &&Cell<'_>| shades.contains(&c.fill());
    // Fills that the header rows share and no record uses.
    let mut fill_end = None;
    if shaded && (first.len() >= 2 || single) {
        let mut end = last(&first_all);
        loop {
            let next = valued(row(group, end + 1));
            if next.is_empty() || !next.iter().all(shade) {
                break;
            }
            end = last(&next);
        }
        let next_row = group
            .iter()
            .filter(|c| c.top > end && c.nonempty())
            .map(|c| c.top)
            .min();
        // Records may keep the header's fill on their labels at the left, as a
        // list written across the page shades its header row and label columns.
        if next_row.is_some_and(|at| {
            let line: Vec<_> = group
                .iter()
                .filter(|c| c.top == at && c.nonempty())
                .collect();
            let plain = line.iter().filter(|c| !shade(c)).map(|c| c.left).min();
            plain.is_some_and(|x| line.iter().all(|c| !shade(c) || c.right < x))
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
        // Every row starts at the left edge.
        let complete = || {
            group
                .iter()
                .filter(|c| c.left == left)
                .map(|c| c.top)
                .collect::<BTreeSet<_>>()
                .len()
                == tops.len()
        };
        if labelled && tops.len() >= 2 && columns.len() >= 3 && complete() {
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
    // The header rows' labels with their places relative to the first row.
    let height = header.1 - header.0;
    let block = |start: u32| -> BTreeSet<(u32, u32, (u32, u32), &str)> {
        in_rows(group, start, start + height)
            .iter()
            .filter(|c| c.nonempty())
            .map(|c| (c.top - start, c.bottom - start, c.span(), c.text()))
            .collect()
    };
    let original = block(header.0);
    let mut repeated = BTreeSet::new();
    let mut k = 1;
    while k + 1 < tops.len() {
        // The whole header written again, such as after a page break, in any look.
        if original.len() >= 2 && block(tops[k]) == original {
            repeated.extend(tops[k]..=tops[k] + height);
            k = tops.partition_point(|t| *t <= tops[k] + height);
            continue;
        }
        let at = k;
        k += 1;
        let cells = valued(row(group, tops[at]));
        if cells.len() < 2 || !cells.iter().all(|c| c.bold() && c.string()) {
            continue;
        }
        let found = labels(cells);
        if found == leaf || (found.len() == leaf.len() && found.values().eq(leaf.values())) {
            repeated.insert(tops[at]);
        } else if split
            && (found.keys().eq(leaf.keys()) || !found.keys().all(|span| leaf.contains_key(span)))
        {
            // A lone cell directly above the new header is that table's title.
            let titled = at >= 2 && valued(row(group, tops[at - 1])).len() == 1;
            return (repeated, Some(tops[if titled { at - 1 } else { at }]));
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

/// Splits a group of touching cells into the tables it holds, each with the
/// index in the result of the table it is nested in.
fn refine(
    cells: &[Cell<'_>],
    boxes: &[(u32, u32, u32, u32)],
    mut group: Group,
    leftovers: &mut Vec<usize>,
) -> Result<Vec<(Group, Option<usize>)>> {
    strip(cells, &mut group, leftovers);
    if group.is_empty() {
        return Ok(vec![]);
    }
    group.sort_by_key(|i| (cells[*i].top, cells[*i].left));
    // Two grids can touch along a full vertical edge. A different final row,
    // together with a header and record labels on both sides, gives a much
    // stronger boundary than adjacency gives evidence of one wide grid.
    if let Some((left, right)) = split_touching_tables(cells, boxes, &group)? {
        let mut tables = refine(cells, boxes, left, leftovers)?;
        append(&mut tables, refine(cells, boxes, right, leftovers)?, None);
        return Ok(tables);
    }
    let view: Vec<_> = group.iter().map(|i| &cells[*i]).collect();
    let header = layout(&view, None)?.header;
    // Ruled rows of labels and values, such as a table's name and storage,
    // written directly over the header of its column list are a form of their own.
    if header.is_none()
        && let Some(&first) = view.first()
        && frame_row(&row(&view, first.top))
    {
        let mut below = first.bottom + 1;
        loop {
            let line = row(&view, below);
            if line.is_empty() || !frame_row(&line) {
                break;
            }
            below = line.iter().map(|c| c.bottom).max().unwrap() + 1;
        }
        let (upper, lower): (Group, Group) = group.iter().partition(|i| cells[**i].top < below);
        let rest: Vec<_> = lower.iter().map(|i| &cells[*i]).collect();
        if !rest.is_empty()
            && rest.iter().all(|c| c.top >= below)
            && layout(&rest, None)?
                .header
                .is_some_and(|(top, _)| top == below)
        {
            let mut tables = vec![(upper, None)];
            append(&mut tables, refine(cells, boxes, lower, leftovers)?, None);
            return Ok(tables);
        }
    }
    let children = nested_tables(&view, header);
    if !children.is_empty() {
        let inside = |c: &Cell<'_>| {
            children.iter().any(|&(left, top, right, bottom)| {
                c.left >= left && c.right <= right && c.top >= top && c.top <= bottom
            })
        };
        let (parent, rest): (Group, Group) = group.into_iter().partition(|i| !inside(&cells[*i]));
        let mut tables = vec![(parent, None)];
        for &(left, top, right, bottom) in &children {
            let child = rest
                .iter()
                .copied()
                .filter(|i| {
                    let c = &cells[*i];
                    c.left >= left && c.right <= right && c.top >= top && c.top <= bottom
                })
                .collect();
            append(
                &mut tables,
                refine(cells, boxes, child, leftovers)?,
                Some(0),
            );
        }
        return Ok(tables);
    }
    let at = match header {
        Some(header) => later_headers(&view, header, true).1,
        None => None,
    };
    let Some(at) = at else {
        return Ok(vec![(group, None)]);
    };
    let (upper, lower) = group.into_iter().partition(|i| cells[*i].top < at);
    let mut tables = vec![(upper, None)];
    append(&mut tables, refine(cells, boxes, lower, leftovers)?, None);
    Ok(tables)
}

/// Appends tables found in a part of a group; their own top-level tables are
/// nested in `parent`, an index of `tables`.
fn append(
    tables: &mut Vec<(Group, Option<usize>)>,
    found: Vec<(Group, Option<usize>)>,
    parent: Option<usize>,
) {
    let offset = tables.len();
    for (group, nested) in found {
        tables.push((group, nested.map(|p| p + offset).or(parent)));
    }
}

/// Header-like text: a bold or filled label.
fn labelled(c: &Cell<'_>) -> bool {
    c.string() && c.nonempty() && (c.bold() || c.fill() != 0)
}

/// A number that numbers the records: a number, digits, or circled numbers
/// such as ① that refer to the same mark on a screen image.
fn number_mark(c: &Cell<'_>) -> bool {
    let text = c.text().trim();
    c.source["value"].is_number()
        || (!text.is_empty()
            && text.chars().all(|ch| {
                ch.is_ascii_digit()
                    || matches!(ch, '\u{2460}'..='\u{2473}' | '\u{24EA}' | '\u{24F5}'..='\u{24FE}'
                        | '\u{2776}'..='\u{2793}' | '\u{3251}'..='\u{325F}' | '\u{32B1}'..='\u{32BF}')
            }))
}

/// An identifier such as `J01` or `SPW0010`, or a date written as text:
/// ASCII letters, digits and separators with at least one digit.
fn identifier(c: &Cell<'_>) -> bool {
    let text = c.text().trim();
    text.chars().any(|ch| ch.is_ascii_digit())
        && text
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '/' | ':'))
}

/// A ruled row of labels and values, such as the frame atop a form sheet:
/// filled labels alternate with plain values, two pairs or more. A label may
/// stand before an empty field, a value not yet written.
fn frame_row(line: &[&Cell<'_>]) -> bool {
    let line = valued(line.to_vec());
    let label = |c: &Cell<'_>| c.fill() != 0 && c.string();
    line.len() >= 4
        && line.iter().all(|c| c.bordered())
        && label(line[0])
        && line.windows(2).all(|pair| {
            if label(pair[1]) {
                !label(pair[0]) || pair[0].right + 1 < pair[1].left
            } else {
                label(pair[0])
            }
        })
        && line.iter().filter(|c| !label(c)).count() >= 2
}

/// Child tables written inside the records of a table, as (left, top, right,
/// bottom). A child starts at a row of two or more adjoining labels of one look that spans
/// only part of the table's columns, beside a parent cell merged down across
/// that row; it is a table of its own only when records follow its labels.
/// It ends before a blank row, a cell crossing its columns, a single cell across
/// all its columns (the parent's text), the next child's labels, or, when the
/// parent's keys are on its left, the next parent record.
fn nested_tables(group: &[&Cell<'_>], header: Option<(u32, u32)>) -> Vec<(u32, u32, u32, u32)> {
    let left = group.iter().map(|c| c.left).min().unwrap();
    let right = group.iter().map(|c| c.right).max().unwrap();
    let top = group.iter().map(|c| c.top).min().unwrap();
    let bottom = group.iter().map(|c| c.bottom).max().unwrap();
    let body_top = header.map_or(top + 1, |h| h.1 + 1);
    let tall = height(group);
    let mut found: Vec<(u32, u32, u32, u32)> = vec![];
    let taken = |found: &[(u32, u32, u32, u32)], c: &Cell<'_>| {
        found
            .iter()
            .any(|&(l, t, r, b)| c.left <= r && c.right >= l && c.top <= b && c.bottom >= t)
    };
    for r in body_top..=bottom {
        let line = row(group, r);
        let mut k = 0;
        while k < line.len() {
            // A maximal run of adjoining labels of one look starting on this row.
            let look = |c: &Cell<'_>| (c.bold(), c.fill());
            let mut end = k;
            while end < line.len()
                && labelled(line[end])
                && (end == k
                    || (line[end - 1].right.checked_add(1) == Some(line[end].left)
                        && look(line[end]) == look(line[k])))
            {
                end += 1;
            }
            if end == k {
                k += 1;
                continue;
            }
            let run = &line[k..end];
            k = end;
            let (x0, x1) = (run[0].left, run[run.len() - 1].right);
            if run.len() < 2 || (x0 == left && x1 == right) || run.iter().any(|c| taken(&found, c))
            {
                continue;
            }
            let outside = |c: &Cell<'_>| c.right < x0 || c.left > x1;
            let crossing =
                |c: &Cell<'_>| (c.left < x0 && c.right >= x0) || (c.left <= x1 && c.right > x1);
            // A parent cell merged down across the labels holds the child.
            if !covering(group, tall, r).any(|c| outside(c) && c.bottom > r) {
                continue;
            }
            let keys_left = x0 > left;
            let mut last = r;
            let mut records = false;
            for y in r + 1..=bottom {
                let starting = row(group, y);
                let within: Vec<_> = starting
                    .iter()
                    .copied()
                    .filter(|c| c.left >= x0 && c.right <= x1)
                    .collect();
                let covered =
                    covering(group, tall, y).any(|c| c.top < y && c.left >= x0 && c.right <= x1);
                let next_labels = records
                    && within.len() >= 2
                    && within.iter().all(|c| labelled(c))
                    && within[0].left == x0
                    && within[within.len() - 1].right == x1;
                let parent_text = matches!(within.as_slice(), [c] if c.span() == (x0, x1));
                let next_record =
                    keys_left && starting.iter().any(|c| c.right < x0 && c.nonempty());
                if starting.iter().any(|c| crossing(c))
                    || (within.is_empty() && !covered)
                    || next_labels
                    || parent_text
                    || next_record
                {
                    break;
                }
                records |= within.iter().any(|c| c.nonempty() && !labelled(c));
                last = y;
            }
            if records {
                found.push((x0, r, x1, last));
            }
        }
    }
    found
}

fn split_touching_tables(
    cells: &[Cell<'_>],
    boxes: &[(u32, u32, u32, u32)],
    group: &[usize],
) -> Result<Option<(Group, Group)>> {
    let (left, top, right, last) = bounds(cells, group);
    if right - left < 3 {
        return Ok(None);
    }
    let seams: BTreeSet<_> = group
        .iter()
        .map(|&i| cells[i].right)
        .filter(|seam| *seam > left && *seam + 1 < right)
        .collect();
    for seam in seams.into_iter().rev() {
        // An empty box of the grid across the seam joins the two sides.
        if group
            .iter()
            .any(|&i| cells[i].left <= seam && cells[i].right > seam)
            || boxes.iter().any(|&(l, t, r, b)| {
                l <= seam && r > seam && l >= left && r <= right && t >= top && b <= last
            })
        {
            continue;
        }
        let (a, b): (Group, Group) = group.iter().copied().partition(|&i| cells[i].right <= seam);
        let plausible = |side: &[usize]| -> Result<Option<u32>> {
            let (l, t, r, mut bottom) = bounds(cells, side);
            // Empty boxes of the grid below a side carry it down.
            while let Some(&(_, _, _, b)) = boxes
                .iter()
                .find(|&&(bl, bt, br, _)| bt == bottom + 1 && bl >= l && br <= r)
            {
                bottom = b;
            }
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
    /// Text cells within one of these column spans (left, right).
    Spans(Vec<(u32, u32)>),
    /// Filled text cells: the labels of a form, wherever they stand.
    Marked,
    None,
}

/// The columns that name the records under a header whose first row has
/// cells merged down through every header row, and the last column of the
/// stub (`heads`: the first header row's cells in column order, without the
/// last column). Design documents merge down every heading without
/// subheadings, attributes included, so a merged-down heading alone does not
/// make its column a row label. From the left, a column names the records
/// while it holds a value in every record and its values seldom repeat within
/// the label on its left, until a column of names that are not codes, such as
/// a function's name after its ID, tells every record apart with the labels
/// on its left; a later column that happens not to repeat, such as a
/// description, is an attribute. A heading with subheadings names them only over stepped names,
/// one value in each record. Under a one-row header, the walk runs only where
/// names step across columns or indents, as in a folder tree, and besides the
/// first column only such stepped names are labels. Adjoining headings, one
/// for each level of a tree, are one label area when each record starts one
/// name there; under a header of several rows, each of them is merged down
/// through all the rows. Numbers and digit codes that do not name the records, such as
/// level numbers, are passed over; the first other column that does not name
/// them ends the stub. `None` when no heading is merged down and no names
/// step, or no column names the records.
fn stub_labels(
    group: &[&Cell<'_>],
    heads: &[&Cell<'_>],
    merged_down: &dyn Fn(&Cell<'_>) -> bool,
    body: &dyn Fn(&Cell<'_>) -> bool,
) -> Option<(u32, Vec<(u32, u32)>)> {
    let mut starts: BTreeMap<u32, Vec<&&Cell<'_>>> = BTreeMap::new();
    for c in group.iter().filter(|c| body(c) && c.nonempty()) {
        starts.entry(c.top).or_default().push(c);
    }
    let records: BTreeSet<u32> = starts.keys().copied().collect();
    let tall = height(group);
    let values = |left: u32, right: u32| -> Vec<&Cell<'_>> {
        group
            .iter()
            .copied()
            .filter(|c| body(c) && c.nonempty() && c.left >= left && c.right <= right)
            .collect()
    };
    let one_each = |values: &[&Cell<'_>]| {
        let tall = height(values);
        records
            .iter()
            .all(|&y| covering(values, tall, y).count() == 1)
    };
    // Names that start in different columns and run to the right edge, merged
    // or spilling over empty cells, or that differ in indent.
    let steps = |values: &[&Cell<'_>]| {
        let right = values.iter().map(|c| c.right).max();
        (values.iter().map(|c| c.left).collect::<HashSet<_>>().len() >= 2
            && values
                .iter()
                .all(|c| Some(c.right) == right || c.left == c.right))
            || values
                .iter()
                .map(|c| c.indent())
                .collect::<HashSet<_>>()
                .len()
                >= 2
    };
    // A tree across the columns of its levels: every record starts one name,
    // in different columns, and a name runs to the right edge, spills over
    // empty cells or is merged down over its descendants. Read record by
    // record, so that the columns of a wide table, tried pair by pair, are
    // told apart at their first record rather than by all of its cells.
    let tree = |left: u32, right: u32| {
        let mut lefts = HashSet::new();
        starts.values().all(|line| {
            let mut names = line.iter().filter(|c| c.left >= left && c.right <= right);
            let (Some(c), None) = (names.next(), names.next()) else {
                return false;
            };
            lefts.insert(c.left);
            c.string() && (c.right == right || c.left == c.right || c.bottom > c.top)
        }) && lefts.len() >= 2
    };
    let layered = heads.iter().any(|c| merged_down(c));
    // Label areas: a heading's columns, or adjoining headings over one tree.
    let mut areas: Vec<(u32, u32, bool)> = heads
        .iter()
        .map(|c| (c.left, c.right, merged_down(c)))
        .collect();
    // Under a header of several rows, each level's heading is merged down
    // through all of them.
    let mut tree_area = None;
    'search: for start in 0..heads.len() {
        for end in (start + 1..heads.len()).rev() {
            let adjoining =
                (start..end).all(|k| heads[k].right.checked_add(1) == Some(heads[k + 1].left));
            let level = !layered || heads[start..=end].iter().all(|c| merged_down(c));
            let (left, right) = (heads[start].left, heads[end].right);
            if adjoining && level && tree(left, right) {
                areas.splice(start..=end, [(left, right, false)]);
                tree_area = Some(start);
                break 'search;
            }
        }
    }
    if !layered
        && tree_area.is_none()
        && !heads.iter().any(|head| {
            let values = values(head.left, head.right);
            values.iter().all(|c| c.string()) && one_each(&values) && steps(&values)
        })
    {
        return None;
    }
    let key = |c: &Cell<'_>| !c.string() || number_mark(c);
    // The names in the label areas tell every record apart.
    let distinguished = |spans: &[(u32, u32)]| {
        let names: HashSet<Vec<&str>> = records
            .iter()
            .map(|&y| {
                covering(group, tall, y)
                    .filter(|c| {
                        body(c)
                            && c.nonempty()
                            && !key(c)
                            && spans.iter().any(|&(l, r)| c.left >= l && c.right <= r)
                    })
                    .map(|c| c.text())
                    .collect()
            })
            .collect();
        names.len() == records.len()
    };
    let mut next = areas.first()?.0;
    let mut end = None;
    let mut spans: Vec<(u32, u32)> = vec![];
    for (k, &(left, right, merged)) in areas.iter().enumerate() {
        if left != next {
            break;
        }
        let values = values(left, right);
        if values.is_empty() {
            break;
        }
        // A cell across several columns, such as a total, covers the row.
        let complete = records.iter().all(|&y| {
            covering(group, tall, y)
                .any(|c| body(c) && c.nonempty() && c.left <= right && c.right >= left)
        });
        let single = one_each(&values);
        let stepped = if tree_area == Some(k) {
            true
        } else if layered {
            merged || single
        } else {
            single && (k == 0 || steps(&values))
        };
        // Distinct values per group: the nearest label on the left merged
        // down across more rows, such as a subsystem over its tables.
        let owner = |c: &Cell<'_>| {
            spans.iter().rev().find_map(|&(l, r)| {
                // Merged down over the whole of `c`, so over its first row.
                covering(group, tall, c.top)
                    .find(|o| {
                        body(o)
                            && o.nonempty()
                            && o.left >= l
                            && o.right <= r
                            && o.bottom >= c.bottom
                            && o.bottom - o.top > c.bottom - c.top
                    })
                    .map(|o| (o.left, o.top))
            })
        };
        let distinct = values
            .iter()
            .map(|c| (owner(c), c.text()))
            .collect::<HashSet<_>>()
            .len();
        if complete && stepped && distinct * 5 >= values.len() * 4 {
            spans.push((left, right));
            if values.iter().any(|c| !key(c) && !identifier(c)) && distinguished(&spans) {
                end = Some(right);
                break;
            }
        } else if !((merged || single) && values.iter().all(|c| key(c))) {
            break;
        }
        end = Some(right);
        next = right + 1;
    }
    Some((end?, spans)).filter(|(_, spans)| !spans.is_empty())
}

/// Roles and header links of one table, or `None` when the group is not a table.
fn interpret(
    cells: &[Cell<'_>],
    indices: &[usize],
    explicit: Explicit,
    sheet_index: usize,
) -> Result<Option<Vec<Value>>> {
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
    let frame_row = header.is_none() && row_counts.len() == 1 && frame_row(&group);
    let is_table = explicit.is_some()
        || frame_row
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
    // Row labels start at the first column holding records. It is the left
    // edge unless a nested table took the records of the leftmost columns.
    let label_left = if group.iter().any(|c| body(c) && c.left == left) {
        left
    } else {
        group
            .iter()
            .filter(|c| body(c) && c.nonempty())
            .map(|c| c.left)
            .min()
            .unwrap_or(left)
    };
    // Header cells merged down through the whole header stand over the columns
    // that may name the records; one in the last column heads those records.
    let mut heads: Vec<_> = group
        .iter()
        .copied()
        .filter(|c| {
            header.is_some_and(|(first, _)| c.top == first)
                && c.right < right
                && c.right >= label_left
                && c.nonempty()
        })
        .collect();
    heads.sort_by_key(|c| c.left);
    let stub = header.and_then(|(first, last)| {
        let merged_down = |c: &Cell<'_>| c.top == first && c.bottom == last && c.bottom > c.top;
        stub_labels(&group, &heads, &merged_down, &body)
    });
    let stub_end = stub.as_ref().map(|(end, _)| *end).or_else(|| {
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
            .filter(|c| c.left == label_left && (label_left == left || body(c)))
            .map(|c| c.right)
            .min()
            .unwrap_or(label_left)
    });
    // A list numbered in its first column, such as items ① to ⑧ beside a
    // screen image, is named by the column of names after the numbers as
    // well, past IDs between them. So is a list numbered after a category
    // merged down over its records, such as the records of a file layout.
    let through = if stub_end.is_none() && label_left == left && header.is_some() {
        let starting = |x: u32| -> Vec<&&Cell<'_>> {
            group
                .iter()
                .filter(|c| body(c) && c.nonempty() && c.left == x)
                .collect()
        };
        let records: BTreeSet<u32> = group
            .iter()
            .filter(|c| body(c) && c.nonempty())
            .map(|c| c.top)
            .collect();
        // One cell in every record, each standing under a heading of its own
        // through every header row, not under a group of columns, such as the
        // source item of a mapping.
        let column = |x: u32| -> Option<(u32, Vec<&&Cell<'_>>)> {
            let cells = starting(x);
            let own_heading = group.iter().any(|c| {
                in_header(c)
                    && header.is_some_and(|(first, last)| c.top == first && c.bottom == last)
                    && c.left == x
            });
            let right = cells.iter().map(|c| c.right).max()?;
            (own_heading
                && cells.len() == records.len()
                && cells.iter().map(|c| c.top).collect::<BTreeSet<_>>() == records)
                .then_some((right, cells))
        };
        let numbered =
            |cells: &[&&Cell<'_>]| cells.len() >= 2 && cells.iter().all(|c| number_mark(c));
        let first = starting(left);
        let category = first.len() < records.len()
            && first.iter().any(|c| c.bottom > c.top)
            && first.iter().all(|c| c.string() && !number_mark(c));
        let mut at = None;
        if first.len() >= 2 && first.iter().all(|c| c.right == through && number_mark(c)) {
            at = Some(through + 1);
        } else if category && let Some((end, numbers)) = column(through + 1) {
            at = numbered(&numbers).then_some(end + 1);
        }
        // A code names a record only with the name after it; a date written
        // as text, with no letters, is an attribute.
        let code = |c: &Cell<'_>| {
            let text = c.text().trim();
            text.chars().any(|ch| ch.is_ascii_alphabetic())
                && text.chars().any(|ch| ch.is_ascii_digit() || ch == '_')
                && text
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
        };
        let mut names = at.and_then(column);
        while let Some((end, cells)) = &names
            && cells.iter().all(|c| code(c))
        {
            names = column(end + 1);
        }
        // Names that do not repeat within the category on their left.
        let first: Vec<&Cell<'_>> = first.into_iter().copied().collect();
        let first_tall = height(&first);
        let owner = |c: &Cell<'_>| {
            covering(&first, first_tall, c.top)
                .find(|_| category)
                .map(|o| o.top)
        };
        match names {
            Some((end, cells))
                if cells
                    .iter()
                    .all(|c| c.string() && !number_mark(c) && !identifier(c) && !code(c))
                    && cells
                        .iter()
                        .map(|c| (owner(c), c.text()))
                        .collect::<HashSet<_>>()
                        .len()
                        == cells.len() =>
            {
                end
            }
            _ => through,
        }
    } else {
        through
    };
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
    // So does a label written only on the first record of its group, blank
    // below, over a table of several columns of numbers, as regions over
    // their cities in monthly figures.
    let first_only = || {
        let records: BTreeSet<u32> = group
            .iter()
            .filter(|c| body(c) && c.nonempty())
            .map(|c| c.top)
            .collect();
        let at_left: Vec<&&Cell<'_>> = group
            .iter()
            .filter(|c| body(c) && c.left == left && c.nonempty())
            .collect();
        let firsts: BTreeSet<u32> = at_left.iter().map(|c| c.top).collect();
        // Each label stands over two records or more, blank below it.
        let grouping = at_left.iter().all(|c| c.string())
            && firsts.iter().all(|&y| {
                records
                    .range(y + 1..)
                    .next()
                    .is_some_and(|next| !firsts.contains(next))
            });
        let measures: BTreeSet<u32> = group
            .iter()
            .filter(|c| body(c) && c.left > left + 1 && c.nonempty() && !c.string())
            .map(|c| c.left)
            .collect();
        grouping
            && firsts.len() >= 2
            && firsts.len() < records.len()
            && records.first() == firsts.first()
            && measures.len() >= 2
    };
    let nested_labels = !single
        && through == left
        && (group
            .iter()
            .any(|c| body(c) && c.left == left && c.bottom > c.top && c.string() && c.nonempty())
            || first_only())
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
        // Labels of a form may not line up in columns when its rows divide
        // the width differently; each row still alternates labels and values.
        let form = frame_row
            || header.is_none()
                && shaded.contains(&left)
                && group
                    .iter()
                    .filter(|c| body(c) && c.nonempty())
                    .map(|c| c.top)
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .filter(|&y| {
                        let line = valued(row(&group, y));
                        line.iter().filter(|c| c.fill() != 0 && c.string()).count() >= 2
                            && line.iter().any(|c| c.fill() == 0)
                    })
                    .count()
                    >= 2;
        if form {
            Labels::Marked
        } else if header.is_none() && plain && shaded.contains(&left) && shaded.len() >= 2 {
            Labels::Columns(shaded)
        } else if year_labels {
            Labels::Through(left)
        } else if let Some((_, spans)) = &stub {
            Labels::Spans(spans.clone())
        } else if nested_labels {
            Labels::Through(left + 1)
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
                // A label across all the label columns, such as a total.
                Labels::Spans(spans) => {
                    spans.iter().any(|&(l, r)| c.left >= l && c.right <= r)
                        || (c.left <= spans[0].0 && c.right >= spans[spans.len() - 1].1)
                }
                Labels::Marked => c.fill() != 0,
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
    // Bands of different looks nest: a look first seen after another is a
    // level below it, as the band of a job under the band of its job net.
    let mut looks: Vec<(bool, u64)> = vec![];
    let mut band_parents: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut open_bands: Vec<(usize, usize)> = vec![];
    for &i in &bands {
        let look = (group[i].bold(), group[i].fill());
        let level = match looks.iter().position(|l| *l == look) {
            Some(level) => level,
            None => {
                looks.push(look);
                looks.len() - 1
            }
        };
        while open_bands.last().is_some_and(|&(_, l)| l >= level) {
            open_bands.pop();
        }
        band_parents.insert(i, open_bands.iter().map(|&(b, _)| b).collect());
        open_bands.push((i, level));
    }
    // A repeated header stands in for the header cells it repeats.
    let repeats: HashSet<usize> = header_rows
        .iter()
        .filter(|(row, _)| repeated.contains(row))
        .flat_map(|(_, headers)| headers.iter().copied())
        .collect();
    // Rows that a label does not reach across.
    let barriers: BTreeSet<_> = bands
        .iter()
        .map(|i| group[*i].top)
        .chain(repeated.iter().copied())
        .collect();
    // Label columns that a cell of the row stands in, including one across
    // several columns such as a total, which a label above does not continue into.
    let occupied: HashSet<_> = group
        .iter()
        .flat_map(|c| {
            label_columns
                .range(c.left..=c.right)
                .flat_map(move |(column, _)| (c.top..=c.bottom).map(move |y| (*column, y)))
        })
        .collect();
    // Indented labels belong to the nearest label above with a smaller indent.
    // Names stepped across the columns of one label span indent by the column
    // they start in.
    let area = |c: &Cell<'_>| match &labels {
        Labels::Spans(spans) => spans
            .iter()
            .find(|&&(l, r)| c.left >= l && c.right <= r)
            .map_or(c.left, |&(l, _)| l),
        _ => c.left,
    };
    let level = |c: &Cell<'_>| (c.left - area(c), c.indent());
    let mut areas: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
    for column_labels in label_columns.values() {
        for &i in column_labels {
            areas.entry(area(group[i])).or_default().push(i);
        }
    }
    let mut parents: HashMap<usize, Vec<usize>> = HashMap::new();
    for column in areas.values_mut() {
        column.sort_by_key(|&i| (group[i].top, group[i].left));
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
                .is_some_and(|p| level(group[*p]) >= level(group[i]))
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
    // The header cells each cell links to, in the header rows above it.
    let header_links = |c: &Cell<'_>, current: &str| -> Vec<usize> {
        let mut links = vec![];
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
                    (
                        h.right.min(c.right) - h.left.max(c.left),
                        std::cmp::Reverse(h.left),
                    )
                })
            {
                links.push(widest);
            }
        }
        // Of a header cell written again, the last repeat linked
        // stands in for the copies above it.
        let mut last_repeat: HashMap<((u32, u32), &str), u32> = HashMap::new();
        for &i in links.iter().filter(|i| repeats.contains(i)) {
            let top = last_repeat
                .entry((group[i].span(), group[i].text()))
                .or_default();
            *top = (*top).max(group[i].top);
        }
        links.retain(|&i| {
            last_repeat
                .get(&(group[i].span(), group[i].text()))
                .is_none_or(|&top| group[i].top >= top)
        });
        links
    };
    // The last row the header cells of the first k header rows reach, so
    // that cells under all of them, spanning the same columns, link alike.
    let header_tops: Vec<u32> = header_rows.keys().copied().collect();
    let mut reach = vec![0; header_tops.len() + 1];
    for (k, headers) in header_rows.values().enumerate() {
        let bottom = headers.iter().map(|&i| group[i].bottom).max().unwrap_or(0);
        reach[k + 1] = reach[k].max(bottom);
    }
    let mut alike: HashMap<(usize, u32, u32), Vec<usize>> = HashMap::new();
    let interpreted: Vec<_> = group
        .iter()
        .enumerate()
        .map(|(index, c)| {
            let current = roles[index];
            let mut links = vec![];
            let wide = !single && c.span() == (left, right);
            if current != "text" && !(wide && !in_header(c) && current != "column_header") {
                let rows_above = header_tops.partition_point(|&top| top < c.top);
                links = if current != "column_header" && reach[rows_above] < c.top {
                    alike
                        .entry((rows_above, c.left, c.right))
                        .or_insert_with(|| header_links(c, current))
                        .clone()
                } else {
                    header_links(c, current)
                };
            }
            if matches!(current, "data" | "row_header") && !band(c) {
                let mut beside: Vec<_> = label_rows.get(&c.top).cloned().unwrap_or_default();
                // A label left blank continues the label above it. Stepped
                // names have their parents instead: a row naming itself in a
                // label span continues no other name of that span.
                let own = label_rows.get(&c.top).map(Vec::as_slice).unwrap_or_default();
                for (column, labels) in &label_columns {
                    if occupied.contains(&(*column, c.top))
                        || own.iter().any(|&o| area(group[o]) == area(group[labels[0]]))
                    {
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
                // another block of columns is not this cell's label. Columns
                // passed over within the stub, such as numbers between a
                // section and the item names, do not part them.
                let passed_over = |to: u32| match &labels {
                    Labels::Spans(spans) => spans.last().is_some_and(|&(_, r)| to <= r + 1),
                    Labels::Through(limit) => to <= limit.saturating_add(1),
                    _ => false,
                };
                let mut edge = c.left;
                let mut chosen = vec![];
                for (k, &i) in before.iter().enumerate() {
                    let adjoining = group[i].right.checked_add(1) == Some(edge)
                        || (group[i].right < edge && passed_over(edge));
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
                // A form label followed directly by other labels groups them
                // and heads the rest of its row, as an estimate over its kinds.
                if matches!(labels, Labels::Marked) {
                    let line = label_rows.get(&c.top).map(Vec::as_slice).unwrap_or_default();
                    links.extend(line.iter().copied().filter(|&i| {
                        group[i].right < c.left
                            && group[i].top == c.top
                            && line
                                .iter()
                                .any(|&j| group[j].left == group[i].right + 1 && group[j].top == c.top)
                    }));
                }
                let above = bands.partition_point(|&i| group[i].top < c.top);
                if let Some(&nearest) = above.checked_sub(1).and_then(|k| bands.get(k)) {
                    links.push(nearest);
                    links.extend(&band_parents[&nearest]);
                }
            }
            if band(c) {
                links.extend(band_parents.get(&index).into_iter().flatten());
            }
            links.retain(|&i| i != index);
            links.sort_unstable();
            links.dedup();
            let links: Vec<_> = links.into_iter().map(|i| id_for(group[i])).collect();
            json!({"id":id_for(c),"address":c.source["address"],"role":current,"headers":links,
                "text_state":if c.nonempty() || c.source["formula"].is_string() {"read"} else {"empty"}})
        })
        .collect();
    Ok(Some(interpreted))
}

/// The parent of each explicit table definition, by index, from the name of
/// the definition it is nested in.
fn explicit_parents(tables: &[Value]) -> Result<Vec<Option<usize>>> {
    let names: HashMap<&str, usize> = tables
        .iter()
        .enumerate()
        .filter_map(|(i, t)| Some((t["name"].as_str()?, i)))
        .collect();
    let parents: Vec<Option<usize>> = tables
        .iter()
        .map(|t| match t["parent"].as_str() {
            Some(name) => names
                .get(name)
                .copied()
                .map(Some)
                .context("unknown parent table definition"),
            None => Ok(None),
        })
        .collect::<Result<_>>()?;
    for start in 0..parents.len() {
        let mut table = start;
        for _ in 0..=parents.len() {
            match parents[table] {
                Some(parent) => table = parent,
                None => break,
            }
        }
        ensure!(parents[table].is_none(), "cyclic table definitions");
    }
    Ok(parents)
}

/// The cells of `parent` that qualify a table nested in it at `child` (left,
/// top, right, bottom): the lowest column headers above the child's columns,
/// and the parent's cells beside the child, those of the records it belongs to.
fn context(
    by_address: &HashMap<&str, &Cell<'_>>,
    parent: &Value,
    child: (u32, u32, u32, u32),
) -> Result<Vec<Value>> {
    let (x0, r0, x1, r1) = child;
    let mut members = vec![];
    for cell in array(&parent["cells"])? {
        let c = *by_address
            .get(string(&cell["address"])?)
            .context("parent cell missing from extraction")?;
        if c.nonempty() {
            members.push((c, cell));
        }
    }
    let above: Vec<_> = members
        .iter()
        .filter(|(c, cell)| {
            cell["role"] == "column_header" && c.left <= x1 && c.right >= x0 && c.bottom < r0
        })
        .collect();
    let lowest = above.iter().map(|(c, _)| c.bottom).max();
    let mut chosen: Vec<_> = above
        .into_iter()
        .filter(|(c, _)| Some(c.bottom) == lowest)
        .chain(members.iter().filter(|(c, cell)| {
            cell["role"] != "column_header"
                && cell["role"] != "text"
                && (c.right < x0 || c.left > x1)
                && c.top <= r1
                && c.bottom >= r0
        }))
        .collect();
    chosen.sort_by_key(|(c, _)| (c.top, c.left));
    Ok(chosen
        .into_iter()
        .map(|(_, cell)| cell["id"].clone())
        .collect())
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
                style: crate::extraction::style(sheet, c)?,
                left,
                top,
                right,
                bottom,
            });
        }
        cells.sort_by_key(|c| (c.top, c.left));
        let mut assigned = BTreeSet::new();
        let mut groups: Vec<Candidate> = vec![];
        // Explicit table boundaries have precedence over layout heuristics. A
        // table nested in another claims its cells first; the outer table
        // keeps the rest of its range.
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
            let parents = explicit_parents(tables)?;
            let ancestors = |mut table: usize| {
                let mut chain = vec![];
                while let Some(parent) = parents[table] {
                    chain.push(parent);
                    table = parent;
                }
                chain
            };
            let mut order: Vec<usize> = (0..tables.len()).collect();
            order.sort_by_key(|&t| std::cmp::Reverse(ancestors(t).len()));
            let mut owner: HashMap<usize, usize> = HashMap::new();
            let mut found = vec![];
            for t in order {
                let bounds = range(string(&tables[t]["range"])?)?;
                let mut indices = vec![];
                for i in rows
                    .range(bounds.0.1..=bounds.1.1)
                    .flat_map(|(_, row)| within(row, bounds.0.0, bounds.1.0))
                {
                    match owner.get(&i) {
                        None => {
                            owner.insert(i, t);
                            indices.push(i);
                        }
                        Some(&inner) => ensure!(
                            ancestors(inner).contains(&t),
                            "overlapping table definitions"
                        ),
                    }
                }
                assigned.extend(indices.iter().copied());
                found.push((
                    t,
                    indices,
                    bounds,
                    tables[t]["header_rows"].as_u64().unwrap_or(0) as u32,
                ));
            }
            found.sort_by_key(|(t, ..)| *t);
            for (t, mut indices, bounds, mut header_rows) in found {
                let mut header_top = bounds.0.1;
                // Group headers written directly above an outermost table:
                // bold cells within its columns, at least one merged across several.
                if header_rows > 0
                    && parents[t].is_none()
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
                indices.sort_by_key(|&i| (cells[i].top, cells[i].left));
                groups.push(Candidate {
                    key: t,
                    group: indices,
                    explicit: Some((header_top, header_rows)),
                    parent: parents[t],
                });
            }
        }
        let mut leftovers = side_annotations(&cells, &assigned);
        assigned.extend(leftovers.iter().copied());
        let prose = prose_bullets(&cells, &assigned);
        leftovers.extend(prose);
        assigned.extend(leftovers.iter().copied());
        // An empty merged range is one box; other ranges list single cells.
        let merged: HashSet<&str> = array(&sheet["merges"])?
            .iter()
            .filter_map(Value::as_str)
            .collect();
        let mut boxes = vec![];
        for r in array(sheet.get("bordered_blanks").unwrap_or(&json!([])))? {
            let ((left, top), (right, bottom)) = range(string(r)?)?;
            if merged.contains(string(r)?) {
                boxes.push(((left, top, right, bottom), true));
            } else {
                for y in top..=bottom {
                    for x in left..=right {
                        boxes.push(((x, y, x, y), false));
                    }
                }
            }
        }
        let free: Vec<_> = (0..cells.len()).filter(|i| !assigned.contains(i)).collect();
        let boxes = fields(&cells, &free, &boxes);
        let (mut remaining, exhausted) = components(&cells, &boxes, &assigned);
        if exhausted {
            leftovers.extend(remaining.into_iter().flatten());
        } else {
            attach_captions(&cells, &mut remaining);
            attach_headers(&cells, &mut remaining);
            join_columns(&cells, &mut remaining);
            for group in remaining.into_iter().filter(|g| !g.is_empty()) {
                let base = groups.iter().map(|c| c.key + 1).max().unwrap_or(0);
                for (offset, (table, parent)) in refine(&cells, &boxes, group, &mut leftovers)?
                    .into_iter()
                    .enumerate()
                {
                    groups.push(Candidate {
                        key: base + offset,
                        group: table,
                        explicit: None,
                        parent: parent.map(|p| base + p),
                    });
                }
            }
        }
        groups.retain(|candidate| !candidate.group.is_empty());
        groups.sort_by_key(|candidate| {
            candidate
                .group
                .iter()
                .map(|i| (cells[*i].top, cells[*i].left))
                .min()
                .unwrap()
        });
        // A short blank band can be a page or writing break inside one table.
        // Join only when numbered record keys continue exactly and the two
        // bands occupy the same horizontal extent. This also tolerates changed
        // logical column widths in the lower band. Nested tables keep their place.
        let mut at = 0;
        while at + 1 < groups.len() {
            let (a, b) = (&groups[at], &groups[at + 1]);
            let nested = |c: &Candidate| {
                c.parent.is_some() || groups.iter().any(|other| other.parent == Some(c.key))
            };
            let same_table =
                if a.explicit.is_none() && b.explicit.is_none() && !nested(a) && !nested(b) {
                    let (al, _, ar, ab) = bounds(&cells, &a.group);
                    let (bl, bt, br, _) = bounds(&cells, &b.group);
                    let view = |ids: &[usize]| -> Vec<_> {
                        let mut v: Vec<_> = ids.iter().map(|&i| &cells[i]).collect();
                        v.sort_by_key(|c| (c.top, c.left));
                        v
                    };
                    let later = view(&b.group);
                    let later_layout = layout(&later, None)?;
                    let titled_later = later_layout.caption
                        && later
                            .iter()
                            .any(|c| c.top == later_layout.top && c.nonempty());
                    let a_headers = leaf_signature(&cells, &a.group, None)?;
                    let b_headers = leaf_signature(&cells, &b.group, None)?;
                    let different_headers = !a_headers.is_empty()
                        && !b_headers.is_empty()
                        && (a_headers.len() != b_headers.len()
                            || !a_headers
                                .iter()
                                .zip(&b_headers)
                                .all(|(first, second)| first.1 == second.1));
                    match (
                        record_series(&view(&a.group), al),
                        record_series(&later, bl),
                    ) {
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
                let next = groups.remove(at + 1).group;
                groups[at].group.extend(next);
                groups[at]
                    .group
                    .sort_by_key(|&i| (cells[i].top, cells[i].left));
            } else {
                at += 1;
            }
        }
        // A blank line inside a hierarchy parts it in two. The lower part goes
        // on under the upper one when it starts deeper than the upper part's
        // top level, within its width, and the two read as one hierarchy.
        let mut at = 0;
        while at + 1 < groups.len() {
            let (a, b) = (&groups[at], &groups[at + 1]);
            let free = |c: &Candidate| {
                c.explicit.is_none()
                    && c.parent.is_none()
                    && !groups.iter().any(|other| other.parent == Some(c.key))
            };
            let (al, _, ar, ab) = bounds(&cells, &a.group);
            let (bl, bt, br, _) = bounds(&cells, &b.group);
            let gap = ab + 1;
            let blank = !cells
                .iter()
                .any(|c| c.top <= gap && c.bottom >= gap && c.left <= ar && c.right >= al);
            let joined = || {
                let mut joined = [a.group.as_slice(), b.group.as_slice()].concat();
                joined.sort_by_key(|&i| (cells[i].top, cells[i].left));
                joined
            };
            if free(a)
                && free(b)
                && bt == gap + 1
                && blank
                && bl > al
                && br <= ar
                && outline::outline(&cells, &joined(), sheet_index).is_some()
            {
                let next = groups.remove(at + 1).group;
                groups[at].group.extend(next);
                groups[at]
                    .group
                    .sort_by_key(|&i| (cells[i].top, cells[i].left));
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
        // Output index of each interpreted candidate, by key.
        let mut interpreted_tables: HashMap<usize, usize> = HashMap::new();
        let mut nested = vec![];
        for candidate in &groups {
            let (indices, explicit) = (&candidate.group, candidate.explicit);
            let free = explicit.is_none()
                && candidate.parent.is_none()
                && !groups
                    .iter()
                    .any(|other| other.parent == Some(candidate.key));
            let hierarchy = free
                .then(|| outline::outline(&cells, indices, sheet_index))
                .flatten();
            let stepped = hierarchy.is_some();
            let interpreted = match hierarchy {
                Some(interpreted) => Some(interpreted),
                None => interpret(&cells, indices, explicit, sheet_index)?,
            };
            let Some(interpreted) = interpreted else {
                leftovers.extend(indices);
                preceding = None;
                continue;
            };
            let rectangle = bounds(&cells, indices);
            let signature = if explicit.is_none() {
                leaf_signature(&cells, indices, explicit)?
            } else {
                vec![]
            };
            let id = format!("sheet-{}-table-{}", sheet_index + 1, output.len() + 1);
            // The cells give the range; the reason leaves it out, so moving the
            // table does not change its reading.
            let mut reason = format!(
                "{}. Header links are layout hypotheses requiring review.",
                if explicit.is_some() {
                    "Explicit table boundary from the source"
                } else if candidate.parent.is_some() {
                    "Labels and records written within the records of another table, beside its merged cells"
                } else if stepped {
                    "A hierarchy of names stepped across columns, indented, numbered by level or merged down; each name heads the deeper names after it"
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
            let element = json!({"id":id,"kind":"table","sheet":sheet["name"],"evidence":[],
                "reading":{"method":"parser","actor":"arp4","reason":reason},
                "cells":interpreted});
            if let Some(parent) = candidate.parent {
                nested.push((output.len(), parent, rectangle));
            }
            interpreted_tables.insert(candidate.key, output.len());
            output.push(element);
        }
        // A table whose parent was not read as a table stands on its own.
        let by_address: HashMap<&str, &Cell<'_>> = cells
            .iter()
            .map(|c| (c.source["address"].as_str().unwrap_or(""), c))
            .collect();
        for (child, parent, rectangle) in nested {
            if let Some(&parent) = interpreted_tables.get(&parent) {
                output[child]["parent"] = json!({"element":output[parent]["id"],
                    "context":context(&by_address, &output[parent], rectangle)?});
            }
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
    fn rows_and_covering_cells_are_those_a_scan_finds() {
        let values: Vec<_> = (1..=12)
            .flat_map(|row| ["A", "B", "C"].map(|column| cell(&format!("{column}{row}"), "x")))
            .collect();
        // B2 is merged down to B5 and C7 to C12; A and the rest are single.
        fn extent(c: &Value) -> Cell<'_> {
            let (left, top) = coordinate(c["address"].as_str().unwrap()).unwrap();
            let bottom = match c["address"].as_str().unwrap() {
                "B2" => 5,
                "C7" => 12,
                _ => top,
            };
            Cell {
                source: c,
                style: &Value::Null,
                left,
                top,
                right: left,
                bottom,
            }
        }
        let cells: Vec<_> = values.iter().map(extent).collect();
        let group: Vec<_> = cells.iter().collect();
        let tall = height(&group);
        assert_eq!(tall, 5);
        let at = |found: Vec<&Cell<'_>>| -> Vec<(u32, u32)> {
            found.iter().map(|c| (c.left, c.top)).collect()
        };
        for y in 0..=13 {
            let scan: Vec<_> = group.iter().copied().filter(|c| c.top == y).collect();
            assert_eq!(at(row(&group, y)), at(scan), "row {y}");
            let scan: Vec<_> = group
                .iter()
                .copied()
                .filter(|c| c.top <= y && c.bottom >= y)
                .collect();
            assert_eq!(at(covering(&group, tall, y).collect()), at(scan), "row {y}");
        }
    }
    #[test]
    fn records_under_a_repeated_header_link_to_its_last_repeat() {
        let head = |address: &str, value: &str| json!({"address":address,"value":value,"type":"string","formula":null,"style":{"bold":true,"fill":1,"border":1}});
        let record = |address: &str, value: &str| json!({"address":address,"value":value,"type":"string","formula":null,"style":{"bold":false,"fill":0,"border":1}});
        let mut cells = vec![];
        for row in 1..=8 {
            if [1, 4, 7].contains(&row) {
                cells.extend([
                    head(&format!("A{row}"), "ID"),
                    head(&format!("B{row}"), "名前"),
                    head(&format!("C{row}"), "種別"),
                ]);
            } else {
                cells.extend([
                    record(&format!("A{row}"), &format!("K-{row:02}")),
                    record(&format!("B{row}"), &format!("名前{row}")),
                    record(&format!("C{row}"), &format!("種別{row}")),
                ]);
            }
        }
        let extraction = json!({"sheets":[{"name":"S","merges":[],"cells":cells}]});
        let result = elements(&extraction).unwrap();
        let table = result.iter().find(|e| e["kind"] == "table").unwrap();
        let headers = |address: &str| -> Vec<String> {
            let found = table["cells"]
                .as_array()
                .unwrap()
                .iter()
                .find(|c| c["address"] == address)
                .unwrap();
            found["headers"]
                .as_array()
                .unwrap()
                .iter()
                .map(|h| h.as_str().unwrap().to_owned())
                .filter(|h| h.starts_with("s1-C"))
                .collect()
        };
        assert_eq!(headers("C2"), ["s1-C1"]);
        assert_eq!(headers("C5"), ["s1-C4"]);
        assert_eq!(headers("C8"), ["s1-C7"]);
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
    fn explicit_nested_tables_claim_their_cells_and_name_their_context() {
        let cells: Vec<_> = [
            ("A1", "Item"),
            ("B1", "Checks"),
            ("A2", "Code"),
            ("B2", "Kind"),
            ("C2", "Message"),
            ("B3", "Required"),
            ("C3", "E001"),
            ("A4", "Date"),
            ("B4", "none"),
        ]
        .into_iter()
        .map(|(address, value)| cell(address, value))
        .collect();
        let tables = json!([
            {"name":"outer","range":"A1:C4","header_rows":1},
            {"name":"inner","range":"B2:C3","header_rows":1,"parent":"outer"}
        ]);
        let mut extraction = json!({"sheets":[{"name":"S","merges":["A2:A3","B1:C1","B4:C4"],
            "tables":tables,"cells":cells}]});
        let result = elements(&extraction).unwrap();
        let addresses = |table: &Value| -> Vec<String> {
            table["cells"]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| c["address"].as_str().unwrap().to_owned())
                .collect()
        };
        assert_eq!(addresses(&result[0]), ["A1", "B1", "A2", "A4", "B4"]);
        assert_eq!(addresses(&result[1]), ["B2", "C2", "B3", "C3"]);
        assert_eq!(
            result[1]["parent"],
            json!({"element":"sheet-1-table-1","context":["s1-B1","s1-A2"]})
        );
        extraction["sheets"][0]["tables"][0]["parent"] = json!("inner");
        assert!(
            elements(&extraction)
                .unwrap_err()
                .to_string()
                .contains("cyclic")
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
