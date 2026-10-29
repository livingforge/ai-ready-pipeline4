//! How the cells of a worksheet look in the original, beyond what the
//! extraction keeps of them: the sizes of the columns and rows, and the fill,
//! borders and alignment of each cell format. A view of the document
//! (`documents view`) lays the content out with them; nothing is written.
use super::fonts::Palette;
use super::*;
use crate::fonts::Color;
use std::collections::HashMap;

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
}

impl SheetLook {
    /// The width of the column numbered `column` (from 1), in pixels.
    pub fn width(&self, column: u32) -> f64 {
        let characters = self
            .columns
            .iter()
            .find(|(min, max, _)| (*min..=*max).contains(&column))
            .map_or(self.column_width, |(_, _, width)| *width);
        (characters * self.digit).round()
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
        self.cells.iter().filter_map(|(place, format)| {
            let look = self.formats.get(*format)?;
            (look.fill.is_some() || look.borders.iter().any(Option::is_some)).then_some(*place)
        })
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
    let mut looks = HashMap::new();
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
                },
            );
        }
    }
    Ok(looks)
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
    let measures = fit::Styles::read(book)?;
    let mut looks = BTreeMap::new();
    for sheet in &book.sheets {
        let part = string(&sheet["part"])?;
        let Some(bytes) = book.parts.get(part) else {
            continue;
        };
        looks.insert(
            string(&sheet["name"])?.to_owned(),
            sheet_look(&xml(bytes)?, &measures, formats.clone())?,
        );
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
        &fit::Styles::read(book)?,
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
    let mut rows = HashMap::new();
    let mut row_formats = HashMap::new();
    let mut cells = HashMap::new();
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
        column_width: number("defaultColWidth")
            .or_else(|| number("baseColWidth").map(|base| base + 1.140625))
            .unwrap_or(9.140625),
        // Excel takes the default height only when it is fixed, and otherwise
        // fits the rows to the normal font.
        row_height: if format.is_some_and(|f| flag(f, "customHeight")) {
            number("defaultRowHeight").unwrap_or(measures.row)
        } else {
            measures.row
        },
        columns,
        rows,
        cells,
        row_formats,
        column_formats,
        formats,
        grid_lines,
    })
}

/// The look of each cell format of the styles part `root`.
fn cell_looks(root: Node<'_, '_>, palette: &Palette) -> Vec<FormatLook> {
    let rgb = |node: Option<Node<'_, '_>>| match node.and_then(|n| palette.color(n)) {
        Some(Color::Rgb { rgb, .. }) => Some(rgb),
        _ => None,
    };
    let fills: Vec<Option<String>> = child(root, "fills")
        .into_iter()
        .flat_map(|fills| fills.children().filter(Node::is_element))
        .map(|fill| {
            if let Some(gradient) = child(fill, "gradientFill") {
                return gradient
                    .children()
                    .find(|n| n.has_tag_name((NS, "stop")))
                    .and_then(|stop| rgb(child(stop, "color")));
            }
            let pattern = child(fill, "patternFill")?;
            match pattern.attribute("patternType") {
                None | Some("none") => None,
                // A solid fill paints its foreground colour; a pattern shows
                // mostly its foreground over its background.
                _ => rgb(child(pattern, "fgColor"))
                    .or_else(|| rgb(child(pattern, "bgColor")))
                    .or_else(|| Some("000000".to_owned())),
            }
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
                fill: fills.get(index("fillId")).cloned().flatten(),
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
<fill><patternFill patternType="solid"><fgColor rgb="FFFFFF00"/><bgColor indexed="64"/></patternFill></fill></fills>
<borders><border><left/><right/><top/><bottom/></border>
<border><left style="thin"><color rgb="FF00FF00"/></left><right/><top/><bottom style="double"><color auto="1"/></bottom></border></borders>
<cellXfs><xf fillId="0" borderId="0"/><xf fillId="2" borderId="1"><alignment horizontal="center" vertical="top" wrapText="1" indent="2"/></xf></cellXfs>
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
        let measures = fit::Styles {
            formats: vec![],
            digit: 7.0,
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
        assert_eq!(look.width(1), 64.0);
        assert_eq!(look.width(2), 140.0);
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
}
