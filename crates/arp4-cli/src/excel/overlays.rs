//! The formats a worksheet lays over those of its cells: its conditional
//! formats, with the differential formats (`dxf`) they give, and the styles
//! of its tables. A view of the document draws them; nothing is written.
use super::fonts::Palette;
use super::look::BorderSide;
use super::*;
use crate::fonts::{Color, tinted};

/// A range of cells: first column and row, last column and row, from 1.
pub type Span = (u32, u32, u32, u32);

/// What a differential format changes of the cells it is laid over.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DifferentialLook {
    /// The fill, `RRGGBB`.
    pub fill: Option<String>,
    /// The colour of the text, `RRGGBB`.
    pub color: Option<String>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub strike: Option<bool>,
    pub underline: Option<bool>,
    /// The left, right, top and bottom sides of the border.
    pub borders: [Option<BorderSide>; 4],
    /// The borders between the rows and between the columns of a range.
    pub inner: [Option<BorderSide>; 2],
}

/// A value a conditional format measures the cells of its range against.
#[derive(Clone, Debug, PartialEq)]
pub enum Threshold {
    Min,
    Max,
    Number(f64),
    Percent(f64),
    Percentile(f64),
    /// A formula, which is not worked out.
    Formula(String),
}

/// What a rule looks at to give its format.
#[derive(Clone, Debug, PartialEq)]
pub enum Test {
    /// The cell's value against one or two operands, by `operator`.
    CellIs {
        operator: String,
        operands: Vec<String>,
    },
    /// A formula written for the first cell of the range.
    Expression(String),
    /// The cell's text against `text`: `containsText`, `notContainsText`,
    /// `beginsWith` or `endsWith`.
    Text {
        kind: String,
        text: String,
    },
    Blanks(bool),
    Errors(bool),
    /// The `rank` largest (or `bottom` smallest) values, or percent of them.
    Top {
        rank: f64,
        percent: bool,
        bottom: bool,
    },
    /// Above (or below) the average of the range, or equal to it too.
    Average {
        above: bool,
        equal: bool,
    },
    Duplicates(bool),
    /// A rule not worked out, which gives no format.
    Other,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Rule {
    Format {
        test: Test,
        format: Box<DifferentialLook>,
        stop: bool,
    },
    /// The fill between the colours of two or three values.
    ColorScale(Vec<(Threshold, String)>),
    DataBar {
        min: Threshold,
        max: Threshold,
        color: String,
        show_value: bool,
    },
    IconSet {
        set: String,
        thresholds: Vec<Threshold>,
        reverse: bool,
        show_value: bool,
    },
}

/// A conditional format of a sheet: its ranges, its priority (lower first)
/// and its rule.
#[derive(Clone, Debug, PartialEq)]
pub struct ConditionalLook {
    pub ranges: Vec<Span>,
    pub priority: i64,
    pub rule: Rule,
}

/// A table of a sheet with what its style gives its parts.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TableLook {
    pub range: Span,
    pub header_rows: u32,
    pub totals_rows: u32,
    pub row_stripes: bool,
    pub column_stripes: bool,
    pub first_column: bool,
    pub last_column: bool,
    pub whole: DifferentialLook,
    pub header: DifferentialLook,
    pub total: DifferentialLook,
    /// The first and second stripes of rows, and of columns.
    pub row_stripe: [DifferentialLook; 2],
    pub column_stripe: [DifferentialLook; 2],
    pub first: DifferentialLook,
    pub last: DifferentialLook,
}

pub(super) fn span(text: &str) -> Option<Span> {
    let (first, last) = text.split_once(':').unwrap_or((text, text));
    let (c0, r0) = coordinate(&first.replace('$', "")).ok()?;
    let (c1, r1) = coordinate(&last.replace('$', "")).ok()?;
    Some((c0.min(c1), r0.min(r1), c0.max(c1), r0.max(r1)))
}

/// The differential formats of the styles part `root`, in order.
pub(super) fn differential_looks(root: Node<'_, '_>, palette: &Palette) -> Vec<DifferentialLook> {
    let rgb = |node: Option<Node<'_, '_>>| match node.and_then(|n| palette.color(n)) {
        Some(Color::Rgb { rgb, .. }) => Some(rgb),
        _ => None,
    };
    child(root, "dxfs")
        .into_iter()
        .flat_map(|list| list.children().filter(|n| n.has_tag_name((NS, "dxf"))))
        .map(|dxf| {
            let font = child(dxf, "font");
            let mark = |name: &str| {
                let mark = child(font?, name)?;
                Some(!matches!(
                    mark.attribute("val"),
                    Some("0" | "false" | "none")
                ))
            };
            // A differential fill is solid unless it names a pattern, and
            // paints its background colour.
            let fill = child(dxf, "fill").and_then(|fill| {
                let pattern = child(fill, "patternFill")?;
                match pattern.attribute("patternType") {
                    Some("none") => None,
                    None | Some("solid") => {
                        rgb(child(pattern, "bgColor")).or_else(|| rgb(child(pattern, "fgColor")))
                    }
                    _ => rgb(child(pattern, "fgColor")).or_else(|| rgb(child(pattern, "bgColor"))),
                }
            });
            let border = child(dxf, "border");
            let side = |names: &[&str]| {
                names.iter().find_map(|name| {
                    let side = child(border?, name)?;
                    let style = side.attribute("style").filter(|s| *s != "none")?;
                    Some(BorderSide {
                        style: style.to_owned(),
                        color: rgb(child(side, "color")),
                    })
                })
            };
            DifferentialLook {
                fill,
                color: font.and_then(|f| rgb(child(f, "color"))),
                bold: mark("b"),
                italic: mark("i"),
                strike: mark("strike"),
                underline: mark("u"),
                borders: [
                    side(&["left", "start"]),
                    side(&["right", "end"]),
                    side(&["top"]),
                    side(&["bottom"]),
                ],
                inner: [side(&["horizontal"]), side(&["vertical"])],
            }
        })
        .collect()
}

/// The conditional formats of the worksheet `root`.
pub(super) fn conditional_looks(
    root: Node<'_, '_>,
    formats: &[DifferentialLook],
    palette: &Palette,
) -> Vec<ConditionalLook> {
    let rgb = |node: Node<'_, '_>| match palette.color(node) {
        Some(Color::Rgb { rgb, .. }) => Some(rgb),
        _ => None,
    };
    let threshold = |node: Node<'_, '_>| {
        let value = node.attribute("val");
        let number = value.and_then(|v| v.parse::<f64>().ok());
        match (node.attribute("type").unwrap_or("num"), number) {
            ("min", _) => Threshold::Min,
            ("max", _) => Threshold::Max,
            ("percent", Some(number)) => Threshold::Percent(number),
            ("percentile", Some(number)) => Threshold::Percentile(number),
            ("num", Some(number)) => Threshold::Number(number),
            _ => Threshold::Formula(value.unwrap_or_default().to_owned()),
        }
    };
    let off = |node: Node<'_, '_>, name: &str| matches!(node.attribute(name), Some("0" | "false"));
    let on = |node: Node<'_, '_>, name: &str| matches!(node.attribute(name), Some("1" | "true"));
    let mut looks = vec![];
    for block in root
        .children()
        .filter(|n| n.has_tag_name((NS, "conditionalFormatting")))
    {
        let ranges: Vec<Span> = block
            .attribute("sqref")
            .unwrap_or_default()
            .split_whitespace()
            .filter_map(span)
            .collect();
        if ranges.is_empty() {
            continue;
        }
        for rule in block.children().filter(|n| n.has_tag_name((NS, "cfRule"))) {
            let kind = rule.attribute("type").unwrap_or_default();
            let thresholds = |node: Node<'_, '_>| -> Vec<Threshold> {
                node.children()
                    .filter(|n| n.has_tag_name((NS, "cfvo")))
                    .map(threshold)
                    .collect()
            };
            let colors = |node: Node<'_, '_>| -> Vec<String> {
                node.children()
                    .filter(|n| n.has_tag_name((NS, "color")))
                    .filter_map(rgb)
                    .collect()
            };
            let formulas: Vec<String> = rule
                .children()
                .filter(|n| n.has_tag_name((NS, "formula")))
                .map(|n| n.text().unwrap_or_default().to_owned())
                .collect();
            let look = match kind {
                "colorScale" => child(rule, "colorScale").and_then(|scale| {
                    let (values, colors) = (thresholds(scale), colors(scale));
                    (values.len() >= 2 && values.len() == colors.len())
                        .then(|| Rule::ColorScale(values.into_iter().zip(colors).collect()))
                }),
                "dataBar" => child(rule, "dataBar").and_then(|bar| {
                    let mut values = thresholds(bar).into_iter();
                    Some(Rule::DataBar {
                        min: values.next()?,
                        max: values.next()?,
                        color: colors(bar).into_iter().next()?,
                        show_value: !off(bar, "showValue"),
                    })
                }),
                "iconSet" => child(rule, "iconSet").map(|icons| Rule::IconSet {
                    set: icons
                        .attribute("iconSet")
                        .unwrap_or("3TrafficLights1")
                        .to_owned(),
                    thresholds: thresholds(icons),
                    reverse: on(icons, "reverse"),
                    show_value: !off(icons, "showValue"),
                }),
                _ => {
                    let text = || rule.attribute("text").unwrap_or_default().to_owned();
                    let test = match kind {
                        "cellIs" => Test::CellIs {
                            operator: rule.attribute("operator").unwrap_or_default().to_owned(),
                            operands: formulas.clone(),
                        },
                        "expression" => formulas
                            .first()
                            .map_or(Test::Other, |f| Test::Expression(f.clone())),
                        "containsText" | "notContainsText" | "beginsWith" | "endsWith" => {
                            Test::Text {
                                kind: kind.to_owned(),
                                text: text(),
                            }
                        }
                        "containsBlanks" => Test::Blanks(true),
                        "notContainsBlanks" => Test::Blanks(false),
                        "containsErrors" => Test::Errors(true),
                        "notContainsErrors" => Test::Errors(false),
                        "top10" => Test::Top {
                            rank: rule
                                .attribute("rank")
                                .and_then(|r| r.parse().ok())
                                .unwrap_or(10.0),
                            percent: on(rule, "percent"),
                            bottom: on(rule, "bottom"),
                        },
                        "aboveAverage" => Test::Average {
                            above: !off(rule, "aboveAverage"),
                            equal: on(rule, "equalAverage"),
                        },
                        "duplicateValues" => Test::Duplicates(true),
                        "uniqueValues" => Test::Duplicates(false),
                        _ => Test::Other,
                    };
                    Some(Rule::Format {
                        test,
                        format: Box::new(
                            rule.attribute("dxfId")
                                .and_then(|id| formats.get(id.parse::<usize>().ok()?))
                                .cloned()
                                .unwrap_or_default(),
                        ),
                        stop: on(rule, "stopIfTrue"),
                    })
                }
            };
            if let Some(rule_look) = look {
                looks.push(ConditionalLook {
                    ranges: ranges.clone(),
                    priority: rule
                        .attribute("priority")
                        .and_then(|p| p.parse().ok())
                        .unwrap_or(i64::MAX),
                    rule: rule_look,
                });
            }
        }
    }
    looks.sort_by_key(|look| look.priority);
    looks
}

/// The tables of the worksheet at `part`, each with what its style gives.
pub(super) fn table_looks(
    parts: &BTreeMap<String, Vec<u8>>,
    part: &str,
    styles: Option<Node<'_, '_>>,
    formats: &[DifferentialLook],
    palette: &Palette,
) -> Vec<TableLook> {
    let (directory, file) = part.rsplit_once('/').unwrap_or(("", part));
    let Some(Ok(rels)) = parts
        .get(&format!("{directory}/_rels/{file}.rels"))
        .map(|bytes| xml(bytes))
    else {
        return vec![];
    };
    let mut looks = vec![];
    for relationship in rels
        .root_element()
        .children()
        .filter(|n| n.attribute("Type").is_some_and(|t| t.ends_with("/table")))
    {
        let Some(table) = relationship
            .attribute("Target")
            .and_then(|target| relationship_target(part, target).ok())
            .and_then(|target| parts.get(&target))
        else {
            continue;
        };
        let Ok(doc) = xml(table) else {
            continue;
        };
        let root = doc.root_element();
        let Some(range) = root.attribute("ref").and_then(span) else {
            continue;
        };
        let count = |name: &str, default: u32| {
            root.attribute(name)
                .and_then(|v| v.parse().ok())
                .unwrap_or(default)
        };
        let info = child(root, "tableStyleInfo");
        let on = |name: &str| {
            info.is_some_and(|info| matches!(info.attribute(name), Some("1" | "true")))
        };
        let mut look = TableLook {
            range,
            header_rows: count("headerRowCount", 1),
            totals_rows: count("totalsRowCount", 0),
            row_stripes: on("showRowStripes"),
            column_stripes: on("showColumnStripes"),
            first_column: on("showFirstColumn"),
            last_column: on("showLastColumn"),
            ..TableLook::default()
        };
        if let Some(name) = info.and_then(|info| info.attribute("name"))
            && !custom_style(&mut look, name, styles, formats)
        {
            preset_style(&mut look, name, palette);
        }
        looks.push(look);
    }
    looks
}

/// Gives `look` the parts of the table style `name` the workbook defines;
/// false when it defines none of that name.
fn custom_style(
    look: &mut TableLook,
    name: &str,
    styles: Option<Node<'_, '_>>,
    formats: &[DifferentialLook],
) -> bool {
    let Some(style) = styles
        .and_then(|root| child(root, "tableStyles"))
        .and_then(|list| {
            list.children()
                .find(|n| n.has_tag_name((NS, "tableStyle")) && n.attribute("name") == Some(name))
        })
    else {
        return false;
    };
    for element in style
        .children()
        .filter(|n| n.has_tag_name((NS, "tableStyleElement")))
    {
        let Some(format) = element
            .attribute("dxfId")
            .and_then(|id| formats.get(id.parse::<usize>().ok()?))
        else {
            continue;
        };
        let part = match element.attribute("type").unwrap_or_default() {
            "wholeTable" => &mut look.whole,
            "headerRow" => &mut look.header,
            "totalRow" => &mut look.total,
            "firstRowStripe" => &mut look.row_stripe[0],
            "secondRowStripe" => &mut look.row_stripe[1],
            "firstColumnStripe" => &mut look.column_stripe[0],
            "secondColumnStripe" => &mut look.column_stripe[1],
            "firstColumn" => &mut look.first,
            "lastColumn" => &mut look.last,
            _ => continue,
        };
        *part = format.clone();
    }
    true
}

/// Gives `look` the parts of Excel's own table style `name`
/// (`TableStyleMedium2`), in the colours of the workbook's theme: the fills,
/// text and borders that tell its header, stripes and edges, nearly as Excel
/// draws them.
fn preset_style(look: &mut TableLook, name: &str, palette: &Palette) {
    let Some(rest) = name.strip_prefix("TableStyle") else {
        return;
    };
    let (family, number) = match rest.find(|c: char| c.is_ascii_digit()) {
        Some(at) => (&rest[..at], rest[at..].parse::<usize>().unwrap_or(0)),
        None => return,
    };
    if number == 0 {
        return;
    }
    // The styles of a family come in sevens: black, then the six accents.
    let (group, slot) = ((number - 1) / 7, (number - 1) % 7);
    let base = match slot {
        0 => "000000".to_owned(),
        accent => palette
            .theme
            .as_ref()
            .and_then(|theme| theme.color(&format!("accent{accent}")))
            .unwrap_or("4472C4")
            .to_owned(),
    };
    // Black's lighter shades are the greys Excel draws.
    let light = |tint: f64| match (slot, tint) {
        (0, t) if t >= 0.75 => "D9D9D9".to_owned(),
        (0, t) if t >= 0.55 => "BFBFBF".to_owned(),
        (0, _) => "808080".to_owned(),
        _ => tinted(&base, tint),
    };
    let thin = |color: &str| {
        Some(BorderSide {
            style: "thin".into(),
            color: Some(color.to_owned()),
        })
    };
    let medium = |color: &str| {
        Some(BorderSide {
            style: "medium".into(),
            color: Some(color.to_owned()),
        })
    };
    let fill = |color: String| DifferentialLook {
        fill: Some(color),
        ..DifferentialLook::default()
    };
    let white = Some("FFFFFF".to_owned());
    let bold = Some(true);
    let emphasis = DifferentialLook {
        bold,
        ..DifferentialLook::default()
    };
    look.first = emphasis.clone();
    look.last = emphasis.clone();
    look.total = emphasis.clone();
    match (family, group) {
        ("Light", 0) => {
            look.whole.borders[2] = thin(&base);
            look.whole.borders[3] = thin(&base);
            look.whole.color = (slot > 0).then(|| tinted(&base, -0.25));
            look.header = emphasis;
            look.header.borders[3] = thin(&base);
            look.total.borders[2] = thin(&base);
            look.row_stripe[0] = fill(light(0.8));
            look.column_stripe[0] = fill(light(0.8));
        }
        ("Light", 1) => {
            look.whole.borders = [thin(&base), thin(&base), thin(&base), thin(&base)];
            look.whole.inner[0] = thin(&base);
            look.header = DifferentialLook {
                fill: Some(base.clone()),
                color: white,
                bold,
                ..DifferentialLook::default()
            };
            look.total.borders[2] = Some(BorderSide {
                style: "double".into(),
                color: Some(base.clone()),
            });
        }
        ("Light", _) => {
            look.whole.borders = [thin(&base), thin(&base), thin(&base), thin(&base)];
            look.whole.inner = [thin(&base), thin(&base)];
            look.header = emphasis;
            look.header.borders[3] = medium(&base);
            look.total.borders[2] = Some(BorderSide {
                style: "double".into(),
                color: Some(base.clone()),
            });
            look.row_stripe[0] = fill(light(0.8));
            look.column_stripe[0] = fill(light(0.8));
        }
        ("Medium", 0) => {
            let edge = light(0.4);
            look.whole.borders = [thin(&edge), thin(&edge), thin(&edge), thin(&edge)];
            look.whole.inner[0] = thin(&edge);
            look.header = DifferentialLook {
                fill: Some(base.clone()),
                color: white,
                bold,
                ..DifferentialLook::default()
            };
            look.total.borders[2] = Some(BorderSide {
                style: "double".into(),
                color: Some(base.clone()),
            });
            look.row_stripe[0] = fill(light(0.8));
            look.column_stripe[0] = fill(light(0.8));
        }
        ("Medium", 1) => {
            look.whole.fill = Some(light(0.8));
            look.whole.inner = [thin("FFFFFF"), thin("FFFFFF")];
            look.header = DifferentialLook {
                fill: Some(base.clone()),
                color: white.clone(),
                bold,
                ..DifferentialLook::default()
            };
            look.header.borders[3] = Some(BorderSide {
                style: "thick".into(),
                color: Some("FFFFFF".into()),
            });
            look.total = look.header.clone();
            look.total.borders = [None, None, medium("FFFFFF"), None];
            look.first = DifferentialLook {
                fill: Some(base.clone()),
                color: white.clone(),
                bold,
                ..DifferentialLook::default()
            };
            look.last = look.first.clone();
            look.row_stripe[0] = fill(light(0.6));
            look.column_stripe[0] = fill(light(0.6));
        }
        ("Medium", 2) => {
            look.whole.borders[2] = medium("000000");
            look.whole.borders[3] = medium("000000");
            look.header = DifferentialLook {
                fill: Some(base.clone()),
                color: white.clone(),
                bold,
                ..DifferentialLook::default()
            };
            look.header.borders[3] = medium("000000");
            look.total.borders[2] = Some(BorderSide {
                style: "double".into(),
                color: Some("000000".into()),
            });
            look.first = look.header.clone();
            look.first.borders = Default::default();
            look.last = look.first.clone();
            look.row_stripe[0] = fill("D9D9D9".into());
            look.column_stripe[0] = fill("D9D9D9".into());
        }
        ("Medium", _) => {
            let edge = light(0.4);
            look.whole.fill = Some(light(0.8));
            look.whole.borders = [thin(&edge), thin(&edge), thin(&edge), thin(&edge)];
            look.whole.inner = [thin(&edge), thin(&edge)];
            look.header = emphasis;
            look.row_stripe[0] = fill(light(0.6));
            look.column_stripe[0] = fill(light(0.6));
        }
        ("Dark", 0) => {
            let body = if slot == 0 {
                "737373".to_owned()
            } else {
                base.clone()
            };
            look.whole.fill = Some(body);
            look.whole.color = white.clone();
            look.header = DifferentialLook {
                fill: Some("000000".into()),
                color: white.clone(),
                bold,
                ..DifferentialLook::default()
            };
            look.header.borders[3] = medium("FFFFFF");
            let shade = if slot == 0 {
                "404040".to_owned()
            } else {
                tinted(&base, -0.25)
            };
            look.row_stripe[0] = fill(shade.clone());
            look.column_stripe[0] = fill(shade.clone());
            look.first = DifferentialLook {
                fill: Some(shade.clone()),
                color: white.clone(),
                bold,
                ..DifferentialLook::default()
            };
            look.last = look.first.clone();
            look.total = look.first.clone();
            look.total.borders[2] = medium("FFFFFF");
        }
        ("Dark", _) => {
            // Dark 8 to 11 pair the accents: the header in one, the body
            // in the lighter shade of the one before.
            let accent = |number: usize| {
                palette
                    .theme
                    .as_ref()
                    .and_then(|theme| theme.color(&format!("accent{number}")))
                    .unwrap_or("4472C4")
                    .to_owned()
            };
            let (head, body) = match number {
                8 => ("000000".to_owned(), "D9D9D9".to_owned()),
                9 => (accent(2), tinted(&accent(1), 0.8)),
                10 => (accent(4), tinted(&accent(3), 0.8)),
                _ => (accent(6), tinted(&accent(5), 0.8)),
            };
            look.whole.fill = Some(body.clone());
            look.header = DifferentialLook {
                fill: Some(head),
                color: white,
                bold,
                ..DifferentialLook::default()
            };
            let stripe = if number == 8 {
                "A6A6A6".to_owned()
            } else {
                tinted(&body, -0.2)
            };
            look.row_stripe[0] = fill(stripe.clone());
            look.column_stripe[0] = fill(stripe);
            look.total.borders[2] = Some(BorderSide {
                style: "double".into(),
                color: Some("000000".into()),
            });
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STYLES: &str = r#"<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
<dxfs><dxf><font><b/><i val="0"/><color rgb="FF9C0006"/></font><fill><patternFill><bgColor rgb="FFFFC7CE"/></patternFill></fill>
<border><left style="thin"><color rgb="FF000000"/></left><horizontal style="hair"/></border></dxf></dxfs>
<tableStyles><tableStyle name="Mine"><tableStyleElement type="headerRow" dxfId="0"/></tableStyle></tableStyles></styleSheet>"#;

    #[test]
    fn differential_formats_and_rules_are_read_with_their_ranges() {
        let styles = Document::parse(STYLES).unwrap();
        let palette = Palette::new(None, Some(&styles));
        let formats = differential_looks(styles.root_element(), &palette);
        assert_eq!(formats[0].fill.as_deref(), Some("FFC7CE"));
        assert_eq!(formats[0].color.as_deref(), Some("9C0006"));
        assert_eq!(
            (formats[0].bold, formats[0].italic),
            (Some(true), Some(false))
        );
        assert_eq!(formats[0].strike, None);
        assert_eq!(formats[0].borders[0].as_ref().unwrap().style, "thin");
        assert_eq!(formats[0].inner[0].as_ref().unwrap().style, "hair");
        let sheet = Document::parse(r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
<conditionalFormatting sqref="A1:B2 D4"><cfRule type="cellIs" dxfId="0" priority="3" operator="between" stopIfTrue="1"><formula>1</formula><formula>$C$1</formula></cfRule>
<cfRule type="expression" dxfId="0" priority="2"><formula>$A1="済"</formula></cfRule></conditionalFormatting>
<conditionalFormatting sqref="C1:C9"><cfRule type="colorScale" priority="1"><colorScale><cfvo type="min"/><cfvo type="percentile" val="50"/><cfvo type="max"/>
<color rgb="FFF8696B"/><color rgb="FFFFEB84"/><color rgb="FF63BE7B"/></colorScale></cfRule>
<cfRule type="dataBar" priority="4"><dataBar showValue="0"><cfvo type="num" val="0"/><cfvo type="max"/><color rgb="FF638EC6"/></dataBar></cfRule>
<cfRule type="iconSet" priority="5"><iconSet iconSet="3Arrows" reverse="1"><cfvo type="percent" val="0"/><cfvo type="percent" val="33"/><cfvo type="num" val="7"/></iconSet></cfRule></conditionalFormatting>
</worksheet>"#).unwrap();
        let looks = conditional_looks(sheet.root_element(), &formats, &palette);
        // In the order of their priorities.
        assert_eq!(
            looks.iter().map(|l| l.priority).collect::<Vec<_>>(),
            [1, 2, 3, 4, 5]
        );
        assert!(matches!(&looks[0].rule, Rule::ColorScale(stops)
            if stops[1] == (Threshold::Percentile(50.0), "FFEB84".to_owned())));
        assert_eq!(looks[1].ranges, [(1, 1, 2, 2), (4, 4, 4, 4)]);
        assert!(
            matches!(&looks[1].rule, Rule::Format { test: Test::Expression(f), stop: false, .. } if f == "$A1=\"済\"")
        );
        assert!(
            matches!(&looks[2].rule, Rule::Format { test: Test::CellIs { operator, operands }, stop: true, format }
            if operator == "between" && operands.len() == 2 && format.fill.as_deref() == Some("FFC7CE"))
        );
        assert!(
            matches!(&looks[3].rule, Rule::DataBar { min: Threshold::Number(n), max: Threshold::Max, show_value: false, color }
            if *n == 0.0 && color == "638EC6")
        );
        assert!(
            matches!(&looks[4].rule, Rule::IconSet { set, thresholds, reverse: true, show_value: true }
            if set == "3Arrows" && thresholds[2] == Threshold::Number(7.0))
        );
    }

    #[test]
    fn tables_take_the_style_the_workbook_defines_or_excels_own() {
        let styles = Document::parse(STYLES).unwrap();
        let palette = Palette::new(None, Some(&styles));
        let formats = differential_looks(styles.root_element(), &palette);
        let table = |style: &str| {
            format!(
                r#"<table xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" ref="B2:D5" totalsRowCount="1"><tableStyleInfo name="{style}" showRowStripes="1" showFirstColumn="1"/></table>"#
            )
        };
        let parts = |style: &str| {
            BTreeMap::from([
                (
                    "xl/worksheets/_rels/sheet1.xml.rels".to_owned(),
                    br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/table" Target="../tables/table1.xml"/></Relationships>"#.to_vec(),
                ),
                ("xl/tables/table1.xml".to_owned(), table(style).into_bytes()),
            ])
        };
        let looks = |style: &str| {
            table_looks(
                &parts(style),
                "xl/worksheets/sheet1.xml",
                Some(styles.root_element()),
                &formats,
                &palette,
            )
        };
        let mine = &looks("Mine")[0];
        assert_eq!(mine.range, (2, 2, 4, 5));
        assert_eq!((mine.header_rows, mine.totals_rows), (1, 1));
        assert!(mine.row_stripes && mine.first_column && !mine.last_column);
        assert_eq!(mine.header.fill.as_deref(), Some("FFC7CE"));
        assert_eq!(mine.row_stripe[0], DifferentialLook::default());
        // Excel's own, in the accent a theme would give: the default's here.
        let medium = &looks("TableStyleMedium2")[0];
        assert_eq!(medium.header.fill.as_deref(), Some("4472C4"));
        assert_eq!(medium.header.color.as_deref(), Some("FFFFFF"));
        assert_eq!(medium.row_stripe[0].fill.as_deref(), Some("DAE3F3"));
        let black = &looks("TableStyleLight1")[0];
        assert_eq!(black.row_stripe[0].fill.as_deref(), Some("D9D9D9"));
        assert_eq!(black.whole.color, None);
    }
}
