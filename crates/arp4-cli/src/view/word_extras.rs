//! What a Word part says of its paragraphs and tables beyond the extraction:
//! the number or bullet of a list paragraph, its indent and alignment, the
//! pictures a paragraph holds, and whether a table draws borders and how
//! wide its columns are. Read from the original for the page to draw.
use super::*;
use foldhash::HashMap;
use roxmltree::{Document as Xml, Node};

const WORD: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const DRAWING: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
/// Twips (twentieths of a point) of a pixel, and EMUs of one.
const TWIPS: f64 = 15.0;
const EMU: f64 = 9525.0;

#[derive(Default)]
pub(super) struct Extras {
    /// By the row of the part's sheet.
    pub rows: HashMap<u32, RowExtra>,
    /// By the first row of the table.
    pub tables: HashMap<u32, TableExtra>,
    /// The image assets placed with a row, which need no place of their own.
    pub placed: Vec<String>,
}

#[derive(Default)]
pub(super) struct RowExtra {
    /// The number or bullet before the paragraph.
    pub label: Option<String>,
    /// The indent of the paragraph and how far its first line hangs out of
    /// it, in pixels.
    pub left: f64,
    pub hanging: f64,
    /// CSS `text-align`.
    pub align: Option<&'static str>,
    /// The pictures shown after the row: a `data:` URL, width and height.
    pub pictures: Vec<(String, f64, f64)>,
}

pub(super) struct TableExtra {
    pub bordered: bool,
    /// The widths of the grid's columns, in pixels; empty for a table that
    /// fits its columns to what they hold.
    pub widths: Vec<f64>,
}

fn child<'a, 'i>(node: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    node.children().find(|n| n.has_tag_name((WORD, name)))
}

fn value<'a>(node: Node<'a, '_>, name: &str) -> Option<&'a str> {
    node.attribute((WORD, name))
}

/// A level of a list: where it starts, how its number is written and laid
/// in its text (`%1.`), and its indent in twips.
#[derive(Clone)]
struct Level {
    start: i64,
    format: String,
    text: String,
    left: Option<f64>,
    hanging: Option<f64>,
}

/// The paragraph properties a style gives, with the style it is based on.
struct Style<'a, 'i> {
    properties: Option<Node<'a, 'i>>,
    table: Option<Node<'a, 'i>>,
    based_on: Option<&'a str>,
}

/// The extras of the Word part `part` of `document`; none when the original
/// is not a Word document or the part cannot be read.
pub(super) fn read(document: &Document<'_>, part: &str) -> Option<Extras> {
    let original = document.original;
    let styles_text = original.office_part("word/styles.xml");
    let styles_xml = styles_text.as_deref().and_then(|t| Xml::parse(t).ok());
    let numbering_text = original.office_part("word/numbering.xml");
    let numbering_xml = numbering_text.as_deref().and_then(|t| Xml::parse(t).ok());
    let (directory, file) = part.rsplit_once('/').unwrap_or(("", part));
    let rels_text = original.office_part(&format!("{directory}/_rels/{file}.rels"));
    let rels_xml = rels_text.as_deref().and_then(|t| Xml::parse(t).ok());
    // The package file each relationship of the part names.
    let targets: HashMap<&str, String> = rels_xml
        .iter()
        .flat_map(|xml| xml.root_element().children())
        .filter_map(|rel| {
            let target = rel.attribute("Target")?;
            let path = match target.strip_prefix('/') {
                Some(absolute) => absolute.to_owned(),
                None => {
                    let mut parts: Vec<&str> = directory.split('/').collect();
                    for step in target.split('/') {
                        match step {
                            ".." => {
                                parts.pop();
                            }
                            "." | "" => {}
                            step => parts.push(step),
                        }
                    }
                    parts.join("/")
                }
            };
            Some((rel.attribute("Id")?, path))
        })
        .collect();
    let styles: HashMap<&str, Style<'_, '_>> = styles_xml
        .iter()
        .flat_map(|xml| xml.root_element().children())
        .filter(|n| n.has_tag_name((WORD, "style")))
        .filter_map(|style| {
            Some((
                value(style, "styleId")?,
                Style {
                    properties: child(style, "pPr"),
                    table: child(style, "tblPr"),
                    based_on: child(style, "basedOn").and_then(|b| value(b, "val")),
                },
            ))
        })
        .collect();
    let levels = levels(numbering_xml.as_ref());
    // The images of the extraction, by the hash of their bytes.
    let assets: HashMap<&str, &str> = document.extraction["assets"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|asset| Some((asset["sha256"].as_str()?, asset["path"].as_str()?)))
        .collect();
    original.word_rows(part, |xml, rows| {
        let mut extras = Extras::default();
        // The count of each level of each list, as the paragraphs go by.
        let mut counts: HashMap<(String, usize), i64> = HashMap::default();
        let mut tables: foldhash::HashSet<roxmltree::NodeId> = foldhash::HashSet::default();
        for (&row, node) in rows {
            let row = row as u32;
            if node.has_tag_name((WORD, "tr")) {
                let Some(table) = node
                    .ancestors()
                    .find(|n| n.has_tag_name((WORD, "tbl")))
                    .filter(|table| tables.insert(table.id()))
                else {
                    continue;
                };
                extras.tables.insert(row, table_extra(table, &styles));
                continue;
            }
            let own = child(*node, "pPr");
            // A property of the paragraph, else of its style and those the
            // style is based on.
            let property = |name: &str| -> Option<Node<'_, '_>> {
                if let Some(found) = own.and_then(|p| child(p, name)) {
                    return Some(found);
                }
                let mut style = own
                    .and_then(|p| child(p, "pStyle"))
                    .and_then(|s| value(s, "val"));
                for _ in 0..8 {
                    let found = styles.get(style?)?;
                    if let Some(property) = found.properties.and_then(|p| child(p, name)) {
                        return Some(property);
                    }
                    style = found.based_on;
                }
                None
            };
            let mut extra = RowExtra::default();
            let twips = |node: Option<Node<'_, '_>>, names: &[&str]| {
                names
                    .iter()
                    .find_map(|name| value(node?, name)?.parse::<f64>().ok())
            };
            let indent = property("ind");
            let mut left = twips(indent, &["left", "start"]);
            let mut hanging = twips(indent, &["hanging"]);
            let first = twips(indent, &["firstLine"]);
            if let Some(list) = property("numPr") {
                let id = child(list, "numId").and_then(|n| value(n, "val"));
                let level = child(list, "ilvl")
                    .and_then(|n| value(n, "val")?.parse::<usize>().ok())
                    .unwrap_or(0);
                // List 0 takes a paragraph out of its style's list.
                if let Some(id) = id.filter(|id| *id != "0") {
                    let of_list = levels.get(id);
                    let at = |level: usize| {
                        of_list
                            .and_then(|levels| levels.get(level))
                            .cloned()
                            .unwrap_or_else(|| default_level(level))
                    };
                    let this = at(level);
                    *counts
                        .entry((id.to_owned(), level))
                        .or_insert(this.start - 1) += 1;
                    // A paragraph of a level starts those under it again.
                    counts.retain(|(list, deeper), _| list != id || *deeper <= level);
                    let mut label = String::new();
                    let mut chars = this.text.chars().peekable();
                    while let Some(c) = chars.next() {
                        match (c, chars.peek().and_then(|d| d.to_digit(10))) {
                            ('%', Some(digit)) if digit >= 1 => {
                                chars.next();
                                let of = digit as usize - 1;
                                let count = counts
                                    .get(&(id.to_owned(), of))
                                    .copied()
                                    .unwrap_or_else(|| at(of).start);
                                label.push_str(&numbered(&at(of).format, count));
                            }
                            // A bullet of a symbol font, by its private code.
                            (c, _) if ('\u{f000}'..='\u{f0ff}').contains(&c) => label.push('•'),
                            (c, _) => label.push(c),
                        }
                    }
                    if this.format != "none" && !label.is_empty() {
                        extra.label = Some(label);
                    }
                    left = left.or(this.left);
                    hanging = hanging.or(this.hanging);
                }
            }
            extra.left = left.unwrap_or(0.0) / TWIPS;
            extra.hanging = match (hanging, first) {
                (Some(hanging), _) => hanging / TWIPS,
                (None, Some(first)) => -first / TWIPS,
                (None, None) => 0.0,
            };
            extra.align = match property("jc").and_then(|jc| value(jc, "val")) {
                Some("center") => Some("center"),
                Some("right" | "end") => Some("right"),
                Some("both" | "distribute") => Some("justify"),
                _ => None,
            };
            if extra.label.is_some()
                || extra.left != 0.0
                || extra.hanging != 0.0
                || extra.align.is_some()
            {
                extras.rows.insert(row, extra);
            }
        }
        // Each picture goes with the row it is in or, in a paragraph without
        // text, which has no row, the row before it.
        let mut starts: Vec<(usize, u32, bool)> = rows
            .iter()
            .map(|(row, node)| {
                (
                    node.range().start,
                    *row as u32,
                    node.has_tag_name((WORD, "tr")),
                )
            })
            .collect();
        starts.sort_unstable();
        for blip in xml
            .descendants()
            .filter(|n| n.has_tag_name((DRAWING, "blip")))
        {
            let at = starts.partition_point(|(start, ..)| *start <= blip.range().start);
            // A picture before every row, or in a table, keeps its place
            // after the text.
            let Some(&(_, row, false)) = at.checked_sub(1).and_then(|at| starts.get(at)) else {
                continue;
            };
            if blip
                .ancestors()
                .any(|n| n.has_tag_name((WORD, "tc")) || n.tag_name().name() == "Fallback")
            {
                continue;
            }
            let Some(asset) = blip
                .attribute((REL, "embed"))
                .and_then(|id| targets.get(id))
                .and_then(|target| original.office_file(target))
                .and_then(|bytes| assets.get(crate::data::hash(&bytes).as_str()).copied())
            else {
                continue;
            };
            let Some(url) = image_url(document.assets, asset) else {
                continue;
            };
            // The size the drawing gives the picture on the paper.
            let extent = blip
                .ancestors()
                .find(|n| matches!(n.tag_name().name(), "inline" | "anchor"))
                .and_then(|frame| frame.children().find(|n| n.tag_name().name() == "extent"))
                .and_then(|extent| {
                    Some((
                        extent.attribute("cx")?.parse::<f64>().ok()? / EMU,
                        extent.attribute("cy")?.parse::<f64>().ok()? / EMU,
                    ))
                })
                .unwrap_or((0.0, 0.0));
            extras
                .rows
                .entry(row)
                .or_default()
                .pictures
                .push((url, extent.0, extent.1));
            extras.placed.push(asset.to_owned());
        }
        extras
    })
}

/// The levels of each list of the numbering part, by the list's number.
fn levels(numbering: Option<&Xml<'_>>) -> HashMap<String, Vec<Level>> {
    let Some(root) = numbering.map(Xml::root_element) else {
        return HashMap::default();
    };
    let level = |node: Node<'_, '_>| {
        let of = |name: &str| child(node, name).and_then(|n| value(n, "val"));
        let indent = child(node, "pPr").and_then(|p| child(p, "ind"));
        let twips = |names: &[&str]| {
            names
                .iter()
                .find_map(|name| value(indent?, name)?.parse::<f64>().ok())
        };
        Level {
            start: of("start").and_then(|s| s.parse().ok()).unwrap_or(1),
            format: of("numFmt").unwrap_or("decimal").to_owned(),
            text: of("lvlText").unwrap_or_default().to_owned(),
            left: twips(&["left", "start"]),
            hanging: twips(&["hanging"]),
        }
    };
    let abstracts: HashMap<&str, Vec<Level>> = root
        .children()
        .filter(|n| n.has_tag_name((WORD, "abstractNum")))
        .filter_map(|list| {
            let mut levels: Vec<(usize, Level)> = list
                .children()
                .filter(|n| n.has_tag_name((WORD, "lvl")))
                .filter_map(|l| Some((value(l, "ilvl")?.parse().ok()?, level(l))))
                .collect();
            levels.sort_by_key(|(at, _)| *at);
            let mut dense = vec![];
            for (at, level) in levels {
                while dense.len() < at {
                    dense.push(default_level(dense.len()));
                }
                dense.push(level);
            }
            Some((value(list, "abstractNumId")?, dense))
        })
        .collect();
    root.children()
        .filter(|n| n.has_tag_name((WORD, "num")))
        .filter_map(|list| {
            let of = child(list, "abstractNumId").and_then(|n| value(n, "val"))?;
            Some((value(list, "numId")?.to_owned(), abstracts.get(of)?.clone()))
        })
        .collect()
}

/// The level Word gives a list whose numbering the document does not
/// define: a number and a full stop, each level half an inch further in.
fn default_level(level: usize) -> Level {
    Level {
        start: 1,
        format: "decimal".into(),
        text: format!("%{}.", level + 1),
        left: Some(720.0 * (level + 1) as f64),
        hanging: Some(360.0),
    }
}

/// `count` as a list of the number format `format` writes it.
fn numbered(format: &str, count: i64) -> String {
    let count = count.max(0);
    let letters = |first: char| {
        // a, b, …, z, aa, bb, …
        let at = ((count.max(1) - 1) % 26) as u8;
        let repeat = ((count.max(1) - 1) / 26 + 1) as usize;
        ((first as u8 + at) as char).to_string().repeat(repeat)
    };
    let roman = || {
        let mut left = count;
        let mut text = String::new();
        for (value, digits) in [
            (1000, "m"),
            (900, "cm"),
            (500, "d"),
            (400, "cd"),
            (100, "c"),
            (90, "xc"),
            (50, "l"),
            (40, "xl"),
            (10, "x"),
            (9, "ix"),
            (5, "v"),
            (4, "iv"),
            (1, "i"),
        ] {
            while left >= value {
                text.push_str(digits);
                left -= value;
            }
        }
        text
    };
    let of = |series: &str| {
        let chars: Vec<char> = series.chars().collect();
        chars[((count.max(1) - 1) as usize) % chars.len()].to_string()
    };
    let kanji = || {
        const DIGITS: [&str; 10] = ["〇", "一", "二", "三", "四", "五", "六", "七", "八", "九"];
        match count {
            0..=9 => DIGITS[count as usize].to_owned(),
            10..=99 => {
                let (tens, ones) = (count / 10, count % 10);
                format!(
                    "{}十{}",
                    if tens > 1 { DIGITS[tens as usize] } else { "" },
                    if ones > 0 { DIGITS[ones as usize] } else { "" }
                )
            }
            _ => count.to_string(),
        }
    };
    match format {
        "none" | "bullet" => String::new(),
        "decimalZero" => format!("{count:02}"),
        "lowerLetter" => letters('a'),
        "upperLetter" => letters('A'),
        "lowerRoman" => roman(),
        "upperRoman" => roman().to_uppercase(),
        "decimalFullWidth" | "decimalFullWidth2" => count
            .to_string()
            .chars()
            .map(|c| char::from_u32(c as u32 - '0' as u32 + '０' as u32).unwrap_or(c))
            .collect(),
        "decimalEnclosedCircle" | "decimalEnclosedCircleChinese" if (1..=20).contains(&count) => {
            char::from_u32('①' as u32 + count as u32 - 1)
                .map(String::from)
                .unwrap_or_default()
        }
        "aiueo" | "aiueoFullWidth" => of(
            "アイウエオカキクケコサシスセソタチツテトナニヌネノハヒフヘホマミムメモヤユヨラリルレロワヲン",
        ),
        "iroha" | "irohaFullWidth" => of(
            "イロハニホヘトチリヌルヲワカヨタレソツネナラムウヰノオクヤマケフコエテアサキユメミシヱヒモセスン",
        ),
        "japaneseCounting"
        | "japaneseDigitalTenThousand"
        | "ideographDigital"
        | "chineseCounting"
        | "chineseCountingThousand" => kanji(),
        _ => count.to_string(),
    }
}

/// Whether `table` draws borders, by its own properties, its style's or a
/// cell's, and the widths of its grid.
fn table_extra(table: Node<'_, '_>, styles: &HashMap<&str, Style<'_, '_>>) -> TableExtra {
    let drawn = |borders: Option<Node<'_, '_>>| {
        borders.is_some_and(|borders| {
            borders
                .children()
                .filter(Node::is_element)
                .any(|side| !matches!(value(side, "val"), None | Some("none" | "nil")))
        })
    };
    let properties = child(table, "tblPr");
    let own = properties.and_then(|p| child(p, "tblBorders"));
    let styled = || {
        let mut style = properties
            .and_then(|p| child(p, "tblStyle"))
            .and_then(|s| value(s, "val"));
        for _ in 0..8 {
            let Some(found) = style.and_then(|style| styles.get(style)) else {
                return false;
            };
            if let Some(borders) = found.table.and_then(|t| child(t, "tblBorders")) {
                return drawn(Some(borders));
            }
            style = found.based_on;
        }
        false
    };
    let bordered = match own {
        Some(own) => drawn(Some(own)),
        None => styled(),
    } || table
        .descendants()
        .filter(|n| n.has_tag_name((WORD, "tcBorders")))
        .any(|borders| drawn(Some(borders)));
    let widths = child(table, "tblGrid")
        .into_iter()
        .flat_map(|grid| {
            grid.children()
                .filter(|n| n.has_tag_name((WORD, "gridCol")))
        })
        .filter_map(|column| Some(value(column, "w")?.parse::<f64>().ok()? / TWIPS))
        .collect();
    TableExtra { bordered, widths }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_are_written_as_their_formats_write_them() {
        assert_eq!(numbered("decimal", 3), "3");
        assert_eq!(numbered("lowerLetter", 28), "bb");
        assert_eq!(numbered("upperRoman", 14), "XIV");
        assert_eq!(numbered("decimalFullWidth", 12), "１２");
        assert_eq!(numbered("decimalEnclosedCircle", 3), "③");
        assert_eq!(numbered("aiueo", 2), "イ");
        assert_eq!(numbered("japaneseCounting", 21), "二十一");
        assert_eq!(numbered("bullet", 1), "");
    }

    #[test]
    fn a_table_draws_borders_its_properties_its_style_or_a_cell_give() {
        let body = format!(
            r#"<w:body xmlns:w="{WORD}">
<w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w="1500"/><w:gridCol w:w="3000"/></w:tblGrid><w:tr><w:tc><w:p/></w:tc></w:tr></w:tbl>
<w:tbl><w:tblPr><w:tblStyle w:val="Grid"/></w:tblPr><w:tr><w:tc><w:p/></w:tc></w:tr></w:tbl>
<w:tbl><w:tblPr><w:tblStyle w:val="Grid"/><w:tblBorders><w:top w:val="nil"/></w:tblBorders></w:tblPr><w:tr><w:tc><w:p/></w:tc></w:tr></w:tbl>
<w:tbl><w:tr><w:tc><w:tcPr><w:tcBorders><w:bottom w:val="single"/></w:tcBorders></w:tcPr><w:p/></w:tc></w:tr></w:tbl>
</w:body>"#
        );
        let xml = Xml::parse(&body).unwrap();
        let styles_text = format!(
            r#"<w:styles xmlns:w="{WORD}"><w:style w:styleId="Base"><w:tblPr><w:tblBorders><w:top w:val="single"/></w:tblBorders></w:tblPr></w:style>
<w:style w:styleId="Grid"><w:basedOn w:val="Base"/></w:style></w:styles>"#
        );
        let styles_xml = Xml::parse(&styles_text).unwrap();
        let styles: HashMap<&str, Style<'_, '_>> = styles_xml
            .root_element()
            .children()
            .filter(|n| n.has_tag_name((WORD, "style")))
            .map(|style| {
                (
                    value(style, "styleId").unwrap(),
                    Style {
                        properties: None,
                        table: child(style, "tblPr"),
                        based_on: child(style, "basedOn").and_then(|b| value(b, "val")),
                    },
                )
            })
            .collect();
        let tables: Vec<TableExtra> = xml
            .root_element()
            .children()
            .filter(|n| n.has_tag_name((WORD, "tbl")))
            .map(|table| table_extra(table, &styles))
            .collect();
        // None of its own; its style's, from the one that is based on; its
        // own saying none over its style's; a cell's.
        assert_eq!(
            tables.iter().map(|t| t.bordered).collect::<Vec<_>>(),
            [false, true, false, true]
        );
        assert_eq!(tables[0].widths, [100.0, 200.0]);
        assert!(tables[1].widths.is_empty());
    }
}
