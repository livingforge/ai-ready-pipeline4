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
/// Numbers and ancestors merged down over their descendants are never marks,
/// nor a name that stands so once, as an ancestor beside its only child.
fn marks(cells: &[&Cell<'_>]) -> HashSet<(u32, u32)> {
    let numbered = numbered_column(cells);
    let candidate = |c: &Cell<'_>| c.string() && c.bottom == c.top && Some(c.left) != numbered;
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
/// the fields of records instead, and then there are no notes.
fn side_notes(cells: &[&Cell<'_>]) -> HashSet<(u32, u32)> {
    let mut rows: BTreeMap<u32, Vec<&Cell<'_>>> = BTreeMap::new();
    for &c in cells {
        rows.entry(c.top).or_default().push(c);
    }
    let mut reach = 0;
    let mut ends = vec![];
    for line in rows.values_mut() {
        line.sort_by_key(|c| c.left);
        let rest = match line.as_slice() {
            [rest @ .., end] if !rest.is_empty() => {
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
/// merged down over the row. `spread` names or more must have descendants on
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
    // over its descendants; records written two rows high are not.
    let over =
        |c: &Cell<'_>| c.bottom > c.top && names.iter().any(|o| o.top > c.top && o.top <= c.bottom);
    let full: BTreeSet<u32> = columns
        .iter()
        .copied()
        .filter(|&x| starting(x).map(|c| c.top).collect::<BTreeSet<_>>() == rows)
        .collect();
    let records = rows.iter().any(|&y| {
        let line: Vec<_> = names
            .iter()
            .filter(|c| c.top == y && !over(c) && Some(c.left) != numbered)
            .collect();
        let named = line.iter().filter(|c| full.contains(&c.left)).count();
        let after = line
            .iter()
            .filter(|c| full.contains(&c.left))
            .map(|c| c.left)
            .min();
        // Names stepping right one column at a time, under an ancestor merged
        // down over the row, are a path of names, as a folder and its only file.
        let path = line
            .windows(2)
            .all(|pair| pair[0].right + 1 == pair[1].left)
            && line.first().is_some_and(|first| {
                names
                    .iter()
                    .any(|m| over(m) && m.top < y && m.bottom >= y && m.right < first.left)
            });
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
/// of a one-row header may be, with fields under the headings on its right:
/// each field belongs to the name on its row and that name's ancestors.
/// On the left of the names stands at most a No of the rows.
pub(super) fn outline(
    cells: &[Cell<'_>],
    indices: &[usize],
    sheet_index: usize,
) -> Option<Vec<Value>> {
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
    } else if set_apart {
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
    let notes = side_notes(&named);
    let annotated: Vec<&Cell<'_>> = named
        .iter()
        .copied()
        .filter(|c| !notes.contains(&(c.top, c.left)))
        .collect();
    let noted = || (!notes.is_empty()).then(|| tree(&annotated, 1)).flatten();
    // Names stepped across columns or indents under one heading, with fields
    // under the headings on its right. Under header rows with subheadings,
    // columns are grouped, not stepped.
    let spanned = || {
        if heads
            .iter()
            .any(|h| h.top != body_top - 1 || h.bottom != body_top - 1)
        {
            return None;
        }
        let (left, right, names) = heads.iter().find_map(|h| {
            let under: Vec<&Cell<'_>> = body
                .iter()
                .copied()
                .filter(|c| c.left >= h.left && c.right <= h.right)
                .collect();
            let under = unmarked(&under);
            let lefts: HashSet<u32> = under.iter().map(|c| c.left).collect();
            let indents: HashSet<usize> = under.iter().map(|c| width(c)).collect();
            (lefts.len() >= 2 || indents.len() >= 2).then_some((h.left, h.right, under))
        })?;
        let fields_headed = body
            .iter()
            .filter(|c| c.right < left || c.left > right)
            .all(|c| {
                let over = heading_of(c);
                !over.is_empty() && over.iter().all(|h| h.right < left || h.left > right)
            });
        // On the left, only a No of the rows: one column of numbers that do
        // not repeat. Item numbers in parts are levels, left to the table
        // reading.
        let before: Vec<&Cell<'_>> = body.iter().copied().filter(|c| c.right < left).collect();
        let key = before.first().map(|c| c.left);
        let texts: HashSet<String> = before
            .iter()
            .map(|c| c.source["value"].to_string())
            .collect();
        let keyed = before.iter().all(|c| {
            Some(c.left) == key && c.bottom == c.top && numbering(c) == Some(Numbering::Parts(1))
        }) && texts.len() == before.len();
        let tree = (fields_headed && keyed)
            .then(|| tree(&names, 2))
            .flatten()?;
        Some((names, (left, right), key, tree))
    };
    let (names, span, key, notes, tree) = if let Some(tree) = tree(&named, 1) {
        (named.clone(), None, None, HashSet::new(), tree)
    } else if let Some((names, span, key, tree)) = spanned() {
        (names, Some(span), key, HashSet::new(), tree)
    } else {
        let tree = noted()?;
        (annotated, None, None, notes, tree)
    };
    let id_for = |c: &Cell<'_>| {
        format!(
            "s{}-{}",
            sheet_index + 1,
            c.source["address"].as_str().unwrap_or("")
        )
    };
    let index: HashMap<(u32, u32), usize> = names
        .iter()
        .enumerate()
        .map(|(k, c)| ((c.top, c.left), k))
        .collect();
    // The deepest name on a row, merged down over it or starting there.
    let row_name = |y: u32| -> Option<usize> {
        names
            .iter()
            .enumerate()
            .filter(|(_, c)| c.top <= y && c.bottom >= y)
            .map(|(k, _)| k)
            .max_by_key(|&k| tree.ancestors[k].len())
    };
    let field = |c: &Cell<'_>| {
        span.is_some_and(|(left, right)| c.right < left || c.left > right)
            || notes.contains(&(c.top, c.left))
    };
    // The No of a row labels the row, as a column of row labels does, but
    // names nothing that the row's fields belong to.
    let keyed = |c: &Cell<'_>| {
        c.nonempty() && key == Some(c.left) && span.is_some_and(|(l, _)| c.right < l)
    };
    // Names with a note on their row head it.
    let annotated: HashSet<usize> = notes.iter().filter_map(|&(y, _)| row_name(y)).collect();
    let interpreted = group
        .iter()
        .map(|c| {
            let headed: Vec<String> = heading_of(c).into_iter().map(&id_for).collect();
            let (role, headers): (&str, Vec<String>) = if title(c) {
                ("text", vec![])
            } else if heading(c) {
                ("column_header", headed)
            } else if keyed(c) {
                ("row_header", headed)
            } else if field(c) && c.nonempty() {
                let owner = row_name(c.top);
                let lineage = owner
                    .into_iter()
                    .flat_map(|k| tree.ancestors[k].iter().copied().chain([k]));
                // A note stands under no heading of its own.
                let headed = if span.is_some() { headed } else { vec![] };
                let headers = headed
                    .into_iter()
                    .chain(lineage.map(|p| id_for(names[p])))
                    .collect();
                ("data", headers)
            } else if let Some(&k) = index.get(&(c.top, c.left)) {
                let role = if span.is_some() || tree.parents.contains(&k) || annotated.contains(&k)
                {
                    "row_header"
                } else {
                    "data"
                };
                let headers = headed
                    .into_iter()
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
        .collect();
    Some(interpreted)
}
