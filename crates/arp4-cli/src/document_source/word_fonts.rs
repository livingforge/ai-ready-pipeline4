//! Fonts of Word text as Word shows them: run properties resolved through the
//! document defaults, the table style, the paragraph and character styles and
//! the theme, and font edits written to the runs.
use super::word::Block;
use super::*;
use crate::fonts::{AUTO, Color, FontEdit, RunFont, Theme};

/// A style of `word/styles.xml`.
struct Style {
    based_on: Option<String>,
    /// The run properties the style sets (`rPr`).
    run: RunFont,
    /// Table styles: the run properties of their conditional formats
    /// (`tblStylePr`) by type (`firstRow`, `lastCol`, …).
    conditional: BTreeMap<String, RunFont>,
}

/// The styles, defaults and theme fonts of a Word document.
pub(super) struct WordStyles {
    theme: Option<Theme>,
    defaults: RunFont,
    styles: BTreeMap<String, Style>,
    default_paragraph: Option<String>,
    default_character: Option<String>,
    default_table: Option<String>,
}

/// The theme slot a Word `themeColor` names.
fn theme_slot(name: &str) -> Option<&'static str> {
    Some(match name {
        "dark1" | "text1" => "dk1",
        "light1" | "background1" => "lt1",
        "dark2" | "text2" => "dk2",
        "light2" | "background2" => "lt2",
        "accent1" => "accent1",
        "accent2" => "accent2",
        "accent3" => "accent3",
        "accent4" => "accent4",
        "accent5" => "accent5",
        "accent6" => "accent6",
        "hyperlink" => "hlink",
        "followedHyperlink" => "folHlink",
        _ => return None,
    })
}

fn word_child<'a, 'input>(node: Node<'a, 'input>, name: &str) -> Option<Node<'a, 'input>> {
    node.children().find(|n| n.has_tag_name((WORD, name)))
}

fn word_value<'a>(node: Node<'a, '_>) -> Option<&'a str> {
    node.attribute((WORD, "val"))
}

impl WordStyles {
    pub(super) fn new(parts: &BTreeMap<String, Vec<u8>>) -> Result<Self> {
        let theme = parts
            .iter()
            .find(|(name, _)| name.starts_with("word/theme/") && name.ends_with(".xml"))
            .map(|(_, bytes)| Theme::parse(&part_text(bytes)?.0))
            .transpose()?;
        let mut out = Self {
            theme,
            defaults: RunFont::default(),
            styles: BTreeMap::new(),
            default_paragraph: None,
            default_character: None,
            default_table: None,
        };
        let Some(bytes) = parts.get("word/styles.xml") else {
            return Ok(out);
        };
        let (text, _) = part_text(bytes)?;
        let doc = Document::parse(&text)?;
        let root = doc.root_element();
        if let Some(defaults) = word_child(root, "docDefaults")
            .and_then(|d| word_child(d, "rPrDefault"))
            .and_then(|d| word_child(d, "rPr"))
        {
            out.defaults = out.run_properties(defaults);
        }
        for style in root.children().filter(|n| n.has_tag_name((WORD, "style"))) {
            let Some(id) = style.attribute((WORD, "styleId")) else {
                continue;
            };
            let kind = style.attribute((WORD, "type")).unwrap_or("paragraph");
            if matches!(
                style.attribute((WORD, "default")),
                Some("1" | "true" | "on")
            ) {
                let slot = match kind {
                    "paragraph" => &mut out.default_paragraph,
                    "character" => &mut out.default_character,
                    "table" => &mut out.default_table,
                    _ => continue,
                };
                slot.get_or_insert_with(|| id.to_owned());
            }
            let conditional = style
                .children()
                .filter(|n| n.has_tag_name((WORD, "tblStylePr")))
                .filter_map(|p| {
                    Some((
                        p.attribute((WORD, "type"))?.to_owned(),
                        word_child(p, "rPr")
                            .map(|r| out.run_properties(r))
                            .unwrap_or_default(),
                    ))
                })
                .collect();
            let parsed = Style {
                based_on: word_child(style, "basedOn")
                    .and_then(word_value)
                    .map(str::to_owned),
                run: word_child(style, "rPr")
                    .map(|r| out.run_properties(r))
                    .unwrap_or_default(),
                conditional,
            };
            out.styles.insert(id.to_owned(), parsed);
        }
        Ok(out)
    }

    /// The properties run properties (`rPr`) set themselves. A theme font or
    /// colour wins over the explicit one beside it, as in Word.
    fn run_properties(&self, properties: Node<'_, '_>) -> RunFont {
        let fonts = word_child(properties, "rFonts");
        let face = |theme_attribute: &str, attribute: &str| -> Option<String> {
            let fonts = fonts?;
            if let Some(reference) = fonts.attribute((WORD, theme_attribute)) {
                let major = reference.starts_with("major");
                let east = reference.ends_with("EastAsia");
                let font = self.theme.as_ref()?.font(major);
                return if east { font.east_asian } else { font.latin };
            }
            fonts
                .attribute((WORD, attribute))
                .filter(|f| !f.is_empty())
                .map(str::to_owned)
        };
        let color = word_child(properties, "color").and_then(|c| {
            if let (Some(slot), Some(theme)) = (
                c.attribute((WORD, "themeColor")).and_then(theme_slot),
                self.theme.as_ref(),
            ) && let Some(rgb) = theme.color(slot)
            {
                return Some(Color::Rgb {
                    rgb: crate::fonts::word_theme_color(
                        rgb,
                        c.attribute((WORD, "themeTint")),
                        c.attribute((WORD, "themeShade")),
                    ),
                    theme: Some(slot.to_owned()),
                });
            }
            match word_value(c)? {
                "auto" => Some(Color::Auto),
                rgb if rgb.len() == 6 && rgb.bytes().all(|b| b.is_ascii_hexdigit()) => {
                    Some(Color::rgb(rgb.to_ascii_uppercase()))
                }
                _ => None,
            }
        });
        RunFont {
            latin: face("asciiTheme", "ascii"),
            east_asian: face("eastAsiaTheme", "eastAsia"),
            size: word_child(properties, "sz")
                .and_then(word_value)
                .and_then(|v| v.parse::<f64>().ok())
                .map(|half| half / 2.0),
            color,
        }
    }

    /// The run properties of style `id` with those of the styles it is based on.
    fn style(&self, id: Option<&str>) -> RunFont {
        let mut font = RunFont::default();
        let mut next = id;
        // Styles may name each other in a loop; Word stops following them.
        for _ in 0..32 {
            let Some(style) = next.and_then(|id| self.styles.get(id)) else {
                break;
            };
            font = font.or(&style.run);
            next = style.based_on.as_deref();
        }
        font
    }

    /// The conditional formats of table style `id` (and its bases) of `kind`.
    fn conditional(&self, id: Option<&str>, kind: &str) -> RunFont {
        let mut font = RunFont::default();
        let mut next = id;
        for _ in 0..32 {
            let Some(style) = next.and_then(|id| self.styles.get(id)) else {
                break;
            };
            if let Some(own) = style.conditional.get(kind) {
                font = font.or(own);
            }
            next = style.based_on.as_deref();
        }
        font
    }

    /// The font of `run` (`w:r`) as Word shows it.
    fn run(&self, run: Node<'_, '_>, place: Option<&TablePlace>) -> RunFont {
        let direct = word_child(run, "rPr")
            .map(|p| self.run_properties(p))
            .unwrap_or_default();
        let character = self.style(
            word_child(run, "rPr")
                .and_then(|p| word_child(p, "rStyle"))
                .and_then(word_value)
                .or(self.default_character.as_deref()),
        );
        let paragraph_node = run.ancestors().find(|n| n.has_tag_name((WORD, "p")));
        let paragraph = self.style(
            paragraph_node
                .and_then(|p| word_child(p, "pPr"))
                .and_then(|p| word_child(p, "pStyle"))
                .and_then(word_value)
                .or(self.default_paragraph.as_deref()),
        );
        let table = match (
            place,
            run.ancestors().find(|n| n.has_tag_name((WORD, "tbl"))),
        ) {
            (Some(place), Some(table)) => self.table(table, place),
            _ => RunFont::default(),
        };
        direct
            .or(&character)
            .or(&paragraph)
            .or(&table)
            .or(&self.defaults)
            // Text without a colour is automatic. A font or size the document
            // leaves unset is Word's own choice, which differs by installation.
            .or(&RunFont {
                color: Some(Color::Auto),
                ..RunFont::default()
            })
    }

    /// What the style of `table` gives a cell at `place`: the conditional
    /// formats its look (`tblLook`) turns on, rows over columns, then the
    /// whole table.
    fn table(&self, table: Node<'_, '_>, place: &TablePlace) -> RunFont {
        let properties = word_child(table, "tblPr");
        let id = properties
            .and_then(|p| word_child(p, "tblStyle"))
            .and_then(word_value)
            .or(self.default_table.as_deref());
        let look = properties.and_then(|p| word_child(p, "tblLook"));
        let flag = |name: &str, bit: u32, default: bool| -> bool {
            let Some(look) = look else {
                return default;
            };
            if let Some(value) = look.attribute((WORD, name)) {
                return matches!(value, "1" | "true" | "on");
            }
            word_value(look)
                .and_then(|v| u32::from_str_radix(v, 16).ok())
                .map_or(default, |mask| mask & bit != 0)
        };
        let mut font = RunFont::default();
        for (kind, applies) in [
            (
                "firstRow",
                place.first_row && flag("firstRow", 0x0020, true),
            ),
            ("lastRow", place.last_row && flag("lastRow", 0x0040, false)),
            (
                "firstCol",
                place.first_column && flag("firstColumn", 0x0080, true),
            ),
            (
                "lastCol",
                place.last_column && flag("lastColumn", 0x0100, false),
            ),
        ] {
            if applies {
                font = font.or(&self.conditional(id, kind));
            }
        }
        font.or(&self.style(id))
    }

    /// The font of a block of Word text from the runs that show it.
    pub(super) fn block_font(&self, block: &Block<'_, '_>) -> Value {
        let runs: Vec<_> = block
            .runs
            .iter()
            .map(|run| self.run(*run, block.place.as_ref()))
            .collect();
        crate::fonts::element_font(&runs, &RunFont::default())
    }
}

/// Where a table cell sits, for the conditional formats of the table style.
#[derive(Clone, Debug, Default)]
pub(super) struct TablePlace {
    pub first_row: bool,
    pub last_row: bool,
    pub first_column: bool,
    pub last_column: bool,
}

/// The children of Word run properties in schema order (CT_RPr, with the
/// change marks a paragraph mark's properties start with).
const WORD_RUN_ORDER: [&str; 44] = [
    "ins",
    "del",
    "moveFrom",
    "moveTo",
    "rStyle",
    "rFonts",
    "b",
    "bCs",
    "i",
    "iCs",
    "caps",
    "smallCaps",
    "strike",
    "dstrike",
    "outline",
    "shadow",
    "emboss",
    "imprint",
    "noProof",
    "snapToGrid",
    "vanish",
    "webHidden",
    "color",
    "spacing",
    "w",
    "kern",
    "position",
    "sz",
    "szCs",
    "highlight",
    "u",
    "effect",
    "bdr",
    "shd",
    "fitText",
    "vertAlign",
    "rtl",
    "cs",
    "em",
    "lang",
    "eastAsianLayout",
    "specVanish",
    "oMath",
    "rPrChange",
];

/// Word run properties (`w:rPr`) with `edit` applied, or new ones (in the
/// namespace prefix `prefix`) for a run that has none.
fn edited_run_properties(
    source: &str,
    element: Option<Node<'_, '_>>,
    prefix: &str,
    edit: &FontEdit,
) -> Result<String> {
    let (opening, mut children, name) = match element {
        Some(element) => (
            crate::fonts::opening_tag(source, element)?.to_owned(),
            crate::fonts::children_of(source, element),
            crate::fonts::qualified_name(source, element).to_owned(),
        ),
        None => (format!("<{prefix}rPr"), vec![], format!("{prefix}rPr")),
    };
    let prefix = crate::fonts::prefix_of(&name).to_owned();
    let attribute = |key: &str| format!("{prefix}{key}");
    if edit.latin.is_some() || edit.east_asian.is_some() {
        let existing = children.iter().position(|(child, _)| child == "rFonts");
        let mut fonts = match existing {
            Some(index) => {
                let xml = children.remove(index).1;
                xml.trim_end_matches('>')
                    .trim_end_matches('/')
                    .trim_end()
                    .to_owned()
            }
            None => format!("<{prefix}rFonts"),
        };
        for (value, faces, theme) in [
            (
                &edit.latin,
                ["ascii", "hAnsi"].as_slice(),
                ["asciiTheme", "hAnsiTheme"].as_slice(),
            ),
            (
                &edit.east_asian,
                ["eastAsia"].as_slice(),
                ["eastAsiaTheme"].as_slice(),
            ),
        ] {
            let Some(face) = value else {
                continue;
            };
            for key in theme.iter().chain(faces) {
                fonts = crate::fonts::without_attribute(&fonts, &attribute(key))?;
            }
            for key in faces {
                fonts.push_str(&format!(
                    " {}=\"{}\"",
                    attribute(key),
                    crate::fonts::attribute_text(face)
                ));
            }
        }
        crate::fonts::insert_ordered(
            &mut children,
            ("rFonts".into(), format!("{fonts}/>")),
            &WORD_RUN_ORDER,
        );
    }
    if let Some(color) = &edit.color {
        children.retain(|(child, _)| child != "color");
        let value = if color == AUTO { "auto" } else { color };
        crate::fonts::insert_ordered(
            &mut children,
            (
                "color".into(),
                format!("<{prefix}color {}=\"{value}\"/>", attribute("val")),
            ),
            &WORD_RUN_ORDER,
        );
    }
    if let Some(size) = edit.size {
        let half = (size * 2.0).round() as i64;
        children.retain(|(child, _)| child != "sz" && child != "szCs");
        for key in ["sz", "szCs"] {
            crate::fonts::insert_ordered(
                &mut children,
                (
                    key.into(),
                    format!("<{prefix}{key} {}=\"{half}\"/>", attribute("val")),
                ),
                &WORD_RUN_ORDER,
            );
        }
    }
    if children.is_empty() {
        return Ok(format!("{opening}/>"));
    }
    let content: String = children.into_iter().map(|(_, xml)| xml).collect();
    Ok(format!("{opening}>{content}</{name}>"))
}

/// Checks a font edit Word can write: sizes in half points up to 1638.
pub(super) fn ensure_word_edit(edit: &FontEdit) -> Result<()> {
    if let Some(size) = edit.size {
        ensure!(
            size <= 1638.0 && (size * 2.0).fract() == 0.0,
            "Word font size must be 1 to 1638 points in steps of 0.5"
        );
    }
    Ok(())
}

/// The edits giving the runs of `block` (and their copies for older readers)
/// the font of `edit`, with the marks of the paragraphs they belong to.
pub(super) fn run_edits<'a, 'input>(
    source: &str,
    block: &Block<'a, 'input>,
    edit: &FontEdit,
    format: &str,
    cache: &mut BTreeMap<(usize, bool), BranchTexts<'a, 'input>>,
) -> Result<Vec<(std::ops::Range<usize>, String)>> {
    let mut runs: Vec<Node<'a, 'input>> = block.runs.clone();
    for run in &block.runs {
        if let Some(text) = run.children().find(|n| n.has_tag_name((WORD, "t"))) {
            for copy in hidden_copies(text, format, cache)? {
                runs.extend(copy.parent());
            }
        }
    }
    let mut seen = BTreeSet::new();
    let mut edits = vec![];
    let mut paragraphs = BTreeSet::new();
    for run in runs {
        if !seen.insert(run.range().start) {
            continue;
        }
        let prefix = crate::fonts::prefix_of(crate::fonts::qualified_name(source, run)).to_owned();
        match word_child(run, "rPr") {
            Some(properties) => edits.push((
                properties.range(),
                edited_run_properties(source, Some(properties), &prefix, edit)?,
            )),
            None => {
                let raw = &source[run.range()];
                let at = run.range().start + raw.find('>').context("invalid run")? + 1;
                edits.push((at..at, edited_run_properties(source, None, &prefix, edit)?));
            }
        }
        if let Some(paragraph) = run.ancestors().find(|n| n.has_tag_name((WORD, "p")))
            && paragraphs.insert(paragraph.range().start)
        {
            edits.push(mark_edit(source, paragraph, edit)?);
        }
    }
    Ok(edits)
}

/// The edit giving the mark of `paragraph` the font of `edit`. A paragraph
/// without mark properties gets them, so that the mark (which sets the height
/// of an empty last line and the look of the paragraph's number) follows its
/// text.
fn mark_edit(
    source: &str,
    paragraph: Node<'_, '_>,
    edit: &FontEdit,
) -> Result<(std::ops::Range<usize>, String)> {
    let Some(properties) = word_child(paragraph, "pPr") else {
        // `pPr` is the first child of a paragraph.
        let prefix = crate::fonts::prefix_of(crate::fonts::qualified_name(source, paragraph));
        let raw = &source[paragraph.range()];
        let at = paragraph.range().start + raw.find('>').context("invalid paragraph")? + 1;
        let mark = edited_run_properties(source, None, prefix, edit)?;
        return Ok((at..at, format!("<{prefix}pPr>{mark}</{prefix}pPr>")));
    };
    let prefix = crate::fonts::prefix_of(crate::fonts::qualified_name(source, properties));
    if let Some(mark) = word_child(properties, "rPr") {
        let prefix = crate::fonts::prefix_of(crate::fonts::qualified_name(source, mark));
        return Ok((
            mark.range(),
            edited_run_properties(source, Some(mark), prefix, edit)?,
        ));
    }
    let mark = edited_run_properties(source, None, prefix, edit)?;
    // Only the section and the tracked change of the properties follow `rPr`.
    if let Some(next) = properties
        .children()
        .find(|n| n.has_tag_name((WORD, "sectPr")) || n.has_tag_name((WORD, "pPrChange")))
    {
        let at = next.range().start;
        return Ok((at..at, mark));
    }
    let raw = &source[properties.range()];
    if raw.ends_with("/>") {
        let opening = crate::fonts::opening_tag(source, properties)?;
        let name = crate::fonts::qualified_name(source, properties);
        return Ok((properties.range(), format!("{opening}>{mark}</{name}>")));
    }
    let at = properties.range().start + raw.rfind("</").context("invalid pPr")?;
    Ok((at..at, mark))
}
