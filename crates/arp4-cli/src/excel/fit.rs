//! Whether the text an edit writes shows where it is written, estimated from
//! the workbook as written: the widths and heights of the cells (merged ones
//! together), their alignment and font size, and the size and insets of a
//! shape. Character widths are estimated (a wide East Asian character one
//! em, others a little over half), so an edit is reported only when its text
//! needs more room than the text it replaces and more than there is.
use super::*;

/// The text an edit writes, and the text it replaces, at its place in the
/// written workbook.
pub struct FitTarget<'a> {
    /// The edit's position in its request.
    pub edit: usize,
    pub sheet: &'a str,
    pub place: Place<'a>,
    pub before: Option<&'a str>,
    pub after: &'a str,
}

pub enum Place<'a> {
    /// A cell by its address in the written workbook.
    Cell(&'a str),
    /// A shape by its extraction ID, `<drawing part>#<cNvPr id>`.
    Shape(&'a str),
}

/// Points of a pixel, at the 96 pixels per inch widths are measured in.
const POINTS_PER_PIXEL: f64 = 0.75;
/// Points of an EMU (English Metric Unit) of DrawingML.
const POINTS_PER_EMU: f64 = 1.0 / 12_700.0;
/// The room Excel leaves between a cell's edges and its text, in points.
const CELL_PADDING: f64 = 3.0;
/// The width of a character that is not wide, in ems: proportional Latin
/// text averages a little under half an em.
const NARROW: f64 = 0.55;

/// The warnings for the `targets` whose text shows cut off, or makes a row
/// grow, in the workbook `book`.
pub fn layout_warnings(book: &Workbook, targets: &[FitTarget<'_>]) -> Result<Vec<Value>> {
    let styles = Styles::read(book)?;
    let mut sheets: BTreeMap<&str, Sheet> = BTreeMap::new();
    let mut warnings = vec![];
    for target in targets {
        if !sheets.contains_key(target.sheet) {
            let part = book
                .sheets
                .iter()
                .find(|sheet| sheet["name"] == target.sheet)
                .map(|sheet| string(&sheet["part"]))
                .transpose()?
                .with_context(|| format!("no worksheet {}", target.sheet))?;
            sheets.insert(
                target.sheet,
                Sheet::read(std::str::from_utf8(&book.parts[part])?, &styles)?,
            );
        }
        let sheet = &sheets[target.sheet];
        let warning = match target.place {
            Place::Cell(address) => sheet.cell_warning(&styles, address, target)?,
            Place::Shape(id) => shape_warning(book, sheet, &styles, id, target)?,
        };
        if let Some(mut warning) = warning {
            warning["edit"] = json!(target.edit);
            warnings.push(warning);
        }
    }
    Ok(warnings)
}

/// The estimated width of `text` on one line, in ems.
fn ems(text: &str) -> f64 {
    text.chars()
        .map(|c| if wide(c) { 1.0 } else { NARROW })
        .sum()
}

/// Whether `c` is drawn a full em wide: East Asian wide and fullwidth
/// characters (CJK, kana, Hangul, fullwidth forms).
fn wide(c: char) -> bool {
    matches!(u32::from(c),
        0x1100..=0x115F
        | 0x2E80..=0x303E
        | 0x3041..=0x33FF
        | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xA000..=0xA4CF
        | 0xAC00..=0xD7A3
        | 0xF900..=0xFAFF
        | 0xFE30..=0xFE4F
        | 0xFF00..=0xFF60
        | 0xFFE0..=0xFFE6
        | 0x20000..=0x3FFFD)
}

/// The lines `text` takes when wrapped `width` points wide in a font of
/// `size` points.
fn lines(text: &str, width: f64, size: f64) -> u32 {
    text.split('\n')
        .map(|line| {
            let needed = ems(line) * size / width.max(size);
            (needed.ceil() as u32).max(1)
        })
        .sum()
}

/// The height of a line of text in a cell in a font, in ems, which is also
/// the default height of a row for its normal font (measured in Excel): Yu
/// and Meiryo fonts leave more room above and below their characters than
/// Calibri, and the MS fonts less.
fn line_height(font: &str) -> f64 {
    if ["游", "Yu ", "メイリオ", "Meiryo"]
        .iter()
        .any(|name| font.contains(name))
    {
        1.7
    } else if ["ＭＳ", "MS "].iter().any(|name| font.contains(name)) {
        1.2
    } else {
        1.31
    }
}

/// The cell formats of a workbook: font and alignment, and the width of a
/// digit of its normal font, which column widths count.
pub(super) struct Styles {
    pub(super) formats: Vec<Format>,
    pub(super) digit: f64,
    /// The default height of a row, in points.
    pub(super) row: f64,
}

#[derive(Clone, Default)]
pub(super) struct Format {
    size: f64,
    font: String,
    wrap: bool,
    shrink: bool,
    /// The horizontal alignment; none for general.
    horizontal: Option<String>,
}

impl Styles {
    pub(super) fn read(book: &Workbook) -> Result<Self> {
        let Some(bytes) = book.parts.get("xl/styles.xml") else {
            return Ok(Self {
                formats: vec![],
                digit: 7.2,
                row: 11.0 * line_height(""),
            });
        };
        let doc = xml(bytes)?;
        let root = doc.root_element();
        let japanese = |scheme: &str| theme_font(book, scheme);
        let fonts: Vec<(f64, String)> = child(root, "fonts")
            .into_iter()
            .flat_map(|fonts| fonts.children().filter(|n| n.has_tag_name((NS, "font"))))
            .map(|font| {
                let value = |name: &str| child(font, name).and_then(|n| n.attribute("val"));
                // A theme font is drawn in the theme's Japanese font in
                // Japanese Excel, which sets the heights of its lines.
                let name = match value("scheme") {
                    Some("minor") => japanese("minorFont"),
                    Some("major") => japanese("majorFont"),
                    _ => None,
                };
                (
                    value("sz").and_then(|s| s.parse().ok()).unwrap_or(11.0),
                    name.unwrap_or_else(|| value("name").unwrap_or_default().to_owned()),
                )
            })
            .collect();
        let font_of = |xf: Node<'_, '_>| {
            xf.attribute("fontId")
                .and_then(|id| id.parse::<usize>().ok())
                .and_then(|id| fonts.get(id))
                .cloned()
                .unwrap_or((11.0, String::new()))
        };
        let normal = child(root, "cellStyleXfs")
            .and_then(|xfs| xfs.children().find(|n| n.has_tag_name((NS, "xf"))))
            .map_or((11.0, String::new()), font_of);
        let formats = child(root, "cellXfs")
            .into_iter()
            .flat_map(|xfs| xfs.children().filter(|n| n.has_tag_name((NS, "xf"))))
            .map(|xf| {
                let (size, font) = font_of(xf);
                let alignment = child(xf, "alignment");
                let flag = |name: &str| {
                    alignment
                        .and_then(|a| a.attribute(name))
                        .is_some_and(|v| matches!(v, "1" | "true"))
                };
                Format {
                    size,
                    font,
                    wrap: flag("wrapText"),
                    shrink: flag("shrinkToFit"),
                    horizontal: alignment
                        .and_then(|a| a.attribute("horizontal"))
                        .filter(|h| *h != "general")
                        .map(str::to_owned),
                }
            })
            .collect();
        Ok(Self {
            formats,
            // A unit of column width is 7.2 pixels with an 11 point normal
            // font, Calibri and ＭＳ Ｐゴシック alike (measured in Excel); other
            // sizes scale with it.
            digit: normal.0 * 7.2 / 11.0,
            row: normal.0 * line_height(&normal.1),
        })
    }

    fn format(&self, style: usize) -> Format {
        self.formats.get(style).cloned().unwrap_or(Format {
            size: 11.0,
            ..Format::default()
        })
    }
}

/// What a worksheet gives the cells: sizes, merges, formats and values.
struct Sheet {
    column_width: f64,
    columns: Vec<(u32, u32, f64)>,
    row_height: f64,
    /// Each row's height and whether it is fixed (customHeight), by number.
    rows: BTreeMap<u32, (Option<f64>, bool)>,
    merges: Vec<((u32, u32), (u32, u32))>,
    /// Each cell's format and whether it holds a value, by (row, column).
    cells: BTreeMap<(u32, u32), (usize, bool)>,
}

impl Sheet {
    fn read(text: &str, styles: &Styles) -> Result<Self> {
        let doc = xml(text.as_bytes())?;
        let root = doc.root_element();
        let format = child(root, "sheetFormatPr");
        let number = |name: &str| {
            format
                .and_then(|f| f.attribute(name))
                .and_then(|v| v.parse::<f64>().ok())
        };
        let columns = child(root, "cols")
            .into_iter()
            .flat_map(|cols| cols.children().filter(|n| n.has_tag_name((NS, "col"))))
            .filter_map(|col| {
                let hidden = matches!(col.attribute("hidden"), Some("1" | "true"));
                Some((
                    col.attribute("min")?.parse().ok()?,
                    col.attribute("max")?.parse().ok()?,
                    if hidden {
                        0.0
                    } else {
                        col.attribute("width")?.parse().ok()?
                    },
                ))
            })
            .collect();
        let mut rows = BTreeMap::new();
        let mut cells = BTreeMap::new();
        for row in child(root, "sheetData")
            .into_iter()
            .flat_map(|data| data.children().filter(|n| n.has_tag_name((NS, "row"))))
        {
            let Some(number) = row.attribute("r").and_then(|r| r.parse::<u32>().ok()) else {
                continue;
            };
            let hidden = matches!(row.attribute("hidden"), Some("1" | "true"));
            rows.insert(
                number,
                (
                    if hidden {
                        Some(0.0)
                    } else {
                        row.attribute("ht").and_then(|h| h.parse().ok())
                    },
                    matches!(row.attribute("customHeight"), Some("1" | "true")),
                ),
            );
            for cell in row.children().filter(|n| n.has_tag_name((NS, "c"))) {
                let Some(address) = cell.attribute("r") else {
                    continue;
                };
                let (column, row) = coordinate(address)?;
                let style = cell
                    .attribute("s")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
                let value = child(cell, "v")
                    .is_some_and(|v| v.text().is_some_and(|t| !t.is_empty()))
                    || child(cell, "is").is_some();
                cells.insert((row, column), (style, value));
            }
        }
        let merges = child(root, "mergeCells")
            .into_iter()
            .flat_map(|merges| {
                merges
                    .children()
                    .filter(|n| n.has_tag_name((NS, "mergeCell")))
            })
            .filter_map(|merge| {
                let area = Area::parse(merge.attribute("ref")?).ok()?;
                Some((area.columns?, area.rows?))
            })
            .collect();
        Ok(Self {
            column_width: number("defaultColWidth")
                .or_else(|| number("baseColWidth").map(|base| base + 1.140625))
                .unwrap_or(9.140625),
            columns,
            // Excel takes the default height only when it is fixed, and
            // otherwise fits the rows to the normal font.
            row_height: if format
                .and_then(|f| f.attribute("customHeight"))
                .is_some_and(|v| matches!(v, "1" | "true"))
            {
                number("defaultRowHeight").unwrap_or(styles.row)
            } else {
                styles.row
            },
            rows,
            merges,
            cells,
        })
    }

    /// The width of a column, in points.
    fn width(&self, column: u32, styles: &Styles) -> f64 {
        let characters = self
            .columns
            .iter()
            .find(|(min, max, _)| (*min..=*max).contains(&column))
            .map_or(self.column_width, |(_, _, width)| *width);
        characters * styles.digit * POINTS_PER_PIXEL
    }

    /// The height of a row in points, and whether Excel keeps it as it is
    /// rather than fitting it to the text on opening.
    fn height(&self, row: u32) -> (f64, bool) {
        match self.rows.get(&row) {
            Some((height, fixed)) => (height.unwrap_or(self.row_height), *fixed),
            None => (self.row_height, false),
        }
    }

    fn cell_warning(
        &self,
        styles: &Styles,
        address: &str,
        target: &FitTarget<'_>,
    ) -> Result<Option<Value>> {
        let (column, row) = coordinate(address)?;
        let merge = self.merges.iter().find(|((c0, c1), (r0, r1))| {
            (*c0..=*c1).contains(&column) && (*r0..=*r1).contains(&row)
        });
        let ((c0, c1), (r0, r1)) = merge.copied().unwrap_or(((column, column), (row, row)));
        let format = styles.format(self.cells.get(&(row, column)).map_or(0, |(s, _)| *s));
        if format.shrink {
            return Ok(None);
        }
        let width = (c0..=c1).map(|c| self.width(c, styles)).sum::<f64>() - CELL_PADDING;
        let heights: Vec<_> = (r0..=r1).map(|r| self.height(r)).collect();
        let height: f64 = heights.iter().map(|(h, _)| h).sum();
        let size = format.size;
        let before = target.before.unwrap_or_default();
        let estimate = |kind: &str, extra: Value| {
            let mut warning = json!({"sheet":target.sheet,"cell":address,"issue":kind,
                "estimate":{"width_pt":round(width),"height_pt":round(height),"font_size":size,"wrap":format.wrap}});
            if let Some(merge) = merge {
                warning["merged"] = json!(format!(
                    "{}{}:{}{}",
                    column_name(merge.0.0)?,
                    merge.1.0,
                    column_name(merge.0.1)?,
                    merge.1.1
                ));
            }
            for (key, value) in extra.as_object().into_iter().flatten() {
                warning["estimate"][key] = value.clone();
            }
            Ok::<_, anyhow::Error>(Some(warning))
        };
        if !format.wrap {
            let breaks = |text: &str| text.matches('\n').count();
            if breaks(target.after) > breaks(before) {
                // Excel draws the lines of a cell only when it wraps its text.
                return estimate("line_breaks_hidden", json!({}));
            }
            let needed = |text: &str| ems(text) * size;
            let after = needed(target.after);
            if after <= width || after <= needed(before) {
                return Ok(None);
            }
            // Text that does not wrap runs on over empty cells to its right,
            // unless it is merged or aligned otherwise.
            let mut room = width;
            if merge.is_none() && matches!(format.horizontal.as_deref(), None | Some("left")) {
                let mut next = c1 + 1;
                while room < after
                    && next <= 16_384
                    && !self
                        .cells
                        .get(&(row, next))
                        .is_some_and(|(_, value)| *value)
                    && !self.merges.iter().any(|((m0, m1), (n0, n1))| {
                        (*m0..=*m1).contains(&next) && (*n0..=*n1).contains(&row)
                    })
                {
                    room += self.width(next, styles);
                    next += 1;
                }
            }
            if after <= room {
                return Ok(None);
            }
            return estimate(
                "clipped",
                json!({"needed_width_pt":round(after),"available_width_pt":round(room)}),
            );
        }
        let line = size * line_height(&format.font);
        let (after, before) = (lines(target.after, width, size), lines(before, width, size));
        let available = ((height / line).floor() as u32).max(1);
        if after <= available || after <= before {
            return Ok(None);
        }
        // Excel fits a row to its wrapped text on opening, but not a fixed
        // row or a merged cell.
        let fitted = merge.is_none() && !heights[0].1;
        estimate(
            if fitted { "row_grows" } else { "clipped" },
            json!({"needed_lines":after,"available_lines":available}),
        )
    }
}

fn round(points: f64) -> f64 {
    (points * 10.0).round() / 10.0
}

/// The warning for the text of the shape `id` on `sheet` when it needs more
/// lines than the shape holds, or is wider than a shape not wrapping it.
fn shape_warning(
    book: &Workbook,
    sheet: &Sheet,
    styles: &Styles,
    id: &str,
    target: &FitTarget<'_>,
) -> Result<Option<Value>> {
    let (part, number) = id.rsplit_once('#').context("invalid shape ID")?;
    let Some(bytes) = book.parts.get(part) else {
        return Ok(None);
    };
    let doc = xml(bytes)?;
    let Some(shape) = doc
        .descendants()
        .filter(|n| n.has_tag_name((XDR, "cNvPr")) && n.attribute("id") == Some(number))
        .find_map(|n| n.ancestors().find(|a| a.has_tag_name((XDR, "sp"))))
    else {
        return Ok(None);
    };
    let Some(body) = shape.children().find(|n| n.has_tag_name((XDR, "txBody"))) else {
        return Ok(None);
    };
    let properties = body
        .children()
        .find(|n| n.has_tag_name((DRAWING, "bodyPr")));
    if properties.is_some_and(|p| {
        p.children()
            .any(|n| n.has_tag_name((DRAWING, "normAutofit")))
    }) {
        return Ok(None);
    }
    let inset = |name: &str, default: f64| {
        properties
            .and_then(|p| p.attribute(name))
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(default)
            * POINTS_PER_EMU
    };
    let Some((width, height)) = shape_size(sheet, styles, shape) else {
        return Ok(None);
    };
    let width = width - inset("lIns", 91_440.0) - inset("rIns", 91_440.0);
    let height = height - inset("tIns", 45_720.0) - inset("bIns", 45_720.0);
    let wraps = properties.and_then(|p| p.attribute("wrap")) != Some("none");
    let size = body
        .descendants()
        .filter(|n| {
            n.has_tag_name((DRAWING, "rPr"))
                || n.has_tag_name((DRAWING, "endParaRPr"))
                || n.has_tag_name((DRAWING, "defRPr"))
        })
        .filter_map(|n| n.attribute("sz").and_then(|s| s.parse::<f64>().ok()))
        .fold(0.0, f64::max)
        / 100.0;
    let size = if size > 0.0 { size } else { 11.0 };
    let wrap_width = if wraps { width } else { f64::MAX };
    let before = target.before.unwrap_or_default();
    let (after, before) = (
        lines(target.after, wrap_width, size),
        lines(before, wrap_width, size),
    );
    // Shapes space the lines of the MS fonts wider than cells do (measured).
    let line = size * line_height(&shape_font(book, body)).max(1.28);
    let available = ((height / line).floor() as u32).max(1);
    let widest = target.after.split('\n').map(ems).fold(0.0, f64::max) * size;
    let overflow = after > available && after > before || !wraps && widest > width;
    Ok(overflow.then(|| {
        json!({"sheet":target.sheet,"shape":id,"issue":"overflows_shape",
            "estimate":{"width_pt":round(width),"height_pt":round(height),"font_size":size,
                "needed_lines":after,"available_lines":available}})
    }))
}

/// The font of a shape's text: the East Asian or Latin typeface its runs
/// name, else the theme's minor font for Japanese text.
fn shape_font(book: &Workbook, body: Node<'_, '_>) -> String {
    let named = body
        .descendants()
        .filter(|n| n.has_tag_name((DRAWING, "ea")) || n.has_tag_name((DRAWING, "latin")))
        .find_map(|n| n.attribute("typeface").filter(|f| !f.starts_with('+')));
    named
        .map(str::to_owned)
        .or_else(|| theme_font(book, "minorFont"))
        .unwrap_or_default()
}

/// The theme's Japanese font of its `scheme` (minorFont or majorFont).
fn theme_font(book: &Workbook, scheme: &str) -> Option<String> {
    let theme = xml(book.parts.get("xl/theme/theme1.xml")?).ok()?;
    let fonts = theme
        .descendants()
        .find(|n| n.has_tag_name((DRAWING, scheme)))?;
    let font = fonts
        .children()
        .find(|n| n.has_tag_name((DRAWING, "font")) && n.attribute("script") == Some("Jpan"))?;
    Some(font.attribute("typeface")?.to_owned()).filter(|name| !name.is_empty())
}

/// The size of a shape in points as Excel draws it: the cells of its anchor
/// give the size of the object anchored, and each group scales the extents
/// of what it holds from its child coordinates to its own size.
fn shape_size(sheet: &Sheet, styles: &Styles, shape: Node<'_, '_>) -> Option<(f64, f64)> {
    let anchor = shape.ancestors().find(|n| {
        n.has_tag_name((XDR, "twoCellAnchor"))
            || n.has_tag_name((XDR, "oneCellAnchor"))
            || n.has_tag_name((XDR, "absoluteAnchor"))
    })?;
    let number = |node: Node<'_, '_>, name: &str| -> Option<f64> {
        node.children()
            .find(|n| n.has_tag_name((XDR, name)))?
            .text()?
            .parse()
            .ok()
    };
    // The place of an anchor's corner on the sheet, in points.
    let corner = |name: &str| -> Option<(f64, f64)> {
        let marker = anchor.children().find(|n| n.has_tag_name((XDR, name)))?;
        let (column, row) = (number(marker, "col")? as u32, number(marker, "row")? as u32);
        Some((
            (1..=column).map(|c| sheet.width(c, styles)).sum::<f64>()
                + number(marker, "colOff")? * POINTS_PER_EMU,
            (1..=row).map(|r| sheet.height(r).0).sum::<f64>()
                + number(marker, "rowOff")? * POINTS_PER_EMU,
        ))
    };
    let extent = |node: Node<'_, '_>, name: &str| -> Option<(f64, f64)> {
        let extent = node
            .children()
            .find(|n| n.has_tag_name((XDR, "spPr")) || n.has_tag_name((XDR, "grpSpPr")))?
            .children()
            .find(|n| n.has_tag_name((DRAWING, "xfrm")))?
            .children()
            .find(|n| n.has_tag_name((DRAWING, name)))?;
        Some((
            extent.attribute("cx")?.parse::<f64>().ok()? * POINTS_PER_EMU,
            extent.attribute("cy")?.parse::<f64>().ok()? * POINTS_PER_EMU,
        ))
    };
    let mut size = if anchor.has_tag_name((XDR, "twoCellAnchor")) {
        let (from, to) = (corner("from")?, corner("to")?);
        (to.0 - from.0, to.1 - from.1)
    } else {
        let ext = anchor.children().find(|n| n.has_tag_name((XDR, "ext")))?;
        (
            ext.attribute("cx")?.parse::<f64>().ok()? * POINTS_PER_EMU,
            ext.attribute("cy")?.parse::<f64>().ok()? * POINTS_PER_EMU,
        )
    };
    // The groups from the one anchored down to the shape, each scaling the
    // next object's extent by its size over its child extent.
    let mut path: Vec<_> = shape
        .ancestors()
        .take_while(|n| *n != anchor)
        .filter(|n| n.has_tag_name((XDR, "grpSp")) || *n == shape)
        .collect();
    path.reverse();
    for pair in path.windows(2) {
        let children = extent(pair[0], "chExt")?;
        let held = extent(pair[1], "ext")?;
        if children.0 > 0.0 && children.1 > 0.0 {
            size = (held.0 * size.0 / children.0, held.1 * size.1 / children.1);
        }
    }
    Some(size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_characters_take_an_em_and_others_a_little_over_half() {
        assert_eq!(ems("受注"), 2.0);
        assert_eq!(ems("ab"), 2.0 * NARROW);
        assert_eq!(ems("ｱ"), NARROW);
        assert_eq!(ems("𠮷"), 1.0);
        // Ten wide characters of 10 points take two lines 60 points wide.
        assert_eq!(lines("あいうえおかきくけこ", 60.0, 10.0), 2);
        assert_eq!(lines("あ\nい", 60.0, 10.0), 2);
        assert_eq!(lines("", 60.0, 10.0), 1);
    }

    /// A shape in a group is as large as its extent scaled by the group's
    /// size, from the cells of its anchor, over the group's child extent:
    /// rows of 15 points and columns of 54, not the extents the file saves.
    #[test]
    fn a_grouped_shape_takes_its_size_from_the_groups_anchor() {
        let drawing = format!(
            r#"<xdr:wsDr xmlns:xdr="{XDR}" xmlns:a="{DRAWING}"><xdr:twoCellAnchor><xdr:from><xdr:col>0</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>0</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from><xdr:to><xdr:col>2</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>4</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:to><xdr:grpSp><xdr:nvGrpSpPr><xdr:cNvPr id="2" name="g"/><xdr:cNvGrpSpPr/></xdr:nvGrpSpPr><xdr:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="1" cy="1"/><a:chOff x="0" y="0"/><a:chExt cx="2743200" cy="1524000"/></a:xfrm></xdr:grpSpPr><xdr:sp><xdr:nvSpPr><xdr:cNvPr id="3" name="s"/><xdr:cNvSpPr/></xdr:nvSpPr><xdr:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="1371600" cy="762000"/></a:xfrm></xdr:spPr><xdr:txBody><a:bodyPr lIns="0" rIns="0" tIns="0" bIns="0"/><a:p><a:r><a:rPr sz="1000"/><a:t>短い</a:t></a:r></a:p></xdr:txBody></xdr:sp></xdr:grpSp><xdr:clientData/></xdr:twoCellAnchor></xdr:wsDr>"#
        );
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        for (name, content) in [
            (
                "xl/workbook.xml",
                format!(
                    r#"<workbook xmlns="{NS}" xmlns:r="{REL}"><sheets><sheet name="S" sheetId="1" r:id="r1"/></sheets></workbook>"#
                ),
            ),
            (
                "xl/_rels/workbook.xml.rels",
                format!(
                    r#"<Relationships xmlns="{PKG_REL}"><Relationship Id="r1" Type="{REL}/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#
                ),
            ),
            (
                "xl/worksheets/sheet1.xml",
                format!(
                    r#"<worksheet xmlns="{NS}" xmlns:r="{REL}"><sheetFormatPr defaultRowHeight="15" customHeight="1"/><cols><col min="1" max="2" width="10" customWidth="1"/></cols><sheetData/><drawing r:id="d"/></worksheet>"#
                ),
            ),
            (
                "xl/worksheets/_rels/sheet1.xml.rels",
                format!(
                    r#"<Relationships xmlns="{PKG_REL}"><Relationship Id="d" Type="{REL}/drawing" Target="../drawings/drawing1.xml"/></Relationships>"#
                ),
            ),
            ("xl/drawings/drawing1.xml", drawing),
            (
                "[Content_Types].xml",
                r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="xml" ContentType="application/xml"/></Types>"#.to_owned(),
            ),
        ] {
            zip.start_file(name, SimpleFileOptions::default()).unwrap();
            zip.write_all(content.as_bytes()).unwrap();
        }
        let book = Workbook::from_bytes(zip.finish().unwrap().into_inner()).unwrap();
        let target = |after| FitTarget {
            edit: 0,
            sheet: "S",
            place: Place::Shape("xl/drawings/drawing1.xml#3"),
            before: Some("短い"),
            after,
        };
        // The group is 108 by 60 points, so the shape half its child
        // extent is 54 by 30: two lines of 10 point text.
        let fits = layout_warnings(&book, &[target("一二三四五")]).unwrap();
        assert!(fits.is_empty(), "{fits:?}");
        let warnings =
            layout_warnings(&book, &[target("一二三四五六七八九十一二三四五六")]).unwrap();
        assert_eq!(warnings[0]["issue"], "overflows_shape");
        assert_eq!(warnings[0]["estimate"]["width_pt"], 54.0);
        assert_eq!(warnings[0]["estimate"]["height_pt"], 30.0);
        assert_eq!(warnings[0]["estimate"]["available_lines"], 2);
    }
}
