//! The fonts of cells and shape text as Excel shows them, and font edits.
use super::*;
use crate::fonts::{Color, ColorContext, FontEdit, RunFont, Theme, tinted};

/// Excel's default colour palette (`indexed` 0–63); 64 is the automatic
/// (system) text colour.
const INDEXED: [&str; 64] = [
    "000000", "FFFFFF", "FF0000", "00FF00", "0000FF", "FFFF00", "FF00FF", "00FFFF", "000000",
    "FFFFFF", "FF0000", "00FF00", "0000FF", "FFFF00", "FF00FF", "00FFFF", "800000", "008000",
    "000080", "808000", "800080", "008080", "C0C0C0", "808080", "9999FF", "993366", "FFFFCC",
    "CCFFFF", "660066", "FF8080", "0066CC", "CCCCFF", "000080", "FF00FF", "FFFF00", "00FFFF",
    "800080", "800000", "008080", "0000FF", "00CCFF", "CCFFFF", "CCFFCC", "FFFF99", "99CCFF",
    "FF99CC", "CC99FF", "FFCC99", "3366FF", "33CCCC", "99CC00", "FFCC00", "FF9900", "FF6600",
    "666699", "969696", "003366", "339966", "003300", "333300", "993300", "993366", "333399",
    "333333",
];

/// Theme slots by the index Excel gives them in `theme="n"`: light and dark
/// come in the opposite order to the theme part.
const THEME_SLOTS: [&str; 12] = [
    "lt1", "dk1", "lt2", "dk2", "accent1", "accent2", "accent3", "accent4", "accent5", "accent6",
    "hlink", "folHlink",
];

/// Excel shape text without size or colour of its own is 11 points in the
/// theme's text colour.
const SHAPE_TEXT_POINTS: f64 = 11.0;

/// The theme of a workbook, from the part its workbook relationships name.
pub(super) fn workbook_theme(parts: &BTreeMap<String, Vec<u8>>) -> Result<Option<Theme>> {
    let Some(rels) = parts.get("xl/_rels/workbook.xml.rels") else {
        return Ok(None);
    };
    let doc = xml(rels)?;
    let Some(target) = doc
        .root_element()
        .children()
        .filter(|n| n.has_tag_name((PKG_REL, "Relationship")))
        .find(|n| {
            n.attribute("Type").is_some_and(|t| {
                t.ends_with("/theme") && n.attribute("TargetMode") != Some("External")
            })
        })
        .and_then(|n| n.attribute("Target"))
    else {
        return Ok(None);
    };
    let part = relationship_target("xl/workbook.xml", target)?;
    parts
        .get(&part)
        .map(|bytes| Theme::parse(std::str::from_utf8(bytes)?))
        .transpose()
}

/// How the colours of a workbook's fonts resolve: its theme and palette.
pub(super) struct Palette {
    pub(super) theme: Option<Theme>,
    indexed: Vec<String>,
}

impl Palette {
    pub(super) fn new(theme: Option<Theme>, styles: Option<&Document<'_>>) -> Self {
        let custom: Vec<String> = styles
            .and_then(|doc| child(doc.root_element(), "colors"))
            .and_then(|colors| child(colors, "indexedColors"))
            .map(|list| {
                list.children()
                    .filter(|n| n.has_tag_name((NS, "rgbColor")))
                    .filter_map(|n| n.attribute("rgb"))
                    .map(|argb| argb[argb.len().saturating_sub(6)..].to_ascii_uppercase())
                    .collect()
            })
            .unwrap_or_default();
        Self {
            theme,
            indexed: if custom.is_empty() {
                INDEXED.iter().map(|c| (*c).to_owned()).collect()
            } else {
                custom
            },
        }
    }

    /// An Excel colour element (`color`) as the colour it shows.
    fn color(&self, node: Node<'_, '_>) -> Option<Color> {
        if matches!(node.attribute("auto"), Some("1" | "true")) {
            return Some(Color::Auto);
        }
        let tint = node
            .attribute("tint")
            .and_then(|t| t.parse::<f64>().ok())
            .unwrap_or(0.0);
        if let Some(argb) = node.attribute("rgb") {
            let rgb = &argb[argb.len().saturating_sub(6)..];
            return Some(Color::rgb(tinted(rgb, tint)));
        }
        if let Some(index) = node
            .attribute("theme")
            .and_then(|t| t.parse::<usize>().ok())
        {
            let slot = THEME_SLOTS.get(index)?;
            let rgb = self.theme.as_ref()?.color(slot)?;
            return Some(Color::Rgb {
                rgb: tinted(rgb, tint),
                theme: Some((*slot).to_owned()),
            });
        }
        if let Some(index) = node
            .attribute("indexed")
            .and_then(|t| t.parse::<usize>().ok())
        {
            return match index {
                64 => Some(Color::Auto),
                index => self
                    .indexed
                    .get(index)
                    .map(|rgb| Color::rgb(tinted(rgb, tint))),
            };
        }
        None
    }

    /// The properties a cell font (`font`) or rich text run (`rPr`, whose
    /// name element is `rFont`) sets. A cell font without a colour is automatic.
    pub(super) fn font(&self, node: Node<'_, '_>, run: bool) -> RunFont {
        let name = child(node, if run { "rFont" } else { "name" })
            .and_then(|n| n.attribute("val"))
            .filter(|n| !n.is_empty())
            .map(str::to_owned);
        // A theme font (`scheme`) shows the theme's Latin and East Asian
        // typefaces, whatever name it records; other fonts have one name.
        let scheme = child(node, "scheme")
            .and_then(|n| n.attribute("val"))
            .filter(|v| matches!(*v, "major" | "minor"));
        let (latin, east_asian) = match (scheme, &self.theme) {
            (Some(scheme), Some(theme)) => {
                let font = theme.font(scheme == "major");
                (
                    font.latin.or_else(|| name.clone()),
                    font.east_asian.or_else(|| name.clone()),
                )
            }
            _ => (name.clone(), name),
        };
        RunFont {
            latin,
            east_asian,
            size: child(node, "sz")
                .and_then(|n| n.attribute("val"))
                .and_then(|v| v.parse().ok()),
            color: match child(node, "color") {
                Some(color) => self.color(color),
                None if !run => Some(Color::Auto),
                None => None,
            },
        }
    }

    /// The fonts of rich text runs (`r` in `si` or `is`) with text, as they set
    /// them: what a run leaves unset comes from its cell.
    pub(super) fn runs(&self, container: Node<'_, '_>) -> Vec<RunFont> {
        container
            .children()
            .filter(|n| n.has_tag_name((NS, "r")))
            .filter(|run| {
                child(*run, "t")
                    .and_then(|t| t.text())
                    .is_some_and(|t| !t.is_empty())
            })
            .map(|run| {
                child(run, "rPr")
                    .map(|p| self.font(p, true))
                    .unwrap_or_default()
            })
            .collect()
    }
}

/// The fonts of the text of an Excel shape (`xdr:sp`) and the font it shows
/// without runs: run properties, the shape's list style, its style's font
/// reference, then Excel's 11-point text in the theme text colour.
pub(super) fn shape_runs(shape: Node<'_, '_>, theme: Option<&Theme>) -> (Vec<RunFont>, RunFont) {
    let map = crate::fonts::default_color_map();
    let base_context = ColorContext {
        theme,
        map: &map,
        placeholder: None,
    };
    let (style, placeholder) = shape
        .children()
        .find(|n| n.has_tag_name((XDR, "style")))
        .map(|style| crate::fonts::style_font(style, &base_context))
        .unwrap_or_default();
    let context = ColorContext {
        theme,
        map: &map,
        placeholder: placeholder.as_ref(),
    };
    let application = RunFont {
        size: Some(SHAPE_TEXT_POINTS),
        color: theme.and_then(|t| t.color("dk1")).map(|rgb| Color::Rgb {
            rgb: rgb.to_owned(),
            theme: Some("dk1".into()),
        }),
        ..theme.map(|t| t.font(false)).unwrap_or_default()
    };
    let Some(body) = shape.children().find(|n| n.has_tag_name((XDR, "txBody"))) else {
        return (vec![], style.or(&application));
    };
    let list = body
        .children()
        .find(|n| n.has_tag_name((DRAWING, "lstStyle")));
    let inherited = |level: usize| {
        crate::fonts::list_level(list, level, &context)
            .or(&style)
            .or(&application)
    };
    let mut runs = vec![];
    let mut empty = None;
    for paragraph in body.children().filter(|n| n.has_tag_name((DRAWING, "p"))) {
        let (own, end) = crate::fonts::paragraph_runs(paragraph, &inherited, &context);
        runs.extend(own);
        empty.get_or_insert(end);
    }
    (runs, empty.unwrap_or_else(|| inherited(1)))
}

impl Workbook {
    /// Writes the edits of [`Workbook::patch_with_operations_and_assets`] after
    /// the original cells and shapes of `fonts` are given their new fonts, so
    /// that the fonts move with row and column operations as the cells do.
    pub fn patch_with_fonts(
        &self,
        destination: &Path,
        operations: &[Value],
        changes: &[Value],
        formulas: &[Value],
        fonts: &[Value],
        assets: &BTreeMap<String, Vec<u8>>,
    ) -> Result<Value> {
        if fonts.is_empty() {
            return self.patch_with_operations_and_assets(
                destination,
                operations,
                changes,
                formulas,
                assets,
            );
        }
        ensure!(
            !self
                .parts
                .keys()
                .any(|n| n.to_lowercase().starts_with("_xmlsignatures/")),
            "signed Excel cannot be modified"
        );
        let (edited, font_parts) = self.with_fonts(fonts)?;
        let mut report = edited.patch_with_operations_and_assets(
            destination,
            operations,
            changes,
            formulas,
            assets,
        )?;
        let mut parts: BTreeSet<String> = report["changed_parts"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| p.as_str().map(str::to_owned))
            .collect();
        parts.extend(font_parts);
        report["changed_parts"] = json!(parts);
        report["font_changes_verified"] = json!(true);
        Ok(report)
    }

    /// The workbook with `fonts` (each a `sheet` with a `cell` or `shape` and
    /// the font `after` sets) written: a cell gets a format with the new font,
    /// and the runs of its rich text and of shape text get the new properties.
    fn with_fonts(&self, fonts: &[Value]) -> Result<(Workbook, Vec<String>)> {
        let mut cell_edits: BTreeMap<String, BTreeMap<String, FontEdit>> = BTreeMap::new();
        let mut shape_edits: BTreeMap<String, BTreeMap<String, FontEdit>> = BTreeMap::new();
        for change in fonts {
            let sheet = self
                .sheets
                .iter()
                .find(|s| s["name"] == change["sheet"])
                .with_context(|| format!("font edit sheet {} is missing", change["sheet"]))?;
            let edit = FontEdit::parse(&change["after"])?;
            ensure!(
                edit.size.is_none_or(|size| size <= 409.0),
                "Excel font size must be 1 to 409 points"
            );
            if let Some(shape) = change["shape"].as_str() {
                let (part, id) = shape
                    .rsplit_once('#')
                    .context("shape ID without drawing part")?;
                ensure!(
                    shape_edits
                        .entry(part.to_owned())
                        .or_default()
                        .insert(id.to_owned(), edit)
                        .is_none(),
                    "duplicate font edit of shape {shape}"
                );
            } else {
                let cell = string(&change["cell"])?;
                // A cell has one font name; shape text has Latin and East Asian fonts.
                crate::fonts::excel_name(&edit)?;
                ensure!(
                    cell_edits
                        .entry(string(&sheet["part"])?.to_owned())
                        .or_default()
                        .insert(cell.to_owned(), edit)
                        .is_none(),
                    "duplicate font edit of {}!{cell}",
                    change["sheet"]
                );
            }
        }
        let mut patched = BTreeMap::new();
        if !cell_edits.is_empty() {
            let styles = std::str::from_utf8(
                self.parts
                    .get("xl/styles.xml")
                    .context("the workbook has no styles part, so ARP cannot set cell fonts")?,
            )?;
            let mut formats = StyleFormats::new(styles)?;
            let shared = self
                .parts
                .get("xl/sharedStrings.xml")
                .map(|bytes| std::str::from_utf8(bytes))
                .transpose()?;
            let shared_doc = shared.map(Document::parse).transpose()?;
            let shared_items: Vec<Node<'_, '_>> = shared_doc
                .as_ref()
                .map(|doc| {
                    doc.root_element()
                        .children()
                        .filter(|n| n.has_tag_name((NS, "si")))
                        .collect()
                })
                .unwrap_or_default();
            for (part, edits) in &cell_edits {
                let source = std::str::from_utf8(&self.parts[part])?;
                let doc = Document::parse(source)?;
                let mut splices = vec![];
                let mut seen = BTreeSet::new();
                for cell in doc.descendants().filter(|n| {
                    n.has_tag_name((NS, "c"))
                        && n.parent().is_some_and(|p| p.has_tag_name((NS, "row")))
                }) {
                    let address = cell.attribute("r").context("missing cell address")?;
                    let Some(edit) = edits.get(address) else {
                        continue;
                    };
                    seen.insert(address);
                    let format: usize = cell.attribute("s").unwrap_or("0").parse()?;
                    let style = formats.with_font(format, edit)?;
                    splices.push((
                        cell.range(),
                        rewritten_cell(source, cell, &style, edit, shared, &shared_items)?,
                    ));
                }
                if let Some(missing) = edits.keys().find(|a| !seen.contains(a.as_str())) {
                    bail!("font edit target {part}!{missing} has no cell element");
                }
                patched.insert(
                    part.clone(),
                    splice(source, splices, "cell font")?.into_bytes(),
                );
            }
            patched.insert("xl/styles.xml".to_owned(), formats.finish()?.into_bytes());
        }
        for (part, edits) in &shape_edits {
            let drawing = std::str::from_utf8(
                self.parts
                    .get(part)
                    .with_context(|| format!("drawing part {part} is missing"))?,
            )?;
            let edited = crate::document_source::edit_shape_fonts(drawing, edits)?;
            patched.insert(part.clone(), edited.into_bytes());
        }
        let raw = archive_bytes(&self.raw, &patched)?;
        let written = Workbook::from_bytes(raw)?;
        ensure!(
            written.parts.len() == self.parts.len()
                && self
                    .parts
                    .iter()
                    .all(|(part, bytes)| written.parts.get(part)
                        == Some(patched.get(part).unwrap_or(bytes))),
            "Excel package preservation failed on font edit"
        );
        self.ensure_fonts_written(&written, fonts)?;
        Ok((written, patched.into_keys().collect()))
    }

    /// Checks that `written` shows each font of `fonts` as edited and every
    /// other cell and shape as before.
    fn ensure_fonts_written(&self, written: &Workbook, fonts: &[Value]) -> Result<()> {
        let before = self.sheet_values();
        let after = written.sheet_values();
        let mut expected: BTreeMap<(String, String), FontEdit> = BTreeMap::new();
        for change in fonts {
            let target = change["shape"]
                .as_str()
                .or(change["cell"].as_str())
                .context("font edit target")?;
            expected.insert(
                (string(&change["sheet"])?.to_owned(), target.to_owned()),
                FontEdit::parse(&change["after"])?,
            );
        }
        for (old, new) in before.iter().zip(&after) {
            let sheet = string(&old["name"])?.to_owned();
            let mut new_cells: BTreeMap<&str, &Value> = BTreeMap::new();
            for cell in array(&new["cells"])? {
                new_cells.insert(string(&cell["address"])?, cell);
            }
            for cell in array(&old["cells"])? {
                let address = string(&cell["address"])?;
                let found = new_cells
                    .get(address)
                    .context("cell disappeared on font edit")?;
                let mut wanted = cell.clone();
                if let Some(edit) = expected.get(&(sheet.clone(), address.to_owned())) {
                    wanted["font"] = edit.applied(&cell["font"]);
                }
                ensure!(
                    crate::fonts::same_font(&found["font"], &wanted["font"])
                        && found["value"] == cell["value"]
                        && found["formula"] == cell["formula"]
                        && found["number_format"] == cell["number_format"]
                        && found["style"] == cell["style"],
                    "Excel font read-back failed for {sheet}!{address}: expected {}, found {}",
                    wanted["font"],
                    found["font"]
                );
            }
            for (old_drawing, new_drawing) in array(&old["drawings"])?
                .iter()
                .zip(array(&new["drawings"])?)
            {
                let id = string(&old_drawing["id"])?;
                let mut wanted = old_drawing.clone();
                if let Some(edit) = expected.get(&(sheet.clone(), id.to_owned())) {
                    wanted["font"] = edit.applied(&old_drawing["font"]);
                }
                ensure!(
                    new_drawing["text"] == old_drawing["text"]
                        && (wanted["font"].is_null() == new_drawing["font"].is_null())
                        && (wanted["font"].is_null()
                            || crate::fonts::same_font(&new_drawing["font"], &wanted["font"])),
                    "Excel font read-back failed for shape {id}: expected {}, found {}",
                    wanted["font"],
                    new_drawing["font"]
                );
            }
        }
        Ok(())
    }
}

/// The cell formats and fonts of a styles part, with formats added for new
/// fonts. A format or font identical to an existing one is reused.
struct StyleFormats<'a> {
    source: &'a str,
    fonts: Vec<String>,
    formats: Vec<String>,
    font_count: usize,
    format_count: usize,
    /// (font index, edit) to the index of the edited font.
    edited_fonts: BTreeMap<(usize, String), usize>,
}

impl<'a> StyleFormats<'a> {
    fn new(source: &'a str) -> Result<Self> {
        let doc = Document::parse(source)?;
        let list = |name: &str| -> Result<Vec<String>> {
            let parent = child(doc.root_element(), name)
                .with_context(|| format!("styles part has no {name}"))?;
            Ok(parent
                .children()
                .filter(Node::is_element)
                .map(|n| source[n.range()].to_owned())
                .collect())
        };
        let fonts = list("fonts")?;
        let formats = list("cellXfs")?;
        Ok(Self {
            source,
            font_count: fonts.len(),
            format_count: formats.len(),
            fonts,
            formats,
            edited_fonts: BTreeMap::new(),
        })
    }

    /// The index of a format like `format` with its font edited by `edit`.
    fn with_font(&mut self, format: usize, edit: &FontEdit) -> Result<String> {
        let xf = self
            .formats
            .get(format)
            .with_context(|| format!("cell format {format} is missing"))?
            .clone();
        let (opening, rest) = xml_opening(&xf)?;
        let font: usize = xml_attribute_value(opening, "fontId")?
            .unwrap_or("0")
            .parse()?;
        let key = (font, format!("{edit:?}"));
        let new_font = match self.edited_fonts.get(&key) {
            Some(index) => *index,
            None => {
                let original = self
                    .fonts
                    .get(font)
                    .with_context(|| format!("font {font} is missing"))?;
                let wrapped = with_namespace(original);
                let doc = Document::parse(&wrapped)?;
                let element = doc
                    .root_element()
                    .first_element_child()
                    .context("font element")?;
                let edited = crate::fonts::edited_excel_font(&wrapped, element, "name", edit)?;
                let index = match self.fonts.iter().position(|f| *f == edited) {
                    Some(index) => index,
                    None => {
                        self.fonts.push(edited);
                        self.fonts.len() - 1
                    }
                };
                self.edited_fonts.insert(key, index);
                index
            }
        };
        let mut opening = set_xml_attribute(opening, "fontId", &new_font.to_string())?;
        opening = set_xml_attribute(&opening, "applyFont", "1")?;
        let edited = format!("{opening}{rest}");
        Ok(match self.formats.iter().position(|f| *f == edited) {
            Some(index) => index,
            None => {
                self.formats.push(edited);
                self.formats.len() - 1
            }
        }
        .to_string())
    }

    /// The styles part with the added fonts and formats.
    fn finish(self) -> Result<String> {
        let doc = Document::parse(self.source)?;
        let mut edits = vec![];
        for (name, items, original) in [
            ("fonts", &self.fonts, self.font_count),
            ("cellXfs", &self.formats, self.format_count),
        ] {
            if items.len() == original {
                continue;
            }
            let parent = child(doc.root_element(), name).context("styles list")?;
            let raw = &self.source[parent.range()];
            let (opening, _) = xml_opening(raw)?;
            let close = raw.rfind("</").context("styles list has no closing tag")?;
            let count = set_xml_attribute(opening, "count", &items.len().to_string())?;
            let added: String = items[original..].concat();
            edits.push((
                parent.range(),
                format!(
                    "{count}{}{added}{}",
                    &raw[opening.len()..close],
                    &raw[close..]
                ),
            ));
        }
        let result = splice(self.source, edits, "styles")?;
        Document::parse(&result)?;
        Ok(result)
    }
}

/// A styles child element wrapped so that it parses on its own: the
/// spreadsheet namespace is declared for unprefixed and `x:` names.
fn with_namespace(element: &str) -> String {
    format!(r#"<wrap xmlns="{NS}" xmlns:x="{NS}">{element}</wrap>"#)
}

/// A cell with format `style` and the runs of its rich text edited: shared
/// rich text is copied into the cell, as text edits do, so other cells using
/// it keep their fonts.
fn rewritten_cell(
    source: &str,
    cell: Node<'_, '_>,
    style: &str,
    edit: &FontEdit,
    shared: Option<&str>,
    shared_items: &[Node<'_, '_>],
) -> Result<String> {
    let raw = &source[cell.range()];
    let (opening, rest) = xml_opening(raw)?;
    let opening = set_xml_attribute(opening, "s", style)?;
    let (container, container_source) = match cell.attribute("t") {
        Some("s") => {
            let index: usize = child(cell, "v")
                .and_then(|n| n.text())
                .context("missing shared string index")?
                .parse()?;
            (
                shared_items
                    .get(index)
                    .copied()
                    .context("invalid shared string index")?,
                shared.context("missing shared strings")?,
            )
        }
        Some("inlineStr") => match child(cell, "is") {
            Some(container) => (container, source),
            None => return Ok(format!("{opening}{rest}")),
        },
        _ => return Ok(format!("{opening}{rest}")),
    };
    if !container.children().any(|n| n.has_tag_name((NS, "r"))) {
        return Ok(format!("{opening}{rest}"));
    }
    let edited_container = edited_runs(container_source, container, edit)?;
    if cell.attribute("t") == Some("inlineStr") {
        let start = container.range().start - cell.range().start;
        let end = container.range().end - cell.range().start;
        return Ok(format!(
            "{opening}{}{edited_container}{}",
            &raw[opening_length(raw)?..start],
            &raw[end..]
        ));
    }
    let start = edited_container
        .find('>')
        .context("invalid rich text container")?
        + 1;
    let close = edited_container.rfind("</").unwrap_or(start);
    let content = &edited_container[start..close];
    // Shared rich text becomes the cell's own inline text with the same runs.
    let namespaces: String = container
        .namespaces()
        .filter(|ns| ns.name() != Some("xml"))
        .map(|ns| match ns.name() {
            Some(name) => format!(" xmlns:{name}=\"{}\"", xml_attr(ns.uri())),
            None => String::new(),
        })
        .collect();
    let tag = element_tag(raw)?;
    let opening = remove_xml_attribute(&opening, "t")?;
    let opening = opening.trim_end_matches('>').trim_end_matches('/');
    let retained: String = cell
        .children()
        .filter(Node::is_element)
        .filter(|node| !node.has_tag_name((NS, "v")) && !node.has_tag_name((NS, "is")))
        .map(|node| &source[node.range()])
        .collect();
    Ok(format!(
        "{opening} t=\"inlineStr\"><is xmlns=\"{NS}\"{namespaces}>{content}</is>{retained}</{tag}>"
    ))
}

/// The length of the opening tag at the start of `raw`.
fn opening_length(raw: &str) -> Result<usize> {
    Ok(xml_opening(raw)?.0.len())
}

/// A rich text container (`si` or `is`) with the properties of each run edited.
fn edited_runs(source: &str, container: Node<'_, '_>, edit: &FontEdit) -> Result<String> {
    let base = container.range().start;
    let mut edits = vec![];
    for run in container.children().filter(|n| n.has_tag_name((NS, "r"))) {
        if let Some(properties) = child(run, "rPr") {
            let range = properties.range();
            edits.push((
                range.start - base..range.end - base,
                crate::fonts::edited_excel_font(source, properties, "rFont", edit)?,
            ));
        }
    }
    splice(&source[container.range()], edits, "rich text font")
}
