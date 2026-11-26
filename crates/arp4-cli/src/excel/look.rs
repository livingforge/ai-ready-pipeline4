//! How the cells of a worksheet look in the original, beyond what the
//! extraction keeps of them: the sizes of the columns and rows, and the fill,
//! borders and alignment of each cell format. A view of the document
//! (`documents view`) lays the content out with them; nothing is written.
use super::fonts::Palette;
use super::overlays::{ConditionalLook, TableLook};
use super::*;
use crate::fonts::Color;
use foldhash::HashMap;

/// A side of a cell's border: Excel's line style (`thin`, `dashed`, …) and
/// its colour, `RRGGBB`, none for automatic.
#[derive(Clone, Debug, PartialEq)]
pub struct BorderSide {
    pub style: String,
    pub color: Option<String>,
}

/// What a cell format (`cellXfs`) shows besides its font and number format.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FormatLook {
    /// The fill behind the text, `RRGGBB`; none when nothing is painted.
    pub fill: Option<String>,
    /// The left, right, top and bottom sides of the border.
    pub borders: [Option<BorderSide>; 4],
    /// The horizontal alignment; none for general.
    pub horizontal: Option<String>,
    /// The vertical alignment; none for bottom, Excel's default.
    pub vertical: Option<String>,
    pub wrap: bool,
    pub shrink: bool,
    pub indent: u32,
    /// Degrees counterclockwise (1–180 as Excel stores them, 91–180 down), or
    /// 255 for letters stacked one under another.
    pub rotation: u32,
    /// The gradient the fill is, as CSS writes one; `fill` is its first
    /// colour.
    pub gradient: Option<String>,
    /// The line between the corners of the cell, and whether it runs up from
    /// the bottom left and down from the top left.
    pub diagonal: Option<(BorderSide, bool, bool)>,
    /// The number format by its number in the workbook (`numFmtId`): one
    /// under 164 is Excel's own, which it shows by the system's language.
    pub number_format: u32,
    /// The font underlines with two lines.
    pub double_underline: bool,
}

impl FormatLook {
    /// Whether the format shows on a cell that holds nothing.
    pub fn paints(&self) -> bool {
        self.fill.is_some() || self.diagonal.is_some() || self.borders.iter().any(Option::is_some)
    }
}

/// The sizes and cell formats of a worksheet, in pixels at 96 per inch.
pub struct SheetLook {
    digit: f64,
    column_width: f64,
    row_height: f64,
    /// Columns of their own width, in characters: `(min, max, width)`, zero
    /// for hidden ones.
    columns: Vec<(u32, u32, f64)>,
    /// Rows of their own height, in points, zero for hidden ones.
    rows: HashMap<u32, f64>,
    /// The format of each cell the sheet writes, by (column, row).
    cells: HashMap<(u32, u32), usize>,
    /// The format of the cells of a row that writes one (`customFormat`).
    row_formats: HashMap<u32, usize>,
    /// The format of the cells of columns that give one: `(min, max, format)`.
    column_formats: Vec<(u32, u32, usize)>,
    formats: std::sync::Arc<Vec<FormatLook>>,
    /// Whether the sheet shows its gridlines.
    pub grid_lines: bool,
    /// The sheet's conditional formats, in the order of their priorities.
    pub conditional: Vec<ConditionalLook>,
    /// The sheet's tables with what their styles give.
    pub tables: Vec<TableLook>,
    /// Whether the workbook was made by Japanese Excel, which shows Excel's
    /// own number formats as Japanese writes them.
    pub japanese: bool,
    /// The parts of the text of a cell set above or below its line, by
    /// (column, row): the characters each covers and whether it is above.
    pub scripts: HashMap<(u32, u32), ScriptRuns>,
}

/// The parts of a text set above or below its line: the characters each
/// covers, from and to, and whether it is above.
pub type ScriptRuns = std::sync::Arc<Vec<(usize, usize, bool)>>;

impl SheetLook {
    /// The width of the column numbered `column` (from 1), in pixels.
    pub fn width(&self, column: u32) -> f64 {
        (self.characters(column) * self.digit).round()
    }

    /// The width of the column numbered `column` as the sheet writes it, in
    /// digits of the normal font with the padding Excel adds to them.
    pub fn characters(&self, column: u32) -> f64 {
        self.columns
            .iter()
            .find(|(min, max, _)| (*min..=*max).contains(&column))
            .map_or(self.column_width, |(_, _, width)| *width)
    }

    /// The width of a unit of column width, a digit of the normal font on
    /// the screen, in pixels.
    pub fn unit(&self) -> f64 {
        self.digit
    }

    /// The height of the row numbered `row` (from 1), in pixels.
    pub fn height(&self, row: u32) -> f64 {
        (self.rows.get(&row).copied().unwrap_or(self.row_height) * 4.0 / 3.0).round()
    }

    /// The format of the cell at `column` and `row`: its own, else its row's,
    /// else its column's.
    pub fn cell(&self, column: u32, row: u32) -> Option<&FormatLook> {
        let format = self
            .cells
            .get(&(column, row))
            .or_else(|| self.row_formats.get(&row))
            .or_else(|| {
                self.column_formats
                    .iter()
                    .find(|(min, max, _)| (*min..=*max).contains(&column))
                    .map(|(_, _, format)| format)
            })?;
        self.formats.get(*format)
    }

    /// The cells the sheet writes whose format paints a fill or a border, by
    /// (column, row): those that show though they hold nothing.
    pub fn painted(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        self.cells
            .iter()
            .filter_map(|(place, format)| self.formats.get(*format)?.paints().then_some(*place))
    }
}

/// How a shape paints its area or its line.
#[derive(Clone, Debug, PartialEq)]
pub enum Paint {
    /// Nothing is painted.
    Nothing,
    /// `RRGGBB`.
    Color(String),
    /// A gradient, picture or pattern, which is not resolved.
    Unresolved,
}

/// What an Excel shape or connector shows besides its place and text: the
/// fill and line its properties or its style give, the width of its line in
/// points, and where its text body puts the text (`anchor` t, ctr or b;
/// `algn` of the first paragraph l, ctr or r).
#[derive(Clone, Debug, PartialEq)]
pub struct DrawingLook {
    pub fill: Paint,
    pub line: Paint,
    pub line_width: Option<f64>,
    pub vertical: Option<String>,
    pub horizontal: Option<String>,
    /// The gradient the shape is filled with, when it is.
    pub gradient: Option<Gradient>,
    /// How much of what is behind the fill it hides, from 0 to 1, when not
    /// all of it.
    pub fill_opacity: Option<f64>,
    pub extras: ShapeExtras,
    /// The parts of the shape's text written bold, by the characters of
    /// the text each covers, from and to.
    pub bold: Vec<(usize, usize)>,
}

/// A gradient fill: its colours (`RRGGBB`) by their place from 0 to 1, and
/// the angle it runs at, in degrees clockwise from left to right; none for
/// one spreading from a point.
#[derive(Clone, Debug, PartialEq)]
pub struct Gradient {
    pub stops: Vec<(f64, String)>,
    pub angle: Option<f64>,
}

/// What a DrawingML shape's geometry and text body say beyond its preset's
/// name, the same in a worksheet drawing and on a slide.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ShapeExtras {
    /// The adjustments of the preset geometry (`a:avLst`), by name.
    pub adjust: Vec<(String, f64)>,
    /// The paths of a custom geometry (`a:custGeom`).
    pub paths: Vec<CustomPath>,
    /// The direction of the text (`vert` of `a:bodyPr`), unless horizontal.
    pub text_direction: Option<String>,
    /// The size of the arrowhead at the head and at the tail of the line
    /// (`w` and `len` of `a:headEnd` and `a:tailEnd`: sm, med, lg).
    pub head_size: Option<(String, String)>,
    pub tail_size: Option<(String, String)>,
}

/// A path of a custom geometry: the size of the box its points are given
/// in, and the path as SVG writes one.
#[derive(Clone, Debug, PartialEq)]
pub struct CustomPath {
    pub width: f64,
    pub height: f64,
    pub d: String,
    pub filled: bool,
    pub stroked: bool,
}

/// The [`ShapeExtras`] of a shape or connector element (`sp`, `cxnSp`) of a
/// worksheet drawing or a slide.
pub fn shape_extras(shape: Node<'_, '_>) -> ShapeExtras {
    fn local<'a, 'i>(node: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
        node.children()
            .find(|n| n.is_element() && n.tag_name().name() == name)
    }
    fn drawing<'a, 'i>(node: Option<Node<'a, 'i>>, name: &str) -> Option<Node<'a, 'i>> {
        node?.children().find(|n| n.has_tag_name((DRAWING, name)))
    }
    let properties = local(shape, "spPr");
    let number = |node: Node<'_, '_>, name: &str| node.attribute(name)?.parse::<f64>().ok();
    let adjust = drawing(drawing(properties, "prstGeom"), "avLst")
        .into_iter()
        .flat_map(|list| list.children().filter(|n| n.has_tag_name((DRAWING, "gd"))))
        .filter_map(|guide| {
            let value = guide.attribute("fmla")?.strip_prefix("val ")?;
            Some((
                guide.attribute("name")?.to_owned(),
                value.trim().parse().ok()?,
            ))
        })
        .collect();
    // A path without a size of its own is in the shape's (EMUs).
    let extent = drawing(drawing(properties, "xfrm"), "ext")
        .and_then(|ext| Some((number(ext, "cx")?, number(ext, "cy")?)));
    let paths = drawing(drawing(properties, "custGeom"), "pathLst")
        .into_iter()
        .flat_map(|list| {
            list.children()
                .filter(|n| n.has_tag_name((DRAWING, "path")))
        })
        .filter_map(|path| {
            let (width, height) = match (number(path, "w"), number(path, "h")) {
                (Some(w), Some(h)) if w > 0.0 && h > 0.0 => (w, h),
                _ => extent?,
            };
            Some(CustomPath {
                width,
                height,
                d: custom_path(path)?,
                filled: path.attribute("fill") != Some("none"),
                stroked: !matches!(path.attribute("stroke"), Some("0" | "false")),
            })
        })
        .collect();
    let outline = drawing(properties, "ln");
    let size = |name: &str| {
        let end = drawing(outline, name)?;
        Some((
            end.attribute("w").unwrap_or("med").to_owned(),
            end.attribute("len").unwrap_or("med").to_owned(),
        ))
    };
    ShapeExtras {
        adjust,
        paths,
        text_direction: drawing(local(shape, "txBody"), "bodyPr")
            .and_then(|body| body.attribute("vert"))
            .filter(|vert| *vert != "horz")
            .map(str::to_owned),
        head_size: size("headEnd"),
        tail_size: size("tailEnd"),
    }
}

/// The path of a custom geometry's `a:path` as SVG writes one; none when it
/// names a point by a guide, which is not worked out.
fn custom_path(path: Node<'_, '_>) -> Option<String> {
    use std::fmt::Write as _;
    let mut d = String::new();
    let mut current = (0.0, 0.0);
    for step in path.children().filter(Node::is_element) {
        let points = step
            .children()
            .filter(|n| n.has_tag_name((DRAWING, "pt")))
            .map(|pt| {
                Some((
                    pt.attribute("x")?.parse::<f64>().ok()?,
                    pt.attribute("y")?.parse::<f64>().ok()?,
                ))
            })
            .collect::<Option<Vec<(f64, f64)>>>()?;
        let letter = match (step.tag_name().name(), points.len()) {
            ("moveTo", 1) => 'M',
            ("lnTo", 1) => 'L',
            ("quadBezTo", 2) => 'Q',
            ("cubicBezTo", 3) => 'C',
            ("close", _) => {
                d.push('Z');
                continue;
            }
            ("arcTo", _) => {
                let value = |name: &str| step.attribute(name)?.parse::<f64>().ok();
                let (rx, ry) = (value("wR")?, value("hR")?);
                let (start, swing) = (value("stAng")? / 60_000.0, value("swAng")? / 60_000.0);
                // An angle is that of the ray from the centre; the point of
                // the ellipse on it is at another angle of its circle.
                let on = |degrees: f64| {
                    let angle = degrees.to_radians();
                    let t = (rx * angle.sin()).atan2(ry * angle.cos());
                    (rx * t.cos(), ry * t.sin())
                };
                let (from, to) = (on(start), on(start + swing));
                current = (current.0 - from.0 + to.0, current.1 - from.1 + to.1);
                let _ = write!(
                    d,
                    "A{rx:.0},{ry:.0} 0 {} {} {:.0},{:.0}",
                    u8::from(swing.abs() > 180.0),
                    u8::from(swing > 0.0),
                    current.0,
                    current.1
                );
                continue;
            }
            _ => return None,
        };
        d.push(letter);
        for (index, (x, y)) in points.iter().enumerate() {
            let _ = write!(d, "{}{x:.0},{y:.0}", if index > 0 { " " } else { "" });
        }
        current = *points.last()?;
    }
    (!d.is_empty()).then_some(d)
}

/// The look of each shape and connector of the drawings of `book`, by its
/// extraction ID (`<drawing part>#<cNvPr id>`). A shape that neither its
/// properties nor a style paint shows nothing, as Excel draws it.
pub fn drawing_looks(book: &Workbook) -> Result<HashMap<String, DrawingLook>> {
    let styles = book
        .parts
        .get("xl/styles.xml")
        .map(|b| xml(b))
        .transpose()?;
    let palette = Palette::new(fonts::workbook_theme(&book.parts)?, styles.as_ref());
    let map = crate::fonts::default_color_map();
    let context = crate::fonts::ColorContext {
        theme: palette.theme.as_ref(),
        map: &map,
        placeholder: None,
    };
    let color = |node: Node<'_, '_>| match crate::fonts::drawing_color(node, &context) {
        Some(Color::Rgb { rgb, .. }) => Paint::Color(rgb),
        _ => Paint::Unresolved,
    };
    // The paint of the fill among `node`'s children, none when it names none.
    let fill_of = |node: Node<'_, '_>| {
        node.children()
            .filter(Node::is_element)
            .find_map(|child| match child.tag_name().name() {
                "noFill" => Some(Paint::Nothing),
                "solidFill" => Some(
                    child
                        .children()
                        .find(Node::is_element)
                        .map_or(Paint::Unresolved, color),
                ),
                "gradFill" | "blipFill" | "pattFill" | "grpFill" => Some(Paint::Unresolved),
                _ => None,
            })
    };
    // The gradient of the fill among `node`'s children: its stops in the
    // colours that resolve, and the angle a linear one runs at.
    let gradient_of = |node: Node<'_, '_>| {
        let fill = node
            .children()
            .find(|n| n.has_tag_name((DRAWING, "gradFill")))?;
        let mut stops: Vec<(f64, String)> = fill
            .descendants()
            .filter(|n| n.has_tag_name((DRAWING, "gs")))
            .filter_map(|stop| {
                let Paint::Color(rgb) = color(stop.children().find(Node::is_element)?) else {
                    return None;
                };
                Some((stop.attribute("pos")?.parse::<f64>().ok()? / 100_000.0, rgb))
            })
            .collect();
        stops.sort_by(|a, b| a.0.total_cmp(&b.0));
        (stops.len() > 1).then(|| Gradient {
            stops,
            angle: fill
                .children()
                .find(|n| n.has_tag_name((DRAWING, "lin")))
                .map(|lin| {
                    lin.attribute("ang")
                        .and_then(|a| a.parse::<f64>().ok())
                        .unwrap_or(0.0)
                        / 60_000.0
                }),
        })
    };
    // How much a solid fill among `node`'s children hides (`a:alpha`).
    let opacity_of = |node: Node<'_, '_>| {
        node.children()
            .find(|n| n.has_tag_name((DRAWING, "solidFill")))?
            .descendants()
            .find(|n| n.has_tag_name((DRAWING, "alpha")))?
            .attribute("val")?
            .parse::<f64>()
            .ok()
            .map(|alpha| (alpha / 100_000.0).clamp(0.0, 1.0))
    };
    // The paint a style reference gives: nothing for index 0.
    let reference = |style: Option<Node<'_, '_>>, name: &str| {
        let reference = style?
            .children()
            .find(|n| n.has_tag_name((DRAWING, name)))?;
        Some(if reference.attribute("idx") == Some("0") {
            Paint::Nothing
        } else {
            reference
                .children()
                .find(Node::is_element)
                .map_or(Paint::Unresolved, color)
        })
    };
    let mut looks = HashMap::default();
    for (part, bytes) in &book.parts {
        if !(part.starts_with("xl/drawings/")
            && part.ends_with(".xml")
            && !part.contains("/_rels/"))
        {
            continue;
        }
        let Ok(doc) = xml(bytes) else {
            continue;
        };
        for shape in doc
            .descendants()
            .filter(|n| n.has_tag_name((XDR, "sp")) || n.has_tag_name((XDR, "cxnSp")))
        {
            let Some(id) = shape
                .descendants()
                .find(|n| n.has_tag_name((XDR, "cNvPr")))
                .and_then(|n| n.attribute("id"))
            else {
                continue;
            };
            let properties = shape.children().find(|n| n.has_tag_name((XDR, "spPr")));
            let style = shape.children().find(|n| n.has_tag_name((XDR, "style")));
            let outline =
                properties.and_then(|p| p.children().find(|n| n.has_tag_name((DRAWING, "ln"))));
            let body = shape.children().find(|n| n.has_tag_name((XDR, "txBody")));
            let attribute = |node: Option<Node<'_, '_>>, tag: &str, name: &str| {
                node?
                    .descendants()
                    .find(|n| n.has_tag_name((DRAWING, tag)))?
                    .attribute(name)
                    .map(str::to_owned)
            };
            looks.insert(
                format!("{part}#{id}"),
                DrawingLook {
                    fill: properties
                        .and_then(fill_of)
                        .or_else(|| reference(style, "fillRef"))
                        .unwrap_or(Paint::Nothing),
                    line: outline
                        .and_then(fill_of)
                        .or_else(|| reference(style, "lnRef"))
                        .unwrap_or(Paint::Nothing),
                    line_width: outline
                        .and_then(|l| l.attribute("w"))
                        .and_then(|w| w.parse::<f64>().ok())
                        .map(|emu| emu / 12_700.0),
                    vertical: attribute(body, "bodyPr", "anchor"),
                    horizontal: attribute(body, "pPr", "algn"),
                    gradient: properties.and_then(gradient_of),
                    fill_opacity: properties.and_then(opacity_of),
                    extras: shape_extras(shape),
                    bold: bold_runs(body),
                },
            );
        }
    }
    Ok(looks)
}

/// The parts of the text of a shape's text body its runs write bold
/// (`b` of `a:rPr`), by the characters of the text each covers: its
/// paragraphs are a line each.
fn bold_runs(body: Option<Node<'_, '_>>) -> Vec<(usize, usize)> {
    let mut bold = vec![];
    let mut at = 0;
    for (index, paragraph) in body
        .into_iter()
        .flat_map(|body| body.children().filter(|n| n.has_tag_name((DRAWING, "p"))))
        .enumerate()
    {
        let (runs, end) = crate::fonts::placed_runs(paragraph, at + usize::from(index > 0));
        for (run, chars) in runs {
            if run
                .children()
                .find(|n| n.has_tag_name((DRAWING, "rPr")))
                .is_some_and(|p| matches!(p.attribute("b"), Some("1" | "true")))
            {
                bold.push((chars.start, chars.end));
            }
        }
        at = end;
    }
    bold
}

/// The look of each worksheet of `book`, by sheet name.
pub fn sheet_looks(book: &Workbook) -> Result<BTreeMap<String, SheetLook>> {
    let styles = book
        .parts
        .get("xl/styles.xml")
        .map(|b| xml(b))
        .transpose()?;
    let palette = Palette::new(fonts::workbook_theme(&book.parts)?, styles.as_ref());
    let formats = std::sync::Arc::new(match &styles {
        Some(doc) => cell_looks(doc.root_element(), &palette),
        None => vec![],
    });
    let measures = fit::Styles::read(&book.parts)?;
    let styles_root = styles.as_ref().map(|doc| doc.root_element());
    let differential = styles_root
        .map(|root| overlays::differential_looks(root, &palette))
        .unwrap_or_default();
    let japanese = styles_root.is_some_and(japanese);
    let scripts = match book.parts.get("xl/sharedStrings.xml") {
        Some(bytes) => script_runs(&xml(bytes)?),
        None => HashMap::default(),
    };
    let mut looks = BTreeMap::new();
    for sheet in &book.sheets {
        let part = string(&sheet["part"])?;
        let Some(bytes) = book.parts.get(part) else {
            continue;
        };
        let doc = xml(bytes)?;
        let mut look = sheet_look(&doc, &measures, formats.clone())?;
        look.conditional = overlays::conditional_looks(doc.root_element(), &differential, &palette);
        look.tables =
            overlays::table_looks(&book.parts, part, styles_root, &differential, &palette);
        look.japanese = japanese;
        // The cells whose shared string sets part of its text above or
        // below the line.
        if !scripts.is_empty() {
            for cell in doc
                .descendants()
                .filter(|n| n.has_tag_name((NS, "c")) && n.attribute("t") == Some("s"))
            {
                let index = child(cell, "v")
                    .and_then(|v| v.text())
                    .and_then(|v| v.trim().parse::<usize>().ok());
                if let (Some(found), Some(Ok(place))) = (
                    index.and_then(|index| scripts.get(&index)),
                    cell.attribute("r").map(coordinate),
                ) {
                    look.scripts.insert(place, found.clone());
                }
            }
        }
        looks.insert(string(&sheet["name"])?.to_owned(), look);
    }
    Ok(looks)
}

/// The look of a worksheet `book` does not have yet, such as one an
/// operation adds: its default sizes and formats.
pub fn blank_sheet_look(book: &Workbook) -> Result<SheetLook> {
    let styles = book
        .parts
        .get("xl/styles.xml")
        .map(|b| xml(b))
        .transpose()?;
    let palette = Palette::new(fonts::workbook_theme(&book.parts)?, styles.as_ref());
    let formats = match &styles {
        Some(doc) => cell_looks(doc.root_element(), &palette),
        None => vec![],
    };
    let text = format!(r#"<worksheet xmlns="{NS}"/>"#);
    let blank = Document::parse(&text)?;
    sheet_look(
        &blank,
        &fit::Styles::read(&book.parts)?,
        std::sync::Arc::new(formats),
    )
}

fn sheet_look(
    doc: &Document<'_>,
    measures: &fit::Styles,
    formats: std::sync::Arc<Vec<FormatLook>>,
) -> Result<SheetLook> {
    let root = doc.root_element();
    let format = child(root, "sheetFormatPr");
    let number = |name: &str| {
        format
            .and_then(|f| f.attribute(name))
            .and_then(|v| v.parse::<f64>().ok())
    };
    let flag = |node: Node<'_, '_>, name: &str| matches!(node.attribute(name), Some("1" | "true"));
    // Excel takes the default height only when it is fixed, and otherwise
    // fits the rows to the normal font.
    let row_height = if format.is_some_and(|f| flag(f, "customHeight")) {
        number("defaultRowHeight").unwrap_or(measures.row)
    } else {
        measures.row
    };
    // A sheet hiding the rows it does not use (`zeroHeight`, as hiding the
    // rows to the last one leaves it) shows only the rows it writes.
    let zero_height = format.is_some_and(|f| flag(f, "zeroHeight"));
    let mut columns = vec![];
    let mut column_formats = vec![];
    for col in child(root, "cols")
        .into_iter()
        .flat_map(|cols| cols.children().filter(|n| n.has_tag_name((NS, "col"))))
    {
        let bound = |name: &str| col.attribute(name).and_then(|v| v.parse::<u32>().ok());
        let (Some(min), Some(max)) = (bound("min"), bound("max")) else {
            continue;
        };
        let max = max.min(16_384);
        if flag(col, "hidden") {
            columns.push((min, max, 0.0));
        } else if let Some(width) = col.attribute("width").and_then(|w| w.parse().ok()) {
            columns.push((min, max, width));
        }
        if let Some(style) = col.attribute("style").and_then(|s| s.parse().ok()) {
            column_formats.push((min, max, style));
        }
    }
    let mut rows = HashMap::default();
    let mut row_formats = HashMap::default();
    let mut cells = HashMap::default();
    for row in child(root, "sheetData")
        .into_iter()
        .flat_map(|data| data.children().filter(|n| n.has_tag_name((NS, "row"))))
    {
        let Some(number) = row.attribute("r").and_then(|r| r.parse::<u32>().ok()) else {
            continue;
        };
        if flag(row, "hidden") {
            rows.insert(number, 0.0);
        } else if let Some(height) = row.attribute("ht").and_then(|h| h.parse().ok()) {
            rows.insert(number, height);
        } else if zero_height {
            rows.insert(number, row_height);
        }
        if flag(row, "customFormat")
            && let Some(style) = row.attribute("s").and_then(|s| s.parse().ok())
        {
            row_formats.insert(number, style);
        }
        for cell in row.children().filter(|n| n.has_tag_name((NS, "c"))) {
            let Some(address) = cell.attribute("r") else {
                continue;
            };
            let place = coordinate(address)?;
            cells.insert(
                place,
                cell.attribute("s")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0),
            );
        }
    }
    let grid_lines = child(root, "sheetViews")
        .and_then(|views| child(views, "sheetView"))
        .is_none_or(|view| !matches!(view.attribute("showGridLines"), Some("0" | "false")));
    Ok(SheetLook {
        digit: measures.digit,
        column_width: fit::default_column_width(format, measures.digit),
        row_height: if zero_height { 0.0 } else { row_height },
        columns,
        rows,
        cells,
        row_formats,
        column_formats,
        formats,
        grid_lines,
        conditional: vec![],
        tables: vec![],
        japanese: false,
        scripts: HashMap::default(),
    })
}

/// Whether the styles part `root` is one Japanese Excel writes: its normal
/// font is in the Japanese character set or has a Japanese name.
fn japanese(root: Node<'_, '_>) -> bool {
    let Some(font) = child(root, "fonts").and_then(|fonts| child(fonts, "font")) else {
        return false;
    };
    let value = |name: &str| child(font, name).and_then(|n| n.attribute("val"));
    value("charset") == Some("128")
        || value("name").is_some_and(|name| {
            !name.is_ascii()
                || [
                    "Yu Gothic",
                    "Meiryo",
                    "MS PGothic",
                    "MS Gothic",
                    "MS Mincho",
                    "MS PMincho",
                ]
                .iter()
                .any(|japanese| name.starts_with(japanese))
        })
}

/// The runs of each shared string that are set above or below the line
/// (`vertAlign`), by the string's number.
fn script_runs(doc: &Document<'_>) -> HashMap<usize, ScriptRuns> {
    let length = |node: Option<Node<'_, '_>>| {
        node.and_then(|t| t.text())
            .map_or(0, |t| decode_xstring(t).chars().count())
    };
    let mut found = HashMap::default();
    for (index, item) in doc
        .root_element()
        .children()
        .filter(Node::is_element)
        .enumerate()
    {
        let mut at = length(child(item, "t"));
        let mut runs = vec![];
        for run in item.children().filter(|n| n.has_tag_name((NS, "r"))) {
            let end = at + length(child(run, "t"));
            let script = child(run, "rPr")
                .and_then(|p| child(p, "vertAlign"))
                .and_then(|v| v.attribute("val"));
            match script {
                Some("superscript") => runs.push((at, end, true)),
                Some("subscript") => runs.push((at, end, false)),
                _ => {}
            }
            at = end;
        }
        if !runs.is_empty() {
            found.insert(index, std::sync::Arc::new(runs));
        }
    }
    found
}

/// How much of a cell a fill pattern covers with its foreground colour.
fn pattern_density(pattern: &str) -> f64 {
    match pattern {
        "solid" => 1.0,
        "darkGray" | "darkTrellis" => 0.75,
        "mediumGray" | "darkHorizontal" | "darkVertical" | "darkDown" | "darkUp" | "darkGrid" => {
            0.5
        }
        "lightGrid" | "lightTrellis" => 0.4,
        "lightGray" | "lightHorizontal" | "lightVertical" | "lightDown" | "lightUp" => 0.25,
        "gray125" => 0.125,
        "gray0625" => 0.0625,
        _ => 1.0,
    }
}

/// The colour `share` of `over` and the rest of `under` mix to, `RRGGBB`.
fn mixed(over: &str, under: &str, share: f64) -> String {
    let part = |rgb: &str, at: usize| {
        rgb.get(at..at + 2)
            .and_then(|hex| u8::from_str_radix(hex, 16).ok())
            .map_or(0.0, f64::from)
    };
    (0..3)
        .map(|k| {
            let value = part(over, k * 2) * share + part(under, k * 2) * (1.0 - share);
            format!("{:02X}", value.round().clamp(0.0, 255.0) as u8)
        })
        .collect()
}

/// The look of each cell format of the styles part `root`.
fn cell_looks(root: Node<'_, '_>, palette: &Palette) -> Vec<FormatLook> {
    let rgb = |node: Option<Node<'_, '_>>| match node.and_then(|n| palette.color(n)) {
        Some(Color::Rgb { rgb, .. }) => Some(rgb),
        _ => None,
    };
    // Each fill: its colour and, for a gradient, the gradient.
    let fills: Vec<(Option<String>, Option<String>)> = child(root, "fills")
        .into_iter()
        .flat_map(|fills| fills.children().filter(Node::is_element))
        .map(|fill| {
            if let Some(gradient) = child(fill, "gradientFill") {
                let stops: Vec<(f64, String)> = gradient
                    .children()
                    .filter(|n| n.has_tag_name((NS, "stop")))
                    .filter_map(|stop| {
                        Some((
                            stop.attribute("position")?.parse::<f64>().ok()?,
                            rgb(child(stop, "color"))?,
                        ))
                    })
                    .collect();
                let colors: Vec<String> = stops
                    .iter()
                    .map(|(at, rgb)| format!("#{rgb} {:.0}%", at * 100.0))
                    .collect();
                let css = match (colors.len() > 1, gradient.attribute("type")) {
                    (false, _) => None,
                    (true, Some("path")) => Some(format!(
                        "radial-gradient(closest-side,{})",
                        colors.join(",")
                    )),
                    // Excel's degrees run clockwise from left to right,
                    // CSS's from bottom to top.
                    (true, _) => Some(format!(
                        "linear-gradient({:.0}deg,{})",
                        gradient
                            .attribute("degree")
                            .and_then(|d| d.parse::<f64>().ok())
                            .unwrap_or(0.0)
                            + 90.0,
                        colors.join(",")
                    )),
                };
                return (stops.first().map(|(_, rgb)| rgb.clone()), css);
            }
            let Some(pattern) = child(fill, "patternFill") else {
                return (None, None);
            };
            let fill = match pattern.attribute("patternType") {
                None | Some("none") => None,
                // A solid fill paints its foreground colour; a pattern is
                // drawn as the colour its foreground and background mix to.
                Some("solid") => rgb(child(pattern, "fgColor"))
                    .or_else(|| rgb(child(pattern, "bgColor")))
                    .or_else(|| Some("000000".to_owned())),
                Some(kind) => Some(mixed(
                    rgb(child(pattern, "fgColor"))
                        .as_deref()
                        .unwrap_or("000000"),
                    rgb(child(pattern, "bgColor"))
                        .as_deref()
                        .unwrap_or("FFFFFF"),
                    pattern_density(kind),
                )),
            };
            (fill, None)
        })
        .collect();
    let double_underlines: Vec<bool> = child(root, "fonts")
        .into_iter()
        .flat_map(|fonts| fonts.children().filter(Node::is_element))
        .map(|font| {
            child(font, "u")
                .is_some_and(|u| matches!(u.attribute("val"), Some("double" | "doubleAccounting")))
        })
        .collect();
    let diagonals: Vec<Option<(BorderSide, bool, bool)>> = child(root, "borders")
        .into_iter()
        .flat_map(|borders| borders.children().filter(Node::is_element))
        .map(|border| {
            let on = |name: &str| matches!(border.attribute(name), Some("1" | "true"));
            let (up, down) = (on("diagonalUp"), on("diagonalDown"));
            let side = child(border, "diagonal")?;
            let style = side.attribute("style").filter(|s| *s != "none")?;
            (up || down).then(|| {
                (
                    BorderSide {
                        style: style.to_owned(),
                        color: rgb(child(side, "color")),
                    },
                    up,
                    down,
                )
            })
        })
        .collect();
    let borders: Vec<[Option<BorderSide>; 4]> = child(root, "borders")
        .into_iter()
        .flat_map(|borders| borders.children().filter(Node::is_element))
        .map(|border| {
            let side = |names: &[&str]| {
                names.iter().find_map(|name| {
                    let side = child(border, name)?;
                    let style = side.attribute("style").filter(|s| *s != "none")?;
                    Some(BorderSide {
                        style: style.to_owned(),
                        color: rgb(child(side, "color")),
                    })
                })
            };
            [
                side(&["left", "start"]),
                side(&["right", "end"]),
                side(&["top"]),
                side(&["bottom"]),
            ]
        })
        .collect();
    child(root, "cellXfs")
        .into_iter()
        .flat_map(|xfs| xfs.children().filter(|n| n.has_tag_name((NS, "xf"))))
        .map(|xf| {
            let index = |name: &str| {
                xf.attribute(name)
                    .and_then(|v| v.parse::<usize>().ok())
                    .unwrap_or(0)
            };
            let alignment = child(xf, "alignment");
            let value = |name: &str| alignment.and_then(|a| a.attribute(name));
            let flag = |name: &str| matches!(value(name), Some("1" | "true"));
            FormatLook {
                fill: fills.get(index("fillId")).and_then(|f| f.0.clone()),
                gradient: fills.get(index("fillId")).and_then(|f| f.1.clone()),
                diagonal: diagonals.get(index("borderId")).cloned().flatten(),
                number_format: index("numFmtId") as u32,
                double_underline: double_underlines
                    .get(index("fontId"))
                    .copied()
                    .unwrap_or(false),
                borders: borders.get(index("borderId")).cloned().unwrap_or_default(),
                horizontal: value("horizontal")
                    .filter(|h| *h != "general")
                    .map(str::to_owned),
                vertical: value("vertical")
                    .filter(|v| *v != "bottom")
                    .map(str::to_owned),
                wrap: flag("wrapText"),
                shrink: flag("shrinkToFit"),
                indent: value("indent").and_then(|i| i.parse().ok()).unwrap_or(0),
                rotation: value("textRotation")
                    .and_then(|r| r.parse().ok())
                    .unwrap_or(0),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const STYLES: &str = r#"<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
<fills><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill>
<fill><patternFill patternType="solid"><fgColor rgb="FFFFFF00"/><bgColor indexed="64"/></patternFill></fill>
<fill><patternFill patternType="lightUp"><fgColor rgb="FFFF0000"/></patternFill></fill>
<fill><gradientFill degree="90"><stop position="0"><color rgb="FF00FFFF"/></stop><stop position="1"><color rgb="FF0000FF"/></stop></gradientFill></fill></fills>
<borders><border><left/><right/><top/><bottom/></border>
<border><left style="thin"><color rgb="FF00FF00"/></left><right/><top/><bottom style="double"><color auto="1"/></bottom></border>
<border diagonalUp="1"><left/><right/><top/><bottom/><diagonal style="thin"><color rgb="FF112233"/></diagonal></border></borders>
<cellXfs><xf fillId="0" borderId="0"/><xf fillId="2" borderId="1"><alignment horizontal="center" vertical="top" wrapText="1" indent="2"/></xf>
<xf numFmtId="38" fillId="3" borderId="2"/><xf fillId="4" borderId="0"/></cellXfs>
</styleSheet>"#;

    #[test]
    fn formats_give_their_fill_borders_and_alignment() {
        let doc = Document::parse(STYLES).unwrap();
        let looks = cell_looks(doc.root_element(), &Palette::new(None, Some(&doc)));
        assert_eq!(looks[0], FormatLook::default());
        assert_eq!(looks[1].fill.as_deref(), Some("FFFF00"));
        assert_eq!(
            looks[1].borders[0],
            Some(BorderSide {
                style: "thin".into(),
                color: Some("00FF00".into())
            })
        );
        assert_eq!(looks[1].borders[1], None);
        // An automatic colour is the one the viewer draws text in.
        assert_eq!(looks[1].borders[3].as_ref().unwrap().color, None);
        assert_eq!(looks[1].horizontal.as_deref(), Some("center"));
        assert_eq!(looks[1].vertical.as_deref(), Some("top"));
        assert!(looks[1].wrap);
        assert_eq!(looks[1].indent, 2);
        // A pattern is the colour its red lines and the white behind mix to.
        assert_eq!(looks[2].fill.as_deref(), Some("FFBFBF"));
        assert_eq!(looks[2].number_format, 38);
        let (side, up, down) = looks[2].diagonal.clone().unwrap();
        assert_eq!(
            (side.color.as_deref(), up, down),
            (Some("112233"), true, false)
        );
        assert!(looks[2].paints() && !looks[0].paints());
        // A gradient running down the cell, from its first colour.
        assert_eq!(looks[3].fill.as_deref(), Some("00FFFF"));
        assert_eq!(
            looks[3].gradient.as_deref(),
            Some("linear-gradient(180deg,#00FFFF 0%,#0000FF 100%)")
        );
        assert!(!japanese(doc.root_element()));
        let japanese_styles = Document::parse(
            r#"<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><fonts><font><name val="Calibri"/><charset val="128"/></font></fonts></styleSheet>"#,
        )
        .unwrap();
        assert!(japanese(japanese_styles.root_element()));
    }

    #[test]
    fn shapes_paint_what_their_properties_or_style_give_and_nothing_else() {
        let drawing = r#"<xdr:wsDr xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
<xdr:twoCellAnchor><xdr:sp><xdr:nvSpPr><xdr:cNvPr id="2" name="Plain"/></xdr:nvSpPr><xdr:spPr/><xdr:txBody><a:bodyPr/><a:p/></xdr:txBody></xdr:sp></xdr:twoCellAnchor>
<xdr:twoCellAnchor><xdr:sp><xdr:nvSpPr><xdr:cNvPr id="3" name="Own"/></xdr:nvSpPr><xdr:spPr><a:solidFill><a:srgbClr val="ff0000"/></a:solidFill><a:ln w="25400"><a:noFill/></a:ln></xdr:spPr>
<xdr:txBody><a:bodyPr anchor="ctr"/><a:p><a:pPr algn="ctr"/></a:p></xdr:txBody></xdr:sp></xdr:twoCellAnchor>
<xdr:twoCellAnchor><xdr:cxnSp><xdr:nvCxnSpPr><xdr:cNvPr id="4" name="Styled"/></xdr:nvCxnSpPr><xdr:spPr/><xdr:style><a:lnRef idx="1"><a:srgbClr val="0000FF"/></a:lnRef><a:fillRef idx="0"><a:srgbClr val="00FF00"/></a:fillRef></xdr:style></xdr:cxnSp></xdr:twoCellAnchor>
</xdr:wsDr>"#;
        let book = Workbook {
            raw: vec![],
            parts: BTreeMap::from([(
                "xl/drawings/drawing1.xml".to_owned(),
                drawing.as_bytes().to_vec(),
            )]),
            sheets: vec![],
            cells: vec![],
            formats: vec![],
            skipped_sheets: vec![],
            date1904: false,
            rich_values: vec![],
            other_images: BTreeMap::new(),
        };
        let looks = drawing_looks(&book).unwrap();
        let plain = &looks["xl/drawings/drawing1.xml#2"];
        assert_eq!(
            (&plain.fill, &plain.line),
            (&Paint::Nothing, &Paint::Nothing)
        );
        let own = &looks["xl/drawings/drawing1.xml#3"];
        assert_eq!(own.fill, Paint::Color("FF0000".into()));
        assert_eq!((&own.line, own.line_width), (&Paint::Nothing, Some(2.0)));
        assert_eq!(
            (own.horizontal.as_deref(), own.vertical.as_deref()),
            (Some("ctr"), Some("ctr"))
        );
        let styled = &looks["xl/drawings/drawing1.xml#4"];
        assert_eq!(
            (&styled.fill, &styled.line),
            (&Paint::Nothing, &Paint::Color("0000FF".into()))
        );
    }

    #[test]
    fn sheets_give_sizes_hidden_lines_and_formats_of_rows_and_columns() {
        let sheet = Document::parse(r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
<sheetViews><sheetView showGridLines="0"/></sheetViews><sheetFormatPr defaultRowHeight="15" customHeight="1"/>
<cols><col min="2" max="3" width="20" style="1"/><col min="4" max="4" width="5" hidden="1"/></cols>
<sheetData><row r="2" ht="30" customHeight="1" s="1" customFormat="1"><c r="A2" s="0"/></row><row r="3" hidden="1"/></sheetData></worksheet>"#).unwrap();
        // Calibri's digit, 9 pixels on the screen.
        let measures = fit::Styles {
            formats: vec![],
            digit: 7.2,
            row: 20.0,
        };
        let looks = std::sync::Arc::new(vec![
            FormatLook::default(),
            FormatLook {
                fill: Some("FF0000".into()),
                ..FormatLook::default()
            },
        ]);
        let look = sheet_look(&sheet, &measures, looks).unwrap();
        assert!(!look.grid_lines);
        // A column of the default width is 8 digits and the room around
        // them, in a whole number of 8 pixels of the screen.
        assert_eq!(look.width(1), 64.0);
        assert_eq!(look.width(2), 144.0);
        assert_eq!(look.width(4), 0.0);
        assert_eq!(look.height(1), 20.0);
        assert_eq!(look.height(2), 40.0);
        assert_eq!(look.height(3), 0.0);
        // The cell's own format comes first, then its row's, then its column's.
        assert_eq!(look.cell(1, 2).unwrap().fill, None);
        assert_eq!(look.cell(5, 2).unwrap().fill.as_deref(), Some("FF0000"));
        assert_eq!(look.cell(2, 9).unwrap().fill.as_deref(), Some("FF0000"));
        assert!(look.cell(1, 9).is_none());
    }

    #[test]
    fn a_sheet_hiding_the_rows_it_does_not_use_shows_only_those_it_writes() {
        let sheet = Document::parse(
            r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
<sheetFormatPr defaultRowHeight="18" zeroHeight="1"/>
<sheetData><row r="1"/><row r="2" ht="30" customHeight="1"/></sheetData></worksheet>"#,
        )
        .unwrap();
        let measures = fit::Styles {
            formats: vec![],
            digit: 7.0,
            row: 15.0,
        };
        let look = sheet_look(&sheet, &measures, std::sync::Arc::new(vec![])).unwrap();
        assert_eq!(look.height(1), 20.0);
        assert_eq!(look.height(2), 40.0);
        assert_eq!(look.height(3), 0.0);
    }
}
