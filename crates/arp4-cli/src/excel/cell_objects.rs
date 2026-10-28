//! What cells carry besides their values and formats: links (`<hyperlinks>`),
//! notes (comments with their VML shapes), data validations and conditional
//! formats. The extraction reads them, and operations set or clear them on a
//! range (`set_link`, `clear_links`, `set_note`, `clear_notes`,
//! `set_validation`, `clear_validations`, `add_conditional_format`,
//! `clear_conditional_formats`). Like a format operation, each names its
//! range in the cells the operations recorded before it leave and is written
//! last, after the row and column operations, into the cells they leave; the
//! operations recorded after it move its range as they move the cells.
use super::*;

/// The members of a worksheet in the order its schema gives them.
const WORKSHEET_ORDER: [&str; 39] = [
    "sheetPr",
    "dimension",
    "sheetViews",
    "sheetFormatPr",
    "cols",
    "sheetData",
    "sheetCalcPr",
    "sheetProtection",
    "protectedRanges",
    "scenarios",
    "autoFilter",
    "sortState",
    "dataConsolidate",
    "customSheetViews",
    "mergeCells",
    "phoneticPr",
    "conditionalFormatting",
    "dataValidations",
    "hyperlinks",
    "printOptions",
    "pageMargins",
    "pageSetup",
    "headerFooter",
    "rowBreaks",
    "colBreaks",
    "customProperties",
    "cellWatches",
    "ignoredErrors",
    "smartTags",
    "drawing",
    "legacyDrawing",
    "legacyDrawingHF",
    "drawingHF",
    "picture",
    "oleObjects",
    "controls",
    "webPublishItems",
    "tableParts",
    "extLst",
];

const HYPERLINK: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink";
const COMMENTS: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/comments";
const VML_DRAWING: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/vmlDrawing";

/// What an operation does to the cells of its range.
#[derive(Clone, Debug, PartialEq)]
pub enum CellObjectKind {
    /// A link to a place in the workbook (`location`, `Sheet!A1` or a name)
    /// or to a URL, replacing the links of the range.
    Link {
        location: Option<String>,
        url: Option<String>,
        tooltip: Option<String>,
        /// Leave the cells' format as it is, without the look of a link.
        keep_format: bool,
    },
    ClearLinks,
    /// A note with this text on the range's top left cell.
    Note {
        text: String,
        author: String,
    },
    ClearNotes,
    /// A data validation over the range, replacing the ones there.
    Validation(Value),
    ClearValidations,
    /// A conditional format over the range, ahead of the others.
    ConditionalFormat(Value),
    ClearConditionalFormats,
    /// The links, notes, validations and conditional formats of the range
    /// copied to the place of its size at `to` (`copy_cells`), as Excel
    /// pastes them.
    Copy {
        to: String,
    },
}

/// An operation setting or clearing what cells carry.
#[derive(Clone, Debug)]
pub struct CellObjectOperation {
    pub id: String,
    pub sheet: String,
    pub range: String,
    pub kind: CellObjectKind,
    /// How many row and column operations were recorded before it.
    pub preceding: usize,
}

/// The operation kinds of this module.
pub const KINDS: [&str; 8] = [
    "set_link",
    "clear_links",
    "set_note",
    "clear_notes",
    "set_validation",
    "clear_validations",
    "add_conditional_format",
    "clear_conditional_formats",
];

/// The operations of `values` this module writes, in the order recorded.
pub fn parse_cell_object_operations(
    values: &[Value],
    sheets: &[impl crate::extraction::Fields],
) -> Result<Vec<CellObjectOperation>> {
    let sheet_names: BTreeSet<String> = sheets
        .iter()
        .map(|sheet| string(&sheet.fields()["name"]).map(str::to_owned))
        .collect::<Result<_>>()?;
    let mut ids = BTreeSet::new();
    let mut operations = vec![];
    let mut preceding = 0;
    for value in values {
        let Some(kind) = value["kind"]
            .as_str()
            .filter(|kind| KINDS.contains(kind) || *kind == "copy_cells")
        else {
            preceding += usize::from(super::operations::structural(value));
            continue;
        };
        let (id, sheet) = super::operations::operation_header(value, &sheet_names, &mut ids)?;
        let range = string(&value["range"])?.to_owned();
        Area::parse(&range)?;
        let text = |name: &str| value[name].as_str().map(str::to_owned);
        let kind = match kind {
            "set_link" => {
                let (location, url) = (text("location"), text("url"));
                ensure!(
                    location.is_some() != url.is_some(),
                    "a link goes to a location or a URL"
                );
                CellObjectKind::Link {
                    location,
                    url,
                    tooltip: text("tooltip"),
                    keep_format: value["keep_format"] == true,
                }
            }
            "clear_links" => CellObjectKind::ClearLinks,
            "set_note" => CellObjectKind::Note {
                text: string(&value["text"])?.to_owned(),
                author: text("author").unwrap_or_default(),
            },
            "clear_notes" => CellObjectKind::ClearNotes,
            "set_validation" => CellObjectKind::Validation(value["rule"].clone()),
            "clear_validations" => CellObjectKind::ClearValidations,
            "add_conditional_format" => CellObjectKind::ConditionalFormat(value["rule"].clone()),
            "copy_cells" => CellObjectKind::Copy {
                to: string(&value["to"])?.to_owned(),
            },
            _ => CellObjectKind::ClearConditionalFormats,
        };
        operations.push(CellObjectOperation {
            id,
            sheet,
            range,
            kind,
            preceding,
        });
    }
    Ok(operations)
}

// ---------------------------------------------------------------- reading

/// The links of a worksheet: the cells (`range`) and where each goes, a
/// `location` in the workbook or a `url`, with its `tooltip`.
pub(super) fn extract_links(
    parts: &BTreeMap<String, Vec<u8>>,
    sheet_part: &str,
    worksheet: Node<'_, '_>,
) -> Result<Vec<Value>> {
    let Some(list) = child(worksheet, "hyperlinks") else {
        return Ok(vec![]);
    };
    let rels = parts
        .get(&relationships_part(sheet_part)?)
        .map(|bytes| std::str::from_utf8(bytes))
        .transpose()?;
    let mut links = vec![];
    for link in list
        .children()
        .filter(|n| n.has_tag_name((NS, "hyperlink")))
    {
        let mut entry = json!({"range":link.attribute("ref").context("link without its cells")?});
        if let Some(location) = link.attribute("location") {
            entry["location"] = json!(location);
        }
        if let (Some(id), Some(rels)) = (link.attribute((REL, "id")), rels)
            && let Some((_, target)) = relationship(rels, id)?
        {
            entry["url"] = json!(target);
        }
        if let Some(tooltip) = link.attribute("tooltip") {
            entry["tooltip"] = json!(tooltip);
        }
        links.push(entry);
    }
    Ok(links)
}

/// The data validations of a worksheet, those of its extension list (which
/// Excel writes for lists on other sheets) included: the cells (`range`),
/// the type, operator, formulas, whether blanks pass, and the messages.
pub(super) fn extract_validations(worksheet: Node<'_, '_>) -> Vec<Value> {
    let mut validations = vec![];
    let ext = worksheet
        .descendants()
        .filter(|n| n.has_tag_name((X14, "dataValidation")));
    let main = child(worksheet, "dataValidations")
        .into_iter()
        .flat_map(|list| {
            list.children()
                .filter(|n| n.has_tag_name((NS, "dataValidation")))
        });
    for node in main.chain(ext) {
        let extended = node.has_tag_name((X14, "dataValidation"));
        let range = if extended {
            node.children()
                .find(|n| n.has_tag_name((XM, "sqref")))
                .and_then(|n| n.text())
        } else {
            node.attribute("sqref")
        };
        let mut entry = json!({"range":range.unwrap_or_default(),
            "type":node.attribute("type").unwrap_or("none"),
            "allow_blank":node.attribute("allowBlank").is_some_and(|v| v == "1" || v == "true")});
        for (attribute, key) in [
            ("operator", "operator"),
            ("errorStyle", "error_style"),
            ("promptTitle", "prompt_title"),
            ("prompt", "prompt"),
            ("errorTitle", "error_title"),
            ("error", "error"),
        ] {
            if let Some(value) = node.attribute(attribute) {
                entry[key] = json!(value);
            }
        }
        for name in ["formula1", "formula2"] {
            let formula = node
                .children()
                .find(|n| n.is_element() && n.tag_name().name() == name)
                .and_then(|f| {
                    if extended {
                        f.children()
                            .find(|n| n.has_tag_name((XM, "f")))
                            .and_then(|n| n.text())
                    } else {
                        f.text()
                    }
                });
            if let Some(formula) = formula {
                entry[name] = json!(formula);
            }
        }
        validations.push(entry);
    }
    validations
}

/// The conditional formats of a worksheet, those of its extension list
/// included: the cells (`range`) and their rules in order, each with its
/// type, operator, formulas, priority and the format it gives (`format`, as
/// `dxfs` gives the differential formats of the styles part).
pub(super) fn extract_conditional_formats(worksheet: Node<'_, '_>, dxfs: &[Value]) -> Vec<Value> {
    let mut formats = vec![];
    let main = worksheet
        .children()
        .filter(|n| n.has_tag_name((NS, "conditionalFormatting")));
    let ext = worksheet
        .descendants()
        .filter(|n| n.has_tag_name((X14, "conditionalFormatting")));
    for node in main.chain(ext) {
        let extended = node.has_tag_name((X14, "conditionalFormatting"));
        let range = if extended {
            node.children()
                .find(|n| n.has_tag_name((XM, "sqref")))
                .and_then(|n| n.text())
        } else {
            node.attribute("sqref")
        };
        let rules: Vec<Value> = node
            .children()
            .filter(|n| n.is_element() && n.tag_name().name() == "cfRule")
            .map(|rule| {
                let formulas: Vec<&str> = rule
                    .descendants()
                    .filter(|n| n.has_tag_name((NS, "formula")) || n.has_tag_name((XM, "f")))
                    .filter_map(|n| n.text())
                    .collect();
                let mut entry = json!({"type":rule.attribute("type").unwrap_or_default(),
                    "priority":rule.attribute("priority").and_then(|p| p.parse::<u64>().ok()),
                    "formulas":formulas});
                if let Some(operator) = rule.attribute("operator") {
                    entry["operator"] = json!(operator);
                }
                if rule
                    .attribute("stopIfTrue")
                    .is_some_and(|v| v == "1" || v == "true")
                {
                    entry["stop"] = json!(true);
                }
                if let Some(format) = rule
                    .attribute("dxfId")
                    .and_then(|id| id.parse::<usize>().ok())
                    .and_then(|id| dxfs.get(id))
                {
                    entry["format"] = format.clone();
                }
                entry
            })
            .collect();
        formats.push(json!({"range":range.unwrap_or_default(),"rules":rules}));
    }
    formats
}

/// The differential formats of a styles part, as conditional formats give
/// them: the fill color, font color and emphasis, number format.
pub(super) fn differential_formats(styles: Option<&[u8]>) -> Result<Vec<Value>> {
    let Some(styles) = styles else {
        return Ok(vec![]);
    };
    let doc = xml(styles)?;
    let Some(list) = child(doc.root_element(), "dxfs") else {
        return Ok(vec![]);
    };
    let rgb = |node: Option<Node<'_, '_>>| -> Option<String> {
        let rgb = node?.attribute("rgb")?;
        Some(rgb.get(rgb.len().saturating_sub(6)..)?.to_ascii_uppercase())
    };
    Ok(list
        .children()
        .filter(|n| n.has_tag_name((NS, "dxf")))
        .map(|dxf| {
            let mut format = json!({});
            if let Some(font) = child(dxf, "font") {
                for (name, key) in [("b", "bold"), ("i", "italic"), ("strike", "strike")] {
                    if let Some(node) = child(font, name) {
                        format[key] = json!(
                            node.attribute("val")
                                .is_none_or(|v| v == "1" || v == "true")
                        );
                    }
                }
                if let Some(color) = rgb(child(font, "color")) {
                    format["font_color"] = json!(color);
                }
            }
            if let Some(fill) = child(dxf, "fill").and_then(|fill| child(fill, "patternFill")) {
                // A solid fill of a differential format is its background.
                if let Some(color) =
                    rgb(child(fill, "bgColor")).or_else(|| rgb(child(fill, "fgColor")))
                {
                    format["fill"] = json!(color);
                }
            }
            if let Some(code) = child(dxf, "numFmt").and_then(|n| n.attribute("formatCode")) {
                format["number_format"] = json!(code);
            }
            format
        })
        .collect())
}

// ---------------------------------------------------------------- areas

/// The bounds of `area`: (first column, last column, first row, last row).
fn bounds(area: Area) -> (u32, u32, u32, u32) {
    let (c0, c1) = area.columns.unwrap_or((1, MAX_COLUMN));
    let (r0, r1) = area.rows.unwrap_or((1, MAX_ROW));
    (c0, c1, r0, r1)
}

fn meets(a: Area, b: Area) -> bool {
    let ((a0, a1, a2, a3), (b0, b1, b2, b3)) = (bounds(a), bounds(b));
    a0 <= b1 && b0 <= a1 && a2 <= b3 && b2 <= a3
}

/// The parts of `area` outside `cut`.
fn subtract(area: Area, cut: Area) -> Vec<Area> {
    if !meets(area, cut) {
        return vec![area];
    }
    let (c0, c1, r0, r1) = bounds(area);
    let (x0, x1, y0, y1) = bounds(cut);
    let rect = |c: (u32, u32), r: (u32, u32)| Area {
        columns: Some(c),
        rows: Some(r),
    };
    let mut parts = vec![];
    if y0 > r0 {
        parts.push(rect((c0, c1), (r0, y0 - 1)));
    }
    if y1 < r1 {
        parts.push(rect((c0, c1), (y1 + 1, r1)));
    }
    let (top, bottom) = (r0.max(y0), r1.min(y1));
    if x0 > c0 {
        parts.push(rect((c0, x0 - 1), (top, bottom)));
    }
    if x1 < c1 {
        parts.push(rect((x1 + 1, c1), (top, bottom)));
    }
    parts
}

/// The space-separated ranges `sqref` without the cells of `cut`; none when
/// no cell is left.
fn sqref_without(sqref: &str, cut: Area) -> Result<Option<String>> {
    let mut kept = vec![];
    for part in sqref.split_whitespace() {
        for area in subtract(Area::parse(part)?, cut) {
            kept.push(area.render()?);
        }
    }
    Ok((!kept.is_empty()).then(|| kept.join(" ")))
}

fn sqref_meets(sqref: &str, cut: Area) -> Result<bool> {
    for part in sqref.split_whitespace() {
        if meets(Area::parse(part)?, cut) {
            return Ok(true);
        }
    }
    Ok(false)
}

// ---------------------------------------------------------------- writing

/// `text` (a worksheet) with `element` put among the worksheet's members
/// where its schema orders `name`: after the last member before it.
fn insert_member(text: &str, name: &str, element: &str) -> Result<String> {
    let doc = xml(text.as_bytes())?;
    let root = doc.root_element();
    let rank = WORKSHEET_ORDER
        .iter()
        .position(|member| *member == name)
        .context("unknown worksheet member")?;
    let after = root.children().filter(Node::is_element).rfind(|node| {
        node.tag_name().namespace() == Some(NS)
            && WORKSHEET_ORDER[..=rank].contains(&node.tag_name().name())
    });
    let at = match after {
        Some(node) => node.range().end,
        None => {
            let (opening, _) = xml_opening(&text[root.range()])?;
            root.range().start + opening.len()
        }
    };
    let mut out = text.to_owned();
    out.insert_str(at, element);
    Ok(out)
}

/// The prefix the worksheet `text` writes its own elements with (`x:`), or
/// none.
fn prefix_of(text: &str) -> Result<String> {
    let doc = xml(text.as_bytes())?;
    Ok(element_tag(&text[doc.root_element().range()])?
        .strip_suffix("worksheet")
        .context("invalid worksheet tag")?
        .to_owned())
}

/// The workbook being written: its parts as the operations change them.
pub(super) struct Writing<'a> {
    pub(super) book: &'a Workbook,
    pub(super) patched: BTreeMap<String, Vec<u8>>,
    pub(super) removed: BTreeSet<String>,
}

impl Writing<'_> {
    pub(super) fn text(&self, part: &str) -> Result<Option<String>> {
        if self.removed.contains(part) {
            return Ok(None);
        }
        Ok(
            match self.patched.get(part).or_else(|| self.book.parts.get(part)) {
                Some(bytes) => Some(String::from_utf8(bytes.clone())?),
                None => None,
            },
        )
    }

    pub(super) fn set(&mut self, part: &str, text: String) {
        self.removed.remove(part);
        self.patched.insert(part.to_owned(), text.into_bytes());
    }

    pub(super) fn exists(&self, part: &str) -> bool {
        !self.removed.contains(part)
            && (self.patched.contains_key(part) || self.book.parts.contains_key(part))
    }

    /// The part a relationship of `kind` of `part` targets, with its ID.
    fn related(&self, part: &str, kind: &str) -> Result<Option<(String, String)>> {
        let Some(rels) = self.text(&relationships_part(part)?)? else {
            return Ok(None);
        };
        let doc = xml(rels.as_bytes())?;
        for rel in doc
            .root_element()
            .children()
            .filter(|n| n.has_tag_name((PKG_REL, "Relationship")))
        {
            if rel.attribute("Type") == Some(kind) {
                let target = rel
                    .attribute("Target")
                    .context("relationship target missing")?;
                return Ok(Some((
                    rel.attribute("Id")
                        .context("relationship ID missing")?
                        .to_owned(),
                    relationship_target(part, target)?,
                )));
            }
        }
        Ok(None)
    }

    /// Adds a relationship of `part`, returning its ID; `external` targets
    /// leave the package.
    pub(super) fn relate(
        &mut self,
        part: &str,
        kind: &str,
        target: &str,
        external: bool,
    ) -> Result<String> {
        let rels_part = relationships_part(part)?;
        let rels = self.text(&rels_part)?;
        let id = relationship_id(rels.as_deref())?;
        let mut text = append_relationship(rels.as_deref(), &id, kind, target)?;
        if external {
            let element = format!(r#"Id="{}""#, xml_attr(&id));
            let at = text.find(&element).context("relationship just added")?;
            text.insert_str(at, r#"TargetMode="External" "#);
        }
        self.set(&rels_part, text);
        Ok(id)
    }

    fn unrelate(&mut self, part: &str, id: &str) -> Result<()> {
        let rels_part = relationships_part(part)?;
        let Some(rels) = self.text(&rels_part)? else {
            return Ok(());
        };
        let doc = xml(rels.as_bytes())?;
        let Some(node) = doc
            .root_element()
            .children()
            .filter(|n| n.has_tag_name((PKG_REL, "Relationship")))
            .find(|n| n.attribute("Id") == Some(id))
        else {
            return Ok(());
        };
        let mut text = rels.clone();
        text.replace_range(node.range(), "");
        self.set(&rels_part, text);
        Ok(())
    }

    pub(super) fn content_type(
        &mut self,
        extension: &str,
        kind: &str,
        part: Option<&str>,
    ) -> Result<()> {
        let types = self
            .text("[Content_Types].xml")?
            .context("package without content types")?;
        let updated = ensure_content_type(&types, extension, kind, part)?;
        self.set("[Content_Types].xml", updated);
        Ok(())
    }
}

/// Writes `operations` into the workbook at `path`, which the other edits
/// and the row and column operations `structural` wrote, and returns the
/// parts they change.
pub(super) fn apply_cell_objects(
    path: &Path,
    structural: &[StructuralOperation],
    operations: &[CellObjectOperation],
) -> Result<Vec<String>> {
    if operations.is_empty() {
        return Ok(vec![]);
    }
    let book = Workbook::open(path)?;
    let mut writing = Writing {
        book: &book,
        patched: BTreeMap::new(),
        removed: BTreeSet::new(),
    };
    let mut written = vec![];
    for operation in operations {
        let part = book
            .sheets
            .iter()
            .find(|sheet| sheet["name"] == operation.sheet.as_str())
            .map(|sheet| string(&sheet["part"]))
            .transpose()?
            .with_context(|| format!("sheet {} is missing", operation.sheet))?
            .to_owned();
        let later: Vec<&StructuralOperation> = structural[operation.preceding..]
            .iter()
            .filter(|later| later.sheet == operation.sheet)
            .collect();
        // Rules grow into rows and columns inserted after them, as Excel
        // formats those like the cells before; links and notes stay on theirs.
        let grows = matches!(
            operation.kind,
            CellObjectKind::Validation(_)
                | CellObjectKind::ClearValidations
                | CellObjectKind::ConditionalFormat(_)
                | CellObjectKind::ClearConditionalFormats
        );
        if let CellObjectKind::Copy { to } = &operation.kind {
            copy_objects(&mut writing, &part, operation, to, &later)?;
            continue;
        }
        let Some(range) = map_sqref(&operation.range, &later, grows)? else {
            continue;
        };
        for part_range in range.split_whitespace() {
            write_one(&mut writing, &part, operation, part_range)?;
        }
        written.push((operation, range));
    }
    link_looks(&mut writing, &book, &written)?;
    let Writing {
        patched, removed, ..
    } = writing;
    let mut patched = patched;
    patched.retain(|part, bytes| book.parts.get(part) != Some(bytes) || removed.contains(part));
    if patched.is_empty() && removed.is_empty() {
        return Ok(vec![]);
    }
    let staged = path.with_extension(format!(
        "objects.{}",
        path.extension()
            .context("workbook without an extension")?
            .to_string_lossy()
    ));
    write_archive_without(&book.raw, &staged, &patched, &removed)?;
    let read = Workbook::open(&staged)?;
    ensure_written(&read, &written)?;
    // What cells carry changes no value.
    for (before, after) in book.cells.iter().zip(&read.cells) {
        let values = |cells: &[Cell]| -> Vec<(String, Value, Option<Box<str>>)> {
            cells
                .iter()
                .map(|cell| {
                    (
                        cell.address.to_string(),
                        cell.value.clone(),
                        cell.formula.clone(),
                    )
                })
                .collect()
        };
        ensure!(
            values(before) == values(after),
            "an operation on links, notes or rules changed cell values"
        );
    }
    fs::rename(&staged, path)?;
    let mut parts: Vec<String> = patched.into_keys().collect();
    parts.extend(removed);
    Ok(parts)
}

/// Gives the cells the links of `written` link the look Excel gives them,
/// underlined in the theme's link color, and takes it off the cells whose
/// links are cleared, leaving the rest of their format.
fn link_looks(
    writing: &mut Writing<'_>,
    book: &Workbook,
    written: &[(&CellObjectOperation, String)],
) -> Result<()> {
    let linked: Vec<_> = written
        .iter()
        .filter(|(operation, _)| {
            matches!(
                operation.kind,
                CellObjectKind::Link {
                    keep_format: false,
                    ..
                } | CellObjectKind::ClearLinks
            )
        })
        .collect();
    if linked.is_empty() {
        return Ok(());
    }
    let source = writing
        .text("xl/styles.xml")?
        .context("the workbook has no styles part, so ARP cannot show links")?;
    let mut styles = super::fonts::StyleFormats::new(&source)?;
    for (operation, range) in linked {
        let part = book
            .sheets
            .iter()
            .find(|sheet| sheet["name"] == operation.sheet.as_str())
            .map(|sheet| string(&sheet["part"]))
            .transpose()?
            .context("linked sheet missing")?;
        for area in range.split_whitespace() {
            let area = Area::parse(area)?;
            let (c0, c1, r0, r1) = bounds(area);
            let text = writing.text(part)?.context("worksheet missing")?;
            let link = matches!(operation.kind, CellObjectKind::Link { .. });
            let restyled =
                super::formatting::restyle(&text, (c0, c1), (r0, r1), |_, _, current| {
                    Ok(Some(if link {
                        styles.with_link_font(current)?
                    } else {
                        styles.without_link_font(current)?
                    }))
                })?;
            writing.set(part, restyled);
        }
    }
    let finished = styles.finish()?;
    writing.set("xl/styles.xml", finished);
    Ok(())
}

/// Writes one operation over one range of the sheet `part`.
fn write_one(
    writing: &mut Writing<'_>,
    part: &str,
    operation: &CellObjectOperation,
    range: &str,
) -> Result<()> {
    let area = Area::parse(range)?;
    match &operation.kind {
        CellObjectKind::Link {
            location,
            url,
            tooltip,
            ..
        } => {
            clear_links(writing, part, area)?;
            add_link(
                writing,
                part,
                range,
                location.as_deref(),
                url.as_deref(),
                tooltip.as_deref(),
            )
        }
        CellObjectKind::ClearLinks => clear_links(writing, part, area),
        CellObjectKind::Note { text, author } => {
            let cell = format!("{}{}", column_name(area.first_column())?, area.first_row());
            set_note(writing, part, &cell, text, author)
        }
        CellObjectKind::ClearNotes => clear_notes(writing, part, area),
        CellObjectKind::Validation(rule) => {
            clear_rules(writing, part, area, "dataValidation")?;
            add_validation(writing, part, range, rule)
        }
        CellObjectKind::ClearValidations => clear_rules(writing, part, area, "dataValidation"),
        CellObjectKind::ConditionalFormat(rule) => {
            add_conditional_format(writing, part, range, rule)
        }
        CellObjectKind::ClearConditionalFormats => {
            clear_rules(writing, part, area, "conditionalFormatting")
        }
        CellObjectKind::Copy { .. } => bail!("a copy is written with its source"),
    }
}

/// Copies the links, notes, validations and conditional formats of the
/// cells of `operation` (a `copy_cells`) to the place at `to`, both moved by
/// the operations recorded after it, as Excel pastes them: those of the place
/// pasted to give way; a rule whose relative references would read other
/// cells there is written as its own, from its new place.
fn copy_objects(
    writing: &mut Writing<'_>,
    part: &str,
    operation: &CellObjectOperation,
    to: &str,
    later: &[&StructuralOperation],
) -> Result<()> {
    let recorded = Area::parse(&operation.range)?;
    let (Some(source), Some(target)) = (
        map_area(recorded, later)?,
        map_area(Area::parse(to)?, later)?,
    ) else {
        return Ok(());
    };
    let (c0, c1, r0, r1) = bounds(source);
    let (down, right) = (
        i64::from(target.first_row()) - i64::from(r0),
        i64::from(target.first_column()) - i64::from(c0),
    );
    let moved = |area: Area| -> Result<Area> {
        let (a0, a1, b0, b1) = bounds(area);
        let shift = |value: u32, by: i64| -> Result<u32> {
            u32::try_from(i64::from(value) + by).context("copied place outside the sheet")
        };
        Ok(Area {
            columns: Some((shift(a0, right)?, shift(a1, right)?)),
            rows: Some((shift(b0, down)?, shift(b1, down)?)),
        })
    };
    let dest = moved(source)?;
    let within = |area: Area| -> Option<Area> {
        let (a0, a1, b0, b1) = bounds(area);
        let (x0, x1, y0, y1) = (a0.max(c0), a1.min(c1), b0.max(r0), b1.min(r1));
        (x0 <= x1 && y0 <= y1).then_some(Area {
            columns: Some((x0, x1)),
            rows: Some((y0, y1)),
        })
    };
    // What the place pasted to holds gives way.
    clear_links(writing, part, dest)?;
    clear_notes(writing, part, dest)?;
    clear_rules(writing, part, dest, "dataValidation")?;
    clear_rules(writing, part, dest, "conditionalFormatting")?;
    let text = writing.text(part)?.context("worksheet missing")?;
    let doc = xml(text.as_bytes())?;
    let root = doc.root_element();
    // Links, with the cells they cover in the range.
    let rels = writing.text(&relationships_part(part)?)?;
    let mut links = vec![];
    for link in child(root, "hyperlinks").into_iter().flat_map(|list| {
        list.children()
            .filter(|n| n.has_tag_name((NS, "hyperlink")))
    }) {
        let Some(area) = link
            .attribute("ref")
            .map(Area::parse)
            .transpose()?
            .and_then(within)
        else {
            continue;
        };
        let url = match (link.attribute((REL, "id")), &rels) {
            (Some(id), Some(rels)) => relationship(rels, id)?.map(|(_, target)| target),
            _ => None,
        };
        links.push((
            moved(area)?.render()?,
            link.attribute("location").map(str::to_owned),
            url,
            link.attribute("tooltip").map(str::to_owned),
        ));
    }
    // Validations and conditional formats, as the elements to add.
    let mut rules: Vec<(&str, String)> = vec![];
    let mut extended: Vec<(String, String)> = vec![];
    for (name, list) in [
        ("dataValidation", "dataValidations"),
        ("conditionalFormatting", ""),
    ] {
        let nodes: Vec<Node<'_, '_>> = if list.is_empty() {
            root.children()
                .filter(|n| n.has_tag_name((NS, name)))
                .collect()
        } else {
            child(root, list)
                .into_iter()
                .flat_map(|l| l.children().filter(|n| n.has_tag_name((NS, name))))
                .collect()
        };
        for node in nodes {
            let sqref = node.attribute("sqref").unwrap_or_default();
            let mut parts = vec![];
            for item in sqref.split_whitespace() {
                if let Some(area) = within(Area::parse(item)?) {
                    parts.push(area);
                }
            }
            let Some(first) = parts.first().copied() else {
                continue;
            };
            // Relative references read from the top left cell of the rule.
            let anchor = Area::parse(sqref.split_whitespace().next().unwrap_or_default())?;
            let (rows_by, columns_by) = (
                i64::from(first.first_row()) - i64::from(anchor.first_row()) + down,
                i64::from(first.first_column()) - i64::from(anchor.first_column()) + right,
            );
            let raw = &text[node.range()];
            let mut element = raw.to_owned();
            let mut edits = vec![];
            for formula in node.descendants().filter(|n| {
                n.has_tag_name((NS, "formula"))
                    || n.has_tag_name((NS, "formula1"))
                    || n.has_tag_name((NS, "formula2"))
            }) {
                if let Some(text_node) = formula.children().find(Node::is_text) {
                    let shifted = super::references::shift_relative(
                        text_node.text().unwrap_or_default(),
                        rows_by,
                        columns_by,
                    )?;
                    let start = text_node.range().start - node.range().start;
                    let end = text_node.range().end - node.range().start;
                    edits.push((start..end, xml_text(&shifted)));
                }
            }
            edits.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
            let same = edits
                .iter()
                .all(|(range, replacement)| raw[range.clone()] == *replacement);
            for (range, replacement) in edits {
                element.replace_range(range, &replacement);
            }
            let sqref = parts
                .iter()
                .map(|area| moved(*area)?.render())
                .collect::<Result<Vec<_>>>()?
                .join(" ");
            // A validation reading the same cells from the copies covers them
            // too, as Excel extends it.
            if same && name == "dataValidation" {
                extended.push((
                    node.attribute("sqref").unwrap_or_default().to_owned(),
                    sqref,
                ));
                continue;
            }
            let (opening, rest) = xml_opening(&element)?;
            let opening = set_xml_attribute(opening, "sqref", &sqref)?;
            rules.push((name, format!("{opening}{rest}")));
        }
    }
    // Notes, by the cells they are on.
    let mut notes = vec![];
    if let Some((comments_part, _)) = note_parts(writing, part, false)? {
        let comments = writing
            .text(&comments_part)?
            .context("comments part missing")?;
        let doc = xml(comments.as_bytes())?;
        let authors: Vec<String> = doc
            .descendants()
            .filter(|n| n.has_tag_name((NS, "author")))
            .map(|n| n.text().unwrap_or_default().to_owned())
            .collect();
        for comment in doc
            .descendants()
            .filter(|n| n.has_tag_name((NS, "comment")))
        {
            let cell = comment.attribute("ref").unwrap_or_default();
            let Some(area) = within(Area::parse(cell)?) else {
                continue;
            };
            let text: String = comment
                .descendants()
                .filter(|n| n.has_tag_name((NS, "t")))
                .filter_map(|n| n.text())
                .collect();
            let author = comment
                .attribute("authorId")
                .and_then(|id| id.parse::<usize>().ok())
                .and_then(|id| authors.get(id).cloned())
                .unwrap_or_default();
            notes.push((moved(area)?.render()?, text, author));
        }
    }
    for (range, location, url, tooltip) in links {
        add_link(
            writing,
            part,
            &range,
            location.as_deref(),
            url.as_deref(),
            tooltip.as_deref(),
        )?;
    }
    for (name, element) in rules {
        let text = writing.text(part)?.context("worksheet missing")?;
        let updated = if name == "conditionalFormatting" {
            // The copies are evaluated after the rules they copy.
            let doc = xml(text.as_bytes())?;
            let next = doc
                .descendants()
                .filter(|n| n.is_element() && n.tag_name().name() == "cfRule")
                .filter_map(|n| n.attribute("priority")?.parse::<u64>().ok())
                .max()
                .unwrap_or(0);
            let mut element = element;
            let mut priority = next;
            let mut at = 0;
            while let Some(found) = element[at..].find("priority=\"") {
                let start = at + found + "priority=\"".len();
                let end = start + element[start..].find('"').context("invalid priority")?;
                priority += 1;
                element.replace_range(start..end, &priority.to_string());
                at = start;
            }
            insert_member(&text, "conditionalFormatting", &element)?
        } else {
            let prefix = prefix_of(&text)?;
            let doc = xml(text.as_bytes())?;
            let updated = match child(doc.root_element(), "dataValidations") {
                Some(list) if !text[list.range()].trim_end().ends_with("/>") => {
                    let raw = &text[list.range()];
                    let close =
                        list.range().start + raw.rfind("</").context("invalid dataValidations")?;
                    let mut out = text.clone();
                    out.insert_str(close, &element);
                    out
                }
                _ => insert_member(
                    &text,
                    "dataValidations",
                    &format!(
                        r#"<{prefix}dataValidations count="1">{element}</{prefix}dataValidations>"#
                    ),
                )?,
            };
            recount(&updated, "dataValidations", "dataValidation")?
        };
        writing.set(part, updated);
    }
    for (sqref, added) in extended {
        let text = writing.text(part)?.context("worksheet missing")?;
        let doc = xml(text.as_bytes())?;
        let Some(node) = doc.descendants().find(|n| {
            n.has_tag_name((NS, "dataValidation")) && n.attribute("sqref") == Some(sqref.as_str())
        }) else {
            continue;
        };
        let opening = opening_range(&text, node)?;
        let mut out = text.clone();
        out.replace_range(
            opening.clone(),
            &set_xml_attribute(&text[opening], "sqref", &format!("{sqref} {added}"))?,
        );
        writing.set(part, out);
    }
    for (cell, text, author) in notes {
        set_note(writing, part, &cell, &text, &author)?;
    }
    Ok(())
}

fn clear_links(writing: &mut Writing<'_>, part: &str, area: Area) -> Result<()> {
    let text = writing.text(part)?.context("worksheet missing")?;
    let doc = xml(text.as_bytes())?;
    let Some(list) = child(doc.root_element(), "hyperlinks") else {
        return Ok(());
    };
    let links: Vec<Node<'_, '_>> = list
        .children()
        .filter(|n| n.has_tag_name((NS, "hyperlink")))
        .collect();
    let mut gone = vec![];
    let mut ids = vec![];
    for link in &links {
        if sqref_meets(link.attribute("ref").unwrap_or_default(), area)? {
            gone.push(link.range());
            if let Some(id) = link.attribute((REL, "id")) {
                ids.push(id.to_owned());
            }
        }
    }
    if gone.is_empty() {
        return Ok(());
    }
    let edits: Vec<_> = if gone.len() == links.len() {
        vec![(list.range(), String::new())]
    } else {
        gone.into_iter()
            .map(|range| (range, String::new()))
            .collect()
    };
    let updated = splice(&text, edits, "links")?;
    writing.set(part, updated);
    for id in ids {
        writing.unrelate(part, &id)?;
    }
    Ok(())
}

fn add_link(
    writing: &mut Writing<'_>,
    part: &str,
    range: &str,
    location: Option<&str>,
    url: Option<&str>,
    tooltip: Option<&str>,
) -> Result<()> {
    let mut attributes = format!(r#" ref="{}""#, xml_attr(range));
    if let Some(url) = url {
        let id = writing.relate(part, HYPERLINK, url, true)?;
        attributes.push_str(&format!(r#" xmlns:r="{REL}" r:id="{}""#, xml_attr(&id)));
    }
    if let Some(location) = location {
        attributes.push_str(&format!(r#" location="{}""#, xml_attr(location)));
    }
    if let Some(tooltip) = tooltip {
        attributes.push_str(&format!(r#" tooltip="{}""#, xml_attr(tooltip)));
    }
    let text = writing.text(part)?.context("worksheet missing")?;
    let prefix = prefix_of(&text)?;
    let element = format!("<{prefix}hyperlink{attributes}/>");
    let doc = xml(text.as_bytes())?;
    let updated = match child(doc.root_element(), "hyperlinks") {
        Some(list) if !text[list.range()].trim_end().ends_with("/>") => {
            let raw = &text[list.range()];
            let close = list.range().start + raw.rfind("</").context("invalid hyperlinks")?;
            let mut out = text.clone();
            out.insert_str(close, &element);
            out
        }
        Some(list) => {
            let mut out = text.clone();
            out.replace_range(
                list.range(),
                &format!("<{prefix}hyperlinks>{element}</{prefix}hyperlinks>"),
            );
            out
        }
        None => insert_member(
            &text,
            "hyperlinks",
            &format!("<{prefix}hyperlinks>{element}</{prefix}hyperlinks>"),
        )?,
    };
    writing.set(part, updated);
    Ok(())
}

/// Clears the cells of `area` from the rules named `name`
/// (`dataValidation`, `conditionalFormatting`) of the worksheet and its
/// extension list: a rule keeps the cells outside `area`, and goes when none
/// is left.
fn clear_rules(writing: &mut Writing<'_>, part: &str, area: Area, name: &str) -> Result<()> {
    let text = writing.text(part)?.context("worksheet missing")?;
    let doc = xml(text.as_bytes())?;
    let mut edits = vec![];
    let mut emptied: BTreeMap<(usize, usize), (usize, usize)> = BTreeMap::new();
    for node in doc
        .descendants()
        .filter(|n| n.has_tag_name((NS, name)) || n.has_tag_name((X14, name)))
    {
        let extended = node.has_tag_name((X14, name));
        let sqref_node = node.children().find(|n| n.has_tag_name((XM, "sqref")));
        let sqref = if extended {
            sqref_node.and_then(|n| n.text()).unwrap_or_default()
        } else {
            node.attribute("sqref").unwrap_or_default()
        };
        if !sqref_meets(sqref, area)? {
            continue;
        }
        // The list holding the rule, which goes with its last rule.
        let list = node.parent_element().context("rule without its list")?;
        let counted = emptied
            .entry((list.range().start, list.range().end))
            .or_insert_with(|| {
                (
                    list.children()
                        .filter(|n| n.is_element() && n.tag_name().name() == name)
                        .count(),
                    0,
                )
            });
        match sqref_without(sqref, area)? {
            Some(kept) if extended => {
                let sqref_node = sqref_node.context("rule without its cells")?;
                let inner = sqref_node
                    .children()
                    .find(Node::is_text)
                    .context("rule without its cells")?;
                edits.push((inner.range(), xml_attr(&kept)));
            }
            Some(kept) => {
                let opening = opening_range(&text, node)?;
                edits.push((
                    opening.clone(),
                    set_xml_attribute(&text[opening], "sqref", &kept)?,
                ));
            }
            None => {
                counted.1 += 1;
                edits.push((node.range(), String::new()));
            }
        }
    }
    // A list left without a rule goes; a conditional format is its own list.
    for (&(start, end), (all, gone)) in &emptied {
        if name == "conditionalFormatting" {
            continue;
        }
        if all == gone {
            edits.retain(|(range, _)| !(range.start >= start && range.end <= end));
            edits.push((start..end, String::new()));
        }
    }
    if edits.is_empty() {
        return Ok(());
    }
    edits.sort_by_key(|(range, _)| (range.start, range.end));
    let mut updated = splice(&text, edits, "rules")?;
    if name == "dataValidation" {
        updated = recount(&updated, "dataValidations", "dataValidation")?;
    }
    writing.set(part, updated);
    Ok(())
}

/// The worksheet `text` with the `count` of its list `list` set to the
/// number of its `member` elements.
fn recount(text: &str, list: &str, member: &str) -> Result<String> {
    let doc = xml(text.as_bytes())?;
    let Some(node) = child(doc.root_element(), list) else {
        return Ok(text.to_owned());
    };
    let count = node
        .children()
        .filter(|n| n.has_tag_name((NS, member)))
        .count();
    let opening = opening_range(text, node)?;
    let mut out = text.to_owned();
    out.replace_range(
        opening.clone(),
        &set_xml_attribute(&text[opening], "count", &count.to_string())?,
    );
    Ok(out)
}

fn add_validation(writing: &mut Writing<'_>, part: &str, range: &str, rule: &Value) -> Result<()> {
    let text = writing.text(part)?.context("worksheet missing")?;
    let prefix = prefix_of(&text)?;
    let mut attributes = format!(r#" type="{}""#, xml_attr(string(&rule["type"])?));
    for (key, attribute) in [("operator", "operator"), ("error_style", "errorStyle")] {
        if let Some(value) = rule[key].as_str() {
            attributes.push_str(&format!(r#" {attribute}="{}""#, xml_attr(value)));
        }
    }
    if rule["allow_blank"] != false {
        attributes.push_str(r#" allowBlank="1""#);
    }
    attributes.push_str(r#" showInputMessage="1" showErrorMessage="1""#);
    for (key, attribute) in [
        ("error_title", "errorTitle"),
        ("error", "error"),
        ("prompt_title", "promptTitle"),
        ("prompt", "prompt"),
    ] {
        if let Some(value) = rule[key].as_str() {
            attributes.push_str(&format!(r#" {attribute}="{}""#, xml_attr(value)));
        }
    }
    attributes.push_str(&format!(r#" sqref="{}""#, xml_attr(range)));
    let mut formulas = String::new();
    for name in ["formula1", "formula2"] {
        if let Some(formula) = rule[name].as_str() {
            formulas.push_str(&format!(
                "<{prefix}{name}>{}</{prefix}{name}>",
                xml_text(formula)
            ));
        }
    }
    let element =
        format!("<{prefix}dataValidation{attributes}>{formulas}</{prefix}dataValidation>");
    let doc = xml(text.as_bytes())?;
    let updated = match child(doc.root_element(), "dataValidations") {
        Some(list) if !text[list.range()].trim_end().ends_with("/>") => {
            let raw = &text[list.range()];
            let close = list.range().start + raw.rfind("</").context("invalid dataValidations")?;
            let mut out = text.clone();
            out.insert_str(close, &element);
            out
        }
        Some(list) => {
            let mut out = text.clone();
            out.replace_range(
                list.range(),
                &format!(
                    r#"<{prefix}dataValidations count="1">{element}</{prefix}dataValidations>"#
                ),
            );
            out
        }
        None => insert_member(
            &text,
            "dataValidations",
            &format!(r#"<{prefix}dataValidations count="1">{element}</{prefix}dataValidations>"#),
        )?,
    };
    writing.set(
        part,
        recount(&updated, "dataValidations", "dataValidation")?,
    );
    Ok(())
}

/// Text written as the content of an element.
fn xml_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn add_conditional_format(
    writing: &mut Writing<'_>,
    part: &str,
    range: &str,
    rule: &Value,
) -> Result<()> {
    let dxf = add_differential_format(writing, &rule["format"])?;
    let text = writing.text(part)?.context("worksheet missing")?;
    let prefix = prefix_of(&text)?;
    // A new rule goes ahead of the others, as Excel puts it.
    let doc = xml(text.as_bytes())?;
    let mut edits = vec![];
    for node in doc
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "cfRule")
    {
        if let Some(priority) = node
            .attribute("priority")
            .and_then(|p| p.parse::<u64>().ok())
        {
            let opening = opening_range(&text, node)?;
            edits.push((
                opening.clone(),
                set_xml_attribute(&text[opening], "priority", &(priority + 1).to_string())?,
            ));
        }
    }
    let shifted = splice(&text, edits, "priorities")?;
    let mut attributes = format!(
        r#" type="{}" dxfId="{dxf}" priority="1""#,
        xml_attr(string(&rule["type"])?)
    );
    if let Some(operator) = rule["operator"].as_str() {
        attributes.push_str(&format!(r#" operator="{}""#, xml_attr(operator)));
    }
    if rule["stop"] == true {
        attributes.push_str(r#" stopIfTrue="1""#);
    }
    let formulas: String = rule["formulas"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|formula| format!("<{prefix}formula>{}</{prefix}formula>", xml_text(formula)))
        .collect();
    let element = format!(
        r#"<{prefix}conditionalFormatting sqref="{}"><{prefix}cfRule{attributes}>{formulas}</{prefix}cfRule></{prefix}conditionalFormatting>"#,
        xml_attr(range)
    );
    writing.set(
        part,
        insert_member(&shifted, "conditionalFormatting", &element)?,
    );
    Ok(())
}

/// Adds the differential format `format` (fill, font color and emphasis,
/// number format) to the styles part, returning its index.
fn add_differential_format(writing: &mut Writing<'_>, format: &Value) -> Result<usize> {
    let styles = writing
        .text("xl/styles.xml")?
        .context("the workbook has no styles part, so ARP cannot add a conditional format")?;
    let doc = xml(styles.as_bytes())?;
    let root = doc.root_element();
    let prefix = element_tag(&styles[root.range()])?
        .strip_suffix("styleSheet")
        .context("invalid styleSheet tag")?
        .to_owned();
    let mut font = String::new();
    for (key, name) in [("bold", "b"), ("italic", "i"), ("strike", "strike")] {
        if format[key] == true {
            font.push_str(&format!("<{prefix}{name}/>"));
        }
    }
    if let Some(color) = format["font_color"].as_str() {
        font.push_str(&format!(r#"<{prefix}color rgb="FF{color}"/>"#));
    }
    let mut dxf = String::new();
    if !font.is_empty() {
        dxf.push_str(&format!("<{prefix}font>{font}</{prefix}font>"));
    }
    if let Some(code) = format["number_format"].as_str() {
        dxf.push_str(&format!(
            r#"<{prefix}numFmt numFmtId="{}" formatCode="{}"/>"#,
            200 + child(root, "dxfs")
                .map_or(0, |list| list.children().filter(Node::is_element).count()),
            xml_attr(code)
        ));
    }
    if let Some(color) = format["fill"].as_str() {
        dxf.push_str(&format!(
            r#"<{prefix}fill><{prefix}patternFill><{prefix}bgColor rgb="FF{color}"/></{prefix}patternFill></{prefix}fill>"#
        ));
    }
    let element = format!("<{prefix}dxf>{dxf}</{prefix}dxf>");
    let (updated, index) = match child(root, "dxfs") {
        Some(list) => {
            let count = list.children().filter(Node::is_element).count();
            let raw = &styles[list.range()];
            let (opening, _) = xml_opening(raw)?;
            let opening = set_xml_attribute(opening, "count", &(count + 1).to_string())?;
            let replaced = if raw.trim_end().ends_with("/>") {
                format!(
                    "{}>{element}</{prefix}dxfs>",
                    opening
                        .trim_end_matches('>')
                        .trim_end_matches('/')
                        .trim_end()
                )
            } else {
                let close = raw.rfind("</").context("invalid dxfs")?;
                format!(
                    "{opening}{}{element}{}",
                    &raw[xml_opening(raw)?.0.len()..close],
                    &raw[close..]
                )
            };
            let mut out = styles.clone();
            out.replace_range(list.range(), &replaced);
            (out, count)
        }
        None => {
            // dxfs follows these members of a styles part.
            const BEFORE: [&str; 7] = [
                "numFmts",
                "fonts",
                "fills",
                "borders",
                "cellStyleXfs",
                "cellXfs",
                "cellStyles",
            ];
            let after = root
                .children()
                .filter(Node::is_element)
                .rfind(|n| BEFORE.contains(&n.tag_name().name()))
                .context("styles part without its formats")?;
            let mut out = styles.clone();
            out.insert_str(
                after.range().end,
                &format!(r#"<{prefix}dxfs count="1">{element}</{prefix}dxfs>"#),
            );
            (out, 0)
        }
    };
    writing.set("xl/styles.xml", updated);
    Ok(index)
}

// ---------------------------------------------------------------- notes

/// The comments part and VML drawing of the sheet `part`, made when it has
/// none.
fn note_parts(
    writing: &mut Writing<'_>,
    part: &str,
    make: bool,
) -> Result<Option<(String, String)>> {
    let comments = writing.related(part, COMMENTS)?;
    let vml = writing.related(part, VML_DRAWING)?;
    match (comments, vml) {
        (Some((_, comments)), Some((_, vml))) => Ok(Some((comments, vml))),
        (None, _) if !make => Ok(None),
        (comments, vml) => {
            ensure!(make, "a note's parts are missing");
            let comments = match comments {
                Some((_, comments)) => comments,
                None => {
                    let name = next_part(|p| writing.exists(p), "xl", "comments");
                    writing.set(
                        &name,
                        format!(r#"<comments xmlns="{NS}"><authors><author>ARP</author></authors><commentList></commentList></comments>"#),
                    );
                    writing.content_type(
                        "xml",
                        "application/vnd.openxmlformats-officedocument.spreadsheetml.comments+xml",
                        Some(&format!("/{name}")),
                    )?;
                    writing.relate(part, COMMENTS, &relative_target(part, &name)?, false)?;
                    name
                }
            };
            let vml = match vml {
                Some((_, vml)) => vml,
                None => {
                    let name = next_vml_part(writing);
                    let block = next_idmap(writing)?;
                    writing.set(&name, new_vml(block));
                    writing.content_type(
                        "vml",
                        "application/vnd.openxmlformats-officedocument.vmlDrawing",
                        None,
                    )?;
                    let id =
                        writing.relate(part, VML_DRAWING, &relative_target(part, &name)?, false)?;
                    let text = writing.text(part)?.context("worksheet missing")?;
                    let prefix = prefix_of(&text)?;
                    let element = format!(
                        r#"<{prefix}legacyDrawing xmlns:r="{REL}" r:id="{}"/>"#,
                        xml_attr(&id)
                    );
                    writing.set(part, insert_member(&text, "legacyDrawing", &element)?);
                    name
                }
            };
            Ok(Some((comments, vml)))
        }
    }
}

fn next_vml_part(writing: &Writing<'_>) -> String {
    (1..)
        .map(|n| format!("xl/drawings/vmlDrawing{n}.vml"))
        .find(|name| !writing.exists(name))
        .unwrap_or_default()
}

/// A block of shape IDs (`o:idmap`) no VML drawing of the workbook uses.
fn next_idmap(writing: &Writing<'_>) -> Result<u32> {
    let mut used = 0;
    let parts: BTreeSet<String> = writing
        .book
        .parts
        .keys()
        .chain(writing.patched.keys())
        .filter(|name| name.ends_with(".vml"))
        .cloned()
        .collect();
    for name in parts {
        if let Some(text) = writing.text(&name)? {
            for data in text.split("data=\"").skip(1) {
                for block in data.split('"').next().unwrap_or_default().split(',') {
                    used = used.max(block.trim().parse::<u32>().unwrap_or(0));
                }
            }
        }
    }
    Ok(used + 1)
}

fn new_vml(block: u32) -> String {
    format!(
        r#"<xml xmlns:v="urn:schemas-microsoft-com:vml" xmlns:o="urn:schemas-microsoft-com:office:office" xmlns:x="urn:schemas-microsoft-com:office:excel"><o:shapelayout v:ext="edit"><o:idmap v:ext="edit" data="{block}"/></o:shapelayout><v:shapetype id="_x0000_t202" coordsize="21600,21600" o:spt="202" path="m,l,21600r21600,l21600,xe"><v:stroke joinstyle="miter"/><v:path gradientshapeok="t" o:connecttype="rect"/></v:shapetype></xml>"#
    )
}

/// The VML shapes of notes in `vml`, by their 0-based (row, column).
fn note_shapes(vml: &str) -> Vec<((u32, u32), std::ops::Range<usize>)> {
    let mut shapes = vec![];
    let mut at = 0;
    while let Some(start) = vml[at..].find("<v:shape ").map(|i| i + at) {
        let Some(end) = vml[start..]
            .find("</v:shape>")
            .map(|i| start + i + "</v:shape>".len())
        else {
            break;
        };
        let shape = &vml[start..end];
        let number = |tag: &str| -> Option<u32> {
            let open = format!("<x:{tag}>");
            let from = shape.find(&open)? + open.len();
            let to = from + shape[from..].find('<')?;
            shape[from..to].trim().parse().ok()
        };
        if shape.contains(r#"ObjectType="Note""#)
            && let (Some(row), Some(column)) = (number("Row"), number("Column"))
        {
            shapes.push(((row, column), start..end));
        }
        at = end;
    }
    shapes
}

fn set_note(
    writing: &mut Writing<'_>,
    part: &str,
    cell: &str,
    text: &str,
    author: &str,
) -> Result<()> {
    if let Some((_, threaded)) = writing.related(part, &format!("{MS_REL}/threadedComment"))?
        && let Some(threads) = writing.text(&threaded)?
        && threads.contains(&format!(r#"ref="{cell}""#))
    {
        bail!("{cell} holds a threaded comment; edit it in Excel");
    }
    let (comments_part, vml_part) = note_parts(writing, part, true)?.context("note parts")?;
    let comments = writing
        .text(&comments_part)?
        .context("comments part missing")?;
    let doc = xml(comments.as_bytes())?;
    let root = doc.root_element();
    let prefix = element_tag(&comments[root.range()])?
        .strip_suffix("comments")
        .context("invalid comments tag")?
        .to_owned();
    // The note's author, added to the authors when new.
    let authors: Vec<Node<'_, '_>> = child(root, "authors")
        .map(|list| {
            list.children()
                .filter(|n| n.has_tag_name((NS, "author")))
                .collect()
        })
        .unwrap_or_default();
    let name = if author.is_empty() { "ARP" } else { author };
    let mut edits = vec![];
    let author_id = match authors.iter().position(|a| a.text() == Some(name)) {
        Some(index) => index,
        None => {
            let list = child(root, "authors").context("comments without authors")?;
            let raw = &comments[list.range()];
            let element = format!("<{prefix}author>{}</{prefix}author>", xml_text(name));
            if raw.trim_end().ends_with("/>") {
                edits.push((
                    list.range(),
                    format!("<{prefix}authors>{element}</{prefix}authors>"),
                ));
            } else {
                let close = list.range().start + raw.rfind("</").context("invalid authors")?;
                edits.push((close..close, element));
            }
            authors.len()
        }
    };
    let body = format!(
        r#"<{prefix}text><{prefix}t xml:space="preserve">{}</{prefix}t></{prefix}text>"#,
        xml_text(text)
    );
    let list = child(root, "commentList").context("comments without their list")?;
    let existing = list
        .children()
        .find(|n| n.has_tag_name((NS, "comment")) && n.attribute("ref") == Some(cell));
    let element =
        format!(r#"<{prefix}comment ref="{cell}" authorId="{author_id}">{body}</{prefix}comment>"#);
    match existing {
        Some(node) => edits.push((node.range(), element)),
        None => {
            let raw = &comments[list.range()];
            if raw.trim_end().ends_with("/>") {
                edits.push((
                    list.range(),
                    format!("<{prefix}commentList>{element}</{prefix}commentList>"),
                ));
            } else {
                let close = list.range().start + raw.rfind("</").context("invalid commentList")?;
                edits.push((close..close, element));
            }
        }
    }
    edits.sort_by_key(|(range, _)| range.start);
    writing.set(&comments_part, splice(&comments, edits, "comments")?);
    // A new note gets its shape, hidden beside the cell as Excel puts it.
    let vml = writing.text(&vml_part)?.context("VML drawing missing")?;
    let (column, row) = coordinate(cell)?;
    if !note_shapes(&vml)
        .iter()
        .any(|(at, _)| *at == (row - 1, column - 1))
    {
        let id = next_shape_id(&vml);
        let shape = format!(
            r##"<v:shape id="_x0000_s{id}" type="#_x0000_t202" style="position:absolute;margin-left:0;margin-top:0;width:108pt;height:59.25pt;z-index:{id};visibility:hidden" fillcolor="#ffffe1" o:insetmode="auto"><v:fill color2="#ffffe1"/><v:shadow on="t" color="black" obscured="t"/><v:path o:connecttype="none"/><v:textbox style="mso-direction-alt:auto"><div style="text-align:left"></div></v:textbox><x:ClientData ObjectType="Note"><x:MoveWithCells/><x:SizeWithCells/><x:Anchor>{}, 15, {}, 10, {}, 15, {}, 4</x:Anchor><x:AutoFill>False</x:AutoFill><x:Row>{}</x:Row><x:Column>{}</x:Column></x:ClientData></v:shape>"##,
            column,
            row.saturating_sub(2),
            column + 2,
            row + 2,
            row - 1,
            column - 1
        );
        let close = vml.rfind("</xml>").context("invalid VML drawing")?;
        let mut out = vml.clone();
        out.insert_str(close, &shape);
        writing.set(&vml_part, out);
    }
    Ok(())
}

/// A shape number the VML drawing `vml` does not use, in its ID block.
fn next_shape_id(vml: &str) -> u32 {
    let block: u32 = vml
        .split("data=\"")
        .nth(1)
        .and_then(|data| data.split(['"', ',']).next())
        .and_then(|data| data.trim().parse().ok())
        .unwrap_or(1);
    let used = vml
        .split("_x0000_s")
        .skip(1)
        .filter_map(|rest| {
            rest.split(|c: char| !c.is_ascii_digit())
                .next()
                .and_then(|n| n.parse::<u32>().ok())
        })
        .max();
    used.map_or(block * 1024 + 1, |used| used + 1)
}

fn clear_notes(writing: &mut Writing<'_>, part: &str, area: Area) -> Result<()> {
    let Some((comments_part, vml_part)) = note_parts(writing, part, false)? else {
        return Ok(());
    };
    let comments = writing
        .text(&comments_part)?
        .context("comments part missing")?;
    let doc = xml(comments.as_bytes())?;
    let mut gone = BTreeSet::new();
    let mut edits = vec![];
    for node in doc
        .descendants()
        .filter(|n| n.has_tag_name((NS, "comment")))
    {
        let cell = node.attribute("ref").unwrap_or_default();
        if sqref_meets(cell, area)? {
            let (column, row) = coordinate(cell)?;
            gone.insert((row - 1, column - 1));
            edits.push((node.range(), String::new()));
        }
    }
    if edits.is_empty() {
        return Ok(());
    }
    writing.set(&comments_part, splice(&comments, edits, "comments")?);
    let vml = writing.text(&vml_part)?.context("VML drawing missing")?;
    let edits: Vec<_> = note_shapes(&vml)
        .into_iter()
        .filter(|(at, _)| gone.contains(at))
        .map(|(_, range)| (range, String::new()))
        .collect();
    writing.set(&vml_part, splice(&vml, edits, "VML drawing")?);
    Ok(())
}

// ---------------------------------------------------------------- checking

/// Each operation is in the workbook `read` back as written: a link, note,
/// validation or conditional format over its range, or none of the cleared
/// ones left there.
fn ensure_written(read: &Workbook, written: &[(&CellObjectOperation, String)]) -> Result<()> {
    // Only the last operation of a range shows: an earlier one is replaced.
    for (index, (operation, range)) in written.iter().enumerate() {
        let sheet = read
            .sheets
            .iter()
            .find(|sheet| sheet["name"] == operation.sheet.as_str())
            .context("written sheet missing")?;
        // A later operation on the same things over some of the cells
        // decides what they read back.
        let replaced = written[index + 1..].iter().any(|(later, later_range)| {
            later.sheet == operation.sheet
                && same_family(&later.kind, &operation.kind)
                && later_range.split_whitespace().any(|part| {
                    Area::parse(part).is_ok_and(|area| sqref_meets(range, area).unwrap_or(true))
                })
        });
        if replaced {
            continue;
        }
        let cells = |list: &str, key: &str| -> Vec<String> {
            sheet[list]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|entry| entry[key].as_str().map(str::to_owned))
                .collect()
        };
        let covers = |list: &str, key: &str| -> Result<bool> {
            for part in range.split_whitespace() {
                let area = Area::parse(part)?;
                if !cells(list, key)
                    .iter()
                    .any(|sqref| sqref_meets(sqref, area).unwrap_or(false))
                {
                    return Ok(false);
                }
            }
            Ok(true)
        };
        let clear = |list: &str, key: &str| -> Result<bool> { Ok(!covers(list, key)?) };
        let fine = match &operation.kind {
            CellObjectKind::Link { .. } => covers("links", "range")?,
            CellObjectKind::ClearLinks => clear("links", "range")?,
            CellObjectKind::Note { text, .. } => sheet["comments"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|note| {
                    note["text"] == text.as_str()
                        && range.starts_with(note["address"].as_str().unwrap_or("?"))
                }),
            CellObjectKind::ClearNotes => clear("comments", "address")?,
            CellObjectKind::Validation(_) => covers("validations", "range")?,
            CellObjectKind::ClearValidations => clear("validations", "range")?,
            CellObjectKind::ConditionalFormat(_) => covers("conditional_formats", "range")?,
            CellObjectKind::ClearConditionalFormats => clear("conditional_formats", "range")?,
            CellObjectKind::Copy { .. } => true,
        };
        ensure!(
            fine,
            "{} of {} did not read back as written over {range}",
            operation.id,
            operation.sheet
        );
    }
    Ok(())
}

/// Whether two kinds set or clear the same things.
fn same_family(a: &CellObjectKind, b: &CellObjectKind) -> bool {
    use CellObjectKind::*;
    let family = |kind: &CellObjectKind| match kind {
        Link { .. } | ClearLinks => 0,
        Note { .. } | ClearNotes => 1,
        Validation(_) | ClearValidations => 2,
        ConditionalFormat(_) | ClearConditionalFormats => 3,
        Copy { .. } => 4,
    };
    family(a) == family(b)
}
