//! PowerPoint slide operations: a slide inserted as a copy of another, as
//! PowerPoint's Duplicate Slide makes it, a new slide made from a layout, as
//! New Slide makes it, a slide deleted together with the
//! parts only it used, a slide moved in the show order, and a slide hidden
//! from or shown in the slide show. Slides keep the names the extraction gave them
//! (`slide-N` by original position); an inserted slide is named by its
//! operation ID and its notes page `notes-<ID>`.
use super::*;
use crate::excel::xml_attr;
use std::ops::Range;

const SLIDE_KINDS: &[&str] = &[
    "insert_slide",
    "add_slide",
    "delete_slide",
    "move_slide",
    "set_slide_visibility",
];
const P14: &str = "http://schemas.microsoft.com/office/powerpoint/2010/main";
const EXTENDED_PROPERTIES: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/extended-properties";
const CONTENT_TYPES: &str = "http://schemas.openxmlformats.org/package/2006/content-types";
const MODERN_COMMENTS: &str = "http://schemas.microsoft.com/office/2018/10/relationships/comments";
/// Relationships a copied slide shares with its original: layouts, masters,
/// media, links and other slides. Every other internal part the slide uses
/// (charts, diagrams, embedded objects, notes, tags) is its own and is copied.
const SHARED: &[&str] = &[
    "slideLayout",
    "slideMaster",
    "notesMaster",
    "theme",
    "image",
    "audio",
    "video",
    "hyperlink",
    "slide",
    "http://schemas.microsoft.com/office/2007/relationships/media",
    "http://schemas.microsoft.com/office/2007/relationships/hdphoto",
];

/// Whether `value` is a slide operation (`insert_slide`, `add_slide`,
/// `delete_slide`, `move_slide` or `set_slide_visibility`).
pub fn is_slide_operation(value: &Value) -> bool {
    value["kind"]
        .as_str()
        .is_some_and(|kind| SLIDE_KINDS.contains(&kind))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SlidePosition {
    After(String),
    Before(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SlideOperation {
    /// A copy of slide `from`, and of its notes page, placed next to a slide.
    Insert {
        id: String,
        from: String,
        position: SlidePosition,
    },
    /// A new slide with the placeholders of the layout in part `layout`.
    Add {
        id: String,
        layout: String,
        position: SlidePosition,
    },
    Delete {
        id: String,
        slide: String,
    },
    /// Slide `slide` placed next to another slide.
    Move {
        id: String,
        slide: String,
        position: SlidePosition,
    },
    /// Slide `slide` hidden from the slide show, or shown again.
    Visibility {
        id: String,
        slide: String,
        hidden: bool,
    },
}

/// The slides in show order as the operations leave them.
#[derive(Clone)]
struct Deck {
    /// Each slide's name and the name of its notes page, if it has one.
    slides: Vec<(String, Option<String>)>,
    /// Every page name given out, so an inserted slide takes a new one.
    names: BTreeSet<String>,
    /// The original page each inserted slide or notes page copies.
    origins: BTreeMap<String, String>,
    /// Whether a slide an operation hid or showed is hidden.
    hidden: BTreeMap<String, bool>,
    /// Slides made from a layout.
    added: BTreeSet<String>,
}

impl Deck {
    fn new(sheets: &[Value]) -> Result<Self> {
        let names = sheets
            .iter()
            .map(|sheet| string(&sheet["name"]).map(str::to_owned))
            .collect::<Result<BTreeSet<_>>>()?;
        let mut slides = vec![];
        for sheet in sheets {
            let name = string(&sheet["name"])?;
            if let Some(number) = name.strip_prefix("slide-") {
                let notes = format!("notes-{number}");
                let notes = names.contains(&notes).then_some(notes);
                slides.push((name.to_owned(), notes));
            }
        }
        Ok(Self {
            slides,
            names,
            origins: BTreeMap::new(),
            hidden: BTreeMap::new(),
            added: BTreeSet::new(),
        })
    }

    fn index(&self, name: &str) -> Result<usize> {
        self.slides
            .iter()
            .position(|(slide, _)| slide == name)
            .with_context(|| {
                format!(
                    "{name} is not a slide here: name a slide of the original (slide-N) or one an earlier insert_slide added (its operation ID), not deleted before"
                )
            })
    }

    fn origin(&self, name: &str) -> String {
        self.origins
            .get(name)
            .cloned()
            .unwrap_or_else(|| name.to_owned())
    }

    fn apply(&mut self, operation: &SlideOperation) -> Result<()> {
        match operation {
            SlideOperation::Insert { id, from, position } => {
                let source = self.index(from)?;
                let notes = format!("notes-{id}");
                ensure!(
                    !id.bytes().all(|b| b.is_ascii_digit())
                        && !self.names.contains(id)
                        && !self.names.contains(&notes),
                    "operation ID {id} would name the new slide {id} and its notes {notes}, but a page already has that name or it is a number; choose another ID"
                );
                let (anchor, after) = match position {
                    SlidePosition::After(anchor) => (anchor, true),
                    SlidePosition::Before(anchor) => (anchor, false),
                };
                let at = self.index(anchor)? + usize::from(after);
                let source_notes = self.slides[source].1.clone();
                self.origins.insert(id.clone(), self.origin(from));
                self.names.insert(id.clone());
                let notes = source_notes.map(|source_notes| {
                    self.origins
                        .insert(notes.clone(), self.origin(&source_notes));
                    self.names.insert(notes.clone());
                    notes
                });
                self.slides.insert(at, (id.clone(), notes));
            }
            SlideOperation::Add { id, position, .. } => {
                ensure!(
                    !id.bytes().all(|b| b.is_ascii_digit()) && !self.names.contains(id),
                    "operation ID {id} would name the new slide {id}, but a page already has that name or it is a number; choose another ID"
                );
                let (anchor, after) = match position {
                    SlidePosition::After(anchor) => (anchor, true),
                    SlidePosition::Before(anchor) => (anchor, false),
                };
                let at = self.index(anchor)? + usize::from(after);
                self.names.insert(id.clone());
                self.added.insert(id.clone());
                self.slides.insert(at, (id.clone(), None));
            }
            SlideOperation::Delete { id, slide } => {
                if let Some(origin) = self.origins.get(slide) {
                    bail!(
                        "{slide} is a copy of {origin} added by operation {slide}; remove that operation instead of deleting the slide"
                    );
                }
                ensure!(
                    !self.added.contains(slide),
                    "{slide} is a new slide added by operation {slide}; remove that operation instead of deleting the slide"
                );
                let index = self.index(slide)?;
                ensure!(
                    self.slides.len() > 1,
                    "operation {id} would delete the last slide; a presentation keeps at least one"
                );
                self.slides.remove(index);
            }
            SlideOperation::Move {
                slide, position, ..
            } => {
                let (anchor, after) = match position {
                    SlidePosition::After(anchor) => (anchor, true),
                    SlidePosition::Before(anchor) => (anchor, false),
                };
                ensure!(
                    slide != anchor,
                    "{slide} cannot be placed next to itself; name another slide"
                );
                let moved = self.slides.remove(self.index(slide)?);
                let at = self.index(anchor)? + usize::from(after);
                self.slides.insert(at, moved);
            }
            SlideOperation::Visibility { slide, hidden, .. } => {
                self.index(slide)?;
                self.hidden.insert(slide.clone(), *hidden);
            }
        }
        Ok(())
    }
}

/// The slide operations among `values`, checked in order against the pages
/// of the extraction (`sheets`).
pub fn parse_slide_operations(values: &[Value], sheets: &[Value]) -> Result<Vec<SlideOperation>> {
    let mut ids = BTreeSet::new();
    for value in values {
        if let Some(id) = value["id"].as_str() {
            ensure!(ids.insert(id), "duplicate operation ID {id}");
        }
    }
    let mut deck = Deck::new(sheets)?;
    let mut operations = vec![];
    for value in values.iter().filter(|value| is_slide_operation(value)) {
        let id = string(&value["id"])?;
        identifier(id)?;
        ensure!(
            !string(&value["reason"])?.trim().is_empty(),
            "operation reason required"
        );
        let position = || match (value["after"].as_str(), value["before"].as_str()) {
            (Some(after), None) => Ok(SlidePosition::After(after.to_owned())),
            (None, Some(before)) => Ok(SlidePosition::Before(before.to_owned())),
            _ => bail!("{} {id} takes one of after and before", value["kind"]),
        };
        let operation = match string(&value["kind"])? {
            "insert_slide" => SlideOperation::Insert {
                id: id.to_owned(),
                from: string(&value["from"])?.to_owned(),
                position: position()?,
            },
            "add_slide" => SlideOperation::Add {
                id: id.to_owned(),
                layout: string(&value["layout"])?.to_owned(),
                position: position()?,
            },
            "move_slide" => SlideOperation::Move {
                id: id.to_owned(),
                slide: string(&value["slide"])?.to_owned(),
                position: position()?,
            },
            "set_slide_visibility" => SlideOperation::Visibility {
                id: id.to_owned(),
                slide: string(&value["slide"])?.to_owned(),
                hidden: value["hidden"]
                    .as_bool()
                    .context("set_slide_visibility takes hidden: true or false")?,
            },
            _ => SlideOperation::Delete {
                id: id.to_owned(),
                slide: string(&value["slide"])?.to_owned(),
            },
        };
        deck.apply(&operation)
            .with_context(|| format!("slide operation {id}"))?;
        operations.push(operation);
    }
    Ok(operations)
}

/// The pages as the operations leave them, each with its content page ID
/// (`page`): deleted slides and their notes pages are left out, and each
/// inserted slide and notes page follows as a copy of the original page it
/// copies (`copy_of`), named and keyed by its own name (`sheet-<name>`). A
/// slide made from a layout follows with the text of the layout's empty
/// placeholders (`from_layout`), as the extraction's `layouts` list them.
pub fn slide_view(
    sheets: &[Value],
    layouts: &[Value],
    operations: &[SlideOperation],
) -> Result<Vec<Value>> {
    let mut deck = Deck::new(sheets)?;
    for operation in operations {
        deck.apply(operation)?;
    }
    let kept: BTreeSet<&str> = deck
        .slides
        .iter()
        .flat_map(|(slide, notes)| std::iter::once(slide).chain(notes))
        .map(String::as_str)
        .collect();
    let mut view = vec![];
    let state = |sheet: &mut Value| {
        if let Some(hidden) = sheet["name"]
            .as_str()
            .and_then(|name| deck.hidden.get(name))
        {
            sheet["state"] = json!(if *hidden { "hidden" } else { "visible" });
        }
    };
    for (index, sheet) in sheets.iter().enumerate() {
        if kept.contains(string(&sheet["name"])?) {
            let mut sheet = sheet.clone();
            sheet["page"] = json!(format!("sheet-{}", index + 1));
            state(&mut sheet);
            view.push(sheet);
        }
    }
    for operation in operations {
        if let SlideOperation::Add { id, layout, .. } = operation
            && deck.added.contains(id)
        {
            let source = layouts
                .iter()
                .find(|l| l["part"] == layout.as_str())
                .with_context(|| format!("{layout} is not a slide layout of the presentation"))?;
            let mut sheet = json!({"name":id,"part":layout,"state":"visible","merges":[],"cells":source["cells"],"tables":[]});
            sheet["page"] = json!(format!("sheet-{id}"));
            sheet["from_layout"] = json!(layout);
            state(&mut sheet);
            view.push(sheet);
            continue;
        }
        let SlideOperation::Insert { id, .. } = operation else {
            continue;
        };
        for name in [id.clone(), format!("notes-{id}")] {
            let Some(origin) = deck.origins.get(&name) else {
                continue;
            };
            let mut sheet = sheets
                .iter()
                .find(|sheet| sheet["name"] == origin.as_str())
                .context("copied page missing")?
                .clone();
            sheet["name"] = json!(name);
            sheet["page"] = json!(format!("sheet-{name}"));
            sheet["copy_of"] = json!(origin);
            state(&mut sheet);
            view.push(sheet);
        }
    }
    Ok(view)
}

/// The slides in show order as the operations leave them, each with the
/// name of its notes page.
pub fn slide_order(
    sheets: &[Value],
    operations: &[SlideOperation],
) -> Result<Vec<(String, Option<String>)>> {
    let mut deck = Deck::new(sheets)?;
    for operation in operations {
        deck.apply(operation)?;
    }
    Ok(deck.slides)
}

/// The original page each inserted slide or notes page copies.
pub fn origins(
    operations: &[SlideOperation],
    sheets: &[Value],
) -> Result<BTreeMap<String, String>> {
    let mut deck = Deck::new(sheets)?;
    for operation in operations {
        deck.apply(operation)?;
    }
    Ok(deck.origins)
}

/// A presentation package after slide operations.
pub struct Restructured {
    /// Parts added or rewritten.
    pub patched: BTreeMap<String, Vec<u8>>,
    /// Original parts left out.
    pub removed: BTreeSet<String>,
    /// The part of each inserted slide and notes page, by page name.
    pub inserted: BTreeMap<String, String>,
    /// The slides in show order, each with its notes page.
    pub order: Vec<(String, Option<String>)>,
    /// Whether each slide of `order` is hidden from the slide show.
    pub hidden: Vec<bool>,
}

impl Restructured {
    /// The bytes of `part` as the operations leave it.
    pub fn part<'a>(
        &'a self,
        parts: &'a BTreeMap<String, Vec<u8>>,
        part: &str,
    ) -> Option<&'a [u8]> {
        match self.patched.get(part) {
            Some(bytes) => Some(bytes),
            None if self.removed.contains(part) => None,
            None => parts.get(part).map(Vec::as_slice),
        }
    }
}

/// Applies the slide `operations` to the presentation `parts`, whose pages
/// the extraction lists as `sheets`.
pub fn restructure(
    parts: &BTreeMap<String, Vec<u8>>,
    sheets: &[Value],
    operations: &[SlideOperation],
) -> Result<Restructured> {
    let mut package = Package {
        base: parts,
        changed: BTreeMap::new(),
        taken: parts.keys().map(|name| name.to_lowercase()).collect(),
        override_types: None,
        override_updates: BTreeMap::new(),
    };
    let presentation = package.main_part()?;
    let mut deck = Deck::new(sheets)?;
    // The part of each page, and the page of each part for messages.
    let mut pages = BTreeMap::new();
    for sheet in sheets {
        pages.insert(
            string(&sheet["name"])?.to_owned(),
            string(&sheet["part"])?.to_owned(),
        );
    }
    let mut inserted = BTreeMap::new();
    for operation in operations {
        match operation {
            SlideOperation::Insert { id, from, position } => {
                let (anchor, after) = match position {
                    SlidePosition::After(anchor) => (anchor, true),
                    SlidePosition::Before(anchor) => (anchor, false),
                };
                let source = pages[from].clone();
                let (slide, notes) = package.duplicate_slide(&source)?;
                package.add_slide(&presentation, &slide, &pages[anchor], after)?;
                pages.insert(id.clone(), slide.clone());
                inserted.insert(id.clone(), slide);
                if let Some(notes) = notes {
                    pages.insert(format!("notes-{id}"), notes.clone());
                    inserted.insert(format!("notes-{id}"), notes);
                }
            }
            SlideOperation::Add {
                id,
                layout,
                position,
            } => {
                let (anchor, after) = match position {
                    SlidePosition::After(anchor) => (anchor, true),
                    SlidePosition::Before(anchor) => (anchor, false),
                };
                let slide = package
                    .new_slide(layout)
                    .with_context(|| format!("a slide cannot be made from {layout}"))?;
                package.add_slide(&presentation, &slide, &pages[anchor], after)?;
                pages.insert(id.clone(), slide.clone());
                inserted.insert(id.clone(), slide);
            }
            SlideOperation::Delete { slide, .. } => {
                let names: BTreeMap<&str, &str> = pages
                    .iter()
                    .map(|(name, part)| (part.as_str(), name.as_str()))
                    .collect();
                package
                    .delete_slide(&presentation, &pages[slide], &names)
                    .with_context(|| format!("{slide} cannot be deleted"))?;
            }
            SlideOperation::Move {
                slide, position, ..
            } => {
                let (anchor, after) = match position {
                    SlidePosition::After(anchor) => (anchor, true),
                    SlidePosition::Before(anchor) => (anchor, false),
                };
                package
                    .move_slide(&presentation, &pages[slide], &pages[anchor], after)
                    .with_context(|| format!("{slide} cannot be moved"))?;
            }
            SlideOperation::Visibility { slide, hidden, .. } => {
                package.set_visibility(&pages[slide], *hidden)?;
            }
        }
        deck.apply(operation)?;
    }
    // Whether each slide is hidden: as an operation set it, else as the
    // original (or the slide a copy was made of) is.
    let mut hidden = vec![];
    for (slide, _) in &deck.slides {
        hidden.push(match deck.hidden.get(slide) {
            Some(hidden) => *hidden,
            None if deck.added.contains(slide) => false,
            None => {
                let origin = deck.origin(slide);
                sheets
                    .iter()
                    .find(|sheet| sheet["name"] == origin.as_str())
                    .is_some_and(|sheet| sheet["state"] == "hidden")
            }
        });
    }
    if !operations.is_empty() {
        package.update_counts(&deck)?;
    }
    package.flush_overrides()?;
    let mut patched = BTreeMap::new();
    let mut removed = BTreeSet::new();
    for (part, bytes) in package.changed {
        match bytes {
            Some(bytes) => {
                patched.insert(part, bytes);
            }
            None if parts.contains_key(&part) => {
                removed.insert(part);
            }
            None => {}
        }
    }
    Ok(Restructured {
        patched,
        removed,
        inserted,
        order: deck.slides,
        hidden,
    })
}

/// A slide made from a layout (`p:sldLayout`) as PowerPoint's New Slide makes
/// it: an empty shape for each placeholder of the layout but the date, footer
/// and slide number, which PowerPoint leaves off new slides.
pub(super) fn slide_from_layout(layout: &str) -> Result<String> {
    let xml = Document::parse(layout)?;
    ensure!(
        xml.root_element().has_tag_name((PRESENTATION, "sldLayout")),
        "not a slide layout"
    );
    let mut shapes = String::new();
    let mut next = 2;
    for shape in xml
        .descendants()
        .filter(|n| n.has_tag_name((PRESENTATION, "sp")))
    {
        let Some(properties) = shape
            .children()
            .find(|n| n.has_tag_name((PRESENTATION, "nvSpPr")))
        else {
            continue;
        };
        let Some(placeholder) = properties
            .children()
            .find(|n| n.has_tag_name((PRESENTATION, "nvPr")))
            .and_then(|n| n.children().find(|n| n.has_tag_name((PRESENTATION, "ph"))))
        else {
            continue;
        };
        if matches!(
            placeholder.attribute("type"),
            Some("dt" | "ftr" | "sldNum" | "hdr")
        ) {
            continue;
        }
        let name = properties
            .children()
            .find(|n| n.has_tag_name((PRESENTATION, "cNvPr")))
            .and_then(|n| n.attribute("name"))
            .unwrap_or("Placeholder");
        let attributes: String = ["type", "orient", "sz", "idx"]
            .iter()
            .filter_map(|key| {
                placeholder
                    .attribute(*key)
                    .map(|value| format!(r#" {key}="{}""#, xml_attr(value)))
            })
            .collect();
        shapes.push_str(&format!(
            r#"<p:sp><p:nvSpPr><p:cNvPr id="{next}" name="{}"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr><p:nvPr><p:ph{attributes}/></p:nvPr></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:endParaRPr lang="ja-JP"/></a:p></p:txBody></p:sp>"#,
            xml_attr(name)
        ));
        next += 1;
    }
    Ok(format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><p:sld xmlns:a="{DRAWING}" xmlns:r="{REL}" xmlns:p="{PRESENTATION}"><p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/><a:chOff x="0" y="0"/><a:chExt cx="0" cy="0"/></a:xfrm></p:grpSpPr>{shapes}</p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sld>"#
    ))
}

/// The slide layouts of a presentation (`parts`) a new slide can be made
/// from: each with its name, its placeholders and the text of the slide made
/// from it (`cells`, with their fonts), in part order.
pub(super) fn layouts(parts: &BTreeMap<String, Vec<u8>>) -> Result<Vec<Value>> {
    let mut layouts = vec![];
    for (part, bytes) in parts {
        if !(part.starts_with("ppt/slideLayouts/")
            && part.ends_with(".xml")
            && !part.contains("/_rels/"))
        {
            continue;
        }
        let (text, _) = part_text(bytes)?;
        let xml = Document::parse(&text)?;
        if !xml.root_element().has_tag_name((PRESENTATION, "sldLayout")) {
            continue;
        }
        let name = xml
            .root_element()
            .children()
            .find(|n| n.has_tag_name((PRESENTATION, "cSld")))
            .and_then(|n| n.attribute("name"))
            .unwrap_or("");
        let placeholders: Vec<Value> = xml
            .descendants()
            .filter(|n| n.has_tag_name((PRESENTATION, "ph")))
            .map(|ph| {
                let shape_name = ph
                    .ancestors()
                    .find(|n| n.tag_name().name().starts_with("nv"))
                    .and_then(|n| n.children().find(|c| c.tag_name().name() == "cNvPr"))
                    .and_then(|n| n.attribute("name"));
                json!({"type":ph.attribute("type").unwrap_or("obj"),"index":ph.attribute("idx"),"name":shape_name})
            })
            .collect();
        let slide = slide_from_layout(&text)?;
        let slide_xml = Document::parse(&slide)?;
        let fonts = super::slide_fonts::SlideFonts::for_layout(parts, part, &slide_xml)?;
        let sheet = super::slide_text::layout(&slide_xml)?.sheet("layout", part, &fonts);
        layouts.push(
            json!({"name":name,"part":part,"placeholders":placeholders,"cells":sheet["cells"]}),
        );
    }
    Ok(layouts)
}

/// The relationships part of `part`; `""` is the package itself.
fn relationships_part(part: &str) -> String {
    match part.rsplit_once('/') {
        Some((directory, file)) => format!("{directory}/_rels/{file}.rels"),
        None => format!("_rels/{part}.rels"),
    }
}

struct Relationship {
    id: String,
    kind: String,
    target: String,
    external: bool,
    /// The element, and its Target attribute, in the relationships part.
    range: Range<usize>,
    target_range: Range<usize>,
}

fn relationships(text: &str) -> Result<Vec<Relationship>> {
    let xml = Document::parse(text)?;
    let mut result = vec![];
    for node in xml
        .descendants()
        .filter(|n| n.has_tag_name((PACKAGE_REL, "Relationship")))
    {
        let target = node
            .attributes()
            .find(|a| a.name() == "Target" && a.namespace().is_none())
            .context("relationship target missing")?;
        result.push(Relationship {
            id: node
                .attribute("Id")
                .context("relationship ID missing")?
                .to_owned(),
            kind: node.attribute("Type").unwrap_or("").to_owned(),
            target: target.value().to_owned(),
            external: node.attribute("TargetMode") == Some("External"),
            range: node.range(),
            target_range: target.range(),
        });
    }
    Ok(result)
}

/// Whether a relationship type is one of `SHARED`, named after the
/// relationships namespace or in full.
fn shared(kind: &str) -> bool {
    SHARED.iter().any(|shared| {
        kind == *shared
            || kind
                .strip_prefix(REL)
                .and_then(|rest| rest.strip_prefix('/'))
                == Some(shared)
    })
}

fn relationship_kind(kind: &str, name: &str) -> bool {
    kind.strip_prefix(REL)
        .and_then(|rest| rest.strip_prefix('/'))
        == Some(name)
}

/// The qualified name of the element whose markup starts `raw`.
fn element_name(raw: &str) -> &str {
    raw.trim_start_matches('<')
        .split(|c: char| c.is_whitespace() || c == '/' || c == '>')
        .next()
        .unwrap_or("")
}

/// An edit adding `child` as the last child of `element`.
fn append_child(text: &str, element: Node<'_, '_>, child: &str) -> (Range<usize>, String) {
    let range = element.range();
    let raw = &text[range.clone()];
    if raw.ends_with("/>") && element.first_child().is_none() {
        let name = element_name(raw);
        let opening = raw[..raw.len() - 2].trim_end();
        (range, format!("{opening}>{child}</{name}>"))
    } else {
        let end = range.start + raw.rfind("</").unwrap_or(raw.len());
        (end..end, child.to_owned())
    }
}

/// The package as the operations change it, over the original parts.
struct Package<'a> {
    base: &'a BTreeMap<String, Vec<u8>>,
    /// New bytes of a part, or None for a part removed.
    changed: BTreeMap<String, Option<Vec<u8>>>,
    /// Case-insensitive names already in the package or assigned to a copy.
    taken: BTreeSet<String>,
    /// Content type overrides by case-insensitive part name, loaded on first use.
    override_types: Option<BTreeMap<String, String>>,
    /// Part names to add or remove from [Content_Types].xml after all operations.
    override_updates: BTreeMap<String, Option<(String, String)>>,
}

impl Package<'_> {
    fn get(&self, part: &str) -> Option<&[u8]> {
        match self.changed.get(part) {
            Some(bytes) => bytes.as_deref(),
            None => self.base.get(part).map(Vec::as_slice),
        }
    }

    fn exists(&self, part: &str) -> bool {
        self.get(part).is_some()
    }

    fn text(&self, part: &str) -> Result<(String, PartEncoding)> {
        let bytes = self
            .get(part)
            .with_context(|| format!("missing Office part: {part}"))?;
        let (text, encoding) = part_text(bytes)?;
        Ok((text.into_owned(), encoding))
    }

    fn edit(&mut self, part: &str, edits: Vec<(Range<usize>, String)>) -> Result<()> {
        if edits.is_empty() {
            return Ok(());
        }
        let (text, encoding) = self.text(part)?;
        let result = splice(&text, edits, part)?;
        Document::parse(&result).with_context(|| format!("rewritten {part}"))?;
        self.changed
            .insert(part.to_owned(), Some(encode_part(&result, encoding)));
        Ok(())
    }

    fn remove(&mut self, part: &str) {
        self.changed.insert(part.to_owned(), None);
        self.taken.remove(&part.to_lowercase());
    }

    fn relationships(&self, part: &str) -> Result<Vec<Relationship>> {
        let rels = relationships_part(part);
        if !self.exists(&rels) {
            return Ok(vec![]);
        }
        relationships(&self.text(&rels)?.0)
    }

    /// The part an internal relationship of `part` names, if it is in the package.
    fn target(&self, part: &str, relationship: &Relationship) -> Option<String> {
        if relationship.external {
            return None;
        }
        resolve_target(part, &relationship.target, |name| self.exists(name))
            .ok()
            .filter(|target| self.exists(target))
    }

    fn main_part(&self) -> Result<String> {
        let root = self
            .relationships("")?
            .into_iter()
            .find(|r| relationship_kind(&r.kind, "officeDocument"))
            .context("Office document relationship missing")?;
        self.target("", &root).context("presentation part missing")
    }

    /// Every part the package reaches through internal relationships.
    fn reachable(&self) -> Result<BTreeSet<String>> {
        self.reachable_except(None)
    }

    /// Like [`Self::reachable`], without following the relationships of `except`.
    fn reachable_except(&self, except: Option<&str>) -> Result<BTreeSet<String>> {
        let mut reached = BTreeSet::new();
        let mut queue = vec![String::new()];
        while let Some(part) = queue.pop() {
            if Some(part.as_str()) == except {
                continue;
            }
            for relationship in self.relationships(&part)? {
                if let Some(target) = self.target(&part, &relationship)
                    && reached.insert(target.clone())
                {
                    queue.push(target);
                }
            }
        }
        Ok(reached)
    }

    /// A part name next to `part` that no part has: its file name with a new
    /// number, in letters, digits, `_` and `-` so that a target needs no escaping.
    fn fresh_name(&mut self, part: &str) -> String {
        let (directory, file) = part.rsplit_once('/').unwrap_or(("", part));
        let (stem, extension) = file.rsplit_once('.').unwrap_or((file, ""));
        let stem: String = stem
            .trim_end_matches(|c: char| c.is_ascii_digit())
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
            .collect();
        let stem = if stem.is_empty() {
            "part".to_owned()
        } else {
            stem
        };
        let name = (1..)
            .map(|n| {
                let file = if extension.is_empty() {
                    format!("{stem}{n}")
                } else {
                    format!("{stem}{n}.{extension}")
                };
                if directory.is_empty() {
                    file
                } else {
                    format!("{directory}/{file}")
                }
            })
            .find(|name| !self.taken.contains(&name.to_lowercase()))
            .unwrap();
        self.taken.insert(name.to_lowercase());
        name
    }

    /// The content type [Content_Types].xml overrides for `part`, if any.
    fn override_type(&mut self, part: &str) -> Result<Option<String>> {
        if self.override_types.is_none() {
            let mut types = BTreeMap::new();
            if self.exists("[Content_Types].xml") {
                let (text, _) = self.text("[Content_Types].xml")?;
                let xml = Document::parse(&text)?;
                for node in xml
                    .descendants()
                    .filter(|n| n.has_tag_name((CONTENT_TYPES, "Override")))
                {
                    if let (Some(name), Some(kind)) =
                        (node.attribute("PartName"), node.attribute("ContentType"))
                    {
                        types
                            .entry(name.to_lowercase())
                            .or_insert_with(|| kind.to_owned());
                    }
                }
            }
            self.override_types = Some(types);
        }
        Ok(self
            .override_types
            .as_ref()
            .unwrap()
            .get(&format!("/{part}").to_lowercase())
            .cloned())
    }

    fn add_override(&mut self, part: &str, content_type: &str) {
        let name = format!("/{part}");
        self.override_types
            .as_mut()
            .unwrap()
            .insert(name.to_lowercase(), content_type.to_owned());
        self.override_updates
            .insert(name.to_lowercase(), Some((name, content_type.to_owned())));
    }

    fn remove_overrides(&mut self, parts: &BTreeSet<String>) -> Result<()> {
        if !self.exists("[Content_Types].xml") {
            return Ok(());
        }
        self.override_type("")?;
        for part in parts {
            let name = format!("/{part}").to_lowercase();
            self.override_types.as_mut().unwrap().remove(&name);
            self.override_updates.insert(name, None);
        }
        Ok(())
    }

    fn flush_overrides(&mut self) -> Result<()> {
        if self.override_updates.is_empty() {
            return Ok(());
        }
        let (text, _) = self.text("[Content_Types].xml")?;
        let xml = Document::parse(&text)?;
        let mut edits: Vec<_> = xml
            .descendants()
            .filter(|n| n.has_tag_name((CONTENT_TYPES, "Override")))
            .filter(|n| {
                n.attribute("PartName")
                    .is_some_and(|name| self.override_updates.contains_key(&name.to_lowercase()))
            })
            .map(|n| (n.range(), String::new()))
            .collect();
        let added: String = self
            .override_updates
            .values()
            .filter_map(Option::as_ref)
            .map(|(name, kind)| {
                format!(
                    r#"<Override PartName="{}" ContentType="{}"/>"#,
                    xml_attr(name),
                    xml_attr(kind)
                )
            })
            .collect();
        if !added.is_empty() {
            edits.push(append_child(&text, xml.root_element(), &added));
        }
        self.edit("[Content_Types].xml", edits)
    }

    /// Copies `part` and the parts it owns under new names, once per part of
    /// `copies`; returns the copy's name. Relationships to a part already
    /// copied (a notes page's link back to its slide) point at the copy;
    /// comments stay with the original.
    fn copy_part(&mut self, part: &str, copies: &mut BTreeMap<String, String>) -> Result<String> {
        if let Some(copy) = copies.get(part) {
            return Ok(copy.clone());
        }
        let copy = self.fresh_name(part);
        copies.insert(part.to_owned(), copy.clone());
        let bytes = self.get(part).context("copied part missing")?.to_vec();
        self.changed.insert(copy.clone(), Some(bytes));
        if let Some(content_type) = self.override_type(part)? {
            self.add_override(&copy, &content_type);
        }
        let rels = relationships_part(part);
        if !self.exists(&rels) {
            return Ok(copy);
        }
        let mut edits = vec![];
        for relationship in self.relationships(part)? {
            if relationship_kind(&relationship.kind, "comments")
                || relationship.kind == MODERN_COMMENTS
            {
                edits.push((relationship.range.clone(), String::new()));
                continue;
            }
            let Some(target) = self.target(part, &relationship) else {
                continue;
            };
            let target_copy = match copies.get(&target) {
                Some(copy) => copy.clone(),
                None if shared(&relationship.kind) => continue,
                None => self.copy_part(&target, copies)?,
            };
            // The copy sits next to the original, so only the file name changes.
            let file = target_copy.rsplit('/').next().unwrap_or(&target_copy);
            let new_target = match relationship.target.rsplit_once('/') {
                Some((directory, _)) => format!("{directory}/{file}"),
                None => file.to_owned(),
            };
            edits.push((
                relationship.target_range.clone(),
                format!(r#"Target="{}""#, xml_attr(&new_target)),
            ));
        }
        let (text, encoding) = self.text(&rels)?;
        let copied_rels = relationships_part(&copy);
        let result = splice(&text, edits, &rels)?;
        Document::parse(&result)?;
        self.taken.insert(copied_rels.to_lowercase());
        self.changed
            .insert(copied_rels, Some(encode_part(&result, encoding)));
        Ok(copy)
    }

    /// Copies a slide with its notes page; returns the new slide and notes parts.
    fn duplicate_slide(&mut self, slide: &str) -> Result<(String, Option<String>)> {
        let notes = self
            .relationships(slide)?
            .iter()
            .find(|r| relationship_kind(&r.kind, "notesSlide"))
            .and_then(|r| self.target(slide, r));
        let mut copies = BTreeMap::new();
        let copy = self.copy_part(slide, &mut copies)?;
        Ok((copy, notes.map(|notes| copies[&notes].clone())))
    }

    /// Lists `slide` in the presentation next to the slide in part `anchor`.
    fn add_slide(
        &mut self,
        presentation: &str,
        slide: &str,
        anchor: &str,
        after: bool,
    ) -> Result<()> {
        let rels = relationships_part(presentation);
        let relationships = self.relationships(presentation)?;
        let anchor_id = relationships
            .iter()
            .find(|r| {
                relationship_kind(&r.kind, "slide")
                    && self.target(presentation, r).as_deref() == Some(anchor)
            })
            .map(|r| r.id.clone())
            .context("the anchor slide is not listed in the presentation")?;
        let ids: BTreeSet<&str> = relationships.iter().map(|r| r.id.as_str()).collect();
        let id = (1..)
            .map(|n| format!("rId{n}"))
            .find(|id| !ids.contains(id.as_str()))
            .unwrap();
        let directory = presentation.rsplit_once('/').map_or("", |(d, _)| d);
        let target = match slide.strip_prefix(&format!("{directory}/")) {
            Some(relative) if !directory.is_empty() => relative.to_owned(),
            _ => format!("/{slide}"),
        };
        let (text, _) = self.text(&rels)?;
        let xml = Document::parse(&text)?;
        let edit = append_child(
            &text,
            xml.root_element(),
            &format!(
                r#"<Relationship Id="{id}" Type="{REL}/slide" Target="{}"/>"#,
                xml_attr(&target)
            ),
        );
        self.edit(&rels, vec![edit])?;

        let (text, _) = self.text(presentation)?;
        let xml = Document::parse(&text)?;
        let slide_ids: Vec<Node> = xml
            .descendants()
            .filter(|n| n.has_tag_name((PRESENTATION, "sldId")))
            .collect();
        let anchor_node = slide_ids
            .iter()
            .find(|n| n.attribute((REL, "id")) == Some(anchor_id.as_str()))
            .context("the anchor slide is not listed in the presentation")?;
        let next = slide_ids
            .iter()
            .filter_map(|n| n.attribute("id")?.parse::<u64>().ok())
            .max()
            .unwrap_or(255)
            + 1;
        ensure!(next < 2_147_483_648, "no slide ID is left for a new slide");
        let raw = &text[anchor_node.range()];
        let name = element_name(raw);
        let relationship_attribute = anchor_node
            .attributes()
            .find(|a| a.namespace() == Some(REL) && a.name() == "id")
            .map(|a| {
                text[a.range()]
                    .split('=')
                    .next()
                    .unwrap_or("r:id")
                    .trim()
                    .to_owned()
            })
            .context("slide relationship missing")?;
        let element = format!(r#"<{name} id="{next}" {relationship_attribute}="{id}"/>"#);
        let at = if after {
            anchor_node.range().end
        } else {
            anchor_node.range().start
        };
        let mut edits = vec![(at..at, element)];
        // A new slide joins the section of the slide it is placed next to.
        let anchor_number = anchor_node.attribute("id").unwrap_or("");
        if let Some(section_slide) = xml.descendants().find(|n| {
            n.has_tag_name((P14, "sldId"))
                && n.attribute("id") == Some(anchor_number)
                && n.ancestors().any(|a| a.has_tag_name((P14, "section")))
        }) {
            let name = element_name(&text[section_slide.range()]);
            let at = if after {
                section_slide.range().end
            } else {
                section_slide.range().start
            };
            edits.push((at..at, format!(r#"<{name} id="{next}"/>"#)));
        }
        self.edit(presentation, edits)
    }

    /// A new slide part made from the layout in part `layout`, with its
    /// relationship to the layout and its content type.
    fn new_slide(&mut self, layout: &str) -> Result<String> {
        ensure!(
            layout.starts_with("ppt/slideLayouts/") && self.exists(layout),
            "{layout} is not a slide layout of the presentation"
        );
        let (text, _) = self.text(layout)?;
        let slide = slide_from_layout(&text)?;
        let part = self.fresh_name("ppt/slides/slide1.xml");
        self.changed.insert(part.clone(), Some(slide.into_bytes()));
        let target = format!(
            "../slideLayouts/{}",
            layout.rsplit('/').next().unwrap_or(layout)
        );
        self.changed.insert(
            relationships_part(&part),
            Some(
                format!(
                    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="{PACKAGE_REL}"><Relationship Id="rId1" Type="{REL}/slideLayout" Target="{}"/></Relationships>"#,
                    xml_attr(&target)
                )
                .into_bytes(),
            ),
        );
        self.override_type("")?;
        self.add_override(
            &part,
            "application/vnd.openxmlformats-officedocument.presentationml.slide+xml",
        );
        Ok(part)
    }

    /// Moves the slide in `part` next to the slide in `anchor` in the show
    /// order, and in a sectioned presentation into the anchor's section.
    fn move_slide(
        &mut self,
        presentation: &str,
        part: &str,
        anchor: &str,
        after: bool,
    ) -> Result<()> {
        let relationships = self.relationships(presentation)?;
        let relationship = |target: &str| {
            relationships
                .iter()
                .find(|r| {
                    relationship_kind(&r.kind, "slide")
                        && self.target(presentation, r).as_deref() == Some(target)
                })
                .map(|r| r.id.clone())
                .context("the slide is not listed in the presentation")
        };
        let (moved_id, anchor_id) = (relationship(part)?, relationship(anchor)?);
        let (text, _) = self.text(presentation)?;
        let xml = Document::parse(&text)?;
        let listed = |id: &str| {
            xml.descendants()
                .find(|n| {
                    n.has_tag_name((PRESENTATION, "sldId")) && n.attribute((REL, "id")) == Some(id)
                })
                .context("the slide is not listed in the presentation")
        };
        let (moved, anchor_node) = (listed(&moved_id)?, listed(&anchor_id)?);
        let place = |node: Node<'_, '_>| {
            if after {
                node.range().end
            } else {
                node.range().start
            }
        };
        // The new place is inserted before the old one is removed, so that a
        // slide placed where it already is stays there.
        let mut edits = vec![
            (
                place(anchor_node)..place(anchor_node),
                text[moved.range()].to_owned(),
            ),
            (moved.range(), String::new()),
        ];
        let section = |number: Option<&str>| {
            xml.descendants().find(|n| {
                n.has_tag_name((P14, "sldId"))
                    && n.attribute("id") == number
                    && n.ancestors().any(|a| a.has_tag_name((P14, "section")))
            })
        };
        if let (Some(entry), Some(target)) = (
            section(moved.attribute("id")),
            section(anchor_node.attribute("id")),
        ) {
            edits.push((place(target)..place(target), text[entry.range()].to_owned()));
            edits.push((entry.range(), String::new()));
        }
        edits.sort_by_key(|(range, replacement)| (range.start, replacement.is_empty()));
        self.edit(presentation, edits)
    }

    /// Hides the slide in `part` from the slide show (`show="0"`), or shows it.
    fn set_visibility(&mut self, part: &str, hidden: bool) -> Result<()> {
        let (text, _) = self.text(part)?;
        let xml = Document::parse(&text)?;
        let root = xml.root_element();
        ensure!(
            root.has_tag_name((PRESENTATION, "sld")),
            "{part} is not a slide"
        );
        let raw = &text[root.range()];
        let end = raw.find('>').context("invalid slide")?;
        let opening = raw[..end].trim_end_matches('/');
        let pattern = regex::Regex::new(r#"\s+show\s*=\s*(?:"[^"]*"|'[^']*')"#)?;
        let mut replaced = pattern.replace(opening, "").into_owned();
        if hidden {
            replaced.push_str(r#" show="0""#);
        }
        let start = root.range().start;
        self.edit(part, vec![(start..start + opening.len(), replaced)])
    }

    /// Removes the slide in `part` from the presentation and the parts only it
    /// used. Refused while another part links to the slide or a custom show
    /// would be left empty. `names` gives the page name of each part.
    fn delete_slide(
        &mut self,
        presentation: &str,
        part: &str,
        names: &BTreeMap<&str, &str>,
    ) -> Result<()> {
        let before = self.reachable()?;
        let relationships = self.relationships(presentation)?;
        let ids: BTreeSet<String> = relationships
            .iter()
            .filter(|r| self.target(presentation, r).as_deref() == Some(part))
            .map(|r| r.id.clone())
            .collect();
        let (text, _) = self.text(presentation)?;
        let xml = Document::parse(&text)?;
        let mut edits = vec![];
        let mut numbers = BTreeSet::new();
        for node in xml.descendants().filter(|n| {
            n.has_tag_name((PRESENTATION, "sldId"))
                && n.attribute((REL, "id")).is_some_and(|id| ids.contains(id))
        }) {
            numbers.insert(node.attribute("id").unwrap_or("").to_owned());
            edits.push((node.range(), String::new()));
        }
        for show in xml
            .descendants()
            .filter(|n| n.has_tag_name((PRESENTATION, "custShow")))
        {
            let slides: Vec<Node> = show
                .descendants()
                .filter(|n| n.has_tag_name((PRESENTATION, "sld")))
                .collect();
            let listed: Vec<&Node> = slides
                .iter()
                .filter(|n| n.attribute((REL, "id")).is_some_and(|id| ids.contains(id)))
                .collect();
            ensure!(
                listed.is_empty() || listed.len() < slides.len(),
                "the custom show {} would have no slides left; delete the custom show in PowerPoint first",
                show.attribute("name").unwrap_or("")
            );
            edits.extend(listed.iter().map(|n| (n.range(), String::new())));
        }
        edits.extend(
            xml.descendants()
                .filter(|n| {
                    n.has_tag_name((P14, "sldId"))
                        && n.attribute("id").is_some_and(|id| numbers.contains(id))
                })
                .map(|n| (n.range(), String::new())),
        );
        self.edit(presentation, edits)?;
        let edits = relationships
            .iter()
            .filter(|r| ids.contains(&r.id))
            .map(|r| (r.range.clone(), String::new()))
            .collect();
        self.edit(&relationships_part(presentation), edits)?;
        // The outline view lists slides through its own relationships.
        if let Some(view) = relationships
            .iter()
            .find(|r| relationship_kind(&r.kind, "viewProps"))
            .and_then(|r| self.target(presentation, r))
        {
            let view_relationships = self.relationships(&view)?;
            let ids: BTreeSet<&str> = view_relationships
                .iter()
                .filter(|r| self.target(&view, r).as_deref() == Some(part))
                .map(|r| r.id.as_str())
                .collect();
            if !ids.is_empty() {
                let (text, _) = self.text(&view)?;
                let xml = Document::parse(&text)?;
                let edits = xml
                    .descendants()
                    .filter(|n| n.attribute((REL, "id")).is_some_and(|id| ids.contains(id)))
                    .map(|n| (n.range(), String::new()))
                    .collect();
                self.edit(&view, edits)?;
                let edits = view_relationships
                    .iter()
                    .filter(|r| ids.contains(r.id.as_str()))
                    .map(|r| (r.range.clone(), String::new()))
                    .collect();
                self.edit(&relationships_part(&view), edits)?;
            }
        }
        let after = self.reachable()?;
        if after.contains(part) {
            // Parts reached only through the slide (its notes page) do not count.
            let mut linking = vec![];
            for source in &self.reachable_except(Some(part))? {
                if source != part
                    && self
                        .relationships(source)?
                        .iter()
                        .any(|r| self.target(source, r).as_deref() == Some(part))
                {
                    linking.push(
                        names
                            .get(source.as_str())
                            .copied()
                            .unwrap_or(source)
                            .to_owned(),
                    );
                }
            }
            bail!(
                "it is linked from {}; remove those links in PowerPoint first",
                linking.join(", ")
            );
        }
        let gone: BTreeSet<String> = before.difference(&after).cloned().collect();
        for part in &gone {
            self.remove(part);
            let rels = relationships_part(part);
            if self.exists(&rels) {
                self.remove(&rels);
            }
        }
        self.remove_overrides(&gone)
    }

    /// Sets the slide and notes counts of the extended properties, if listed.
    fn update_counts(&mut self, deck: &Deck) -> Result<()> {
        let Some(properties) = self
            .relationships("")?
            .iter()
            .find(|r| relationship_kind(&r.kind, "extended-properties"))
            .and_then(|r| self.target("", r))
        else {
            return Ok(());
        };
        let (text, _) = self.text(&properties)?;
        let xml = Document::parse(&text)?;
        let counts = [
            ("Slides", deck.slides.len()),
            (
                "Notes",
                deck.slides
                    .iter()
                    .filter(|(_, notes)| notes.is_some())
                    .count(),
            ),
        ];
        let mut edits = vec![];
        for (name, count) in counts {
            if let Some(node) = xml
                .root_element()
                .children()
                .find(|n| n.has_tag_name((EXTENDED_PROPERTIES, name)))
            {
                let raw = &text[node.range()];
                let element = element_name(raw);
                edits.push((node.range(), format!("<{element}>{count}</{element}>")));
            }
        }
        self.edit(&properties, edits)
    }
}
