//! The fonts of cells and shape text as Excel shows them, and font edits.
use super::*;
use crate::fonts::{Color, ColorContext, Emphasis, FontEdit, RunFont, Theme, tinted};

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
    /// Either sets every emphasis mark, as Excel writes a run's properties whole.
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
            emphasis: Emphasis {
                italic: Some(super::on(node, "i")),
                underline: Some(
                    child(node, "u").is_some_and(|u| u.attribute("val") != Some("none")),
                ),
                strike: Some(super::on(node, "strike")),
                highlight: None,
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
        // Sheets added go in first, as the sheets the other edits write, and
        // sheets deleted and renamed go last.
        let sheets = super::parse_sheet_operations(operations, &self.sheets)?;
        let sheet_changes = super::parse_sheet_changes(operations, &self.sheets)?;
        if !sheets.is_empty() || !sheet_changes.is_empty() {
            let (added, removed) = super::sheet_add::add_sheets(self, &sheets)?;
            let staged = destination.with_extension(format!(
                "sheets.{}",
                destination
                    .extension()
                    .context("workbook without an extension")?
                    .to_string_lossy()
            ));
            write_archive_without(&self.raw, &staged, &added, &removed)?;
            let book = Workbook::open(&staged);
            fs::remove_file(&staged)?;
            let rest: Vec<Value> = operations
                .iter()
                .filter(|operation| !super::is_sheet_operation(operation))
                .cloned()
                .collect();
            let mut report =
                book?.patch_with_fonts(destination, &rest, changes, formulas, fonts, assets)?;
            let mut parts: BTreeSet<String> = report["changed_parts"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|p| p.as_str().map(str::to_owned))
                .collect();
            parts.extend(added.into_keys());
            parts.extend(super::sheet_add::change_sheets(
                destination,
                &sheet_changes,
            )?);
            report["changed_parts"] = json!(parts);
            if !sheets.is_empty() {
                report["added_sheets"] = json!(sheets.iter().map(|s| &s.name).collect::<Vec<_>>());
            }
            let (renamed, deleted): (Vec<_>, Vec<_>) = sheet_changes
                .iter()
                .partition(|change| change.name.is_some());
            if !renamed.is_empty() {
                report["renamed_sheets"] = json!(
                    renamed
                        .iter()
                        .map(|change| json!({"sheet":change.sheet,"name":change.name}))
                        .collect::<Vec<_>>()
                );
            }
            if !deleted.is_empty() {
                report["deleted_sheets"] =
                    json!(deleted.iter().map(|c| &c.sheet).collect::<Vec<_>>());
            }
            if let Some(report) = report.as_object_mut() {
                report.remove("non_edit_preservation_verified");
            }
            return Ok(report);
        }
        // A formula written as the cells stand after an operation goes in
        // once every operation has moved the cells.
        let (framed, formulas): (Vec<Value>, Vec<Value>) = formulas
            .iter()
            .cloned()
            .partition(|formula| formula["written_after"].is_string());
        let formulas = formulas.as_slice();
        let mut report = if fonts.is_empty() {
            self.patch_with_operations_and_assets(
                destination,
                operations,
                changes,
                formulas,
                assets,
            )?
        } else {
            self.patch_fonts_first(destination, operations, changes, formulas, fonts, assets)?
        };
        // Formats go in last, in the cells every operation leaves.
        let formats = super::parse_format_operations(operations, &self.sheets)?;
        let structural = super::parse_operations(operations, &self.sheets)?;
        let mut formatted =
            super::formula_edit::apply_framed_formulas(destination, &structural, &framed)?;
        formatted.extend(super::formatting::apply_formats(
            destination,
            &structural,
            &formats,
        )?);
        let placements = super::parse_place_operations(operations, &self.sheets)?;
        formatted.extend(super::placing::apply_placements(
            self,
            destination,
            &structural,
            &placements,
        )?);
        let objects = super::parse_cell_object_operations(operations, &self.sheets)?;
        formatted.extend(super::cell_objects::apply_cell_objects(
            destination,
            &structural,
            &objects,
        )?);
        if !formatted.is_empty() {
            let mut parts: BTreeSet<String> = report["changed_parts"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|p| p.as_str().map(str::to_owned))
                .collect();
            parts.extend(formatted);
            report["changed_parts"] = json!(parts);
            // That check covered the edits before the formats.
            if let Some(report) = report.as_object_mut() {
                report.remove("non_edit_preservation_verified");
            }
        }
        Ok(report)
    }

    /// [`Workbook::patch_with_fonts`] with fonts to write: the original cells
    /// and shapes get them before the other edits.
    fn patch_fonts_first(
        &self,
        destination: &Path,
        operations: &[Value],
        changes: &[Value],
        formulas: &[Value],
        fonts: &[Value],
        assets: &BTreeMap<String, Vec<u8>>,
    ) -> Result<Value> {
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
pub(super) struct StyleFormats<'a> {
    source: &'a str,
    fonts: Vec<String>,
    formats: Vec<String>,
    borders: Vec<String>,
    fills: Vec<String>,
    font_count: usize,
    format_count: usize,
    border_count: usize,
    fill_count: usize,
    /// The first index of each font, format, border and fill.
    font_index: BTreeMap<String, usize>,
    format_index: BTreeMap<String, usize>,
    border_index: BTreeMap<String, usize>,
    fill_index: BTreeMap<String, usize>,
    /// The number formats the part defines, by ID, and how many it had.
    number_formats: Vec<(u32, String)>,
    number_format_count: usize,
    /// (font index, edit) to the index of the edited font.
    edited_fonts: BTreeMap<(usize, String), usize>,
}

/// The index of the first `item` in `items`, added at the end if there is none.
fn index_of(items: &mut Vec<String>, index: &mut BTreeMap<String, usize>, item: String) -> usize {
    if let Some(found) = index.get(&item) {
        return *found;
    }
    index.insert(item.clone(), items.len());
    items.push(item);
    items.len() - 1
}

fn first_indices(items: &[String]) -> BTreeMap<String, usize> {
    let mut index = BTreeMap::new();
    for (position, item) in items.iter().enumerate() {
        index.entry(item.clone()).or_insert(position);
    }
    index
}

impl<'a> StyleFormats<'a> {
    pub(super) fn new(source: &'a str) -> Result<Self> {
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
        // A styles part may leave out the borders and fills no format draws.
        let optional = |name: &str| -> Result<Vec<String>> {
            if child(doc.root_element(), name).is_some() {
                list(name)
            } else {
                Ok(vec![])
            }
        };
        let borders = optional("borders")?;
        let fills = optional("fills")?;
        let number_formats: Vec<(u32, String)> = child(doc.root_element(), "numFmts")
            .into_iter()
            .flat_map(|list| list.children().filter(|n| n.has_tag_name((NS, "numFmt"))))
            .filter_map(|n| {
                Some((
                    n.attribute("numFmtId")?.parse().ok()?,
                    n.attribute("formatCode")?.to_owned(),
                ))
            })
            .collect();
        Ok(Self {
            source,
            font_count: fonts.len(),
            format_count: formats.len(),
            border_count: borders.len(),
            fill_count: fills.len(),
            font_index: first_indices(&fonts),
            format_index: first_indices(&formats),
            border_index: first_indices(&borders),
            fill_index: first_indices(&fills),
            number_format_count: number_formats.len(),
            number_formats,
            fonts,
            formats,
            borders,
            fills,
            edited_fonts: BTreeMap::new(),
        })
    }

    /// The index of a format like `format` with the font emphasis, fill and
    /// number format `look` sets, and the sides of `lines` (left, right, top,
    /// bottom) drawn, or taken away when the line is none.
    pub(super) fn with_look(
        &mut self,
        format: usize,
        look: &super::CellLook,
        lines: &[Option<Option<Line>>; 4],
    ) -> Result<usize> {
        if look.is_empty() && lines.iter().all(Option::is_none) {
            return Ok(format);
        }
        let xf = self
            .formats
            .get(format)
            .with_context(|| format!("cell format {format} is missing"))?
            .clone();
        let (opening, rest) = xml_opening(&xf)?;
        let mut opening = opening.to_owned();
        let index = |opening: &str, name: &str| -> Result<usize> {
            Ok(xml_attribute_value(opening, name)?.unwrap_or("0").parse()?)
        };
        if look.bold.is_some()
            || look.italic.is_some()
            || look.strike.is_some()
            || look.underline.is_some()
        {
            let font = index(&opening, "fontId")?;
            let original = self
                .fonts
                .get(font)
                .with_context(|| format!("font {font} is missing"))?;
            let emphasized = emphasized_font(original, look)?;
            let font = index_of(&mut self.fonts, &mut self.font_index, emphasized);
            opening = set_xml_attribute(&opening, "fontId", &font.to_string())?;
            opening = set_xml_attribute(&opening, "applyFont", "1")?;
        }
        if let Some(fill) = &look.fill {
            ensure!(
                !self.fills.is_empty(),
                "the workbook's styles part has no fills to add to"
            );
            let prefix = element_tag(&self.fills[0])?
                .strip_suffix("fill")
                .context("invalid fill tag")?
                .to_owned();
            let element = if fill == "none" {
                format!(r#"<{prefix}fill><{prefix}patternFill patternType="none"/></{prefix}fill>"#)
            } else {
                format!(
                    r#"<{prefix}fill><{prefix}patternFill patternType="solid"><{prefix}fgColor rgb="FF{fill}"/><{prefix}bgColor indexed="64"/></{prefix}patternFill></{prefix}fill>"#
                )
            };
            let fill = index_of(&mut self.fills, &mut self.fill_index, element);
            opening = set_xml_attribute(&opening, "fillId", &fill.to_string())?;
            opening = set_xml_attribute(&opening, "applyFill", "1")?;
        }
        if let Some(code) = &look.number_format {
            let id = self.number_format_id(code);
            opening = set_xml_attribute(&opening, "numFmtId", &id.to_string())?;
            opening = set_xml_attribute(&opening, "applyNumberFormat", "1")?;
        }
        if lines.iter().any(Option::is_some) {
            ensure!(
                !self.borders.is_empty(),
                "the workbook's styles part has no borders to add to"
            );
            let border = index(&opening, "borderId")?;
            let original = self
                .borders
                .get(border)
                .with_context(|| format!("border {border} is missing"))?;
            let drawn = drawn_border(original, lines)?;
            let border = index_of(&mut self.borders, &mut self.border_index, drawn);
            opening = set_xml_attribute(&opening, "borderId", &border.to_string())?;
            opening = set_xml_attribute(&opening, "applyBorder", "1")?;
        }
        Ok(index_of(
            &mut self.formats,
            &mut self.format_index,
            format!("{opening}{rest}"),
        ))
    }

    /// The index of a format like `format` whose font shows a link, as Excel
    /// gives a cell it links: underlined, in the theme's link color.
    pub(super) fn with_link_font(&mut self, format: usize) -> Result<usize> {
        self.link_font(format, true)
    }

    /// The index of a format like `format` whose font no longer shows a link:
    /// without the underline and the theme's link color, when it has them.
    pub(super) fn without_link_font(&mut self, format: usize) -> Result<usize> {
        self.link_font(format, false)
    }

    fn link_font(&mut self, format: usize, linked: bool) -> Result<usize> {
        let xf = self
            .formats
            .get(format)
            .with_context(|| format!("cell format {format} is missing"))?
            .clone();
        let (opening, rest) = xml_opening(&xf)?;
        let font: usize = xml_attribute_value(opening, "fontId")?
            .unwrap_or("0")
            .parse()?;
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
        let prefix = element_tag(original)?
            .strip_suffix("font")
            .context("invalid font tag")?
            .to_owned();
        let shows_link =
            child(element, "color").is_some_and(|color| color.attribute("theme") == Some("10"));
        if !linked && !shows_link {
            return Ok(format);
        }
        let mut children = if linked {
            format!(r#"<{prefix}u/><{prefix}color theme="10"/>"#)
        } else {
            String::new()
        };
        for node in element.children().filter(Node::is_element) {
            if !matches!(node.tag_name().name(), "u" | "color") {
                children.push_str(&wrapped[node.range()]);
            }
        }
        let (font_opening, _) = xml_opening(original)?;
        let font_opening = font_opening.trim_end_matches("/>").trim_end_matches('>');
        let linked = format!("{font_opening}>{children}</{prefix}font>");
        let font = index_of(&mut self.fonts, &mut self.font_index, linked);
        let mut opening = set_xml_attribute(opening, "fontId", &font.to_string())?;
        opening = set_xml_attribute(&opening, "applyFont", "1")?;
        Ok(index_of(
            &mut self.formats,
            &mut self.format_index,
            format!("{opening}{rest}"),
        ))
    }

    /// The ID of the number format `code`: a built-in one, one the part
    /// defines, or one defined for it.
    fn number_format_id(&mut self, code: &str) -> u32 {
        const BUILT_IN: [(u32, &str); 8] = [
            (0, "General"),
            (1, "0"),
            (2, "0.00"),
            (3, "#,##0"),
            (4, "#,##0.00"),
            (9, "0%"),
            (10, "0.00%"),
            (49, "@"),
        ];
        if let Some((id, _)) = BUILT_IN.iter().find(|(_, built)| *built == code) {
            return *id;
        }
        if let Some((id, _)) = self
            .number_formats
            .iter()
            .find(|(_, defined)| defined == code)
        {
            return *id;
        }
        let id = self
            .number_formats
            .iter()
            .map(|(id, _)| id + 1)
            .max()
            .unwrap_or(0)
            .max(164);
        self.number_formats.push((id, code.to_owned()));
        id
    }

    /// Whether the format `format` draws a line on its `side` (`left`, ...).
    pub(super) fn draws(&self, format: usize, side: &str) -> Result<bool> {
        let xf = self
            .formats
            .get(format)
            .with_context(|| format!("cell format {format} is missing"))?;
        let border: usize = xml_attribute_value(xml_opening(xf)?.0, "borderId")?
            .unwrap_or("0")
            .parse()?;
        let Some(border) = self.borders.get(border) else {
            return Ok(false);
        };
        let wrapped = with_namespace(border);
        let doc = Document::parse(&wrapped)?;
        Ok(doc
            .root_element()
            .first_element_child()
            .and_then(|element| child(element, side))
            .and_then(|node| node.attribute("style"))
            .is_some_and(|style| style != "none"))
    }

    /// The index of a format like `format` whose borders are only those
    /// the format `next` draws alike: Excel's format of a cell inserted
    /// between a cell of `format` and one of `next`, which keeps a line only
    /// where the cells on both sides of the insertion draw it.
    pub(super) fn with_shared_borders(&mut self, format: usize, next: usize) -> Result<usize> {
        let border_of = |formats: &[String], index: usize| -> Result<usize> {
            let xf = formats
                .get(index)
                .with_context(|| format!("cell format {index} is missing"))?;
            Ok(xml_attribute_value(xml_opening(xf)?.0, "borderId")?
                .unwrap_or("0")
                .parse()?)
        };
        let (own, other) = (
            border_of(&self.formats, format)?,
            border_of(&self.formats, next)?,
        );
        if own == other {
            return Ok(format);
        }
        // A border the part does not list draws nothing ARP can tell.
        let (Some(own_border), Some(other_border)) =
            (self.borders.get(own), self.borders.get(other))
        else {
            return Ok(format);
        };
        let Some(shared) = shared_border(own_border, other_border)? else {
            return Ok(format);
        };
        let index = index_of(&mut self.borders, &mut self.border_index, shared);
        let xf = self.formats[format].clone();
        let (opening, rest) = xml_opening(&xf)?;
        let mut opening = set_xml_attribute(opening, "borderId", &index.to_string())?;
        opening = set_xml_attribute(&opening, "applyBorder", "1")?;
        Ok(index_of(
            &mut self.formats,
            &mut self.format_index,
            format!("{opening}{rest}"),
        ))
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
                let index = index_of(&mut self.fonts, &mut self.font_index, edited);
                self.edited_fonts.insert(key, index);
                index
            }
        };
        let mut opening = set_xml_attribute(opening, "fontId", &new_font.to_string())?;
        opening = set_xml_attribute(&opening, "applyFont", "1")?;
        let edited = format!("{opening}{rest}");
        Ok(index_of(&mut self.formats, &mut self.format_index, edited).to_string())
    }

    /// The index of a format like `format` with the alignment settings of
    /// `alignment` changed; a setting back at Excel's default is left out.
    pub(super) fn with_alignment(
        &mut self,
        format: usize,
        alignment: &super::Alignment,
    ) -> Result<usize> {
        let xf = self
            .formats
            .get(format)
            .with_context(|| format!("cell format {format} is missing"))?
            .clone();
        let wrapped = with_namespace(&xf);
        let doc = Document::parse(&wrapped)?;
        let element = doc
            .root_element()
            .first_element_child()
            .context("format element")?;
        let prefix = element_tag(&xf)?
            .strip_suffix("xf")
            .context("invalid format tag")?
            .to_owned();
        let old = child(element, "alignment");
        let mut attributes: Vec<(String, String)> = old
            .map(|node| {
                node.attributes()
                    .map(|a| (a.name().to_owned(), a.value().to_owned()))
                    .collect()
            })
            .unwrap_or_default();
        let mut set = |name: &str, value: Option<&str>| {
            attributes.retain(|(key, _)| key != name);
            if let Some(value) = value {
                attributes.push((name.to_owned(), value.to_owned()));
            }
        };
        if let Some(wrap) = alignment.wrap {
            set("wrapText", wrap.then_some("1"));
        }
        if let Some(shrink) = alignment.shrink {
            set("shrinkToFit", shrink.then_some("1"));
        }
        if let Some(horizontal) = &alignment.horizontal {
            set(
                "horizontal",
                (horizontal != "general").then_some(horizontal),
            );
        }
        if let Some(vertical) = &alignment.vertical {
            set("vertical", (vertical != "bottom").then_some(vertical));
        }
        let rendered: String = attributes
            .iter()
            .map(|(name, value)| format!(" {name}=\"{}\"", xml_attr(value)))
            .collect();
        let replacement = if attributes.is_empty() {
            String::new()
        } else {
            format!("<{prefix}alignment{rendered}/>")
        };
        // Offsets in the wrapped text less the wrapper's opening tag.
        let offset = wrapped.find(&xf).context("format in wrapper")?;
        let (opening, _) = xml_opening(&xf)?;
        let opening = set_xml_attribute(opening, "applyAlignment", "1")?;
        let edited = match old {
            Some(node) => {
                let range = node.range().start - offset..node.range().end - offset;
                let mut edited = xf.clone();
                edited.replace_range(range, &replacement);
                let (old_opening, _) = xml_opening(&edited)?;
                format!("{opening}{}", &edited[old_opening.len()..])
            }
            // An alignment goes first among the format's children.
            None if xf.trim_end().ends_with("/>") => format!(
                "{}>{replacement}</{prefix}xf>",
                opening
                    .trim_end_matches('>')
                    .trim_end_matches('/')
                    .trim_end()
            ),
            None => format!("{opening}{replacement}{}", &xf[xml_opening(&xf)?.0.len()..]),
        };
        Ok(index_of(&mut self.formats, &mut self.format_index, edited))
    }

    /// The styles part with the added fonts and formats.
    pub(super) fn finish(self) -> Result<String> {
        let doc = Document::parse(self.source)?;
        let mut edits = vec![];
        if self.number_formats.len() > self.number_format_count {
            let added: String = self.number_formats[self.number_format_count..]
                .iter()
                .map(|(id, code)| {
                    format!(
                        r#"<numFmt numFmtId="{id}" formatCode="{}"/>"#,
                        xml_attr(code)
                    )
                })
                .collect();
            let count = self.number_formats.len();
            match child(doc.root_element(), "numFmts") {
                Some(list) => {
                    let raw = &self.source[list.range()];
                    let (opening, _) = xml_opening(raw)?;
                    let count = set_xml_attribute(opening, "count", &count.to_string())?;
                    let prefix = element_tag(opening)?
                        .strip_suffix("numFmts")
                        .context("invalid numFmts tag")?;
                    let added = added.replace("<numFmt ", &format!("<{prefix}numFmt "));
                    let inner = match raw.rfind("</") {
                        Some(close) if !raw.trim_end().ends_with("/>") => {
                            format!("{}{added}{}", &raw[opening.len()..close], &raw[close..])
                        }
                        _ => format!("{added}</{prefix}numFmts>"),
                    };
                    let count = if raw.trim_end().ends_with("/>") {
                        format!(
                            "{}>",
                            count.trim_end_matches('>').trim_end_matches('/').trim_end()
                        )
                    } else {
                        count
                    };
                    edits.push((list.range(), format!("{count}{inner}")));
                }
                None => {
                    // The number formats come first in a styles part.
                    let root = doc.root_element();
                    let (opening, _) = xml_opening(&self.source[root.range()])?;
                    let prefix = element_tag(opening)?
                        .strip_suffix("styleSheet")
                        .context("invalid styleSheet tag")?;
                    let added = added.replace("<numFmt ", &format!("<{prefix}numFmt "));
                    let at = root.range().start + opening.len();
                    edits.push((
                        at..at,
                        format!(r#"<{prefix}numFmts count="{count}">{added}</{prefix}numFmts>"#),
                    ));
                }
            }
        }
        for (name, items, original) in [
            ("fonts", &self.fonts, self.font_count),
            ("fills", &self.fills, self.fill_count),
            ("borders", &self.borders, self.border_count),
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

/// A line of a cell border: its style (`thin`, ...) and `RRGGBB` color,
/// none for the automatic color.
pub(super) type Line = (String, Option<String>);

/// The `font` element `font` with the emphasis `look` sets.
fn emphasized_font(font: &str, look: &super::CellLook) -> Result<String> {
    let wrapped = with_namespace(font);
    let doc = Document::parse(&wrapped)?;
    let element = doc
        .root_element()
        .first_element_child()
        .context("font element")?;
    let prefix = element_tag(font)?
        .strip_suffix("font")
        .context("invalid font tag")?
        .to_owned();
    let set = [
        ("b", look.bold.map(|on| on.then(|| format!("<{prefix}b/>")))),
        (
            "i",
            look.italic.map(|on| on.then(|| format!("<{prefix}i/>"))),
        ),
        (
            "strike",
            look.strike
                .map(|on| on.then(|| format!("<{prefix}strike/>"))),
        ),
        (
            "u",
            look.underline.as_deref().map(|underline| match underline {
                "none" => None,
                "double" => Some(format!(r#"<{prefix}u val="double"/>"#)),
                _ => Some(format!("<{prefix}u/>")),
            }),
        ),
    ];
    // An emphasis goes first; a font's children may come in any order.
    let mut children = String::new();
    for (name, value) in &set {
        if let Some(Some(xml)) = value {
            children.push_str(xml);
        } else if value.is_none()
            && let Some(node) = child(element, name)
        {
            children.push_str(&wrapped[node.range()]);
        }
    }
    for node in element.children().filter(Node::is_element) {
        if !set.iter().any(|(name, _)| node.tag_name().name() == *name) {
            children.push_str(&wrapped[node.range()]);
        }
    }
    let (opening, _) = xml_opening(font)?;
    let opening = opening.trim_end_matches("/>").trim_end_matches('>');
    Ok(format!("{opening}>{children}</{prefix}font>"))
}

/// The `border` element `border` with the sides of `lines` (left, right,
/// top, bottom) drawn, or taken away when the line is none, its sides in
/// the order Excel writes them.
fn drawn_border(border: &str, lines: &[Option<Option<Line>>; 4]) -> Result<String> {
    let wrapped = with_namespace(border);
    let doc = Document::parse(&wrapped)?;
    let element = doc
        .root_element()
        .first_element_child()
        .context("border element")?;
    let prefix = element_tag(border)?
        .strip_suffix("border")
        .context("invalid border tag")?
        .to_owned();
    let mut children = String::new();
    for name in [
        "start",
        "end",
        "left",
        "right",
        "top",
        "bottom",
        "diagonal",
        "vertical",
        "horizontal",
    ] {
        let side = ["left", "right", "top", "bottom"]
            .iter()
            .position(|side| *side == name);
        match side.and_then(|side| lines[side].as_ref()) {
            Some(Some((style, color))) => {
                let color = match color {
                    Some(color) => format!(r#"<{prefix}color rgb="FF{color}"/>"#),
                    None => format!(r#"<{prefix}color indexed="64"/>"#),
                };
                children.push_str(&format!(
                    r#"<{prefix}{name} style="{}">{color}</{prefix}{name}>"#,
                    xml_attr(style)
                ));
            }
            Some(None) => children.push_str(&format!("<{prefix}{name}/>")),
            None => match child(element, name) {
                Some(node) => children.push_str(&wrapped[node.range()]),
                // Excel writes the four sides and the diagonal of every border.
                None if side.is_some() || name == "diagonal" => {
                    children.push_str(&format!("<{prefix}{name}/>"));
                }
                None => {}
            },
        }
    }
    let (opening, _) = xml_opening(border)?;
    let opening = opening.trim_end_matches("/>").trim_end_matches('>');
    Ok(format!("{opening}>{children}</{prefix}border>"))
}

/// The `border` element `own` with only the lines `other` draws alike on
/// the same side: the other sides are written empty, as Excel writes a side
/// without a line, and the diagonal keeps its direction only while drawn.
/// None when `own` keeps all its lines.
fn shared_border(own: &str, other: &str) -> Result<Option<String>> {
    let (own_wrapped, other_wrapped) = (with_namespace(own), with_namespace(other));
    let (own_doc, other_doc) = (
        Document::parse(&own_wrapped)?,
        Document::parse(&other_wrapped)?,
    );
    let own_border = own_doc
        .root_element()
        .first_element_child()
        .context("border element")?;
    let other_border = other_doc
        .root_element()
        .first_element_child()
        .context("border element")?;
    let prefix = element_tag(own)?
        .strip_suffix("border")
        .context("invalid border tag")?
        .to_owned();
    // A side as it draws: its line style and color.
    let line = |border: Node<'_, '_>, name: &str| -> Option<(String, String)> {
        let side = child(border, name)?;
        let style = side.attribute("style").filter(|style| *style != "none")?;
        let color = side
            .children()
            .find(|n| n.is_element() && n.tag_name().name() == "color")
            .map(|color| {
                let mut attributes: Vec<_> = color
                    .attributes()
                    .map(|a| format!("{}={}", a.name(), a.value()))
                    .collect();
                attributes.sort();
                attributes.join(" ")
            })
            .unwrap_or_default();
        Some((style.to_owned(), color))
    };
    let diagonal = |border: Node<'_, '_>| {
        (
            border.attribute("diagonalUp") == Some("1"),
            border.attribute("diagonalDown") == Some("1"),
        )
    };
    let mut children = String::new();
    let mut kept_diagonal = false;
    let mut dropped = false;
    for node in own_border.children().filter(Node::is_element) {
        let name = node.tag_name().name();
        let drawn = line(own_border, name).is_some();
        let shared = drawn
            && line(own_border, name) == line(other_border, name)
            && (name != "diagonal" || diagonal(own_border) == diagonal(other_border));
        if shared || !drawn {
            children.push_str(&own_wrapped[node.range()]);
            kept_diagonal |= shared && name == "diagonal";
        } else {
            dropped = true;
            children.push_str(&format!("<{prefix}{name}/>"));
        }
    }
    if !dropped {
        return Ok(None);
    }
    let (opening, _) = xml_opening(own)?;
    let mut opening = opening
        .trim_end_matches("/>")
        .trim_end_matches('>')
        .to_owned();
    if !kept_diagonal {
        for name in ["diagonalUp", "diagonalDown"] {
            if xml_attribute_value(&format!("{opening}>"), name)?.is_some() {
                opening = set_xml_attribute(&format!("{opening}>"), name, "0")?
                    .trim_end_matches('>')
                    .to_owned();
            }
        }
    }
    Ok(Some(format!("{opening}>{children}</{prefix}border>")))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edited_formats_and_fonts_take_the_first_identical_one() {
        let styles = |fonts: &[&str], formats: &[&str]| {
            format!(
                r#"<styleSheet xmlns="{NS}"><fonts count="{}">{}</fonts><cellXfs count="{}">{}</cellXfs></styleSheet>"#,
                fonts.len(),
                fonts.concat(),
                formats.len(),
                formats.concat()
            )
        };
        let edit = FontEdit::parse(&json!({"size": 12})).unwrap();
        let font = r#"<font><sz val="11"/></font>"#;
        let plain = styles(&[font], &[r#"<xf fontId="0"/>"#, r#"<xf fontId="0"/>"#]);
        let mut formats = StyleFormats::new(&plain).unwrap();
        assert_eq!(formats.with_font(0, &edit).unwrap(), "2");
        assert_eq!(formats.with_font(1, &edit).unwrap(), "2");
        assert_eq!((formats.fonts.len(), formats.formats.len()), (2, 3));
        let (edited_font, edited_format) = (formats.fonts[1].clone(), formats.formats[2].clone());
        // Existing copies of the edited font and format are taken, the first of each.
        let listed = styles(
            &[font, &edited_font, &edited_font],
            &[r#"<xf fontId="0"/>"#, &edited_format, &edited_format],
        );
        let mut formats = StyleFormats::new(&listed).unwrap();
        assert_eq!(formats.with_font(0, &edit).unwrap(), "1");
        assert_eq!((formats.fonts.len(), formats.formats.len()), (3, 3));
        assert_eq!(formats.finish().unwrap(), listed);
    }

    /// A format with new alignment settings keeps the others and the format's
    /// other children; a setting back at Excel's default is left out, and a
    /// format equal to one listed is that one.
    #[test]
    fn alignment_changes_make_formats_like_the_old_ones() {
        let source = format!(
            r#"<x:styleSheet xmlns:x="{NS}"><x:fonts count="1"><x:font/></x:fonts><x:cellXfs count="3"><x:xf fontId="0"/><x:xf fontId="0" applyAlignment="1"><x:alignment horizontal="center" wrapText="1"/><x:protection locked="0"/></x:xf><x:xf fontId="0" applyAlignment="1"><x:alignment horizontal="center"/><x:protection locked="0"/></x:xf></x:cellXfs></x:styleSheet>"#
        );
        let mut formats = StyleFormats::new(&source).unwrap();
        let wrap = |wrap| super::super::Alignment {
            wrap: Some(wrap),
            ..Default::default()
        };
        assert_eq!(formats.with_alignment(1, &wrap(false)).unwrap(), 2);
        assert_eq!(formats.with_alignment(0, &wrap(true)).unwrap(), 3);
        assert_eq!(
            formats.formats[3],
            r#"<x:xf fontId="0" applyAlignment="1"><x:alignment wrapText="1"/></x:xf>"#
        );
        let top = super::super::Alignment {
            vertical: Some("top".into()),
            horizontal: Some("general".into()),
            ..Default::default()
        };
        assert_eq!(formats.with_alignment(1, &top).unwrap(), 4);
        assert_eq!(
            formats.formats[4],
            r#"<x:xf fontId="0" applyAlignment="1"><x:alignment wrapText="1" vertical="top"/><x:protection locked="0"/></x:xf>"#
        );
        Document::parse(&formats.finish().unwrap()).unwrap();
    }
}
