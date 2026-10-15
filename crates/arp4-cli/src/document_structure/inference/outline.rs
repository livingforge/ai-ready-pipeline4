//! Hierarchies drawn on the grid rather than records with fields: names
//! stepped across columns, indented, numbered by level or merged down over
//! their descendants. Each source of depth gives a level, and one reading
//! builds the tree from the levels, whichever source the author used.
use super::*;

/// How an item number is written: digits in parts, such as `2.1.3` or
/// `2-1-3`, by the count of parts, or another form of one number, such as
/// `(1)`, `①` or `第1章`, by its shape with the digits left out.
#[derive(Clone, PartialEq, Eq, Hash)]
enum Numbering {
    Parts(usize),
    Form(String),
}

fn numbering(c: &Cell<'_>) -> Option<Numbering> {
    if !c.string() {
        return c.source["value"]
            .as_f64()
            .is_some_and(|v| v >= 0.0 && v.fract() == 0.0)
            .then_some(Numbering::Parts(1));
    }
    let text = c.text().trim();
    let digits = |part: &str| !part.is_empty() && part.chars().all(|ch| ch.is_ascii_digit());
    let bare = text.strip_suffix('.').unwrap_or(text);
    let parts: Vec<&str> = bare.split(['.', '-', '－', '‐']).collect();
    if parts.iter().all(|part| digits(part)) {
        return Some(Numbering::Parts(parts.len()));
    }
    // The shape of one number in a form of its own: digits as `#`, a circled
    // number as `◯`, and brackets, `)` or the words of a chapter around them.
    let mut shape = String::new();
    for ch in text.chars() {
        let mark = match ch {
            '0'..='9' | '０'..='９' => '#',
            '\u{2460}'..='\u{2473}' | '\u{2776}'..='\u{2793}' | '\u{3251}'..='\u{325F}' => '◯',
            '(' | ')' | '（' | '）' | '[' | ']' | '［' | '］' | '.' | '．' | '第' | '章' | '節'
            | '項' | '条' => ch,
            _ => return None,
        };
        if !(mark == '#' && shape.ends_with('#')) {
            shape.push(mark);
        }
    }
    (shape.matches(['#', '◯']).count() == 1).then_some(Numbering::Form(shape))
}

/// The column of item numbers written in two forms or more, such as `1` and
/// `1.1`, or `1`, `(1)` and `①`; one form alone numbers records, not levels.
fn numbered_column(cells: &[&Cell<'_>]) -> Option<u32> {
    let columns: BTreeSet<u32> = cells.iter().map(|c| c.left).collect();
    let starting = |x: u32| cells.iter().filter(move |c| c.left == x);
    columns.into_iter().find(|&x| {
        let forms: Option<HashSet<Numbering>> = starting(x).map(|c| numbering(c)).collect();
        starting(x).count() >= 2 && forms.is_some_and(|forms| forms.len() >= 2)
    })
}

/// How far a name is indented, in half-width characters: the indent of the
/// cell's format, and the spaces and ruled-line pieces (such as `│` and `├─`)
/// before the name, a full-width space or piece counting as two.
fn width(c: &Cell<'_>) -> usize {
    let piece = |ch: char| match ch {
        ' ' => Some(1),
        '\u{3000}' | '\u{2500}'..='\u{257F}' => Some(2),
        _ => None,
    };
    // A cell of pieces alone, such as a branch in a cell of its own, stands
    // before no name.
    let named = c.text().chars().any(|ch| piece(ch).is_none());
    let lead: usize = c.text().chars().map_while(piece).sum();
    c.style["indent"].as_u64().unwrap_or(0) as usize * 2 + if named { lead } else { 0 }
}

/// Marks written before the names, such as a `+` to unfold a folder, a branch
/// of ruled lines or a `[D]` for a folder, as (row, column). A cell before
/// another on its row, where the later cell's parent stands or further left,
/// would cut the later cell off its parent if it were a name; a parent is
/// the last name on a row of one or two cells, or merged down over later
/// rows; the last of three cells or more is a field of a record. It is a mark
/// when it has no letter or digit, or when the same text stands so more than
/// once; that text is a mark wherever it is written, even alone on a row.
/// Numbers are never marks, nor ancestors merged down over their descendants
/// unless they have no letter or digit, as a `+` merged down over the two
/// rows of its folder, nor a name that stands so once, as an ancestor beside
/// its only child.
fn marks(cells: &[&Cell<'_>]) -> HashSet<(u32, u32)> {
    let numbered = numbered_column(cells);
    let symbol = |c: &Cell<'_>| !c.text().chars().any(char::is_alphanumeric);
    let candidate =
        |c: &Cell<'_>| c.string() && (c.bottom == c.top || symbol(c)) && Some(c.left) != numbered;
    let level = |c: &Cell<'_>| (c.left, width(c));
    let mut rows: BTreeMap<u32, Vec<&Cell<'_>>> = BTreeMap::new();
    for &c in cells {
        rows.entry(c.top).or_default().push(c);
    }
    // Names above, each with whether it can be a parent: the last on its row,
    // or merged down; a field of a record before another field is no parent.
    let mut open: Vec<(&Cell<'_>, bool)> = vec![];
    let mut cutting: Vec<&Cell<'_>> = vec![];
    for line in rows.values_mut() {
        line.sort_by_key(|c| c.left);
        let mut kept = vec![];
        for (k, &c) in line.iter().enumerate() {
            let cuts = candidate(c)
                && line.get(k + 1).is_some_and(|next| {
                    open.iter()
                        .rev()
                        .find(|(p, _)| level(p) < level(next))
                        .is_some_and(|&(p, parent)| parent && level(c) <= level(p))
                });
            if cuts {
                cutting.push(c);
            } else {
                kept.push(c);
            }
        }
        // Three cells or more on a row are a record, whose last is a field.
        let record = kept.iter().filter(|c| c.bottom == c.top).count() >= 3;
        let last = kept.len();
        for (k, c) in kept.into_iter().enumerate() {
            while open.last().is_some_and(|(p, _)| level(p) >= level(c)) {
                open.pop();
            }
            open.push((c, (k + 1 == last && !record) || c.bottom > c.top));
        }
    }
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for c in &cutting {
        *counts.entry(c.text().trim()).or_default() += 1;
    }
    let texts: HashSet<&str> = cutting
        .iter()
        .map(|c| c.text().trim())
        .filter(|text| !text.chars().any(char::is_alphanumeric) || counts[text] >= 2)
        .collect();
    cells
        .iter()
        .filter(|c| candidate(c) && texts.contains(c.text().trim()))
        .map(|c| (c.top, c.left))
        .collect()
}

/// The cells that are not marks.
fn unmarked<'c, 'a>(cells: &[&'c Cell<'a>]) -> Vec<&'c Cell<'a>> {
    let marked = marks(cells);
    cells
        .iter()
        .copied()
        .filter(|c| !marked.contains(&(c.top, c.left)))
        .collect()
}

/// Notes written on the right of the names, such as `※旧版`, as (row,
/// column): the last cells of rows, beyond every other cell, in columns
/// written on fewer than half of the rows. Columns written on most rows hold
/// the fields of records instead, and then there are no notes. A value alone
/// on its row, as on the second line of a record, ends it when it stands
/// beyond every other cell under a heading of its own (`headed`).
fn side_notes(cells: &[&Cell<'_>], headed: impl Fn(&Cell<'_>) -> bool) -> HashSet<(u32, u32)> {
    let mut rows: BTreeMap<u32, Vec<&Cell<'_>>> = BTreeMap::new();
    for &c in cells {
        rows.entry(c.top).or_default().push(c);
    }
    // The rightmost cell of each column, to find what stands beyond the
    // cells of all other columns.
    let mut rightmost: HashMap<u32, u32> = HashMap::new();
    for c in cells {
        let r = rightmost.entry(c.left).or_default();
        *r = (*r).max(c.right);
    }
    let beyond_others = |c: &Cell<'_>| {
        rightmost
            .iter()
            .all(|(&x, &right)| x == c.left || right < c.left)
    };
    let mut reach = 0;
    let mut ends = vec![];
    for line in rows.values_mut() {
        line.sort_by_key(|c| c.left);
        let rest = match line.as_slice() {
            [rest @ .., end] if !rest.is_empty() || (headed(end) && beyond_others(end)) => {
                ends.push(*end);
                rest
            }
            all => all,
        };
        reach = reach.max(rest.iter().map(|c| c.right).max().unwrap_or(0));
    }
    let beyond: Vec<&Cell<'_>> = ends.into_iter().filter(|c| c.left > reach).collect();
    let sparse = beyond.iter().all(|c| {
        let written = beyond.iter().filter(|o| o.left == c.left).count();
        written * 2 < rows.len()
    });
    if !sparse {
        return HashSet::new();
    }
    beyond.iter().map(|c| (c.top, c.left)).collect()
}

/// Whether the first row (`heads`), written as plain as the names, heads
/// them: its cells are text on one row, and one of them reaches over the
/// columns the names under it step across, one name on a row, as a heading
/// over the levels of a folder tree, merged over them or written at their
/// start. A name stands over no names stepping in the columns after it.
fn plain_heading(group: &[&Cell<'_>], heads: &[&Cell<'_>]) -> bool {
    let Some(top) = heads.first().map(|h| h.top) else {
        return false;
    };
    // A heading reaches over the columns up to the next heading, merged
    // over them or not.
    let edge = group.iter().map(|c| c.right).max().unwrap_or(0);
    let reach = |h: &Cell<'_>| {
        heads
            .iter()
            .filter(|o| o.left > h.right)
            .map(|o| o.left - 1)
            .min()
            .unwrap_or(edge)
    };
    heads.len() >= 2
        && heads
            .iter()
            .all(|h| h.string() && h.top == top && h.bottom == top)
        && heads.iter().any(|h| {
            let right = reach(h);
            if right == h.left {
                return false;
            }
            let mut rows: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
            for c in group.iter().filter(|c| {
                c.top > h.bottom
                    && c.left >= h.left
                    && c.right <= right
                    && c.nonempty()
                    && c.text().chars().any(char::is_alphanumeric)
            }) {
                rows.entry(c.top).or_default().push(c.left);
            }
            let starts: BTreeSet<u32> = rows.values().flatten().copied().collect();
            rows.len() >= 2 && starts.len() >= 2 && rows.values().all(|xs| xs.len() == 1)
        })
}

/// Ways to read names as a hierarchy: the whole width as one tree, names
/// under one heading with fields beside them, and a tree with notes on the
/// right.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Path {
    Whole,
    Spanned,
    Noted,
}

const PATHS: [Path; 3] = [Path::Whole, Path::Spanned, Path::Noted];

/// A reading of the names from column `from` on, with the columns on its
/// left labelling the rows.
struct Reading<'c, 'a> {
    from: u32,
    path: Path,
    names: Vec<&'c Cell<'a>>,
    /// The columns of the names when fields stand under headings beside them.
    span: Option<(u32, u32)>,
    /// The column of the No of the rows.
    key: Option<u32>,
    /// The columns grouping the rows.
    groups: Vec<Vec<&'c Cell<'a>>>,
    notes: HashSet<(u32, u32)>,
    tree: Tree,
}

impl Reading<'_, '_> {
    /// How the reading reads the names, with `peeled` columns on the left
    /// labelling the rows.
    fn describe(&self, peeled: usize) -> String {
        let way = match self.path {
            Path::Whole => "one tree over the whole width",
            Path::Spanned => "names under one heading with fields under the headings beside them",
            Path::Noted => "a tree with notes on its right",
        };
        match peeled {
            0 => way.to_owned(),
            1 => format!("{way}, the first column labelling the rows"),
            n => format!("{way}, the first {n} columns labelling the rows"),
        }
    }
}

/// How well a reading fits, the best least: the cells it cuts off from a
/// heading of their own, the names it spreads over headings of several
/// depths each, the columns it sets apart on the left, and the order of its
/// way to read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Score {
    cut_off: usize,
    spread: usize,
    peeled: usize,
    path: usize,
}

/// The best of the scored readings, and another that reads the cells
/// differently yet fits as well but for the order of the ways to read.
fn choose<T: PartialEq>(scored: &[(T, Score)]) -> Option<(usize, Option<usize>)> {
    let mut order: Vec<usize> = (0..scored.len()).collect();
    order.sort_by_key(|&k| scored[k].1);
    let best = *order.first()?;
    let tie =
        |a: Score, b: Score| (a.cut_off, a.spread, a.peeled) == (b.cut_off, b.spread, b.peeled);
    let competing = order
        .iter()
        .skip(1)
        .copied()
        .find(|&k| tie(scored[best].1, scored[k].1) && scored[k].0 != scored[best].0);
    Some((best, competing))
}

/// The roles and header links of a hierarchy, and another reading that fits
/// as well, when there is one.
pub(super) struct Outline {
    pub(super) cells: Vec<Value>,
    pub(super) competing: Option<String>,
}

/// The columns of `cells`, on the left of the names of `tree`, that label
/// its rows: at most one No of the rows, one column of numbers that do not
/// repeat, and the columns that group them (see [`grouping`]); and the other
/// columns.
#[allow(clippy::type_complexity)]
fn labels<'c, 'a>(
    cells: &[&'c Cell<'a>],
    names: &[&Cell<'_>],
    tree: &Tree,
) -> (Option<u32>, Vec<Vec<&'c Cell<'a>>>, Vec<Vec<&'c Cell<'a>>>) {
    let columns: BTreeSet<u32> = cells.iter().map(|c| c.left).collect();
    let mut key = None;
    let mut groups = vec![];
    let mut others = vec![];
    for x in columns {
        let column: Vec<&Cell<'_>> = cells.iter().copied().filter(|c| c.left == x).collect();
        let texts: HashSet<String> = column
            .iter()
            .map(|c| c.source["value"].to_string())
            .collect();
        let numbers = column
            .iter()
            .all(|c| c.bottom == c.top && numbering(c) == Some(Numbering::Parts(1)))
            && texts.len() == column.len();
        if numbers && key.is_none() {
            key = Some(x);
        } else if grouping(&column, names, tree) {
            groups.push(column);
        } else {
            others.push(column);
        }
    }
    (key, groups, others)
}

/// The label of row `y` in a column of labels: the one merged down over it
/// or the nearest written above it.
fn label<'c, 'a>(column: &[&'c Cell<'a>], y: u32) -> Option<&'c Cell<'a>> {
    column
        .iter()
        .copied()
        .filter(|c| c.top <= y)
        .max_by_key(|c| c.top)
}

/// Whether a column on the left of a tree groups its rows, as the machine a
/// folder tree is on: a label for every name's row, merged down over the
/// rows, written on each of them or on the first only, and each group of
/// rows under one label holds whole branches, every name with its
/// ancestors. A label on every row, such as a kind of each name, that
/// changes within a branch describes the names rather than groups them.
fn grouping(column: &[&Cell<'_>], names: &[&Cell<'_>], tree: &Tree) -> bool {
    let text = |y: u32| label(column, y).map(|c| c.text().trim());
    let Some(first) = names.iter().map(|c| c.top).min() else {
        return false;
    };
    let last = names.iter().map(|c| c.top).max().unwrap_or(first);
    if text(first).is_none() {
        return false;
    }
    // Each run of rows under the same label, by row.
    let mut runs: HashMap<u32, usize> = HashMap::new();
    let mut run = 0;
    for y in first..=last {
        if y > first && text(y) != text(y - 1) {
            run += 1;
        }
        runs.insert(y, run);
    }
    names.iter().enumerate().all(|(k, c)| {
        tree.ancestors[k]
            .iter()
            .all(|&p| runs[&names[p].top] == runs[&c.top])
    })
}

/// The tree of names, as indices into the names: each name's ancestors,
/// nearest last, and the names with children.
struct Tree {
    ancestors: Vec<Vec<usize>>,
    parents: HashSet<usize>,
}

/// The tree of `names`, sorted by row and column, or `None` when they are the
/// fields of records rather than a hierarchy. A name's parent is the nearest
/// earlier name on a shallower level, on its row or above: a column further
/// left, a smaller indent or a higher item number. Besides its number, a name
/// shares its row only with ancestors: merged down over it, in a column that
/// names only the first of its descendants' rows, left of the column that
/// names every row, or stepping right one column at a time under an ancestor
/// merged down over the row or on fewer than half of the rows, as a folder
/// beside its only file. `spread` names or more must have descendants on
/// later rows: one for a tree alone, two for a tree beside fields, where a
/// column of names moved once, as when the column widths change partway down
/// a table, is no hierarchy. A value written beside its name,
/// as a description beside its item, heads no line indented under it: the
/// line goes on with the item, as a note does.
fn tree(names: &[&Cell<'_>], spread: usize) -> Option<Tree> {
    if names.len() < 3 {
        return None;
    }
    let rows: BTreeSet<u32> = names.iter().map(|c| c.top).collect();
    let starting = |x: u32| names.iter().filter(move |c| c.left == x);
    let columns: BTreeSet<u32> = names.iter().map(|c| c.left).collect();
    let numbered = numbered_column(names);
    // Merged down over names that start on its later rows, as an ancestor is
    // over its descendants, on the right of every cell merged down over the
    // same rows beside it, as a folder and its only subfolder over a file;
    // records written two rows high are not, the values of their later rows
    // standing among their cells merged down side by side.
    let over = |c: &Cell<'_>| {
        let edge = names
            .iter()
            .filter(|m| m.top == c.top && m.bottom == c.bottom && m.left >= c.left)
            .map(|m| m.right)
            .max()
            .unwrap_or(c.right);
        c.bottom > c.top
            && names
                .iter()
                .any(|o| o.top > c.top && o.top <= c.bottom && o.left > edge)
    };
    let full: BTreeSet<u32> = columns
        .iter()
        .copied()
        .filter(|&x| starting(x).map(|c| c.top).collect::<BTreeSet<_>>() == rows)
        .collect();
    let line = |y: u32| -> Vec<&&Cell<'_>> {
        names
            .iter()
            .filter(|c| c.top == y && !over(c) && Some(c.left) != numbered)
            .collect()
    };
    let stepping = |line: &[&&Cell<'_>]| {
        line.windows(2)
            .all(|pair| pair[0].right + 1 == pair[1].left)
    };
    let merged_over = |line: &[&&Cell<'_>], y: u32| {
        line.first().is_some_and(|first| {
            names
                .iter()
                .any(|m| over(m) && m.top < y && m.bottom >= y && m.right < first.left)
        })
    };
    // Names stepping right one column at a time are a path of names, as a
    // folder and its only file: under an ancestor merged down over the row,
    // or on fewer than half of the rows, where names beside each other on
    // most rows are the fields of records.
    let unmerged = rows
        .iter()
        .filter(|&&y| {
            let line = line(y);
            line.len() > 1 && stepping(&line) && !merged_over(&line, y)
        })
        .count();
    let records = rows.iter().any(|&y| {
        let line = line(y);
        let named = line.iter().filter(|c| full.contains(&c.left)).count();
        let after = line
            .iter()
            .filter(|c| full.contains(&c.left))
            .map(|c| c.left)
            .min();
        let path = stepping(&line) && (unmerged * 2 < rows.len() || merged_over(&line, y));
        named > 1
            || (named == 0 && line.len() > 1 && !path)
            || line
                .iter()
                .any(|c| !full.contains(&c.left) && after.is_some_and(|x| c.left > x))
    });
    if records {
        return None;
    }
    // An item number gives the level of every name on its row, and of the
    // rows after it until the next number: digits by their parts, other forms
    // by the order they first appear in, as a document nests `1`, `(1)`, `①`.
    let forms: Vec<Numbering> = numbered.map_or(vec![], |x| {
        let mut forms = vec![];
        for form in starting(x).filter_map(|c| numbering(c)) {
            if !forms.contains(&form) {
                forms.push(form);
            }
        }
        forms
    });
    let in_parts = forms.iter().all(|f| matches!(f, Numbering::Parts(_)));
    let mut depths: HashMap<u32, usize> = HashMap::new();
    let mut depth = 0;
    for &y in &rows {
        if let Some(x) = numbered
            && let Some(form) = starting(x).find(|c| c.top == y).and_then(|c| numbering(c))
        {
            depth = match form {
                Numbering::Parts(n) if in_parts => n,
                _ => forms.iter().position(|f| *f == form).unwrap_or(0) + 1,
            };
        }
        depths.insert(y, depth);
    }
    let level = |c: &Cell<'_>| (depths[&c.top], c.left, width(c));
    let beside = |p: &Cell<'_>| {
        names
            .iter()
            .any(|o| o.top == p.top && !over(o) && o.left < p.left && Some(o.left) != numbered)
    };
    // Deeper only by indent, under a value written beside its name.
    let note = |p: &Cell<'_>, c: &Cell<'_>| {
        beside(p) && (depths[&p.top], p.left) == (depths[&c.top], c.left)
    };
    let mut open: Vec<usize> = vec![];
    let mut ancestors: Vec<Vec<usize>> = vec![];
    let mut parents_of_later: HashSet<usize> = HashSet::new();
    let mut parents: HashSet<usize> = HashSet::new();
    for (k, c) in names.iter().enumerate() {
        while open
            .last()
            .is_some_and(|&p| level(names[p]) >= level(c) || note(names[p], c))
        {
            open.pop();
        }
        parents_of_later.extend(open.iter().copied().filter(|&p| names[p].top < c.top));
        parents.extend(open.last());
        ancestors.push(open.clone());
        open.push(k);
    }
    (parents_of_later.len() >= spread).then_some(Tree { ancestors, parents })
}

/// Roles and header links of a hierarchy, or `None` when the cells are not
/// one. A heading row set apart by bold text or fill heads the names. Marks
/// before the names (see [`marks`]) name nothing and take no part in the tree.
/// Sparse notes on the right of the names (see [`side_notes`]) belong to the
/// name on their row and its ancestors, as fields do.
/// When the whole width is not one tree, the names stepped under one heading
/// of a one-row header may be, with fields under the headings on its sides:
/// each field belongs to the name on its row and that name's ancestors.
/// On the left of the names stand at most a No of the rows and columns that
/// group them (see [`labels`]), whose labels head the names and fields of
/// their rows; the names are read as though those columns were not there.
pub(super) fn outline(
    cells: &[Cell<'_>],
    indices: &[usize],
    sheet_index: usize,
) -> Option<Outline> {
    let mut group: Vec<&Cell<'_>> = indices.iter().map(|&i| &cells[i]).collect();
    group.sort_by_key(|c| (c.top, c.left));
    let top = group.first()?.top;
    let heads: Vec<&Cell<'_>> = group
        .iter()
        .copied()
        .filter(|c| c.top == top && c.nonempty())
        .collect();
    let set_apart = !heads.is_empty()
        && heads.iter().all(|h| {
            h.string()
                && (h.bold() || h.fill() != 0)
                && group
                    .iter()
                    .filter(|c| c.top == h.bottom + 1 && c.left <= h.right && c.right >= h.left)
                    .all(|c| (c.bold(), c.fill()) != (h.bold(), h.fill()))
        });
    // A title and header rows as a table has them come first; otherwise one
    // row set apart, such as a heading only filled over a tree.
    let layout = layout(&group, None).ok()?;
    let titled = layout.header.is_some() && layout.caption;
    let title = |c: &Cell<'_>| titled && c.top == top;
    let (heads, body_top) = if let Some((_, last)) = layout.header {
        let heads = group
            .iter()
            .copied()
            .filter(|c| c.top <= last && c.nonempty() && !title(c))
            .collect();
        (heads, last + 1)
    } else if set_apart || plain_heading(&group, &heads) {
        let bottom = heads.iter().map(|h| h.bottom).max()?;
        (heads, bottom + 1)
    } else {
        (vec![], top)
    };
    let heading = |c: &Cell<'_>| c.top < body_top;
    let body: Vec<&Cell<'_>> = group
        .iter()
        .copied()
        .filter(|c| !heading(c) && c.nonempty())
        .collect();
    let named = unmarked(&body);
    if !(!heads.is_empty() || named.iter().all(|c| c.bordered())) {
        return None;
    }
    // The headings over a cell's columns in every header row above it, or
    // the nearest on its left.
    let heading_of = |c: &Cell<'_>| -> Vec<&Cell<'_>> {
        let over: Vec<&Cell<'_>> = heads
            .iter()
            .copied()
            .filter(|h| h.bottom < c.top && h.left <= c.left && h.right >= c.right)
            .collect();
        if !over.is_empty() {
            return over;
        }
        let nearest = heads
            .iter()
            .copied()
            .filter(|h| h.bottom < c.top && h.left <= c.left)
            .max_by_key(|h| (h.left, h.bottom));
        match (nearest, heads.as_slice()) {
            (Some(h), _) => vec![h],
            (None, [only]) if only.bottom < c.top => vec![*only],
            _ => vec![],
        }
    };
    // The reading of the names from column `from` on, the cells on its left
    // set apart: the names, the columns of the names and fields under their
    // headings, the No and the groups of the rows on the left, the notes and
    // the tree. It reads the whole width as one tree, or names under one
    // heading with fields beside them, or a tree with notes.
    let read = |from: u32, path: Path| {
        let body: Vec<&Cell<'_>> = body.iter().copied().filter(|c| c.left >= from).collect();
        let named = unmarked(&body);
        // The one column of the cells under each heading, if they share one.
        let sole: Vec<Option<u32>> = heads
            .iter()
            .map(|h| {
                let mut under = body
                    .iter()
                    .filter(|o| o.left >= h.left && o.right <= h.right)
                    .map(|o| o.left);
                let first = under.next()?;
                under.all(|x| x == first).then_some(first)
            })
            .collect();
        let notes = side_notes(&named, |c| {
            heads.iter().zip(&sole).any(|(h, &sole)| {
                h.bottom < c.top && h.left <= c.left && h.right >= c.right && sole == Some(c.left)
            })
        });
        let annotated: Vec<&Cell<'_>> = named
            .iter()
            .copied()
            .filter(|c| !notes.contains(&(c.top, c.left)))
            .collect();
        let noted = || (!notes.is_empty()).then(|| tree(&annotated, 1)).flatten();
        // Names stepped across columns or indents under one heading, with
        // fields under the headings on its sides. Under header rows with
        // subheadings, columns are grouped, not stepped.
        let spanned = || {
            if heads
                .iter()
                .any(|h| h.top != body_top - 1 || h.bottom != body_top - 1)
            {
                return None;
            }
            // A heading reaches over the columns up to the next heading,
            // merged over them or not, as a heading written at the start of
            // a band.
            let edge = group.iter().map(|c| c.right).max()?;
            let reach = |h: &Cell<'_>| {
                heads
                    .iter()
                    .filter(|o| o.left > h.right)
                    .map(|o| o.left - 1)
                    .min()
                    .unwrap_or(edge)
            };
            let within = |left: u32, right: u32| -> Vec<&Cell<'_>> {
                body.iter()
                    .copied()
                    .filter(|c| c.left >= left && c.right <= right)
                    .collect()
            };
            let steps = |cells: &[&Cell<'_>]| {
                let lefts: HashSet<u32> = cells.iter().map(|c| c.left).collect();
                let indents: HashSet<usize> = cells.iter().map(|c| width(c)).collect();
                lefts.len() >= 2 || indents.len() >= 2
            };
            let (left, right, names) = heads.iter().find_map(|h| {
                let mut right = reach(h);
                // A heading over item numbers in two forms or more and the
                // heading over the names they number make one band, when the
                // numbers alone give the levels.
                if numbered_column(&within(h.left, right)).is_some()
                    && let Some(next) = heads.iter().find(|o| o.left > right)
                    && !steps(&unmarked(&within(next.left, reach(next))))
                {
                    right = reach(next);
                }
                let under = unmarked(&within(h.left, right));
                steps(&under).then_some((h.left, right, under))
            })?;
            let fields_headed = body
                .iter()
                .filter(|c| c.right < left || c.left > right)
                .all(|c| {
                    let over = heading_of(c);
                    !over.is_empty() && over.iter().all(|h| h.right < left || h.left > right)
                });
            let tree = fields_headed.then(|| tree(&names, 2)).flatten()?;
            // On the left, the columns that label the rows (see [`labels`]);
            // the other columns hold fields, as those on the right do. Item
            // numbers in two forms or more are levels, left to the table
            // reading.
            let before: Vec<&Cell<'_>> = body.iter().copied().filter(|c| c.right < left).collect();
            let (key, groups, fields) = labels(&before, &names, &tree);
            // Lines indented under a value written beside its item go on
            // with the item, as notes do, rather than group under it.
            let stepped = names.iter().map(|c| c.left).collect::<HashSet<_>>().len() >= 2;
            if fields
                .iter()
                .any(|column| numbered_column(column).is_some())
                || (!stepped && !groups.is_empty())
            {
                return None;
            }
            Some((names, (left, right), key, groups, tree))
        };
        match path {
            Path::Whole => {
                let tree = tree(&named, 1)?;
                Some((named.clone(), None, None, vec![], HashSet::new(), tree))
            }
            Path::Spanned => {
                let (names, span, key, groups, tree) = spanned()?;
                Some((names, Some(span), key, groups, HashSet::new(), tree))
            }
            Path::Noted => {
                let tree = noted()?;
                Some((annotated, None, None, vec![], notes, tree))
            }
        }
    };
    // Columns on the left that label the rows rather than name anything, as
    // the machine a folder tree is on, are set apart before reading the
    // names, the fewest first, and must label the rows of the tree read.
    let starts: Vec<u32> = body
        .iter()
        .map(|c| c.left)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .take(3)
        .collect();
    let readings: Vec<Reading<'_, '_>> = starts
        .iter()
        .flat_map(|&from| PATHS.map(|path| (from, path)))
        .filter_map(|(from, path)| {
            if body.iter().any(|c| c.left < from && c.right >= from) {
                return None;
            }
            let (names, span, key, groups, notes, tree) = read(from, path)?;
            let before: Vec<&Cell<'_>> = body.iter().copied().filter(|c| c.right < from).collect();
            let (left_key, mut left_groups, fields) = labels(&before, &names, &tree);
            // Item numbers in two forms or more give the levels of the names
            // and label no rows.
            let numbered = left_groups
                .iter()
                .any(|column| numbered_column(column).is_some());
            if !fields.is_empty() || numbered || (key.is_some() && left_key.is_some()) {
                return None;
            }
            left_groups.extend(groups);
            Some(Reading {
                from,
                path,
                names,
                span,
                key: key.or(left_key),
                groups: left_groups,
                notes,
                tree,
            })
        })
        .collect();
    let id_for = |c: &Cell<'_>| {
        format!(
            "s{}-{}",
            sheet_index + 1,
            c.source["address"].as_str().unwrap_or("")
        )
    };
    let assemble = |reading: &Reading<'_, '_>| -> Vec<Value> {
        let Reading {
            names,
            span,
            key,
            groups,
            notes,
            tree,
            ..
        } = reading;
        let (span, key) = (*span, *key);
        let index: HashMap<(u32, u32), usize> = names
            .iter()
            .enumerate()
            .map(|(k, c)| ((c.top, c.left), k))
            .collect();
        // The deepest name on a row, merged down over it or starting there.
        // A row with no name of its own goes on with the record above it, as
        // a second line of values beside a folder drawn over two rows.
        let mut owners_by_row: BTreeMap<u32, usize> = BTreeMap::new();
        for (k, c) in names.iter().enumerate() {
            for y in c.top..=c.bottom {
                let deeper = owners_by_row
                    .get(&y)
                    .is_none_or(|&o| tree.ancestors[k].len() >= tree.ancestors[o].len());
                if deeper {
                    owners_by_row.insert(y, k);
                }
            }
        }
        let row_name =
            |y: u32| -> Option<usize> { owners_by_row.range(..=y).next_back().map(|(_, &k)| k) };
        let field = |c: &Cell<'_>| {
            span.is_some_and(|(left, right)| c.right < left || c.left > right)
                || notes.contains(&(c.top, c.left))
        };
        // The No of a row labels the row, as a column of row labels does, but
        // names nothing that the row's fields belong to.
        let keyed = |c: &Cell<'_>| c.nonempty() && !heading(c) && key == Some(c.left);
        // A column grouping the rows labels them as an ancestor merged down over
        // them does, by the label nearest above each row.
        let grouped = |c: &Cell<'_>| {
            groups
                .iter()
                .any(|column| column.iter().any(|g| (g.top, g.left) == (c.top, c.left)))
        };
        let labels = |y: u32| -> Vec<String> {
            groups
                .iter()
                .filter_map(|column| label(column, y))
                .map(&id_for)
                .collect()
        };
        // Names with a field or a note on their row head it, as names with
        // children head theirs.
        let owners: HashSet<usize> = group
            .iter()
            .filter(|c| c.nonempty() && field(c) && !keyed(c) && !grouped(c))
            .filter_map(|c| row_name(c.top))
            .collect();
        group
            .iter()
            .map(|c| {
                let headed: Vec<String> = heading_of(c).into_iter().map(&id_for).collect();
                let (role, headers): (&str, Vec<String>) = if title(c) {
                    ("text", vec![])
                } else if heading(c) {
                    ("column_header", headed)
                } else if keyed(c) || grouped(c) {
                    ("row_header", headed)
                } else if field(c) && c.nonempty() {
                    let owner = row_name(c.top);
                    let lineage = owner
                        .into_iter()
                        .flat_map(|k| tree.ancestors[k].iter().copied().chain([k]));
                    // A note stands under the heading over its column, if any,
                    // not the nearest one on its left.
                    let headed = if span.is_some() {
                        headed
                    } else {
                        heads
                            .iter()
                            .filter(|h| h.bottom < c.top && h.left <= c.left && h.right >= c.right)
                            .map(|h| id_for(h))
                            .collect()
                    };
                    let headers = headed
                        .into_iter()
                        .chain(labels(c.top))
                        .chain(lineage.map(|p| id_for(names[p])))
                        .collect();
                    ("data", headers)
                } else if let Some(&k) = index.get(&(c.top, c.left)) {
                    let role = if tree.parents.contains(&k) || owners.contains(&k) {
                        "row_header"
                    } else {
                        "data"
                    };
                    let headers = headed
                        .into_iter()
                        .chain(labels(c.top))
                        .chain(tree.ancestors[k].iter().map(|&p| id_for(names[p])))
                        .collect();
                    (role, headers)
                } else if c.nonempty() {
                    ("text", vec![])
                } else {
                    ("data", headed.into_iter().collect())
                };
                json!({"id":id_for(c),"address":c.source["address"],"role":role,"headers":headers,
                    "text_state":if c.nonempty() || c.source["formula"].is_string() {"read"} else {"empty"}})
            })
            .collect()
    };
    // Each reading is scored (see [`Score`]): a heading of its own that heads
    // no name cuts a cell off when the cell does not link to it, as machine
    // names read as marks or notes read apart from the heading over them;
    // names spread over headings of several depths each, as kinds in a column
    // of their own read as the deepest names, where headings of the levels
    // each head one depth.
    let scored: Vec<(Vec<Value>, Score)> = readings
        .iter()
        .map(|reading| {
            let interpreted = assemble(reading);
            let naming: HashSet<String> = reading
                .names
                .iter()
                .flat_map(|c| heading_of(c))
                .map(&id_for)
                .collect();
            let headers: HashMap<&str, HashSet<&str>> = interpreted
                .iter()
                .map(|c| {
                    let headers = c["headers"].as_array().map_or(HashSet::new(), |h| {
                        h.iter().filter_map(Value::as_str).collect()
                    });
                    (c["id"].as_str().unwrap_or(""), headers)
                })
                .collect();
            let cut_off = body
                .iter()
                .filter(|c| {
                    let own: Vec<String> = heading_of(c).into_iter().map(&id_for).collect();
                    !own.is_empty()
                        && own.iter().all(|h| !naming.contains(h))
                        && headers
                            .get(id_for(c).as_str())
                            .is_none_or(|linked| own.iter().all(|h| !linked.contains(h.as_str())))
                })
                .count();
            let over: Vec<String> = reading
                .names
                .iter()
                .map(|c| {
                    heading_of(c)
                        .into_iter()
                        .map(&id_for)
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .collect();
            let mut depths: HashMap<&str, HashSet<usize>> = HashMap::new();
            for (k, heading) in over.iter().enumerate() {
                depths
                    .entry(heading)
                    .or_default()
                    .insert(reading.tree.ancestors[k].len());
            }
            let spread = if depths.len() >= 2 {
                over.iter()
                    .filter(|heading| depths[heading.as_str()].len() >= 2)
                    .count()
            } else {
                0
            };
            let peeled = starts.iter().position(|&x| x == reading.from).unwrap_or(0);
            let path = PATHS.iter().position(|&p| p == reading.path).unwrap_or(0);
            let score = Score {
                cut_off,
                spread,
                peeled,
                path,
            };
            (interpreted, score)
        })
        .collect();
    let (best, competing) = choose(&scored)?;
    let competing = competing.map(|k| {
        let differing = scored[best]
            .0
            .iter()
            .zip(&scored[k].0)
            .filter(|(a, b)| a != b)
            .count();
        format!(
            "Another reading also fits, as {}, and reads {differing} cells differently; confirm which one the source means.",
            readings[k].describe(scored[k].1.peeled)
        )
    });
    Some(Outline {
        cells: scored.into_iter().nth(best)?.0,
        competing,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn score(cut_off: usize, spread: usize, peeled: usize, path: usize) -> Score {
        Score {
            cut_off,
            spread,
            peeled,
            path,
        }
    }

    #[test]
    fn the_reading_that_fits_best_wins_whatever_its_order() {
        let scored = [
            ("whole", score(1, 0, 0, 0)),
            ("spanned", score(0, 2, 0, 1)),
            ("peeled", score(0, 0, 1, 1)),
        ];
        assert_eq!(choose(&scored), Some((2, None)));
    }

    #[test]
    fn readings_that_tie_but_for_their_order_compete_when_they_differ() {
        let scored = [("tree", score(0, 0, 0, 1)), ("fields", score(0, 0, 0, 0))];
        assert_eq!(choose(&scored), Some((1, Some(0))));
        let same = [("tree", score(0, 0, 0, 1)), ("tree", score(0, 0, 0, 0))];
        assert_eq!(choose(&same), Some((1, None)));
        let worse = [("tree", score(0, 0, 1, 0)), ("fields", score(0, 0, 0, 1))];
        assert_eq!(choose(&worse), Some((1, None)));
        assert_eq!(choose::<&str>(&[]), None);
    }
}
