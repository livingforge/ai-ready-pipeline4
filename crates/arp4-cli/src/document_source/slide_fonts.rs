//! Fonts of PowerPoint text as PowerPoint shows them: run properties resolved
//! through the shape's list style, the placeholders of the layout and master,
//! the master's text styles, the shape style, table styles, the presentation's
//! default text style and the theme, and font edits written to the runs.
use super::word::Block;
use super::word_fonts::TablePlace;
use super::*;
use crate::fonts::{Color, ColorContext, FontEdit, RunFont, Theme};

/// The sizes PowerPoint gives text whose master defines no text styles:
/// titles 44 points, body text by level, notes 12 and other text 18 points.
const TITLE_POINTS: f64 = 44.0;
const BODY_POINTS: [f64; 9] = [32.0, 28.0, 24.0, 20.0, 20.0, 20.0, 20.0, 20.0, 20.0];
const NOTES_POINTS: f64 = 12.0;
const OTHER_POINTS: f64 = 18.0;
const LEVELS: usize = 9;

/// The run properties of the nine paragraph levels of a list style.
type Levels = Vec<RunFont>;

/// A placeholder of a layout or master: its type, index and list style.
struct Placeholder {
    kind: String,
    index: Option<String>,
    levels: Levels,
}

/// What the text of one slide or notes page inherits.
pub(super) struct SlideFonts {
    theme: Option<Theme>,
    colors: BTreeMap<String, String>,
    layout: Vec<Placeholder>,
    master: Vec<Placeholder>,
    title: Levels,
    body: Levels,
    other: Levels,
    defaults: Levels,
    /// Table style text by style ID and part (`wholeTbl`, `firstRow`, …).
    tables: BTreeMap<String, BTreeMap<String, RunFont>>,
    default_table: Option<String>,
    notes: bool,
}

fn presentation_child<'a, 'input>(node: Node<'a, 'input>, name: &str) -> Option<Node<'a, 'input>> {
    node.children()
        .find(|n| n.has_tag_name((PRESENTATION, name)))
}

fn drawing_child<'a, 'input>(node: Node<'a, 'input>, name: &str) -> Option<Node<'a, 'input>> {
    node.children().find(|n| n.has_tag_name((DRAWING, name)))
}

/// The part `part` relates to by a relationship whose type ends with `kind`.
fn related(parts: &BTreeMap<String, Vec<u8>>, part: &str, kind: &str) -> Result<Option<String>> {
    let (directory, filename) = part.rsplit_once('/').unwrap_or(("", part));
    let rels = format!("{directory}/_rels/{filename}.rels");
    let Some(bytes) = parts.get(&rels) else {
        return Ok(None);
    };
    let (text, _) = part_text(bytes)?;
    let xml = Document::parse(&text)?;
    let Some(rel) = xml.descendants().find(|n| {
        n.has_tag_name((PACKAGE_REL, "Relationship"))
            && n.attribute("Type").is_some_and(|t| t.ends_with(kind))
            && n.attribute("TargetMode") != Some("External")
    }) else {
        return Ok(None);
    };
    Ok(Some(relationship_target(parts, part, rel)?).filter(|target| parts.contains_key(target)))
}

/// The colour map override of a layout or slide.
fn color_override<'a, 'input>(doc: &'a Document<'input>) -> Option<Node<'a, 'input>> {
    presentation_child(doc.root_element(), "clrMapOvr")
        .and_then(|o| drawing_child(o, "overrideClrMapping"))
}

/// The placeholder (`p:ph`) of a shape, as (type, index). A placeholder
/// without a type holds content (`obj`).
fn placeholder(shape: Node<'_, '_>) -> Option<(String, Option<String>)> {
    let ph = shape
        .children()
        .find(|n| n.tag_name().name().starts_with("nv"))?
        .children()
        .find(|n| n.has_tag_name((PRESENTATION, "nvPr")))?
        .children()
        .find(|n| n.has_tag_name((PRESENTATION, "ph")))?;
    Some((
        ph.attribute("type").unwrap_or("obj").to_owned(),
        ph.attribute("idx").map(str::to_owned),
    ))
}

/// The colour map of a master (`p:clrMap`) with a layout's or slide's
/// override (`p:clrMapOvr/a:overrideClrMapping`) applied.
fn color_map(
    base: &BTreeMap<String, String>,
    node: Option<Node<'_, '_>>,
) -> BTreeMap<String, String> {
    let mut map = base.clone();
    if let Some(node) = node {
        for attribute in node.attributes() {
            map.insert(attribute.name().to_owned(), attribute.value().to_owned());
        }
    }
    map
}

fn levels(list: Option<Node<'_, '_>>, context: &ColorContext<'_>) -> Levels {
    (1..=LEVELS)
        .map(|level| crate::fonts::list_level(list, level, context))
        .collect()
}

/// The placeholders of a layout or master with their list styles.
fn placeholders(doc: &Document<'_>, context: &ColorContext<'_>) -> Vec<Placeholder> {
    doc.descendants()
        .filter(|n| n.has_tag_name((PRESENTATION, "sp")))
        .filter_map(|shape| {
            let (kind, index) = placeholder(shape)?;
            let list =
                presentation_child(shape, "txBody").and_then(|b| drawing_child(b, "lstStyle"));
            Some(Placeholder {
                kind,
                index,
                levels: levels(list, context),
            })
        })
        .collect()
}

/// The master placeholder type a layout or slide placeholder takes its
/// formatting from.
fn master_kind(kind: &str) -> &str {
    match kind {
        "title" | "ctrTitle" => "title",
        "dt" | "ftr" | "sldNum" | "hdr" => kind,
        _ => "body",
    }
}

fn find<'p>(list: &'p [Placeholder], kind: &str, index: Option<&str>) -> Option<&'p Placeholder> {
    index
        .and_then(|index| list.iter().find(|p| p.index.as_deref() == Some(index)))
        .or_else(|| list.iter().find(|p| p.kind == kind))
        .or_else(|| {
            list.iter()
                .find(|p| master_kind(&p.kind) == master_kind(kind))
        })
}

impl SlideFonts {
    /// What the text of `part`, a slide or notes page, inherits in `parts`.
    pub(super) fn new(
        parts: &BTreeMap<String, Vec<u8>>,
        part: &str,
        xml: &Document<'_>,
    ) -> Result<Self> {
        let notes = xml.root_element().has_tag_name((PRESENTATION, "notes"));
        let layout_part = if notes {
            None
        } else {
            related(parts, part, "/slideLayout")?
        };
        Self::with_layout(parts, part, layout_part, xml)
    }

    /// What the text of a slide made from `layout` inherits.
    pub(super) fn for_layout(
        parts: &BTreeMap<String, Vec<u8>>,
        layout: &str,
        xml: &Document<'_>,
    ) -> Result<Self> {
        Self::with_layout(parts, layout, Some(layout.to_owned()), xml)
    }

    fn with_layout(
        parts: &BTreeMap<String, Vec<u8>>,
        part: &str,
        layout_part: Option<String>,
        xml: &Document<'_>,
    ) -> Result<Self> {
        let notes = xml.root_element().has_tag_name((PRESENTATION, "notes"));
        let master_part = match &layout_part {
            Some(layout) => related(parts, layout, "/slideMaster")?,
            None if notes => related(parts, part, "/notesMaster")?,
            None => None,
        };
        let theme = master_part
            .as_ref()
            .map(|master| related(parts, master, "/theme"))
            .transpose()?
            .flatten()
            .map(|theme| Theme::parse(&xml_part(parts, &theme)?))
            .transpose()?;
        let master_text = master_part
            .as_ref()
            .map(|m| xml_part(parts, m))
            .transpose()?;
        let master_doc = master_text.as_deref().map(Document::parse).transpose()?;
        let layout_text = layout_part
            .as_ref()
            .map(|l| xml_part(parts, l))
            .transpose()?;
        let layout_doc = layout_text.as_deref().map(Document::parse).transpose()?;
        let mut colors = crate::fonts::default_color_map();
        if let Some(map) = master_doc
            .as_ref()
            .and_then(|doc| presentation_child(doc.root_element(), "clrMap"))
        {
            colors = color_map(&colors, Some(map));
        }
        if let Some(layout) = &layout_doc {
            colors = color_map(&colors, color_override(layout));
        }
        colors = color_map(&colors, color_override(xml));
        let context = ColorContext {
            theme: theme.as_ref(),
            map: &colors,
            placeholder: None,
        };
        let styles = master_doc
            .as_ref()
            .and_then(|doc| presentation_child(doc.root_element(), "txStyles"));
        let style = |name: &str| levels(styles.and_then(|s| presentation_child(s, name)), &context);
        let (title, body, other) = if notes {
            let notes_style = master_doc
                .as_ref()
                .and_then(|doc| presentation_child(doc.root_element(), "notesStyle"));
            (
                levels(None, &context),
                levels(notes_style, &context),
                levels(None, &context),
            )
        } else {
            (style("titleStyle"), style("bodyStyle"), style("otherStyle"))
        };
        let presentation = parts
            .get("ppt/presentation.xml")
            .map(|bytes| part_text(bytes).map(|(text, _)| text.into_owned()))
            .transpose()?;
        let presentation_doc = presentation.as_deref().map(Document::parse).transpose()?;
        let defaults = levels(
            presentation_doc
                .as_ref()
                .and_then(|doc| presentation_child(doc.root_element(), "defaultTextStyle")),
            &context,
        );
        let mut tables = BTreeMap::new();
        let mut default_table = None;
        if let Some(bytes) = parts.get("ppt/tableStyles.xml") {
            let (text, _) = part_text(bytes)?;
            let doc = Document::parse(&text)?;
            default_table = doc.root_element().attribute("def").map(str::to_owned);
            for style in doc
                .root_element()
                .children()
                .filter(|n| n.has_tag_name((DRAWING, "tblStyle")))
            {
                let Some(id) = style.attribute("styleId") else {
                    continue;
                };
                let mut by_part = BTreeMap::new();
                for part in style.children().filter(Node::is_element) {
                    let Some(text_style) = drawing_child(part, "tcTxStyle") else {
                        continue;
                    };
                    let (mut font, _) = crate::fonts::style_font(text_style, &context);
                    if let Some(color) = text_style
                        .children()
                        .filter(Node::is_element)
                        .find(|n| n.tag_name().name() != "fontRef" && n.tag_name().name() != "font")
                        .and_then(|c| crate::fonts::drawing_color(c, &context))
                    {
                        font.color = Some(color);
                    }
                    by_part.insert(part.tag_name().name().to_owned(), font);
                }
                tables.insert(id.to_owned(), by_part);
            }
        }
        Ok(Self {
            layout: layout_doc
                .as_ref()
                .map(|doc| placeholders(doc, &context))
                .unwrap_or_default(),
            master: master_doc
                .as_ref()
                .map(|doc| placeholders(doc, &context))
                .unwrap_or_default(),
            theme,
            colors,
            title,
            body,
            other,
            defaults,
            tables,
            default_table,
            notes,
        })
    }

    /// How colours of the slide resolve: its theme and colour map.
    pub(super) fn color_context(&self) -> ColorContext<'_> {
        self.context()
    }

    fn context(&self) -> ColorContext<'_> {
        ColorContext {
            theme: self.theme.as_ref(),
            map: &self.colors,
            placeholder: None,
        }
    }

    /// The last resort, PowerPoint's own text: titles in the theme's heading
    /// font and other text in its body font, in the text colour, at the size
    /// PowerPoint gives text of the role (`title`, `body` or other) and level.
    fn application(&self, role: &str, level: usize) -> RunFont {
        let title = role == "title";
        let mut font = self
            .theme
            .as_ref()
            .map(|t| t.font(title))
            .unwrap_or_default();
        font.size = Some(match role {
            "title" => TITLE_POINTS,
            "body" if self.notes => NOTES_POINTS,
            "body" => BODY_POINTS[(level - 1).min(BODY_POINTS.len() - 1)],
            _ => OTHER_POINTS,
        });
        let slot = self.colors.get("tx1").map_or("dk1", String::as_str);
        font.color = self
            .theme
            .as_ref()
            .and_then(|t| t.color(slot))
            .map(|rgb| Color::Rgb {
                rgb: rgb.to_owned(),
                theme: Some(slot.to_owned()),
            });
        font
    }

    /// What a run in a paragraph of `level` (1–9) inherits from outside its
    /// own shape's list style.
    fn inherited(
        &self,
        shape: Option<Node<'_, '_>>,
        cell: Option<(Node<'_, '_>, &TablePlace)>,
        level: usize,
    ) -> RunFont {
        let at = |levels: &Levels| levels.get(level - 1).cloned().unwrap_or_default();
        let mut font = RunFont::default();
        if let Some((table, place)) = cell {
            let properties = drawing_child(table, "tblPr");
            let id = properties
                .and_then(|p| drawing_child(p, "tableStyleId"))
                .and_then(|n| n.text())
                .map(str::trim)
                .or(self.default_table.as_deref());
            if let Some(style) = id.and_then(|id| self.tables.get(id)) {
                let flag = |name: &str| {
                    properties.is_some_and(|p| matches!(p.attribute(name), Some("1" | "true")))
                };
                for (part, applies) in [
                    ("firstRow", place.first_row && flag("firstRow")),
                    ("lastRow", place.last_row && flag("lastRow")),
                    ("firstCol", place.first_column && flag("firstCol")),
                    ("lastCol", place.last_column && flag("lastCol")),
                    ("wholeTbl", true),
                ] {
                    if applies && let Some(own) = style.get(part) {
                        font = font.or(own);
                    }
                }
            }
            return font
                .or(&at(&self.defaults))
                .or(&self.application("other", level));
        }
        match shape.and_then(placeholder) {
            Some((kind, index)) => {
                if let Some(own) = find(&self.layout, &kind, index.as_deref()) {
                    font = font.or(&at(&own.levels));
                }
                if let Some(own) = find(&self.master, master_kind(&kind), None) {
                    font = font.or(&at(&own.levels));
                }
                let style = match master_kind(&kind) {
                    "title" => &self.title,
                    "body" => &self.body,
                    _ => &self.other,
                };
                font.or(&at(style))
                    .or(&self.application(master_kind(&kind), level))
            }
            None => {
                let context = self.context();
                let (style, _) = shape
                    .and_then(|s| presentation_child(s, "style"))
                    .map(|s| crate::fonts::style_font(s, &context))
                    .unwrap_or_default();
                font.or(&style)
                    .or(&at(&self.defaults))
                    .or(&self.application("other", level))
            }
        }
    }

    /// Whether the style of `table` is one the file defines, or none.
    fn table_style_known(&self, table: Node<'_, '_>) -> bool {
        drawing_child(table, "tblPr")
            .and_then(|p| drawing_child(p, "tableStyleId"))
            .and_then(|n| n.text())
            .map(str::trim)
            .or(self.default_table.as_deref())
            .is_none_or(|id| self.tables.contains_key(id))
    }

    /// The font of a block of slide text from the runs that show it; an empty
    /// placeholder shows the font of its paragraph end.
    pub(super) fn block_font(&self, block: &Block<'_, '_>) -> Value {
        self.runs_font(&block.runs, block.place.as_ref(), block.empty)
    }

    /// The font of `runs`, in a table cell at `place`, or of the empty
    /// placeholder paragraph `empty`.
    pub(super) fn runs_font(
        &self,
        runs: &[Node<'_, '_>],
        place: Option<&TablePlace>,
        empty: Option<Node<'_, '_>>,
    ) -> Value {
        let context = self.context();
        let ends: Vec<Node<'_, '_>> = empty
            .into_iter()
            .map(|paragraph| drawing_child(paragraph, "endParaRPr").unwrap_or(paragraph))
            .collect();
        let runs: Vec<_> = runs
            .iter()
            .chain(&ends)
            .map(|run| {
                let paragraph = run.ancestors().find(|n| n.has_tag_name((DRAWING, "p")));
                let level = paragraph.map_or(1, crate::fonts::paragraph_level);
                let body = paragraph.and_then(|p| p.parent());
                let own_list = body
                    .and_then(|b| drawing_child(b, "lstStyle"))
                    .map(|list| crate::fonts::list_level(Some(list), level, &context))
                    .unwrap_or_default();
                let cell = run.ancestors().find(|n| n.has_tag_name((DRAWING, "tc")));
                let table = cell
                    .and_then(|c| c.ancestors().find(|n| n.has_tag_name((DRAWING, "tbl"))))
                    .zip(place);
                let shape = run
                    .ancestors()
                    .find(|n| n.has_tag_name((PRESENTATION, "sp")));
                let mut inherited = self.inherited(shape, table, level);
                // A built-in table style the file does not define colours its
                // text in a way only PowerPoint knows.
                if table.is_some_and(|(table, _)| !self.table_style_known(table)) {
                    inherited.color = None;
                }
                let own = if run.has_tag_name((DRAWING, "endParaRPr")) {
                    Some(*run)
                } else {
                    drawing_child(*run, "rPr")
                };
                own.map(|p| crate::fonts::drawing_run(p, &context))
                    .unwrap_or_default()
                    .or(&own_list)
                    .or(&inherited)
            })
            .collect();
        crate::fonts::element_font(&runs, &RunFont::default())
    }
}

/// The edits giving the runs of `block` (and their copies for older readers)
/// the font of `edit`, with the ends of the paragraphs they belong to.
pub(super) fn run_edits<'a, 'input>(
    source: &str,
    block: &Block<'a, 'input>,
    edit: &FontEdit,
    cache: &mut BTreeMap<(usize, bool), BranchTexts<'a, 'input>>,
) -> Result<Vec<(std::ops::Range<usize>, String)>> {
    let mut runs: Vec<Node<'a, 'input>> = block.runs.clone();
    for run in &block.runs {
        if let Some(text) = drawing_child(*run, "t") {
            for copy in hidden_copies(text, "pptx", cache)? {
                runs.extend(copy.parent());
            }
        }
    }
    let mut seen = BTreeSet::new();
    let mut paragraphs = BTreeSet::new();
    let mut edits = vec![];
    // An empty placeholder keeps its font in the paragraph end, which the
    // text written into it takes.
    if let Some(paragraph) = block.empty {
        let raw = &source[paragraph.range()];
        let (name, prefix) = {
            let name = crate::fonts::qualified_name(source, paragraph);
            (name.to_owned(), crate::fonts::prefix_of(name).to_owned())
        };
        match drawing_child(paragraph, "endParaRPr") {
            Some(end) => edits.push((
                end.range(),
                crate::fonts::edited_drawing_run(source, Some(end), "endParaRPr", &prefix, edit)?,
            )),
            None => {
                let end =
                    crate::fonts::edited_drawing_run(source, None, "endParaRPr", &prefix, edit)?;
                if let Some(opening) = raw.strip_suffix("/>") {
                    edits.push((
                        paragraph.range(),
                        format!("{}>{end}</{name}>", opening.trim_end()),
                    ));
                } else {
                    let at =
                        paragraph.range().start + raw.rfind("</").context("invalid paragraph")?;
                    edits.push((at..at, end));
                }
            }
        }
    }
    for run in runs {
        if !seen.insert(run.range().start) {
            continue;
        }
        let prefix = crate::fonts::prefix_of(crate::fonts::qualified_name(source, run)).to_owned();
        match drawing_child(run, "rPr") {
            Some(properties) => edits.push((
                properties.range(),
                crate::fonts::edited_drawing_run(source, Some(properties), "rPr", &prefix, edit)?,
            )),
            None => {
                // Run properties come first in a run.
                let raw = &source[run.range()];
                let at = run.range().start + raw.find('>').context("invalid run")? + 1;
                edits.push((
                    at..at,
                    crate::fonts::edited_drawing_run(source, None, "rPr", &prefix, edit)?,
                ));
            }
        }
        let Some(paragraph) = run.ancestors().find(|n| n.has_tag_name((DRAWING, "p"))) else {
            continue;
        };
        if !paragraphs.insert(paragraph.range().start) {
            continue;
        }
        // Line breaks carry a font of their own, which PowerPoint counts.
        for line_break in paragraph
            .children()
            .filter(|n| n.has_tag_name((DRAWING, "br")))
        {
            let prefix = crate::fonts::prefix_of(crate::fonts::qualified_name(source, line_break))
                .to_owned();
            match drawing_child(line_break, "rPr") {
                Some(properties) => edits.push((
                    properties.range(),
                    crate::fonts::edited_drawing_run(
                        source,
                        Some(properties),
                        "rPr",
                        &prefix,
                        edit,
                    )?,
                )),
                None => {
                    let raw = &source[line_break.range()];
                    let opening = raw.find('>').context("invalid line break")?;
                    let created =
                        crate::fonts::edited_drawing_run(source, None, "rPr", &prefix, edit)?;
                    if raw[..opening].ends_with('/') {
                        let name = crate::fonts::qualified_name(source, line_break);
                        edits.push((
                            line_break.range(),
                            format!(
                                "{}>{created}</{name}>",
                                raw[..opening].trim_end_matches('/')
                            ),
                        ));
                    } else {
                        let at = line_break.range().start + opening + 1;
                        edits.push((at..at, created));
                    }
                }
            }
        }
        if let Some(end) = drawing_child(paragraph, "endParaRPr") {
            let prefix =
                crate::fonts::prefix_of(crate::fonts::qualified_name(source, end)).to_owned();
            edits.push((
                end.range(),
                crate::fonts::edited_drawing_run(source, Some(end), "endParaRPr", &prefix, edit)?,
            ));
        }
    }
    Ok(edits)
}
