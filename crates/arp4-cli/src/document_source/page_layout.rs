//! The rows, columns and tables of a page, read from where it shows its
//! strings and rulings: a PDF page, and a picture whose words and lines
//! were read from its pixels.
//!
//! A page draws strings in the order its writer chose, a line often in
//! pieces and a table cell by cell or column by column. The strings are put
//! back together here: those along one line whose ends meet are one text,
//! the rulings that cross one another are a table whose cells hold the text
//! drawn inside them, text set in columns that share their lines is a table
//! without rulings, and columns that do not are read one after another. A
//! cell names the strings it is made of, so that an edit of its text is
//! written to the strings it changes.
//!
//! Where a page says which of its strings are one paragraph (as a tagged
//! PDF does), they are one text however many lines they take, their lines
//! joined as running text is; and a table it names by its rows and cells is
//! that table, wherever its rulings are.
use super::PdfPath;
use anyhow::{Result, ensure};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// A string as the page shows it: where it starts (`x`, and `y` of its
/// baseline, in points from the page's top left), how far it runs, its
/// size, whether it is written left to right along a level line, and
/// whether the page draws it invisibly, as the text read from a scanned
/// page is laid over its image. `unit` names the paragraph the page says
/// the string is of, if it says.
pub(crate) struct Shown<'a> {
    pub text: &'a str,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub size: f64,
    pub upright: bool,
    pub invisible: bool,
    pub unit: Option<usize>,
}

/// A table a page names by its rows and their cells, with the header rows
/// it names (none when it names none).
pub(crate) struct Marked {
    pub rows: Vec<Vec<MarkedCell>>,
    pub header_rows: usize,
}

/// A cell of a [`Marked`] table: the units of its text, and the columns and
/// rows it spans.
pub(crate) struct MarkedCell {
    pub units: Vec<usize>,
    pub columns: usize,
    pub rows: usize,
}

/// A string of a cell's text, by its index among the page's strings, and
/// what the cell's text has before it: nothing, a space where the page
/// leaves a gap, or a line break where it starts a line.
#[derive(Clone, Debug, PartialEq)]
pub struct Piece {
    pub string: usize,
    pub before: &'static str,
}

/// A cell of the page's sheet, at its 1-based column and row.
#[derive(Clone, Debug, PartialEq)]
pub struct Cell {
    pub column: u32,
    pub row: u32,
    pub pieces: Vec<Piece>,
}

/// The sheet of a page: its cells by row and column, the ranges its merged
/// cells span and its tables as an extraction records them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Layout {
    pub cells: Vec<Cell>,
    pub merges: Vec<String>,
    pub tables: Vec<Value>,
}

/// The widest gap, in sizes of the text, between strings of one text.
const TEXT_GAP: f64 = 1.8;
/// The gap, in sizes of the text, from which strings are a space apart.
const SPACE_GAP: f64 = 0.18;
/// That gap where one of the strings is written without spaces: programs
/// set Japanese a quarter of its size apart from the letters and digits
/// beside it, which is no space.
const MIXED_SPACE_GAP: f64 = 0.6;
/// How far apart rulings and their ends may be and still meet.
const MEET: f64 = 3.0;
/// The thickest line that is a ruling rather than an area.
const THICK: f64 = 3.0;
/// The most rulings a page's tables are looked for among.
const MAX_RULES: usize = 20_000;
/// The most columns a table may have, as a sheet has.
const MAX_COLUMNS: usize = 16_000;

/// The text `pieces` make of the strings `text` gives.
pub fn composed<'a>(pieces: &[Piece], text: impl Fn(usize) -> &'a str) -> String {
    let mut out = String::new();
    for piece in pieces {
        out.push_str(piece.before);
        out.push_str(text(piece.string));
    }
    out
}

/// The new text of each string that an edit of a cell to `after` changes.
/// What the edit replaces is found as the characters between the longest
/// unchanged start and end; the first string it touches takes the new
/// characters, as typing over a selection does. The spaces and line breaks
/// between strings come from where the page places them, so an edit that
/// would change them is refused.
pub fn distribute<'a>(
    pieces: &[Piece],
    text: impl Fn(usize) -> &'a str,
    after: &str,
) -> Result<BTreeMap<usize, String>> {
    let before = composed(pieces, &text);
    let (start, end) = super::word::changed_range(&before, after, true);
    let inserted = &after[start..after.len() - (before.len() - end)];
    let mut changes = BTreeMap::new();
    if before == after {
        return Ok(changes);
    }
    let mut offset = 0;
    let mut taken = false;
    let mut texts: Vec<String> = vec![];
    for piece in pieces {
        offset += piece.before.len();
        let own = text(piece.string);
        let (from, to) = (offset, offset + own.len());
        offset = to;
        let touched = if start == end {
            !taken && from <= start && start <= to
        } else {
            from < end && start < to
        };
        if !touched {
            texts.push(own.to_owned());
            continue;
        }
        let mut new = String::new();
        if !taken {
            new.push_str(&own[..start.max(from) - from]);
            new.push_str(inserted);
            taken = true;
        }
        new.push_str(&own[end.min(to).max(from) - from..]);
        if new != own {
            changes.insert(piece.string, new.clone());
        }
        texts.push(new);
    }
    let mut written = String::new();
    for (piece, text) in pieces.iter().zip(&texts) {
        written.push_str(piece.before);
        written.push_str(text);
    }
    ensure!(
        written == after,
        "the edit changes a space or line break that is not a character of the PDF but where the page places its strings ({before:?} is drawn as {} strings); keep the spaces and line breaks between them, or edit it in a PDF editor",
        pieces.len()
    );
    Ok(changes)
}

/// A string that shows characters, by its index, with its extent along its
/// line.
#[derive(Clone, Copy, Debug)]
struct Word {
    string: usize,
    x0: f64,
    x1: f64,
    y: f64,
    size: f64,
    upright: bool,
}

/// Strings that are one text: those of one line, or of the lines of a
/// paragraph. `y` is its first baseline and `last` its last.
#[derive(Clone, Debug)]
struct Fragment {
    pieces: Vec<Piece>,
    x0: f64,
    x1: f64,
    y: f64,
    last: f64,
    size: f64,
}

impl Fragment {
    fn top(&self) -> f64 {
        self.y - 0.8 * self.size
    }
    fn bottom(&self) -> f64 {
        self.last + 0.2 * self.size
    }
}

/// Whether `c` is written without spaces between words, as Japanese and
/// Chinese are.
pub(crate) fn unspaced(c: char) -> bool {
    matches!(u32::from(c),
        0x2E80..=0x9FFF | 0xF900..=0xFAFF | 0xFE30..=0xFE4F | 0xFF00..=0xFFEF | 0x20000..=0x3FFFF)
}

/// What stands between two strings of a line `gap` apart.
fn separator(previous: &str, next: &str, gap: f64, size: f64) -> &'static str {
    let (Some(last), Some(first)) = (previous.chars().next_back(), next.chars().next()) else {
        return "";
    };
    let least = match (unspaced(last), unspaced(first)) {
        (true, true) => return "",
        (false, false) => SPACE_GAP,
        _ => MIXED_SPACE_GAP,
    };
    if gap <= least * size || last.is_whitespace() || first.is_whitespace() {
        ""
    } else {
        " "
    }
}

/// `words` as lines, top to bottom, each left to right: strings whose
/// baselines are within a third of their size are of one line. A string
/// that is turned is a line of its own.
fn lines(mut words: Vec<Word>) -> Vec<Vec<Word>> {
    words.sort_by(|a, b| a.y.total_cmp(&b.y).then(a.x0.total_cmp(&b.x0)));
    let mut out: Vec<Vec<Word>> = vec![];
    for word in words {
        match out.last_mut() {
            Some(line)
                if word.upright
                    && line[0].upright
                    && word.y - line[0].y <= 0.3 * word.size.min(line[0].size).max(1.0) =>
            {
                line.push(word);
            }
            _ => out.push(vec![word]),
        }
    }
    for line in &mut out {
        line.sort_by(|a, b| a.x0.total_cmp(&b.x0));
    }
    out
}

/// The texts of a `line`: one for each run of strings that are all shown or
/// all invisible, and when `split` no further apart than [`TEXT_GAP`], so
/// that the text laid invisibly over what a page shows is read apart from
/// it. The strings of only spaces
/// that a text starts or ends with are not of it, as a program writes one
/// after a paragraph; a text of nothing else is none.
fn fragments(line: &[Word], shown: &[Shown<'_>], split: bool) -> Vec<Fragment> {
    let blank = |piece: &Piece| shown[piece.string].text.trim().is_empty();
    let mut out = spaced_fragments(line, shown, split);
    for fragment in &mut out {
        let keep = fragment.pieces.iter().position(|p| !blank(p));
        fragment
            .pieces
            .drain(..keep.unwrap_or(fragment.pieces.len()));
        while fragment.pieces.last().is_some_and(blank) {
            fragment.pieces.pop();
        }
        if let Some(first) = fragment.pieces.first_mut() {
            first.before = "";
        }
    }
    out.retain(|fragment| !fragment.pieces.is_empty());
    out
}

/// [`fragments`] with the strings of only spaces at their ends.
fn spaced_fragments(line: &[Word], shown: &[Shown<'_>], split: bool) -> Vec<Fragment> {
    let mut out: Vec<Fragment> = vec![];
    let mut previous: Option<&Word> = None;
    for word in line {
        let text = shown[word.string].text;
        match (out.last_mut(), previous) {
            (Some(fragment), Some(last))
                if shown[word.string].invisible == shown[last.string].invisible
                    && (!split || word.x0 - fragment.x1 <= TEXT_GAP * word.size.max(last.size)) =>
            {
                fragment.pieces.push(Piece {
                    string: word.string,
                    before: separator(
                        shown[last.string].text,
                        text,
                        word.x0 - fragment.x1,
                        word.size.max(last.size),
                    ),
                });
                fragment.x1 = fragment.x1.max(word.x1);
                fragment.size = fragment.size.max(word.size);
            }
            _ => out.push(Fragment {
                pieces: vec![Piece {
                    string: word.string,
                    before: "",
                }],
                x0: word.x0,
                x1: word.x1,
                y: word.y,
                last: word.y,
                size: word.size,
            }),
        }
        previous = Some(word);
    }
    out
}

/// The pieces of the `texts` of one cell: top to bottom, a line break
/// between them, and a space between texts of one line.
fn cell_pieces(mut texts: Vec<Fragment>) -> Vec<Piece> {
    texts.sort_by(|a, b| a.y.total_cmp(&b.y).then(a.x0.total_cmp(&b.x0)));
    let mut pieces: Vec<Piece> = vec![];
    let mut previous: Option<(f64, f64, f64)> = None;
    for text in texts {
        let first = pieces.len();
        pieces.extend(text.pieces);
        if let Some((y, last, size)) = previous {
            let beside = y == last && text.y - y <= 0.3 * size.min(text.size);
            pieces[first].before = if beside { " " } else { "\n" };
        }
        previous = Some((text.y, text.last, text.size));
    }
    pieces
}

/// The texts of the lines `words` make, each line one text.
fn line_texts(words: Vec<Word>, shown: &[Shown<'_>]) -> Vec<Fragment> {
    lines(words)
        .iter()
        .flat_map(|line| fragments(line, shown, false))
        .collect()
}

/// The one text of the `lines` of a paragraph (at least one), which may
/// run as far as `right`, or as far as its longest line when none is given.
/// A line that the first word of the next would not have fitted on was
/// wrapped: the two are joined as running text is, with a space where one
/// ends and the next begins in letters written with spaces, and nothing
/// otherwise. A line with room for it was ended: a line break follows it.
/// Lines of a character each are text written downwards.
fn paragraph(lines: Vec<Fragment>, shown: &[Shown<'_>], right: Option<f64>) -> Fragment {
    let right = right.unwrap_or_else(|| lines.iter().map(|l| l.x1).fold(f64::MIN, f64::max));
    let text = |line: &Fragment| composed(&line.pieces, |i| shown[i].text);
    let down = lines.len() > 1 && lines.iter().all(|line| text(line).chars().count() == 1);
    let mut lines = lines.into_iter();
    let mut whole = lines.next().expect("a line of the paragraph");
    // How far the last line joined runs, and its last character.
    let mut reach = whole.x1;
    for line in lines {
        let ends = whole
            .pieces
            .last()
            .and_then(|piece| shown[piece.string].text.chars().next_back());
        let next = text(&line);
        let begins = next.chars().next();
        // The first word of the line: a character of what is written
        // without spaces, else up to the first space.
        let word = match begins {
            Some(c) if unspaced(c) => 1,
            _ => next.chars().take_while(|c| !c.is_whitespace()).count(),
        };
        let wide = (line.x1 - line.x0) * word as f64 / next.chars().count().max(1) as f64;
        let wrapped = down || reach + wide > right - 0.5 * line.size;
        let apart = ends.zip(begins).is_some_and(|(a, b)| {
            !a.is_whitespace()
                && !b.is_whitespace()
                && !matches!(a, '-' | '_' | '/')
                && !unspaced(a)
                && !unspaced(b)
        });
        let first = whole.pieces.len();
        whole.pieces.extend(line.pieces);
        whole.pieces[first].before = match (wrapped, apart && !down) {
            (false, _) => "\n",
            (true, true) => " ",
            (true, false) => "",
        };
        reach = line.x1;
        whole.x0 = whole.x0.min(line.x0);
        whole.x1 = whole.x1.max(line.x1);
        whole.last = whole.last.max(line.y);
        whole.size = whole.size.max(line.size);
    }
    whole
}

/// A straight line a page rules, level or upright: where it lies across its
/// direction and from where to where it runs along it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Rule {
    pub level: bool,
    pub at: f64,
    pub from: f64,
    pub to: f64,
}

/// An area a page fills with a colour, `[x0, y0, x1, y1]`.
pub(crate) type Shade = [f64; 4];

/// A subpath of an outline: its points, whether it closes and whether it
/// curves.
type Subpath = (Vec<(f64, f64)>, bool, bool);

/// The subpaths of an outline [`super::pdf_paths`] writes: their points,
/// whether they close and whether they curve.
fn subpaths(outline: &str) -> Vec<Subpath> {
    let mut out: Vec<Subpath> = vec![];
    let mut rest = outline;
    while let Some(command) = rest.chars().next() {
        let end = rest[1..]
            .find(['M', 'L', 'C', 'Z'])
            .map_or(rest.len(), |i| i + 1);
        let points = rest[1..end].split(' ').filter_map(|point| {
            let (x, y) = point.split_once(',')?;
            Some((x.parse().ok()?, y.parse().ok()?))
        });
        match command {
            'M' => out.push((points.collect(), false, false)),
            'Z' => {
                if let Some(last) = out.last_mut() {
                    last.1 = true;
                }
            }
            command => {
                if let Some(last) = out.last_mut() {
                    last.0.extend(points);
                    last.2 |= command == 'C';
                }
            }
        }
        rest = &rest[end..];
    }
    out
}

/// The rulings and the filled areas among `paths`: the level and upright
/// edges of what is stroked and the filled rectangles as thin as a line are
/// rulings, the other filled rectangles areas. What is painted white is
/// neither.
fn rulings(paths: &[PdfPath]) -> (Vec<Rule>, Vec<Shade>) {
    let mut rules = vec![];
    let mut shades = vec![];
    let seen = |color: &Option<String>| color.as_deref().is_some_and(|c| c != "FFFFFF");
    for path in paths {
        let (stroked, filled) = (seen(&path.stroke), seen(&path.fill));
        if !stroked && !filled {
            continue;
        }
        for (points, closed, curved) in subpaths(&path.outline) {
            if curved || points.len() < 2 {
                continue;
            }
            if stroked {
                let closing = closed.then(|| (points[points.len() - 1], points[0]));
                for (a, b) in points
                    .windows(2)
                    .map(|pair| (pair[0], pair[1]))
                    .chain(closing)
                {
                    let (dx, dy) = ((a.0 - b.0).abs(), (a.1 - b.1).abs());
                    if dy <= 0.6 && dx >= 2.0 {
                        rules.push(Rule {
                            level: true,
                            at: (a.1 + b.1) / 2.0,
                            from: a.0.min(b.0),
                            to: a.0.max(b.0),
                        });
                    } else if dx <= 0.6 && dy >= 2.0 {
                        rules.push(Rule {
                            level: false,
                            at: (a.0 + b.0) / 2.0,
                            from: a.1.min(b.1),
                            to: a.1.max(b.1),
                        });
                    }
                }
            }
            if filled {
                let fold = |pick: fn(&(f64, f64)) -> f64| {
                    points
                        .iter()
                        .map(pick)
                        .fold((f64::MAX, f64::MIN), |(low, high), v| {
                            (low.min(v), high.max(v))
                        })
                };
                let ((x0, x1), (y0, y1)) = (fold(|p| p.0), fold(|p| p.1));
                let cornered = points.iter().all(|(x, y)| {
                    ((x - x0).abs() < 0.3 || (x - x1).abs() < 0.3)
                        && ((y - y0).abs() < 0.3 || (y - y1).abs() < 0.3)
                });
                let (wide, high) = (x1 - x0, y1 - y0);
                if !cornered {
                    continue;
                }
                if high <= THICK && wide >= 2.0 && wide >= 2.0 * high {
                    rules.push(Rule {
                        level: true,
                        at: (y0 + y1) / 2.0,
                        from: x0,
                        to: x1,
                    });
                } else if wide <= THICK && high >= 2.0 && high >= 2.0 * wide {
                    rules.push(Rule {
                        level: false,
                        at: (x0 + x1) / 2.0,
                        from: y0,
                        to: y1,
                    });
                } else if wide > THICK && high > THICK {
                    shades.push([x0, y0, x1, y1]);
                }
            }
        }
    }
    (rules, shades)
}

/// `rules` of one direction with those along one line that meet or overlap
/// made one, as a dashed line or a line drawn cell by cell is.
fn joined(mut rules: Vec<Rule>) -> Vec<Rule> {
    rules.sort_by(|a, b| a.at.total_cmp(&b.at).then(a.from.total_cmp(&b.from)));
    let mut out: Vec<Rule> = vec![];
    // The rules of the line being gathered start at `first`.
    let mut first = 0;
    for rule in rules {
        if out.get(first).is_none_or(|start| rule.at - start.at > 1.5) {
            first = out.len();
            out.push(rule);
            continue;
        }
        match out[first..]
            .iter_mut()
            .find(|seen| rule.from <= seen.to + MEET && seen.from <= rule.to + MEET)
        {
            Some(seen) => {
                seen.from = seen.from.min(rule.from);
                seen.to = seen.to.max(rule.to);
            }
            None => out.push(rule),
        }
    }
    out
}

/// `values` in order, those within [`MEET`] of the one before made one at
/// their mean.
fn clustered(mut values: Vec<f64>) -> Vec<f64> {
    values.sort_by(f64::total_cmp);
    let mut out: Vec<(f64, usize)> = vec![];
    let mut last = f64::MIN;
    for value in values {
        match out.last_mut() {
            Some((sum, count)) if value - last <= MEET => {
                *sum += value;
                *count += 1;
            }
            _ => out.push((value, 1)),
        }
        last = value;
    }
    out.into_iter()
        .map(|(sum, count)| sum / count as f64)
        .collect()
}

/// A table: where it lies on the page, its columns and rows, and its cells
/// with the columns and rows each spans (from 0, both ends included).
#[derive(Clone, Debug)]
struct Table {
    bounds: [f64; 4],
    columns: usize,
    rows: usize,
    cells: Vec<([usize; 4], Vec<Piece>)>,
    header_rows: usize,
}

/// Rulings that cross one another: the lines of its columns and rows, and
/// the region of cells without a ruling between them that each cell of its
/// grid is of, with what each region spans.
struct Lattice {
    xs: Vec<f64>,
    ys: Vec<f64>,
    region: Vec<usize>,
    spans: Vec<[usize; 4]>,
    /// The strings in each region that no paragraph is said to hold, and
    /// the paragraphs in it.
    words: Vec<Vec<Word>>,
    texts: Vec<Vec<Fragment>>,
}

impl Lattice {
    fn columns(&self) -> usize {
        self.xs.len() - 1
    }

    /// The region at a point of the page, if the lattice covers it.
    fn region_at(&self, x: f64, y: f64) -> Option<usize> {
        let within = |lines: &[f64], v: f64| {
            (v >= lines[0] - 1.0 && v <= lines[lines.len() - 1] + 1.0)
                .then(|| lines.partition_point(|l| *l <= v).clamp(1, lines.len() - 1) - 1)
        };
        let (column, row) = (within(&self.xs, x)?, within(&self.ys, y)?);
        Some(self.region[row * self.columns() + column])
    }

    fn area(&self, region: usize) -> f64 {
        let [c0, r0, c1, r1] = self.spans[region];
        (self.xs[c1 + 1] - self.xs[c0]) * (self.ys[r1 + 1] - self.ys[r0])
    }
}

/// The lattices of `rules`: each set of level and upright rulings that
/// cross, where it rules more than one box.
fn lattices(rules: Vec<Rule>) -> Vec<Lattice> {
    let (level, upright): (Vec<Rule>, Vec<Rule>) = rules.into_iter().partition(|r| r.level);
    let (level, upright) = (joined(level), joined(upright));
    if level.len() + upright.len() > MAX_RULES {
        return vec![];
    }
    // Rulings that cross are of one set.
    let mut parent: Vec<usize> = (0..level.len() + upright.len()).collect();
    fn root(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    for (h, across) in level.iter().enumerate() {
        for (v, down) in upright.iter().enumerate() {
            if across.from - MEET <= down.at
                && down.at <= across.to + MEET
                && down.from - MEET <= across.at
                && across.at <= down.to + MEET
            {
                let (a, b) = (root(&mut parent, h), root(&mut parent, level.len() + v));
                parent[a] = b;
            }
        }
    }
    let mut sets: BTreeMap<usize, (Vec<Rule>, Vec<Rule>)> = BTreeMap::new();
    for (i, rule) in level.iter().chain(&upright).enumerate() {
        let set = sets.entry(root(&mut parent, i)).or_default();
        if rule.level {
            set.0.push(*rule);
        } else {
            set.1.push(*rule);
        }
    }
    let mut out = vec![];
    for (level, upright) in sets.into_values() {
        let enough = (level.len() >= 2 && upright.len() >= 2)
            || (level.len() >= 3 && !upright.is_empty())
            || (upright.len() >= 3 && !level.is_empty());
        if !enough {
            continue;
        }
        let xs = clustered(
            upright
                .iter()
                .map(|r| r.at)
                .chain(level.iter().flat_map(|r| [r.from, r.to]))
                .collect(),
        );
        let ys = clustered(
            level
                .iter()
                .map(|r| r.at)
                .chain(upright.iter().flat_map(|r| [r.from, r.to]))
                .collect(),
        );
        if xs.len() < 2 || ys.len() < 2 || xs.len() > MAX_COLUMNS {
            continue;
        }
        let (columns, rows) = (xs.len() - 1, ys.len() - 1);
        // Whether a ruling of `rules` at `at` runs along most of `from..to`.
        let ruled = |rules: &[Rule], at: f64, from: f64, to: f64| {
            rules.iter().any(|r| {
                (r.at - at).abs() <= MEET && r.to.min(to) - r.from.max(from) >= 0.5 * (to - from)
            })
        };
        let mut cells: Vec<usize> = (0..columns * rows).collect();
        for row in 0..rows {
            for column in 0..columns {
                let here = row * columns + column;
                if column + 1 < columns && !ruled(&upright, xs[column + 1], ys[row], ys[row + 1]) {
                    let (a, b) = (root(&mut cells, here), root(&mut cells, here + 1));
                    cells[a] = b;
                }
                if row + 1 < rows && !ruled(&level, ys[row + 1], xs[column], xs[column + 1]) {
                    let (a, b) = (root(&mut cells, here), root(&mut cells, here + columns));
                    cells[a] = b;
                }
            }
        }
        // The regions in reading order, each by what it spans; one that is
        // not a rectangle is left as the cells of the grid.
        let mut extent: BTreeMap<usize, ([usize; 4], usize)> = BTreeMap::new();
        for here in 0..columns * rows {
            let (column, row) = (here % columns, here / columns);
            let (span, count) = extent
                .entry(root(&mut cells, here))
                .or_insert(([column, row, column, row], 0));
            *span = [
                span[0].min(column),
                span[1].min(row),
                span[2].max(column),
                span[3].max(row),
            ];
            *count += 1;
        }
        let mut region = vec![0; columns * rows];
        let mut spans = vec![];
        for here in 0..columns * rows {
            let (column, row) = (here % columns, here / columns);
            let set = root(&mut cells, here);
            let (span, count) = extent[&set];
            let whole = (span[2] - span[0] + 1) * (span[3] - span[1] + 1) == count;
            if !whole {
                region[here] = spans.len();
                spans.push([column, row, column, row]);
            } else if [column, row] == [span[0], span[1]] {
                for r in span[1]..=span[3] {
                    for c in span[0]..=span[2] {
                        region[r * columns + c] = spans.len();
                    }
                }
                spans.push(span);
            }
        }
        if spans.len() < 2 {
            continue;
        }
        out.push(Lattice {
            xs,
            ys,
            region,
            words: vec![vec![]; spans.len()],
            texts: vec![vec![]; spans.len()],
            spans,
        });
    }
    out
}

/// The header rows of a table as a hypothesis for review, as a Word table's
/// are: its leading rows whose cells are all shaded, else the first row of a
/// table of three or more columns. A header cell spanning rows takes them.
fn header_rows(
    cells: &[([usize; 4], Vec<Piece>)],
    shaded: &[bool],
    columns: usize,
    rows: usize,
) -> usize {
    let mut all = vec![(0usize, true); rows];
    for ((span, _), shaded) in cells.iter().zip(shaded) {
        all[span[1]].0 += 1;
        all[span[1]].1 &= shaded;
    }
    let emphasized = all
        .iter()
        .take_while(|(count, all)| *count > 0 && *all)
        .count();
    let mut header = if emphasized > 0 && emphasized < rows {
        emphasized
    } else if columns >= 3 && rows >= 2 {
        1
    } else {
        0
    };
    while let Some(deeper) = cells
        .iter()
        .map(|(span, _)| span)
        .filter(|span| span[1] < header && span[3] >= header)
        .map(|span| span[3] + 1)
        .max()
    {
        header = deeper;
    }
    if header < rows { header } else { 0 }
}

/// What a region of the page holds, in the order it is read.
enum Block {
    /// The texts of one line, left to right.
    Row(Vec<Fragment>),
    Table(Table),
}

/// Something laid out among others: a text or a ruled table.
#[derive(Clone)]
enum Item {
    Text(Fragment),
    Table(usize),
}

struct Free<'a> {
    tables: &'a [Table],
    out: Vec<Block>,
}

/// The median of `values`, or `default` of none.
fn median(mut values: Vec<f64>, default: f64) -> f64 {
    if values.is_empty() {
        return default;
    }
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}

impl Free<'_> {
    fn bounds(&self, item: &Item) -> [f64; 4] {
        match item {
            Item::Text(f) => [f.x0, f.top(), f.x1, f.bottom()],
            Item::Table(t) => self.tables[*t].bounds,
        }
    }

    /// `items` in groups along an axis (`low` and `high` give an item's
    /// extent on it), a new group where nothing reaches across a gap of
    /// `least` or more. Also the gaps between what follows one another.
    fn cut(
        &self,
        mut items: Vec<Item>,
        low: usize,
        high: usize,
        least: impl Fn(&[f64]) -> f64,
    ) -> Vec<Vec<Item>> {
        items.sort_by(|a, b| self.bounds(a)[low].total_cmp(&self.bounds(b)[low]));
        let mut reach = f64::MIN;
        let mut gaps = vec![];
        for item in &items {
            let bounds = self.bounds(item);
            if reach > f64::MIN {
                gaps.push(bounds[low] - reach);
            }
            reach = reach.max(bounds[high]);
        }
        let least = least(&gaps);
        let mut groups: Vec<Vec<Item>> = vec![];
        for (i, item) in items.into_iter().enumerate() {
            if i == 0 || gaps[i - 1] >= least {
                groups.push(vec![]);
            }
            groups.last_mut().unwrap().push(item);
        }
        groups
    }

    /// Lays `items` out: side by side where a gap runs down between them,
    /// else one under another where they stand apart, else line by line.
    fn lay(&mut self, items: Vec<Item>) {
        if items.is_empty() {
            return;
        }
        let em = median(
            items
                .iter()
                .filter_map(|item| match item {
                    Item::Text(f) => Some(f.size),
                    Item::Table(_) => None,
                })
                .collect(),
            10.0,
        );
        let river = (0.8 * em).max(6.0);
        let columns = self.cut(items, 0, 2, |_| river);
        if columns.len() > 1 {
            let texts: Option<Vec<Vec<Fragment>>> = columns
                .iter()
                .map(|column| {
                    column
                        .iter()
                        .map(|item| match item {
                            Item::Text(f) => Some(f.clone()),
                            Item::Table(_) => None,
                        })
                        .collect()
                })
                .collect();
            match texts.map_or(Columns::Apart, unruled) {
                Columns::Table(table) => {
                    self.out.push(Block::Table(table));
                    return;
                }
                Columns::Apart => {
                    for column in columns {
                        self.lay(column);
                    }
                    return;
                }
                // Read line by line, as what no gap parts is.
                Columns::Lines => {}
            }
        }
        let items: Vec<Item> = columns.into_iter().flatten().collect();
        // Lines stand apart where the gap between them is wider than a
        // line and than most gaps there.
        let bands = self.cut(items, 1, 3, |gaps| {
            let between: Vec<f64> = gaps.iter().copied().filter(|g| *g > 0.0).collect();
            (0.9 * em).max(1.6 * median(between, 0.0))
        });
        if bands.len() > 1 {
            for band in bands {
                self.lay(band);
            }
            return;
        }
        self.leaf(bands.into_iter().next().unwrap());
    }

    /// `items` that nothing parts, line by line; lines set in the same
    /// columns are a table.
    fn leaf(&mut self, items: Vec<Item>) {
        // Each line with where it starts: its texts, or a ruled table.
        let mut lines: Vec<(f64, Result<Vec<Fragment>, usize>)> = vec![];
        let mut texts: Vec<Fragment> = vec![];
        for item in items {
            match item {
                Item::Text(f) => texts.push(f),
                Item::Table(t) => lines.push((self.tables[t].bounds[1], Err(t))),
            }
        }
        texts.sort_by(|a, b| a.y.total_cmp(&b.y).then(a.x0.total_cmp(&b.x0)));
        let mut gathered: Vec<Vec<Fragment>> = vec![];
        for text in texts {
            match gathered.last_mut() {
                Some(line) if text.y - line[0].y <= 0.3 * text.size.min(line[0].size) => {
                    line.push(text);
                }
                _ => gathered.push(vec![text]),
            }
        }
        lines.extend(gathered.into_iter().map(|line| {
            (
                line.iter().map(Fragment::top).fold(f64::MAX, f64::min),
                Ok(line),
            )
        }));
        lines.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut at = 0;
        while at < lines.len() {
            let line = match &mut lines[at].1 {
                Err(table) => {
                    self.out.push(Block::Table(self.tables[*table].clone()));
                    at += 1;
                    continue;
                }
                Ok(line) => {
                    line.sort_by(|a, b| a.x0.total_cmp(&b.x0));
                    line.clone()
                }
            };
            // The gaps of this line that the lines after it leave open.
            let mut gaps: Vec<(f64, f64)> = line.windows(2).map(|w| (w[0].x1, w[1].x0)).collect();
            let mut end = at + 1;
            let mut set = 1;
            let mut pitch = f64::MAX;
            while !gaps.is_empty() && end < lines.len() {
                let (Ok(next), Ok(last)) = (&lines[end].1, &lines[end - 1].1) else {
                    break;
                };
                let step = next[0].y - last[0].y;
                if step > 2.5 * pitch.min(4.0 * next[0].size) {
                    break;
                }
                let open: Vec<(f64, f64)> = gaps
                    .iter()
                    .flat_map(|(from, to)| {
                        // What of the gap is left of, between and right of
                        // the texts of the next line.
                        let mut parts = vec![];
                        let mut start = *from;
                        let mut texts: Vec<&Fragment> = next.iter().collect();
                        texts.sort_by(|a, b| a.x0.total_cmp(&b.x0));
                        for text in texts {
                            if text.x0 > start {
                                parts.push((start, text.x0.min(*to)));
                            }
                            start = start.max(text.x1);
                        }
                        parts.push((start, *to));
                        parts
                    })
                    .filter(|(from, to)| to - from >= 0.8 * line[0].size)
                    .collect();
                if open.is_empty() {
                    break;
                }
                gaps = open;
                if next.len() > 1 {
                    set += 1;
                }
                pitch = pitch.min(step);
                end += 1;
            }
            // A last line with one text may be what follows the table.
            while end > at + 1 && matches!(&lines[end - 1].1, Ok(l) if l.len() < 2) {
                end -= 1;
            }
            if set >= 2 && end > at + 1 {
                let mut columns: Vec<Vec<Fragment>> = vec![vec![]; gaps.len() + 1];
                for (_, line) in &lines[at..end] {
                    for text in line.as_ref().into_iter().flatten() {
                        let column = gaps.partition_point(|(from, _)| *from < text.x1 - 0.5);
                        columns[column.min(gaps.len())].push(text.clone());
                    }
                }
                columns.retain(|column| !column.is_empty());
                if let Columns::Table(table) = unruled(columns) {
                    self.out.push(Block::Table(table));
                    at = end;
                    continue;
                }
            }
            self.out.push(Block::Row(line));
            at += 1;
        }
    }
}

/// What texts with gaps running down between them are.
enum Columns {
    /// A table: the columns share their lines, and two lines or more have
    /// texts in several of them.
    Table(Table),
    /// Columns read one after another: side by side they each have lines
    /// of their own, or are filled to their width line after line, as
    /// running text is.
    Apart,
    /// Lines read one after another, a line here or there with a text set
    /// apart on it, as a heading is from the indented text under it.
    Lines,
}

/// What the texts set in `columns` are; the rows of a table are their
/// lines.
fn unruled(columns: Vec<Vec<Fragment>>) -> Columns {
    let baselines = |texts: &mut dyn Iterator<Item = &Fragment>| {
        let mut ys: Vec<(f64, f64)> = texts.map(|f| (f.y, f.size)).collect();
        ys.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut lines: Vec<f64> = vec![];
        let mut start = (f64::MIN, 0.0);
        for (y, size) in ys {
            if y - start.0 > 0.3 * size.min(start.1) {
                lines.push(y);
                start = (y, size);
            }
        }
        lines
    };
    let rows = baselines(&mut columns.iter().flatten());
    if rows.len() < 2 {
        return Columns::Lines;
    }
    let own: Vec<usize> = columns
        .iter()
        .map(|column| baselines(&mut column.iter()).len())
        .collect();
    let most = own.iter().copied().max().unwrap_or(0);
    if rows.len() as f64 > 1.3 * most as f64 || columns.len() > MAX_COLUMNS {
        // Columns of running text stand side by side, each with about as
        // many lines there as the others.
        let extent = |column: &Vec<Fragment>| {
            column
                .iter()
                .fold((f64::MAX, f64::MIN), |(top, bottom), f| {
                    (top.min(f.y), bottom.max(f.y))
                })
        };
        let (top, bottom) = columns
            .iter()
            .map(extent)
            .fold((f64::MIN, f64::MAX), |(top, bottom), (t, b)| {
                (top.max(t), bottom.min(b))
            });
        let beside: Vec<usize> = columns
            .iter()
            .map(|column| {
                let mut within = column.iter().filter(|f| (top..=bottom).contains(&f.y));
                baselines(&mut within).len()
            })
            .collect();
        let (least, most) = (
            beside.iter().copied().min().unwrap_or(0),
            beside.iter().copied().max().unwrap_or(0),
        );
        return if least >= 2 && 2 * least >= most {
            Columns::Apart
        } else {
            Columns::Lines
        };
    }
    // Running text fills its column line after line.
    let running = columns.len() <= 3
        && columns.iter().zip(&own).all(|(column, lines)| {
            let (left, right) = column
                .iter()
                .fold((f64::MAX, f64::MIN), |(l, r), f| (l.min(f.x0), r.max(f.x1)));
            let full = column
                .iter()
                .filter(|f| f.x1 - f.x0 >= 0.85 * (right - left))
                .count();
            *lines >= 4 && full as f64 >= 0.6 * *lines as f64
        });
    if running {
        return Columns::Apart;
    }
    let mut cells: BTreeMap<(usize, usize), Vec<Fragment>> = BTreeMap::new();
    let mut bounds = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
    for (column, texts) in columns.iter().enumerate() {
        for text in texts {
            let row = rows
                .partition_point(|y| *y <= text.y + 0.01)
                .saturating_sub(1);
            cells.entry((row, column)).or_default().push(text.clone());
            bounds = [
                bounds[0].min(text.x0),
                bounds[1].min(text.top()),
                bounds[2].max(text.x1),
                bounds[3].max(text.bottom()),
            ];
        }
    }
    let mut filled = vec![0usize; rows.len()];
    for (row, _) in cells.keys() {
        filled[*row] += 1;
    }
    if filled.iter().filter(|columns| **columns > 1).count() < 2 {
        return Columns::Lines;
    }
    let cells: Vec<([usize; 4], Vec<Piece>)> = cells
        .into_iter()
        .map(|((row, column), mut texts)| {
            texts.sort_by(|a, b| a.x0.total_cmp(&b.x0));
            let mut pieces: Vec<Piece> = vec![];
            for text in texts {
                let first = pieces.len();
                pieces.extend(text.pieces);
                if first > 0 {
                    pieces[first].before = " ";
                }
            }
            ([column, row, column, row], pieces)
        })
        .collect();
    let header_rows = header_rows(&cells, &vec![false; cells.len()], columns.len(), rows.len());
    Columns::Table(Table {
        bounds,
        columns: columns.len(),
        rows: rows.len(),
        cells,
        header_rows,
    })
}

/// The address of the cell at a 1-based `column` and `row`.
fn address(column: u32, row: u32) -> String {
    let column = crate::excel::column_name(column).expect("a column of a sheet");
    format!("{column}{row}")
}

/// The sheet of a page that shows `shown`, paints `paths` and names the
/// tables `marked`.
pub(super) fn layout(shown: &[Shown<'_>], paths: &[PdfPath], marked: &[Marked]) -> Layout {
    let (rules, shades) = rulings(paths);
    ruled_layout(shown, rules, &shades, marked)
}

/// The table a page names by its rows and cells, whose text is that of the
/// paragraphs whose lines `units` holds; those it takes are taken from
/// `units`. None when it shows no text.
fn marked_table(
    marked: &Marked,
    units: &mut BTreeMap<usize, Vec<Fragment>>,
    shown: &[Shown<'_>],
) -> Option<Table> {
    let mut taken: std::collections::BTreeSet<(usize, usize)> = Default::default();
    let mut cells = vec![];
    let mut bounds = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
    let (mut columns, mut rows) = (0, marked.rows.len());
    for (row, row_cells) in marked.rows.iter().enumerate() {
        let mut column = 0;
        for cell in row_cells {
            while taken.contains(&(row, column)) {
                column += 1;
            }
            let (right, bottom) = (column + cell.columns.max(1), row + cell.rows.max(1));
            for r in row..bottom {
                for c in column..right {
                    taken.insert((r, c));
                }
            }
            (columns, rows) = (columns.max(right), rows.max(bottom));
            let texts: Vec<Fragment> = cell
                .units
                .iter()
                .filter_map(|unit| units.remove(unit))
                .map(|lines| paragraph(lines, shown, None))
                .collect();
            for text in &texts {
                bounds = [
                    bounds[0].min(text.x0),
                    bounds[1].min(text.top()),
                    bounds[2].max(text.x1),
                    bounds[3].max(text.bottom()),
                ];
            }
            if !texts.is_empty() {
                cells.push(([column, row, right - 1, bottom - 1], cell_pieces(texts)));
            }
            column = right;
        }
    }
    if cells.is_empty() || columns > MAX_COLUMNS {
        return None;
    }
    let header_rows = if marked.header_rows > 0 && marked.header_rows < rows {
        marked.header_rows
    } else {
        header_rows(&cells, &vec![false; cells.len()], columns, rows)
    };
    Some(Table {
        bounds,
        columns,
        rows,
        cells,
        header_rows,
    })
}

/// The sheet of a page that shows `shown`, is ruled by `rules`, has the
/// areas `shades` filled with a colour and names the tables `marked`.
pub(crate) fn ruled_layout(
    shown: &[Shown<'_>],
    rules: Vec<Rule>,
    shades: &[Shade],
    marked: &[Marked],
) -> Layout {
    let mut tables = vec![];
    // The strings of each paragraph the page names are one text; a table
    // it names takes those of its cells.
    let mut held: BTreeMap<usize, Vec<Word>> = BTreeMap::new();
    let mut units = BTreeMap::new();
    let words: Vec<Word> = shown
        .iter()
        .enumerate()
        .filter(|(_, s)| !s.text.is_empty())
        .map(|(string, s)| Word {
            string,
            x0: s.x,
            x1: s.x + s.width.max(0.0),
            y: s.y,
            size: if s.size > 0.0 { s.size } else { 10.0 },
            upright: s.upright,
        })
        .collect();
    let (held_words, words): (Vec<Word>, Vec<Word>) = words
        .into_iter()
        .partition(|word| shown[word.string].unit.is_some());
    for word in held_words {
        if let Some(unit) = shown[word.string].unit {
            held.entry(unit).or_default().push(word);
        }
    }
    for (unit, words) in held {
        let lines = line_texts(words, shown);
        if !lines.is_empty() {
            units.insert(unit, lines);
        }
    }
    for marked in marked {
        tables.extend(marked_table(marked, &mut units, shown));
    }
    let mut lattices = lattices(rules);
    // A string, or a paragraph, is of the smallest ruled cell it starts in.
    let within = |lattices: &[Lattice], [x, y]: [f64; 2]| {
        lattices
            .iter()
            .enumerate()
            .filter_map(|(i, lattice)| Some((i, lattice.region_at(x, y)?)))
            .min_by(|a, b| lattices[a.0].area(a.1).total_cmp(&lattices[b.0].area(b.1)))
    };
    let mut free = vec![];
    for word in words {
        let x = word.x0 + (0.25 * word.size).min((word.x1 - word.x0) / 2.0);
        match within(&lattices, [x, word.y - 0.3 * word.size]) {
            Some((lattice, region)) => lattices[lattice].words[region].push(word),
            None => free.push(word),
        }
    }
    let mut items = vec![];
    for lines in units.into_values() {
        let first = &lines[0];
        let x = first.x0 + (0.25 * first.size).min((first.x1 - first.x0) / 2.0);
        match within(&lattices, [x, first.y - 0.3 * first.size]) {
            Some((lattice, region)) => {
                // The paragraph may run to the right edge of its cell.
                let lattice = &mut lattices[lattice];
                let right = lattice.xs[lattice.spans[region][2] + 1];
                lattice.texts[region].push(paragraph(lines, shown, Some(right)));
            }
            None => items.push(Item::Text(paragraph(lines, shown, None))),
        }
    }
    for mut lattice in lattices {
        let bounds = [
            lattice.xs[0],
            lattice.ys[0],
            lattice.xs[lattice.xs.len() - 1],
            lattice.ys[lattice.ys.len() - 1],
        ];
        let whole = (bounds[2] - bounds[0]) * (bounds[3] - bounds[1]);
        let mut cells = vec![];
        let mut shaded = vec![];
        for region in 0..lattice.spans.len() {
            let mut texts = line_texts(std::mem::take(&mut lattice.words[region]), shown);
            texts.append(&mut lattice.texts[region]);
            let pieces = cell_pieces(texts);
            if pieces.is_empty() {
                continue;
            }
            let [c0, r0, c1, r1] = lattice.spans[region];
            let centre = [
                (lattice.xs[c0] + lattice.xs[c1 + 1]) / 2.0,
                (lattice.ys[r0] + lattice.ys[r1 + 1]) / 2.0,
            ];
            shaded.push(shades.iter().any(|[x0, y0, x1, y1]| {
                (x1 - x0) * (y1 - y0) < 0.8 * whole
                    && (*x0..=*x1).contains(&centre[0])
                    && (*y0..=*y1).contains(&centre[1])
            }));
            cells.push((lattice.spans[region], pieces));
        }
        if cells.is_empty() {
            continue;
        }
        let (columns, rows) = (lattice.columns(), lattice.ys.len() - 1);
        tables.push(Table {
            bounds,
            columns,
            rows,
            header_rows: header_rows(&cells, &shaded, columns, rows),
            cells,
        });
    }
    items.extend((0..tables.len()).map(Item::Table));
    for line in lines(free) {
        items.extend(fragments(&line, shown, true).into_iter().map(Item::Text));
    }
    let mut free = Free {
        tables: &tables,
        out: vec![],
    };
    free.lay(items);
    let mut layout = Layout::default();
    let mut row = 1u32;
    for block in free.out {
        match block {
            Block::Row(texts) => {
                for (column, text) in texts.into_iter().enumerate() {
                    // Texts beyond the columns of a sheet join the last.
                    match layout.cells.last_mut() {
                        Some(last) if column >= MAX_COLUMNS => {
                            let first = last.pieces.len();
                            last.pieces.extend(text.pieces);
                            last.pieces[first].before = " ";
                        }
                        _ => layout.cells.push(Cell {
                            column: column as u32 + 1,
                            row,
                            pieces: text.pieces,
                        }),
                    }
                }
                row += 1;
            }
            Block::Table(mut table) => {
                table.cells.sort_by_key(|(span, _)| (span[1], span[0]));
                for ([c0, r0, c1, r1], pieces) in table.cells {
                    let (column, top) = (c0 as u32 + 1, row + r0 as u32);
                    if [c0, r0] != [c1, r1] {
                        layout.merges.push(format!(
                            "{}:{}",
                            address(column, top),
                            address(c1 as u32 + 1, row + r1 as u32)
                        ));
                    }
                    layout.cells.push(Cell {
                        column,
                        row: top,
                        pieces,
                    });
                }
                let last = row + table.rows as u32 - 1;
                layout.tables.push(json!({
                    "name":format!("table-{}", layout.tables.len() + 1),
                    "range":format!("{}:{}", address(1, row), address(table.columns as u32, last)),
                    "header_rows":table.header_rows,"totals_rows":0}));
                row = last + 1;
            }
        }
    }
    layout
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Strings of size 10 at `(x, y)`, each as wide as half its size a
    /// character.
    fn shown<'a>(strings: &[(&'a str, f64, f64)]) -> Vec<Shown<'a>> {
        strings
            .iter()
            .map(|(text, x, y)| Shown {
                text,
                x: *x,
                y: *y,
                width: text.chars().count() as f64 * 5.0,
                size: 10.0,
                upright: true,
                invisible: false,
                unit: None,
            })
            .collect()
    }

    /// [`layout`] of a page that names no tables.
    fn layout(shown: &[Shown<'_>], paths: &[PdfPath]) -> Layout {
        super::layout(shown, paths, &[])
    }

    fn line(x0: f64, y0: f64, x1: f64, y1: f64) -> PdfPath {
        PdfPath {
            outline: format!("M{x0:.2},{y0:.2}L{x1:.2},{y1:.2}"),
            fill: None,
            even_odd: false,
            stroke: Some("000000".into()),
            width: 0.5,
        }
    }

    /// The cells of a layout as `(address, text)`.
    fn texts(layout: &Layout, shown: &[Shown<'_>]) -> Vec<(String, String)> {
        layout
            .cells
            .iter()
            .map(|cell| {
                (
                    address(cell.column, cell.row),
                    composed(&cell.pieces, |i| shown[i].text),
                )
            })
            .collect()
    }

    fn pairs(cells: &[(&str, &str)]) -> Vec<(String, String)> {
        cells
            .iter()
            .map(|(a, t)| ((*a).to_owned(), (*t).to_owned()))
            .collect()
    }

    #[test]
    fn strings_of_a_line_are_one_text_in_the_order_they_are_read() {
        // Drawn out of order and in pieces: a heading a character at a
        // time, words a space apart, and a line under them.
        let shown = shown(&[
            ("second line", 50.0, 130.0),
            ("書", 60.0, 100.0),
            ("world", 130.0, 115.0),
            ("仕", 50.0, 100.0),
            ("Hello", 50.0, 115.0),
            ("様", 55.0, 100.0),
            (",", 75.0, 115.0),
        ]);
        let layout = layout(&shown, &[]);
        assert_eq!(
            texts(&layout, &shown),
            pairs(&[
                ("A1", "仕様書"),
                ("A2", "Hello,"),
                ("B2", "world"),
                ("A3", "second line")
            ])
        );
        assert!(layout.tables.is_empty() && layout.merges.is_empty());
    }

    #[test]
    fn a_gap_between_words_is_a_space_but_not_between_japanese_characters() {
        let shown = shown(&[
            ("Order", 50.0, 100.0),
            ("number", 79.0, 100.0),
            ("受", 50.0, 120.0),
            ("注", 65.0, 120.0),
        ]);
        assert_eq!(
            texts(&layout(&shown, &[]), &shown),
            pairs(&[("A1", "Order number"), ("A2", "受注")])
        );
    }

    #[test]
    fn rulings_that_cross_are_a_table_with_its_merged_cells() {
        // Three columns and three rows; the first row is one cell, and the
        // last two rows of the first column are one.
        let mut paths = vec![];
        for y in [100.0, 120.0, 140.0, 160.0] {
            let from = if y == 140.0 { 200.0 } else { 100.0 };
            paths.push(line(from, y, 400.0, y));
        }
        for x in [100.0, 200.0, 300.0, 400.0] {
            let from = if x == 200.0 || x == 300.0 {
                120.0
            } else {
                100.0
            };
            paths.push(line(x, from, x, 160.0));
        }
        let shown = shown(&[
            ("9", 305.0, 155.0),
            ("Title", 105.0, 115.0),
            ("above", 100.0, 80.0),
            ("key", 105.0, 135.0),
            ("a", 205.0, 135.0),
            ("b", 305.0, 135.0),
            ("two", 205.0, 150.0),
            ("lines", 205.0, 158.0),
            ("below", 100.0, 190.0),
        ]);
        let layout = layout(&shown, &paths);
        assert_eq!(
            texts(&layout, &shown),
            pairs(&[
                ("A1", "above"),
                ("A2", "Title"),
                ("A3", "key"),
                ("B3", "a"),
                ("C3", "b"),
                ("B4", "two\nlines"),
                ("C4", "9"),
                ("A5", "below"),
            ])
        );
        assert_eq!(layout.merges, ["A2:C2", "A3:A4"]);
        assert_eq!(
            layout.tables,
            [json!({"name":"table-1","range":"A2:C4","header_rows":1,"totals_rows":0})]
        );
    }

    #[test]
    fn thin_filled_rectangles_and_dashes_rule_a_table_and_shading_heads_it() {
        let fill = |x0: f64, y0: f64, x1: f64, y1: f64, color: &str| PdfPath {
            outline: format!("M{x0:.2},{y0:.2}L{x1:.2},{y0:.2} {x1:.2},{y1:.2} {x0:.2},{y1:.2}Z"),
            fill: Some(color.into()),
            even_odd: false,
            stroke: None,
            width: 1.0,
        };
        let mut paths = vec![fill(100.0, 100.0, 300.0, 120.0, "D9D9D9")];
        for y in [100.0, 120.0, 140.0] {
            paths.push(fill(100.0, y - 0.5, 300.0, y + 0.5, "000000"));
        }
        for x in [100.0, 200.0, 300.0] {
            // Dashes two points long, a point apart.
            let mut y = 100.0;
            while y < 140.0 {
                paths.push(line(x, y, x, (y + 2.0_f64).min(140.0)));
                y += 3.0;
            }
        }
        let shown = shown(&[
            ("Name", 105.0, 115.0),
            ("Value", 205.0, 115.0),
            ("x", 105.0, 135.0),
            ("1", 205.0, 135.0),
        ]);
        let layout = layout(&shown, &paths);
        assert_eq!(
            texts(&layout, &shown),
            pairs(&[("A1", "Name"), ("B1", "Value"), ("A2", "x"), ("B2", "1")])
        );
        assert_eq!(
            layout.tables,
            [json!({"name":"table-1","range":"A1:B2","header_rows":1,"totals_rows":0})]
        );
    }

    #[test]
    fn a_frame_around_text_and_an_underline_are_not_tables() {
        let paths = vec![
            line(40.0, 90.0, 300.0, 90.0),
            line(40.0, 140.0, 300.0, 140.0),
            line(40.0, 90.0, 40.0, 140.0),
            line(300.0, 90.0, 300.0, 140.0),
            line(50.0, 162.0, 120.0, 162.0),
        ];
        let shown = shown(&[("framed note", 50.0, 110.0), ("underlined", 50.0, 160.0)]);
        let layout = layout(&shown, &paths);
        assert_eq!(
            texts(&layout, &shown),
            pairs(&[("A1", "framed note"), ("A2", "underlined")])
        );
        assert!(layout.tables.is_empty());
    }

    #[test]
    fn lines_set_in_the_same_columns_are_a_table_without_rulings() {
        let shown = shown(&[
            ("Item list", 50.0, 80.0),
            ("Code", 50.0, 100.0),
            ("Name", 150.0, 100.0),
            ("Price", 300.0, 100.0),
            ("A-1", 50.0, 114.0),
            ("Bolt", 150.0, 114.0),
            ("120", 300.0, 114.0),
            ("A-2", 50.0, 128.0),
            ("300", 300.0, 128.0),
            (
                "Prices are per piece and exclude tax where one applies.",
                50.0,
                150.0,
            ),
        ]);
        let layout = layout(&shown, &[]);
        assert_eq!(
            texts(&layout, &shown),
            pairs(&[
                ("A1", "Item list"),
                ("A2", "Code"),
                ("B2", "Name"),
                ("C2", "Price"),
                ("A3", "A-1"),
                ("B3", "Bolt"),
                ("C3", "120"),
                ("A4", "A-2"),
                ("C4", "300"),
                (
                    "A5",
                    "Prices are per piece and exclude tax where one applies."
                ),
            ])
        );
        assert_eq!(
            layout.tables,
            [json!({"name":"table-1","range":"A2:C4","header_rows":1,"totals_rows":0})]
        );
    }

    #[test]
    fn columns_of_running_text_are_read_one_after_another() {
        // Two columns whose lines do not share baselines, under a title
        // across both.
        let mut strings = vec![("A title across both columns of the page", 50.0, 60.0)];
        let left = ["left one", "left two", "left three", "left four"];
        let right = ["right one", "right two", "right three"];
        for (i, text) in left.iter().enumerate() {
            strings.push((text, 50.0, 100.0 + 14.0 * i as f64));
        }
        for (i, text) in right.iter().enumerate() {
            strings.push((text, 320.0, 105.0 + 17.0 * i as f64));
        }
        let shown = shown(&strings);
        let layout = layout(&shown, &[]);
        let read: Vec<String> = texts(&layout, &shown).into_iter().map(|(_, t)| t).collect();
        assert_eq!(
            read,
            [
                "A title across both columns of the page",
                "left one",
                "left two",
                "left three",
                "left four",
                "right one",
                "right two",
                "right three"
            ]
        );
        assert!(layout.tables.is_empty());
        assert!(layout.cells.iter().all(|cell| cell.column == 1));
    }

    #[test]
    fn headings_over_indented_text_are_read_line_by_line() {
        let shown = shown(&[
            ("1. Scope", 40.0, 100.0),
            (
                "The first section says what the document covers.",
                100.0,
                114.0,
            ),
            ("It runs over two lines.", 100.0, 128.0),
            ("2. Terms", 40.0, 142.0),
            ("The second section defines its terms.", 100.0, 156.0),
        ]);
        let layout = layout(&shown, &[]);
        let read: Vec<String> = texts(&layout, &shown).into_iter().map(|(_, t)| t).collect();
        assert_eq!(
            read,
            [
                "1. Scope",
                "The first section says what the document covers.",
                "It runs over two lines.",
                "2. Terms",
                "The second section defines its terms."
            ]
        );
        assert!(layout.tables.is_empty());
    }

    #[test]
    fn tables_side_by_side_are_read_left_then_right() {
        let mut paths = vec![];
        for left in [100.0, 400.0] {
            for y in [100.0, 120.0, 140.0] {
                paths.push(line(left, y, left + 200.0, y));
            }
            for x in [0.0, 100.0, 200.0] {
                paths.push(line(left + x, 100.0, left + x, 140.0));
            }
        }
        let shown = shown(&[
            ("r1", 405.0, 115.0),
            ("l1", 105.0, 115.0),
            ("r2", 505.0, 135.0),
            ("l2", 205.0, 135.0),
        ]);
        let layout = layout(&shown, &paths);
        assert_eq!(
            texts(&layout, &shown),
            pairs(&[("A1", "l1"), ("B2", "l2"), ("A3", "r1"), ("B4", "r2")])
        );
        assert_eq!(layout.tables.len(), 2);
        assert_eq!(layout.tables[1]["range"], "A3:B4");
    }

    /// `strings` with the unit each is of.
    fn held<'a>(strings: &[(&'a str, f64, f64, Option<usize>)]) -> Vec<Shown<'a>> {
        let plain: Vec<(&str, f64, f64)> =
            strings.iter().map(|(t, x, y, _)| (*t, *x, *y)).collect();
        let mut shown = shown(&plain);
        for (shown, (.., unit)) in shown.iter_mut().zip(strings) {
            shown.unit = *unit;
        }
        shown
    }

    #[test]
    fn the_lines_of_a_paragraph_a_page_names_are_one_text() {
        let shown = held(&[
            // Wrapped where the next word would not fit, and ended early.
            ("A paragraph that the page wraps", 50.0, 100.0, Some(0)),
            ("where its line is full.", 50.0, 114.0, Some(0)),
            ("Ended here", 50.0, 128.0, Some(0)),
            ("and begun again.", 50.0, 142.0, Some(0)),
            // Japanese wraps without a space; a word broken at a hyphen too.
            ("受注を登録すると在庫を", 50.0, 170.0, Some(1)),
            ("引き当てる", 50.0, 184.0, Some(1)),
            // Not of a paragraph: line by line.
            ("page 1", 50.0, 220.0, None),
        ]);
        let layout = layout(&shown, &[]);
        assert_eq!(
            texts(&layout, &shown),
            pairs(&[
                (
                    "A1",
                    "A paragraph that the page wraps where its line is full.\nEnded here\nand begun again."
                ),
                ("A2", "受注を登録すると在庫を引き当てる"),
                ("A3", "page 1"),
            ])
        );
    }

    #[test]
    fn a_paragraph_in_a_ruled_cell_wraps_at_the_edge_of_the_cell() {
        // A cell 28 wide and one 372 wide. Each string is 5 wide a
        // character: after "NOT", "NULL" does not fit in the narrow cell,
        // so the line was wrapped; after "1:first", "2:second" fits in the
        // wide one, so the line was ended. A character a line is text
        // written downwards.
        let mut paths = vec![];
        for y in [100.0, 160.0, 220.0] {
            paths.push(line(100.0, y, 500.0, y));
        }
        for x in [100.0, 128.0, 500.0] {
            paths.push(line(x, 100.0, x, 220.0));
        }
        let shown = held(&[
            ("NOT", 105.0, 120.0, Some(0)),
            ("NULL", 105.0, 134.0, Some(0)),
            ("1:first", 205.0, 120.0, Some(1)),
            ("2:second", 205.0, 134.0, Some(1)),
            ("受", 105.0, 180.0, Some(2)),
            ("注", 105.0, 194.0, Some(2)),
            ("票", 105.0, 208.0, Some(2)),
        ]);
        let layout = layout(&shown, &paths);
        assert_eq!(
            texts(&layout, &shown),
            pairs(&[
                ("A1", "NOT NULL"),
                ("B1", "1:first\n2:second"),
                ("A2", "受注票")
            ])
        );
    }

    #[test]
    fn a_table_a_page_names_is_that_table_without_rulings() {
        // A heading cell across two columns and three rows of cells, one
        // of two paragraphs; nothing is ruled, and the columns are ragged.
        let shown = held(&[
            ("Before", 50.0, 80.0, None),
            ("Order", 50.0, 100.0, Some(0)),
            ("No", 50.0, 114.0, Some(1)),
            ("Customer and address", 70.0, 114.0, Some(2)),
            ("7", 50.0, 128.0, Some(3)),
            ("Acme", 70.0, 128.0, Some(4)),
            ("Osaka", 70.0, 142.0, Some(5)),
            ("After", 50.0, 170.0, None),
        ]);
        let cell = |units: &[usize], columns: usize| MarkedCell {
            units: units.to_vec(),
            columns,
            rows: 1,
        };
        let marked = [Marked {
            rows: vec![
                vec![cell(&[0], 2)],
                vec![cell(&[1], 1), cell(&[2], 1)],
                vec![cell(&[3], 1), cell(&[4, 5], 1)],
            ],
            header_rows: 2,
        }];
        let layout = super::layout(&shown, &[], &marked);
        assert_eq!(
            texts(&layout, &shown),
            pairs(&[
                ("A1", "Before"),
                ("A2", "Order"),
                ("A3", "No"),
                ("B3", "Customer and address"),
                ("A4", "7"),
                ("B4", "Acme\nOsaka"),
                ("A5", "After"),
            ])
        );
        assert_eq!(layout.merges, ["A2:B2"]);
        assert_eq!(
            layout.tables,
            [json!({"name":"table-1","range":"A2:B4","header_rows":2,"totals_rows":0})]
        );
    }

    #[test]
    fn an_edit_is_written_to_the_strings_it_changes() {
        let strings = ["受", "注", "管理", "No", "12"];
        let pieces: Vec<Piece> = [(0, ""), (1, ""), (2, ""), (3, "\n"), (4, " ")]
            .into_iter()
            .map(|(string, before)| Piece { string, before })
            .collect();
        let text = |i: usize| strings[i];
        assert_eq!(composed(&pieces, text), "受注管理\nNo 12");
        let changed = |after: &str| distribute(&pieces, text, after);
        assert!(changed("受注管理\nNo 12").unwrap().is_empty());
        // Within one string, and typed over several: the first takes it.
        assert_eq!(
            changed("受注管理\nNo 34").unwrap(),
            BTreeMap::from([(4, "34".to_owned())])
        );
        assert_eq!(
            changed("販売管理\nNo 12").unwrap(),
            BTreeMap::from([(0, "販売".to_owned()), (1, String::new())])
        );
        assert_eq!(
            changed("受注管理部\nNo 12").unwrap(),
            BTreeMap::from([(2, "管理部".to_owned())])
        );
        assert_eq!(
            changed("\nNo 12").unwrap(),
            BTreeMap::from([(0, String::new()), (1, String::new()), (2, String::new())])
        );
        // The line break and the space are where the page places strings.
        for after in ["受注管理 No 12", "受注管理\nNo12"] {
            let error = changed(after).unwrap_err().to_string();
            assert!(error.contains("space or line break"), "{error}");
        }
    }
}
