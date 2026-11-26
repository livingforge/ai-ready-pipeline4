//! The figures a PDF page draws with lines and areas rather than as an
//! image: the shapes of Word, Excel and PowerPoint, a chart, an SVG picture
//! printed from a browser. A PDF keeps no figure, only the paths that paint
//! it among the rulings and shading of the page's tables, so the figures are
//! found here from where the paths lie, and each is written as an SVG
//! picture of its own, an asset beside the page's images.
//!
//! Paths that touch, or lie a little apart, are one group. A group that
//! curves or slants is a figure, as rulings and shading do neither; one of
//! level and upright lines and areas alone is a figure (a bar chart, boxes
//! joined by lines) when it has three or more of them, is not what the
//! page's sheet reads as a table and is more than a frame. Rulings alone
//! whose cells cover the group are a table with no text, and areas that
//! fill the group around a table of the sheet are the shading of a table
//! without rulings: neither is a figure. Three or more boxes that stand alone,
//! with nothing drawn in them and no box along their sides, are the boxes of
//! a diagram whatever the sheet reads of them, as are blocks of boxes that a
//! line joins (the entities of a data model). A slanted line from one corner of
//! a ruled cell to another strikes the cell out and is of its table. Where a table
//! holds drawings far smaller than itself, each is a figure and the table is
//! not. A mark drawn on one of the page's images is a figure with the image,
//! what a figure is drawn over is of it, and a hollow box over half of the
//! page borders the page's content rather than a figure.
//!
//! A figure is drawn as the page draws it within its box: the paths, the
//! strings of text (those of one cell of the page's sheet as one text, so
//! that the picture's text reads as the sheet does) and the page's images,
//! named by their asset files. Its `viewBox` is where it lies on the page,
//! in points from the page's top left. A gradient is painted in the colour
//! it starts from, and what a path of the page does not say is not drawn:
//! clipping, dashes and opacity, as in the view of a PDF page; text not
//! written along a level line is left out.

use super::{ImageAssets, PdfPage, PdfPath, page_layout};
use crate::project::AssetLimits;
use anyhow::Result;
use lopdf::Document as Pdf;
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write,
};

/// How far apart, in points, paths of one figure may lie.
const GAP: f64 = 6.0;
/// The shortest side of a figure, in points: smaller drawings are bullets,
/// check marks and icons set in text.
const MIN_SIDE: f64 = 16.0;
/// The fewest level and upright lines and areas that are a figure.
const MIN_SHAPES: usize = 3;
/// The most paths of a page figures are looked for among.
const MAX_SHAPES: usize = 50_000;
/// The share of a group the page's tables cover, or of its paths that lie
/// in them, from which it is theirs.
const TABLE_SHARE: f64 = 0.5;
/// The share of a group lying in tables its curved and slanted paths cover
/// below which they are drawings set in the tables.
const DRAWING_SHARE: f64 = 0.15;
/// How far, in points, from a figure's paths a string may lie and be its
/// text.
const TEXT_REACH: f64 = 3.0;
/// The fewest boxes standing alone that are the boxes of a diagram.
const BOXES: usize = 3;
/// The most boxes of a group looked at for those standing alone.
const MAX_BOXES: usize = 2000;
/// The share of a grid its closed cells cover from which it is a table.
const CLOSED: f64 = 0.6;
/// The share of a group its filled areas cover from which they shade it.
const SHADED: f64 = 0.85;
/// The share of the page's area from which a hollow box borders the page's
/// content rather than a figure.
const PAGE_FRAME: f64 = 0.5;
/// The thickest area, in points, that is a line.
const LINE: f64 = 3.0;
/// How far, in points, the end of a line may lie from the rulings whose
/// corner it is at.
const CORNER: f64 = 3.0;
/// The share of the page's width and height from which a path or an image
/// is the page's background.
const BACKGROUND: f64 = 0.9;

/// What a path draws.
#[derive(Clone, Copy)]
enum Kind {
    /// Level and upright lines and the areas between them.
    Straight,
    /// One slanted line, from one point to another.
    Slant([(f64, f64); 2]),
    /// Curves, and outlines with slanted sides.
    Other,
}

/// A path of a page: its index among the page's paths, the box it lies in
/// (`[x0, y0, x1, y1]`), what it draws, whether it shows (it is painted in
/// a colour other than white), whether it is hollow (no colour fills it)
/// and whether it is a box: one rectangle wider and higher than a line.
struct Shape {
    path: usize,
    bounds: [f64; 4],
    kind: Kind,
    seen: bool,
    hollow: bool,
    boxed: bool,
}

fn area(b: &[f64; 4]) -> f64 {
    (b[2] - b[0]).max(0.0) * (b[3] - b[1]).max(0.0)
}

fn union(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
    [
        a[0].min(b[0]),
        a[1].min(b[1]),
        a[2].max(b[2]),
        a[3].max(b[3]),
    ]
}

/// Whether `a` lies in `b` grown by `pad`.
fn inside(a: &[f64; 4], b: &[f64; 4], pad: f64) -> bool {
    a[0] >= b[0] - pad && a[1] >= b[1] - pad && a[2] <= b[2] + pad && a[3] <= b[3] + pad
}

/// Whether `a` and `b` are one outline: an area and the line around it.
fn alike(a: &[f64; 4], b: &[f64; 4]) -> bool {
    a.iter().zip(b).all(|(a, b)| (a - b).abs() <= 1.5)
}

/// Whether the boxes `a` and `b` stand side by side, no further apart than
/// the paths of a figure and along half or more of the shorter of their
/// sides, as the cells of a table and the bars of a chart do.
fn beside(a: &[f64; 4], b: &[f64; 4]) -> bool {
    let along = |axis: usize| {
        let shared = a[axis + 2].min(b[axis + 2]) - a[axis].max(b[axis]);
        shared >= 0.5 * (a[axis + 2] - a[axis]).min(b[axis + 2] - b[axis])
    };
    let meets =
        |axis: usize| (a[axis + 2] - b[axis]).abs() <= GAP || (b[axis + 2] - a[axis]).abs() <= GAP;
    (meets(0) && along(1)) || (meets(1) && along(0))
}

/// Whether the boxes `a` and `b` stand one on the other with sides of one
/// length: the parts of one block.
fn stacked(a: &[f64; 4], b: &[f64; 4]) -> bool {
    (0..2).any(|axis| {
        let across = 1 - axis;
        ((a[axis + 2] - b[axis]).abs() <= 1.5 || (b[axis + 2] - a[axis]).abs() <= 1.5)
            && (a[across] - b[across]).abs() <= LINE
            && (a[across + 2] - b[across + 2]).abs() <= LINE
    })
}

/// Whether `a` and `b` lie within `gap` of one another.
fn near(a: &[f64; 4], b: &[f64; 4], gap: f64) -> bool {
    a[0] <= b[2] + gap && b[0] <= a[2] + gap && a[1] <= b[3] + gap && b[1] <= a[3] + gap
}

/// The shapes of `paths` on a page `width` by `height`, without its
/// background and the paths that draw nothing.
fn shapes(paths: &[PdfPath], width: f64, height: f64) -> Vec<Shape> {
    let seen = |color: &Option<String>| color.as_deref().is_some_and(|c| c != "FFFFFF");
    let mut shapes = vec![];
    for (path, drawn) in paths.iter().enumerate() {
        let subpaths = page_layout::subpaths(&drawn.outline);
        let mut points = subpaths.iter().flat_map(|(points, ..)| points).copied();
        let Some((x, y)) = points.next() else {
            continue;
        };
        let half = if drawn.stroke.is_some() {
            drawn.width / 2.0
        } else {
            0.0
        };
        let bounds = points.fold([x, y, x, y], |b, (x, y)| union(b, [x, y, x, y]));
        let boxed = page_layout::boxed(drawn).is_some();
        let bounds = [
            bounds[0] - half,
            bounds[1] - half,
            bounds[2] + half,
            bounds[3] + half,
        ];
        if bounds[2] - bounds[0] >= BACKGROUND * width
            && bounds[3] - bounds[1] >= BACKGROUND * height
        {
            continue;
        }
        let slanted =
            |(a, b): ((f64, f64), (f64, f64))| (a.0 - b.0).abs() > 0.6 && (a.1 - b.1).abs() > 0.6;
        let curved = subpaths.iter().any(|(.., curved)| *curved);
        let slants = subpaths.iter().any(|(points, closed, _)| {
            let closing = closed
                .then(|| points.last().copied().zip(points.first().copied()))
                .flatten();
            points
                .windows(2)
                .map(|pair| (pair[0], pair[1]))
                .chain(closing)
                .any(slanted)
        });
        let kind = match (curved, slants, &subpaths[..]) {
            (false, false, _) => Kind::Straight,
            (false, true, [(points, closed, _)]) => match line(points, *closed, drawn) {
                Some(ends) => Kind::Slant(ends),
                None => Kind::Other,
            },
            _ => Kind::Other,
        };
        shapes.push(Shape {
            path,
            bounds,
            kind,
            seen: seen(&drawn.fill) || seen(&drawn.stroke),
            hollow: !seen(&drawn.fill),
            boxed,
        });
    }
    shapes
}

/// The ends of the line the outline `points` of `path` draws, if it draws
/// one: a line stroked from one point to another, or an area as thin as a
/// line, as programs that draw a line by its outline write one.
fn line(points: &[(f64, f64)], closed: bool, path: &PdfPath) -> Option<[(f64, f64); 2]> {
    if let ([from, to], false, None) = (points, closed, &path.fill) {
        return Some([*from, *to]);
    }
    if path.fill.is_none() || points.len() < 3 || points.len() > 8 {
        return None;
    }
    // Its ends are the points furthest apart, and it is as thick as its
    // area is to its length.
    let apart = |a: &(f64, f64), b: &(f64, f64)| (a.0 - b.0).hypot(a.1 - b.1);
    let (from, to) = points
        .iter()
        .flat_map(|a| points.iter().map(move |b| (a, b)))
        .max_by(|a, b| apart(a.0, a.1).total_cmp(&apart(b.0, b.1)))?;
    let length = apart(from, to);
    let twice: f64 = points
        .iter()
        .zip(points.iter().cycle().skip(1))
        .map(|(a, b)| a.0 * b.1 - b.0 * a.1)
        .sum();
    let thick = twice.abs() / 2.0 / length.max(f64::MIN_POSITIVE);
    (thick <= LINE && length >= 4.0 * thick).then_some([*from, *to])
}

/// The `boxes` in groups of those within [`GAP`] of one another, each by
/// the indices of its boxes.
fn groups(boxes: &[[f64; 4]]) -> Vec<Vec<usize>> {
    let mut order: Vec<usize> = (0..boxes.len()).collect();
    order.sort_by(|a, b| boxes[*a][0].total_cmp(&boxes[*b][0]));
    let mut parent: Vec<usize> = (0..boxes.len()).collect();
    fn root(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    for (n, &i) in order.iter().enumerate() {
        for &j in &order[n + 1..] {
            if boxes[j][0] > boxes[i][2] + GAP {
                break;
            }
            if near(&boxes[i], &boxes[j], GAP) {
                let (a, b) = (root(&mut parent, i), root(&mut parent, j));
                parent[a.max(b)] = a.min(b);
            }
        }
    }
    let mut sets: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for i in 0..boxes.len() {
        sets.entry(root(&mut parent, i)).or_default().push(i);
    }
    sets.into_values().collect()
}

/// The rulings of a page, level and upright, each in the order of where
/// they lie.
struct Rules {
    level: Vec<page_layout::Rule>,
    upright: Vec<page_layout::Rule>,
    /// The grids the rulings make, with the share of each its closed cells
    /// cover.
    grids: Vec<([f64; 4], f64)>,
}

impl Rules {
    fn of(paths: &[PdfPath]) -> Self {
        let (mut level, mut upright): (Vec<_>, Vec<_>) = page_layout::rulings(paths)
            .0
            .into_iter()
            .partition(|rule| rule.level);
        level.sort_by(|a, b| a.at.total_cmp(&b.at));
        upright.sort_by(|a, b| a.at.total_cmp(&b.at));
        Self {
            level,
            upright,
            grids: page_layout::grids(paths),
        }
    }

    /// Whether a ruling of `rules` at `at` runs past `along`.
    fn on(rules: &[page_layout::Rule], at: f64, along: f64) -> bool {
        rules[rules.partition_point(|r| r.at < at - CORNER)..]
            .iter()
            .take_while(|r| r.at <= at + CORNER)
            .any(|r| r.from - CORNER <= along && along <= r.to + CORNER)
    }

    /// Whether a point is a corner of a ruled cell: on a level and on an
    /// upright ruling.
    fn corner(&self, (x, y): (f64, f64)) -> bool {
        Self::on(&self.level, y, x) && Self::on(&self.upright, x, y)
    }

    /// Whether a ruling along a side of `rect` runs on past a corner of it,
    /// as the rulings of a table run on along the cells beside one. The
    /// side of another box a little off the line is not such a ruling.
    fn beyond(&self, rect: &[f64; 4]) -> bool {
        let runs = |rules: &[page_layout::Rule], at: f64, along: f64| {
            rules[rules.partition_point(|r| r.at < at - 1.0)..]
                .iter()
                .take_while(|r| r.at <= at + 1.0)
                .any(|r| r.from <= along && along <= r.to)
        };
        let past = CORNER + 1.0;
        [rect[1], rect[3]]
            .into_iter()
            .any(|y| runs(&self.level, y, rect[0] - past) || runs(&self.level, y, rect[2] + past))
            || [rect[0], rect[2]].into_iter().any(|x| {
                runs(&self.upright, x, rect[1] - past) || runs(&self.upright, x, rect[3] + past)
            })
    }
}

/// A figure of a page: the box its paths lie in, whether it is a group of
/// paths whole (rather than a drawing set in a table) and whether it is a
/// diagram of boxes.
struct Figure {
    bounds: [f64; 4],
    whole: bool,
    diagram: bool,
}

/// The boxes of the figures among `shapes` on a page ruled by `rules` whose
/// sheet has the tables `tables`, which draws images over the boxes
/// `images` and whose area is `page`: the larger first, none overlapping
/// another.
fn figure_boxes(
    shapes: &[Shape],
    rules: &Rules,
    tables: &[[f64; 4]],
    images: &[[f64; 4]],
    sure: &[[f64; 4]],
    page: f64,
) -> Vec<Figure> {
    let seen: Vec<&Shape> = shapes.iter().filter(|s| s.seen).collect();
    if seen.is_empty() || seen.len() > MAX_SHAPES {
        return vec![];
    }
    // The groups that are figures whole, each with whether it is a diagram
    // of boxes.
    let mut wholes: Vec<([f64; 4], bool)> = vec![];
    let boxes: Vec<[f64; 4]> = seen.iter().map(|s| s.bounds).collect();
    let whole = |members: &[usize]| {
        members
            .iter()
            .map(|i| boxes[*i])
            .reduce(union)
            .unwrap_or([0.0; 4])
    };
    // A hollow box over much of the page borders the page's content. It
    // is left out, as it would make one group of all it holds: a figure of
    // the page. So are the rulings and shading of a table beyond doubt: a
    // figure drawn against the table is a figure without it.
    let plain: Vec<usize> = (0..seen.len())
        .filter(|i| {
            let straight = matches!(seen[*i].kind, Kind::Straight);
            !(straight
                && ((seen[*i].hollow && area(&boxes[*i]) >= PAGE_FRAME * page)
                    || sure.iter().any(|table| inside(&boxes[*i], table, 2.0))))
        })
        .collect();
    let plain_boxes: Vec<[f64; 4]> = plain.iter().map(|i| boxes[*i]).collect();
    let mut found = vec![];
    // The groups that are neither figures nor of the tables.
    let mut loose = vec![];
    for members in groups(&plain_boxes) {
        let members: Vec<usize> = members.into_iter().map(|i| plain[i]).collect();
        let bounds = whole(&members);
        // How much of the group the tables cover, and how many of its
        // paths lie in them: a frame around tables is theirs, though they
        // cover little of it.
        let within = members
            .iter()
            .filter(|i| {
                let b = &boxes[**i];
                let centre = [(b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0];
                tables
                    .iter()
                    .any(|t| inside(&[centre[0], centre[1], centre[0], centre[1]], t, 2.0))
            })
            .count();
        let covered = if area(&bounds) > 0.0 {
            tables
                .iter()
                .map(|t| {
                    area(&[
                        bounds[0].max(t[0]),
                        bounds[1].max(t[1]),
                        bounds[2].min(t[2]),
                        bounds[3].min(t[3]),
                    ])
                })
                .sum::<f64>()
                / area(&bounds)
        } else {
            0.0
        };
        let tabled = covered >= TABLE_SHARE || within as f64 >= TABLE_SHARE * members.len() as f64;
        // What is not a ruling, shading or a line across a cell.
        let odd: Vec<usize> = members
            .iter()
            .copied()
            .filter(|i| match seen[*i].kind {
                Kind::Straight => false,
                Kind::Slant([from, to]) => !(rules.corner(from) && rules.corner(to)),
                Kind::Other => true,
            })
            .collect();
        // An area and the line around it are two paths of one outline.
        let mut outlines: Vec<[f64; 4]> = vec![];
        let mut rects: Vec<[f64; 4]> = vec![];
        for i in &members {
            if !outlines.iter().any(|o| alike(o, &boxes[*i])) {
                outlines.push(boxes[*i]);
                if seen[*i].boxed {
                    rects.push(boxes[*i]);
                }
            }
        }
        // Boxes that stand alone, with nothing drawn in them, no box along
        // their sides and no ruling running on past their corners, are the
        // boxes of a diagram (an organization chart, the fields of a
        // screen): the cells of a table stand side by side between
        // rulings that run the length of the table.
        let standing = if rects.len() <= MAX_BOXES {
            rects
                .iter()
                .filter(|rect| {
                    !outlines
                        .iter()
                        .any(|other| !alike(other, rect) && inside(other, rect, 0.0))
                        && !rects
                            .iter()
                            .any(|other| !alike(other, rect) && beside(rect, other))
                        && !rules.beyond(rect)
                })
                .count()
        } else {
            0
        };
        // Boxes one on another, as wide or as high as one another, are one
        // block (an entity of a diagram, its name over its columns). Blocks that stand alone and
        // that a line joins, ending at a side of each, are a diagram too.
        let joined = rects.len() >= 2 && rects.len() <= MAX_BOXES && {
            let mut block: Vec<usize> = (0..rects.len()).collect();
            fn root(block: &mut [usize], mut i: usize) -> usize {
                while block[i] != i {
                    block[i] = block[block[i]];
                    i = block[i];
                }
                i
            }
            for a in 0..rects.len() {
                for b in a + 1..rects.len() {
                    if stacked(&rects[a], &rects[b]) {
                        let (a, b) = (root(&mut block, a), root(&mut block, b));
                        block[a.max(b)] = a.min(b);
                    }
                }
            }
            let mut blocks: BTreeMap<usize, [f64; 4]> = BTreeMap::new();
            for (i, rect) in rects.iter().enumerate() {
                let bounds = blocks.entry(root(&mut block, i)).or_insert(*rect);
                *bounds = union(*bounds, *rect);
            }
            let all: Vec<[f64; 4]> = blocks.into_values().collect();
            // A box in a block is a part of it (the frame of a name).
            let blocks: Vec<[f64; 4]> = all
                .iter()
                .filter(|block| {
                    !rules.beyond(block)
                        && !all
                            .iter()
                            .any(|other| !alike(other, block) && inside(block, other, 0.0))
                })
                .copied()
                .collect();
            // A level line ends at the left or right side of a block,
            // between its corners; an upright one at its top or bottom.
            let ends = |line: &[f64; 4], block: &[f64; 4]| {
                let axis = usize::from(line[2] - line[0] < line[3] - line[1]);
                let across = 1 - axis;
                let middle = (line[across] + line[across + 2]) / 2.0;
                middle > block[across] + CORNER
                    && middle < block[across + 2] - CORNER
                    && ((line[axis + 2] - block[axis]).abs() <= 2.0
                        || (line[axis] - block[axis + 2]).abs() <= 2.0)
            };
            blocks.len() >= 2
                && outlines
                    .iter()
                    .filter(|line| (line[2] - line[0]).min(line[3] - line[1]) <= LINE)
                    .any(|line| blocks.iter().filter(|block| ends(line, block)).count() >= 2)
        };
        let diagram = standing >= BOXES || joined;
        if odd.is_empty() {
            // Lines along the sides of the group alone are a frame around
            // what the page shows in it, not a figure.
            let frame = members.iter().all(|i| {
                let b = &boxes[*i];
                let thin = (b[2] - b[0]).min(b[3] - b[1]) <= LINE;
                let side = b[2] <= bounds[0] + 2.0 * GAP
                    || b[0] >= bounds[2] - 2.0 * GAP
                    || b[3] <= bounds[1] + 2.0 * GAP
                    || b[1] >= bounds[3] - 2.0 * GAP;
                (thin && side) || (seen[*i].hollow && inside(&bounds, b, 2.0 * GAP))
            });
            // A mark drawn on a picture (a box around a part of a screen)
            // is a figure with it; the picture's own outline is not.
            let marks = images
                .iter()
                .any(|image| near(&bounds, image, 0.0) && !inside(image, &bounds, 2.0));
            // Rulings alone whose cells cover the group are a table with
            // no text in it, which the sheet does not have.
            let ruled = members.iter().all(|i| {
                let b = &boxes[*i];
                seen[*i].hollow || (b[2] - b[0]).min(b[3] - b[1]) <= LINE
            }) && rules.grids.iter().any(|(grid, closed)| {
                let shared = [
                    bounds[0].max(grid[0]),
                    bounds[1].max(grid[1]),
                    bounds[2].min(grid[2]),
                    bounds[3].min(grid[3]),
                ];
                *closed >= CLOSED && area(&shared) >= TABLE_SHARE * area(&bounds)
            });
            // Areas that fill the group around a table of the sheet shade
            // its cells: a table without rulings, whose place the sheet
            // knows only by its text.
            let filled: f64 = members
                .iter()
                .filter(|i| !seen[**i].hollow)
                .map(|i| area(&boxes[*i]))
                .sum();
            let shading =
                filled >= SHADED * area(&bounds) && tables.iter().any(|t| inside(t, &bounds, 2.0));
            let diagram = diagram && !ruled && !shading;
            let tabled = tabled || ruled || shading;
            if diagram || (!tabled && ((outlines.len() >= MIN_SHAPES && !frame) || marks)) {
                found.push(bounds);
                wholes.push((bounds, diagram));
            } else if !tabled {
                loose.push(bounds);
            }
        } else if tabled && !diagram && area(&whole(&odd)) < DRAWING_SHARE * area(&bounds) {
            let drawn: Vec<[f64; 4]> = odd.iter().map(|i| boxes[*i]).collect();
            found.extend(
                groups(&drawn)
                    .into_iter()
                    .map(|members| members.iter().map(|i| drawn[*i]).reduce(union).unwrap()),
            );
        } else {
            found.push(bounds);
            wholes.push((bounds, diagram));
        }
    }
    // A figure drawn on an image is of the image whole: the marks on one
    // picture are one figure.
    for bounds in &mut found {
        for image in images {
            if near(bounds, image, 0.0) {
                *bounds = union(*bounds, *image);
            }
        }
    }
    found.retain(|b| b[2] - b[0] >= MIN_SIDE && b[3] - b[1] >= MIN_SIDE);
    // What a figure is drawn over is of it: a figure it overlaps, and a
    // frame or a line no larger than it that is no figure alone (the frame
    // of a note set among its shapes).
    loop {
        let mut joined = false;
        'figures: for i in 0..found.len() {
            for j in i + 1..found.len() {
                if near(&found[i], &found[j], 0.0) {
                    found[i] = union(found[i], found[j]);
                    found.remove(j);
                    joined = true;
                    break 'figures;
                }
            }
            if let Some(at) = loose
                .iter()
                .position(|b| near(&found[i], b, 0.0) && area(b) <= area(&found[i]))
            {
                found[i] = union(found[i], loose.remove(at));
                joined = true;
                break;
            }
        }
        if !joined {
            break;
        }
    }
    found.sort_by(|a, b| area(b).total_cmp(&area(a)));
    found
        .into_iter()
        .map(|bounds| {
            let mut parts = wholes.iter().filter(|(part, _)| inside(part, &bounds, 1.0));
            Figure {
                bounds,
                whole: parts.clone().next().is_some(),
                diagram: parts.any(|(_, diagram)| *diagram),
            }
        })
        .collect()
}

/// The boxes of the figures a page `width` by `height` draws with `paths`,
/// each with whether it is a diagram of boxes, when the rulings and text of
/// the page would make the tables `tables`, `sure` those of them beyond
/// doubt: the figures whose lines rule no
/// table of the page's sheet. A drawing set in a table is not among them,
/// as the table is a table.
pub(super) fn regions(
    paths: &[PdfPath],
    width: f64,
    height: f64,
    tables: &[[f64; 4]],
    sure: &[[f64; 4]],
) -> Vec<([f64; 4], bool)> {
    if paths.is_empty() {
        return vec![];
    }
    let shapes = shapes(paths, width, height);
    figure_boxes(
        &shapes,
        &Rules::of(paths),
        tables,
        &[],
        sure,
        width * height,
    )
    .into_iter()
    .filter(|figure| figure.whole)
    .map(|figure| (figure.bounds, figure.diagram))
    .collect()
}

/// Whether a page `width` by `height` painting `paths` may draw a figure,
/// whatever its tables are; one that draws images may with any path.
fn drawn(paths: &[PdfPath], width: f64, height: f64, images: bool) -> bool {
    let shapes = shapes(paths, width, height);
    (images && shapes.iter().any(|s| s.seen))
        || !figure_boxes(&shapes, &Rules::of(paths), &[], &[], &[], width * height).is_empty()
}

/// `text` as the content of an XML element or attribute.
fn xml(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            // What XML cannot hold.
            c if (c < ' ' && !matches!(c, '\t' | '\n' | '\r'))
                || matches!(c, '\u{fffe}' | '\u{ffff}') => {}
            c => out.push(c),
        }
    }
    out
}

/// The box an image's SVG matrix maps its unit square onto.
fn image_box([a, b, c, d, e, f]: [f64; 6]) -> [f64; 4] {
    let xs = [e, a + e, c + e, a + c + e];
    let ys = [f, b + f, d + f, b + d + f];
    [
        xs.into_iter().fold(f64::INFINITY, f64::min),
        ys.into_iter().fold(f64::INFINITY, f64::min),
        xs.into_iter().fold(f64::NEG_INFINITY, f64::max),
        ys.into_iter().fold(f64::NEG_INFINITY, f64::max),
    ]
}

/// The figures of `page` as SVG pictures, in the order of the page (down,
/// then across), and how many were left out for their shape by `limits`.
/// `files` gives the asset file of each image saved, by its name without
/// the extension.
pub(super) fn figures(
    page: &PdfPage,
    files: &BTreeMap<String, String>,
    limits: &AssetLimits,
) -> (Vec<String>, usize) {
    let shapes = shapes(&page.paths, page.width, page.height);
    // The images saved, other than a page's background, with the box each
    // is drawn over.
    let shown: Vec<(&str, [f64; 6], [f64; 4])> = page
        .images
        .iter()
        .filter_map(|image| {
            let covered = image_box(image.matrix);
            let background = covered[2] - covered[0] >= BACKGROUND * page.width
                && covered[3] - covered[1] >= BACKGROUND * page.height;
            (!image.mask && !background)
                .then(|| Some((files.get(&image.asset)?.as_str(), image.matrix, covered)))
                .flatten()
        })
        .collect();
    let covered: Vec<[f64; 4]> = shown.iter().map(|(.., covered)| *covered).collect();
    let mut boxes: Vec<[f64; 4]> = figure_boxes(
        &shapes,
        &Rules::of(&page.paths),
        &page.tables,
        &covered,
        &page.sure_tables,
        page.width * page.height,
    )
    .into_iter()
    .map(|figure| figure.bounds)
    .collect();
    boxes.sort_by(|a, b| a[1].total_cmp(&b[1]).then(a[0].total_cmp(&b[0])));
    let mut pictures = vec![];
    let mut slender = 0;
    for drawn in boxes {
        let mut bounds = drawn;
        // The strings by the figure's paths, with the box each is written in.
        let mut strings = BTreeSet::new();
        for (i, [x, y, width]) in page.places.iter().copied().enumerate() {
            if page.texts[i].trim().is_empty()
                || page.invisible.contains(&i)
                || page.turned.contains(&i)
            {
                continue;
            }
            let size = page.fonts[i]["size"].as_f64().unwrap_or(10.0);
            let written = [x, y - size, x + width.max(0.0), y + 0.25 * size];
            // The text of a table beside the figure is the table's.
            let tabled = page.sure_tables.iter().any(|t| {
                (t[0]..=t[2]).contains(&(x + 0.25 * size))
                    && (t[1]..=t[3]).contains(&(y - 0.3 * size))
            });
            if !tabled && near(&written, &drawn, TEXT_REACH) {
                strings.insert(i);
                bounds = union(bounds, written);
            }
        }
        let images: Vec<(&str, [f64; 6])> = shown
            .iter()
            .filter(|(.., covered)| near(covered, &drawn, 0.0))
            .map(|(file, matrix, _)| (*file, *matrix))
            .collect();
        for (_, matrix) in &images {
            bounds = union(bounds, image_box(*matrix));
        }
        let bounds = [
            (bounds[0] - 1.0).max(0.0),
            (bounds[1] - 1.0).max(0.0),
            (bounds[2] + 1.0).min(page.width),
            (bounds[3] + 1.0).min(page.height),
        ];
        let (width, height) = (bounds[2] - bounds[0], bounds[3] - bounds[1]);
        if width <= 0.0 || height <= 0.0 {
            continue;
        }
        if limits.slender(width, height) {
            slender += 1;
            continue;
        }
        let mut svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width:.2}" height="{height:.2}" viewBox="{:.2} {:.2} {width:.2} {height:.2}">"#,
            bounds[0], bounds[1]
        );
        for (file, matrix) in images {
            // Adding 0 writes a negative zero as 0.
            let [a, b, c, d, e, f] = matrix.map(|n| n + 0.0);
            let _ = write!(
                svg,
                r#"<image href="{}" width="1" height="1" preserveAspectRatio="none" transform="matrix({a:.4} {b:.4} {c:.4} {d:.4} {e:.4} {f:.4})"/>"#,
                xml(file)
            );
        }
        for shape in shapes.iter().filter(|s| inside(&s.bounds, &drawn, 0.5)) {
            let path = &page.paths[shape.path];
            let color =
                |c: &Option<String>| c.as_deref().map_or("none".to_owned(), |c| format!("#{c}"));
            let _ = write!(
                svg,
                r#"<path d="{}" fill="{}"{} stroke="{}" stroke-width="{:.2}"/>"#,
                path.outline,
                color(&path.fill),
                if path.even_odd {
                    r#" fill-rule="evenodd""#
                } else {
                    ""
                },
                color(&path.stroke),
                path.width
            );
        }
        // The strings of a cell of the page's sheet are one text, set apart
        // as the cell's text sets them; a string of no cell is its own.
        let mut written = BTreeSet::new();
        let text = |svg: &mut String, pieces: &[(usize, &str)]| {
            svg.push_str("<text>");
            for (n, (string, before)) in pieces.iter().enumerate() {
                if n > 0 {
                    svg.push_str(if before.is_empty() { "" } else { " " });
                }
                let [x, y, width] = page.places[*string];
                let font = &page.fonts[*string];
                let size = font["size"].as_f64().unwrap_or(10.0);
                let mut style = crate::fonts::pdf_css_families(font);
                if let Some(rgb) = font["color"]
                    .as_str()
                    .filter(|c| c.len() == 6 && c.bytes().all(|b| b.is_ascii_hexdigit()))
                {
                    let _ = write!(style, "fill:#{rgb};");
                }
                let fit = if width > 0.0 {
                    format!(r#" textLength="{width:.2}" lengthAdjust="spacingAndGlyphs""#)
                } else {
                    String::new()
                };
                let _ = write!(
                    svg,
                    r#"<tspan x="{x:.2}" y="{y:.2}" font-size="{size:.2}" style="{}"{fit}>{}</tspan>"#,
                    xml(&style),
                    xml(&page.texts[*string])
                );
            }
            svg.push_str("</text>");
        };
        for cell in &page.cells {
            let pieces: Vec<(usize, &str)> = cell
                .iter()
                .filter(|piece| strings.contains(&piece.string))
                .map(|piece| (piece.string, piece.before))
                .collect();
            if !pieces.is_empty() {
                written.extend(pieces.iter().map(|(string, _)| *string));
                text(&mut svg, &pieces);
            }
        }
        for string in strings.difference(&written) {
            text(&mut svg, &[(*string, "")]);
        }
        svg.push_str("</svg>");
        pictures.push(svg);
    }
    (pictures, slender)
}

/// The asset name of the `n`th figure of a document, from 0.
fn name(n: usize) -> String {
    format!("figure-{:03}.svg", n + 1)
}

/// Adds the figures the pages of `doc` draw to `images`, which holds the
/// files of the images they draw as `placed` places them: each figure once,
/// with the pages that draw it alike in the same place. The figures left
/// out for their shape by `limits` are counted in `images.slender`.
pub(super) fn read(
    doc: &Pdf,
    images: &mut ImageAssets,
    placed: super::pdf_images::PagePlacements,
    limits: &AssetLimits,
) -> Result<()> {
    let files: BTreeMap<String, String> = images
        .files
        .keys()
        .filter_map(|name| Some((name.rsplit_once('.')?.0.to_owned(), name.clone())))
        .collect();
    let mapped = super::pdf_mapped_fonts(doc);
    let mut fonts = super::PdfFonts::new(&mapped);
    let mut tags = None;
    let mut placed = placed.into_iter();
    let mut names: BTreeMap<String, String> = BTreeMap::new();
    for (number, id) in doc.get_pages() {
        let placements = placed
            .next()
            .map(|(_, placements)| placements)
            .unwrap_or_default();
        // Most pages draw no figure: their text is not read again.
        let [x0, y0, x1, y1] = super::pdf_page_box(doc, id);
        let raw = doc.get_page_content_with_limit(id, super::MAX_PAGE)?;
        let (content, _) = super::pdf_content(&raw)?;
        let paths = super::pdf_paths(&content, [x0, y1], &super::pdf_pattern_colors(doc, id));
        if !drawn(&paths, x1 - x0, y1 - y0, !placements.is_empty()) {
            continue;
        }
        let tags = tags.get_or_insert_with(|| super::pdf_tags::read(doc));
        let page = super::pdf_page(doc, id, &mut fonts, tags, placements)?;
        let (pictures, slender) = figures(&page, &files, limits);
        if slender > 0 {
            *images.slender.entry(number).or_default() += slender;
        }
        for svg in pictures {
            let next = name(names.len());
            let name = names.entry(svg).or_insert(next).clone();
            let pages = images.pages.entry(name).or_default();
            if pages.last() != Some(&number) {
                pages.push(number);
            }
        }
    }
    for (svg, name) in names {
        images.files.insert(name, svg.into_bytes());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(outline: &str, fill: Option<&str>, stroke: Option<&str>) -> PdfPath {
        PdfPath {
            outline: outline.to_owned(),
            fill: fill.map(str::to_owned),
            even_odd: false,
            stroke: stroke.map(str::to_owned),
            width: 1.0,
        }
    }

    /// A filled rectangle.
    fn filled(x: f64, y: f64, width: f64, height: f64) -> PdfPath {
        let (right, bottom) = (x + width, y + height);
        path(
            &format!("M{x},{y}L{right},{y} {right},{bottom} {x},{bottom}Z"),
            Some("336699"),
            None,
        )
    }

    /// A stroked line.
    fn stroked(from: (f64, f64), to: (f64, f64)) -> PdfPath {
        path(
            &format!("M{},{}L{},{}", from.0, from.1, to.0, to.1),
            None,
            Some("000000"),
        )
    }

    /// A page 600 by 800 painting `paths`, whose sheet has `tables`.
    fn page(paths: Vec<PdfPath>, tables: Vec<[f64; 4]>) -> PdfPage {
        PdfPage {
            width: 600.0,
            height: 800.0,
            paths,
            places: vec![],
            texts: vec![],
            fonts: vec![],
            cells: vec![],
            invisible: BTreeSet::new(),
            turned: BTreeSet::new(),
            images: vec![],
            tables,
            sure_tables: vec![],
        }
    }

    /// The view boxes of the figures of `page`, in whole points.
    fn boxes(page: &PdfPage) -> Vec<String> {
        let limits = AssetLimits {
            max_aspect_ratio: None,
        };
        figures(page, &BTreeMap::new(), &limits)
            .0
            .iter()
            .map(|svg| {
                let start = svg.find("viewBox=\"").unwrap() + 9;
                let end = start + svg[start..].find('"').unwrap();
                svg[start..end]
                    .split(' ')
                    .map(|n| format!("{:.0}", n.parse::<f64>().unwrap()))
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect()
    }

    /// The rulings of a table of two columns and two rows at `(100, 100)`,
    /// 200 wide and 60 high.
    fn ruled() -> Vec<PdfPath> {
        let mut paths = vec![];
        for y in [100.0, 130.0, 160.0] {
            paths.push(stroked((100.0, y), (300.0, y)));
        }
        for x in [100.0, 200.0, 300.0] {
            paths.push(stroked((x, 100.0), (x, 160.0)));
        }
        paths
    }

    #[test]
    fn bars_are_a_figure_and_a_frame_or_a_table_is_not() {
        let bars = |count: usize| -> Vec<PdfPath> {
            [
                filled(100.0, 100.0, 30.0, 80.0),
                filled(134.0, 120.0, 30.0, 60.0),
                filled(168.0, 140.0, 30.0, 40.0),
            ]
            .into_iter()
            .take(count)
            .collect()
        };
        assert_eq!(boxes(&page(bars(3), vec![])), ["99 99 100 82"]);
        // Two of them are too few.
        assert!(boxes(&page(bars(2), vec![])).is_empty());
        // What the sheet reads as a table is the table's.
        assert!(boxes(&page(bars(3), vec![[100.0, 100.0, 198.0, 180.0]])).is_empty());
        // Lines along the sides of a box frame what is in it.
        let frame = vec![
            filled(100.0, 100.0, 300.0, 2.0),
            filled(100.0, 298.0, 300.0, 2.0),
            filled(100.0, 100.0, 2.0, 200.0),
            filled(398.0, 100.0, 2.0, 200.0),
        ];
        assert!(boxes(&page(frame, vec![])).is_empty());
    }

    #[test]
    fn a_line_between_the_corners_of_a_cell_is_of_its_table() {
        let table = [100.0, 100.0, 300.0, 160.0];
        let mut struck = ruled();
        struck.push(stroked((100.0, 100.0), (200.0, 130.0)));
        // Excel draws the line by its outline, an area as thin as a line.
        struck.push(path(
            "M200,130L201.5,130 300,159.6 300,160 298.5,160 200,130.4Z",
            Some("000000"),
            None,
        ));
        assert!(boxes(&page(struck, vec![table])).is_empty());
        // A line that ends inside a cell is drawn on the table, and is a
        // figure with it when it is as large as the table.
        let mut drawn = ruled();
        drawn.push(stroked((110.0, 105.0), (290.0, 155.0)));
        assert_eq!(boxes(&page(drawn, vec![table])), ["98 98 203 63"]);
    }

    #[test]
    fn a_drawing_set_in_a_table_is_a_figure_without_the_table() {
        let mut paths = ruled();
        for y in [190.0, 220.0, 250.0, 280.0, 310.0, 340.0, 370.0, 400.0] {
            paths.push(stroked((100.0, y), (300.0, y)));
        }
        paths.push(stroked((100.0, 160.0), (100.0, 400.0)));
        paths.push(stroked((300.0, 160.0), (300.0, 400.0)));
        // A circle in a cell, and a mark too small to be a figure.
        paths.push(path(
            "M130,105C141,105 150,114 150,125C150,136 141,145 130,145C119,145 110,136 110,125C110,114 119,105 130,105Z",
            None,
            Some("CC0000"),
        ));
        paths.push(path(
            "M250,200C253,200 255,202 255,205C255,208 253,210 250,210Z",
            Some("CC0000"),
            None,
        ));
        let page = page(paths, vec![[100.0, 100.0, 300.0, 400.0]]);
        assert_eq!(boxes(&page), ["108 104 43 43"]);
    }

    #[test]
    fn a_figure_holds_its_paths_text_and_images_and_a_slender_one_is_left_out() {
        let mut page = page(
            vec![
                // The page's background, and a white box under a curve,
                // which lies within the points that bend it.
                filled(0.0, 0.0, 600.0, 800.0),
                path("M100,100L200,100 200,150 100,150Z", Some("FFFFFF"), None),
                path("M100,150C140,100 160,200 200,150", None, Some("0000FF")),
            ],
            vec![],
        );
        page.places = vec![
            [110.0, 130.0, 30.0],
            [110.0, 400.0, 30.0],
            [150.0, 130.0, 20.0],
        ];
        page.texts = vec!["A<B".into(), "body".into(), "turned".into()];
        page.fonts = vec![
            serde_json::json!({"latin":"Arial","east_asian":null,"size":10.0,"color":"FF0000"});
            3
        ];
        page.cells = vec![vec![super::super::PdfPiece {
            string: 0,
            before: "",
        }]];
        page.turned.insert(2);
        let image = |asset: &str| super::super::PdfImage {
            asset: asset.into(),
            matrix: [40.0, 0.0, 0.0, 40.0, 190.0, 110.0],
            mask: false,
        };
        // The second image was not saved.
        page.images = vec![image("image-001"), image("image-002")];
        let files = BTreeMap::from([("image-001".to_owned(), "image-001.png".to_owned())]);
        let (pictures, slender) = figures(&page, &files, &AssetLimits::default());
        assert_eq!(slender, 0);
        let [svg] = &pictures[..] else {
            panic!("{pictures:?}")
        };
        assert!(
            svg.starts_with(
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="132.50" height="103.00" viewBox="98.50 98.50 132.50 103.00">"#
            ),
            "{svg}"
        );
        // The image saved, the paths in the order of the page without its
        // background, and the text of the page's cell.
        assert_eq!(svg.matches("<image ").count(), 1);
        assert!(svg.contains(r#"<image href="image-001.png""#), "{svg}");
        assert_eq!(svg.matches("<path ").count(), 2);
        assert!(svg.find("#FFFFFF").unwrap() < svg.find("#0000FF").unwrap());
        assert!(
            svg.contains(
                r#"<text><tspan x="110.00" y="130.00" font-size="10.00" style="font-family:'Arial',sans-serif;fill:#FF0000;" textLength="30.00" lengthAdjust="spacingAndGlyphs">A&lt;B</tspan></text>"#
            ),
            "{svg}"
        );
        assert!(!svg.contains("body") && !svg.contains("turned"));
        assert_eq!(
            crate::ocr::recognize("figure-001.svg", svg.as_bytes())["text"],
            "A<B"
        );
        // It is more than 1.2 times as long as it is short.
        let limits = AssetLimits {
            max_aspect_ratio: Some(1.2),
        };
        assert_eq!(figures(&page, &files, &limits), (vec![], 1));
    }

    #[test]
    fn what_a_figure_is_drawn_over_is_of_it() {
        let curve = || path("M100,150C140,120 160,180 200,150", None, Some("0000FF"));
        // The frame of a note reaching out of the figure, and a figure
        // drawn over its corner, are one figure with it.
        let frame = path("M180,160L320,160 320,200 180,200Z", None, Some("000000"));
        let other = path("M90,110C95,100 105,100 110,130", None, Some("00AA00"));
        assert_eq!(
            boxes(&page(vec![curve(), frame, other], vec![])),
            ["88 98 233 103"]
        );
        // A frame apart from the figure is not, nor one around most of
        // the page.
        let apart = path("M300,160L420,160 420,200 300,200Z", None, Some("000000"));
        let around = path("M50,50L550,50 550,700 50,700Z", None, Some("000000"));
        assert_eq!(
            boxes(&page(vec![curve(), apart, around], vec![])),
            ["98 118 103 63"]
        );
    }

    #[test]
    fn a_mark_on_a_picture_is_a_figure_with_the_picture() {
        let picture = |paths: Vec<PdfPath>| {
            let mut page = page(paths, vec![]);
            page.images = vec![super::super::PdfImage {
                asset: "image-001".into(),
                matrix: [200.0, 0.0, 0.0, 100.0, 100.0, 100.0],
                mask: false,
            }];
            page
        };
        let files = BTreeMap::from([("image-001".to_owned(), "image-001.png".to_owned())]);
        let limits = AssetLimits::default();
        // A box around a part of the picture.
        let marked = picture(vec![path(
            "M150,120L220,120 220,160 150,160Z",
            None,
            Some("FF0000"),
        )]);
        let (pictures, _) = figures(&marked, &files, &limits);
        let [svg] = &pictures[..] else {
            panic!("{pictures:?}")
        };
        assert!(
            svg.contains(r#"viewBox="99.00 99.00 202.00 102.00""#),
            "{svg}"
        );
        assert!(svg.contains(r#"<image href="image-001.png""#) && svg.contains("#FF0000"));
        // The outline of the picture itself is no mark, and neither is a
        // box on a picture that was not saved.
        let outlined = picture(vec![path(
            "M100,100L300,100 300,200 100,200Z",
            None,
            Some("000000"),
        )]);
        assert!(figures(&outlined, &files, &limits).0.is_empty());
        assert!(figures(&marked, &BTreeMap::new(), &limits).0.is_empty());
    }

    /// A stroked rectangle with nothing filling it.
    fn outlined(x: f64, y: f64, width: f64, height: f64) -> PdfPath {
        let (right, bottom) = (x + width, y + height);
        path(
            &format!("M{x},{y}L{right},{y} {right},{bottom} {x},{bottom}Z"),
            None,
            Some("000000"),
        )
    }

    #[test]
    fn a_table_without_text_or_without_rulings_is_not_a_figure() {
        // Rulings whose cells cover them are a table though no cell holds
        // text, so that the sheet has no table there.
        assert!(boxes(&page(ruled(), vec![])).is_empty());
        // The shading of the cells of a table without rulings, wider than
        // the text the sheet knows the table by.
        let shaded = || {
            vec![
                filled(100.0, 100.0, 200.0, 30.0),
                filled(100.0, 131.0, 200.0, 30.0),
                filled(100.0, 162.0, 200.0, 30.0),
            ]
        };
        assert!(boxes(&page(shaded(), vec![[108.0, 108.0, 150.0, 184.0]])).is_empty());
        // Without a table of the sheet the areas are a figure.
        assert_eq!(boxes(&page(shaded(), vec![])), ["99 99 202 94"]);
    }

    #[test]
    fn boxes_standing_alone_are_a_diagram_whatever_the_sheet_reads() {
        // Three boxes joined by lines, as an organization chart without
        // arrowheads is drawn: the sheet reads the lines as a table.
        let chart = || {
            vec![
                outlined(200.0, 100.0, 100.0, 30.0),
                outlined(100.0, 180.0, 100.0, 30.0),
                outlined(300.0, 180.0, 100.0, 30.0),
                stroked((250.0, 130.0), (250.0, 155.0)),
                stroked((150.0, 155.0), (350.0, 155.0)),
                stroked((150.0, 155.0), (150.0, 180.0)),
                stroked((350.0, 155.0), (350.0, 180.0)),
            ]
        };
        let table = vec![[100.0, 100.0, 400.0, 210.0]];
        assert_eq!(boxes(&page(chart(), table.clone())), ["98 98 303 113"]);
        // Boxes side by side are the cells of a table.
        let cells = vec![
            outlined(100.0, 100.0, 100.0, 30.0),
            outlined(200.0, 100.0, 100.0, 30.0),
            outlined(300.0, 100.0, 100.0, 30.0),
        ];
        assert!(boxes(&page(cells, table.clone())).is_empty());
        // So are areas between rulings that run the length of the table.
        let mut shaded = vec![
            filled(100.0, 100.0, 100.0, 20.0),
            filled(100.0, 140.0, 100.0, 20.0),
            filled(100.0, 180.0, 100.0, 20.0),
        ];
        for y in [100.0, 120.0, 140.0, 160.0, 180.0, 200.0] {
            shaded.push(stroked((100.0, y), (400.0, y)));
        }
        for x in [100.0, 200.0, 400.0] {
            shaded.push(stroked((x, 100.0), (x, 200.0)));
        }
        assert!(boxes(&page(shaded, vec![[100.0, 100.0, 400.0, 200.0]])).is_empty());
    }

    #[test]
    fn an_area_and_the_line_around_it_are_one_outline() {
        // A heading on a bar over a framed paragraph: two outlines, though
        // the bar is painted and then stroked.
        let paths = vec![
            filled(100.0, 100.0, 300.0, 20.0),
            outlined(100.0, 100.0, 300.0, 20.0),
            outlined(100.0, 124.0, 300.0, 40.0),
        ];
        assert!(boxes(&page(paths, vec![])).is_empty());
    }

    #[test]
    fn the_marks_on_one_picture_are_one_figure() {
        let mut page = page(
            vec![
                outlined(120.0, 120.0, 30.0, 20.0),
                outlined(240.0, 160.0, 30.0, 20.0),
            ],
            vec![],
        );
        page.images = vec![super::super::PdfImage {
            asset: "image-001".into(),
            matrix: [200.0, 0.0, 0.0, 100.0, 100.0, 100.0],
            mask: false,
        }];
        let files = BTreeMap::from([("image-001".to_owned(), "image-001.png".to_owned())]);
        let (pictures, _) = figures(&page, &files, &AssetLimits::default());
        let [svg] = &pictures[..] else {
            panic!("{pictures:?}")
        };
        assert_eq!(svg.matches("<path ").count(), 2, "{svg}");
        assert_eq!(svg.matches("<image ").count(), 1, "{svg}");
    }

    #[test]
    fn blocks_of_boxes_a_line_joins_are_a_diagram() {
        // Two entities, a name on a bar over ruled columns, and the line
        // of their relation: the sheet reads each as a table.
        let entities = |joined: bool| {
            let mut paths = vec![];
            for x in [100.0, 340.0] {
                paths.push(filled(x, 100.0, 150.0, 24.0));
                paths.push(outlined(x, 124.0, 150.0, 66.0));
                paths.push(stroked((x, 146.0), (x + 150.0, 146.0)));
                paths.push(stroked((x, 168.0), (x + 150.0, 168.0)));
            }
            if joined {
                paths.push(stroked((250.0, 157.0), (340.0, 157.0)));
            }
            paths
        };
        let tables = vec![[100.0, 100.0, 250.0, 190.0], [340.0, 100.0, 490.0, 190.0]];
        assert_eq!(
            boxes(&page(entities(true), tables.clone())),
            ["98 99 393 92"]
        );
        // Apart, they are two tables.
        assert!(boxes(&page(entities(false), tables)).is_empty());
    }

    #[test]
    fn figures_whose_lines_rule_no_table_are_those_that_are_groups_whole() {
        // An organization chart the sheet would read as a table, and a
        // circle set in a table far larger than it.
        let mut paths = vec![
            outlined(200.0, 400.0, 100.0, 30.0),
            outlined(100.0, 480.0, 100.0, 30.0),
            outlined(300.0, 480.0, 100.0, 30.0),
            stroked((250.0, 430.0), (250.0, 455.0)),
            stroked((150.0, 455.0), (350.0, 455.0)),
            stroked((150.0, 455.0), (150.0, 480.0)),
            stroked((350.0, 455.0), (350.0, 480.0)),
        ];
        paths.extend(ruled());
        for y in [190.0, 220.0, 250.0, 280.0, 310.0] {
            paths.push(stroked((100.0, y), (300.0, y)));
        }
        paths.push(stroked((100.0, 160.0), (100.0, 310.0)));
        paths.push(stroked((300.0, 160.0), (300.0, 310.0)));
        paths.push(path(
            "M130,105C141,105 150,114 150,125C150,136 141,145 130,145C119,145 110,136 110,125C110,114 119,105 130,105Z",
            None,
            Some("CC0000"),
        ));
        let tables = [[100.0, 100.0, 300.0, 310.0], [100.0, 400.0, 400.0, 510.0]];
        let found = regions(&paths, 600.0, 800.0, &tables, &[]);
        assert_eq!(found.len(), 1, "{found:?}");
        let (bounds, diagram) = found[0];
        assert!(diagram);
        assert_eq!(bounds.map(f64::round), [100.0, 400.0, 401.0, 511.0]);
    }

    #[test]
    fn a_figure_drawn_against_a_table_is_a_figure_without_the_table() {
        // A ruled table, a box touching its last ruling and a curve from
        // the box: with the table known beyond doubt, the figure is the box
        // and the curve; without, the box is taken for a part of the table
        // and the curve for a drawing set in it.
        let mut paths = ruled();
        paths.push(outlined(100.0, 162.0, 100.0, 30.0));
        paths.push(path(
            "M150,192C150,220 250,200 250,230",
            None,
            Some("0000FF"),
        ));
        let table = [100.0, 100.0, 300.0, 160.0];
        let mut drawn = page(paths, vec![table]);
        assert_eq!(boxes(&drawn), ["148 190 103 41"]);
        drawn.sure_tables = vec![table];
        assert_eq!(boxes(&drawn), ["98 160 153 71"]);
        // The text of the table is not the figure's, though it lies by it.
        drawn.places = vec![[105.0, 155.0, 20.0], [105.0, 180.0, 20.0]];
        drawn.texts = vec!["cell".into(), "box".into()];
        drawn.fonts = vec![serde_json::json!({"latin":"Arial","size":10.0}); 2];
        let (pictures, _) = figures(&drawn, &BTreeMap::new(), &AssetLimits::default());
        assert!(pictures[0].contains(">box<") && !pictures[0].contains(">cell<"));
    }
}
