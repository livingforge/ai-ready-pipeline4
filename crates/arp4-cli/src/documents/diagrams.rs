//! The diagrams of a page: its lines, what each end of a line meets, and the
//! shapes the lines join, which the body writes together as one element. Read
//! from where the drawings lie on the page and the connections the original
//! records: an end the original does not connect meets the shape it touches
//! or lies on, within the distance the structure policy gives, else a cell
//! holding text on an Excel sheet. These are facts of the drawings; what a
//! diagram means is for the structure interpretation.
use super::*;
use crate::document_body::{
    Arrow, CHECKBOX, CheckBox, Diagram, DiagramLine, DiagramShape, LineEnd,
};
use std::sync::OnceLock;

const EMU_PER_POINT: f64 = 12_700.0;
/// The maximum digit width of Excel's default font in pixels, which turns
/// column widths in characters into points.
const DIGIT_PIXELS: f64 = 7.0;
/// Preset geometries drawing a line from one corner of their box to the other.
const LINE_GEOMETRIES: [&str; 10] = [
    "line",
    "straightConnector1",
    "bentConnector2",
    "bentConnector3",
    "bentConnector4",
    "bentConnector5",
    "curvedConnector2",
    "curvedConnector3",
    "curvedConnector4",
    "curvedConnector5",
];
/// How far apart two lengths in points may lie and still be one.
const EPSILON: f64 = 0.5;

/// The names of shapes by their preset geometry.
fn vocabulary() -> &'static Value {
    static NAMES: OnceLock<Value> = OnceLock::new();
    NAMES.get_or_init(|| {
        serde_json::from_str(include_str!("../../../../contracts/diagram-shapes.json")).unwrap()
    })
}

/// How far from a shape a line's end may lie and meet it, in points.
fn attach_distance() -> f64 {
    crate::document_structure::requirement_policy()["diagram_attach_distance_pt"]
        .as_f64()
        .unwrap()
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Rect {
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
}

type Point = (f64, f64);

impl Rect {
    fn new(left: f64, top: f64, width: f64, height: f64) -> Self {
        Self {
            left: left.min(left + width),
            top: top.min(top + height),
            right: left.max(left + width),
            bottom: top.max(top + height),
        }
    }

    fn width(&self) -> f64 {
        self.right - self.left
    }

    fn height(&self) -> f64 {
        self.bottom - self.top
    }

    fn area(&self) -> f64 {
        self.width() * self.height()
    }

    fn center(&self) -> Point {
        (
            (self.left + self.right) / 2.0,
            (self.top + self.bottom) / 2.0,
        )
    }

    fn contains(&self, (x, y): Point) -> bool {
        self.left <= x && x <= self.right && self.top <= y && y <= self.bottom
    }

    /// Whether `other` lies within this one, and this one is the larger.
    fn encloses(&self, other: &Rect) -> bool {
        self.left - EPSILON <= other.left
            && other.right <= self.right + EPSILON
            && self.top - EPSILON <= other.top
            && other.bottom <= self.bottom + EPSILON
            && self.area() > other.area() + EPSILON
    }

    /// How far `point` lies from the edge of this box, inside or out.
    fn edge_distance(&self, (x, y): Point) -> f64 {
        if self.contains((x, y)) {
            (x - self.left)
                .min(self.right - x)
                .min(y - self.top)
                .min(self.bottom - y)
        } else {
            let dx = (self.left - x).max(x - self.right).max(0.0);
            let dy = (self.top - y).max(y - self.bottom).max(0.0);
            dx.hypot(dy)
        }
    }

    /// This box turned by `degrees` about its center: its sides swap for a
    /// quarter turn, as the box of a turned shape is drawn.
    fn turned(self, degrees: f64) -> Self {
        let quarter = (degrees / 90.0).round() as i64;
        if quarter.rem_euclid(2) == 0 {
            return self;
        }
        let (x, y) = self.center();
        let (w, h) = (self.height(), self.width());
        Rect::new(x - w / 2.0, y - h / 2.0, w, h)
    }
}

/// The side of the squares [`Squares`] divides a page into, in points.
const SQUARE: f64 = 72.0;
/// The most squares a box is listed in; a larger box is looked at for every point.
const MOST_SQUARES: i64 = 4096;

/// The boxes of drawings by the squares of the page they cover, so that the
/// boxes at a point are found without going through every drawing: a page
/// may hold thousands of shapes and lines.
struct Squares {
    squares: HashMap<(i64, i64), Vec<usize>>,
    large: Vec<usize>,
}

impl Squares {
    /// The boxes given by drawing, each grown by `margin` on every side.
    fn new(boxes: impl IntoIterator<Item = (usize, Rect)>, margin: f64) -> Self {
        let mut squares: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
        let mut large = vec![];
        let square = |v: f64| (v / SQUARE).floor() as i64;
        for (index, rect) in boxes {
            let (left, top) = (square(rect.left - margin), square(rect.top - margin));
            let (right, bottom) = (square(rect.right + margin), square(rect.bottom + margin));
            let finite = [rect.left, rect.top, rect.right, rect.bottom]
                .iter()
                .all(|v| v.is_finite());
            if !finite || (right - left + 1).saturating_mul(bottom - top + 1) > MOST_SQUARES {
                large.push(index);
                continue;
            }
            for x in left..=right {
                for y in top..=bottom {
                    squares.entry((x, y)).or_default().push(index);
                }
            }
        }
        Self { squares, large }
    }

    /// The drawings whose grown boxes may hold `point`, in order.
    fn at(&self, (x, y): Point) -> Vec<usize> {
        let key = ((x / SQUARE).floor() as i64, (y / SQUARE).floor() as i64);
        let mut found: Vec<usize> = self
            .squares
            .get(&key)
            .into_iter()
            .flatten()
            .chain(&self.large)
            .copied()
            .collect();
        found.sort_unstable();
        found
    }
}

/// The sizes along one axis of an Excel sheet, in points: the default size
/// and the runs of lines (numbered from 1) of other sizes.
struct Axis {
    default: f64,
    /// Each run of lines of another size as (first, last, size), in order,
    /// with what the runs before it add to the default sizes.
    runs: Vec<(u32, u32, f64, f64)>,
    limit: u64,
}

impl Axis {
    /// The axis of lines `default` in size but for the `runs` of other sizes
    /// as (first, last, size) and the `hidden` lines, which take no room.
    fn new(
        default: f64,
        mut runs: Vec<(u32, u32, f64)>,
        hidden: &BTreeSet<u32>,
        limit: u64,
    ) -> Self {
        runs.sort_by_key(|run| run.0);
        let mut split = vec![];
        for (first, last, size) in runs {
            let mut start = first;
            for &line in hidden.range(first..=last) {
                if start < line {
                    split.push((start, line - 1, size));
                }
                start = line + 1;
            }
            if start <= last {
                split.push((start, last, size));
            }
        }
        split.extend(hidden.iter().map(|&line| (line, line, 0.0)));
        split.sort_by_key(|run| run.0);
        let mut added = 0.0;
        let mut runs = Vec::with_capacity(split.len());
        // A later run over lines an earlier one sized does not size them again.
        let mut next = 1;
        for (first, last, size) in split {
            let first = first.max(next);
            if first > last {
                continue;
            }
            runs.push((first, last, size, added));
            added += f64::from(last - first + 1) * (size - default);
            next = last + 1;
        }
        Self {
            default,
            runs,
            limit,
        }
    }

    /// Where the line at the zero-based `index` starts.
    fn start(&self, index: u64) -> f64 {
        // The lines before it are those numbered up to `index`.
        let at = self.runs.partition_point(|run| u64::from(run.0) <= index);
        let added = at.checked_sub(1).map_or(0.0, |i| {
            let (first, last, size, before) = self.runs[i];
            let lines = index.min(u64::from(last)) - u64::from(first) + 1;
            before + lines as f64 * (size - self.default)
        });
        self.default * index as f64 + added
    }

    /// The zero-based index of the line holding `position`.
    fn at(&self, position: f64) -> u64 {
        let (mut low, mut high) = (0, self.limit);
        while low + 1 < high {
            let middle = low + (high - low) / 2;
            if self.start(middle) <= position {
                low = middle;
            } else {
                high = middle;
            }
        }
        low
    }
}

/// The columns and rows of an Excel sheet, which place its cells and the
/// drawings anchored on them.
struct Grid {
    columns: Axis,
    rows: Axis,
}

/// The width in points of a column `characters` wide, as Excel draws it.
fn column_points(characters: f64) -> f64 {
    let pixels = ((256.0 * characters + (128.0 / DIGIT_PIXELS).trunc()) / 256.0 * DIGIT_PIXELS)
        .trunc()
        .max(0.0);
    pixels * 0.75
}

impl Grid {
    fn of(sheet: &Sheet) -> Result<Option<Self>> {
        let grid = &sheet["grid"];
        if !grid.is_object() {
            return Ok(None);
        }
        let number = |v: &Value| v.as_f64().context("invalid sheet grid");
        let runs = |key: &str, size: &dyn Fn(f64) -> f64| -> Result<Vec<(u32, u32, f64)>> {
            array(&grid[key])?
                .iter()
                .map(|run| {
                    Ok((
                        number(&run[0])? as u32,
                        number(&run[1])? as u32,
                        size(number(&run[2])?),
                    ))
                })
                .collect()
        };
        let hidden_columns = sheet["hidden_columns"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|name| excel::column_number(string(name)?))
            .collect::<Result<BTreeSet<_>>>()?;
        let hidden_rows = sheet["hidden_rows"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|row| Ok(u32::try_from(row.as_u64().context("invalid hidden row")?)?))
            .collect::<Result<BTreeSet<_>>>()?;
        Ok(Some(Self {
            columns: Axis::new(
                column_points(number(&grid["column_width"])?),
                runs("columns", &column_points)?,
                &hidden_columns,
                16_384,
            ),
            rows: Axis::new(
                number(&grid["row_height"])?,
                runs("rows", &|height| height)?,
                &hidden_rows,
                1_048_576,
            ),
        }))
    }

    /// The zero-based column and row of the cell holding `point`.
    fn cell_at(&self, (x, y): Point) -> (u64, u64) {
        (self.columns.at(x), self.rows.at(y))
    }

    /// The box of the cell at the zero-based `column` and `row`.
    fn cell(&self, column: u64, row: u64) -> Rect {
        let (left, top) = (self.columns.start(column), self.rows.start(row));
        Rect {
            left,
            top,
            right: self.columns.start(column + 1),
            bottom: self.rows.start(row + 1),
        }
    }
}

/// A length in EMU, given as a number or as text.
fn emu(value: &Value) -> Option<f64> {
    match value {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
    .map(|v| v / EMU_PER_POINT)
}

/// A pair of lengths in EMU of the object `value`.
fn emu_pair(value: &Value, x: &str, y: &str) -> Option<Point> {
    Some((emu(&value[x])?, emu(&value[y])?))
}

/// Where `anchor` places a drawing, in points on the page.
fn anchor_rect(anchor: &Value, grid: Option<&Grid>) -> Option<Rect> {
    let marker = |m: &Value| -> Option<Point> {
        let grid = grid?;
        Some((
            grid.columns.start(m["col"].as_u64()?) + emu(&m["colOff"]).unwrap_or(0.0),
            grid.rows.start(m["row"].as_u64()?) + emu(&m["rowOff"]).unwrap_or(0.0),
        ))
    };
    match anchor["kind"].as_str()? {
        "twoCellAnchor" => {
            let (from, to) = (marker(&anchor["from"])?, marker(&anchor["to"])?);
            Some(Rect::new(from.0, from.1, to.0 - from.0, to.1 - from.1))
        }
        "oneCellAnchor" => {
            let from = marker(&anchor["from"])?;
            let (w, h) = emu_pair(&anchor["ext"], "cx", "cy")?;
            Some(Rect::new(from.0, from.1, w, h))
        }
        "absoluteAnchor" => {
            let (x, y) = emu_pair(&anchor["pos"], "x", "y")?;
            let (w, h) = emu_pair(&anchor["ext"], "cx", "cy")?;
            Some(Rect::new(x, y, w, h))
        }
        "slide" => Some(Rect::new(
            anchor["left"].as_f64()?,
            anchor["top"].as_f64()?,
            anchor["width"].as_f64()?,
            anchor["height"].as_f64()?,
        )),
        _ => None,
    }
}

/// Where a drawing of a group lies, given where the group lies (`group`)
/// and the transforms of the group and of the drawing: the group maps the
/// box of its children (`chOff`, `chExt`) onto its own.
fn child_rect(group: Rect, group_transform: &Value, transform: &Value) -> Option<Rect> {
    let (cx, cy) = emu_pair(&group_transform["chOff"], "x", "y")?;
    let (cw, ch) = emu_pair(&group_transform["chExt"], "cx", "cy")?;
    let (x, y) = emu_pair(&transform["off"], "x", "y")?;
    let (w, h) = emu_pair(&transform["ext"], "cx", "cy")?;
    let scale = |size: f64, child: f64| if child > 0.0 { size / child } else { 1.0 };
    let (sx, sy) = (scale(group.width(), cw), scale(group.height(), ch));
    Some(Rect::new(
        group.left + (x - cx) * sx,
        group.top + (y - cy) * sy,
        w * sx,
        h * sy,
    ))
}

fn flag(transform: &Value, name: &str) -> bool {
    matches!(transform[name].as_str(), Some("1" | "true"))
}

/// The turn of a drawing in degrees.
fn rotation(transform: &Value) -> f64 {
    transform["rot"]
        .as_str()
        .and_then(|r| r.parse::<f64>().ok())
        .map_or(0.0, |r| r / 60_000.0)
}

/// The start and the end of a line drawn across `rect`, as its flips and
/// turn place them.
fn line_ends(rect: Rect, transform: &Value) -> (Point, Point) {
    let (x1, x2) = if flag(transform, "flipH") {
        (rect.right, rect.left)
    } else {
        (rect.left, rect.right)
    };
    let (y1, y2) = if flag(transform, "flipV") {
        (rect.bottom, rect.top)
    } else {
        (rect.top, rect.bottom)
    };
    let (cx, cy) = rect.center();
    let (sin, cos) = rotation(transform).to_radians().sin_cos();
    let turn = |(x, y): Point| {
        let (dx, dy) = (x - cx, y - cy);
        (cx + dx * cos - dy * sin, cy + dx * sin + dy * cos)
    };
    (turn((x1, y1)), turn((x2, y2)))
}

#[derive(Clone, Copy, PartialEq)]
enum Role {
    /// A shape lines may join, which the diagram shows.
    Shape,
    Line,
    Group,
    /// A picture or chart: lines may point at it, but the body keeps it as an
    /// element of its own.
    Apart,
}

struct Drawing<'a> {
    value: &'a Value,
    visual: String,
    key: String,
    role: Role,
    rect: Option<Rect>,
    group: Option<usize>,
}

/// What an end of a line meets.
#[derive(Clone, Copy, PartialEq)]
enum Meets {
    Drawing(usize),
    /// The zero-based column and row of a cell, which holds text if any.
    Cell(u64, u64),
    Nothing,
}

/// Leaves both ends of a line meeting nothing when neither is connected and
/// both meet the same shape or cell: a line drawn on one shape or cell, as
/// a mark on it, joins nothing.
fn apart(ends: &mut [(Option<Point>, Meets, bool); 2]) {
    if ends[0].2 && ends[1].2 && ends[0].1 == ends[1].1 {
        for end in ends.iter_mut() {
            *end = (end.0, Meets::Nothing, false);
        }
    }
}

/// The root of `item` among the groups `parents` makes.
fn root(parents: &mut [usize], item: usize) -> usize {
    let mut at = item;
    while parents[at] != at {
        parents[at] = parents[parents[at]];
        at = parents[at];
    }
    at
}

fn join(parents: &mut [usize], a: usize, b: usize) {
    let (a, b) = (root(parents, a), root(parents, b));
    if a != b {
        parents[a.max(b)] = a.min(b);
    }
}

/// The cell range `A1:B2` from the zero-based columns and rows given.
fn range(first: (u64, u64), last: (u64, u64)) -> Result<String> {
    let cell = |(column, row): (u64, u64)| -> Result<String> {
        Ok(format!(
            "{}{}",
            excel::column_name(u32::try_from(column + 1)?)?,
            row + 1
        ))
    };
    let (first, last) = (cell(first)?, cell(last)?);
    Ok(if first == last {
        first
    } else {
        format!("{first}:{last}")
    })
}

/// The cells a two-cell `anchor` covers, as zero-based (first, last)
/// columns and rows: a marker at the very start of a cell ends before it.
fn anchor_cells(anchor: &Value) -> Option<((u64, u64), (u64, u64))> {
    if anchor["kind"] != "twoCellAnchor" {
        return None;
    }
    let (from, to) = (&anchor["from"], &anchor["to"]);
    let first = (from["col"].as_u64()?, from["row"].as_u64()?);
    let end = |index: &str, offset: &str, start: u64| -> Option<u64> {
        let at = to[index].as_u64()?;
        Some(if to[offset].as_i64().unwrap_or(0) == 0 && at > start {
            at - 1
        } else {
            at
        })
    };
    let last = (
        end("col", "colOff", first.0)?,
        end("row", "rowOff", first.1)?,
    );
    Some((first, last))
}

fn number(value: f64) -> String {
    let rounded = (value * 10.0).round() / 10.0;
    if rounded.fract() == 0.0 {
        format!("{}", rounded as i64)
    } else {
        format!("{rounded}")
    }
}

/// The diagrams of the extraction `sheet`, whose drawings the layout
/// `typed` binds as `visuals` (by visual ID): each the members of one
/// group of drawings holding a line, with its shapes, its lines and the
/// elements of their texts. Pending `operations` move the cells they show.
pub(super) fn derive(
    sheet: &Sheet,
    visuals: &Value,
    typed: &LayoutSheet,
    operations: &[excel::StructuralOperation],
) -> Result<Vec<Diagram<String>>> {
    // The drawings the page shows: not those deleted with their cells.
    let list: Vec<&Value> = sheet["drawings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|drawing| {
            drawing["id"]
                .as_str()
                .is_some_and(|id| visuals.get(elements::token("visual", &[id])).is_some())
        })
        .collect();
    if list.is_empty() {
        return Ok(vec![]);
    }
    let grid = Grid::of(sheet)?;
    let mut drawings = Vec::with_capacity(list.len());
    let mut by_id = HashMap::new();
    let mut keys = BTreeSet::new();
    for (index, value) in list.iter().enumerate() {
        let id = string(&value["id"])?;
        let raw = id.rsplit_once('#').map_or(id, |(_, raw)| raw);
        let mut key = format!("s{raw}");
        let mut n = 1;
        while !keys.insert(key.clone()) {
            n += 1;
            key = format!("s{raw}-{n}");
        }
        let geometry = value["geometry"].as_str().unwrap_or("");
        let role = match value["kind"].as_str().unwrap_or("") {
            "connector" => Role::Line,
            "shape" if LINE_GEOMETRIES.contains(&geometry) => Role::Line,
            "shape" => Role::Shape,
            "group" => Role::Group,
            "graphic" if value["chart_part"].is_null() => Role::Shape,
            _ => Role::Apart,
        };
        by_id.insert(id, index);
        drawings.push(Drawing {
            value,
            visual: elements::token("visual", &[id]),
            key,
            role,
            rect: None,
            group: None,
        });
    }
    // Where each drawing lies: a drawing of a group on an Excel sheet by
    // the transforms of its groups, as its anchor is the group's.
    let slide = list.iter().any(|d| d["anchor"]["kind"] == "slide");
    for drawing in &mut drawings {
        drawing.group = drawing.value["group"]
            .as_str()
            .and_then(|g| by_id.get(g).copied());
    }
    fn place(
        drawings: &mut [Drawing<'_>],
        index: usize,
        grid: Option<&Grid>,
        slide: bool,
        depth: usize,
    ) -> Option<Rect> {
        if let Some(rect) = drawings[index].rect {
            return Some(rect);
        }
        let value = drawings[index].value;
        let rect = match drawings[index].group {
            Some(group) if !slide && depth < 64 => {
                let outer = place(drawings, group, grid, slide, depth + 1)?;
                child_rect(
                    outer,
                    &drawings[group].value["transform"],
                    &value["transform"],
                )
                .or(Some(outer))
            }
            _ => anchor_rect(&value["anchor"], grid),
        };
        drawings[index].rect = rect;
        rect
    }
    for index in 0..drawings.len() {
        place(&mut drawings, index, grid.as_ref(), slide, 0);
    }
    let tolerance = attach_distance();
    // What the ends of lines may meet, as drawn on the page.
    let targets: Vec<Option<Rect>> = drawings
        .iter()
        .map(|d| {
            let rect = d
                .rect
                .filter(|_| matches!(d.role, Role::Shape | Role::Apart))?;
            Some(rect.turned(rotation(&d.value["transform"])))
        })
        .collect();
    let squares = Squares::new(
        targets
            .iter()
            .enumerate()
            .filter_map(|(i, rect)| Some((i, (*rect)?))),
        tolerance,
    );
    // The merges, each with its place in the sheet's list, by first row.
    let mut merges: Vec<_> = sheet["merges"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| {
            let (a, b) = m.as_str()?.split_once(':')?;
            let (c1, r1) = excel::coordinate(a).ok()?;
            let (c2, r2) = excel::coordinate(b).ok()?;
            Some((
                (u64::from(c1), u64::from(r1)),
                (u64::from(c2), u64::from(r2)),
            ))
        })
        .enumerate()
        .map(|(order, (first, last))| (order, first, last))
        .collect();
    merges.sort_by_key(|&(_, (_, r1), _)| r1);
    // The furthest last row of each merge and those before it: looking back
    // from a point stops once none of them reaches its row.
    let reach: Vec<u64> = merges
        .iter()
        .scan(0, |reach, &(_, _, (_, r2))| {
            *reach = r2.max(*reach);
            Some(*reach)
        })
        .collect();
    // The cell holding a point, or the first cell of the first merge holding it.
    let cell_of = |(column, row): (u64, u64)| -> (u64, u64) {
        let (c, r) = (column + 1, row + 1);
        let end = merges.partition_point(|&(_, (_, r1), _)| r1 <= r);
        (0..end)
            .rev()
            .take_while(|&i| reach[i] >= r)
            .map(|i| merges[i])
            .filter(|&(_, (c1, _), (c2, r2))| c1 <= c && c <= c2 && r <= r2)
            .min_by_key(|&(order, _, _)| order)
            .map_or((column, row), |(_, (c1, r1), _)| (c1 - 1, r1 - 1))
    };
    // The ends of each line: (point, meets, near) for its start and end.
    let mut ends: BTreeMap<usize, [(Option<Point>, Meets, bool); 2]> = BTreeMap::new();
    let mut wanted: BTreeSet<(u64, u64)> = BTreeSet::new();
    for (index, drawing) in drawings.iter().enumerate() {
        if drawing.role != Role::Line {
            continue;
        }
        let points = drawing
            .rect
            .map(|rect| line_ends(rect, &drawing.value["transform"]));
        let mut meets = [
            (points.map(|p| p.0), Meets::Nothing, false),
            (points.map(|p| p.1), Meets::Nothing, false),
        ];
        for connection in drawing.value["connections"]
            .as_array()
            .into_iter()
            .flatten()
        {
            let end = match connection["end"].as_str() {
                Some("stCxn") => 0,
                Some("endCxn") => 1,
                _ => continue,
            };
            if let Some(&target) = connection["target"].as_str().and_then(|t| by_id.get(t)) {
                meets[end].1 = Meets::Drawing(target);
            }
        }
        for end in &mut meets {
            let (Some(point), Meets::Nothing) = (end.0, end.1) else {
                continue;
            };
            let candidates: Vec<(usize, Rect)> = squares
                .at(point)
                .into_iter()
                .filter(|&i| i != index)
                .filter_map(|i| Some((i, targets[i]?)))
                .collect();
            let near = candidates
                .iter()
                .map(|(i, rect)| (*i, rect.edge_distance(point), rect.area()))
                .filter(|(_, distance, _)| *distance <= tolerance)
                .min_by(|a, b| {
                    // Of two as near, the smaller is the one inside the other.
                    if (a.1 - b.1).abs() < 0.01 {
                        a.2.total_cmp(&b.2)
                    } else {
                        a.1.total_cmp(&b.1)
                    }
                });
            // A box around the whole line holds the line, not an end of it.
            let around =
                |rect: &Rect| points.is_some_and(|(a, b)| rect.contains(a) && rect.contains(b));
            let inside = candidates
                .iter()
                .filter(|(_, rect)| rect.contains(point) && !around(rect))
                .min_by(|a, b| a.1.area().total_cmp(&b.1.area()));
            if let Some((target, _, _)) = near {
                *end = (end.0, Meets::Drawing(target), true);
            } else if let Some((target, _)) = inside {
                *end = (end.0, Meets::Drawing(*target), true);
            } else if let Some(grid) = &grid {
                let cell = cell_of(grid.cell_at(point));
                wanted.insert(cell);
                // A cell beside it, which the end may touch.
                let (column, row) = grid.cell_at(point);
                for (dc, dr) in [(-1i64, 0i64), (1, 0), (0, -1), (0, 1)] {
                    let (Some(c), Some(r)) =
                        (column.checked_add_signed(dc), row.checked_add_signed(dr))
                    else {
                        continue;
                    };
                    if grid.cell(c, r).edge_distance(point) <= tolerance {
                        wanted.insert(cell_of((c, r)));
                    }
                }
                *end = (end.0, Meets::Cell(cell.0, cell.1), true);
            }
        }
        apart(&mut meets);
        ends.insert(index, meets);
    }
    // The text of the cells ends may meet, read in one pass over the cells.
    let mut texts: HashMap<(u64, u64), String> = HashMap::new();
    if !wanted.is_empty() {
        for cell in &sheet.cells {
            let Ok((column, row)) = excel::coordinate(cell.address()) else {
                continue;
            };
            let at = (u64::from(column) - 1, u64::from(row) - 1);
            if wanted.contains(&at) {
                let text = match cell.value() {
                    CellValue::Null => continue,
                    CellValue::String(text) => text.to_owned(),
                    value => value.to_json().to_string(),
                };
                if !text.trim().is_empty() {
                    texts.insert(at, text);
                }
            }
        }
    }
    if let Some(grid) = &grid {
        for ends in ends.values_mut() {
            for end in ends.iter_mut() {
                let (Some(point), Meets::Cell(column, row)) = (end.0, end.1) else {
                    continue;
                };
                if texts.contains_key(&(column, row)) {
                    continue;
                }
                // The cell beside it nearest the end, if it holds text.
                let (c0, r0) = grid.cell_at(point);
                let beside = [(-1i64, 0i64), (1, 0), (0, -1), (0, 1)]
                    .into_iter()
                    .filter_map(|(dc, dr)| {
                        let (c, r) = (c0.checked_add_signed(dc)?, r0.checked_add_signed(dr)?);
                        let distance = grid.cell(c, r).edge_distance(point);
                        let cell = cell_of((c, r));
                        (distance <= tolerance && texts.contains_key(&cell))
                            .then_some((cell, distance))
                    })
                    .min_by(|a, b| a.1.total_cmp(&b.1));
                match beside {
                    Some(((c, r), _)) => end.1 = Meets::Cell(c, r),
                    None => end.2 = false,
                }
            }
            apart(ends);
        }
    }
    // The drawings each line joins, the drawings of one group, and the
    // shapes in a shape, make one diagram.
    let mut parents: Vec<usize> = (0..drawings.len()).collect();
    for (&line, meets) in &ends {
        for (_, meets, _) in meets {
            if let Meets::Drawing(target) = meets
                && drawings[*target].role == Role::Shape
            {
                join(&mut parents, line, *target);
            }
        }
    }
    for (index, drawing) in drawings.iter().enumerate() {
        if let Some(group) = drawing.group
            && drawing.role != Role::Apart
        {
            join(&mut parents, index, group);
        }
    }
    let shapes: Vec<usize> = (0..drawings.len())
        .filter(|&i| drawings[i].role == Role::Shape && drawings[i].rect.is_some())
        .collect();
    // A shape holding another holds its top left corner.
    let holders = Squares::new(
        shapes.iter().map(|&i| (i, drawings[i].rect.unwrap())),
        EPSILON,
    );
    for &inner in &shapes {
        let rect = drawings[inner].rect.unwrap();
        for outer in holders.at((rect.left, rect.top)) {
            // A placeholder lays out the slide's text, which a diagram does not hold.
            if drawings[outer].value["placeholder"].is_object() {
                continue;
            }
            if drawings[outer].rect.unwrap().encloses(&rect) {
                join(&mut parents, outer, inner);
            }
        }
    }
    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    let group_of: Vec<usize> = (0..drawings.len())
        .map(|index| root(&mut parents, index))
        .collect();
    for (index, drawing) in drawings.iter().enumerate() {
        if drawing.role != Role::Apart {
            groups.entry(group_of[index]).or_default().push(index);
        }
    }
    // The identities of the rows and columns by their keys, which name the
    // cells of the paragraphs of PowerPoint shapes: a sheet has one for each
    // of its rows, and only slides need them.
    let keys = list.iter().any(|d| d["paragraphs"].is_array()).then(|| {
        (
            identity::key_identities(typed, "rows"),
            identity::key_identities(typed, "columns"),
        )
    });
    // The elements of the text of a drawing: the shape text of an Excel
    // shape, the paragraphs of a PowerPoint shape.
    let text_elements = |drawing: &Drawing<'_>| -> Vec<String> {
        let visual = &visuals[&drawing.visual];
        if let Some(key) = visual["text_key"].as_str() {
            return vec![elements::token("shape", &[key, "text"])];
        }
        let Some((rows, columns)) = &keys else {
            return vec![];
        };
        drawing.value["paragraphs"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|address| {
                let row = address.as_str()?.strip_prefix('A')?;
                let row = rows.get(&format!("r{row}"))?;
                Some(elements::cell_id(row, columns.get("A")?))
            })
            .collect()
    };
    // Where a drawing is on the page: its cells on an Excel sheet, its box
    // in points on a slide.
    let cells_of = |index: usize| -> Option<((u64, u64), (u64, u64))> {
        let drawing = &drawings[index];
        if drawing.group.is_none()
            && let Some(cells) = anchor_cells(&visuals[&drawing.visual]["anchor"])
        {
            return Some(cells);
        }
        let (grid, rect) = (grid.as_ref()?, drawing.rect?);
        Some((
            grid.cell_at((rect.left, rect.top)),
            grid.cell_at((
                (rect.right - EPSILON).max(rect.left),
                (rect.bottom - EPSILON).max(rect.top),
            )),
        ))
    };
    let place_of = |index: usize| -> Result<Option<String>> {
        if slide {
            return Ok(drawings[index].rect.map(|r| {
                format!(
                    "x{} y{} w{} h{}",
                    number(r.left),
                    number(r.top),
                    number(r.width()),
                    number(r.height())
                )
            }));
        }
        cells_of(index)
            .map(|(first, last)| range(first, last))
            .transpose()
    };
    let moved = |column: u64, row: u64| -> (u64, u64) {
        let anchor =
            json!({"kind":"oneCellAnchor","from":{"col":column,"colOff":0,"row":row,"rowOff":0}});
        excel::project_anchor(&anchor, operations)
            .ok()
            .and_then(|a| Some((a["from"]["col"].as_u64()?, a["from"]["row"].as_u64()?)))
            .unwrap_or((column, row))
    };
    let mut diagrams = vec![];
    let mut joined = BTreeSet::new();
    for members in groups.into_values() {
        if !members.iter().any(|&i| drawings[i].role == Role::Line) {
            continue;
        }
        joined.extend(members.iter().copied());
        let top_left = |i: &usize| {
            let rect = drawings[*i]
                .rect
                .unwrap_or(Rect::new(f64::MAX, f64::MAX, 0.0, 0.0));
            (rect.top, rect.left)
        };
        let mut ordered = members.clone();
        ordered.sort_by(|a, b| {
            let (a, b) = (top_left(a), top_left(b));
            a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1))
        });
        let mut shapes = vec![];
        for &index in ordered.iter().filter(|&&i| drawings[i].role == Role::Shape) {
            let drawing = &drawings[index];
            let inside = drawing.rect.and_then(|rect| {
                holders
                    .at((rect.left, rect.top))
                    .into_iter()
                    .filter(|&o| o != index && group_of[o] == group_of[index])
                    .filter_map(|o| Some((o, drawings[o].rect?)))
                    .filter(|(_, outer)| outer.encloses(&rect))
                    .min_by(|a, b| a.1.area().total_cmp(&b.1.area()))
                    .map(|(o, _)| drawings[o].key.clone())
            });
            let geometry = drawing.value["geometry"].as_str();
            let names = vocabulary();
            let name = match (drawing.value["kind"].as_str(), geometry) {
                (Some("graphic"), _) => string(&names["graphic"])?.to_owned(),
                (_, Some(geometry)) => names["names"][geometry]
                    .as_str()
                    .unwrap_or(geometry)
                    .to_owned(),
                (_, None) => string(&names["freeform"])?.to_owned(),
            };
            shapes.push(DiagramShape {
                key: drawing.key.clone(),
                name,
                at: place_of(index)?,
                inside,
                texts: text_elements(drawing),
            });
        }
        let mut lines = vec![];
        for &index in ordered.iter().filter(|&&i| drawings[i].role == Role::Line) {
            let drawing = &drawings[index];
            let end = |(point, meets, near): (Option<Point>, Meets, bool)| -> Result<LineEnd> {
                let other = |label: String, near: bool| LineEnd::Other { label, near };
                Ok(match meets {
                    Meets::Drawing(target) => {
                        let target = &drawings[target];
                        match target.role {
                            Role::Shape => LineEnd::Shape {
                                key: target.key.clone(),
                                near,
                            },
                            Role::Apart => {
                                let name = target.value["image"]["asset"]
                                    .as_str()
                                    .unwrap_or(&target.key);
                                let kind = if target.value["chart_part"].is_string() {
                                    "グラフ"
                                } else {
                                    "画像"
                                };
                                other(format!("{kind}({name})"), near)
                            }
                            _ => other(format!("図形({})", target.key), near),
                        }
                    }
                    Meets::Cell(column, row) => {
                        let (c, r) = moved(column, row);
                        let at = range((c, r), (c, r))?;
                        match texts.get(&(column, row)) {
                            Some(text) => {
                                let text = text.lines().next().unwrap_or("").trim();
                                let text: String = text.chars().take(30).collect();
                                other(format!("セル{at}「{text}」"), near)
                            }
                            None => other(format!("未接続({at})"), false),
                        }
                    }
                    Meets::Nothing => {
                        let at = match (point, &grid) {
                            (Some(p), Some(grid)) => {
                                let (column, row) = grid.cell_at(p);
                                let (c, r) = moved(column, row);
                                format!("({})", range((c, r), (c, r))?)
                            }
                            (Some((x, y)), None) => format!("(x{} y{})", number(x), number(y)),
                            _ => String::new(),
                        };
                        other(format!("未接続{at}"), false)
                    }
                })
            };
            let [start, finish] = ends[&index];
            let arrow = match (
                drawing.value["head_end"].is_string(),
                drawing.value["tail_end"].is_string(),
            ) {
                (false, true) => Arrow::Forward,
                (true, false) => Arrow::Backward,
                (true, true) => Arrow::Both,
                (false, false) => Arrow::None,
            };
            lines.push(DiagramLine {
                start: end(start)?,
                end: end(finish)?,
                arrow,
                dashed: drawing.value["dash"].is_string(),
                texts: text_elements(drawing),
            });
        }
        let at = if slide {
            None
        } else {
            let cells: Vec<_> = members.iter().filter_map(|&i| cells_of(i)).collect();
            match (
                cells.iter().map(|c| c.0.0).min(),
                cells.iter().map(|c| c.0.1).min(),
                cells.iter().map(|c| c.1.0).max(),
                cells.iter().map(|c| c.1.1).max(),
            ) {
                (Some(c1), Some(r1), Some(c2), Some(r2)) => Some(range((c1, r1), (c2, r2))?),
                _ => None,
            }
        };
        let members = members
            .iter()
            .map(|&i| drawings[i].visual.clone())
            .collect();
        diagrams.push(Diagram {
            at,
            shapes,
            lines,
            check_box: None,
            members,
        });
    }
    // A check box no line joins is written alone: its label with its state
    // and the cell it is linked to, which the label's place on the page
    // alone does not tell.
    for (index, drawing) in drawings.iter().enumerate() {
        let control = &drawing.value["control"];
        if control["kind"] != "CheckBox" || joined.contains(&index) {
            continue;
        }
        let linked_cell = control["linked_cell"].as_str().map(|cell| {
            // A cell of the sheet moves with the pending operations; a cell
            // of another sheet or a name is as the original writes it.
            excel::coordinate(&cell.replace('$', ""))
                .ok()
                .and_then(|(column, row)| {
                    let (c, r) = moved(u64::from(column) - 1, u64::from(row) - 1);
                    range((c, r), (c, r)).ok()
                })
                .unwrap_or_else(|| cell.to_owned())
        });
        diagrams.push(Diagram {
            at: place_of(index)?,
            shapes: vec![DiagramShape {
                key: drawing.key.clone(),
                name: CHECKBOX.into(),
                at: None,
                inside: None,
                texts: text_elements(drawing),
            }],
            lines: vec![],
            check_box: Some(CheckBox {
                checked: control["checked"].as_bool(),
                linked_cell,
            }),
            members: vec![drawing.visual.clone()],
        });
    }
    Ok(diagrams)
}

/// The diagrams [`derive`] gives the page `sheet` lays out, by the places of
/// their elements in its metadata: their drawings, and the texts of their
/// shapes and lines that are elements of their own.
pub(super) fn of(sheet: &LayoutSheet) -> Result<crate::document_body::Diagrams> {
    use crate::document_body::Diagrams;
    if sheet.diagrams.is_empty() {
        return Ok(Diagrams::default());
    }
    let mut drawings: HashMap<&str, usize> = HashMap::new();
    let mut texts: HashMap<&str, usize> = HashMap::new();
    for (place, element) in sheet.metadata.iter().enumerate() {
        if let Some(reference) = element["ref"].as_str() {
            drawings.insert(reference, place);
        } else if element["type"] == "text"
            && let Some(id) = element["id"].as_str()
        {
            texts.insert(id, place);
        }
    }
    let mut diagrams = vec![];
    for diagram in sheet.diagrams.iter() {
        let mut members = vec![];
        for visual in &diagram.members {
            members.push(
                *drawings
                    .get(visual.as_str())
                    .context("diagram drawing has no element")?,
            );
        }
        // A text within a table stays in its table.
        let mut places = |ids: &[String]| -> Vec<usize> {
            let found: Vec<usize> = ids
                .iter()
                .filter_map(|id| texts.get(id.as_str()).copied())
                .collect();
            members.extend(&found);
            found
        };
        let shapes = diagram
            .shapes
            .iter()
            .map(|shape| DiagramShape {
                key: shape.key.clone(),
                name: shape.name.clone(),
                at: shape.at.clone(),
                inside: shape.inside.clone(),
                texts: places(&shape.texts),
            })
            .collect();
        let lines = diagram
            .lines
            .iter()
            .map(|line| DiagramLine {
                start: line.start.clone(),
                end: line.end.clone(),
                arrow: line.arrow,
                dashed: line.dashed,
                texts: places(&line.texts),
            })
            .collect();
        diagrams.push(Diagram {
            at: diagram.at.clone(),
            shapes,
            lines,
            check_box: diagram.check_box.clone(),
            members,
        });
    }
    Diagrams::new(diagrams)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn axis_places_lines_of_their_own_size() {
        let axis = Axis::new(15.0, vec![(2, 2, 30.0)], &BTreeSet::from([4]), 100);
        assert_eq!(axis.start(0), 0.0);
        assert_eq!(axis.start(1), 15.0);
        assert_eq!(axis.start(2), 45.0);
        assert_eq!(axis.start(3), 60.0);
        assert_eq!(axis.start(4), 60.0);
        assert_eq!(axis.start(5), 75.0);
        assert_eq!(axis.at(44.0), 1);
        assert_eq!(axis.at(45.0), 2);
        assert_eq!(axis.at(61.0), 4);
        // A hidden line inside a run of another size splits it.
        let axis = Axis::new(10.0, vec![(1, 4, 20.0)], &BTreeSet::from([2]), 100);
        assert_eq!(axis.start(1), 20.0);
        assert_eq!(axis.start(2), 20.0);
        assert_eq!(axis.start(4), 60.0);
        assert_eq!(axis.start(5), 70.0);
    }

    #[test]
    fn line_ends_follow_flips_and_turns() {
        let rect = Rect::new(0.0, 0.0, 10.0, 4.0);
        assert_eq!(line_ends(rect, &json!({})), ((0.0, 0.0), (10.0, 4.0)));
        assert_eq!(
            line_ends(rect, &json!({"flipH":"1"})),
            ((10.0, 0.0), (0.0, 4.0))
        );
        let (start, end) = line_ends(Rect::new(0.0, 0.0, 10.0, 0.0), &json!({"rot":"5400000"}));
        assert!((start.0 - 5.0).abs() < 1e-9 && (start.1 + 5.0).abs() < 1e-9);
        assert!((end.0 - 5.0).abs() < 1e-9 && (end.1 - 5.0).abs() < 1e-9);
    }

    #[test]
    fn edge_distance_measures_from_inside_and_outside() {
        let rect = Rect::new(0.0, 0.0, 10.0, 10.0);
        assert_eq!(rect.edge_distance((2.0, 5.0)), 2.0);
        assert_eq!(rect.edge_distance((13.0, 14.0)), 5.0);
        assert!(Rect::new(-1.0, -1.0, 12.0, 12.0).encloses(&rect));
        assert!(!rect.encloses(&rect));
    }

    #[test]
    fn column_widths_are_drawn_in_whole_pixels() {
        // Excel's default column: 64 pixels.
        assert_eq!(column_points(9.140625), 48.0);
        assert_eq!(column_points(13.0), 68.25);
        assert_eq!(column_points(0.0), 0.0);
    }
}
