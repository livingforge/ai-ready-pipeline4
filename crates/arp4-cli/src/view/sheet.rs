//! An Excel sheet on its grid: the columns and rows in their sizes, with
//! the letters and numbers Excel heads them with, the gridlines the sheet
//! shows, each cell's fill, borders, alignment and font, merged cells as one,
//! text that does not wrap running on over the empty cells beside it, and the
//! sheet's drawings where their anchors place them.
use super::drawings::{Defaults, Rect};
use super::*;
use crate::excel::{DrawingLook, Paint, SheetLook, Workbook, column_name, coordinate};
use crate::extraction::{Cell, CellKind, Sheet};
use foldhash::HashMap;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

/// The width of the row numbers and the height of the column letters.
const HEAD_W: f64 = 40.0;
const HEAD_H: f64 = 20.0;
/// EMUs (DrawingML's unit) of a pixel at 96 per inch.
const EMU_PER_PIXEL: f64 = 9525.0;
/// The most cells whose formats of a row or column alone are looked at: a
/// larger sheet shows the formats of the cells it writes only.
const FORMATTED_AREA: u64 = 250_000;

pub(super) fn sheets(
    document: &Document<'_>,
    book: &Workbook,
    out: &mut Out,
) -> Result<Vec<Section>> {
    let looks = crate::excel::sheet_looks(book)?;
    let blank = crate::excel::blank_sheet_look(book)?;
    let drawn = crate::excel::drawing_looks(book)?;
    let mut sections = vec![];
    for (index, sheet) in document.extraction.sheets.iter().enumerate() {
        let name = sheet["name"].as_str().unwrap_or_default();
        let look = looks.get(name).unwrap_or(&blank);
        out.page = index;
        let mut section = Section::new(
            name,
            grid(document, sheet, look, &drawn, book.date1904, out)?,
        );
        section.hidden = sheet["state"].as_str().is_some_and(|s| s != "visible");
        sections.push(section);
    }
    Ok(sections)
}

/// A range of cells: first and last column and row, from 1.
#[derive(Clone, Copy)]
struct Range {
    c0: u32,
    r0: u32,
    c1: u32,
    r1: u32,
}

fn range(text: &str) -> Option<Range> {
    let (first, last) = text.split_once(':').unwrap_or((text, text));
    let (c0, r0) = coordinate(&first.replace('$', "")).ok()?;
    let (c1, r1) = coordinate(&last.replace('$', "")).ok()?;
    Some(Range {
        c0: c0.min(c1),
        r0: r0.min(r1),
        c1: c0.max(c1),
        r1: r0.max(r1),
    })
}

/// The left of each column and the top of each row on the page, from 1, one
/// past the last for its right and bottom edges.
struct Lines {
    x: Vec<f64>,
    y: Vec<f64>,
}

impl Lines {
    fn new(look: &SheetLook, columns: u32, rows: u32) -> Self {
        let mut x = vec![HEAD_W; columns as usize + 2];
        for c in 1..=columns as usize {
            x[c + 1] = x[c] + look.width(c as u32);
        }
        let mut y = vec![HEAD_H; rows as usize + 2];
        for r in 1..=rows as usize {
            y[r + 1] = y[r] + look.height(r as u32);
        }
        Self { x, y }
    }

    fn x(&self, column: u32) -> f64 {
        self.x[(column as usize).min(self.x.len() - 1)]
    }

    fn y(&self, row: u32) -> f64 {
        self.y[(row as usize).min(self.y.len() - 1)]
    }

    fn rect(&self, range: Range) -> Rect {
        let (x, y) = (self.x(range.c0), self.y(range.r0));
        Rect {
            x,
            y,
            w: self.x(range.c1 + 1) - x,
            h: self.y(range.r1 + 1) - y,
        }
    }

    /// The point a DrawingML anchor marker (`col`, `row` from 0, offsets in
    /// EMUs) names. Excel keeps an offset within its cell: one past the edge
    /// of a column or row made narrower or hidden since stops at the edge.
    fn marker(&self, marker: &Value) -> Option<(f64, f64)> {
        let column = marker["col"].as_u64()? as u32 + 1;
        let row = marker["row"].as_u64()? as u32 + 1;
        let offset = |name: &str, size: f64| {
            (marker[name].as_f64().unwrap_or(0.0) / EMU_PER_PIXEL).clamp(0.0, size)
        };
        let (x, y) = (self.x(column), self.y(row));
        Some((
            x + offset("colOff", self.x(column + 1) - x),
            y + offset("rowOff", self.y(row + 1) - y),
        ))
    }
}

/// A size or offset of a DrawingML extent or point, in pixels.
fn emu(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str()?.parse().ok())
        .map(|v| v / EMU_PER_PIXEL)
}

fn grid(
    document: &Document<'_>,
    sheet: &Sheet,
    look: &SheetLook,
    drawn: &HashMap<String, DrawingLook>,
    date1904: bool,
    out: &mut Out,
) -> Result<String> {
    let name = sheet["name"].as_str().unwrap_or_default();
    let cells: HashMap<(u32, u32), Cell<'_>> = sheet
        .cells
        .iter()
        .filter_map(|cell| Some((coordinate(cell.address()).ok()?, cell)))
        .collect();
    let content: HashMap<(u32, u32), &Shown> = document
        .content
        .cells
        .range((name.to_owned(), String::new())..)
        .take_while(|((sheet, _), _)| sheet == name)
        .filter_map(|((_, address), shown)| Some((coordinate(address).ok()?, shown)))
        .collect();
    let merges: Vec<Range> = sheet["merges"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| range(m.as_str()?))
        .collect();
    let drawings: Vec<&Value> = sheet["drawings"].as_array().into_iter().flatten().collect();
    // The extent of what the sheet shows, and a column and row beyond it.
    let (mut columns, mut rows) = (1, 1);
    let mut reach = |c: u32, r: u32| {
        columns = columns.max(c);
        rows = rows.max(r);
    };
    for &(c, r) in cells.keys().chain(content.keys()) {
        reach(c, r);
    }
    for (c, r) in look.painted() {
        reach(c, r);
    }
    for merge in &merges {
        reach(merge.c1, merge.r1);
    }
    for drawing in &drawings {
        for marker in [&drawing["anchor"]["from"], &drawing["anchor"]["to"]] {
            if let (Some(c), Some(r)) = (marker["col"].as_u64(), marker["row"].as_u64()) {
                reach(c as u32 + 2, r as u32 + 2);
            }
        }
    }
    let (columns, rows) = ((columns + 1).min(16_384), (rows + 1).min(1_048_576));
    let lines = Lines::new(look, columns, rows);
    let (width, height) = (lines.x(columns + 1), lines.y(rows + 1));
    let mut merged: HashMap<(u32, u32), usize> = HashMap::default();
    for (index, merge) in merges.iter().enumerate() {
        for r in merge.r0..=merge.r1.min(rows) {
            for c in merge.c0..=merge.c1.min(columns) {
                merged.insert((c, r), index);
            }
        }
    }
    let mut html = format!(r#"<div class="grid" style="width:{width:.0}px;height:{height:.0}px">"#);
    // The column letters and row numbers, which stay in view as the sheet
    // scrolls under them, as Excel freezes its headings.
    let _ = write!(
        html,
        r#"<div class="heads-x" style="width:{width:.0}px"><div class="corner"></div>"#
    );
    // Excel marks where columns and rows are hidden with a double line
    // between the headings beside them.
    let hidden = |edges: &[f64], i: usize| edges[i + 1] <= edges[i] && edges[i - 1] < edges[i];
    let mut gaps = String::new();
    for c in 1..=columns {
        let w = lines.x(c + 1) - lines.x(c);
        if w > 0.0 {
            let _ = write!(
                html,
                r#"<div class="hd" data-c="{c}" style="left:{:.0}px;top:0;width:{w:.0}px;height:{HEAD_H}px">{}</div>"#,
                lines.x(c),
                column_name(c)?
            );
        } else if c == 1 || hidden(&lines.x, c as usize) {
            let _ = write!(
                gaps,
                r#"<div class="gap" style="left:{:.0}px;top:0;width:3px;height:{HEAD_H}px"></div>"#,
                (lines.x(c) - 2.0).max(HEAD_W)
            );
        }
    }
    html.push_str(&gaps);
    gaps.clear();
    let _ = write!(
        html,
        r#"</div><div class="heads-y" style="height:{:.0}px">"#,
        height - HEAD_H
    );
    for r in 1..=rows {
        let h = lines.y(r + 1) - lines.y(r);
        if h > 0.0 {
            let _ = write!(
                html,
                r#"<div class="hd" data-r="{r}" style="left:0;top:{:.0}px;width:{HEAD_W}px;height:{h:.0}px">{r}</div>"#,
                lines.y(r) - HEAD_H
            );
        } else if r == 1 || hidden(&lines.y, r as usize) {
            let _ = write!(
                gaps,
                r#"<div class="gap" style="left:0;top:{:.0}px;width:{HEAD_W}px;height:3px"></div>"#,
                (lines.y(r) - HEAD_H - 2.0).max(0.0)
            );
        }
    }
    html.push_str(&gaps);
    html.push_str("</div>");
    // Gridlines, under the fills, which hide them.
    if look.grid_lines {
        let mut path = String::new();
        for c in 1..=columns + 1 {
            let _ = write!(path, "M{:.1},{HEAD_H}V{height:.1}", lines.x(c) - 0.5);
        }
        for r in 1..=rows + 1 {
            let _ = write!(path, "M{HEAD_W},{:.1}H{width:.1}", lines.y(r) - 0.5);
        }
        let _ = write!(
            html,
            r##"<svg class="lines" width="{width:.0}" height="{height:.0}"><path d="{path}" stroke="#e1e1e1" stroke-width="1" fill="none"/></svg>"##
        );
    }
    // The places drawn: those holding something, and every place of a
    // sheet small enough for the formats of its rows and columns to count.
    let mut places: BTreeSet<(u32, u32)> = cells.keys().chain(content.keys()).copied().collect();
    places.extend(look.painted());
    places.extend(merges.iter().map(|m| (m.c0, m.r0)));
    if u64::from(columns) * u64::from(rows) <= FORMATTED_AREA {
        for r in 1..=rows {
            for c in 1..=columns {
                if look
                    .cell(c, r)
                    .is_some_and(|f| f.fill.is_some() || f.borders.iter().any(Option::is_some))
                {
                    places.insert((c, r));
                }
            }
        }
    }
    // Fills, one for a merged range.
    for &(c, r) in &places {
        let merge = merged.get(&(c, r)).map(|i| merges[*i]);
        if merge.is_some_and(|m| (m.c0, m.r0) != (c, r)) {
            continue;
        }
        let rect = lines.rect(merge.unwrap_or(Range {
            c0: c,
            r0: r,
            c1: c,
            r1: r,
        }));
        // A merged cell hides the gridlines inside it.
        let fill = look
            .cell(c, r)
            .and_then(|f| f.fill.clone())
            .map(|rgb| format!("#{rgb}"))
            .or_else(|| merge.map(|_| "#fff".to_owned()));
        if let Some(fill) = fill
            && rect.w > 0.0
            && rect.h > 0.0
        {
            let _ = write!(
                html,
                r#"<div class="f" style="left:{:.0}px;top:{:.0}px;width:{:.0}px;height:{:.0}px;background:{fill}"></div>"#,
                rect.x, rect.y, rect.w, rect.h
            );
        }
    }
    // Borders, a merged range's only on its edges.
    let mut rules = Rules::default();
    for &(c, r) in &places {
        let Some(format) = look.cell(c, r) else {
            continue;
        };
        let merge = merged.get(&(c, r)).map(|i| merges[*i]);
        let (x0, x1, y0, y1) = (lines.x(c), lines.x(c + 1), lines.y(r), lines.y(r + 1));
        // A cell in a hidden column or row draws no border.
        if x1 <= x0 || y1 <= y0 {
            continue;
        }
        let sides = [
            (merge.is_none_or(|m| m.c0 == c), (x0, y0, x0, y1)),
            (merge.is_none_or(|m| m.c1 == c), (x1, y0, x1, y1)),
            (merge.is_none_or(|m| m.r0 == r), (x0, y0, x1, y0)),
            (merge.is_none_or(|m| m.r1 == r), (x0, y1, x1, y1)),
        ];
        for (side, (edge, line)) in format.borders.iter().zip(sides) {
            if let Some(side) = side
                && edge
            {
                rules.add(side, line);
            }
        }
    }
    let borders = rules.draw();
    if !borders.is_empty() {
        let _ = write!(
            html,
            r#"<svg class="lines" width="{width:.0}" height="{height:.0}">{borders}</svg>"#
        );
    }
    // Values. Text runs on over a hidden column, whatever it holds.
    let holds = |c: u32, r: u32| {
        merged.contains_key(&(c, r))
            || lines.x(c + 1) > lines.x(c)
                && content
                    .get(&(c, r))
                    .map(|s| !s.value.is_null())
                    .unwrap_or_else(|| {
                        cells
                            .get(&(c, r))
                            .is_some_and(|cell| !cell.value().is_null())
                    })
    };
    let shown_places: BTreeSet<(u32, u32)> = cells.keys().chain(content.keys()).copied().collect();
    for &(c, r) in &shown_places {
        let merge = merged.get(&(c, r)).map(|i| merges[*i]);
        if merge.is_some_and(|m| (m.c0, m.r0) != (c, r)) {
            continue;
        }
        let mut area = merge.unwrap_or(Range {
            c0: c,
            r0: r,
            c1: c,
            r1: r,
        });
        let shown = content.get(&(c, r)).copied();
        // A cell in a hidden column or row shows nothing, not even over the
        // cells beside it; a merge shows what of it is not hidden. A value the
        // content changes there is still listed among the differences.
        let own = lines.rect(area);
        if own.w <= 0.0 || own.h <= 0.0 {
            if shown.is_some_and(Shown::changed) {
                out.mark(shown, name, &format!("{}{r}（非表示）", column_name(c)?));
            }
            continue;
        }
        let cell = cells.get(&(c, r)).copied();
        let format = look.cell(c, r).cloned().unwrap_or_default();
        let font = excel_font(cell_font(sheet, cell, shown));
        let font = font.as_ref();
        let emphasis = cell_style(sheet, cell);
        let room = Room::new(
            look,
            area,
            font,
            emphasis["bold"] == true || font["bold"] == true,
        );
        // A number Excel fits to its cell, unless it shrinks it or turns it.
        let across = format.rotation == 0;
        let (text, kind) = display(
            cell,
            shown,
            date1904,
            (across && !format.shrink).then_some(&room),
        );
        let changed = shown.is_some_and(Shown::changed);
        if text.is_empty() && !changed {
            continue;
        }
        // Excel breaks the lines of a cell only when it wraps its text.
        let text = if format.wrap {
            text
        } else {
            text.replace(['\r', '\n'], "")
        };
        let horizontal = format.horizontal.as_deref().unwrap_or(match kind {
            Kind::Number => "right",
            Kind::Centered => "center",
            Kind::Text => "left",
        });
        // Text centred across a selection is centred over the empty cells
        // after it centred so too.
        if merge.is_none() && horizontal == "centerContinuous" {
            while area.c1 < columns
                && !holds(area.c1 + 1, r)
                && look
                    .cell(area.c1 + 1, r)
                    .and_then(|f| f.horizontal.as_deref())
                    == Some("centerContinuous")
            {
                area.c1 += 1;
            }
        }
        // Text that does not wrap runs on over the empty cells it points to,
        // unless it shrinks to fit its own.
        if merge.is_none() && !format.wrap && !format.shrink && kind == Kind::Text {
            if matches!(horizontal, "left" | "general") {
                while area.c1 < columns && !holds(area.c1 + 1, r) {
                    area.c1 += 1;
                }
            } else if horizontal == "right" {
                while area.c0 > 1 && !holds(area.c0 - 1, r) {
                    area.c0 -= 1;
                }
            }
        }
        let rect = lines.rect(area);
        if rect.w <= 0.0 || rect.h <= 0.0 {
            continue;
        }
        let address = format!("{}{r}", column_name(c)?);
        let mut attributes = format!(r#" data-a="{address}"{}"#, out.mark(shown, name, &address));
        // The formula bar shows a formula, rather than its result.
        let formula = match shown {
            Some(shown) if shown.formula => Some(plain(&shown.value)),
            _ => cell.and_then(|c| c.formula()).map(|f| format!("={f}")),
        };
        if let Some(formula) = formula {
            let _ = write!(attributes, r#" data-r="{}""#, escape(&formula));
        }
        let stacked = format.rotation == 255;
        let across_place = match horizontal {
            "center" | "centerContinuous" => "center",
            "right" => "flex-end",
            // Letters stacked one under another are centred unless placed.
            "general" | "left" if stacked && format.horizontal.is_none() => "center",
            _ => "flex-start",
        };
        let down_place = match format.vertical.as_deref() {
            Some("top") => "flex-start",
            Some("center" | "justify" | "distributed") => "center",
            _ => "flex-end",
        };
        // Stacked letters run down the cell, so the flex axes swap, and its
        // start across is the right.
        let (justify, align) = if stacked {
            let across = match across_place {
                "flex-start" => "flex-end",
                "flex-end" => "flex-start",
                place => place,
            };
            (down_place.to_owned(), across.to_owned())
        } else if format.wrap {
            // Wrapped text taller than its cell shows from its top.
            (across_place.to_owned(), format!("safe {down_place}"))
        } else {
            (across_place.to_owned(), down_place.to_owned())
        };
        // The look many cells share is a class of the page; the place is the
        // cell's own.
        let mut style = format!(
            "justify-content:{justify};align-items:{align};text-align:{};",
            match horizontal {
                "center" | "centerContinuous" => "center",
                "right" => "right",
                "justify" | "distributed" => "justify",
                _ => "left",
            }
        );
        if format.indent > 0 {
            let _ = write!(style, "padding-left:{}px;", 3 + format.indent * 9);
        }
        if stacked {
            style.push_str("writing-mode:vertical-rl;text-orientation:upright;");
        }
        // Text turned 1 to 90 degrees counterclockwise, or 91 to 180 for 1 to
        // 90 clockwise.
        let turned = match format.rotation {
            0 | 255 => 0,
            r if r <= 90 => -(r as i32),
            r => r as i32 - 90,
        };
        if turned != 0 {
            let _ = write!(style, "--turn:{turned}deg;");
        }
        style.push_str(&font_css(font, emphasis));
        // The font is a whole number of the screen's pixels, as Excel draws
        // it there, made smaller to fit the cell's width when it shrinks.
        let mut pixels = crate::excel::glyphs::screen_pixels(room.size);
        let needed = room.font.text(&text);
        if format.shrink && !format.wrap && across && !room.fits(needed) {
            pixels *= room.width.max(0.0) / needed;
        }
        // Lines are as far apart as the font sets them in Excel, as are
        // stacked letters, each a line.
        let line = crate::excel::line_height(font["latin"].as_str().unwrap_or_default());
        let _ = write!(style, "font-size:{pixels:.2}px;line-height:{line};");
        if stacked {
            let _ = write!(style, "letter-spacing:{:.2}em;", line - 1.0);
        }
        let _ = write!(
            html,
            r#"<div class="c k{}{}{}" style="left:{:.0}px;top:{:.0}px;width:{:.0}px;height:{:.0}px"{attributes}><span>{}</span></div>"#,
            out.class(style),
            if format.wrap { " w" } else { "" },
            if turned != 0 { " t" } else { "" },
            rect.x,
            rect.y,
            rect.w,
            rect.h,
            escape(&text)
        );
    }
    // Drawings, in the order the sheet lists them: later ones on top.
    let by_id: HashMap<&str, &Value> = drawings
        .iter()
        .filter_map(|d| Some((d["id"].as_str()?, *d)))
        .collect();
    for drawing in &drawings {
        let Some(rect) = drawing_rect(drawing, &by_id, &lines, 0) else {
            continue;
        };
        let id = drawing["id"].as_str().unwrap_or_default();
        // The fill, line and text place the drawing part gives the shape;
        // Excel's text starts at the top left.
        let shape = drawn.get(id);
        let mut drawing = (*drawing).clone();
        if let Some(shape) = shape {
            let paint = |paint: &Paint| match paint {
                Paint::Nothing => json!("none"),
                Paint::Color(rgb) => json!(rgb),
                Paint::Unresolved => Value::Null,
            };
            drawing["fill"] = paint(&shape.fill);
            drawing["line"] = json!({"color":paint(&shape.line),"width":shape.line_width});
        }
        let defaults = Defaults {
            fill: "#ffffff",
            line: "#404040",
            horizontal: "left",
            vertical: "flex-start",
        }
        .placed(
            shape.and_then(|s| s.horizontal.as_deref()),
            shape.and_then(|s| s.vertical.as_deref()),
        );
        let drawing = &drawing;
        let shown = document.content.shape(name, id);
        let text = shown
            .map(|s| plain(&s.value))
            .unwrap_or_else(|| plain(&drawing["text"]));
        // The end of the drawing's ID names the shape in a diagram (`s3`).
        let number = id.rsplit_once('#').map_or("", |(_, n)| n);
        let mark = format!(
            r##"{} data-shape="#{number}""##,
            out.mark(shown, name, drawing["name"].as_str().unwrap_or(id))
        );
        let rotation = drawing["transform"]["rot"]
            .as_str()
            .and_then(|r| r.parse::<f64>().ok())
            .unwrap_or(0.0)
            / 60_000.0;
        html.push_str(&drawings::drawing(
            drawing,
            rect,
            &escape(&text),
            &mark,
            rotation,
            document.assets,
            &defaults,
        ));
    }
    // The selection, and where the columns, rows and merges are for it.
    let edges = |edges: &[f64]| {
        edges
            .iter()
            .map(|e| format!("{e:.0}"))
            .collect::<Vec<_>>()
            .join(",")
    };
    let merged: Vec<String> = merges
        .iter()
        .map(|m| format!("[{},{},{},{}]", m.c0, m.r0, m.c1, m.r1))
        .collect();
    let _ = write!(
        html,
        r#"<div class="sel" hidden></div><script type="application/json" class="geo">{{"x":[{}],"y":[{}],"m":[{}]}}</script></div>"#,
        edges(&lines.x),
        edges(&lines.y),
        merged.join(",")
    );
    Ok(html)
}

/// The font a cell is drawn in: Japanese Excel draws all the text of a theme
/// font in its East Asian typeface, which the extraction gives beside the
/// Latin one.
fn excel_font(font: &Value) -> std::borrow::Cow<'_, Value> {
    match font["east_asian"].as_str() {
        Some(east_asian) if east_asian != "mixed" && font["latin"] != east_asian => {
            let mut drawn = font.clone();
            drawn["latin"] = json!(east_asian);
            std::borrow::Cow::Owned(drawn)
        }
        _ => std::borrow::Cow::Borrowed(font),
    }
}

/// How a value is aligned when its format leaves it to the value.
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Text,
    Number,
    Centered,
}

/// The text a cell shows and its kind: the content's value, in the cell's
/// number format; a formula the content leaves as it is shows its result. A
/// number is fitted to the `room` the cell has.
fn display(
    cell: Option<Cell<'_>>,
    shown: Option<&Shown>,
    date1904: bool,
    room: Option<&Room>,
) -> (String, Kind) {
    let format = cell.map_or("General", |c| c.number_format());
    let value = match shown {
        Some(shown) if shown.formula && shown.original.is_none() => {
            cell.map(|c| c.value().to_json()).unwrap_or(Value::Null)
        }
        Some(shown) => shown.value.clone(),
        None => cell.map(|c| c.value().to_json()).unwrap_or(Value::Null),
    };
    let error = cell.is_some_and(|c| c.kind() == CellKind::Error)
        && shown.is_none_or(|s| s.original.is_none());
    match &value {
        Value::Null => (String::new(), Kind::Text),
        Value::Bool(b) => (
            (if *b { "TRUE" } else { "FALSE" }).to_owned(),
            Kind::Centered,
        ),
        Value::Number(number) => {
            let text = crate::excel::number_format::display(&value, format, date1904);
            let text = match number.as_f64() {
                Some(number) => fitted(text, number, format, room),
                None => text,
            };
            (text, Kind::Number)
        }
        Value::String(text) if error => (text.clone(), Kind::Centered),
        other => (plain(other), Kind::Text),
    }
}

/// What Excel shows of a number whose text is `text`: in the General format
/// at most 11 characters besides a sign, shortened to fit the `room` there is;
/// in another, the text when it fits. What does not fit fills the cell with
/// as many `#` as fit.
fn fitted(text: String, number: f64, format: &str, room: Option<&Room>) -> String {
    if format.eq_ignore_ascii_case("general") {
        let fits = |text: &str| {
            text.trim_start_matches('-').chars().count() <= 11
                && room.is_none_or(|room| room.fits(room.general(text)))
        };
        if let Some(text) = crate::excel::number_format::general_within(number, fits) {
            return text;
        }
    } else if room.is_none_or(|room| room.fits(room.font.text(&text))) {
        return text;
    }
    room.map_or(text, |room| {
        "#".repeat((room.width / room.font.width('#')).floor().max(0.0) as usize)
    })
}

/// The room a cell gives its text, as Excel measures it: the width between
/// the edges of its cells less what it leaves around the text, in pixels, for
/// text in the cell's font at 96 per inch, whatever the screen (measured in
/// Excel).
struct Room {
    font: crate::excel::glyphs::Font,
    /// The size of the font, in points.
    size: f64,
    width: f64,
}

impl Room {
    fn new(look: &SheetLook, area: Range, font: &Value, bold: bool) -> Self {
        use crate::excel::glyphs;
        // A theme font is drawn in the theme's East Asian typeface in
        // Japanese Excel; another has one name.
        let name = [&font["east_asian"], &font["latin"]]
            .into_iter()
            .filter_map(Value::as_str)
            .find(|name| *name != "mixed");
        let size = font["size"].as_f64().unwrap_or(11.0);
        let columns = (area.c0..=area.c1).map(|c| look.characters(c)).sum::<f64>();
        Self::within(
            glyphs::Font::new(name, bold, size, glyphs::PAGE),
            size,
            columns * look.unit(),
        )
    }

    /// The room in cells `width` pixels wide for text in `font`.
    fn within(font: crate::excel::glyphs::Font, size: f64, width: f64) -> Self {
        // The room left is more for a wider font.
        let padding = 2.0 * (font.digit() / 4.0).ceil() + 0.5;
        Self {
            width: width - padding,
            font,
            size,
        }
    }

    /// Whether text `width` pixels wide fits: a little over is taken as
    /// fitting, as the column widths are Excel's to a few hundredths.
    fn fits(&self, width: f64) -> bool {
        width <= self.width + 0.05
    }

    /// The width Excel takes a number in the General format to have: its
    /// sign, and the rest at least a digit for each character.
    fn general(&self, text: &str) -> f64 {
        let body = text.trim_start_matches('-');
        let sign = if body.len() < text.len() {
            self.font.width('-')
        } else {
            0.0
        };
        sign + (self.font.digit() * body.chars().count() as f64).max(self.font.text(body))
    }
}

/// The sides of cells' borders, gathered by the line they lie on and how
/// they are drawn, so that an edge two cells share is drawn once and the
/// sides of a run of cells along one line are one line.
#[derive(Default)]
struct Rules<'a> {
    /// Each way of drawing a side, in the order the sheet first uses it.
    looks: Vec<&'a crate::excel::BorderSide>,
    /// The spans from and to along each line: by the look, whether the
    /// line is vertical, and where it stands across, in hundredths of a pixel.
    spans: BTreeMap<(usize, bool, i64), Vec<(f64, f64)>>,
}

impl<'a> Rules<'a> {
    fn add(&mut self, side: &'a crate::excel::BorderSide, (x0, y0, x1, y1): (f64, f64, f64, f64)) {
        let look = match self.looks.iter().position(|l| *l == side) {
            Some(look) => look,
            None => {
                self.looks.push(side);
                self.looks.len() - 1
            }
        };
        let vertical = (x0 - x1).abs() < 0.01;
        let (across, from, to) = if vertical { (x0, y0, y1) } else { (y0, x0, x1) };
        self.spans
            .entry((look, vertical, (across * 100.0).round() as i64))
            .or_default()
            .push((from.min(to), from.max(to)));
    }

    /// The SVG lines of the spans, those meeting or overlapping joined.
    fn draw(mut self) -> String {
        let mut svg = String::new();
        for ((look, vertical, across), spans) in &mut self.spans {
            spans.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
            let across = *across as f64 / 100.0;
            let mut joined: Vec<(f64, f64)> = vec![];
            for &(from, to) in spans.iter() {
                match joined.last_mut() {
                    Some(last) if from <= last.1 + 0.01 => last.1 = last.1.max(to),
                    _ => joined.push((from, to)),
                }
            }
            for (from, to) in joined {
                let line = if *vertical {
                    (across, from, across, to)
                } else {
                    (from, across, to, across)
                };
                border(&mut svg, self.looks[*look], line);
            }
        }
        svg
    }
}

/// Draws a side of a cell's border along `line` (x0, y0, x1, y1).
fn border(svg: &mut String, side: &crate::excel::BorderSide, line: (f64, f64, f64, f64)) {
    let color = side
        .color
        .as_deref()
        .map_or_else(|| "#000".to_owned(), |rgb| format!("#{rgb}"));
    let (width, dash) = match side.style.as_str() {
        "medium" => (2.0, ""),
        "thick" => (3.0, ""),
        "double" => (1.0, ""),
        "hair" => (1.0, "1 1"),
        "dotted" => (1.0, "1 2"),
        "dashed" => (1.0, "3 2"),
        "dashDot" => (1.0, "3 2 1 2"),
        "dashDotDot" => (1.0, "3 2 1 2 1 2"),
        "mediumDashed" => (2.0, "5 2"),
        "mediumDashDot" | "slantDashDot" => (2.0, "5 2 1 2"),
        "mediumDashDotDot" => (2.0, "5 2 1 2 1 2"),
        _ => (1.0, ""),
    };
    let dash = if dash.is_empty() {
        String::new()
    } else {
        format!(r#" stroke-dasharray="{dash}""#)
    };
    let (x0, y0, x1, y1) = line;
    // A line sits on the edge, half in each cell; a double one is two lines.
    let offsets: &[f64] = if side.style == "double" {
        &[-1.0, 1.0]
    } else {
        &[0.0]
    };
    let vertical = (x0 - x1).abs() < 0.01;
    for offset in offsets {
        let (dx, dy) = if vertical {
            (*offset, 0.0)
        } else {
            (0.0, *offset)
        };
        let _ = write!(
            svg,
            r#"<line x1="{:.1}" y1="{:.1}" x2="{:.1}" y2="{:.1}" stroke="{color}" stroke-width="{width}"{dash}/>"#,
            x0 - 0.5 + dx,
            y0 - 0.5 + dy,
            x1 - 0.5 + dx,
            y1 - 0.5 + dy
        );
    }
}

/// The box of a drawing on the sheet: where its anchor places it, or, for a
/// drawing in a group, where the group's transform takes its own.
fn drawing_rect(
    drawing: &Value,
    by_id: &HashMap<&str, &Value>,
    lines: &Lines,
    depth: usize,
) -> Option<Rect> {
    let transform = &drawing["transform"];
    if depth < 16
        && let Some(group) = drawing["group"].as_str().and_then(|g| by_id.get(g))
        && let Some(outer) = drawing_rect(group, by_id, lines, depth + 1)
    {
        let frame = &group["transform"];
        if let (Some(x), Some(y), Some(w), Some(h), Some(cx), Some(cy), Some(cw), Some(ch)) = (
            emu(&transform["off"]["x"]),
            emu(&transform["off"]["y"]),
            emu(&transform["ext"]["cx"]),
            emu(&transform["ext"]["cy"]),
            emu(&frame["chOff"]["x"]),
            emu(&frame["chOff"]["y"]),
            emu(&frame["chExt"]["cx"]),
            emu(&frame["chExt"]["cy"]),
        ) {
            let (sx, sy) = (
                if cw > 0.0 { outer.w / cw } else { 1.0 },
                if ch > 0.0 { outer.h / ch } else { 1.0 },
            );
            return Some(Rect {
                x: outer.x + (x - cx) * sx,
                y: outer.y + (y - cy) * sy,
                w: w * sx,
                h: h * sy,
            });
        }
    }
    let anchor = &drawing["anchor"];
    let (x, y) = match anchor["kind"].as_str()? {
        "absoluteAnchor" => (
            emu(&anchor["pos"]["x"])? + HEAD_W,
            emu(&anchor["pos"]["y"])? + HEAD_H,
        ),
        _ => lines.marker(&anchor["from"])?,
    };
    let (w, h) = match anchor["kind"].as_str()? {
        "twoCellAnchor" => {
            let (x1, y1) = lines.marker(&anchor["to"])?;
            (x1 - x, y1 - y)
        }
        _ => (
            emu(&anchor["ext"]["cx"]).unwrap_or(0.0),
            emu(&anchor["ext"]["cy"]).unwrap_or(0.0),
        ),
    };
    Some(Rect { x, y, w, h })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_read_in_either_order() {
        let r = range("C5:A1").unwrap();
        assert_eq!((r.c0, r.r0, r.c1, r.r1), (1, 1, 3, 5));
        let one = range("$B$2").unwrap();
        assert_eq!((one.c0, one.r0, one.c1, one.r1), (2, 2, 2, 2));
    }

    #[test]
    fn numbers_wider_than_their_cell_show_as_excel_fits_them() {
        use crate::excel::glyphs::{Font, PAGE};
        // As Excel shows them in cells `width` pixels wide, in Calibri of 11
        // points unless another font is given.
        let fit = |text: &str, number: f64, format: &str, width: f64| {
            let room = Room::within(Font::new(Some("Calibri"), false, 11.0, PAGE), 11.0, width);
            fitted(text.to_owned(), number, format, Some(&room))
        };
        assert_eq!(fit("1234567.89", 1234567.89, "General", 54.4), "1234568");
        assert_eq!(fit("1234567.89", 1234567.89, "General", 52.8), "1E+06");
        assert_eq!(fit("123456789", 123456789.0, "General", 60.8), "1.23E+08");
        assert_eq!(fit("0.123456789", 0.123456789, "General", 54.4), "0.12346");
        // A sign is measured as it is, the other characters at least a digit.
        assert_eq!(fit("-1234567.89", -1234567.89, "General", 44.8), "-1E+06");
        assert_eq!(fit("-1234567.89", -1234567.89, "General", 60.0), "-1234568");
        // Rounded to nothing, a number is shown in scientific notation, else
        // as nothing.
        assert_eq!(fit("-0.000123456", -0.000123456, "General", 44.8), "-1E-04");
        assert_eq!(fit("-0.000123456", -0.000123456, "General", 16.8), "-0");
        assert_eq!(fit("1.5E-10", 1.5e-10, "General", 40.0), "2E-10");
        assert_eq!(fit("1.5E-10", 1.5e-10, "General", 12.8), "0");
        // The General format shows no more than 11 characters besides a sign.
        assert_eq!(
            fit("3.14159265358979", std::f64::consts::PI, "General", 200.0),
            "3.141592654"
        );
        // Another format shows its text whole or fills the cell with `#`.
        assert_eq!(fit("1,234,567", 1234567.0, "#,##0", 61.6), "1,234,567");
        assert_eq!(fit("1,234,567", 1234567.0, "#,##0", 60.8), "########");
        assert_eq!(fit("2023/3/15", 45000.0, "yyyy/m/d", 64.8), "########");
        assert_eq!(fit("12.35%", 0.123456, "0.00%", 48.0), "12.35%");
        // A column too narrow for one shows nothing.
        assert_eq!(fit("12345", 12345.0, "#,##0", 11.2), "");
        // Each font is measured in its own widths and leaves its own room.
        let meiryo = Room::within(Font::new(Some("メイリオ"), false, 11.0, PAGE), 11.0, 31.2);
        assert_eq!(
            fitted("1,234,567".into(), 1234567.0, "#,##0", Some(&meiryo)),
            "##"
        );
        // A number not fitted, as one turned, keeps no more than General's
        // characters.
        assert_eq!(
            fitted("123456789012".into(), 123456789012.0, "General", None),
            "1.23457E+11"
        );
    }

    #[test]
    fn a_theme_font_is_drawn_in_its_east_asian_typeface() {
        let theme = json!({"latin":"Calibri","east_asian":"游ゴシック","size":11});
        assert_eq!(excel_font(&theme)["latin"], "游ゴシック");
        let own = json!({"latin":"Arial","east_asian":"Arial"});
        assert!(matches!(excel_font(&own), std::borrow::Cow::Borrowed(_)));
    }

    #[test]
    fn anchor_offsets_stop_at_the_edge_of_their_cell() {
        // Column B hidden and C four pixels wide; row 2 two pixels high.
        let lines = Lines {
            x: vec![40.0, 40.0, 100.0, 100.0, 104.0, 168.0],
            y: vec![20.0, 20.0, 40.0, 42.0, 62.0],
        };
        let at = |col: u32, col_off: f64, row: u32, row_off: f64| {
            lines
                .marker(&json!({"col":col,"colOff":col_off * EMU_PER_PIXEL,
                    "row":row,"rowOff":row_off * EMU_PER_PIXEL}))
                .unwrap()
        };
        assert_eq!(at(1, 20.0, 1, 10.0), (100.0, 42.0));
        assert_eq!(at(2, 20.0, 0, 10.0), (104.0, 30.0));
        assert_eq!(at(3, 20.0, 2, 30.0), (124.0, 62.0));
    }

    #[test]
    fn borders_cells_share_and_run_along_are_one_line() {
        let thin = crate::excel::BorderSide {
            style: "thin".into(),
            color: None,
        };
        let thick = crate::excel::BorderSide {
            style: "thick".into(),
            color: None,
        };
        let mut rules = Rules::default();
        // The right of A1 and the left of B1, then the rights of A2 and A3.
        rules.add(&thin, (50.0, 0.0, 50.0, 20.0));
        rules.add(&thin, (50.0, 0.0, 50.0, 20.0));
        rules.add(&thin, (50.0, 20.0, 50.0, 40.0));
        rules.add(&thin, (50.0, 40.0, 50.0, 60.0));
        // Apart from them: a thick side on the same line, and a thin one
        // further down past a gap.
        rules.add(&thick, (50.0, 60.0, 50.0, 80.0));
        rules.add(&thin, (50.0, 100.0, 50.0, 120.0));
        let svg = rules.draw();
        assert_eq!(svg.matches("<line").count(), 3, "{svg}");
        assert!(
            svg.contains(r#"x1="49.5" y1="-0.5" x2="49.5" y2="59.5""#),
            "{svg}"
        );
        assert!(svg.contains(r#"y1="99.5" x2="49.5" y2="119.5""#), "{svg}");
        assert!(svg.contains(r#"stroke-width="3""#), "{svg}");
    }

    #[test]
    fn a_double_border_is_two_lines_and_a_dashed_one_dashed() {
        let mut svg = String::new();
        border(
            &mut svg,
            &crate::excel::BorderSide {
                style: "double".into(),
                color: Some("FF0000".into()),
            },
            (10.0, 0.0, 10.0, 20.0),
        );
        assert_eq!(svg.matches("<line").count(), 2);
        assert!(svg.contains("#FF0000"));
        svg.clear();
        border(
            &mut svg,
            &crate::excel::BorderSide {
                style: "dashed".into(),
                color: None,
            },
            (0.0, 5.0, 30.0, 5.0),
        );
        assert!(svg.contains(r#"stroke-dasharray="3 2""#) && svg.contains("#000"));
    }
}
